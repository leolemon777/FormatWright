//! Shared immediate conversion orchestration for first-party surfaces.

use std::path::{Path, PathBuf};

use formatwright_engine_sdk::EngineIdentity;
use tokio_util::sync::CancellationToken;

use crate::domain::{JobState, Plan, PlanRequest, Probe, ValidationReport};
use crate::error::{ErrorCode, Result};
use crate::job_store::{JobRecord, SqliteJobStore};
use crate::runner::{ExecutionMilestone, execute_plan_observed};
use crate::workflow::{ensure_plan_approved, prepare_conversion};

use super::ReportService;

/// Completed immediate conversion with its durable terminal Job and report.
#[derive(Clone, Debug)]
pub struct ConversionRunResult {
    pub job: JobRecord,
    pub output_path: PathBuf,
    pub report: ValidationReport,
}

/// Owns the shared Inspect → Plan → approval → Job → Execute → Report →
/// terminal-state workflow used by CLI and Desktop.
#[derive(Debug, Default)]
pub struct ConversionService;

impl ConversionService {
    /// Re-prepares the approved Plan, executes it, persists the report before
    /// terminal state, and normalizes execution failures into Failed/Cancelled.
    ///
    /// `on_job_updated` is invoked after Running, Validating, and terminal
    /// transitions so presentation surfaces may emit live updates.
    ///
    /// # Errors
    ///
    /// Returns typed inspection, approval, execution, report, or storage errors.
    pub async fn run_approved<F>(
        store: &mut SqliteJobStore,
        reports: &ReportService,
        input: &Path,
        request: &PlanRequest,
        approved_plan_hash: &str,
        cancellation: CancellationToken,
        on_job_updated: F,
    ) -> Result<ConversionRunResult>
    where
        F: FnMut(&JobRecord),
    {
        let (probe, plan, validation_engine) = prepare_conversion(input, request).await?;
        Self::run_prepared(
            store,
            reports,
            &probe,
            &plan,
            &validation_engine,
            approved_plan_hash,
            cancellation,
            on_job_updated,
        )
        .await
    }

    /// Executes an already-prepared Plan through the shared durable workflow.
    /// This supports special planners such as metadata-clean while retaining
    /// the same approval, report, failure, and state-transition contract.
    ///
    /// # Errors
    ///
    /// Returns typed approval, execution, report, or storage errors.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_prepared<F>(
        store: &mut SqliteJobStore,
        reports: &ReportService,
        probe: &Probe,
        plan: &Plan,
        validation_engine: &EngineIdentity,
        approved_plan_hash: &str,
        cancellation: CancellationToken,
        mut on_job_updated: F,
    ) -> Result<ConversionRunResult>
    where
        F: FnMut(&JobRecord),
    {
        ensure_plan_approved(plan, Some(approved_plan_hash))?;
        let output = plan.output_path.as_ref().ok_or_else(|| {
            crate::FormatWrightError::new(
                ErrorCode::InputInvalid,
                crate::Stage::Plan,
                "Immediate conversion requires a resolved output path",
                "Choose an output path and preview the Plan again.",
            )
        })?;
        let created = store.create_job(&probe.artifact.canonical_path, output, plan)?;
        let running = store.transition(created.id, JobState::Running, "ENGINE_STARTED")?;
        on_job_updated(&running);

        let execution = execute_plan_observed(
            probe,
            plan,
            validation_engine,
            running.id,
            cancellation,
            |milestone| match milestone {
                ExecutionMilestone::EngineFinished => {
                    let validating =
                        store.transition(running.id, JobState::Validating, "ENGINE_FINISHED")?;
                    on_job_updated(&validating);
                    Ok(())
                }
            },
        )
        .await;

        match execution {
            Ok(result) => {
                let job = reports.persist_before_terminal(
                    store,
                    running.id,
                    &result.report,
                    "VALIDATION_FINISHED",
                )?;
                on_job_updated(&job);
                Ok(ConversionRunResult {
                    job,
                    output_path: result.output_path,
                    report: result.report,
                })
            }
            Err(mut error) => {
                let recovery = (|| -> Result<Option<JobRecord>> {
                    let Some(current) = store.get_job(running.id)? else {
                        return Ok(None);
                    };
                    if !matches!(current.state, JobState::Running | JobState::Validating) {
                        return Ok(None);
                    }
                    let (state, code) = if error.code == ErrorCode::Cancelled {
                        (JobState::Cancelled, "EXECUTION_CANCELLED")
                    } else {
                        (JobState::Failed, "EXECUTION_STOPPED")
                    };
                    store.transition(running.id, state, code).map(Some)
                })();
                match recovery {
                    Ok(Some(job)) => on_job_updated(&job),
                    Ok(None) => {}
                    Err(recovery_error) => {
                        let original = error.diagnostic.take().unwrap_or_default();
                        error.diagnostic = Some(if original.is_empty() {
                            format!("job recovery transition also failed: {recovery_error}")
                        } else {
                            format!(
                                "{original}; job recovery transition also failed: {recovery_error}"
                            )
                        });
                    }
                }
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    use super::ConversionService;
    use crate::application::ReportService;
    use crate::domain::{JobState, PlanRequest};
    use crate::job_store::SqliteJobStore;
    use crate::workflow::prepare_conversion;

    #[tokio::test]
    async fn shared_immediate_service_persists_report_before_terminal_state() {
        let directory = tempdir().expect("conversion suite");
        let input = directory.path().join("records.json");
        let output = directory.path().join("records.yaml");
        fs::write(&input, r#"[{"id":1,"name":"alpha"}]"#).expect("input fixture");
        let request = PlanRequest {
            target_format: "yaml".to_owned(),
            output_path: Some(output.clone()),
            ..PlanRequest::default()
        };
        let (_, approved, _) = prepare_conversion(&input, &request).await.expect("preview");
        let mut store = SqliteJobStore::open(directory.path().join("jobs.sqlite3")).expect("store");
        let reports = ReportService::new(directory.path().join("reports"));
        let mut observed = Vec::new();

        let result = ConversionService::run_approved(
            &mut store,
            &reports,
            &input,
            &request,
            &approved.plan_hash,
            CancellationToken::new(),
            |job| observed.push(job.state),
        )
        .await
        .expect("execute shared conversion");

        assert_eq!(result.job.state, JobState::Completed);
        assert_eq!(observed.first(), Some(&JobState::Running));
        assert_eq!(observed.last(), Some(&JobState::Completed));
        assert!(output.is_file());
        assert_eq!(
            reports.read(result.job.id).expect("read report"),
            Some(result.report)
        );
    }

    #[tokio::test]
    async fn stale_approval_creates_no_job_or_output() {
        let directory = tempdir().expect("conversion suite");
        let input = directory.path().join("records.json");
        let output = directory.path().join("records.yaml");
        fs::write(&input, r#"[{"id":1}]"#).expect("input fixture");
        let request = PlanRequest {
            target_format: "yaml".to_owned(),
            output_path: Some(output.clone()),
            ..PlanRequest::default()
        };
        let mut store = SqliteJobStore::open_in_memory().expect("store");

        ConversionService::run_approved(
            &mut store,
            &ReportService::new(directory.path().join("reports")),
            &input,
            &request,
            "blake3:not-approved",
            CancellationToken::new(),
            |_| {},
        )
        .await
        .expect_err("stale approval must fail");

        assert_eq!(store.count_jobs().expect("job count"), 0);
        assert!(!output.exists());
    }
}
