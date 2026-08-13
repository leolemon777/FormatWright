//! Shared durable-queue execution window used by every surface.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use formatwright_engine_sdk::EngineIdentity;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::doctor::{inspect_builtin_engine, inspect_engine};
use crate::document::inspect_document;
use crate::domain::{JobState, Plan, Probe, ValidationReport, ValidationStatus};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::inspect::inspect_media;
use crate::job_store::{JobDetails, JobRecord, SqliteJobStore};
use crate::office::inspect_office;
use crate::pdf::inspect_pdf;
use crate::runner::{ExecutionMilestone, ExecutionResult, execute_plan_observed};
use crate::scheduler::{
    AdmissionBlocker, ResourceRequest, ResourceScheduler, SchedulerPolicy, request_for_plan,
};
use crate::structured::inspect_structured;

/// Machine-readable summary of one bounded `jobs run` scheduling window.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QueueRunReport {
    pub schema_version: u32,
    pub selected: usize,
    pub completed: usize,
    pub warning: usize,
    pub blocked: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub contended: usize,
    pub stopped: bool,
    pub parallelism: usize,
    pub peak_active: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueueWaitReason {
    AlreadyRunning,
    ProcessLimit,
    MemoryBudget,
    ExclusiveEngine,
    WorkClassLimit,
    AdmissionPaused,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QueueProgressUpdate {
    pub schema_version: u32,
    pub job_id: Uuid,
    pub job_sequence: u64,
    pub state: JobState,
    pub wait_reason: Option<QueueWaitReason>,
    pub occurred_unix_ms: i64,
    pub eta_milliseconds: Option<u64>,
}

/// Controls admission and worker cancellation for one durable queue window.
///
/// - [`Self::pause_finish_current`] stops admitting new jobs but lets active
///   workers finish.
/// - [`Self::pause_immediate`] also cancels active workers (CLI Ctrl+C semantics).
#[derive(Clone, Debug)]
pub struct QueueWindowControl {
    admission: CancellationToken,
    workers: CancellationToken,
}

impl QueueWindowControl {
    /// Creates an idle control plane for a new scheduling window.
    #[must_use]
    pub fn new() -> Self {
        Self {
            admission: CancellationToken::new(),
            workers: CancellationToken::new(),
        }
    }

    /// Stops new admissions; in-flight workers keep running.
    pub fn pause_finish_current(&self) {
        self.admission.cancel();
    }

    /// Stops new admissions and cancels active workers.
    pub fn pause_immediate(&self) {
        self.admission.cancel();
        self.workers.cancel();
    }

    #[must_use]
    pub fn admission_cancelled(&self) -> bool {
        self.admission.is_cancelled()
    }

    #[must_use]
    pub fn workers_cancelled(&self) -> bool {
        self.workers.is_cancelled()
    }

    fn worker_token(&self) -> CancellationToken {
        self.workers.clone()
    }
}

impl Default for QueueWindowControl {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared control-plane executor for durable queued jobs.
#[derive(Debug, Default)]
pub struct JobExecutionService;

impl JobExecutionService {
    /// Hydrates at most `limit` queued jobs, admits them through the deterministic
    /// resource scheduler, rechecks engine identity and input fingerprint, then
    /// executes through the shared runner while persisting milestones centrally.
    ///
    /// `cancellation` stops new admissions **and** cancels active workers
    /// (immediate pause). Surfaces that need finish-current pause should call
    /// [`Self::run_window_observed`] with a [`QueueWindowControl`].
    ///
    /// # Errors
    ///
    /// Returns typed errors for invalid bounds, storage failures, scheduler
    /// exhaustion when no pending job fits, or unexpected worker panics.
    pub async fn run_window(
        store: &mut SqliteJobStore,
        limit: usize,
        parallel: usize,
        cancellation: CancellationToken,
    ) -> Result<QueueRunReport> {
        let control = QueueWindowControl::new();
        if cancellation.is_cancelled() {
            control.pause_immediate();
            return Self::run_window_observed(store, limit, parallel, control, |_, _| Ok(())).await;
        }
        let linked = control.clone();
        let execution = Self::run_window_observed(store, limit, parallel, control, |_, _| Ok(()));
        tokio::pin!(execution);
        tokio::select! {
            result = &mut execution => result,
            () = cancellation.cancelled() => {
                linked.pause_immediate();
                execution.await
            }
        }
    }

    /// Same as [`Self::run_window`], with explicit pause control and a report
    /// callback invoked for every job that produced a `ValidationReport` before
    /// the terminal state is committed.
    ///
    /// Surfaces use this to persist report files or stream them to clients.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::run_window`], plus any error from
    /// `on_report` (which leaves the job non-terminal so recovery can retry).
    #[allow(clippy::too_many_lines)]
    pub async fn run_window_observed<F>(
        store: &mut SqliteJobStore,
        limit: usize,
        parallel: usize,
        control: QueueWindowControl,
        on_report: F,
    ) -> Result<QueueRunReport>
    where
        F: FnMut(Uuid, &ValidationReport) -> Result<()>,
    {
        Self::run_window_observed_with_progress(store, limit, parallel, control, on_report, |_| {})
            .await
    }

    /// Runs a bounded queue window and emits truthful state/wait-reason
    /// snapshots. ETA remains absent unless an engine can supply real timing.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::run_window_observed`].
    #[allow(clippy::too_many_lines)]
    pub async fn run_window_observed_with_progress<F, P>(
        store: &mut SqliteJobStore,
        limit: usize,
        parallel: usize,
        control: QueueWindowControl,
        mut on_report: F,
        mut on_progress: P,
    ) -> Result<QueueRunReport>
    where
        F: FnMut(Uuid, &ValidationReport) -> Result<()>,
        P: FnMut(QueueProgressUpdate),
    {
        if !(1..=16).contains(&parallel) {
            return Err(FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Store,
                "Parallelism must be between 1 and 16",
                "Choose --parallel 1 through 16.",
            ));
        }
        if limit > 256 {
            return Err(FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Store,
                "A scheduling window cannot hydrate more than 256 jobs",
                "Use --limit 256 or less and run another bounded window afterward.",
            ));
        }
        let jobs = store.list_queued_jobs_fair(limit)?;
        let mut pending = VecDeque::with_capacity(jobs.len());
        for job in &jobs {
            let details = store.get_job_details(job.id)?.ok_or_else(|| {
                FormatWrightError::new(
                    ErrorCode::StorageFailed,
                    Stage::Store,
                    format!("Queued job disappeared: {}", job.id),
                    "Run jobs recover and inspect the database.",
                )
            })?;
            pending.push_back(PendingQueueJob {
                resources: request_for_plan(job.id, &details.plan),
                details,
            });
        }
        let mut completed = 0_usize;
        let mut warning = 0_usize;
        let mut blocked = 0_usize;
        let mut failed = 0_usize;
        let mut cancelled = 0_usize;
        let mut contended = 0_usize;
        let policy = SchedulerPolicy::bounded(parallel);
        let mut scheduler = ResourceScheduler::new(policy);
        let mut workers = tokio::task::JoinSet::new();
        let (milestone_sender, mut milestone_receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut peak_active = 0_usize;
        let mut active_jobs = HashSet::new();
        let mut last_wait_reasons = HashMap::new();

        let execution = async {
            while !pending.is_empty() || !workers.is_empty() {
                if control.admission_cancelled() {
                    for candidate in &pending {
                        emit_wait_reason(
                            &mut on_progress,
                            &mut last_wait_reasons,
                            &candidate.details.job,
                            QueueWaitReason::AdmissionPaused,
                        );
                    }
                }
                while !control.admission_cancelled()
                    && scheduler.active_count() < policy.max_processes
                    && !pending.is_empty()
                {
                    let scan_length = pending.len();
                    let mut admitted = None;
                    for _ in 0..scan_length {
                        let Some(candidate) = pending.pop_front() else {
                            break;
                        };
                        if scheduler.try_admit(candidate.resources.clone()) {
                            last_wait_reasons.remove(&candidate.details.job.id);
                            admitted = Some(candidate);
                            break;
                        }
                        if let Some(blocker) = scheduler.admission_blocker(&candidate.resources) {
                            emit_wait_reason(
                                &mut on_progress,
                                &mut last_wait_reasons,
                                &candidate.details.job,
                                queue_wait_reason(blocker),
                            );
                        }
                        pending.push_back(candidate);
                    }
                    let Some(candidate) = admitted else {
                        break;
                    };
                    let job_id = candidate.details.job.id;
                    active_jobs.insert(job_id);
                    match prepare_queued_job(store, candidate.details, |job| {
                        on_progress(queue_progress(job, None));
                    })
                    .await?
                    {
                        QueuePreparation::Ready(prepared) => {
                            let prepared = *prepared;
                            let worker_cancellation = control.worker_token();
                            let worker_milestones = milestone_sender.clone();
                            workers.spawn(async move {
                                #[cfg(test)]
                                run_worker_test_hook(&prepared.plan).await;
                                let result = execute_plan_observed(
                                    &prepared.probe,
                                    &prepared.plan,
                                    &prepared.validation_engine,
                                    prepared.job_id,
                                    worker_cancellation,
                                    |milestone| {
                                        if milestone == ExecutionMilestone::EngineFinished {
                                            let _ = worker_milestones.send(prepared.job_id);
                                        }
                                        Ok(())
                                    },
                                )
                                .await;
                                QueueWorkerOutcome {
                                    job_id: prepared.job_id,
                                    result,
                                }
                            });
                            peak_active = peak_active.max(scheduler.active_count());
                        }
                        QueuePreparation::Blocked => {
                            blocked = blocked.saturating_add(1);
                            scheduler.release(job_id);
                            active_jobs.remove(&job_id);
                        }
                        QueuePreparation::Failed => {
                            failed = failed.saturating_add(1);
                            scheduler.release(job_id);
                            active_jobs.remove(&job_id);
                        }
                        QueuePreparation::Contended => {
                            contended = contended.saturating_add(1);
                            scheduler.release(job_id);
                            active_jobs.remove(&job_id);
                        }
                    }
                }

                if scheduler.active_count() >= policy.max_processes {
                    for candidate in &pending {
                        emit_wait_reason(
                            &mut on_progress,
                            &mut last_wait_reasons,
                            &candidate.details.job,
                            QueueWaitReason::ProcessLimit,
                        );
                    }
                }

                if workers.is_empty() {
                    if control.admission_cancelled() || pending.is_empty() {
                        break;
                    }
                    return Err(FormatWrightError::new(
                        ErrorCode::ResourceExhausted,
                        Stage::Execute,
                        "No queued job fits within the configured scheduler resource budget",
                        "Reduce --parallel or split the queue into a smaller scheduling window.",
                    ));
                }

                let joined = tokio::select! {
                    Some(job_id) = milestone_receiver.recv() => {
                        if let Some(job) = mark_job_validating(store, job_id)? {
                            on_progress(queue_progress(&job, None));
                        }
                        continue;
                    }
                    joined = workers.join_next() => joined,
                };
                let outcome = joined
                    .ok_or_else(|| {
                        FormatWrightError::new(
                            ErrorCode::Internal,
                            Stage::Execute,
                            "Scheduler lost its active worker set",
                            "Run jobs recover and retry interrupted work.",
                        )
                    })?
                    .map_err(|error| {
                        FormatWrightError::new(
                            ErrorCode::Internal,
                            Stage::Execute,
                            format!("Queue worker stopped unexpectedly: {error}"),
                            "Run jobs recover and retry interrupted work.",
                        )
                    })?;
                scheduler.release(outcome.job_id);
                match outcome.result {
                    Ok(result) => {
                        on_report(outcome.job_id, &result.report)?;
                        match result.report.status {
                            ValidationStatus::Pass => {
                                if let Some(job) = mark_job_validating(store, outcome.job_id)? {
                                    on_progress(queue_progress(&job, None));
                                }
                                let job = store.transition(
                                    outcome.job_id,
                                    JobState::Completed,
                                    "VALIDATION_FINISHED",
                                )?;
                                on_progress(queue_progress(&job, None));
                                completed = completed.saturating_add(1);
                            }
                            ValidationStatus::Warning | ValidationStatus::Unknown => {
                                if let Some(job) = mark_job_validating(store, outcome.job_id)? {
                                    on_progress(queue_progress(&job, None));
                                }
                                let job = store.transition(
                                    outcome.job_id,
                                    JobState::Warning,
                                    "VALIDATION_FINISHED",
                                )?;
                                on_progress(queue_progress(&job, None));
                                warning = warning.saturating_add(1);
                            }
                            ValidationStatus::Fail => {
                                if let Some(job) = mark_job_validating(store, outcome.job_id)? {
                                    on_progress(queue_progress(&job, None));
                                }
                                let job = store.transition(
                                    outcome.job_id,
                                    JobState::Failed,
                                    "VALIDATION_FINISHED",
                                )?;
                                on_progress(queue_progress(&job, None));
                                failed = failed.saturating_add(1);
                            }
                        }
                    }
                    Err(error) if error.code == ErrorCode::Cancelled => {
                        let (state, code) = if control.workers_cancelled() {
                            (JobState::Interrupted, "QUEUE_PAUSED_IMMEDIATE")
                        } else {
                            (JobState::Cancelled, "QUEUE_CANCELLED")
                        };
                        let job = store.transition(outcome.job_id, state, code)?;
                        on_progress(queue_progress(&job, None));
                        cancelled = cancelled.saturating_add(1);
                    }
                    Err(_) => {
                        let job = store.transition(
                            outcome.job_id,
                            JobState::Failed,
                            "EXECUTION_STOPPED",
                        )?;
                        on_progress(queue_progress(&job, None));
                        failed = failed.saturating_add(1);
                    }
                }
                active_jobs.remove(&outcome.job_id);
            }
            Ok(())
        }
        .await;
        if let Err(mut error) = execution {
            if let Err(cleanup_error) =
                abort_active_window(store, &control, &mut workers, &mut scheduler, &active_jobs)
                    .await
            {
                let previous = error.diagnostic.take().unwrap_or_default();
                error.diagnostic = Some(format!(
                    "{previous} control-plane cleanup also failed: {cleanup_error}"
                ));
            }
            return Err(error);
        }
        let terminal = completed
            .saturating_add(warning)
            .saturating_add(blocked)
            .saturating_add(failed)
            .saturating_add(cancelled)
            .saturating_add(contended);
        Ok(QueueRunReport {
            schema_version: 1,
            selected: jobs.len(),
            completed,
            warning,
            blocked,
            failed,
            cancelled,
            contended,
            stopped: control.admission_cancelled() || terminal < jobs.len(),
            parallelism: policy.max_processes,
            peak_active,
        })
    }
}

async fn abort_active_window(
    store: &mut SqliteJobStore,
    control: &QueueWindowControl,
    workers: &mut tokio::task::JoinSet<QueueWorkerOutcome>,
    scheduler: &mut ResourceScheduler,
    active_jobs: &HashSet<Uuid>,
) -> Result<()> {
    control.pause_immediate();
    while let Some(joined) = workers.join_next().await {
        if let Ok(outcome) = joined {
            scheduler.release(outcome.job_id);
        }
    }

    let mut first_error = None;
    for job_id in active_jobs {
        scheduler.release(*job_id);
        let transition = match store.get_job(*job_id) {
            Ok(Some(job)) if matches!(job.state, JobState::Running | JobState::Validating) => store
                .transition(*job_id, JobState::Interrupted, "CONTROL_PLANE_FAILED")
                .map(drop),
            Ok(Some(job)) if matches!(job.state, JobState::Inspecting | JobState::Planned) => store
                .transition(*job_id, JobState::Failed, "CONTROL_PLANE_FAILED")
                .map(drop),
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        };
        if let Err(error) = transition
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
async fn run_worker_test_hook(plan: &Plan) {
    if plan
        .constraints
        .get("__test_worker_panic")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        panic!("injected queue worker panic");
    }
    if let Some(delay) = plan
        .constraints
        .get("__test_worker_delay_millis")
        .and_then(serde_json::Value::as_u64)
    {
        tokio::time::sleep(std::time::Duration::from_millis(delay.min(5_000))).await;
    }
}

struct PendingQueueJob {
    details: JobDetails,
    resources: ResourceRequest,
}

struct PreparedQueueJob {
    job_id: Uuid,
    probe: Probe,
    plan: Plan,
    validation_engine: EngineIdentity,
}

enum QueuePreparation {
    Ready(Box<PreparedQueueJob>),
    Blocked,
    Failed,
    Contended,
}

struct QueueWorkerOutcome {
    job_id: Uuid,
    result: Result<ExecutionResult>,
}

async fn prepare_queued_job<P>(
    store: &mut SqliteJobStore,
    details: JobDetails,
    mut on_state: P,
) -> Result<QueuePreparation>
where
    P: FnMut(&JobRecord),
{
    let job_id = details.job.id;
    let Some(inspecting) = store.claim_queued_job(job_id, "QUEUE_REINSPECTING")? else {
        return Ok(QueuePreparation::Contended);
    };
    on_state(&inspecting);
    if let Err(error) = store.validate_output_reservation(job_id) {
        if matches!(
            error.code,
            ErrorCode::InputInvalid | ErrorCode::OutputConflict
        ) {
            let blocked = store.transition(job_id, JobState::Blocked, "OUTPUT_IDENTITY_CHANGED")?;
            on_state(&blocked);
            return Ok(QueuePreparation::Blocked);
        }
        return Err(error);
    }
    let Some(stored_engine) = details.plan.steps.first().map(|step| step.engine.clone()) else {
        let failed = store.transition(job_id, JobState::Failed, "PLAN_INVALID")?;
        on_state(&failed);
        return Ok(QueuePreparation::Failed);
    };
    for stored_step in &details.plan.steps {
        let stored_identity = &stored_step.engine;
        let current = if stored_identity.engine_id == "formatwright.structured" {
            inspect_builtin_engine("formatwright.structured").await
        } else {
            inspect_engine(&stored_identity.engine_id).await
        };
        if !current.is_ok_and(|engine| {
            engine.binary_sha256 == stored_identity.binary_sha256
                && engine.version == stored_identity.version
        }) {
            let blocked = store.transition(job_id, JobState::Blocked, "ENGINE_IDENTITY_CHANGED")?;
            on_state(&blocked);
            return Ok(QueuePreparation::Blocked);
        }
    }

    let inspected = inspect_queued_input(&details, &stored_engine).await;
    let (probe, validation_engine) = match inspected {
        Ok((probe, validation_engine))
            if probe.artifact.fast_fingerprint == details.plan.input_fingerprint =>
        {
            (probe, validation_engine)
        }
        Ok(_) => {
            let blocked = store.transition(job_id, JobState::Blocked, "INPUT_CHANGED")?;
            on_state(&blocked);
            return Ok(QueuePreparation::Blocked);
        }
        Err(_) => {
            let blocked = store.transition(job_id, JobState::Blocked, "REINSPECTION_FAILED")?;
            on_state(&blocked);
            return Ok(QueuePreparation::Blocked);
        }
    };
    let planned = store.transition(job_id, JobState::Planned, "PLAN_REVALIDATED")?;
    on_state(&planned);
    let running = store.transition(job_id, JobState::Running, "ENGINE_STARTED")?;
    on_state(&running);
    Ok(QueuePreparation::Ready(Box::new(PreparedQueueJob {
        job_id,
        probe,
        plan: details.plan,
        validation_engine,
    })))
}

fn queue_wait_reason(blocker: AdmissionBlocker) -> QueueWaitReason {
    match blocker {
        AdmissionBlocker::AlreadyActive => QueueWaitReason::AlreadyRunning,
        AdmissionBlocker::ProcessLimit => QueueWaitReason::ProcessLimit,
        AdmissionBlocker::MemoryBudget => QueueWaitReason::MemoryBudget,
        AdmissionBlocker::ExclusiveEngine => QueueWaitReason::ExclusiveEngine,
        AdmissionBlocker::ClassLimit => QueueWaitReason::WorkClassLimit,
    }
}

fn emit_wait_reason<P>(
    on_progress: &mut P,
    last_wait_reasons: &mut HashMap<Uuid, QueueWaitReason>,
    job: &JobRecord,
    reason: QueueWaitReason,
) where
    P: FnMut(QueueProgressUpdate),
{
    if last_wait_reasons.get(&job.id) == Some(&reason) {
        return;
    }
    last_wait_reasons.insert(job.id, reason);
    on_progress(queue_progress(job, Some(reason)));
}

fn queue_progress(job: &JobRecord, wait_reason: Option<QueueWaitReason>) -> QueueProgressUpdate {
    QueueProgressUpdate {
        schema_version: 1,
        job_id: job.id,
        job_sequence: job.sequence,
        state: job.state,
        wait_reason,
        occurred_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| i64::try_from(duration.as_millis()).ok())
            .unwrap_or_default(),
        eta_milliseconds: None,
    }
}

async fn inspect_queued_input(
    details: &JobDetails,
    stored_engine: &EngineIdentity,
) -> Result<(Probe, EngineIdentity)> {
    if stored_engine.engine_id == "formatwright.structured" {
        return Ok((
            inspect_structured(&details.job.input_path).await?,
            inspect_builtin_engine("formatwright.structured").await?,
        ));
    }
    if stored_engine.engine_id == "pandoc" {
        let validation_engine = if details.plan.target_format == "pdf" {
            inspect_engine("pdfinfo").await?
        } else {
            stored_engine.clone()
        };
        return Ok((
            inspect_document(&details.job.input_path).await?,
            validation_engine,
        ));
    }
    if stored_engine.engine_id == "pdftoppm" {
        let pdfinfo = inspect_engine("pdfinfo").await?;
        return Ok((
            inspect_pdf(&details.job.input_path, &pdfinfo).await?,
            inspect_engine("ffprobe").await?,
        ));
    }
    if stored_engine.engine_id == "soffice" {
        return Ok((
            inspect_office(&details.job.input_path).await?,
            inspect_engine("pdfinfo").await?,
        ));
    }
    let validation_engine = inspect_engine("ffprobe").await?;
    Ok((
        inspect_media(&details.job.input_path, &validation_engine).await?,
        validation_engine,
    ))
}

fn mark_job_validating(store: &mut SqliteJobStore, job_id: Uuid) -> Result<Option<JobRecord>> {
    let job = store.get_job(job_id)?.ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Store,
            format!("Active job disappeared: {job_id}"),
            "Run jobs recover and inspect the database.",
        )
    })?;
    if job.state == JobState::Running {
        return store
            .transition(job_id, JobState::Validating, "ENGINE_FINISHED")
            .map(Some);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    use super::{JobExecutionService, QueueRunReport, QueueWaitReason, QueueWindowControl};
    use crate::domain::{
        ChangeSet, JobState, NetworkPolicy, Plan, PlanRequest, SCHEMA_VERSION, ValidationStatus,
    };
    use crate::job_store::SqliteJobStore;
    use crate::structured::{inspect_structured, plan_structured_conversion};
    use crate::{ErrorCode, FormatWrightError, Stage};
    use crate::{inspect_builtin_engine, prepare_conversion};

    async fn queue_structured_job(
        store: &mut SqliteJobStore,
        input: &PathBuf,
        output: &PathBuf,
    ) -> uuid::Uuid {
        queue_structured_job_with_test_controls(store, input, output, None, false).await
    }

    async fn queue_structured_job_with_test_controls(
        store: &mut SqliteJobStore,
        input: &PathBuf,
        output: &PathBuf,
        delay_millis: Option<u64>,
        panic_worker: bool,
    ) -> uuid::Uuid {
        fs::write(input, r#"[{"id":1,"ok":true}]"#).expect("write JSON");
        let engine = inspect_builtin_engine("formatwright.structured")
            .await
            .expect("structured engine");
        let probe = inspect_structured(input).await.expect("inspect");
        let mut plan = plan_structured_conversion(
            &probe,
            &PlanRequest {
                target_format: "yaml".to_owned(),
                output_path: Some(output.clone()),
                ..PlanRequest::default()
            },
            &engine,
        )
        .expect("plan");
        if let Some(delay) = delay_millis {
            plan.constraints.insert(
                "__test_worker_delay_millis".to_owned(),
                serde_json::json!(delay),
            );
        }
        if panic_worker {
            plan.constraints
                .insert("__test_worker_panic".to_owned(), serde_json::json!(true));
        }
        let job = store.create_job(input, output, &plan).expect("create job");
        store
            .transition(job.id, JobState::Queued, "JOB_ENQUEUED")
            .expect("enqueue");
        job.id
    }

    fn assert_recoverable_and_release_reservation(store: &mut SqliteJobStore, job_id: uuid::Uuid) {
        assert_eq!(
            store.get_job(job_id).expect("read").expect("exists").state,
            JobState::Interrupted
        );
        let details = store
            .get_job_details(job_id)
            .expect("read details")
            .expect("details exist");
        assert_eq!(
            details.events.last().expect("last event").code,
            "CONTROL_PLANE_FAILED"
        );
        store
            .transition(job_id, JobState::Queued, "TEST_RETRY")
            .expect("interrupted job can be requeued");
        store
            .transition(job_id, JobState::Cancelled, "TEST_CLEANUP")
            .expect("cancelled retry releases its reservation");
    }

    #[test]
    fn completed_windows_leave_no_cancellation_link_tasks() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("isolated runtime");
        let metrics = runtime.metrics();
        let mut store = SqliteJobStore::open_in_memory().expect("store");
        for iteration in 0..64 {
            let report = runtime
                .block_on(JobExecutionService::run_window(
                    &mut store,
                    8,
                    1,
                    CancellationToken::new(),
                ))
                .expect("empty window");
            assert_eq!(report.selected, 0);
            assert_eq!(
                metrics.num_alive_tasks(),
                0,
                "window {iteration} left a detached Tokio task"
            );
        }
    }

    #[tokio::test]
    async fn rejects_parallelism_outside_one_to_sixteen() {
        let mut store = SqliteJobStore::open_in_memory().expect("store");
        let error = JobExecutionService::run_window(&mut store, 1, 0, CancellationToken::new())
            .await
            .expect_err("parallel 0");
        assert_eq!(error.code, ErrorCode::InputInvalid);
        let error = JobExecutionService::run_window(&mut store, 1, 17, CancellationToken::new())
            .await
            .expect_err("parallel 17");
        assert_eq!(error.code, ErrorCode::InputInvalid);
    }

    #[tokio::test]
    async fn rejects_scheduling_windows_above_256() {
        let mut store = SqliteJobStore::open_in_memory().expect("store");
        let error = JobExecutionService::run_window(&mut store, 257, 1, CancellationToken::new())
            .await
            .expect_err("limit 257");
        assert_eq!(error.code, ErrorCode::InputInvalid);
    }

    #[tokio::test]
    async fn completes_structured_queued_job_and_releases_resources() {
        let suite = tempdir().expect("suite");
        let database = suite.path().join("jobs.sqlite3");
        let mut store = SqliteJobStore::open(&database).expect("store");
        let input = suite.path().join("item.json");
        let output = suite.path().join("item.yaml");
        let job_id = queue_structured_job(&mut store, &input, &output).await;

        let report = JobExecutionService::run_window(&mut store, 16, 2, CancellationToken::new())
            .await
            .expect("run window");
        assert_eq!(
            report,
            QueueRunReport {
                schema_version: 1,
                selected: 1,
                completed: 1,
                warning: 0,
                blocked: 0,
                failed: 0,
                cancelled: 0,
                contended: 0,
                stopped: false,
                parallelism: 2,
                peak_active: 1,
            }
        );
        let job = store.get_job(job_id).expect("read").expect("exists");
        assert_eq!(job.state, JobState::Completed);
        assert!(output.is_file());

        let second_input = suite.path().join("item-2.json");
        let second_output = suite.path().join("item-2.yaml");
        let second_id = queue_structured_job(&mut store, &second_input, &second_output).await;
        let second = JobExecutionService::run_window(&mut store, 16, 1, CancellationToken::new())
            .await
            .expect("second window after release");
        assert_eq!(second.completed, 1);
        assert_eq!(second.peak_active, 1);
        assert_eq!(
            store
                .get_job(second_id)
                .expect("read")
                .expect("exists")
                .state,
            JobState::Completed
        );
    }

    #[tokio::test]
    async fn observed_report_callback_runs_before_terminal_state() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let job_id = queue_structured_job(
            &mut store,
            &suite.path().join("item.json"),
            &suite.path().join("item.yaml"),
        )
        .await;
        let reports = suite.path().join("reports");
        fs::create_dir_all(&reports).expect("reports dir");
        let captured = std::sync::Mutex::new(None);
        let report = JobExecutionService::run_window_observed(
            &mut store,
            8,
            1,
            QueueWindowControl::new(),
            |id, validation| {
                assert_eq!(id, job_id);
                assert_eq!(validation.status, ValidationStatus::Pass);
                let path = reports.join(format!("{id}.json"));
                let bytes = serde_json::to_vec_pretty(validation).expect("serialize");
                fs::write(&path, bytes).expect("write report");
                *captured.lock().expect("lock") = Some(path);
                Ok(())
            },
        )
        .await
        .expect("run observed");
        assert_eq!(report.completed, 1);
        let path = captured.lock().expect("lock").clone().expect("path");
        assert!(path.is_file());
        assert_eq!(
            store.get_job(job_id).expect("read").expect("exists").state,
            JobState::Completed
        );
    }

    #[tokio::test]
    async fn progress_reports_real_stages_and_scheduler_wait_reason_without_eta() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let first = queue_structured_job_with_test_controls(
            &mut store,
            &suite.path().join("first.json"),
            &suite.path().join("first.yaml"),
            Some(100),
            false,
        )
        .await;
        let second = queue_structured_job(
            &mut store,
            &suite.path().join("second.json"),
            &suite.path().join("second.yaml"),
        )
        .await;
        let mut progress = Vec::new();

        let report = JobExecutionService::run_window_observed_with_progress(
            &mut store,
            8,
            1,
            QueueWindowControl::new(),
            |_, _| Ok(()),
            |update| progress.push(update),
        )
        .await
        .expect("run with progress");

        assert_eq!(report.completed, 2);
        assert!(
            progress
                .iter()
                .all(|update| update.eta_milliseconds.is_none())
        );
        assert!(progress.iter().any(|update| {
            update.job_id == first
                && update.state == JobState::Running
                && update.wait_reason.is_none()
        }));
        assert!(progress.iter().any(|update| {
            update.job_id == second
                && update.state == JobState::Queued
                && update.wait_reason == Some(QueueWaitReason::ProcessLimit)
        }));
        assert!(progress.iter().any(|update| {
            update.job_id == second
                && update.state == JobState::Completed
                && update.wait_reason.is_none()
        }));
    }

    #[tokio::test]
    async fn report_storage_failure_cancels_drains_and_interrupts_all_workers() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let first = queue_structured_job(
            &mut store,
            &suite.path().join("first.json"),
            &suite.path().join("first.yaml"),
        )
        .await;
        let second = queue_structured_job_with_test_controls(
            &mut store,
            &suite.path().join("second.json"),
            &suite.path().join("second.yaml"),
            Some(250),
            false,
        )
        .await;
        let control = QueueWindowControl::new();
        let observed_control = control.clone();

        let error = JobExecutionService::run_window_observed(&mut store, 8, 2, control, |_, _| {
            Err(FormatWrightError::new(
                ErrorCode::StorageFailed,
                Stage::Store,
                "injected report storage failure",
                "retry",
            ))
        })
        .await
        .expect_err("report persistence must fail the window");

        assert_eq!(error.code, ErrorCode::StorageFailed);
        assert!(observed_control.workers_cancelled());
        assert_recoverable_and_release_reservation(&mut store, first);
        assert_recoverable_and_release_reservation(&mut store, second);
        assert_eq!(
            fs::read_dir(suite.path())
                .expect("read suite")
                .filter_map(std::result::Result::ok)
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .contains("formatwright-partial"))
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn worker_panic_cancels_drains_and_interrupts_peer_workers() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let panicked = queue_structured_job_with_test_controls(
            &mut store,
            &suite.path().join("panic.json"),
            &suite.path().join("panic.yaml"),
            None,
            true,
        )
        .await;
        let peer = queue_structured_job_with_test_controls(
            &mut store,
            &suite.path().join("peer.json"),
            &suite.path().join("peer.yaml"),
            Some(250),
            false,
        )
        .await;
        let control = QueueWindowControl::new();
        let observed_control = control.clone();

        let error =
            JobExecutionService::run_window_observed(&mut store, 8, 2, control, |_, _| Ok(()))
                .await
                .expect_err("worker panic must fail the window");

        assert_eq!(error.code, ErrorCode::Internal);
        assert!(error.message.contains("stopped unexpectedly"));
        assert!(observed_control.workers_cancelled());
        assert_recoverable_and_release_reservation(&mut store, panicked);
        assert_recoverable_and_release_reservation(&mut store, peer);
    }

    #[tokio::test]
    async fn finish_current_pause_leaves_hydrated_jobs_queued() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let first = queue_structured_job(
            &mut store,
            &suite.path().join("a.json"),
            &suite.path().join("a.yaml"),
        )
        .await;
        let second = queue_structured_job(
            &mut store,
            &suite.path().join("b.json"),
            &suite.path().join("b.yaml"),
        )
        .await;
        let control = QueueWindowControl::new();
        control.pause_finish_current();
        let report =
            JobExecutionService::run_window_observed(&mut store, 16, 2, control, |_, _| Ok(()))
                .await
                .expect("finish-current window");
        assert_eq!(report.selected, 2);
        assert_eq!(report.completed, 0);
        assert_eq!(report.cancelled, 0);
        assert!(report.stopped);
        assert_eq!(
            store.get_job(first).expect("read").expect("exists").state,
            JobState::Queued
        );
        assert_eq!(
            store.get_job(second).expect("read").expect("exists").state,
            JobState::Queued
        );
    }

    #[tokio::test]
    async fn in_flight_finish_current_pause_drains_only_the_admitted_worker() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let first_output = suite.path().join("first.yaml");
        let first = queue_structured_job_with_test_controls(
            &mut store,
            &suite.path().join("first.json"),
            &first_output,
            Some(250),
            false,
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let second_output = suite.path().join("second.yaml");
        let second = queue_structured_job(
            &mut store,
            &suite.path().join("second.json"),
            &second_output,
        )
        .await;
        let control = QueueWindowControl::new();
        let pause = control.clone();
        let pause_task = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            pause.pause_finish_current();
        });

        let paused =
            JobExecutionService::run_window_observed(&mut store, 8, 1, control, |_, _| Ok(()))
                .await
                .expect("finish-current window");
        pause_task.await.expect("pause task");
        assert!(paused.stopped);
        assert_eq!(paused.completed, 1);
        assert_eq!(paused.cancelled, 0);
        assert_eq!(
            store.get_job(first).expect("read").expect("exists").state,
            JobState::Completed
        );
        assert_eq!(
            store.get_job(second).expect("read").expect("exists").state,
            JobState::Queued
        );
        assert!(first_output.is_file());
        assert!(!second_output.exists());

        let resumed = JobExecutionService::run_window(&mut store, 8, 1, CancellationToken::new())
            .await
            .expect("next window");
        assert_eq!(resumed.completed, 1);
        assert_eq!(
            store.get_job(second).expect("read").expect("exists").state,
            JobState::Completed
        );
        assert!(second_output.is_file());
    }

    #[tokio::test]
    async fn immediate_pause_also_leaves_unstarted_jobs_queued() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let job_id = queue_structured_job(
            &mut store,
            &suite.path().join("item.json"),
            &suite.path().join("item.yaml"),
        )
        .await;
        let control = QueueWindowControl::new();
        control.pause_immediate();
        assert!(control.admission_cancelled());
        assert!(control.workers_cancelled());
        let report =
            JobExecutionService::run_window_observed(&mut store, 8, 1, control, |_, _| Ok(()))
                .await
                .expect("immediate pause");
        assert!(report.stopped);
        assert_eq!(report.completed, 0);
        assert_eq!(
            store.get_job(job_id).expect("read").expect("exists").state,
            JobState::Queued
        );
    }

    #[tokio::test]
    async fn in_flight_immediate_pause_is_recoverable_in_the_next_window() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let input = suite.path().join("item.json");
        let output = suite.path().join("item.yaml");
        let job_id =
            queue_structured_job_with_test_controls(&mut store, &input, &output, Some(250), false)
                .await;
        let control = QueueWindowControl::new();
        let pause = control.clone();
        let pause_task = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            pause.pause_immediate();
        });

        let paused =
            JobExecutionService::run_window_observed(&mut store, 8, 1, control, |_, _| Ok(()))
                .await
                .expect("pause window");
        pause_task.await.expect("pause task");
        assert!(paused.stopped);
        assert_eq!(paused.cancelled, 1);
        let details = store
            .get_job_details(job_id)
            .expect("read details")
            .expect("details");
        assert_eq!(details.job.state, JobState::Interrupted);
        assert_eq!(
            details.events.last().expect("last event").code,
            "QUEUE_PAUSED_IMMEDIATE"
        );
        assert!(!output.exists());

        store
            .transition(job_id, JobState::Queued, "DESKTOP_JOB_RESUMED")
            .expect("resume interrupted job");
        let resumed = JobExecutionService::run_window(&mut store, 8, 1, CancellationToken::new())
            .await
            .expect("resume window");
        assert_eq!(resumed.completed, 1);
        assert_eq!(
            store.get_job(job_id).expect("read").expect("exists").state,
            JobState::Completed
        );
        assert!(output.is_file());
    }

    #[tokio::test]
    async fn external_cancellation_after_admission_is_drained_before_return() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let output = suite.path().join("external-cancel.yaml");
        let job_id = queue_structured_job_with_test_controls(
            &mut store,
            &suite.path().join("external-cancel.json"),
            &output,
            Some(250),
            false,
        )
        .await;
        let cancellation = CancellationToken::new();
        let trigger = cancellation.clone();
        let cancellation_task = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            trigger.cancel();
        });

        let report = JobExecutionService::run_window(&mut store, 8, 1, cancellation)
            .await
            .expect("cancelled window drains");
        cancellation_task.await.expect("cancellation task");
        assert!(report.stopped);
        assert_eq!(report.cancelled, 1);
        let details = store
            .get_job_details(job_id)
            .expect("read details")
            .expect("details");
        assert_eq!(details.job.state, JobState::Interrupted);
        assert_eq!(
            details.events.last().expect("last event").code,
            "QUEUE_PAUSED_IMMEDIATE"
        );
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn pre_cancelled_token_stops_admission_without_mutating_queued_jobs() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let job_id = queue_structured_job(
            &mut store,
            &suite.path().join("item.json"),
            &suite.path().join("item.yaml"),
        )
        .await;
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let report = JobExecutionService::run_window(&mut store, 16, 1, cancellation)
            .await
            .expect("cancelled window");
        assert_eq!(report.selected, 1);
        assert_eq!(report.completed, 0);
        assert!(report.stopped);
        assert_eq!(report.peak_active, 0);
        assert_eq!(
            store.get_job(job_id).expect("read").expect("exists").state,
            JobState::Queued
        );
    }

    #[tokio::test]
    async fn blocked_preparation_releases_slot_so_later_job_can_complete() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");

        // Oldest queued job is blocked so the scheduler must release its slot
        // before the newer valid job can complete in the same window.
        let bad_input = suite.path().join("bad.json");
        let bad_output = suite.path().join("bad.yaml");
        fs::write(&bad_input, r#"[{"id":2,"ok":true}]"#).expect("write");
        let (_, mut plan, _) = prepare_conversion(
            &bad_input,
            &PlanRequest {
                target_format: "yaml".to_owned(),
                output_path: Some(bad_output.clone()),
                ..PlanRequest::default()
            },
        )
        .await
        .expect("plan bad");
        plan.input_fingerprint = "fwfp-v1:stale".to_owned();
        let bad_job = store
            .create_job(&bad_input, &bad_output, &plan)
            .expect("create bad");
        store
            .transition(bad_job.id, JobState::Queued, "JOB_ENQUEUED")
            .expect("enqueue bad");

        let good_input = suite.path().join("good.json");
        let good_output = suite.path().join("good.yaml");
        let good_id = queue_structured_job(&mut store, &good_input, &good_output).await;

        let report = JobExecutionService::run_window(&mut store, 16, 1, CancellationToken::new())
            .await
            .expect("mixed window");
        assert_eq!(report.selected, 2);
        assert_eq!(report.blocked, 1);
        assert_eq!(report.completed, 1);
        assert_eq!(report.failed, 0);
        assert_eq!(
            store
                .get_job(bad_job.id)
                .expect("read")
                .expect("exists")
                .state,
            JobState::Blocked
        );
        assert_eq!(
            store.get_job(good_id).expect("read").expect("exists").state,
            JobState::Completed
        );
        assert!(good_output.is_file());
        assert!(!bad_output.exists());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn retargeted_output_link_is_blocked_before_worker_execution() {
        use std::os::windows::fs::symlink_dir;

        let suite = tempdir().expect("suite");
        let first_target = suite.path().join("first-target");
        let second_target = suite.path().join("second-target");
        let alias = suite.path().join("output-link");
        fs::create_dir(&first_target).expect("create first target");
        fs::create_dir(&second_target).expect("create second target");
        if let Err(error) = symlink_dir(&first_target, &alias) {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                eprintln!("skipped queue reparse-retarget assertion: {error}");
                return;
            }
            panic!("create directory symlink: {error}");
        }

        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let input = suite.path().join("input.json");
        let output = alias.join("output.yaml");
        let job_id = queue_structured_job(&mut store, &input, &output).await;
        fs::remove_dir(&alias).expect("remove output symlink");
        symlink_dir(&second_target, &alias).expect("retarget output symlink");

        let report = JobExecutionService::run_window(&mut store, 8, 1, CancellationToken::new())
            .await
            .expect("run guarded window");
        assert_eq!(report.blocked, 1);
        assert_eq!(report.completed, 0);
        let details = store
            .get_job_details(job_id)
            .expect("read details")
            .expect("details");
        assert_eq!(details.job.state, JobState::Blocked);
        assert_eq!(
            details.events.last().expect("last event").code,
            "OUTPUT_IDENTITY_CHANGED"
        );
        assert!(!second_target.join("output.yaml").exists());
    }

    #[tokio::test]
    async fn invalid_plan_without_steps_marks_job_failed() {
        let suite = tempdir().expect("suite");
        let mut store = SqliteJobStore::open(suite.path().join("jobs.sqlite3")).expect("store");
        let input = suite.path().join("empty-plan.json");
        let output = suite.path().join("empty-plan.yaml");
        fs::write(&input, r#"[{"id":3}]"#).expect("write");
        let plan = Plan {
            schema_version: SCHEMA_VERSION,
            plan_id: uuid::Uuid::new_v4(),
            plan_hash: "blake3:empty".to_owned(),
            input_fingerprint: "fwfp-v1:unused".to_owned(),
            target_format: "yaml".to_owned(),
            constraints: std::collections::BTreeMap::new(),
            steps: Vec::new(),
            changes: ChangeSet::default(),
            validators: Vec::new(),
            network_policy: NetworkPolicy::Deny,
            output_path: Some(output.clone()),
            estimated_output_bytes: None,
        };
        let job = store.create_job(&input, &output, &plan).expect("create");
        store
            .transition(job.id, JobState::Queued, "JOB_ENQUEUED")
            .expect("enqueue");
        let report = JobExecutionService::run_window(&mut store, 8, 1, CancellationToken::new())
            .await
            .expect("run");
        assert_eq!(report.failed, 1);
        assert_eq!(report.completed, 0);
        assert_eq!(
            store.get_job(job.id).expect("read").expect("exists").state,
            JobState::Failed
        );
    }
}
