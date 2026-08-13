//! Atomic `ValidationReport` persistence shared by every surface.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use tempfile::TempPath;
use uuid::Uuid;

use crate::domain::{JobState, ValidationReport, ValidationStatus};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::job_store::{JobRecord, SqliteJobStore};

const MAX_REPORT_BYTES: u64 = 16 * 1024 * 1024;

/// Owns the durable report directory and its recoverable replace protocol.
#[derive(Clone, Debug)]
pub struct ReportService {
    directory: PathBuf,
}

impl ReportService {
    #[must_use]
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Writes or replaces one report through a unique partial and recoverable
    /// same-directory backup.
    ///
    /// # Errors
    ///
    /// Returns a typed storage error for mismatched IDs, oversized serialized
    /// reports, inaccessible storage, or a failed atomic replacement.
    pub fn save(&self, job_id: Uuid, report: &ValidationReport) -> Result<PathBuf> {
        if report.job_id != job_id {
            return Err(report_error(
                "ValidationReport job ID does not match its destination",
                "Persist the report under its own Job ID.",
            ));
        }
        fs::create_dir_all(&self.directory).map_err(report_io_error)?;
        let destination = self.report_path(job_id);
        let backup = self.backup_path(job_id);
        Self::recover_backup(&destination, &backup)?;

        let bytes = serde_json::to_vec_pretty(report).map_err(|error| {
            report_error(
                "Unable to serialize ValidationReport",
                "Retry after checking the report schema.",
            )
            .with_diagnostic(error.to_string())
        })?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_REPORT_BYTES {
            return Err(report_error(
                "ValidationReport exceeds the 16 MiB persistence limit",
                "Reduce diagnostic detail and retry report persistence.",
            ));
        }

        let partial = self
            .directory
            .join(format!(".{job_id}.{}.partial", Uuid::new_v4()));
        let write_result = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&partial)?;
            file.write_all(&bytes)?;
            file.sync_all()
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&partial);
            return Err(report_io_error(error));
        }

        if destination.is_file()
            && let Err(error) = fs::rename(&destination, &backup)
        {
            let _ = fs::remove_file(&partial);
            return Err(report_io_error(error));
        }
        persist_partial_noclobber(&partial, &destination, &backup)?;
        if backup.is_file() {
            fs::remove_file(&backup).map_err(report_io_error)?;
        }
        Ok(destination)
    }

    /// Reads and validates a report bounded to 16 MiB.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the file is oversized, unreadable,
    /// malformed, or names another Job ID.
    pub fn read(&self, job_id: Uuid) -> Result<Option<ValidationReport>> {
        let destination = self.report_path(job_id);
        let backup = self.backup_path(job_id);
        Self::recover_backup(&destination, &backup)?;
        if !destination.is_file() {
            return Ok(None);
        }
        let metadata = fs::metadata(&destination).map_err(report_io_error)?;
        if metadata.len() > MAX_REPORT_BYTES {
            return Err(report_error(
                "Stored ValidationReport exceeds the 16 MiB read limit",
                "Restore a valid report or remove the corrupted oversized file.",
            ));
        }
        let bytes = fs::read(&destination).map_err(report_io_error)?;
        let report = serde_json::from_slice::<ValidationReport>(&bytes).map_err(|error| {
            report_error(
                "Stored ValidationReport is invalid",
                "Restore the report from a trusted application-state backup.",
            )
            .with_diagnostic(error.to_string())
        })?;
        if report.job_id != job_id {
            return Err(report_error(
                "Stored ValidationReport belongs to another Job ID",
                "Restore the report directory from a consistent backup.",
            ));
        }
        Ok(Some(report))
    }

    /// Saves a report before committing its terminal job state. A failed
    /// report write converts an active job to Interrupted when possible.
    ///
    /// # Errors
    ///
    /// Returns the report write failure or the terminal transition failure.
    pub fn persist_before_terminal(
        &self,
        store: &mut SqliteJobStore,
        job_id: Uuid,
        report: &ValidationReport,
        terminal_event_code: &str,
    ) -> Result<JobRecord> {
        if let Err(mut error) = self.save(job_id, report) {
            if let Err(recovery) = interrupt_after_report_failure(store, job_id) {
                let previous = error.diagnostic.take().unwrap_or_default();
                error.diagnostic = Some(format!(
                    "{previous}; recovery transition also failed: {recovery}"
                ));
            }
            return Err(error);
        }
        let final_state = match report.status {
            ValidationStatus::Pass => JobState::Completed,
            ValidationStatus::Warning | ValidationStatus::Unknown => JobState::Warning,
            ValidationStatus::Fail => JobState::Failed,
        };
        store.transition(job_id, final_state, terminal_event_code)
    }

    fn report_path(&self, job_id: Uuid) -> PathBuf {
        self.directory.join(format!("{job_id}.json"))
    }

    fn backup_path(&self, job_id: Uuid) -> PathBuf {
        self.directory.join(format!(".{job_id}.backup"))
    }

    fn recover_backup(destination: &Path, backup: &Path) -> Result<()> {
        if !destination.exists() && backup.is_file() {
            fs::rename(backup, destination).map_err(report_io_error)?;
        } else if destination.is_file() && backup.is_file() {
            fs::remove_file(backup).map_err(report_io_error)?;
        }
        Ok(())
    }
}

fn interrupt_after_report_failure(store: &mut SqliteJobStore, job_id: Uuid) -> Result<()> {
    if store
        .get_job(job_id)?
        .is_some_and(|job| matches!(job.state, JobState::Running | JobState::Validating))
    {
        store
            .transition(job_id, JobState::Interrupted, "REPORT_PERSIST_FAILED")
            .map(drop)?;
    }
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
fn report_io_error(error: std::io::Error) -> FormatWrightError {
    report_error(
        "Unable to persist or read ValidationReport",
        "Check the report directory permissions and available storage, then retry.",
    )
    .with_diagnostic(error.to_string())
}

fn report_error(message: &str, action: &str) -> FormatWrightError {
    FormatWrightError::new(ErrorCode::StorageFailed, Stage::Validate, message, action)
}

fn persist_partial_noclobber(partial: &Path, destination: &Path, backup: &Path) -> Result<()> {
    let temporary = TempPath::try_from_path(partial.to_path_buf()).map_err(report_io_error)?;
    if let Err(error) = temporary.persist_noclobber(destination) {
        if backup.is_file() && !destination.exists() {
            let _ = fs::rename(backup, destination);
        }
        return Err(report_io_error(error.error));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;
    use uuid::Uuid;

    use super::{MAX_REPORT_BYTES, ReportService, persist_partial_noclobber};
    use crate::domain::{
        ArtifactSummary, ChangeSet, JobState, NetworkPolicy, Plan, ReportRedaction,
        ValidationReport, ValidationStatus,
    };
    use crate::job_store::SqliteJobStore;

    fn plan(output: PathBuf) -> Plan {
        Plan {
            schema_version: 1,
            plan_id: Uuid::new_v4(),
            plan_hash: "blake3:report-service-test".to_owned(),
            input_fingerprint: "fwfp-v1:report-service-test".to_owned(),
            target_format: "yaml".to_owned(),
            constraints: BTreeMap::new(),
            steps: Vec::new(),
            changes: ChangeSet::default(),
            validators: Vec::new(),
            network_policy: NetworkPolicy::Deny,
            output_path: Some(output),
            estimated_output_bytes: None,
        }
    }

    fn report(job_id: Uuid, status: ValidationStatus) -> ValidationReport {
        let artifact = ArtifactSummary {
            display_path: None,
            format_id: "yaml".to_owned(),
            size_bytes: 10,
            fast_fingerprint: "fwfp-v1:report-service-test".to_owned(),
            full_blake3: None,
        };
        ValidationReport {
            schema_version: 1,
            report_id: Uuid::new_v4(),
            job_id,
            plan_hash: "blake3:report-service-test".to_owned(),
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
        let output = root.join("output.yaml");
        let job = store
            .create_job(root.join("input.json"), &output, &plan(output.clone()))
            .expect("create job");
        store
            .transition(job.id, JobState::Running, "TEST_RUNNING")
            .expect("run job");
        store
            .transition(job.id, JobState::Validating, "TEST_VALIDATING")
            .expect("validate job");
        job.id
    }

    #[test]
    fn replacement_is_atomic_and_backup_recovery_is_idempotent() {
        let directory = tempdir().expect("reports directory");
        let service = ReportService::new(directory.path());
        let job_id = Uuid::new_v4();
        let first = report(job_id, ValidationStatus::Pass);
        service.save(job_id, &first).expect("save first report");
        let destination = directory.path().join(format!("{job_id}.json"));
        let backup = directory.path().join(format!(".{job_id}.backup"));
        fs::rename(&destination, &backup).expect("simulate interrupted replacement");
        assert_eq!(service.read(job_id).expect("recover backup"), Some(first));

        let second = report(job_id, ValidationStatus::Warning);
        service.save(job_id, &second).expect("replace report");
        assert_eq!(
            service.read(job_id).expect("read replacement"),
            Some(second)
        );
        assert!(!backup.exists());
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("read reports")
                .filter_map(std::result::Result::ok)
                .count(),
            1
        );
    }

    #[test]
    fn report_failure_interrupts_before_terminal_state() {
        let directory = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(directory.path().join("jobs.sqlite3")).expect("store");
        let job_id = validating_job(&mut store, directory.path());
        let blocked = directory.path().join("reports-is-a-file");
        fs::write(&blocked, b"not a directory").expect("blocking file");

        ReportService::new(blocked)
            .persist_before_terminal(
                &mut store,
                job_id,
                &report(job_id, ValidationStatus::Pass),
                "TEST_FINISHED",
            )
            .expect_err("report write must fail");
        let details = store
            .get_job_details(job_id)
            .expect("details")
            .expect("job");
        assert_eq!(details.job.state, JobState::Interrupted);
        assert_eq!(
            details.events.last().expect("event").code,
            "REPORT_PERSIST_FAILED"
        );
    }

    #[test]
    fn read_rejects_report_bound_to_another_job() {
        let directory = tempdir().expect("reports directory");
        let service = ReportService::new(directory.path());
        let expected_id = Uuid::new_v4();
        let other = report(Uuid::new_v4(), ValidationStatus::Pass);
        fs::write(
            directory.path().join(format!("{expected_id}.json")),
            serde_json::to_vec(&other).expect("serialize report"),
        )
        .expect("write mismatched report");

        service
            .read(expected_id)
            .expect_err("cross-job report must be rejected");
    }

    #[test]
    fn read_and_write_reject_reports_above_the_size_bound() {
        let directory = tempdir().expect("reports directory");
        let service = ReportService::new(directory.path());
        let job_id = Uuid::new_v4();
        let mut oversized = report(job_id, ValidationStatus::Pass);
        oversized
            .intentional_changes
            .push("x".repeat(usize::try_from(MAX_REPORT_BYTES).expect("bounded test size")));

        service
            .save(job_id, &oversized)
            .expect_err("oversized report write must be rejected");
        assert!(!directory.path().join(format!("{job_id}.json")).exists());

        let stored_id = Uuid::new_v4();
        let stored = fs::File::create(directory.path().join(format!("{stored_id}.json")))
            .expect("oversized fixture");
        stored
            .set_len(MAX_REPORT_BYTES + 1)
            .expect("extend oversized fixture");
        service
            .read(stored_id)
            .expect_err("oversized report read must be rejected");
    }

    #[test]
    fn late_report_destination_race_never_clobbers_the_external_file() {
        let directory = tempdir().expect("reports directory");
        let partial = directory.path().join("partial.json");
        let destination = directory.path().join("report.json");
        let backup = directory.path().join("report.backup");
        fs::write(&partial, b"new-report").expect("partial");
        fs::write(&backup, b"old-report").expect("backup");
        fs::write(&destination, b"external-winner").expect("late destination");

        persist_partial_noclobber(&partial, &destination, &backup)
            .expect_err("late destination must win");

        assert_eq!(
            fs::read(&destination).expect("destination"),
            b"external-winner"
        );
        assert_eq!(fs::read(&backup).expect("backup"), b"old-report");
        assert!(!partial.exists());
    }
}
