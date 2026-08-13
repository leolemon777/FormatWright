//! Shared application-layer use cases for every first-party surface.
//!
//! Surfaces adapt protocols and presentation. Durable job scheduling, state
//! transitions, and resource admission live here so CLI, Desktop, and future
//! API/MCP workers do not fork the state machine.

pub mod bulk_jobs;
pub mod conversion_service;
pub mod folder_batch;
pub mod job_execution;
pub mod report_service;

pub use bulk_jobs::BulkJobService;
pub use conversion_service::{ConversionRunResult, ConversionService};
pub use folder_batch::{
    FolderBatchService, FolderDiskBudget, FolderMappingEntry, FolderMappingPlan,
    MAX_FOLDER_BATCH_FILES,
};
pub use job_execution::{JobExecutionService, QueueRunReport, QueueWindowControl};
pub use report_service::ReportService;
