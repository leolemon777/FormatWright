//! Audited recovery actions for deterministic per-job staging artifacts.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;
use crate::job_store::{JobRecord, SqliteJobStore};
use crate::runner::cleanup_staged_output;

/// Result of one manual, exact-path staging cleanup.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StagedCleanupReport {
    pub job: JobRecord,
    pub removed: bool,
}

/// Coordinates deterministic filesystem cleanup with durable job state.
#[derive(Debug, Default)]
pub struct JobRecoveryService;

impl JobRecoveryService {
    /// Removes only staging paths derived from the trusted Job ID/output pair.
    ///
    /// The database holds an immediate writer transaction while the current
    /// state is checked and cleanup runs, preventing a concurrent retry from
    /// starting the same job between those operations. Every successful call
    /// appends a cleaned/not-found audit event without changing job state.
    ///
    /// # Errors
    ///
    /// Returns a policy, path, filesystem, or storage error for missing/active
    /// jobs, unsafe paths, failed removal, or concurrent database damage.
    pub fn cleanup_staging(
        store: &mut SqliteJobStore,
        job_id: Uuid,
    ) -> Result<StagedCleanupReport> {
        let (job, removed) = store.cleanup_staging_persisted(job_id, |job| {
            cleanup_staged_output(&job.output_path, job.id)
        })?;
        Ok(StagedCleanupReport { job, removed })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use tempfile::tempdir;
    use uuid::Uuid;

    use super::JobRecoveryService;
    use crate::domain::{ChangeSet, JobState, NetworkPolicy, Plan, SCHEMA_VERSION};
    use crate::{SqliteJobStore, staged_output_candidates};

    fn plan(output: PathBuf) -> Plan {
        let mut plan = Plan {
            schema_version: SCHEMA_VERSION,
            plan_id: Uuid::new_v4(),
            plan_hash: String::new(),
            input_fingerprint: "fwfp-v1:recovery".to_owned(),
            target_format: "mp4".to_owned(),
            constraints: BTreeMap::new(),
            steps: Vec::new(),
            changes: ChangeSet::default(),
            validators: Vec::new(),
            network_policy: NetworkPolicy::Deny,
            output_path: Some(output),
            estimated_output_bytes: None,
        };
        plan.plan_hash = crate::planner::deterministic_plan_hash(&plan).expect("plan hash");
        plan
    }

    #[test]
    fn cleanup_removes_only_exact_staging_and_audits_cleaned_then_not_found() {
        let suite = tempdir().expect("suite");
        let output = suite.path().join("output.mp4");
        let database = suite.path().join("jobs.sqlite3");
        let mut store = SqliteJobStore::open(&database).expect("store");
        let job = store
            .create_job("input.mkv", &output, &plan(output.clone()))
            .expect("job");
        store
            .transition(job.id, JobState::Cancelled, "USER_CANCELLED")
            .expect("cancelled");
        let candidates = staged_output_candidates(&output, job.id).expect("candidates");
        fs::write(&candidates[0], b"staging").expect("staging");
        fs::write(&output, b"final-output").expect("final output");
        let unrelated = suite
            .path()
            .join(".formatwright-partial-unrelated-output.mp4");
        fs::write(&unrelated, b"unrelated").expect("unrelated");

        let report = JobRecoveryService::cleanup_staging(&mut store, job.id).expect("cleanup");
        assert!(report.removed);
        assert_eq!(report.job.state, JobState::Cancelled);
        assert!(!candidates[0].exists());
        assert_eq!(fs::read(&output).expect("final remains"), b"final-output");
        assert!(unrelated.is_file());
        let details = store
            .get_job_details(job.id)
            .expect("details")
            .expect("job");
        assert_eq!(
            details.events.last().expect("event").code,
            "STAGED_OUTPUT_CLEANED"
        );
        assert_eq!(
            details.events.last().expect("event").previous_state,
            Some(JobState::Cancelled)
        );
        assert_eq!(
            details.events.last().expect("event").next_state,
            JobState::Cancelled
        );

        let second = JobRecoveryService::cleanup_staging(&mut store, job.id).expect("idempotent");
        assert!(!second.removed);
        let after_noop = store
            .get_job_details(job.id)
            .expect("details")
            .expect("job");
        assert_eq!(after_noop.events.len(), details.events.len() + 1);
        assert_eq!(
            after_noop.events.last().expect("event").code,
            "STAGED_OUTPUT_NOT_FOUND"
        );
        drop(store);
        assert!(
            crate::MaintenanceService::new(database)
                .integrity_check()
                .expect("integrity")
                .ok
        );
    }

    #[test]
    fn cleanup_rejects_runnable_jobs_without_touching_staging() {
        let suite = tempdir().expect("suite");
        let output = suite.path().join("output.mp4");
        let mut store = SqliteJobStore::open_in_memory().expect("store");
        let job = store
            .create_job("input.mkv", &output, &plan(output.clone()))
            .expect("job");
        store
            .transition(job.id, JobState::Queued, "JOB_ENQUEUED")
            .expect("queued");
        let staged = staged_output_candidates(&output, job.id)
            .expect("candidates")
            .remove(0);
        fs::write(&staged, b"possibly active").expect("staging");

        let error = JobRecoveryService::cleanup_staging(&mut store, job.id)
            .expect_err("queued cleanup must be refused");
        assert_eq!(error.code, crate::ErrorCode::PolicyBlocked);
        assert!(staged.is_file());
        assert_eq!(
            store.get_job(job.id).expect("job").expect("stored").state,
            JobState::Queued
        );
    }

    #[test]
    fn cleanup_refuses_an_unexpected_partial_beside_a_completed_output() {
        let suite = tempdir().expect("suite");
        let output = suite.path().join("output.mp4");
        let mut store = SqliteJobStore::open_in_memory().expect("store");
        let job = store
            .create_job("input.mkv", &output, &plan(output.clone()))
            .expect("job");
        store
            .transition(job.id, JobState::Running, "ENGINE_STARTED")
            .expect("running");
        store
            .transition(job.id, JobState::Validating, "ENGINE_FINISHED")
            .expect("validating");
        store
            .transition(job.id, JobState::Completed, "VALIDATION_FINISHED")
            .expect("completed");
        fs::write(&output, b"final-output").expect("final output");
        let staged = staged_output_candidates(&output, job.id)
            .expect("candidates")
            .remove(0);
        fs::write(&staged, b"unexpected staging").expect("staging");

        let error = JobRecoveryService::cleanup_staging(&mut store, job.id)
            .expect_err("completed cleanup must be refused");
        assert_eq!(error.code, crate::ErrorCode::PolicyBlocked);
        assert!(staged.is_file());
        assert_eq!(fs::read(&output).expect("final remains"), b"final-output");
    }

    #[cfg(windows)]
    #[test]
    fn cleanup_refuses_a_reparse_staging_directory() {
        use std::os::windows::fs::symlink_dir;

        let suite = tempdir().expect("suite");
        let output = suite.path().join("output.mp4");
        let mut store = SqliteJobStore::open_in_memory().expect("store");
        let job = store
            .create_job("input.mkv", &output, &plan(output.clone()))
            .expect("job");
        store
            .transition(job.id, JobState::Cancelled, "USER_CANCELLED")
            .expect("cancelled");
        let external = suite.path().join("external");
        fs::create_dir(&external).expect("external");
        fs::write(external.join("keep.txt"), b"keep").expect("external file");
        let staged = staged_output_candidates(&output, job.id)
            .expect("candidates")
            .remove(0);
        if let Err(error) = symlink_dir(&external, &staged) {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                eprintln!("skipped staging reparse assertion: {error}");
                return;
            }
            panic!("create staging directory symlink: {error}");
        }

        let error = JobRecoveryService::cleanup_staging(&mut store, job.id)
            .expect_err("linked staging must be refused");
        assert_eq!(error.code, crate::ErrorCode::PolicyBlocked);
        assert_eq!(
            fs::read(external.join("keep.txt")).expect("external remains"),
            b"keep"
        );
        assert!(staged.exists());
    }

    #[test]
    fn cleanup_writer_transaction_blocks_a_concurrent_retry_until_removal_finishes() {
        let suite = tempdir().expect("suite");
        let database = suite.path().join("jobs.sqlite3");
        let output = suite.path().join("output.mp4");
        let mut setup = SqliteJobStore::open(&database).expect("store");
        let job = setup
            .create_job("input.mkv", &output, &plan(output.clone()))
            .expect("job");
        setup
            .transition(job.id, JobState::Cancelled, "USER_CANCELLED")
            .expect("cancelled");
        let staged = staged_output_candidates(&output, job.id)
            .expect("candidates")
            .remove(0);
        fs::write(&staged, b"staging").expect("staging");
        drop(setup);
        let mut retry_store = SqliteJobStore::open(&database).expect("retry store");

        let (cleanup_entered_tx, cleanup_entered_rx) = mpsc::channel();
        let (release_cleanup_tx, release_cleanup_rx) = mpsc::channel();
        let cleanup_database = database.clone();
        let cleanup_job_id = job.id;
        let cleanup_thread = thread::spawn(move || {
            let mut store = SqliteJobStore::open(cleanup_database).expect("cleanup store");
            store
                .cleanup_staging_persisted(cleanup_job_id, |current| {
                    cleanup_entered_tx.send(()).expect("signal cleanup");
                    release_cleanup_rx.recv().expect("release cleanup");
                    crate::cleanup_staged_output(&current.output_path, current.id)
                })
                .expect("cleanup transaction")
        });
        cleanup_entered_rx.recv().expect("cleanup entered");

        let (retry_started_tx, retry_started_rx) = mpsc::channel();
        let (retry_done_tx, retry_done_rx) = mpsc::channel();
        let retry_job_id = job.id;
        let retry_thread = thread::spawn(move || {
            retry_started_tx.send(()).expect("signal retry");
            let result = retry_store.transition(retry_job_id, JobState::Queued, "JOB_RETRIED");
            retry_done_tx.send(result).expect("retry result");
        });
        retry_started_rx.recv().expect("retry started");
        assert!(
            retry_done_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "retry crossed the active cleanup writer transaction"
        );

        release_cleanup_tx.send(()).expect("finish cleanup");
        let (_, removed) = cleanup_thread.join().expect("cleanup thread");
        assert!(removed);
        assert!(!staged.exists());
        let retried = retry_done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("retry completion")
            .expect("retry succeeds after cleanup");
        assert_eq!(retried.state, JobState::Queued);
        retry_thread.join().expect("retry thread");
        let details = SqliteJobStore::open(&database)
            .expect("store")
            .get_job_details(job.id)
            .expect("details")
            .expect("job");
        let cleanup_index = details
            .events
            .iter()
            .position(|event| event.code == "STAGED_OUTPUT_CLEANED")
            .expect("cleanup event");
        let retry_index = details
            .events
            .iter()
            .position(|event| event.code == "JOB_RETRIED")
            .expect("retry event");
        assert!(cleanup_index < retry_index);
    }
}
