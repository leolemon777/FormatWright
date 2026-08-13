#![forbid(unsafe_code)]

mod queue_bridge;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use formatwright_core::{
    CapabilitySnapshot, ConversionPreset, DoctorReport, EngineDiscoveryPolicy, ExecutionMilestone,
    JobExecutionService, JobRecord, JobState, PRESET_SCHEMA_VERSION, Plan, PlanRequest,
    PresetLibrary, Probe, QueueRunReport, QueueWindowControl, SqliteJobStore, ValidationReport,
    ValidationStatus, VerifiedEnginePack, activate_engine_pack, capability_snapshot_for_input,
    execute_plan_observed, prepare_conversion,
};
use queue_bridge::{DEFAULT_BATCH_JOBS, DEFAULT_BENCHMARK_JOBS, QueueBatchIter};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct DesktopState {
    store: Mutex<SqliteJobStore>,
    job_database_path: PathBuf,
    cancellations: Mutex<HashMap<Uuid, CancellationToken>>,
    queue_control: Mutex<Option<QueueWindowControl>>,
    presets: Mutex<PresetLibrary>,
    presets_path: PathBuf,
    reports_directory: PathBuf,
    engine_registry_directory: PathBuf,
    engine_store_directory: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopPresetRequest {
    preset_id: Option<Uuid>,
    name: String,
    target_format: String,
    quality: Option<u8>,
    width: Option<u32>,
    dpi: Option<u16>,
    color_mode: Option<String>,
    preserve_all_streams: Option<bool>,
}

impl DesktopPresetRequest {
    fn into_preset(self) -> ConversionPreset {
        ConversionPreset {
            schema_version: PRESET_SCHEMA_VERSION,
            preset_id: self.preset_id.unwrap_or_else(Uuid::new_v4),
            name: self.name,
            target_format: self.target_format,
            quality: self.quality,
            width: self.width,
            dpi: self.dpi,
            color_mode: self.color_mode,
            preserve_all_streams: self.preserve_all_streams.unwrap_or(true),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct DesktopPresetImportResult {
    imported: usize,
    total: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopConversionRequest {
    input_path: PathBuf,
    output_path: PathBuf,
    target_format: String,
    quality: Option<u8>,
    width: Option<u32>,
    dpi: Option<u16>,
    color_mode: Option<String>,
    preserve_all_streams: Option<bool>,
    approved_plan_hash: Option<String>,
}

impl DesktopConversionRequest {
    fn plan_request(&self) -> PlanRequest {
        PlanRequest {
            target_format: self.target_format.clone(),
            output_path: Some(self.output_path.clone()),
            preserve_all_streams: self.preserve_all_streams.unwrap_or(true),
            quality: self.quality,
            width: self.width,
            dpi: self.dpi,
            color_mode: self.color_mode.clone(),
            ..PlanRequest::default()
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct DesktopPreview {
    probe: Probe,
    plan: Plan,
}

#[derive(Clone, Debug, Serialize)]
struct DesktopRunResult {
    job: JobRecord,
    report: ValidationReport,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DesktopEngineRegistryEntry {
    #[serde(default)]
    engine_id: Option<String>,
    manifest_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
struct DesktopEngineBundle {
    schema_version: u32,
    bundle_id: String,
    packs: Vec<PathBuf>,
}

#[derive(Clone, Debug, Serialize)]
struct DesktopEnginePackSummary {
    manifest_path: PathBuf,
    engine_id: Option<String>,
    version: Option<String>,
    manifest_sha256: Option<String>,
    executable_names: Vec<String>,
    signature_present: bool,
    valid: bool,
    message: String,
}

#[derive(Clone, Debug, Serialize)]
struct QueueBridgeBenchmark {
    total_jobs: u32,
    emitted_batches: u32,
    maximum_batch_jobs: u32,
    elapsed_milliseconds: u128,
}

#[tauri::command]
async fn run_queue_bridge_benchmark(
    window: tauri::WebviewWindow,
    job_count: Option<u32>,
) -> Result<QueueBridgeBenchmark, String> {
    let total_jobs = job_count.unwrap_or(DEFAULT_BENCHMARK_JOBS);
    let batches = QueueBatchIter::new(total_jobs, DEFAULT_BATCH_JOBS).map_err(str::to_owned)?;
    let started = Instant::now();
    let mut emitted_batches = 0_u32;
    let mut maximum_batch_jobs = 0_u32;
    for batch in batches {
        maximum_batch_jobs = maximum_batch_jobs.max(
            u32::try_from(batch.jobs.len()).map_err(|_| "batch length exceeds u32".to_owned())?,
        );
        window
            .emit("formatwright://queue-delta", &batch)
            .map_err(|error| format!("unable to emit queue batch: {error}"))?;
        emitted_batches = emitted_batches.saturating_add(1);
        tokio::task::yield_now().await;
    }
    Ok(QueueBridgeBenchmark {
        total_jobs,
        emitted_batches,
        maximum_batch_jobs,
        elapsed_milliseconds: started.elapsed().as_millis(),
    })
}

#[tauri::command]
async fn desktop_doctor() -> DoctorReport {
    formatwright_core::doctor().await
}

#[tauri::command]
async fn desktop_capability_snapshot(input_path: PathBuf) -> CapabilitySnapshot {
    capability_snapshot_for_input(&input_path, EngineDiscoveryPolicy::for_current_build()).await
}

#[tauri::command]
async fn import_desktop_engine_pack(
    state: tauri::State<'_, DesktopState>,
    manifest_path: PathBuf,
) -> Result<DesktopEnginePackSummary, String> {
    let engine_store_directory = state.engine_store_directory.clone();
    let verified = tokio::task::spawn_blocking(move || {
        formatwright_core::install_engine_pack(manifest_path, engine_store_directory)
    })
    .await
    .map_err(|error| format!("engine-pack verification worker failed: {error}"))?
    .map_err(serialize_error)?;
    persist_engine_registry_entry(&state.engine_registry_directory, &verified)?;
    Ok(valid_engine_summary(&verified))
}

#[tauri::command]
async fn list_imported_engine_packs(
    state: tauri::State<'_, DesktopState>,
) -> Result<Vec<DesktopEnginePackSummary>, String> {
    let paths = registered_manifest_paths(&state.engine_registry_directory)?;
    let mut summaries = Vec::with_capacity(paths.len());
    for path in paths {
        let display_path = path.clone();
        let result = tokio::task::spawn_blocking(move || activate_engine_pack(path))
            .await
            .map_err(|error| format!("engine-pack verification worker failed: {error}"))?;
        summaries.push(match result {
            Ok(verified) => valid_engine_summary(&verified),
            Err(error) => DesktopEnginePackSummary {
                manifest_path: display_path,
                engine_id: None,
                version: None,
                manifest_sha256: None,
                executable_names: Vec::new(),
                signature_present: false,
                valid: false,
                message: error.message,
            },
        });
    }
    Ok(summaries)
}

#[tauri::command]
async fn preview_conversion(request: DesktopConversionRequest) -> Result<DesktopPreview, String> {
    let (probe, plan, _) = prepare_conversion(&request.input_path, &request.plan_request())
        .await
        .map_err(serialize_error)?;
    Ok(DesktopPreview { probe, plan })
}

async fn prepare_approved_desktop_conversion(
    request: &DesktopConversionRequest,
) -> Result<(Probe, Plan, formatwright_core::EngineIdentity), String> {
    let prepared = prepare_conversion(&request.input_path, &request.plan_request())
        .await
        .map_err(serialize_error)?;
    formatwright_core::ensure_plan_approved(&prepared.1, request.approved_plan_hash.as_deref())
        .map_err(serialize_error)?;
    Ok(prepared)
}

#[tauri::command]
async fn run_desktop_conversion(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    request: DesktopConversionRequest,
) -> Result<DesktopRunResult, String> {
    let (probe, plan, validation_engine) = prepare_approved_desktop_conversion(&request).await?;
    let job = {
        let mut guard = lock(&state.store)?;
        guard
            .create_job(&request.input_path, &request.output_path, &plan)
            .and_then(|job| guard.transition(job.id, JobState::Running, "DESKTOP_ENGINE_STARTED"))
            .map_err(serialize_error)?
    };
    let cancellation = CancellationToken::new();
    lock(&state.cancellations)?.insert(job.id, cancellation.clone());
    let _ = window.emit("formatwright://job-updated", &job);
    let state_for_observer = &state;
    let execution = execute_plan_observed(
        &probe,
        &plan,
        &validation_engine,
        job.id,
        cancellation,
        move |milestone| {
            if milestone == ExecutionMilestone::EngineFinished {
                let mut guard = lock_core_store(&state_for_observer.store)?;
                guard.transition(job.id, JobState::Validating, "DESKTOP_VALIDATION_STARTED")?;
            }
            Ok(())
        },
    )
    .await;
    lock(&state.cancellations)?.remove(&job.id);
    match execution {
        Ok(result) => {
            let job = {
                let mut guard = lock(&state.store)?;
                persist_report_before_terminal(
                    &mut guard,
                    &state.reports_directory,
                    job.id,
                    &result.report,
                )?
            };
            let _ = window.emit("formatwright://job-updated", &job);
            Ok(DesktopRunResult {
                job,
                report: result.report,
            })
        }
        Err(error) => {
            let final_state = if error.code == formatwright_core::ErrorCode::Cancelled {
                JobState::Cancelled
            } else {
                JobState::Failed
            };
            if let Ok(mut store) = state.store.lock() {
                let _ = store.transition(job.id, final_state, "DESKTOP_CONVERSION_FAILED");
            }
            Err(serialize_error(error))
        }
    }
}

#[tauri::command]
async fn queue_desktop_conversion(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    request: DesktopConversionRequest,
) -> Result<JobRecord, String> {
    let (probe, plan, _) = prepare_approved_desktop_conversion(&request).await?;
    let job = {
        let mut store = lock(&state.store)?;
        store
            .create_job(&probe.artifact.canonical_path, &request.output_path, &plan)
            .and_then(|job| store.transition(job.id, JobState::Queued, "JOB_ENQUEUED"))
            .map_err(serialize_error)?
    };
    let _ = window.emit("formatwright://job-updated", &job);
    Ok(job)
}

async fn run_queue_window_on_database<F>(
    database_path: &Path,
    limit: usize,
    parallel: usize,
    control: QueueWindowControl,
    on_report: F,
) -> formatwright_core::Result<QueueRunReport>
where
    F: FnMut(Uuid, &ValidationReport) -> formatwright_core::Result<()>,
{
    let mut queue_store = SqliteJobStore::open(database_path)?;
    JobExecutionService::run_window_observed(&mut queue_store, limit, parallel, control, on_report)
        .await
}

#[tauri::command]
async fn run_desktop_queue_window(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    limit: Option<usize>,
    parallel: Option<usize>,
) -> Result<QueueRunReport, String> {
    let limit = limit.unwrap_or(100).clamp(1, 256);
    let parallel = parallel.unwrap_or(4).clamp(1, 16);
    let (control, _lease) = acquire_queue_window(&state.queue_control)?;
    let reports_directory = state.reports_directory.clone();
    let database_path = state.job_database_path.clone();
    let report = run_queue_window_on_database(
        &database_path,
        limit,
        parallel,
        control,
        |job_id, validation| {
            save_report(&reports_directory, job_id, validation).map_err(|message| {
                formatwright_core::FormatWrightError::new(
                    formatwright_core::ErrorCode::StorageFailed,
                    formatwright_core::Stage::Validate,
                    "Unable to persist ValidationReport for a queued job",
                    "Check the application reports directory and retry the queue window.",
                )
                .with_diagnostic(message)
            })
        },
    )
    .await;
    let report = report.map_err(serialize_error)?;
    let _ = window.emit("formatwright://queue-window-finished", &report);
    Ok(report)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum DesktopQueuePauseMode {
    FinishCurrent,
    Immediate,
}

struct DesktopQueueWindowLease<'a> {
    slot: &'a Mutex<Option<QueueWindowControl>>,
}

impl Drop for DesktopQueueWindowLease<'_> {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.slot.lock() {
            *slot = None;
        }
    }
}

fn acquire_queue_window(
    slot: &Mutex<Option<QueueWindowControl>>,
) -> Result<(QueueWindowControl, DesktopQueueWindowLease<'_>), String> {
    let control = QueueWindowControl::new();
    let mut current = lock(slot)?;
    if current.is_some() {
        return Err("a durable queue window is already running".to_owned());
    }
    *current = Some(control.clone());
    drop(current);
    Ok((control, DesktopQueueWindowLease { slot }))
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn pause_desktop_queue_window(
    state: tauri::State<'_, DesktopState>,
    mode: DesktopQueuePauseMode,
) -> Result<bool, String> {
    let queue_control = lock(&state.queue_control)?;
    let Some(control) = queue_control.as_ref() else {
        return Ok(false);
    };
    match mode {
        DesktopQueuePauseMode::FinishCurrent => control.pause_finish_current(),
        DesktopQueuePauseMode::Immediate => control.pause_immediate(),
    }
    Ok(true)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn cancel_desktop_queue_window(state: tauri::State<'_, DesktopState>) -> Result<bool, String> {
    pause_desktop_queue_window(state, DesktopQueuePauseMode::Immediate)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn cancel_desktop_job(
    state: tauri::State<'_, DesktopState>,
    job_id: String,
) -> Result<bool, String> {
    let job_id = Uuid::parse_str(&job_id).map_err(|error| error.to_string())?;
    let cancellations = lock(&state.cancellations)?;
    if let Some(token) = cancellations.get(&job_id) {
        token.cancel();
        Ok(true)
    } else {
        Ok(false)
    }
}

fn requeue_job(store: &mut SqliteJobStore, job_id: Uuid) -> formatwright_core::Result<JobRecord> {
    let job = store.get_job(job_id)?.ok_or_else(|| {
        formatwright_core::FormatWrightError::new(
            formatwright_core::ErrorCode::StorageFailed,
            formatwright_core::Stage::Store,
            format!("Job does not exist: {job_id}"),
            "Refresh the job list.",
        )
    })?;
    let code = match job.state {
        JobState::Interrupted | JobState::Blocked => "DESKTOP_JOB_RESUMED",
        JobState::Failed | JobState::Cancelled => "DESKTOP_JOB_RETRIED",
        state => {
            return Err(formatwright_core::FormatWrightError::new(
                formatwright_core::ErrorCode::PolicyBlocked,
                formatwright_core::Stage::Store,
                format!("Job in state {state:?} cannot be queued again"),
                "Only interrupted, blocked, failed, or cancelled jobs can be resumed or retried.",
            ));
        }
    };
    store.transition(job_id, JobState::Queued, code)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn requeue_desktop_job(
    state: tauri::State<'_, DesktopState>,
    job_id: String,
) -> Result<JobRecord, String> {
    let job_id = Uuid::parse_str(&job_id).map_err(|error| error.to_string())?;
    let mut store = lock(&state.store)?;
    requeue_job(&mut store, job_id).map_err(serialize_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn list_desktop_jobs(
    state: tauri::State<'_, DesktopState>,
    limit: Option<usize>,
) -> Result<Vec<JobRecord>, String> {
    lock(&state.store)?
        .list_jobs(limit.unwrap_or(100).clamp(1, 500))
        .map_err(serialize_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_desktop_report(
    state: tauri::State<'_, DesktopState>,
    job_id: String,
) -> Result<Option<ValidationReport>, String> {
    let job_id = Uuid::parse_str(&job_id).map_err(|error| error.to_string())?;
    read_report(&state.reports_directory, job_id)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn list_desktop_presets(
    state: tauri::State<'_, DesktopState>,
) -> Result<Vec<ConversionPreset>, String> {
    Ok(lock(&state.presets)?.presets.clone())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn save_desktop_preset(
    state: tauri::State<'_, DesktopState>,
    request: DesktopPresetRequest,
) -> Result<ConversionPreset, String> {
    let preset = request.into_preset();
    let preset_id = preset.preset_id;
    let mut current = lock(&state.presets)?;
    let mut updated = current.clone();
    updated.upsert(preset).map_err(serialize_error)?;
    let saved = updated
        .presets
        .iter()
        .find(|candidate| candidate.preset_id == preset_id)
        .cloned()
        .ok_or_else(|| "saved preset was not found in the updated library".to_owned())?;
    persist_preset_library(&state.presets_path, &updated)?;
    *current = updated;
    Ok(saved)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn delete_desktop_preset(
    state: tauri::State<'_, DesktopState>,
    preset_id: String,
) -> Result<bool, String> {
    let preset_id = Uuid::parse_str(&preset_id).map_err(|error| error.to_string())?;
    let mut current = lock(&state.presets)?;
    let mut updated = current.clone();
    if !updated.remove(preset_id) {
        return Ok(false);
    }
    persist_preset_library(&state.presets_path, &updated)?;
    *current = updated;
    Ok(true)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn import_desktop_presets(
    state: tauri::State<'_, DesktopState>,
    source_path: PathBuf,
) -> Result<DesktopPresetImportResult, String> {
    let imported = read_preset_library(&source_path)?;
    let imported_count = imported.presets.len();
    let mut current = lock(&state.presets)?;
    let mut updated = current.clone();
    updated.merge(imported).map_err(serialize_error)?;
    persist_preset_library(&state.presets_path, &updated)?;
    let total = updated.presets.len();
    *current = updated;
    Ok(DesktopPresetImportResult {
        imported: imported_count,
        total,
    })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn export_desktop_presets(
    state: tauri::State<'_, DesktopState>,
    destination_path: PathBuf,
) -> Result<usize, String> {
    let library = lock(&state.presets)?.clone();
    persist_preset_library(&destination_path, &library)?;
    Ok(library.presets.len())
}

fn lock<T>(mutex: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, String> {
    mutex
        .lock()
        .map_err(|_| "desktop state lock was poisoned".to_owned())
}

fn lock_core_store(
    mutex: &Mutex<SqliteJobStore>,
) -> formatwright_core::Result<std::sync::MutexGuard<'_, SqliteJobStore>> {
    mutex.lock().map_err(|_| {
        formatwright_core::FormatWrightError::new(
            formatwright_core::ErrorCode::Internal,
            formatwright_core::Stage::Store,
            "Desktop state lock was poisoned",
            "Restart FormatWright and retry.",
        )
    })
}

#[allow(clippy::needless_pass_by_value)]
fn serialize_error(error: formatwright_core::FormatWrightError) -> String {
    serde_json::to_string(&error).unwrap_or_else(|_| error.to_string())
}

fn report_path(directory: &std::path::Path, job_id: Uuid) -> PathBuf {
    directory.join(format!("{job_id}.json"))
}

fn save_report(
    directory: &std::path::Path,
    job_id: Uuid,
    report: &ValidationReport,
) -> Result<(), String> {
    if report.job_id != job_id {
        return Err("ValidationReport job ID does not match its destination".to_owned());
    }
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let destination = report_path(directory, job_id);
    let nonce = Uuid::new_v4();
    let partial = directory.join(format!(".{job_id}.{nonce}.partial"));
    let backup = directory.join(format!(".{job_id}.backup"));
    if !destination.exists() && backup.is_file() {
        std::fs::rename(&backup, &destination).map_err(|error| error.to_string())?;
    } else if destination.is_file() && backup.is_file() {
        std::fs::remove_file(&backup).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(report).map_err(|error| error.to_string())?;
    if let Err(error) = std::fs::write(&partial, bytes) {
        let _ = std::fs::remove_file(&partial);
        return Err(error.to_string());
    }
    if destination.is_file()
        && let Err(error) = std::fs::rename(&destination, &backup)
    {
        let _ = std::fs::remove_file(&partial);
        return Err(error.to_string());
    }
    if let Err(error) = std::fs::rename(&partial, &destination) {
        let _ = std::fs::remove_file(&partial);
        if backup.is_file() && !destination.exists() {
            let _ = std::fs::rename(&backup, &destination);
        }
        return Err(error.to_string());
    }
    if backup.is_file() {
        let _ = std::fs::remove_file(backup);
    }
    Ok(())
}

fn persist_report_before_terminal(
    store: &mut SqliteJobStore,
    reports_directory: &Path,
    job_id: Uuid,
    report: &ValidationReport,
) -> Result<JobRecord, String> {
    if let Err(report_error) = save_report(reports_directory, job_id, report) {
        let recovery_error = store
            .get_job(job_id)
            .and_then(|job| {
                if job.is_some_and(|job| {
                    matches!(job.state, JobState::Running | JobState::Validating)
                }) {
                    store
                        .transition(job_id, JobState::Interrupted, "REPORT_PERSIST_FAILED")
                        .map(drop)
                } else {
                    Ok(())
                }
            })
            .err()
            .map(|error| format!("; recovery transition also failed: {error}"))
            .unwrap_or_default();
        return Err(serialize_error(
            formatwright_core::FormatWrightError::new(
                formatwright_core::ErrorCode::StorageFailed,
                formatwright_core::Stage::Validate,
                "Unable to persist ValidationReport before terminal job state",
                "Check the reports directory, then retry or resume the interrupted job.",
            )
            .with_diagnostic(format!("{report_error}{recovery_error}")),
        ));
    }
    let final_state = match report.status {
        ValidationStatus::Pass => JobState::Completed,
        ValidationStatus::Warning | ValidationStatus::Unknown => JobState::Warning,
        ValidationStatus::Fail => JobState::Failed,
    };
    store
        .transition(job_id, final_state, "DESKTOP_CONVERSION_FINISHED")
        .map_err(serialize_error)
}

fn read_report(
    directory: &std::path::Path,
    job_id: Uuid,
) -> Result<Option<ValidationReport>, String> {
    let path = report_path(directory, job_id);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn read_preset_library(path: &Path) -> Result<PresetLibrary, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > 1024 * 1024 {
        return Err("preset library exceeds the 1 MiB import limit".to_owned());
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let library = serde_json::from_slice::<PresetLibrary>(&bytes)
        .map_err(|error| format!("invalid preset library: {error}"))?;
    library.validate().map_err(serialize_error)?;
    Ok(library)
}

fn load_preset_library(path: &Path) -> Result<PresetLibrary, String> {
    let backup = backup_path(path);
    if !path.exists() && backup.is_file() {
        std::fs::rename(&backup, path).map_err(|error| error.to_string())?;
    }
    if !path.exists() {
        return Ok(PresetLibrary::empty());
    }
    let library = read_preset_library(path)?;
    if backup.is_file() {
        std::fs::remove_file(backup).map_err(|error| error.to_string())?;
    }
    Ok(library)
}

fn persist_preset_library(path: &Path, library: &PresetLibrary) -> Result<(), String> {
    library.validate().map_err(serialize_error)?;
    let parent = path
        .parent()
        .ok_or_else(|| "preset destination has no parent directory".to_owned())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let partial = parent.join(format!(".formatwright-presets-{}.partial", Uuid::new_v4()));
    let backup = backup_path(path);
    let bytes = serde_json::to_vec_pretty(library).map_err(|error| error.to_string())?;
    std::fs::write(&partial, bytes).map_err(|error| error.to_string())?;
    if path.exists() {
        if backup.exists() {
            std::fs::remove_file(&backup).map_err(|error| error.to_string())?;
        }
        std::fs::rename(path, &backup).map_err(|error| error.to_string())?;
    }
    if let Err(error) = std::fs::rename(&partial, path) {
        if backup.is_file() && !path.exists() {
            let _ = std::fs::rename(&backup, path);
        }
        let _ = std::fs::remove_file(&partial);
        return Err(error.to_string());
    }
    if backup.is_file() {
        std::fs::remove_file(backup).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn backup_path(path: &Path) -> PathBuf {
    let filename = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("presets.json");
    path.with_file_name(format!(".{filename}.backup"))
}

fn valid_engine_summary(verified: &VerifiedEnginePack) -> DesktopEnginePackSummary {
    DesktopEnginePackSummary {
        manifest_path: verified.manifest_path.clone(),
        engine_id: Some(verified.manifest.engine_id.clone()),
        version: Some(verified.manifest.version.clone()),
        manifest_sha256: Some(verified.manifest_sha256.clone()),
        executable_names: verified.executables.keys().cloned().collect(),
        signature_present: verified.signature_present,
        valid: true,
        message: if verified.signature_present {
            "Integrity verified; signature present but not yet trusted by a release keyring."
                .to_owned()
        } else {
            "Integrity verified; unsigned pack remains unverified.".to_owned()
        },
    }
}

fn persist_engine_registry_entry(
    directory: &std::path::Path,
    verified: &VerifiedEnginePack,
) -> Result<(), String> {
    let destination = directory.join(format!("{}.json", verified.manifest.engine_id));
    let partial = directory.join(format!(
        ".{}.{}.partial",
        verified.manifest.engine_id,
        Uuid::new_v4()
    ));
    let entry = DesktopEngineRegistryEntry {
        engine_id: Some(verified.manifest.engine_id.clone()),
        manifest_path: verified.manifest_path.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&entry).map_err(|error| error.to_string())?;
    std::fs::write(&partial, bytes).map_err(|error| error.to_string())?;
    remove_superseded_registry_entries(directory, &verified.manifest.engine_id, &destination)?;
    let backup = directory.join(format!(".{}.backup", verified.manifest.engine_id));
    if destination.is_file() {
        let _ = std::fs::remove_file(&backup);
        std::fs::rename(&destination, &backup).map_err(|error| error.to_string())?;
    }
    match std::fs::rename(&partial, &destination) {
        Ok(()) => {
            let _ = std::fs::remove_file(backup);
            Ok(())
        }
        Err(error) => {
            let _ = std::fs::remove_file(partial);
            if backup.is_file() {
                let _ = std::fs::rename(&backup, &destination);
            }
            Err(error.to_string())
        }
    }
}

fn remove_superseded_registry_entries(
    directory: &Path,
    engine_id: &str,
    active_entry: &Path,
) -> Result<(), String> {
    for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
            continue;
        }
        if path == active_entry {
            continue;
        }
        let record = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<DesktopEngineRegistryEntry>(&bytes).ok());
        let superseded = record.is_some_and(|record| {
            record.engine_id.as_deref() == Some(engine_id)
                || formatwright_core::verify_engine_pack(record.manifest_path)
                    .is_ok_and(|pack| pack.manifest.engine_id == engine_id)
        });
        if superseded {
            std::fs::remove_file(path).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn registered_manifest_paths(directory: &std::path::Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    let entries = std::fs::read_dir(directory).map_err(|error| error.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
        let record =
            serde_json::from_slice::<DesktopEngineRegistryEntry>(&bytes).map_err(|error| {
                format!("invalid engine registry entry {}: {error}", path.display())
            })?;
        paths.push(record.manifest_path);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn bundled_manifest_paths(resource_directory: &Path) -> Result<Vec<PathBuf>, String> {
    let bundle_root = resource_directory.join("engine-packs").join("starter");
    let bundle_path = bundle_root.join("bundle.json");
    if !bundle_path.is_file() {
        return Ok(Vec::new());
    }
    let canonical_root = bundle_root
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let bytes = std::fs::read(&bundle_path).map_err(|error| error.to_string())?;
    let bundle = serde_json::from_slice::<DesktopEngineBundle>(&bytes)
        .map_err(|error| format!("invalid bundled engine definition: {error}"))?;
    if bundle.schema_version != 1 || bundle.bundle_id != "formatwright-windows-starter" {
        return Err("unsupported bundled engine definition".to_owned());
    }
    if bundle.packs.is_empty() {
        return Err("bundled engine definition contains no packs".to_owned());
    }

    let mut seen = HashSet::new();
    let mut manifests = Vec::with_capacity(bundle.packs.len());
    for relative in bundle.packs {
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
            || relative.file_name().and_then(std::ffi::OsStr::to_str) != Some("manifest.json")
        {
            return Err(format!(
                "unsafe bundled engine manifest path: {}",
                relative.display()
            ));
        }
        let manifest = canonical_root
            .join(&relative)
            .canonicalize()
            .map_err(|error| {
                format!(
                    "bundled engine manifest is unavailable ({}): {error}",
                    relative.display()
                )
            })?;
        if !manifest.starts_with(&canonical_root) || !manifest.is_file() {
            return Err(format!(
                "bundled engine manifest escapes its resource directory: {}",
                relative.display()
            ));
        }
        if !seen.insert(manifest.clone()) {
            return Err(format!(
                "duplicate bundled engine manifest: {}",
                relative.display()
            ));
        }
        manifests.push(manifest);
    }
    Ok(manifests)
}

fn install_bundled_engine_packs(
    resource_directory: &Path,
    engine_store_directory: &Path,
    engine_registry_directory: &Path,
) -> Result<Vec<VerifiedEnginePack>, String> {
    let manifests = bundled_manifest_paths(resource_directory)?;
    let mut installed = Vec::with_capacity(manifests.len());
    for manifest in manifests {
        let verified = formatwright_core::install_engine_pack(manifest, engine_store_directory)
            .map_err(serialize_error)?;
        persist_engine_registry_entry(engine_registry_directory, &verified)?;
        installed.push(verified);
    }
    Ok(installed)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Starts the bundled local desktop application.
///
/// # Panics
///
/// Panics when Tauri cannot initialize the configured window or event loop.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_directory = app.path().app_data_dir()?;
            let resource_directory = app.path().resource_dir()?;
            let reports_directory = data_directory.join("reports");
            let engine_registry_directory = data_directory.join("engine-registry");
            let engine_store_directory = data_directory.join("engines");
            let presets_path = data_directory.join("presets.json");
            std::fs::create_dir_all(&reports_directory)?;
            std::fs::create_dir_all(&engine_registry_directory)?;
            std::fs::create_dir_all(&engine_store_directory)?;
            install_bundled_engine_packs(
                &resource_directory,
                &engine_store_directory,
                &engine_registry_directory,
            )
            .map_err(Box::<dyn std::error::Error>::from)?;
            for manifest_path in registered_manifest_paths(&engine_registry_directory)
                .map_err(Box::<dyn std::error::Error>::from)?
            {
                let _ = activate_engine_pack(manifest_path);
            }
            let job_database_path = data_directory.join("jobs.sqlite3");
            let mut store = SqliteJobStore::open(&job_database_path)
                .map_err(|error| Box::<dyn std::error::Error>::from(error.to_string()))?;
            store
                .interrupt_active_jobs()
                .map_err(|error| Box::<dyn std::error::Error>::from(error.to_string()))?;
            let presets =
                load_preset_library(&presets_path).map_err(Box::<dyn std::error::Error>::from)?;
            app.manage(DesktopState {
                store: Mutex::new(store),
                job_database_path,
                cancellations: Mutex::new(HashMap::new()),
                queue_control: Mutex::new(None),
                presets: Mutex::new(presets),
                presets_path,
                reports_directory,
                engine_registry_directory,
                engine_store_directory,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            run_queue_bridge_benchmark,
            desktop_doctor,
            desktop_capability_snapshot,
            import_desktop_engine_pack,
            list_imported_engine_packs,
            preview_conversion,
            run_desktop_conversion,
            queue_desktop_conversion,
            run_desktop_queue_window,
            pause_desktop_queue_window,
            cancel_desktop_queue_window,
            cancel_desktop_job,
            requeue_desktop_job,
            list_desktop_jobs,
            get_desktop_report,
            list_desktop_presets,
            save_desktop_preset,
            delete_desktop_preset,
            import_desktop_presets,
            export_desktop_presets,
        ])
        .run(tauri::generate_context!())
        .expect("error while running FormatWright desktop");
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Duration;

    use tempfile::tempdir;

    use formatwright_core::{
        ArtifactSummary, ChangeSet, ConversionPreset, JobState, NetworkPolicy,
        PRESET_SCHEMA_VERSION, Plan, PlanRequest, PresetLibrary, QueueWindowControl,
        ReportRedaction, SqliteJobStore, ValidationReport, ValidationStatus,
    };
    use uuid::Uuid;

    use super::{
        DesktopConversionRequest, DesktopEngineRegistryEntry, acquire_queue_window, backup_path,
        bundled_manifest_paths, load_preset_library, persist_preset_library,
        persist_report_before_terminal, prepare_approved_desktop_conversion, read_report,
        registered_manifest_paths, requeue_job, run_queue_window_on_database, save_report,
    };

    fn plan(output_path: PathBuf) -> Plan {
        Plan {
            schema_version: 1,
            plan_id: Uuid::new_v4(),
            plan_hash: "blake3:desktop-report-fixture".to_owned(),
            input_fingerprint: "fwfp-v1:desktop-report-fixture".to_owned(),
            target_format: "yaml".to_owned(),
            constraints: std::collections::BTreeMap::new(),
            steps: Vec::new(),
            changes: ChangeSet::default(),
            validators: Vec::new(),
            network_policy: NetworkPolicy::Deny,
            output_path: Some(output_path),
            estimated_output_bytes: None,
        }
    }

    fn report(job_id: Uuid, status: ValidationStatus) -> ValidationReport {
        let artifact = ArtifactSummary {
            display_path: None,
            format_id: "yaml".to_owned(),
            size_bytes: 10,
            fast_fingerprint: "fwfp-v1:desktop-report-fixture".to_owned(),
            full_blake3: None,
        };
        ValidationReport {
            schema_version: 1,
            report_id: Uuid::new_v4(),
            job_id,
            plan_hash: "blake3:desktop-report-fixture".to_owned(),
            status,
            input: artifact.clone(),
            output: artifact,
            engines: Vec::new(),
            checks: Vec::new(),
            intentional_changes: Vec::new(),
            redaction: ReportRedaction {
                paths_redacted: true,
                metadata_values_redacted: true,
            },
        }
    }

    fn validating_job(store: &mut SqliteJobStore, root: &std::path::Path) -> Uuid {
        let output = root.join(format!("{}.yaml", Uuid::new_v4()));
        let job = store
            .create_job(root.join("input.json"), &output, &plan(output.clone()))
            .expect("create job");
        store
            .transition(job.id, JobState::Running, "TEST_ENGINE_STARTED")
            .expect("start job");
        store
            .transition(job.id, JobState::Validating, "TEST_ENGINE_FINISHED")
            .expect("validate job");
        job.id
    }

    fn conversion_request(
        input_path: PathBuf,
        output_path: PathBuf,
        approved_plan_hash: Option<String>,
    ) -> DesktopConversionRequest {
        DesktopConversionRequest {
            input_path,
            output_path,
            target_format: "yaml".to_owned(),
            quality: None,
            width: None,
            dpi: None,
            color_mode: None,
            preserve_all_streams: Some(true),
            approved_plan_hash,
        }
    }

    #[test]
    fn engine_registry_reads_entries_and_ignores_partials() {
        let directory = tempdir().expect("temporary registry");
        let expected = PathBuf::from("C:/engine-pack/manifest.json");
        let entry = DesktopEngineRegistryEntry {
            engine_id: Some("fixture-engine".to_owned()),
            manifest_path: expected.clone(),
        };
        fs::write(
            directory.path().join("abc.json"),
            serde_json::to_vec(&entry).expect("serialize entry"),
        )
        .expect("write registry entry");
        fs::write(directory.path().join(".abc.partial"), b"incomplete").expect("write partial");

        assert_eq!(
            registered_manifest_paths(directory.path()).expect("read registry"),
            vec![expected]
        );
    }

    #[test]
    fn bundled_manifest_paths_reject_traversal() {
        let resource_directory = tempdir().expect("temporary resources");
        let bundle_root = resource_directory.path().join("engine-packs/starter");
        fs::create_dir_all(&bundle_root).expect("create bundle root");
        fs::write(
            bundle_root.join("bundle.json"),
            br#"{
                "schema_version": 1,
                "bundle_id": "formatwright-windows-starter",
                "packs": ["../manifest.json"]
            }"#,
        )
        .expect("write bundle");

        let error = bundled_manifest_paths(resource_directory.path())
            .expect_err("path traversal must be rejected");
        assert!(error.contains("unsafe bundled engine manifest path"));
    }

    #[test]
    fn report_replacement_is_atomic_and_leaves_only_the_active_report() {
        let directory = tempdir().expect("temporary reports");
        let job_id = Uuid::new_v4();
        let first = report(job_id, ValidationStatus::Pass);
        save_report(directory.path(), job_id, &first).expect("write first report");
        let second = report(job_id, ValidationStatus::Warning);
        save_report(directory.path(), job_id, &second).expect("replace report");

        assert_eq!(
            read_report(directory.path(), job_id).expect("read report"),
            Some(second)
        );
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("read report directory")
                .filter_map(std::result::Result::ok)
                .count(),
            1
        );
    }

    #[test]
    fn report_write_recovers_an_interrupted_replacement_backup() {
        let directory = tempdir().expect("temporary reports");
        let job_id = Uuid::new_v4();
        let first = report(job_id, ValidationStatus::Pass);
        save_report(directory.path(), job_id, &first).expect("write first report");
        let destination = super::report_path(directory.path(), job_id);
        let backup = directory.path().join(format!(".{job_id}.backup"));
        fs::rename(&destination, &backup).expect("simulate interrupted replacement");

        let second = report(job_id, ValidationStatus::Warning);
        save_report(directory.path(), job_id, &second).expect("recover and replace report");

        assert_eq!(
            read_report(directory.path(), job_id).expect("read report"),
            Some(second)
        );
        assert!(!backup.exists());
    }

    #[test]
    fn report_failure_interrupts_job_before_terminal_state() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let job_id = validating_job(&mut store, suite.path());
        let blocked_reports = suite.path().join("reports-is-a-file");
        fs::write(&blocked_reports, b"not a directory").expect("write blocking file");

        let error = persist_report_before_terminal(
            &mut store,
            &blocked_reports,
            job_id,
            &report(job_id, ValidationStatus::Pass),
        )
        .expect_err("report persistence must fail");

        assert!(error.contains("Unable to persist ValidationReport"));
        let details = store
            .get_job_details(job_id)
            .expect("read details")
            .expect("details");
        assert_eq!(details.job.state, JobState::Interrupted);
        assert_eq!(
            details.events.last().expect("last event").code,
            "REPORT_PERSIST_FAILED"
        );
    }

    #[test]
    fn report_is_readable_before_successful_terminal_transition_returns() {
        let suite = tempdir().expect("suite");
        let reports = suite.path().join("reports");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let job_id = validating_job(&mut store, suite.path());
        let validation = report(job_id, ValidationStatus::Pass);

        let job = persist_report_before_terminal(&mut store, &reports, job_id, &validation)
            .expect("persist and finish");

        assert_eq!(job.state, JobState::Completed);
        assert_eq!(
            read_report(&reports, job_id).expect("read report"),
            Some(validation)
        );
    }

    #[test]
    fn desktop_requeues_recoverable_jobs_and_rejects_terminal_success() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");

        let interrupted_output = suite.path().join("interrupted.yaml");
        let interrupted = store
            .create_job(
                suite.path().join("interrupted.json"),
                &interrupted_output,
                &plan(interrupted_output.clone()),
            )
            .expect("create interrupted job");
        store
            .transition(interrupted.id, JobState::Running, "TEST_STARTED")
            .expect("start interrupted job");
        store
            .transition(interrupted.id, JobState::Interrupted, "TEST_INTERRUPTED")
            .expect("interrupt job");
        let resumed = requeue_job(&mut store, interrupted.id).expect("resume job");
        assert_eq!(resumed.state, JobState::Queued);
        let details = store
            .get_job_details(interrupted.id)
            .expect("read resumed details")
            .expect("resumed details");
        assert_eq!(
            details.events.last().expect("resume event").code,
            "DESKTOP_JOB_RESUMED"
        );

        let failed_output = suite.path().join("failed.yaml");
        let failed = store
            .create_job(
                suite.path().join("failed.json"),
                &failed_output,
                &plan(failed_output.clone()),
            )
            .expect("create failed job");
        store
            .transition(failed.id, JobState::Running, "TEST_STARTED")
            .expect("start failed job");
        store
            .transition(failed.id, JobState::Failed, "TEST_FAILED")
            .expect("fail job");
        let retried = requeue_job(&mut store, failed.id).expect("retry job");
        assert_eq!(retried.state, JobState::Queued);

        let completed = validating_job(&mut store, suite.path());
        store
            .transition(completed, JobState::Completed, "TEST_COMPLETED")
            .expect("complete job");
        let error = requeue_job(&mut store, completed)
            .expect_err("successful terminal job must not be requeued");
        assert_eq!(error.code, formatwright_core::ErrorCode::PolicyBlocked);
        assert_eq!(
            store
                .get_job(completed)
                .expect("read")
                .expect("exists")
                .state,
            JobState::Completed
        );
    }

    #[test]
    fn queue_window_keeps_live_reads_paging_and_enqueue_available() {
        let suite = tempdir().expect("suite");
        let database_path = suite.path().join("jobs.sqlite3");
        let first_input = suite.path().join("first.json");
        let first_output = suite.path().join("first.yaml");
        let second_input = suite.path().join("second.json");
        let second_output = suite.path().join("second.yaml");
        fs::write(&first_input, r#"[{"id":1}]"#).expect("write first input");
        fs::write(&second_input, r#"[{"id":2}]"#).expect("write second input");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("planning runtime");
        let first_plan = runtime
            .block_on(formatwright_core::prepare_conversion(
                &first_input,
                &PlanRequest {
                    target_format: "yaml".to_owned(),
                    output_path: Some(first_output.clone()),
                    ..PlanRequest::default()
                },
            ))
            .expect("prepare first")
            .1;
        let second_plan = runtime
            .block_on(formatwright_core::prepare_conversion(
                &second_input,
                &PlanRequest {
                    target_format: "yaml".to_owned(),
                    output_path: Some(second_output.clone()),
                    ..PlanRequest::default()
                },
            ))
            .expect("prepare second")
            .1;
        drop(runtime);

        let mut ui_store = SqliteJobStore::open(&database_path).expect("UI store");
        let first = ui_store
            .create_job(&first_input, &first_output, &first_plan)
            .and_then(|job| ui_store.transition(job.id, JobState::Queued, "JOB_ENQUEUED"))
            .expect("queue first");
        let (callback_entered_tx, callback_entered_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let queue_database = database_path.clone();
        let queue_thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("queue runtime");
            runtime.block_on(run_queue_window_on_database(
                &queue_database,
                8,
                1,
                QueueWindowControl::new(),
                move |_, _| {
                    callback_entered_tx.send(()).expect("signal callback");
                    release_rx.recv().expect("release callback");
                    Ok(())
                },
            ))
        });
        callback_entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("queue reached report callback");

        let visible = ui_store.list_jobs(100).expect("live job list");
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, first.id);
        let second = ui_store
            .create_job(&second_input, &second_output, &second_plan)
            .and_then(|job| ui_store.transition(job.id, JobState::Queued, "JOB_ENQUEUED"))
            .expect("enqueue while queue window runs");
        assert_eq!(ui_store.count_jobs().expect("live count"), 2);
        assert_eq!(ui_store.list_jobs_page(1, 0).expect("first page").len(), 1);
        assert_eq!(ui_store.list_jobs_page(1, 1).expect("second page").len(), 1);

        release_tx.send(()).expect("release queue callback");
        let report = queue_thread
            .join()
            .expect("join queue thread")
            .expect("queue window");
        assert_eq!(report.selected, 1);
        assert_eq!(report.completed, 1);
        assert_eq!(
            ui_store
                .get_job(first.id)
                .expect("read first")
                .expect("first")
                .state,
            JobState::Completed
        );
        assert_eq!(
            ui_store
                .get_job(second.id)
                .expect("read second")
                .expect("second")
                .state,
            JobState::Queued
        );
    }

    #[test]
    fn queue_window_lease_clears_exclusivity_on_every_drop_path() {
        let slot = std::sync::Mutex::new(None);
        let (_, lease) = acquire_queue_window(&slot).expect("acquire first window");
        assert!(slot.lock().expect("read slot").is_some());
        let error = acquire_queue_window(&slot)
            .err()
            .expect("parallel queue window must be rejected");
        assert!(error.contains("already running"));
        drop(lease);
        assert!(slot.lock().expect("read cleared slot").is_none());
        let (_, replacement) = acquire_queue_window(&slot).expect("acquire after cleanup");
        drop(replacement);
    }

    #[tokio::test]
    async fn desktop_execution_requires_the_exact_preview_plan_hash() {
        let suite = tempdir().expect("suite");
        let input = suite.path().join("input.json");
        let output = suite.path().join("output.yaml");
        fs::write(&input, r#"[{"id":1}]"#).expect("write input");
        let request = conversion_request(input.clone(), output.clone(), None);
        let preview = formatwright_core::prepare_conversion(&input, &request.plan_request())
            .await
            .expect("preview");

        let missing = prepare_approved_desktop_conversion(&request)
            .await
            .expect_err("missing approval must fail");
        assert!(missing.contains("POLICY_BLOCKED"));

        let approved = conversion_request(
            input.clone(),
            output.clone(),
            Some(preview.1.plan_hash.clone()),
        );
        prepare_approved_desktop_conversion(&approved)
            .await
            .expect("unchanged preview is approved");

        fs::write(&input, r#"[{"id":2}]"#).expect("change input");
        let changed = prepare_approved_desktop_conversion(&approved)
            .await
            .expect_err("changed input must invalidate approval");
        assert!(changed.contains("INPUT_CHANGED"));
    }

    #[test]
    fn preset_library_write_is_recoverable_from_backup() {
        let directory = tempdir().expect("temporary presets");
        let path = directory.path().join("presets.json");
        let mut library = PresetLibrary::empty();
        library
            .upsert(ConversionPreset {
                schema_version: PRESET_SCHEMA_VERSION,
                preset_id: Uuid::new_v4(),
                name: "Smaller image".to_owned(),
                target_format: "webp".to_owned(),
                quality: Some(78),
                width: None,
                dpi: None,
                color_mode: Some("rgb".to_owned()),
                preserve_all_streams: true,
            })
            .expect("valid preset");
        persist_preset_library(&path, &library).expect("persist presets");
        let backup = backup_path(&path);
        fs::rename(&path, &backup).expect("simulate interrupted replacement");
        assert_eq!(load_preset_library(&path).expect("recover backup"), library);
        assert!(path.is_file());
        assert!(!backup.exists());
    }
}
