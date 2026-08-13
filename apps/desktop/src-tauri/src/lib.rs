#![forbid(unsafe_code)]

mod queue_bridge;

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use formatwright_core::{
    ApplicationSettings, ApplicationSettingsService, ApplicationStateLayout,
    ApplicationStateService, BatchRecord, BulkActionReport, BulkJobAction, BulkJobService,
    CapabilitySnapshot, CompactReport, ConversionPreset, ConversionService, DoctorReport,
    EngineDiscoveryPolicy, EngineRegistryIdentity, FolderBatchService, FolderMappingEntry,
    IntegrityReport, JobCreateRequest, JobExecutionService, JobQueryPage, JobRecord,
    JobSelectionQuery, JobState, JobStateCount, MaintenanceService, MaintenanceStatus,
    PRESET_SCHEMA_VERSION, Plan, PlanRequest, PresetLibrary, Probe, QueueRunReport,
    QueueWindowControl, ReportService, SelectionSnapshot, SqliteJobStore, StateBundleBackupReport,
    StateBundleOptions, StateBundlePreflightReport, ValidationReport, VerifiedEnginePack,
    activate_engine_pack, capability_snapshot_for_input, cleanup_staged_output, prepare_conversion,
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
    settings: Mutex<Option<ApplicationSettings>>,
    settings_path: PathBuf,
    reports_directory: PathBuf,
    engine_registry_directory: PathBuf,
    engine_store_directory: PathBuf,
    startup_recovery: DesktopStartupRecovery,
    operation_gate: Mutex<DesktopOperationGate>,
    folder_previews: Mutex<HashMap<Uuid, DesktopFolderPreviewCache>>,
}

#[derive(Clone, Debug, Default, Serialize)]
struct DesktopStartupRecovery {
    recovered_after_restart: usize,
    removed_staged_outputs: usize,
    restored_bundle_id: Option<Uuid>,
    restore_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct DesktopRecoverySummary {
    recovered_after_restart: usize,
    removed_staged_outputs: usize,
    restored_bundle_id: Option<Uuid>,
    restore_error: Option<String>,
    state_counts: Vec<JobStateCount>,
}

#[derive(Clone, Debug, Serialize)]
struct DesktopScheduledRestore {
    bundle_id: Uuid,
    bundle_path: PathBuf,
    restart_required: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct DesktopOperationGate {
    active_operations: usize,
    maintenance_exclusive: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DesktopPendingRestore {
    schema_version: u16,
    bundle_id: Uuid,
    bundle_path: PathBuf,
    bundle_size_bytes: u64,
    bundle_blake3: String,
}

const DESKTOP_PENDING_RESTORE_SCHEMA_VERSION: u16 = 1;
const DESKTOP_PENDING_RESTORE_FILE: &str = ".desktop-state-restore.json";
const DESKTOP_FOLDER_PREVIEW_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);
const DESKTOP_FOLDER_PREVIEW_LIMIT: usize = 32;
const DESKTOP_FOLDER_PREVIEW_SAMPLE: usize = 100;
const DESKTOP_FOLDER_PLAN_LIMIT: usize = 10_000;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopFolderPreviewRequest {
    input_root: PathBuf,
    output_root: PathBuf,
    target_format: String,
    quality: Option<u8>,
    width: Option<u32>,
    dpi: Option<u16>,
    color_mode: Option<String>,
    preserve_all_streams: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
struct DesktopFolderPreview {
    preview_id: Uuid,
    created_unix_ms: i64,
    expires_unix_ms: i64,
    input_root: PathBuf,
    output_root: PathBuf,
    target_format: String,
    discovered: usize,
    planned: usize,
    skipped: usize,
    sample: Vec<FolderMappingEntry>,
    truncated: bool,
}

#[derive(Debug)]
struct DesktopFolderPreviewCache {
    preview: DesktopFolderPreview,
    requests: Vec<JobCreateRequest>,
    output_directories: Vec<PathBuf>,
    created: Instant,
}

#[derive(Clone, Debug, Serialize)]
struct DesktopFolderQueueResult {
    batch: BatchRecord,
    queued: usize,
}

struct DesktopOperationLease<'a> {
    gate: &'a Mutex<DesktopOperationGate>,
    exclusive: bool,
}

impl Drop for DesktopOperationLease<'_> {
    fn drop(&mut self) {
        if let Ok(mut gate) = self.gate.lock() {
            if self.exclusive {
                gate.maintenance_exclusive = false;
            } else {
                gate.active_operations = gate.active_operations.saturating_sub(1);
            }
        }
    }
}

fn acquire_active_operation(
    gate: &Mutex<DesktopOperationGate>,
) -> Result<DesktopOperationLease<'_>, String> {
    let mut current = lock(gate)?;
    if current.maintenance_exclusive {
        return Err("maintenance is running; retry after it finishes".to_owned());
    }
    current.active_operations = current.active_operations.saturating_add(1);
    drop(current);
    Ok(DesktopOperationLease {
        gate,
        exclusive: false,
    })
}

fn acquire_maintenance_operation(
    gate: &Mutex<DesktopOperationGate>,
) -> Result<DesktopOperationLease<'_>, String> {
    let mut current = lock(gate)?;
    if current.maintenance_exclusive || current.active_operations > 0 {
        return Err("stop the queue and wait for active conversions before maintenance".to_owned());
    }
    current.maintenance_exclusive = true;
    drop(current);
    Ok(DesktopOperationLease {
        gate,
        exclusive: true,
    })
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

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_desktop_settings(
    state: tauri::State<'_, DesktopState>,
) -> Result<Option<ApplicationSettings>, String> {
    Ok(lock(&state.settings)?.clone())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn save_desktop_settings(
    state: tauri::State<'_, DesktopState>,
    settings: ApplicationSettings,
) -> Result<ApplicationSettings, String> {
    let _operation = acquire_active_operation(&state.operation_gate)?;
    let mut current = lock(&state.settings)?;
    ApplicationSettingsService::new(&state.settings_path)
        .save(&settings)
        .map_err(serialize_error)?;
    *current = Some(settings.clone());
    Ok(settings)
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
    idempotency_key: Option<String>,
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

type DesktopEngineRegistryEntry = EngineRegistryIdentity;

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
    let _operation = acquire_active_operation(&state.operation_gate)?;
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
    let _operation = acquire_active_operation(&state.operation_gate)?;
    let (probe, plan, validation_engine) = prepare_approved_desktop_conversion(&request).await?;
    let cancellation = CancellationToken::new();
    let cancellation_slot = &state.cancellations;
    let job_id = Mutex::new(None);
    let mut execution_store =
        SqliteJobStore::open(&state.job_database_path).map_err(serialize_error)?;
    let result = ConversionService::run_prepared(
        &mut execution_store,
        &ReportService::new(&state.reports_directory),
        &probe,
        &plan,
        &validation_engine,
        &plan.plan_hash,
        cancellation.clone(),
        |job| {
            if job.state == JobState::Running {
                if let Ok(mut current) = job_id.lock() {
                    *current = Some(job.id);
                }
                if let Ok(mut tokens) = cancellation_slot.lock() {
                    tokens.insert(job.id, cancellation.clone());
                }
            }
            let _ = window.emit("formatwright://job-updated", job);
        },
    )
    .await
    .map_err(serialize_error);
    if let Some(job_id) = *lock(&job_id)? {
        lock(&state.cancellations)?.remove(&job_id);
    }
    result.map(|result| DesktopRunResult {
        job: result.job,
        report: result.report,
    })
}

#[tauri::command]
async fn queue_desktop_conversion(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    request: DesktopConversionRequest,
) -> Result<JobRecord, String> {
    let _operation = acquire_active_operation(&state.operation_gate)?;
    let (probe, plan, _) = prepare_approved_desktop_conversion(&request).await?;
    let job = {
        let mut store = lock(&state.store)?;
        if let Some(key) = request.idempotency_key.as_deref() {
            let created = store
                .enqueue_job_idempotent(
                    key,
                    &formatwright_core::JobCreateRequest {
                        input_path: probe.artifact.canonical_path.clone(),
                        output_path: request.output_path.clone(),
                        plan: plan.clone(),
                    },
                )
                .map_err(serialize_error)?;
            created.job
        } else {
            store
                .create_job(&probe.artifact.canonical_path, &request.output_path, &plan)
                .and_then(|job| store.transition(job.id, JobState::Queued, "JOB_ENQUEUED"))
                .map_err(serialize_error)?
        }
    };
    let _ = window.emit("formatwright://job-updated", &job);
    Ok(job)
}

#[tauri::command]
#[allow(clippy::too_many_lines)]
async fn preview_desktop_folder_batch(
    state: tauri::State<'_, DesktopState>,
    request: DesktopFolderPreviewRequest,
) -> Result<DesktopFolderPreview, String> {
    let _operation = acquire_active_operation(&state.operation_gate)?;
    let target = request.target_format.trim().to_ascii_lowercase();
    let mapping = tokio::task::spawn_blocking({
        let input_root = request.input_root.clone();
        let output_root = request.output_root.clone();
        let target = target.clone();
        move || FolderBatchService::preview_mapping(input_root, output_root, &target)
    })
    .await
    .map_err(|error| format!("folder-enumeration worker failed: {error}"))?
    .map_err(serialize_error)?;
    if mapping.mappings.len() > DESKTOP_FOLDER_PLAN_LIMIT {
        return Err(serialize_error(formatwright_core::FormatWrightError::new(
            formatwright_core::ErrorCode::ResourceExhausted,
            formatwright_core::Stage::Plan,
            "Desktop folder preview exceeds the 10,000-Plan limit",
            "Split the source into smaller folders before previewing.",
        )));
    }

    let (requests, skipped, output_directories) = async {
        let mut requests = Vec::new();
        let mut skipped = mapping.skipped;
        let mut output_directories = HashSet::new();
        for entry in &mapping.mappings {
            let plan_request = PlanRequest {
                target_format: target.clone(),
                output_path: Some(entry.output_path.clone()),
                preserve_all_streams: request.preserve_all_streams.unwrap_or(true),
                quality: request.quality,
                width: request.width,
                dpi: request.dpi,
                color_mode: request.color_mode.clone(),
                ..PlanRequest::default()
            };
            match prepare_conversion(&entry.input_path, &plan_request).await {
                Ok((probe, plan, _)) => {
                    if entry.output_path.exists() {
                        return Err(serialize_error(formatwright_core::FormatWrightError::new(
                            formatwright_core::ErrorCode::OutputConflict,
                            formatwright_core::Stage::Plan,
                            format!(
                                "Folder batch output already exists: {}",
                                entry.output_path.display()
                            ),
                            "Choose an empty output root or move the existing output.",
                        )));
                    }
                    if let Some(parent) = entry.output_path.parent() {
                        output_directories.insert(parent.to_path_buf());
                    }
                    requests.push(JobCreateRequest {
                        input_path: probe.artifact.canonical_path,
                        output_path: entry.output_path.clone(),
                        plan,
                    });
                }
                Err(_) => skipped = skipped.saturating_add(1),
            }
        }
        Ok::<_, String>((requests, skipped, output_directories))
    }
    .await?;
    if requests.is_empty() {
        return Err(serialize_error(formatwright_core::FormatWrightError::new(
            formatwright_core::ErrorCode::Unsupported,
            formatwright_core::Stage::Plan,
            "No file in the selected folder can use this conversion route",
            "Choose another target or a folder containing supported inputs.",
        )));
    }
    let planned_outputs = requests
        .iter()
        .map(|candidate| candidate.output_path.clone())
        .collect::<HashSet<_>>();
    let sample = mapping
        .mappings
        .into_iter()
        .filter(|entry| planned_outputs.contains(&entry.output_path))
        .take(DESKTOP_FOLDER_PREVIEW_SAMPLE)
        .collect::<Vec<_>>();
    let now = unix_ms_now();
    let preview = DesktopFolderPreview {
        preview_id: Uuid::new_v4(),
        created_unix_ms: now,
        expires_unix_ms: now.saturating_add(
            i64::try_from(DESKTOP_FOLDER_PREVIEW_TTL.as_millis()).unwrap_or(i64::MAX),
        ),
        input_root: mapping.input_root,
        output_root: mapping.output_root,
        target_format: target,
        discovered: mapping.discovered,
        planned: requests.len(),
        skipped,
        truncated: requests.len() > sample.len(),
        sample,
    };
    let cache = DesktopFolderPreviewCache {
        preview: preview.clone(),
        requests,
        output_directories: output_directories.into_iter().collect(),
        created: Instant::now(),
    };
    let mut previews = lock(&state.folder_previews)?;
    previews.retain(|_, value| value.created.elapsed() <= DESKTOP_FOLDER_PREVIEW_TTL);
    if previews.len() >= DESKTOP_FOLDER_PREVIEW_LIMIT
        && let Some(oldest) = previews
            .iter()
            .min_by_key(|(_, value)| value.created)
            .map(|(id, _)| *id)
    {
        previews.remove(&oldest);
    }
    previews.insert(preview.preview_id, cache);
    Ok(preview)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn queue_desktop_folder_batch(
    state: tauri::State<'_, DesktopState>,
    preview_id: String,
    batch_name: Option<String>,
) -> Result<DesktopFolderQueueResult, String> {
    let _operation = acquire_active_operation(&state.operation_gate)?;
    let preview_id = Uuid::parse_str(&preview_id).map_err(|error| error.to_string())?;
    let cache = lock(&state.folder_previews)?
        .remove(&preview_id)
        .ok_or_else(|| "folder preview is missing, expired, or already queued".to_owned())?;
    if cache.created.elapsed() > DESKTOP_FOLDER_PREVIEW_TTL {
        return Err("folder preview expired; preview the mapping again".to_owned());
    }
    for request in &cache.requests {
        if request.output_path.exists() {
            return Err(serialize_error(formatwright_core::FormatWrightError::new(
                formatwright_core::ErrorCode::OutputConflict,
                formatwright_core::Stage::Store,
                format!(
                    "Folder batch output appeared after preview: {}",
                    request.output_path.display()
                ),
                "Preview the folder mapping again and resolve the output conflict.",
            )));
        }
    }
    let mut created_directories = Vec::new();
    let mut directories = cache.output_directories;
    directories.sort_by_key(|path| path.components().count());
    for directory in directories {
        if !directory.exists() {
            std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
            created_directories.push(directory);
        }
    }
    let default_name = format!(
        "Folder: {} -> {}",
        cache
            .preview
            .input_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("files"),
        cache.preview.target_format
    );
    let result = lock(&state.store)?
        .create_queued_batch(
            batch_name.as_deref().unwrap_or(&default_name),
            &cache.requests,
        )
        .map_err(serialize_error);
    let batch = match result {
        Ok(batch) => batch,
        Err(error) => {
            for directory in created_directories.into_iter().rev() {
                let _ = std::fs::remove_dir(directory);
            }
            return Err(error);
        }
    };
    Ok(DesktopFolderQueueResult {
        batch,
        queued: cache.requests.len(),
    })
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
    let _operation = acquire_active_operation(&state.operation_gate)?;
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
            ReportService::new(&reports_directory)
                .save(job_id, validation)
                .map(drop)
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
    let _operation = acquire_active_operation(&state.operation_gate)?;
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
fn query_desktop_jobs(
    state: tauri::State<'_, DesktopState>,
    batch_id: Option<String>,
    states: Vec<JobState>,
    search: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<JobQueryPage, String> {
    let batch_id = batch_id
        .map(|value| Uuid::parse_str(&value).map_err(|error| error.to_string()))
        .transpose()?;
    lock(&state.store)?
        .query_jobs_page(
            &JobSelectionQuery {
                batch_id,
                states,
                search,
            },
            limit.unwrap_or(100),
            offset.unwrap_or_default(),
        )
        .map_err(serialize_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_desktop_recovery_summary(
    state: tauri::State<'_, DesktopState>,
) -> Result<DesktopRecoverySummary, String> {
    Ok(DesktopRecoverySummary {
        recovered_after_restart: state.startup_recovery.recovered_after_restart,
        removed_staged_outputs: state.startup_recovery.removed_staged_outputs,
        restored_bundle_id: state.startup_recovery.restored_bundle_id,
        restore_error: state.startup_recovery.restore_error.clone(),
        state_counts: lock(&state.store)?
            .count_jobs_by_state()
            .map_err(serialize_error)?,
    })
}

#[tauri::command]
async fn get_desktop_maintenance_status(
    state: tauri::State<'_, DesktopState>,
) -> Result<MaintenanceStatus, String> {
    let database_path = state.job_database_path.clone();
    tokio::task::spawn_blocking(move || MaintenanceService::new(database_path).status())
        .await
        .map_err(|error| format!("maintenance status worker failed: {error}"))?
        .map_err(serialize_error)
}

#[tauri::command]
async fn check_desktop_integrity(
    state: tauri::State<'_, DesktopState>,
) -> Result<IntegrityReport, String> {
    let _operation = acquire_maintenance_operation(&state.operation_gate)?;
    let database_path = state.job_database_path.clone();
    tokio::task::spawn_blocking(move || MaintenanceService::new(database_path).integrity_check())
        .await
        .map_err(|error| format!("integrity-check worker failed: {error}"))?
        .map_err(serialize_error)
}

#[tauri::command]
async fn backup_desktop_state(
    state: tauri::State<'_, DesktopState>,
    destination_path: PathBuf,
    include_reports: Option<bool>,
) -> Result<StateBundleBackupReport, String> {
    let _operation = acquire_maintenance_operation(&state.operation_gate)?;
    let database_path = state.job_database_path.clone();
    tokio::task::spawn_blocking(move || {
        ApplicationStateService::from_database(database_path)?.backup(
            destination_path,
            StateBundleOptions {
                include_reports: include_reports.unwrap_or(false),
            },
        )
    })
    .await
    .map_err(|error| format!("state-backup worker failed: {error}"))?
    .map_err(serialize_error)
}

#[tauri::command]
async fn preflight_desktop_state_restore(
    state: tauri::State<'_, DesktopState>,
    bundle_path: PathBuf,
) -> Result<StateBundlePreflightReport, String> {
    let _operation = acquire_maintenance_operation(&state.operation_gate)?;
    let database_path = state.job_database_path.clone();
    tokio::task::spawn_blocking(move || {
        ApplicationStateService::from_database(database_path)?.restore_preflight(bundle_path)
    })
    .await
    .map_err(|error| format!("restore-preflight worker failed: {error}"))?
    .map_err(serialize_error)
}

#[tauri::command]
async fn schedule_desktop_state_restore(
    state: tauri::State<'_, DesktopState>,
    bundle_path: PathBuf,
    expected_bundle_id: String,
) -> Result<DesktopScheduledRestore, String> {
    let _operation = acquire_maintenance_operation(&state.operation_gate)?;
    let database_path = state.job_database_path.clone();
    let expected_bundle_id =
        Uuid::parse_str(&expected_bundle_id).map_err(|error| error.to_string())?;
    tokio::task::spawn_blocking(move || {
        stage_pending_restore(&database_path, &bundle_path, expected_bundle_id)
    })
    .await
    .map_err(|error| format!("restore-preflight worker failed: {error}"))?
    .map_err(serialize_error)
}

#[tauri::command]
async fn compact_desktop_database(
    state: tauri::State<'_, DesktopState>,
) -> Result<CompactReport, String> {
    let _operation = acquire_maintenance_operation(&state.operation_gate)?;
    let database_path = state.job_database_path.clone();
    let report =
        tokio::task::spawn_blocking(move || MaintenanceService::new(database_path).compact())
            .await
            .map_err(|error| format!("compact worker failed: {error}"))?
            .map_err(serialize_error)?;
    let replacement = SqliteJobStore::open(&state.job_database_path).map_err(serialize_error)?;
    *lock(&state.store)? = replacement;
    Ok(report)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn list_desktop_batches(
    state: tauri::State<'_, DesktopState>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<BatchRecord>, String> {
    lock(&state.store)?
        .list_batches_page(
            limit.unwrap_or(100).clamp(1, 500),
            offset.unwrap_or_default(),
        )
        .map_err(serialize_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn capture_desktop_job_selection(
    state: tauri::State<'_, DesktopState>,
    batch_id: Option<String>,
    states: Vec<JobState>,
    search: Option<String>,
) -> Result<SelectionSnapshot, String> {
    let _operation = acquire_active_operation(&state.operation_gate)?;
    let batch_id = batch_id
        .map(|value| Uuid::parse_str(&value).map_err(|error| error.to_string()))
        .transpose()?;
    lock(&state.store)?
        .capture_selection(&JobSelectionQuery {
            batch_id,
            states,
            search,
        })
        .map_err(serialize_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn run_desktop_bulk_action(
    state: tauri::State<'_, DesktopState>,
    selection_id: String,
    action: BulkJobAction,
) -> Result<BulkActionReport, String> {
    let _operation = acquire_active_operation(&state.operation_gate)?;
    let selection_id = Uuid::parse_str(&selection_id).map_err(|error| error.to_string())?;
    let mut store = lock(&state.store)?;
    BulkJobService::apply(&mut store, selection_id, action).map_err(serialize_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_desktop_report(
    state: tauri::State<'_, DesktopState>,
    job_id: String,
) -> Result<Option<ValidationReport>, String> {
    let job_id = Uuid::parse_str(&job_id).map_err(|error| error.to_string())?;
    ReportService::new(&state.reports_directory)
        .read(job_id)
        .map_err(serialize_error)
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
    let _operation = acquire_active_operation(&state.operation_gate)?;
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
    let _operation = acquire_active_operation(&state.operation_gate)?;
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
    let _operation = acquire_active_operation(&state.operation_gate)?;
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

fn unix_ms_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

#[allow(clippy::needless_pass_by_value)]
fn serialize_error(error: formatwright_core::FormatWrightError) -> String {
    serde_json::to_string(&error).unwrap_or_else(|_| error.to_string())
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

fn pending_restore_path(database_path: &Path) -> PathBuf {
    database_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(DESKTOP_PENDING_RESTORE_FILE)
}

fn stage_pending_restore(
    database_path: &Path,
    bundle_path: &Path,
    expected_bundle_id: Uuid,
) -> formatwright_core::Result<DesktopScheduledRestore> {
    let state = ApplicationStateService::from_database(database_path.to_path_buf())?;
    let report = state.restore_preflight(bundle_path)?;
    if report.bundle_id != expected_bundle_id {
        return Err(formatwright_core::FormatWrightError::new(
            formatwright_core::ErrorCode::InputChanged,
            formatwright_core::Stage::Store,
            "The selected bundle changed after restore preflight",
            "Choose the bundle again and repeat restore preflight.",
        ));
    }
    let root = database_path.parent().unwrap_or_else(|| Path::new("."));
    let staged_bundle = root.join(format!(
        ".desktop-state-restore-{}.fwstate",
        report.bundle_id
    ));
    let mut source = std::fs::File::open(&report.bundle_path).map_err(desktop_storage_error)?;
    let mut staged = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged_bundle)
        .map_err(desktop_storage_error)?;
    if let Err(error) = std::io::copy(&mut source, &mut staged).and_then(|_| staged.sync_all()) {
        let _ = std::fs::remove_file(&staged_bundle);
        return Err(desktop_storage_error(error));
    }
    drop(staged);
    let staged_report = match state.restore_preflight(&staged_bundle) {
        Ok(report) if report.bundle_id == expected_bundle_id => report,
        Ok(_) => {
            let _ = std::fs::remove_file(&staged_bundle);
            return Err(formatwright_core::FormatWrightError::new(
                formatwright_core::ErrorCode::InputChanged,
                formatwright_core::Stage::Store,
                "The state bundle changed while it was staged",
                "Choose the bundle again and repeat restore preflight.",
            ));
        }
        Err(error) => {
            let _ = std::fs::remove_file(&staged_bundle);
            return Err(error);
        }
    };
    let request = DesktopPendingRestore {
        schema_version: DESKTOP_PENDING_RESTORE_SCHEMA_VERSION,
        bundle_id: staged_report.bundle_id,
        bundle_size_bytes: std::fs::metadata(&staged_bundle)
            .map_err(desktop_storage_error)?
            .len(),
        bundle_blake3: blake3_file(&staged_bundle)?,
        bundle_path: staged_bundle,
    };
    if let Err(error) = persist_pending_restore(&pending_restore_path(database_path), &request) {
        let _ = std::fs::remove_file(&request.bundle_path);
        return Err(error);
    }
    Ok(DesktopScheduledRestore {
        bundle_id: request.bundle_id,
        bundle_path: request.bundle_path,
        restart_required: true,
    })
}

fn apply_pending_restore(database_path: &Path) -> (Option<Uuid>, Option<String>) {
    let path = pending_restore_path(database_path);
    if !path.is_file() {
        cleanup_orphaned_pending_restore_bundles(database_path);
        return (None, None);
    }
    let result = (|| -> formatwright_core::Result<Uuid> {
        let bytes = std::fs::read(&path).map_err(desktop_storage_error)?;
        if bytes.len() > 64 * 1024 {
            return Err(formatwright_core::FormatWrightError::new(
                formatwright_core::ErrorCode::StorageFailed,
                formatwright_core::Stage::Store,
                "Pending desktop restore request exceeds 64 KiB",
                "Remove the pending restore request and schedule the restore again.",
            ));
        }
        let request = serde_json::from_slice::<DesktopPendingRestore>(&bytes).map_err(|error| {
            formatwright_core::FormatWrightError::new(
                formatwright_core::ErrorCode::StorageFailed,
                formatwright_core::Stage::Store,
                "Pending desktop restore request is invalid",
                "Remove the pending restore request and schedule the restore again.",
            )
            .with_diagnostic(error.to_string())
        })?;
        if request.schema_version != DESKTOP_PENDING_RESTORE_SCHEMA_VERSION {
            return Err(formatwright_core::FormatWrightError::new(
                formatwright_core::ErrorCode::InputInvalid,
                formatwright_core::Stage::Store,
                "Pending desktop restore request uses an unsupported version",
                "Update FormatWright and schedule the restore again.",
            ));
        }
        let current_size = std::fs::metadata(&request.bundle_path)
            .map_err(desktop_storage_error)?
            .len();
        let current_blake3 = blake3_file(&request.bundle_path)?;
        if current_size != request.bundle_size_bytes || current_blake3 != request.bundle_blake3 {
            return Err(formatwright_core::FormatWrightError::new(
                formatwright_core::ErrorCode::InputChanged,
                formatwright_core::Stage::Store,
                "The scheduled state bundle changed before restart",
                "Choose the bundle again and repeat restore preflight.",
            ));
        }
        let service = ApplicationStateService::from_database(database_path.to_path_buf())?;
        let preflight = service.restore_preflight(&request.bundle_path)?;
        if preflight.bundle_id != request.bundle_id {
            return Err(formatwright_core::FormatWrightError::new(
                formatwright_core::ErrorCode::InputChanged,
                formatwright_core::Stage::Store,
                "The scheduled state bundle changed before restart",
                "Choose the bundle again and repeat restore preflight.",
            ));
        }
        let restored = service.restore(&request.bundle_path)?;
        std::fs::remove_file(&path).map_err(desktop_storage_error)?;
        let _ = std::fs::remove_file(&request.bundle_path);
        Ok(restored.bundle_id)
    })();
    match result {
        Ok(bundle_id) => (Some(bundle_id), None),
        Err(error) => {
            cleanup_pending_restore_bundle(database_path, &path);
            let _ = std::fs::remove_file(&path);
            (None, Some(error.to_string()))
        }
    }
}

fn cleanup_orphaned_pending_restore_bundles(database_path: &Path) {
    let root = database_path.parent().unwrap_or_else(|| Path::new("."));
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.filter_map(std::result::Result::ok) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(".desktop-state-restore-")
            && name.ends_with(".fwstate")
            && entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn cleanup_pending_restore_bundle(database_path: &Path, request_path: &Path) {
    let Ok(bytes) = std::fs::read(request_path) else {
        return;
    };
    let Ok(request) = serde_json::from_slice::<DesktopPendingRestore>(&bytes) else {
        return;
    };
    let expected_parent = database_path.parent().unwrap_or_else(|| Path::new("."));
    let safe_name = request
        .bundle_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name == format!(".desktop-state-restore-{}.fwstate", request.bundle_id)
        });
    if safe_name && request.bundle_path.parent() == Some(expected_parent) {
        let _ = std::fs::remove_file(request.bundle_path);
    }
}

fn persist_pending_restore(
    path: &Path,
    request: &DesktopPendingRestore,
) -> formatwright_core::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        formatwright_core::FormatWrightError::new(
            formatwright_core::ErrorCode::StorageFailed,
            formatwright_core::Stage::Store,
            "Pending restore request has no parent directory",
            "Choose a valid application data directory.",
        )
    })?;
    std::fs::create_dir_all(parent).map_err(desktop_storage_error)?;
    let partial = parent.join(format!(".desktop-state-restore-{}.partial", Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(request).map_err(|error| {
        formatwright_core::FormatWrightError::new(
            formatwright_core::ErrorCode::Internal,
            formatwright_core::Stage::Store,
            "Pending restore request could not be serialized",
            "Retry scheduling the restore.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)
        .map_err(desktop_storage_error)?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        let _ = std::fs::remove_file(&partial);
        return Err(desktop_storage_error(error));
    }
    drop(file);
    if path.exists() {
        std::fs::remove_file(&partial).map_err(desktop_storage_error)?;
        return Err(formatwright_core::FormatWrightError::new(
            formatwright_core::ErrorCode::OutputConflict,
            formatwright_core::Stage::Store,
            "A desktop state restore is already scheduled",
            "Restart FormatWright before scheduling another restore.",
        ));
    }
    std::fs::rename(&partial, path).map_err(|error| {
        let _ = std::fs::remove_file(&partial);
        desktop_storage_error(error)
    })
}

#[allow(clippy::needless_pass_by_value)]
fn desktop_storage_error(error: std::io::Error) -> formatwright_core::FormatWrightError {
    formatwright_core::FormatWrightError::new(
        formatwright_core::ErrorCode::StorageFailed,
        formatwright_core::Stage::Store,
        "Desktop maintenance state could not be persisted",
        "Check the application data directory permissions and retry.",
    )
    .with_diagnostic(error.to_string())
}

fn blake3_file(path: &Path) -> formatwright_core::Result<String> {
    let mut file = std::fs::File::open(path).map_err(desktop_storage_error)?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher).map_err(desktop_storage_error)?;
    Ok(format!("blake3:{}", hasher.finalize().to_hex()))
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

fn recover_desktop_jobs(
    store: &mut SqliteJobStore,
) -> formatwright_core::Result<DesktopStartupRecovery> {
    let interrupted = store.interrupt_active_jobs()?;
    let mut removed_staged_outputs = 0;
    for job in &interrupted {
        if cleanup_staged_output(&job.output_path, job.id)? {
            removed_staged_outputs += 1;
        }
    }
    Ok(DesktopStartupRecovery {
        recovered_after_restart: interrupted.len(),
        removed_staged_outputs,
        restored_bundle_id: None,
        restore_error: None,
    })
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
            let job_database_path = data_directory.join("jobs.sqlite3");
            let (restored_bundle_id, restore_error) = apply_pending_restore(&job_database_path);
            let state_layout = ApplicationStateLayout::from_database(&job_database_path)
                .map_err(|error| Box::<dyn std::error::Error>::from(error.to_string()))?;
            ApplicationStateService::new(state_layout.clone())
                .recover_interrupted_restore()
                .map_err(|error| Box::<dyn std::error::Error>::from(error.to_string()))?;
            let reports_directory = state_layout.reports_directory;
            let engine_registry_directory = state_layout.engine_registry_directory;
            let engine_store_directory = data_directory.join("engines");
            let presets_path = state_layout.presets_path;
            let settings_path = state_layout.settings_path;
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
            let mut store = SqliteJobStore::open(&job_database_path)
                .map_err(|error| Box::<dyn std::error::Error>::from(error.to_string()))?;
            let mut startup_recovery = recover_desktop_jobs(&mut store)
                .map_err(|error| Box::<dyn std::error::Error>::from(error.to_string()))?;
            startup_recovery.restored_bundle_id = restored_bundle_id;
            startup_recovery.restore_error = restore_error;
            let presets =
                load_preset_library(&presets_path).map_err(Box::<dyn std::error::Error>::from)?;
            let settings = ApplicationSettingsService::new(&settings_path)
                .read()
                .map_err(|error| Box::<dyn std::error::Error>::from(error.to_string()))?;
            app.manage(DesktopState {
                store: Mutex::new(store),
                job_database_path,
                cancellations: Mutex::new(HashMap::new()),
                queue_control: Mutex::new(None),
                presets: Mutex::new(presets),
                presets_path,
                settings: Mutex::new(settings),
                settings_path,
                reports_directory,
                engine_registry_directory,
                engine_store_directory,
                startup_recovery,
                operation_gate: Mutex::new(DesktopOperationGate::default()),
                folder_previews: Mutex::new(HashMap::new()),
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
            preview_desktop_folder_batch,
            queue_desktop_folder_batch,
            run_desktop_queue_window,
            pause_desktop_queue_window,
            cancel_desktop_queue_window,
            cancel_desktop_job,
            requeue_desktop_job,
            list_desktop_jobs,
            query_desktop_jobs,
            get_desktop_recovery_summary,
            get_desktop_maintenance_status,
            check_desktop_integrity,
            backup_desktop_state,
            preflight_desktop_state_restore,
            schedule_desktop_state_restore,
            compact_desktop_database,
            list_desktop_batches,
            capture_desktop_job_selection,
            run_desktop_bulk_action,
            get_desktop_report,
            list_desktop_presets,
            save_desktop_preset,
            delete_desktop_preset,
            import_desktop_presets,
            export_desktop_presets,
            get_desktop_settings,
            save_desktop_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running FormatWright desktop");
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Duration;

    use tempfile::tempdir;

    use formatwright_core::{
        ApplicationStateService, ArtifactSummary, ChangeSet, ConversionPreset, JobState,
        NetworkPolicy, PRESET_SCHEMA_VERSION, Plan, PlanRequest, PresetLibrary, QueueWindowControl,
        ReportRedaction, ReportService, SqliteJobStore, StateBundleOptions, ValidationReport,
        ValidationStatus,
    };
    use uuid::Uuid;

    use super::{
        DesktopConversionRequest, DesktopEngineRegistryEntry, DesktopOperationGate,
        acquire_active_operation, acquire_maintenance_operation, acquire_queue_window,
        apply_pending_restore, backup_path, bundled_manifest_paths, load_preset_library,
        pending_restore_path, persist_preset_library, prepare_approved_desktop_conversion,
        recover_desktop_jobs, registered_manifest_paths, requeue_job, run_queue_window_on_database,
        stage_pending_restore,
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
            idempotency_key: None,
        }
    }

    fn structured_job_request(
        root: &std::path::Path,
        name: &str,
    ) -> formatwright_core::JobCreateRequest {
        let input = root.join(format!("{name}.json"));
        let output = root.join(format!("{name}.yaml"));
        fs::write(&input, r#"[{"id":1}]"#).expect("write structured input");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("planning runtime");
        let plan = runtime
            .block_on(formatwright_core::prepare_conversion(
                &input,
                &PlanRequest {
                    target_format: "yaml".to_owned(),
                    output_path: Some(output.clone()),
                    ..PlanRequest::default()
                },
            ))
            .expect("prepare structured conversion")
            .1;
        formatwright_core::JobCreateRequest {
            input_path: input,
            output_path: output,
            plan,
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
        let service = ReportService::new(directory.path());
        let first = report(job_id, ValidationStatus::Pass);
        service.save(job_id, &first).expect("write first report");
        let second = report(job_id, ValidationStatus::Warning);
        service.save(job_id, &second).expect("replace report");

        assert_eq!(service.read(job_id).expect("read report"), Some(second));
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
        let service = ReportService::new(directory.path());
        let first = report(job_id, ValidationStatus::Pass);
        service.save(job_id, &first).expect("write first report");
        let destination = directory.path().join(format!("{job_id}.json"));
        let backup = directory.path().join(format!(".{job_id}.backup"));
        fs::rename(&destination, &backup).expect("simulate interrupted replacement");

        let second = report(job_id, ValidationStatus::Warning);
        service
            .save(job_id, &second)
            .expect("recover and replace report");

        assert_eq!(service.read(job_id).expect("read report"), Some(second));
        assert!(!backup.exists());
    }

    #[test]
    fn report_failure_interrupts_job_before_terminal_state() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let job_id = validating_job(&mut store, suite.path());
        let blocked_reports = suite.path().join("reports-is-a-file");
        fs::write(&blocked_reports, b"not a directory").expect("write blocking file");

        let error = ReportService::new(&blocked_reports)
            .persist_before_terminal(
                &mut store,
                job_id,
                &report(job_id, ValidationStatus::Pass),
                "DESKTOP_CONVERSION_FINISHED",
            )
            .expect_err("report persistence must fail");

        assert!(
            error
                .message
                .contains("Unable to persist or read ValidationReport")
        );
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

        let report_service = ReportService::new(&reports);
        let job = report_service
            .persist_before_terminal(
                &mut store,
                job_id,
                &validation,
                "DESKTOP_CONVERSION_FINISHED",
            )
            .expect("persist and finish");

        assert_eq!(job.state, JobState::Completed);
        assert_eq!(
            report_service.read(job_id).expect("read report"),
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
    fn desktop_startup_interrupts_active_jobs_and_removes_exact_staging_artifacts() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let output = suite.path().join("recovered.yaml");
        let job = store
            .create_job(
                suite.path().join("recovered.json"),
                &output,
                &plan(output.clone()),
            )
            .expect("create job");
        store
            .transition(job.id, JobState::Running, "TEST_STARTED")
            .expect("start job");
        let staged = formatwright_core::staged_output_path(&output, job.id).expect("staged path");
        fs::write(&staged, b"incomplete").expect("write staged output");

        let recovery = recover_desktop_jobs(&mut store).expect("recover desktop jobs");

        assert_eq!(recovery.recovered_after_restart, 1);
        assert_eq!(recovery.removed_staged_outputs, 1);
        assert!(!staged.exists());
        let details = store
            .get_job_details(job.id)
            .expect("read details")
            .expect("job details");
        assert_eq!(details.job.state, JobState::Interrupted);
        assert_eq!(
            details.events.last().expect("recovery event").code,
            "RECOVERED_AFTER_RESTART"
        );
    }

    #[test]
    fn desktop_operation_gate_prevents_maintenance_overlap() {
        let gate = std::sync::Mutex::new(DesktopOperationGate::default());
        let active = acquire_active_operation(&gate).expect("start active operation");
        assert!(acquire_maintenance_operation(&gate).is_err());
        drop(active);

        let maintenance = acquire_maintenance_operation(&gate).expect("start maintenance");
        assert!(acquire_active_operation(&gate).is_err());
        assert!(acquire_maintenance_operation(&gate).is_err());
        drop(maintenance);

        assert!(acquire_active_operation(&gate).is_ok());
    }

    #[test]
    fn scheduled_desktop_restore_is_staged_verified_and_applied_before_open() {
        let suite = tempdir().expect("suite");
        let source_root = suite.path().join("source");
        let live_root = suite.path().join("live");
        fs::create_dir_all(&source_root).expect("source root");
        fs::create_dir_all(&live_root).expect("live root");
        let source_database = source_root.join("jobs.sqlite3");
        let live_database = live_root.join("jobs.sqlite3");
        drop(SqliteJobStore::open(&source_database).expect("source store"));
        drop(SqliteJobStore::open(&live_database).expect("live store"));
        let mut source = SqliteJobStore::open(&source_database).expect("source store");
        let mut live = SqliteJobStore::open(&live_database).expect("live store");
        source
            .create_batch(
                "source batch",
                &[structured_job_request(&source_root, "source")],
            )
            .expect("source marker");
        live.create_batch(
            "live batch 1",
            &[structured_job_request(&live_root, "live-1")],
        )
        .expect("live marker");
        live.create_batch(
            "live batch 2",
            &[structured_job_request(&live_root, "live-2")],
        )
        .expect("second live marker");
        drop(source);
        drop(live);
        let bundle = suite.path().join("portable.fwstate");
        let backup = ApplicationStateService::from_database(&source_database)
            .expect("source state")
            .backup(&bundle, StateBundleOptions::default())
            .expect("state backup");

        let scheduled = stage_pending_restore(&live_database, &bundle, backup.bundle_id)
            .expect("schedule restore");
        assert!(scheduled.restart_required);
        assert_ne!(scheduled.bundle_path, bundle);
        assert!(scheduled.bundle_path.is_file());
        assert!(pending_restore_path(&live_database).is_file());
        fs::write(&bundle, b"the original selected bundle may move or change")
            .expect("change original bundle after staging");

        let (restored, error) = apply_pending_restore(&live_database);
        assert_eq!(restored, Some(backup.bundle_id));
        assert_eq!(error, None);
        assert!(!pending_restore_path(&live_database).exists());
        assert!(!scheduled.bundle_path.exists());
        assert_eq!(
            SqliteJobStore::open(&live_database)
                .expect("restored store")
                .list_batches_page(10, 0)
                .expect("restored batches")
                .len(),
            1
        );
    }

    #[test]
    fn tampered_scheduled_restore_preserves_live_database() {
        let suite = tempdir().expect("suite");
        let source_database = suite.path().join("source/jobs.sqlite3");
        let live_database = suite.path().join("live/jobs.sqlite3");
        fs::create_dir_all(source_database.parent().expect("source parent")).expect("source root");
        fs::create_dir_all(live_database.parent().expect("live parent")).expect("live root");
        SqliteJobStore::open(&source_database).expect("source store");
        let mut live = SqliteJobStore::open(&live_database).expect("live store");
        live.create_batch(
            "keep",
            &[structured_job_request(
                live_database.parent().expect("live root"),
                "keep",
            )],
        )
        .expect("live marker");
        drop(live);
        let bundle = suite.path().join("portable.fwstate");
        let backup = ApplicationStateService::from_database(&source_database)
            .expect("source state")
            .backup(&bundle, StateBundleOptions::default())
            .expect("state backup");
        let scheduled = stage_pending_restore(&live_database, &bundle, backup.bundle_id)
            .expect("schedule restore");
        let mut file = OpenOptions::new()
            .append(true)
            .open(&scheduled.bundle_path)
            .expect("open staged bundle");
        file.write_all(b"tamper").expect("tamper bundle");
        drop(file);

        let (restored, error) = apply_pending_restore(&live_database);
        assert_eq!(restored, None);
        assert!(
            error
                .expect("restore error")
                .contains("changed before restart")
        );
        assert!(!pending_restore_path(&live_database).exists());
        assert!(!scheduled.bundle_path.exists());
        assert_eq!(
            SqliteJobStore::open(&live_database)
                .expect("unchanged live store")
                .list_batches_page(10, 0)
                .expect("live batches")
                .len(),
            1
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
