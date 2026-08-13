//! Durable bulk actions shared by CLI, Desktop, and future API surfaces.

use uuid::Uuid;

use crate::error::Result;
use crate::job_store::{BulkActionReport, BulkJobAction, SqliteJobStore};
use crate::runner::cleanup_staged_output;

/// Coordinates filesystem recovery cleanup with the persisted bulk-action
/// transaction. Surfaces must use this service instead of mutating job states
/// directly.
#[derive(Debug)]
pub struct BulkJobService;

impl BulkJobService {
    /// Applies an action to the immutable membership of a selection snapshot.
    ///
    /// Deterministic partial outputs are removed before a job becomes eligible
    /// to run again. The database action then re-reads every current state and
    /// records one auditable outcome per selected job.
    ///
    /// # Errors
    ///
    /// Returns an input, storage, or output-path error without committing a
    /// partial database action.
    pub fn apply(
        store: &mut SqliteJobStore,
        selection_id: Uuid,
        action: BulkJobAction,
    ) -> Result<BulkActionReport> {
        store.apply_bulk_action_persisted(selection_id, action, |job| {
            cleanup_staged_output(&job.output_path, job.id).map(|_| ())
        })
    }
}
