#![forbid(unsafe_code)]

pub mod application;
pub mod application_state;
pub mod capabilities;
pub mod doctor;
mod document;
pub mod domain;
pub mod engine_pack;
pub mod engine_registry;
pub use engine_registry::{EngineFallback, EngineRecovery, EngineRegistry, InstalledEngineVersion};
mod edge_pdf;
pub mod error;
pub mod fingerprint;
pub mod inspect;
pub mod job_store;
pub mod maintenance;
mod office;
mod pdf;
pub mod planner;
pub mod preset;
pub mod runner;
pub mod scheduler;
pub mod structured;
pub mod validation;
mod workflow;

pub use application::{
    BulkJobService, ConversionRunResult, ConversionService, FolderBatchService, FolderDiskBudget,
    FolderMappingEntry, FolderMappingPlan, JobExecutionService, JobRecoveryService,
    MAX_FOLDER_BATCH_FILES, QueueProgressUpdate, QueueRunReport, QueueWaitReason,
    QueueWindowControl, ReportService, RevalidationService, StagedCleanupReport,
};
pub use application_state::{
    APPLICATION_SETTINGS_SCHEMA_VERSION, APPLICATION_STATE_BUNDLE_SCHEMA_VERSION,
    ApplicationSettings, ApplicationSettingsService, ApplicationStateLayout,
    ApplicationStateService, EngineRegistryIdentity, StateBundleBackupReport, StateBundleComponent,
    StateBundleComponents, StateBundleEntry, StateBundleManifest, StateBundleOptions,
    StateBundlePreflightReport, StateBundleRestoreReport,
};
pub use capabilities::{
    CapabilitySnapshot, RouteAvailability, capability_snapshot_for_input, ensure_route_available,
};
pub use doctor::{
    EngineDiscoveryPolicy, doctor, doctor_with_policy, find_executable, inspect_builtin_engine,
    inspect_engine, inspect_engine_with_policy,
};
pub use document::{
    inspect_document, plan_markup_to_docx, plan_markup_to_epub, plan_markup_to_pdf,
};
pub use domain::{
    ArtifactIdentity, ArtifactSummary, ChangeSet, FormatDescriptor, FormatKind, JobState,
    MetadataEntry, NetworkPolicy, Plan, PlanRequest, PlanStep, Probe, ProbeEvidence,
    ReportRedaction, StreamKind, StreamProbe, ValidationCheck, ValidationReport, ValidationStatus,
};
pub use edge_pdf::plan_edge_print_to_pdf;
pub use engine_pack::{
    ENGINE_PROTOCOL_VERSION, VerifiedEnginePack, activate_engine_pack, embedded_release_keyring,
    install_engine_pack, load_release_keyring, verify_engine_pack, verify_engine_pack_with_keyring,
};
pub use error::{ErrorCode, FormatWrightError, Result, Stage};
pub use fingerprint::{full_blake3, identify_artifact};
pub use formatwright_engine_sdk::{
    Certification, DoctorReport, EngineHealth, EngineIdentity, LossClass, Operation,
    SignatureTrust, SupplyChainReviewStatus, derive_engine_certification,
    engine_provenance_message,
};
pub use inspect::inspect_media;
pub use job_store::{
    BatchRecord, BulkActionReport, BulkJobAction, IdempotentJobResult, JobCreateRequest,
    JobDetails, JobEventRecord, JobProgress, JobQueryPage, JobRecord, JobSelectionQuery,
    JobStateCount, RevalidationRecord, SelectionSnapshot, SqliteJobStore,
};
pub use maintenance::{
    BackupReport, CompactReport, IntegrityReport, MaintenanceService, MaintenanceStatus,
    RestorePreflightReport, RestoreReport,
};
pub use office::{inspect_office, office_format_hint, plan_office_to_pdf};
pub use pdf::{inspect_pdf, pdf_format_hint, plan_pdf_render};
pub use planner::{plan_conversion, plan_heic_conversion, plan_metadata_clean};
pub use preset::{ConversionPreset, PRESET_SCHEMA_VERSION, PresetLibrary};
pub use runner::{
    ExecutionMilestone, ExecutionResult, cleanup_staged_output, execute_plan,
    execute_plan_observed, resolve_output_path, staged_output_candidates, staged_output_path,
};
pub use scheduler::{
    AdmissionBlocker, ResourceRequest, ResourceScheduler, SchedulerPolicy, WorkClass,
    request_for_plan,
};
pub use structured::{inspect_structured, plan_structured_conversion, structured_format_hint};
pub use validation::validate_media_output;
pub use workflow::{ensure_plan_approved, prepare_conversion};
