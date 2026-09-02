#![forbid(unsafe_code)]

mod queue_bridge;
mod shell_convert;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use formatwright_core::{
    ApplicationSettings, ApplicationSettingsService, ApplicationStateLayout,
    ApplicationStateService, BatchRecord, BulkActionReport, BulkJobAction, BulkJobService,
    CapabilitySnapshot, Certification, CompactReport, ConversionPreset, ConversionService,
    DoctorReport, EngineDiscoveryPolicy, EngineRegistry, FolderBatchService, FolderDiskBudget,
    FolderMappingEntry, IntegrityReport, JobCreateRequest, JobExecutionService, JobQueryPage,
    JobRecord, JobRecoveryService, JobSelectionQuery, JobState, JobStateCount, MaintenanceService,
    MaintenanceStatus, PRESET_SCHEMA_VERSION, Plan, PlanRequest, PresetLibrary, Probe,
    QueueProgressUpdate, QueueRunReport, QueueWindowControl, ReportService, RevalidationService,
    SelectionSnapshot, SignatureTrust, SqliteJobStore, StagedCleanupReport,
    StateBundleBackupReport, StateBundleOptions, StateBundlePreflightReport,
    SupplyChainReviewStatus, ValidationReport, VerifiedEnginePack, activate_engine_pack,
    capability_snapshot_for_input, cleanup_staged_output, prepare_conversion,
};
use queue_bridge::{DEFAULT_BATCH_JOBS, DEFAULT_BENCHMARK_JOBS, QueueBatchIter};
use serde::{Deserialize, Serialize};
use shell_convert::{
    CONVERT_MERGE_QUIET, DesktopShellOpenBatch, ShellConvertCoordinator, plan_convert_outputs,
    should_run_immediately, surviving_convert_items,
};
use tauri::{Emitter, Manager};
use tempfile::TempPath;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const MAX_PENDING_SHELL_OPEN_PATHS: usize = 32;

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
    revalidations: Mutex<HashSet<Uuid>>,
    shell_open_paths: Arc<Mutex<VecDeque<DesktopShellOpen>>>,
    convert_batches: Arc<Mutex<ShellConvertCoordinator>>,
}

#[derive(Clone, Debug, Default, Serialize)]
struct DesktopStartupRecovery {
    recovered_after_restart: usize,
    removed_staged_outputs: usize,
    restored_bundle_id: Option<Uuid>,
    restore_error: Option<String>,
    #[serde(default)]
    engine_recovery: Vec<formatwright_core::EngineRecovery>,
}

#[derive(Clone, Debug, Serialize)]
struct DesktopRecoverySummary {
    recovered_after_restart: usize,
    removed_staged_outputs: usize,
    restored_bundle_id: Option<Uuid>,
    restore_error: Option<String>,
    engine_recovery: Vec<formatwright_core::EngineRecovery>,
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
const DESKTOP_JOB_PAGE_LIMIT: usize = 100;
const DESKTOP_EXPORT_SCHEMA_VERSION: u16 = 1;
const MAX_DESKTOP_EXPORT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
struct DesktopRecipeExport {
    schema_version: u16,
    job_id: Uuid,
    input_path: PathBuf,
    output_path: PathBuf,
    plan: Plan,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct DesktopShellOpen {
    path: PathBuf,
    directory: bool,
    convert_to: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct DesktopDropClassification {
    kind: String,
    path: Option<PathBuf>,
}

const ALLOWED_SHELL_CONVERT_TARGETS: &[&str] = &[
    "jpg", "png", "webp", "avif", "mp4", "mp3", "m4a", "wav", "gif", "pdf", "docx", "json", "csv",
    "yaml", "xml",
];

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
    disk_budget: FolderDiskBudget,
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

struct DesktopRevalidationLease<'a> {
    active: &'a Mutex<HashSet<Uuid>>,
    job_id: Uuid,
}

impl Drop for DesktopRevalidationLease<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(&self.job_id);
        }
    }
}

fn acquire_revalidation(
    active: &Mutex<HashSet<Uuid>>,
    job_id: Uuid,
) -> Result<DesktopRevalidationLease<'_>, String> {
    let mut current = lock(active)?;
    if !current.insert(job_id) {
        return Err("this job is already being revalidated".to_owned());
    }
    drop(current);
    Ok(DesktopRevalidationLease { active, job_id })
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
    video_crf: Option<u8>,
    video_preset: Option<String>,
    audio_bitrate_kbps: Option<u32>,
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
            video_crf: self.video_crf,
            video_preset: self.video_preset,
            audio_bitrate_kbps: self.audio_bitrate_kbps,
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
    video_crf: Option<u8>,
    video_preset: Option<String>,
    audio_bitrate_kbps: Option<u32>,
    audio_stream_index: Option<u32>,
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
            video_crf: self.video_crf,
            video_preset: self.video_preset.clone(),
            audio_bitrate_kbps: self.audio_bitrate_kbps,
            audio_stream_index: self.audio_stream_index,
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

#[derive(Clone, Debug, Serialize)]
struct DesktopIngestResult {
    ran_immediately: bool,
    batch_id: Option<Uuid>,
    queued: usize,
    job: Option<JobRecord>,
    report: Option<ValidationReport>,
    skipped_conflict: usize,
    skipped_disk: usize,
    rejected: usize,
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
    signature_trust: Option<SignatureTrust>,
    review_status: SupplyChainReviewStatus,
    certification: Certification,
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
    EngineRegistry::new(
        state.engine_registry_directory.clone(),
        state.engine_store_directory.clone(),
    )
    .set_active(&verified)
    .map_err(serialize_error)?;
    Ok(valid_engine_summary(&verified))
}

#[tauri::command]
async fn list_imported_engine_packs(
    state: tauri::State<'_, DesktopState>,
) -> Result<Vec<DesktopEnginePackSummary>, String> {
    let paths = EngineRegistry::new(
        state.engine_registry_directory.clone(),
        state.engine_store_directory.clone(),
    )
    .active_entries()
    .map_err(serialize_error)?
    .into_iter()
    .map(|entry| entry.manifest_path)
    .collect::<Vec<_>>();
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
                signature_trust: None,
                review_status: SupplyChainReviewStatus::Missing,
                certification: Certification::Unverified,
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
            let _ = window.emit(
                "formatwright://job-progress",
                &QueueProgressUpdate {
                    schema_version: 1,
                    job_id: job.id,
                    job_sequence: job.sequence,
                    state: job.state,
                    wait_reason: None,
                    occurred_unix_ms: job.updated_unix_ms,
                    eta_milliseconds: None,
                },
            );
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

fn empty_ingest_result(
    skipped_conflict: usize,
    skipped_disk: usize,
    rejected: usize,
) -> DesktopIngestResult {
    DesktopIngestResult {
        ran_immediately: false,
        batch_id: None,
        queued: 0,
        job: None,
        report: None,
        skipped_conflict,
        skipped_disk,
        rejected,
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn ingest_shell_convert(
    store: &Mutex<SqliteJobStore>,
    job_database_path: &Path,
    reports_directory: &Path,
    queue_control: &Mutex<Option<QueueWindowControl>>,
    cancellations: &Mutex<HashMap<Uuid, CancellationToken>>,
    window: Option<&tauri::WebviewWindow>,
    paths: Vec<PathBuf>,
    target: String,
) -> Result<DesktopIngestResult, String> {
    let planned = plan_convert_outputs(&paths, &target);
    let skipped_conflict = planned.iter().filter(|item| item.skipped_conflict).count();
    let rejected_files = planned.iter().filter(|item| item.rejected).count();
    let mut requests = Vec::new();
    let mut rejected = rejected_files;
    for item in surviving_convert_items(&planned) {
        let plan_request = PlanRequest {
            target_format: target.clone(),
            output_path: Some(item.output.clone()),
            preserve_all_streams: true,
            ..PlanRequest::default()
        };
        match prepare_conversion(&item.input, &plan_request).await {
            Ok((_probe, plan, _)) => requests.push(JobCreateRequest {
                input_path: item.input.clone(),
                output_path: item.output.clone(),
                plan,
            }),
            Err(_) => rejected += 1,
        }
    }
    let mut skipped_disk = 0;
    if !requests.is_empty() {
        let mut kept = Vec::new();
        let mut by_parent: HashMap<PathBuf, Vec<JobCreateRequest>> = HashMap::new();
        for request in requests {
            let parent = request
                .output_path
                .parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
            by_parent.entry(parent).or_default().push(request);
        }
        for (parent, group) in by_parent {
            match FolderBatchService::disk_budget(&parent, &group, 4) {
                Ok(budget) if budget.sufficient => kept.extend(group),
                _ => skipped_disk += group.len(),
            }
        }
        requests = kept;
    }
    if requests.is_empty() {
        return Ok(empty_ingest_result(
            skipped_conflict,
            skipped_disk,
            rejected,
        ));
    }
    let queue_busy = queue_control.lock().map_or(true, |guard| guard.is_some());
    if should_run_immediately(paths.len(), queue_busy) && requests.len() == 1 {
        let request = requests.remove(0);
        let prepared = prepare_conversion(
            &request.input_path,
            &PlanRequest {
                target_format: target,
                output_path: Some(request.output_path.clone()),
                preserve_all_streams: true,
                ..PlanRequest::default()
            },
        )
        .await
        .map_err(serialize_error)?;
        formatwright_core::ensure_plan_approved(&prepared.1, Some(prepared.1.plan_hash.as_str()))
            .map_err(serialize_error)?;
        let cancellation = CancellationToken::new();
        let mut execution_store =
            SqliteJobStore::open(job_database_path).map_err(serialize_error)?;
        let result = ConversionService::run_prepared(
            &mut execution_store,
            &ReportService::new(reports_directory),
            &prepared.0,
            &prepared.1,
            &prepared.2,
            &prepared.1.plan_hash,
            cancellation.clone(),
            |job| {
                if job.state == JobState::Running
                    && let Ok(mut tokens) = cancellations.lock()
                {
                    tokens.insert(job.id, cancellation.clone());
                }
                if let Some(window) = window {
                    let _ = window.emit("formatwright://job-updated", job);
                }
            },
        )
        .await
        .map_err(serialize_error)?;
        if let Ok(mut tokens) = cancellations.lock() {
            tokens.remove(&result.job.id);
        }
        return Ok(DesktopIngestResult {
            ran_immediately: true,
            batch_id: None,
            queued: 0,
            job: Some(result.job),
            report: Some(result.report),
            skipped_conflict,
            skipped_disk,
            rejected,
        });
    }
    let batch = {
        let mut store = lock(store)?;
        store
            .create_queued_batch(
                &format!("Explorer: {} files → {target}", requests.len()),
                &requests,
            )
            .map_err(serialize_error)?
    };
    if let Some(window) = window {
        let _ = window.emit("formatwright://job-updated", ());
    }
    Ok(DesktopIngestResult {
        ran_immediately: false,
        batch_id: Some(batch.id),
        queued: requests.len(),
        job: None,
        report: None,
        skipped_conflict,
        skipped_disk,
        rejected,
    })
}

#[tauri::command]
async fn ingest_shell_convert_paths(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    paths: Vec<PathBuf>,
    target: String,
) -> Result<DesktopIngestResult, String> {
    ingest_shell_convert(
        &state.store,
        &state.job_database_path,
        &state.reports_directory,
        &state.queue_control,
        &state.cancellations,
        Some(&window),
        paths,
        target,
    )
    .await
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn take_desktop_shell_convert_batch(
    state: tauri::State<'_, DesktopState>,
) -> Option<DesktopShellOpenBatch> {
    state.convert_batches.lock().ok()?.take_ready()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn desktop_queue_window_busy(state: tauri::State<'_, DesktopState>) -> bool {
    state
        .queue_control
        .lock()
        .is_ok_and(|guard| guard.is_some())
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
    let disk_budget = FolderBatchService::disk_budget(&mapping.output_root, &requests, 4)
        .map_err(serialize_error)?;
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
        disk_budget: disk_budget.clone(),
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
    let current_budget =
        FolderBatchService::disk_budget(&cache.preview.output_root, &cache.requests, 4)
            .map_err(serialize_error)?;
    if !current_budget.sufficient {
        return Err(serialize_error(formatwright_core::FormatWrightError::new(
            formatwright_core::ErrorCode::ResourceExhausted,
            formatwright_core::Stage::Store,
            format!(
                "Folder batch requires {} bytes but only {} bytes are available",
                current_budget.required_bytes, current_budget.available_bytes
            ),
            "Free disk space or choose another output root, then preview again.",
        )));
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

async fn run_queue_window_on_database<F, P>(
    database_path: &Path,
    limit: usize,
    parallel: usize,
    control: QueueWindowControl,
    on_report: F,
    on_progress: P,
) -> formatwright_core::Result<QueueRunReport>
where
    F: FnMut(Uuid, &ValidationReport) -> formatwright_core::Result<()>,
    P: FnMut(QueueProgressUpdate),
{
    let mut queue_store = SqliteJobStore::open(database_path)?;
    JobExecutionService::run_window_observed_with_progress(
        &mut queue_store,
        limit,
        parallel,
        control,
        on_report,
        on_progress,
    )
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
    let progress_window = window.clone();
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
        move |progress| {
            let _ = progress_window.emit("formatwright://job-progress", &progress);
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
async fn cleanup_desktop_job_staging(
    state: tauri::State<'_, DesktopState>,
    job_id: String,
) -> Result<StagedCleanupReport, String> {
    let _operation = acquire_active_operation(&state.operation_gate)?;
    let job_id = Uuid::parse_str(&job_id).map_err(|error| error.to_string())?;
    let database_path = state.job_database_path.clone();
    tokio::task::spawn_blocking(move || {
        let mut store = SqliteJobStore::open(database_path)?;
        JobRecoveryService::cleanup_staging(&mut store, job_id)
    })
    .await
    .map_err(|error| format!("staging-cleanup worker failed: {error}"))?
    .map_err(serialize_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn list_desktop_jobs(
    state: tauri::State<'_, DesktopState>,
    limit: Option<usize>,
) -> Result<Vec<JobRecord>, String> {
    lock(&state.store)?
        .list_jobs(desktop_job_page_limit(limit))
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
            desktop_job_page_limit(limit),
            offset.unwrap_or_default(),
        )
        .map_err(serialize_error)
}

fn desktop_job_page_limit(requested: Option<usize>) -> usize {
    requested
        .unwrap_or(DESKTOP_JOB_PAGE_LIMIT)
        .clamp(1, DESKTOP_JOB_PAGE_LIMIT)
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
        engine_recovery: state.startup_recovery.engine_recovery.clone(),
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
    if let Some(record) = lock(&state.store)?
        .latest_revalidation(job_id)
        .map_err(serialize_error)?
    {
        return Ok(Some(record.report));
    }
    ReportService::new(&state.reports_directory)
        .read(job_id)
        .map_err(serialize_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn export_desktop_report(
    state: tauri::State<'_, DesktopState>,
    job_id: String,
    destination_path: PathBuf,
    redact_paths: Option<bool>,
) -> Result<u64, String> {
    let job_id = Uuid::parse_str(&job_id).map_err(|error| error.to_string())?;
    let report = if let Some(record) = lock(&state.store)?
        .latest_revalidation(job_id)
        .map_err(serialize_error)?
    {
        record.report
    } else {
        ReportService::new(&state.reports_directory)
            .read(job_id)
            .map_err(serialize_error)?
            .ok_or_else(|| serialize_error(desktop_missing_job_artifact("ValidationReport")))?
    };
    let report = report_for_export(report, redact_paths.unwrap_or(true));
    let bytes = serde_json::to_vec_pretty(&report).map_err(|error| {
        serialize_error(desktop_export_error(
            formatwright_core::ErrorCode::StorageFailed,
            "ValidationReport could not be serialized for export",
            "Retry the export or restore a valid report from backup.",
            Some(error.to_string()),
        ))
    })?;
    write_desktop_export_noclobber(&destination_path, &bytes).map_err(serialize_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn export_desktop_recipe(
    state: tauri::State<'_, DesktopState>,
    job_id: String,
    destination_path: PathBuf,
) -> Result<u64, String> {
    let job_id = Uuid::parse_str(&job_id).map_err(|error| error.to_string())?;
    let details = lock(&state.store)?
        .get_job_details(job_id)
        .map_err(serialize_error)?
        .ok_or_else(|| serialize_error(desktop_missing_job_artifact("job")))?;
    let recipe = DesktopRecipeExport {
        schema_version: DESKTOP_EXPORT_SCHEMA_VERSION,
        job_id,
        input_path: details.job.input_path,
        output_path: details.job.output_path,
        plan: details.plan,
    };
    let bytes = serde_json::to_vec_pretty(&recipe).map_err(|error| {
        serialize_error(desktop_export_error(
            formatwright_core::ErrorCode::StorageFailed,
            "Job recipe could not be serialized for export",
            "Retry the export or restore a valid job database from backup.",
            Some(error.to_string()),
        ))
    })?;
    write_desktop_export_noclobber(&destination_path, &bytes).map_err(serialize_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn reveal_desktop_job_output(
    state: tauri::State<'_, DesktopState>,
    job_id: String,
) -> Result<(), String> {
    let job_id = Uuid::parse_str(&job_id).map_err(|error| error.to_string())?;
    let job = lock(&state.store)?
        .get_job(job_id)
        .map_err(serialize_error)?
        .ok_or_else(|| serialize_error(desktop_missing_job_artifact("job")))?;
    reveal_existing_output(&job.output_path).map_err(serialize_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
async fn revalidate_desktop_job(
    state: tauri::State<'_, DesktopState>,
    job_id: String,
) -> Result<ValidationReport, String> {
    let _operation = acquire_active_operation(&state.operation_gate)?;
    let job_id = Uuid::parse_str(&job_id).map_err(|error| error.to_string())?;
    let _revalidation = acquire_revalidation(&state.revalidations, job_id)?;
    let details = lock(&state.store)?
        .get_job_details(job_id)
        .map_err(serialize_error)?
        .ok_or_else(|| serialize_error(desktop_missing_job_artifact("job")))?;
    if !matches!(
        details.job.state,
        JobState::Completed | JobState::Warning | JobState::Failed
    ) {
        return Err(serialize_error(desktop_export_error(
            formatwright_core::ErrorCode::PolicyBlocked,
            "Only completed, warning, or validation-failed jobs can be revalidated",
            "Finish the conversion successfully before running validation-only.",
            None,
        )));
    }
    let original_report = ReportService::new(&state.reports_directory)
        .read(job_id)
        .map_err(serialize_error)?
        .ok_or_else(|| serialize_error(desktop_missing_job_artifact("ValidationReport")))?;
    if original_report.plan_hash != details.job.plan_hash
        || details.plan.plan_hash != details.job.plan_hash
    {
        return Err(serialize_error(desktop_export_error(
            formatwright_core::ErrorCode::InputChanged,
            "Stored conversion evidence does not match the immutable Plan",
            "Run an integrity check and restore a consistent application-state backup.",
            None,
        )));
    }
    let report = RevalidationService::revalidate(
        &details.job.input_path,
        &details.job.output_path,
        &details.plan,
        job_id,
        CancellationToken::new(),
    )
    .await
    .map_err(serialize_error)?;
    lock(&state.store)?
        .record_revalidation(job_id, &report)
        .map_err(serialize_error)?;
    Ok(report)
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

fn normalize_shell_convert_target(value: &str) -> Option<String> {
    let normalized = value.trim().trim_start_matches('.').to_ascii_lowercase();
    let normalized = match normalized.as_str() {
        "jpeg" => "jpg".to_owned(),
        "yml" => "yaml".to_owned(),
        other => other.to_owned(),
    };
    ALLOWED_SHELL_CONVERT_TARGETS
        .contains(&normalized.as_str())
        .then_some(normalized)
}

fn parse_shell_invocation<I, S>(arguments: I) -> Option<(PathBuf, Option<String>)>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString>,
{
    let mut arguments = arguments.into_iter().map(Into::into);
    let _executable = arguments.next()?;
    let mut path = None;
    let mut convert_to = None;
    let mut saw_open = false;
    let mut saw_convert = false;
    while let Some(argument) = arguments.next() {
        if argument == "--shell-open" {
            saw_open = true;
            path = arguments.next().map(PathBuf::from);
        } else if argument == "--shell-convert" {
            saw_convert = true;
        } else if argument == "--to" {
            convert_to = arguments
                .next()
                .and_then(|value| normalize_shell_convert_target(&value.to_string_lossy()));
        } else if path.is_none()
            && (saw_open || saw_convert)
            && !argument.to_string_lossy().starts_with('-')
        {
            path = Some(PathBuf::from(argument));
        }
    }
    let path = path?;
    if saw_convert {
        return Some((path, Some(convert_to?)));
    }
    if saw_open {
        return Some((path, None));
    }
    None
}

#[cfg(test)]
fn shell_open_path_from_args<I, S>(arguments: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString>,
{
    let (path, convert_to) = parse_shell_invocation(arguments)?;
    convert_to.is_none().then_some(path)
}

fn path_is_local_absolute(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    #[cfg(windows)]
    {
        matches!(
            path.components().next(),
            Some(std::path::Component::Prefix(prefix))
                if matches!(
                    prefix.kind(),
                    std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                )
        )
    }
    #[cfg(not(windows))]
    {
        true
    }
}

fn canonical_is_local_disk(path: &Path) -> bool {
    #[cfg(windows)]
    {
        matches!(
            path.components().next(),
            Some(std::path::Component::Prefix(prefix))
                if matches!(prefix.kind(), std::path::Prefix::VerbatimDisk(_))
        )
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        true
    }
}

fn classify_local_absolute_path(requested: &Path) -> DesktopDropClassification {
    if !path_is_local_absolute(requested) {
        return DesktopDropClassification {
            kind: "rejected".to_owned(),
            path: None,
        };
    }
    let Ok(canonical) = requested.canonicalize() else {
        return DesktopDropClassification {
            kind: "rejected".to_owned(),
            path: None,
        };
    };
    if !canonical_is_local_disk(&canonical) {
        return DesktopDropClassification {
            kind: "rejected".to_owned(),
            path: None,
        };
    }
    if canonical.is_file() {
        return DesktopDropClassification {
            kind: "file".to_owned(),
            path: Some(requested.to_path_buf()),
        };
    }
    if canonical.is_dir() {
        return DesktopDropClassification {
            kind: "directory".to_owned(),
            path: Some(requested.to_path_buf()),
        };
    }
    DesktopDropClassification {
        kind: "rejected".to_owned(),
        path: None,
    }
}

fn validated_shell_request(
    arguments: impl IntoIterator<Item = std::ffi::OsString>,
) -> Option<DesktopShellOpen> {
    let (requested, convert_to) = parse_shell_invocation(arguments)?;
    let classified = classify_local_absolute_path(&requested);
    match classified.kind.as_str() {
        "file" => Some(DesktopShellOpen {
            path: requested,
            directory: false,
            convert_to,
        }),
        "directory" if convert_to.is_none() => Some(DesktopShellOpen {
            path: requested,
            directory: true,
            convert_to: None,
        }),
        _ => None,
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri commands receive owned IPC values.
fn classify_desktop_drop_path(path: PathBuf) -> DesktopDropClassification {
    classify_local_absolute_path(&path)
}

#[cfg(test)]
fn validated_shell_open_path(
    arguments: impl IntoIterator<Item = std::ffi::OsString>,
) -> Option<PathBuf> {
    let request = validated_shell_request(arguments)?;
    request.convert_to.is_none().then_some(request.path)
}

fn desktop_shell_open_from_args(
    arguments: impl IntoIterator<Item = std::ffi::OsString>,
) -> Option<DesktopShellOpen> {
    validated_shell_request(arguments)
}

fn enqueue_shell_request(pending: &Mutex<VecDeque<DesktopShellOpen>>, request: DesktopShellOpen) {
    if let Ok(mut pending) = pending.lock() {
        if pending.len() >= MAX_PENDING_SHELL_OPEN_PATHS {
            pending.pop_front();
        }
        pending.push_back(request);
    }
}

#[cfg(test)]
fn enqueue_shell_open_path(pending: &Mutex<VecDeque<DesktopShellOpen>>, path: PathBuf) {
    enqueue_shell_request(
        pending,
        DesktopShellOpen {
            path,
            directory: false,
            convert_to: None,
        },
    );
}

fn schedule_convert_quiet_flush(
    app: tauri::AppHandle,
    coordinator: Arc<Mutex<ShellConvertCoordinator>>,
    generation: u64,
) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(CONVERT_MERGE_QUIET).await;
        let flushed = coordinator
            .lock()
            .is_ok_and(|mut guard| guard.generation == generation && guard.flush_quiet().is_some());
        if flushed {
            let _ = app.emit("formatwright://shell-convert-batch", ());
        }
    });
}

fn accept_desktop_shell_request(
    app: &tauri::AppHandle,
    shell_open_paths: &Mutex<VecDeque<DesktopShellOpen>>,
    convert_batches: &Arc<Mutex<ShellConvertCoordinator>>,
    request: DesktopShellOpen,
) {
    if let Some(target) = request.convert_to.clone() {
        let (outcome, generation) = {
            let Ok(mut coordinator) = convert_batches.lock() else {
                return;
            };
            let outcome = coordinator.push(target, request.path);
            (outcome, coordinator.generation)
        };
        if outcome.flushed_ready {
            let _ = app.emit("formatwright://shell-convert-batch", ());
        }
        schedule_convert_quiet_flush(app.clone(), Arc::clone(convert_batches), generation);
        return;
    }
    enqueue_shell_request(shell_open_paths, request);
    let _ = app.emit("formatwright://shell-open-requested", ());
}

fn handle_second_instance(
    app: &tauri::AppHandle,
    arguments: Vec<String>,
    shell_open_paths: &Mutex<VecDeque<DesktopShellOpen>>,
    convert_batches: &Arc<Mutex<ShellConvertCoordinator>>,
) {
    if let Some(shell_open) =
        desktop_shell_open_from_args(arguments.into_iter().map(std::ffi::OsString::from))
    {
        accept_desktop_shell_request(app, shell_open_paths, convert_batches, shell_open);
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri commands receive managed state through this extractor.
fn get_desktop_shell_open(state: tauri::State<'_, DesktopState>) -> Option<DesktopShellOpen> {
    let mut pending = state.shell_open_paths.lock().ok()?;
    while let Some(request) = pending.pop_front() {
        if request.convert_to.is_none() {
            return Some(request);
        }
    }
    None
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value, clippy::unnecessary_wraps)]
fn show_desktop_toast(app: tauri::AppHandle, title: String, body: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        let escaped_title = title.replace('\'', "''");
        let escaped_body = body.replace('\'', "''");
        let script = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; $template = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); $text = $template.GetElementsByTagName('text'); $text.Item(0).AppendChild($template.CreateTextNode('{escaped_title}')) > $null; $text.Item(1).AppendChild($template.CreateTextNode('{escaped_body}')) > $null; $toast = [Windows.UI.Notifications.ToastNotification]::new($template); [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('FormatWright').Show($toast)"
        );
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status();
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (app, title, body);
        Ok(())
    }
}

fn report_for_export(mut report: ValidationReport, redact_paths: bool) -> ValidationReport {
    if redact_paths {
        report.input.display_path = None;
        report.output.display_path = None;
        report.redaction.paths_redacted = true;
    }
    report
}

fn write_desktop_export_noclobber(
    destination: &Path,
    bytes: &[u8],
) -> formatwright_core::Result<u64> {
    if bytes.len() > MAX_DESKTOP_EXPORT_BYTES {
        return Err(desktop_export_error(
            formatwright_core::ErrorCode::ResourceExhausted,
            "Desktop JSON export exceeds the 16 MiB limit",
            "Export a smaller report or recipe.",
            None,
        ));
    }
    if destination.file_name().is_none() {
        return Err(desktop_export_error(
            formatwright_core::ErrorCode::InputInvalid,
            "Desktop JSON export needs a file destination",
            "Choose a JSON filename inside an existing local directory.",
            None,
        ));
    }
    let parent = destination.parent().ok_or_else(|| {
        desktop_export_error(
            formatwright_core::ErrorCode::InputInvalid,
            "Desktop JSON export destination has no parent directory",
            "Choose a JSON filename inside an existing local directory.",
            None,
        )
    })?;
    let file_name = destination
        .file_name()
        .ok_or_else(|| {
            desktop_export_error(
                formatwright_core::ErrorCode::InputInvalid,
                "Desktop JSON export needs a file destination",
                "Choose a JSON filename inside an existing local directory.",
                None,
            )
        })?
        .to_os_string();
    let parent = parent.canonicalize().map_err(|error| {
        desktop_export_error(
            formatwright_core::ErrorCode::InputInvalid,
            "Desktop JSON export directory is unavailable",
            "Choose an existing writable local directory.",
            Some(error.to_string()),
        )
    })?;
    let destination = parent.join(file_name);
    if destination.exists() {
        return Err(desktop_export_error(
            formatwright_core::ErrorCode::OutputConflict,
            "Desktop JSON export will not overwrite an existing file",
            "Choose another filename or remove the existing file first.",
            None,
        ));
    }

    let partial = parent.join(format!(".formatwright-export-{}.partial", Uuid::new_v4()));
    let write_result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&partial);
        return Err(desktop_export_error(
            formatwright_core::ErrorCode::StorageFailed,
            "Desktop JSON export could not be persisted",
            "Check destination permissions and available storage, then retry.",
            Some(error.to_string()),
        ));
    }
    let temporary = TempPath::try_from_path(partial).map_err(|error| {
        desktop_export_error(
            formatwright_core::ErrorCode::StorageFailed,
            "Desktop JSON export staging path is invalid",
            "Choose another local destination and retry.",
            Some(error.to_string()),
        )
    })?;
    if let Err(error) = temporary.persist_noclobber(&destination) {
        return Err(desktop_export_error(
            if destination.exists() {
                formatwright_core::ErrorCode::OutputConflict
            } else {
                formatwright_core::ErrorCode::StorageFailed
            },
            "Desktop JSON export could not be committed without overwriting",
            "Choose another filename and retry.",
            Some(error.error.to_string()),
        ));
    }
    Ok(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
}

fn reveal_existing_output(path: &Path) -> formatwright_core::Result<()> {
    let path = path.canonicalize().map_err(|error| {
        desktop_export_error(
            formatwright_core::ErrorCode::InputInvalid,
            "The job output is no longer available",
            "Restore the output or run the conversion again.",
            Some(error.to_string()),
        )
    })?;
    let mut command = platform_reveal_command(&path);
    command.spawn().map_err(|error| {
        desktop_export_error(
            formatwright_core::ErrorCode::ExecutionFailed,
            "The operating-system file browser could not be opened",
            "Open the output path manually from the report.",
            Some(error.to_string()),
        )
    })?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn platform_reveal_command(path: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("explorer.exe");
    command.arg("/select,").arg(path);
    command
}

#[cfg(target_os = "macos")]
fn platform_reveal_command(path: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("open");
    command.arg("-R").arg(path);
    command
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_reveal_command(path: &Path) -> std::process::Command {
    let target = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let mut command = std::process::Command::new("xdg-open");
    command.arg(target);
    command
}

fn desktop_missing_job_artifact(artifact: &str) -> formatwright_core::FormatWrightError {
    desktop_export_error(
        formatwright_core::ErrorCode::InputInvalid,
        format!("The requested {artifact} was not found"),
        "Refresh the job list and select an existing completed job.",
        None,
    )
}

fn desktop_export_error(
    code: formatwright_core::ErrorCode,
    message: impl Into<String>,
    action: impl Into<String>,
    diagnostic: Option<String>,
) -> formatwright_core::FormatWrightError {
    let error = formatwright_core::FormatWrightError::new(
        code,
        formatwright_core::Stage::Store,
        message,
        action,
    );
    match diagnostic {
        Some(diagnostic) => error.with_diagnostic(diagnostic),
        None => error,
    }
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
        signature_trust: verified.signature_trust.clone(),
        review_status: verified.review_status,
        certification: verified.certification(),
        valid: true,
        message: verified.provenance_message(),
    }
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
        EngineRegistry::new(engine_registry_directory, engine_store_directory)
            .set_active(&verified)
            .map_err(|error| error.to_string())?;
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
        engine_recovery: Vec::new(),
    })
}

fn setup_desktop(
    app: &mut tauri::App,
    shell_open_paths: Arc<Mutex<VecDeque<DesktopShellOpen>>>,
    convert_batches: Arc<Mutex<ShellConvertCoordinator>>,
) -> Result<(), Box<dyn std::error::Error>> {
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
    let engine_recovery = EngineRegistry::new(
        engine_registry_directory.clone(),
        engine_store_directory.clone(),
    )
    .recover()
    .map_err(Box::<dyn std::error::Error>::from)?;
    if engine_recovery
        .iter()
        .any(|outcome| matches!(outcome, formatwright_core::EngineRecovery::Failed { .. }))
    {
        // A failed engine disables its routes until a working pack is
        // imported; it must never be silently skipped (ADR-0011 item 6).
        eprintln!("engine recovery reported failures: {engine_recovery:?}");
    }
    let mut store = SqliteJobStore::open(&job_database_path)
        .map_err(|error| Box::<dyn std::error::Error>::from(error.to_string()))?;
    let mut startup_recovery = recover_desktop_jobs(&mut store)
        .map_err(|error| Box::<dyn std::error::Error>::from(error.to_string()))?;
    startup_recovery.restored_bundle_id = restored_bundle_id;
    startup_recovery.restore_error = restore_error;
    startup_recovery.engine_recovery = engine_recovery;
    let presets = load_preset_library(&presets_path).map_err(Box::<dyn std::error::Error>::from)?;
    let settings = ApplicationSettingsService::new(&settings_path)
        .read()
        .map_err(|error| Box::<dyn std::error::Error>::from(error.to_string()))?;
    if let Some(request) = validated_shell_request(std::env::args_os()) {
        accept_desktop_shell_request(app.handle(), &shell_open_paths, &convert_batches, request);
    }
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
        revalidations: Mutex::new(HashSet::new()),
        shell_open_paths,
        convert_batches,
    });
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Starts the bundled local desktop application.
///
/// # Panics
///
/// Panics when Tauri cannot initialize the configured window or event loop.
pub fn run() {
    let shell_open_paths = Arc::new(Mutex::new(VecDeque::new()));
    let forwarded_shell_open_paths = Arc::clone(&shell_open_paths);
    let convert_batches = Arc::new(Mutex::new(ShellConvertCoordinator::new()));
    let forwarded_convert_batches = Arc::clone(&convert_batches);
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(
            move |app, arguments, _working_directory| {
                handle_second_instance(
                    app,
                    arguments,
                    &forwarded_shell_open_paths,
                    &forwarded_convert_batches,
                );
            },
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            setup_desktop(
                app,
                Arc::clone(&shell_open_paths),
                Arc::clone(&convert_batches),
            )
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
            get_desktop_shell_open,
            classify_desktop_drop_path,
            take_desktop_shell_convert_batch,
            desktop_queue_window_busy,
            ingest_shell_convert_paths,
            show_desktop_toast,
            requeue_desktop_job,
            cleanup_desktop_job_staging,
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
            export_desktop_report,
            export_desktop_recipe,
            reveal_desktop_job_output,
            revalidate_desktop_job,
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
    use std::path::{Path, PathBuf};
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
        DESKTOP_JOB_PAGE_LIMIT, DesktopConversionRequest, DesktopOperationGate,
        MAX_PENDING_SHELL_OPEN_PATHS, acquire_active_operation, acquire_maintenance_operation,
        acquire_queue_window, apply_pending_restore, backup_path, bundled_manifest_paths,
        classify_local_absolute_path, desktop_job_page_limit, enqueue_shell_open_path,
        ingest_shell_convert, load_preset_library, parse_shell_invocation, pending_restore_path,
        persist_preset_library, prepare_approved_desktop_conversion, recover_desktop_jobs,
        report_for_export, requeue_job, run_queue_window_on_database, shell_open_path_from_args,
        stage_pending_restore, validated_shell_open_path, validated_shell_request,
        write_desktop_export_noclobber,
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

    #[test]
    fn report_export_path_redaction_does_not_mutate_the_stored_report() {
        let job_id = Uuid::new_v4();
        let mut stored = report(job_id, ValidationStatus::Pass);
        stored.input.display_path = Some("E:\\private\\input.pdf".to_owned());
        stored.output.display_path = Some("E:\\private\\output.png".to_owned());
        stored.redaction.paths_redacted = false;

        let exported = report_for_export(stored.clone(), true);

        assert_eq!(
            stored.input.display_path.as_deref(),
            Some("E:\\private\\input.pdf")
        );
        assert_eq!(
            stored.output.display_path.as_deref(),
            Some("E:\\private\\output.png")
        );
        assert_eq!(exported.input.display_path, None);
        assert_eq!(exported.output.display_path, None);
        assert!(exported.redaction.paths_redacted);
    }

    #[test]
    fn desktop_json_export_is_durable_and_never_overwrites() {
        let directory = tempdir().expect("export directory");
        let destination = directory.path().join("report.json");

        assert_eq!(
            write_desktop_export_noclobber(&destination, b"first").expect("first export"),
            5
        );
        assert_eq!(fs::read(&destination).expect("read export"), b"first");

        write_desktop_export_noclobber(&destination, b"second")
            .expect_err("existing export must win");
        assert_eq!(fs::read(&destination).expect("read export"), b"first");
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("read directory")
                .filter_map(std::result::Result::ok)
                .count(),
            1
        );
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
            video_crf: None,
            video_preset: None,
            audio_bitrate_kbps: None,
            audio_stream_index: None,
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
        let entry = formatwright_core::EngineRegistryIdentity {
            engine_id: Some("fixture-engine".to_owned()),
            manifest_path: expected.clone(),
        };
        fs::write(
            directory.path().join("fixture-engine.json"),
            serde_json::to_vec(&entry).expect("serialize entry"),
        )
        .expect("write registry entry");
        fs::write(directory.path().join(".abc.partial"), b"incomplete").expect("write partial");

        let registry = formatwright_core::EngineRegistry::new(
            directory.path().to_path_buf(),
            directory.path().join("store"),
        );
        assert_eq!(
            registry
                .active_entries()
                .expect("read registry")
                .into_iter()
                .map(|entry| entry.manifest_path)
                .collect::<Vec<_>>(),
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
    fn desktop_job_pages_are_hard_bounded() {
        assert_eq!(desktop_job_page_limit(None), DESKTOP_JOB_PAGE_LIMIT);
        assert_eq!(desktop_job_page_limit(Some(0)), 1);
        assert_eq!(desktop_job_page_limit(Some(25)), 25);
        assert_eq!(
            desktop_job_page_limit(Some(usize::MAX)),
            DESKTOP_JOB_PAGE_LIMIT
        );
    }

    #[test]
    fn shell_open_accepts_one_explicit_existing_local_path() {
        let suite = tempdir().expect("suite");
        let input = suite.path().join("名字 with spaces.json");
        fs::write(&input, b"{}").expect("input");
        assert_eq!(
            validated_shell_open_path([
                "formatwright-desktop.exe".into(),
                "--shell-open".into(),
                input.clone().into_os_string(),
            ]),
            Some(input)
        );
        assert_eq!(
            shell_open_path_from_args([
                "formatwright-desktop.exe",
                "--unknown",
                "value",
                "--shell-open",
                "selected.txt",
                "ignored.txt",
            ]),
            Some(PathBuf::from("selected.txt"))
        );
        let pending = std::sync::Mutex::new(std::collections::VecDeque::new());
        for index in 0..=MAX_PENDING_SHELL_OPEN_PATHS {
            enqueue_shell_open_path(&pending, PathBuf::from(index.to_string()));
        }
        let pending = pending.into_inner().expect("pending paths");
        assert_eq!(pending.len(), MAX_PENDING_SHELL_OPEN_PATHS);
        assert_eq!(
            pending.front().map(|request| request.path.clone()),
            Some(PathBuf::from("1"))
        );
        assert_eq!(
            pending.back().map(|request| request.path.clone()),
            Some(PathBuf::from(MAX_PENDING_SHELL_OPEN_PATHS.to_string()))
        );
    }

    #[test]
    fn shell_convert_requires_an_allowed_target_and_a_real_file() {
        let suite = tempdir().expect("suite");
        let input = suite.path().join("manual.pdf");
        fs::write(&input, b"%PDF-1.4").expect("input");
        let parsed = parse_shell_invocation([
            "formatwright-desktop.exe",
            "--shell-convert",
            "--to",
            "PNG",
            input.to_str().expect("utf8"),
        ]);
        assert_eq!(parsed, Some((input.clone(), Some("png".to_owned()))));
        assert_eq!(
            parse_shell_invocation([
                "formatwright-desktop.exe",
                "--shell-convert",
                input.to_str().expect("utf8"),
                "--to",
                "jpeg",
            ]),
            Some((input.clone(), Some("jpg".to_owned())))
        );
        assert_eq!(
            parse_shell_invocation([
                "formatwright-desktop.exe",
                "--shell-convert",
                "--to",
                "exe",
                input.to_str().expect("utf8"),
            ]),
            None
        );
        let request = validated_shell_request([
            "formatwright-desktop.exe".into(),
            "--shell-convert".into(),
            "--to".into(),
            "png".into(),
            input.clone().into_os_string(),
        ])
        .expect("valid convert request");
        assert_eq!(request.convert_to.as_deref(), Some("png"));
        assert!(!request.directory);
        assert_eq!(
            validated_shell_request([
                "formatwright-desktop.exe".into(),
                "--shell-convert".into(),
                "--to".into(),
                "png".into(),
                suite.path().as_os_str().to_os_string(),
            ]),
            None
        );
    }

    #[test]
    fn classify_desktop_drop_path_accepts_local_file_and_directory() {
        let suite = tempdir().expect("suite");
        let file = suite.path().join("photo.png");
        fs::write(&file, b"png").expect("file");
        let file_class = classify_local_absolute_path(&file);
        assert_eq!(file_class.kind, "file");
        assert_eq!(file_class.path.as_deref(), Some(file.as_path()));
        let dir_class = classify_local_absolute_path(suite.path());
        assert_eq!(dir_class.kind, "directory");
        assert_eq!(dir_class.path.as_deref(), Some(suite.path()));
        assert_eq!(
            classify_local_absolute_path(Path::new("relative-drop.bin")).kind,
            "rejected"
        );
        #[cfg(windows)]
        assert_eq!(
            classify_local_absolute_path(Path::new(r"\\server\share\album")).kind,
            "rejected"
        );
    }

    #[test]
    fn ingest_all_conflict_creates_zero_batch_rows() {
        let suite = tempdir().expect("suite");
        let database = suite.path().join("jobs.sqlite3");
        let reports = suite.path().join("reports");
        fs::create_dir_all(&reports).expect("reports");
        let store = std::sync::Mutex::new(SqliteJobStore::open(&database).expect("store"));
        let queue = std::sync::Mutex::new(None);
        let cancellations = std::sync::Mutex::new(std::collections::HashMap::new());
        let input = suite.path().join("notes.json");
        fs::write(&input, br#"[{"id":1}]"#).expect("input");
        fs::write(suite.path().join("notes.converted.yaml"), b"exists: true\n").expect("conflict");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let result = runtime
            .block_on(ingest_shell_convert(
                &store,
                &database,
                &reports,
                &queue,
                &cancellations,
                None,
                vec![input],
                "yaml".to_owned(),
            ))
            .expect("ingest");
        assert!(!result.ran_immediately);
        assert!(result.batch_id.is_none());
        assert_eq!(result.queued, 0);
        assert_eq!(result.skipped_conflict, 1);
        assert_eq!(
            store
                .lock()
                .expect("store")
                .list_batches_page(10, 0)
                .expect("batches")
                .len(),
            0
        );
    }

    #[test]
    fn parse_shell_invocation_prefers_convert_when_both_markers_are_present() {
        let parsed = parse_shell_invocation([
            "formatwright-desktop.exe",
            "--shell-open",
            r"C:\in\manual.pdf",
            "--shell-convert",
            "--to",
            "png",
        ]);
        assert_eq!(
            parsed,
            Some((PathBuf::from(r"C:\in\manual.pdf"), Some("png".to_owned())))
        );
        assert_eq!(
            parse_shell_invocation([
                "formatwright-desktop.exe",
                "--shell-open",
                r"C:\in\manual.pdf",
                "--shell-convert",
                "--to",
                "exe",
            ]),
            None
        );
    }

    #[test]
    fn shell_open_rejects_missing_or_incomplete_requests() {
        assert_eq!(
            validated_shell_open_path([
                "formatwright-desktop.exe".into(),
                "--shell-open".into(),
                PathBuf::from("definitely-missing-formatwright-input").into_os_string(),
            ]),
            None
        );
        #[cfg(windows)]
        for rejected in [r"\\server\share\file.txt", r"\\.\C:\device.txt"] {
            assert_eq!(
                validated_shell_open_path([
                    "formatwright-desktop.exe".into(),
                    "--shell-open".into(),
                    PathBuf::from(rejected).into_os_string(),
                ]),
                None
            );
        }
        assert_eq!(
            shell_open_path_from_args(["formatwright-desktop.exe", "--shell-open"]),
            None
        );
        assert_eq!(
            shell_open_path_from_args(["formatwright-desktop.exe", "selected.txt"]),
            None
        );
        assert_eq!(
            validated_shell_open_path([
                "formatwright-desktop.exe".into(),
                "--shell-open".into(),
                PathBuf::from("relative-input.txt").into_os_string(),
            ]),
            None
        );
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
                |_| {},
            ))
        });
        // Linux CI runners schedule the queue thread slowly; five seconds
        // flaked there while passing everywhere else.
        callback_entered_rx
            .recv_timeout(Duration::from_secs(60))
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
                video_crf: None,
                video_preset: None,
                audio_bitrate_kbps: None,
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
