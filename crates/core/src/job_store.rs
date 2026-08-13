use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{
    Connection, ErrorCode as SqliteErrorCode, OptionalExtension, Transaction, TransactionBehavior,
    params, params_from_iter, types::Value,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::{JobState, Plan, SCHEMA_VERSION};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::maintenance::automatic_snapshot_before_migration;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JobRecord {
    pub id: Uuid,
    pub state: JobState,
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub plan_hash: String,
    pub sequence: u64,
    pub created_unix_ms: i64,
    pub updated_unix_ms: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JobProgress {
    pub completed: f64,
    pub total: Option<f64>,
    pub unit: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JobEventRecord {
    pub schema_version: u32,
    pub event_id: Uuid,
    pub job_id: Uuid,
    pub sequence: u64,
    pub previous_state: Option<JobState>,
    pub next_state: JobState,
    pub code: String,
    pub timestamp_unix_ms: i64,
    pub progress: Option<JobProgress>,
    pub data: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JobDetails {
    pub job: JobRecord,
    pub plan: Plan,
    pub events: Vec<JobEventRecord>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JobCreateRequest {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub plan: Plan,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BatchRecord {
    pub id: Uuid,
    pub name: String,
    pub job_count: u64,
    pub created_unix_ms: i64,
    pub updated_unix_ms: i64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct JobSelectionQuery {
    pub batch_id: Option<Uuid>,
    pub states: Vec<JobState>,
    pub search: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JobQueryPage {
    pub jobs: Vec<JobRecord>,
    pub total: u64,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JobStateCount {
    pub state: JobState,
    pub count: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SelectionSnapshot {
    pub id: Uuid,
    pub query: JobSelectionQuery,
    pub member_count: u64,
    pub created_unix_ms: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BulkJobAction {
    Cancel,
    Resume,
    Retry,
}

impl BulkJobAction {
    pub(crate) const fn target_for(self, state: JobState) -> Option<JobState> {
        match self {
            Self::Cancel
                if matches!(
                    state,
                    JobState::Planned
                        | JobState::Queued
                        | JobState::Blocked
                        | JobState::Interrupted
                ) =>
            {
                Some(JobState::Cancelled)
            }
            Self::Resume if matches!(state, JobState::Blocked | JobState::Interrupted) => {
                Some(JobState::Queued)
            }
            Self::Retry
                if matches!(
                    state,
                    JobState::Failed | JobState::Cancelled | JobState::Interrupted
                ) =>
            {
                Some(JobState::Queued)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BulkActionReport {
    pub action_id: Uuid,
    pub selection_id: Uuid,
    pub action: BulkJobAction,
    pub matched: u64,
    pub transitioned: u64,
    pub skipped_state: u64,
    pub skipped_conflict: u64,
    pub created_unix_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IdempotentJobResult {
    pub job: JobRecord,
    pub created: bool,
}

#[derive(Debug)]
pub struct SqliteJobStore {
    connection: Connection,
}

impl SqliteJobStore {
    /// Opens or creates a persistent job database and applies migrations.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the database cannot open or migrate the file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        automatic_snapshot_before_migration(path)?;
        Self::open_for_restore_staging(path)
    }

    pub(crate) fn open_for_restore_staging(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path).map_err(storage_error)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(storage_error)?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    /// Opens a migrated in-memory database for tests and ephemeral work.
    ///
    /// # Errors
    ///
    /// Returns a storage error when database initialization fails.
    pub fn open_in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory().map_err(storage_error)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(storage_error)?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<()> {
        self.connection
            .execute_batch(
                "
                PRAGMA foreign_keys = ON;
                PRAGMA journal_mode = WAL;
                CREATE TABLE IF NOT EXISTS schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_unix_ms INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS jobs (
                    id TEXT PRIMARY KEY,
                    state TEXT NOT NULL,
                    input_path TEXT NOT NULL,
                    output_path TEXT NOT NULL,
                    plan_hash TEXT NOT NULL,
                    plan_json TEXT NOT NULL,
                    sequence INTEGER NOT NULL,
                    created_unix_ms INTEGER NOT NULL,
                    updated_unix_ms INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS job_events (
                    event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                    sequence INTEGER NOT NULL,
                    previous_state TEXT,
                    next_state TEXT,
                    code TEXT NOT NULL,
                    timestamp_unix_ms INTEGER NOT NULL,
                    UNIQUE(job_id, sequence)
                );
                CREATE INDEX IF NOT EXISTS idx_jobs_state_updated
                    ON jobs(state, updated_unix_ms);
                CREATE TABLE IF NOT EXISTS output_reservations (
                    canonical_output_path TEXT PRIMARY KEY,
                    job_id TEXT NOT NULL UNIQUE REFERENCES jobs(id) ON DELETE CASCADE,
                    created_unix_ms INTEGER NOT NULL
                );
                INSERT OR IGNORE INTO schema_migrations(version, applied_unix_ms)
                    VALUES (1, 0);
                INSERT OR IGNORE INTO schema_migrations(version, applied_unix_ms)
                    VALUES (2, 0);
                INSERT OR IGNORE INTO output_reservations(
                    canonical_output_path, job_id, created_unix_ms
                )
                    SELECT output_path, id, updated_unix_ms FROM jobs
                    WHERE state NOT IN ('completed', 'warning', 'failed', 'cancelled');
                ",
            )
            .map_err(storage_error)?;
        self.migrate_output_reservation_identity()?;
        self.migrate_batch_selection_schema()
    }

    fn migrate_output_reservation_identity(&mut self) -> Result<()> {
        let already_applied = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 3)",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_error)?;
        if already_applied {
            return Ok(());
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let active_reservations = {
            let mut statement = transaction
                .prepare(
                    "SELECT jobs.id,
                            COALESCE(output_reservations.canonical_output_path, jobs.output_path),
                            jobs.updated_unix_ms
                     FROM jobs
                     LEFT JOIN output_reservations ON output_reservations.job_id = jobs.id
                     WHERE jobs.state NOT IN ('completed', 'warning', 'failed', 'cancelled')
                     ORDER BY jobs.created_unix_ms ASC, jobs.id ASC",
                )
                .map_err(storage_error)?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .map_err(storage_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(storage_error)?
        };
        transaction
            .execute("DELETE FROM output_reservations", [])
            .map_err(storage_error)?;
        for (job_id, prior_key, updated_unix_ms) in active_reservations {
            let key = reservation_key(Path::new(&prior_key))?;
            transaction
                .execute(
                    "INSERT INTO output_reservations(
                        canonical_output_path, job_id, created_unix_ms
                     ) VALUES (?1, ?2, ?3)",
                    params![key, job_id, updated_unix_ms],
                )
                .map_err(output_reservation_error)?;
        }
        transaction
            .execute(
                "INSERT INTO schema_migrations(version, applied_unix_ms) VALUES (3, ?1)",
                [now_unix_ms()],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }

    fn migrate_batch_selection_schema(&mut self) -> Result<()> {
        let already_applied = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 4)",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_error)?;
        if already_applied {
            return Ok(());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        transaction
            .execute_batch(
                "CREATE TABLE batches (
                     id TEXT PRIMARY KEY,
                     name TEXT NOT NULL,
                     created_unix_ms INTEGER NOT NULL,
                     updated_unix_ms INTEGER NOT NULL
                 );
                 CREATE TABLE batch_members (
                     batch_id TEXT NOT NULL REFERENCES batches(id) ON DELETE CASCADE,
                     job_id TEXT NOT NULL UNIQUE REFERENCES jobs(id) ON DELETE CASCADE,
                     ordinal INTEGER NOT NULL,
                     PRIMARY KEY(batch_id, ordinal)
                 );
                 CREATE INDEX idx_batch_members_job ON batch_members(job_id);
                 CREATE TABLE job_idempotency_keys (
                     idempotency_key TEXT PRIMARY KEY,
                     job_id TEXT NOT NULL UNIQUE REFERENCES jobs(id) ON DELETE CASCADE,
                     created_unix_ms INTEGER NOT NULL
                 );
                 CREATE TABLE selection_snapshots (
                     id TEXT PRIMARY KEY,
                     query_json TEXT NOT NULL,
                     member_count INTEGER NOT NULL,
                     created_unix_ms INTEGER NOT NULL
                 );
                 CREATE TABLE selection_members (
                     selection_id TEXT NOT NULL
                         REFERENCES selection_snapshots(id) ON DELETE CASCADE,
                     job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                     ordinal INTEGER NOT NULL,
                     PRIMARY KEY(selection_id, ordinal),
                     UNIQUE(selection_id, job_id)
                 );
                 CREATE TABLE bulk_actions (
                     action_id TEXT PRIMARY KEY,
                     selection_id TEXT NOT NULL REFERENCES selection_snapshots(id),
                     action TEXT NOT NULL,
                     matched_count INTEGER NOT NULL,
                     transitioned_count INTEGER NOT NULL,
                     skipped_state_count INTEGER NOT NULL,
                     skipped_conflict_count INTEGER NOT NULL,
                     created_unix_ms INTEGER NOT NULL
                 );
                 CREATE TABLE bulk_action_members (
                     action_id TEXT NOT NULL REFERENCES bulk_actions(action_id) ON DELETE CASCADE,
                     job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                     outcome TEXT NOT NULL,
                     previous_state TEXT NOT NULL,
                     next_state TEXT,
                     PRIMARY KEY(action_id, job_id)
                 );",
            )
            .map_err(storage_error)?;
        transaction
            .execute(
                "INSERT INTO schema_migrations(version, applied_unix_ms) VALUES (4, ?1)",
                [now_unix_ms()],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }

    /// Creates a durable planned job and its initial event atomically.
    ///
    /// # Errors
    ///
    /// Returns a storage or serialization error when the job cannot be saved.
    pub fn create_job(
        &mut self,
        input_path: impl AsRef<Path>,
        output_path: impl AsRef<Path>,
        plan: &Plan,
    ) -> Result<JobRecord> {
        let request = JobCreateRequest {
            input_path: input_path.as_ref().to_path_buf(),
            output_path: output_path.as_ref().to_path_buf(),
            plan: plan.clone(),
        };
        self.create_jobs(std::slice::from_ref(&request))?
            .pop()
            .ok_or_else(|| {
                FormatWrightError::new(
                    ErrorCode::Internal,
                    Stage::Store,
                    "Bulk job creation returned no job",
                    "Retry creating the job.",
                )
            })
    }

    /// Creates a batch of planned jobs, events, and output reservations in one
    /// transaction. Any conflict rolls back the complete batch.
    ///
    /// # Errors
    ///
    /// Returns an output-conflict, storage, or serialization error without
    /// leaving a partially inserted batch.
    pub fn create_jobs(&mut self, requests: &[JobCreateRequest]) -> Result<Vec<JobRecord>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        let now = now_unix_ms();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let records = insert_jobs(&transaction, requests, now)?;
        transaction.commit().map_err(storage_error)?;
        Ok(records)
    }

    /// Creates one persistent batch and all of its planned jobs atomically.
    ///
    /// # Errors
    ///
    /// Returns an input, output-conflict, serialization, or storage error
    /// without leaving a partial batch.
    pub fn create_batch(
        &mut self,
        name: &str,
        requests: &[JobCreateRequest],
    ) -> Result<BatchRecord> {
        self.create_batch_inner(name, requests, false)
    }

    /// Creates and queues one persistent batch in a single transaction.
    ///
    /// # Errors
    ///
    /// Returns an input, output-conflict, serialization, or storage error
    /// without leaving a partial batch or any planned-only members.
    pub fn create_queued_batch(
        &mut self,
        name: &str,
        requests: &[JobCreateRequest],
    ) -> Result<BatchRecord> {
        self.create_batch_inner(name, requests, true)
    }

    fn create_batch_inner(
        &mut self,
        name: &str,
        requests: &[JobCreateRequest],
        enqueue: bool,
    ) -> Result<BatchRecord> {
        let name = validate_batch_name(name)?;
        if requests.is_empty() || requests.len() > 100_000 {
            return Err(FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Store,
                "A batch must contain between 1 and 100,000 jobs",
                "Choose a non-empty bounded batch.",
            ));
        }
        let id = Uuid::new_v4();
        let now = now_unix_ms();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        transaction
            .execute(
                "INSERT INTO batches(id, name, created_unix_ms, updated_unix_ms)
                 VALUES (?1, ?2, ?3, ?3)",
                params![id.to_string(), name, now],
            )
            .map_err(storage_error)?;
        let jobs = insert_jobs(&transaction, requests, now)?;
        for (ordinal, job) in jobs.iter().enumerate() {
            transaction
                .execute(
                    "INSERT INTO batch_members(batch_id, job_id, ordinal)
                     VALUES (?1, ?2, ?3)",
                    params![
                        id.to_string(),
                        job.id.to_string(),
                        i64::try_from(ordinal).unwrap_or(i64::MAX)
                    ],
                )
                .map_err(storage_error)?;
        }
        if enqueue {
            for job in &jobs {
                transaction
                    .execute(
                        "UPDATE jobs
                         SET state = 'queued', sequence = 1, updated_unix_ms = ?1
                         WHERE id = ?2 AND state = 'planned' AND sequence = 0",
                        params![now, job.id.to_string()],
                    )
                    .map_err(storage_error)?;
                transaction
                    .execute(
                        "INSERT INTO job_events(
                            job_id, sequence, previous_state, next_state, code,
                            timestamp_unix_ms
                         ) VALUES (?1, 1, 'planned', 'queued', 'BATCH_QUEUED', ?2)",
                        params![job.id.to_string(), now],
                    )
                    .map_err(storage_error)?;
            }
        }
        transaction.commit().map_err(storage_error)?;
        Ok(BatchRecord {
            id,
            name,
            job_count: u64::try_from(jobs.len()).unwrap_or(u64::MAX),
            created_unix_ms: now,
            updated_unix_ms: now,
        })
    }

    /// Returns an existing job for the same idempotency key and intent, or
    /// creates and binds a new job atomically.
    ///
    /// # Errors
    ///
    /// Returns a policy error when a key is reused for a different intent.
    #[allow(clippy::too_many_lines)]
    pub fn create_job_idempotent(
        &mut self,
        idempotency_key: &str,
        request: &JobCreateRequest,
    ) -> Result<IdempotentJobResult> {
        self.create_job_idempotent_with_state(idempotency_key, request, false)
    }

    /// Creates and queues a job in the same transaction that binds its
    /// idempotency key. A replay returns the already-bound current job.
    ///
    /// # Errors
    ///
    /// Returns a policy error when a key is reused for a different intent.
    pub fn enqueue_job_idempotent(
        &mut self,
        idempotency_key: &str,
        request: &JobCreateRequest,
    ) -> Result<IdempotentJobResult> {
        self.create_job_idempotent_with_state(idempotency_key, request, true)
    }

    #[allow(clippy::too_many_lines)]
    fn create_job_idempotent_with_state(
        &mut self,
        idempotency_key: &str,
        request: &JobCreateRequest,
        enqueue: bool,
    ) -> Result<IdempotentJobResult> {
        let idempotency_key = validate_idempotency_key(idempotency_key)?;
        let now = now_unix_ms();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let existing = transaction
            .query_row(
                "SELECT jobs.id, jobs.state, jobs.input_path, jobs.output_path,
                        jobs.plan_hash, jobs.sequence, jobs.created_unix_ms,
                        jobs.updated_unix_ms
                 FROM job_idempotency_keys
                 JOIN jobs ON jobs.id = job_idempotency_keys.job_id
                 WHERE job_idempotency_keys.idempotency_key = ?1",
                [&idempotency_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, u64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?;
        if let Some(row) = existing {
            let same_intent = row.2 == request.input_path.to_string_lossy()
                && row.4 == request.plan.plan_hash
                && reservation_key(Path::new(&row.3))? == reservation_key(&request.output_path)?;
            if !same_intent {
                return Err(FormatWrightError::new(
                    ErrorCode::PolicyBlocked,
                    Stage::Store,
                    "The idempotency key is already bound to a different job intent",
                    "Reuse the key only for an identical input, Plan, and output identity.",
                ));
            }
            let job = job_record_from_row(row)?;
            transaction.commit().map_err(storage_error)?;
            return Ok(IdempotentJobResult {
                job,
                created: false,
            });
        }
        let mut jobs = insert_jobs(&transaction, std::slice::from_ref(request), now)?;
        let mut job = jobs.pop().ok_or_else(|| {
            FormatWrightError::new(
                ErrorCode::Internal,
                Stage::Store,
                "Idempotent job creation produced no job",
                "Retry creating the job.",
            )
        })?;
        transaction
            .execute(
                "INSERT INTO job_idempotency_keys(idempotency_key, job_id, created_unix_ms)
                 VALUES (?1, ?2, ?3)",
                params![idempotency_key, job.id.to_string(), now],
            )
            .map_err(storage_error)?;
        if enqueue {
            job.state = JobState::Queued;
            job.sequence = 1;
            transaction
                .execute(
                    "UPDATE jobs SET state = 'queued', sequence = 1, updated_unix_ms = ?1
                     WHERE id = ?2 AND state = 'planned' AND sequence = 0",
                    params![now, job.id.to_string()],
                )
                .map_err(storage_error)?;
            transaction
                .execute(
                    "INSERT INTO job_events(
                        job_id, sequence, previous_state, next_state, code, timestamp_unix_ms
                     ) VALUES (?1, 1, 'planned', 'queued', 'JOB_ENQUEUED', ?2)",
                    params![job.id.to_string(), now],
                )
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(IdempotentJobResult { job, created: true })
    }

    /// Moves a complete planned batch into the durable queue in one
    /// transaction. Any missing job, invalid state, or reservation failure
    /// rolls back every transition in the batch.
    ///
    /// # Errors
    ///
    /// Returns a storage error without partially queueing the supplied IDs.
    #[allow(clippy::too_many_lines)]
    pub fn queue_jobs(&mut self, job_ids: &[Uuid], code: &str) -> Result<Vec<JobRecord>> {
        if job_ids.is_empty() {
            return Ok(Vec::new());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let now = now_unix_ms();
        let next = JobState::Queued;
        let mut records = Vec::with_capacity(job_ids.len());
        for job_id in job_ids {
            let row = transaction
                .query_row(
                    "SELECT state, input_path, output_path, plan_hash, sequence,
                            created_unix_ms
                     FROM jobs WHERE id = ?1",
                    [job_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, u64>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .optional()
                .map_err(storage_error)?
                .ok_or_else(|| {
                    FormatWrightError::new(
                        ErrorCode::StorageFailed,
                        Stage::Store,
                        format!("Job does not exist: {job_id}"),
                        "Refresh the job list.",
                    )
                })?;
            let current = parse_state(&row.0)?;
            if !current.can_transition_to(next) {
                return Err(FormatWrightError::new(
                    ErrorCode::StorageFailed,
                    Stage::Store,
                    format!("Invalid job transition: {current:?} -> {next:?}"),
                    "Refresh the job and retry a valid action.",
                ));
            }
            reserve_output(&transaction, *job_id, Path::new(&row.2), now)?;
            let sequence = row.4.saturating_add(1);
            let updated = transaction
                .execute(
                    "UPDATE jobs
                     SET state = ?1, sequence = ?2, updated_unix_ms = ?3
                     WHERE id = ?4 AND state = ?5 AND sequence = ?6",
                    params![
                        state_name(next),
                        sequence,
                        now,
                        job_id.to_string(),
                        state_name(current),
                        row.4
                    ],
                )
                .map_err(storage_error)?;
            if updated != 1 {
                return Err(FormatWrightError::new(
                    ErrorCode::StorageFailed,
                    Stage::Store,
                    "Job state changed concurrently",
                    "Refresh the job and retry.",
                )
                .retryable(true));
            }
            transaction
                .execute(
                    "INSERT INTO job_events(
                        job_id, sequence, previous_state, next_state, code, timestamp_unix_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        job_id.to_string(),
                        sequence,
                        state_name(current),
                        state_name(next),
                        code,
                        now
                    ],
                )
                .map_err(storage_error)?;
            records.push(JobRecord {
                id: *job_id,
                state: next,
                input_path: PathBuf::from(row.1),
                output_path: PathBuf::from(row.2),
                plan_hash: row.3,
                sequence,
                created_unix_ms: row.5,
                updated_unix_ms: now,
            });
        }
        transaction.commit().map_err(storage_error)?;
        Ok(records)
    }

    /// Applies a validated state transition and event in one transaction.
    ///
    /// # Errors
    ///
    /// Returns a storage error for missing jobs, invalid transitions, write
    /// conflicts, or database failures.
    #[allow(clippy::too_many_lines)]
    pub fn transition(&mut self, job_id: Uuid, next: JobState, code: &str) -> Result<JobRecord> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let row = transaction
            .query_row(
                "SELECT state, input_path, output_path, plan_hash, sequence,
                        created_unix_ms, updated_unix_ms
                 FROM jobs WHERE id = ?1",
                [job_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, u64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?
            .ok_or_else(|| {
                FormatWrightError::new(
                    ErrorCode::StorageFailed,
                    Stage::Store,
                    format!("Job does not exist: {job_id}"),
                    "Refresh the job list.",
                )
            })?;
        let current = parse_state(&row.0)?;
        if !current.can_transition_to(next) {
            return Err(FormatWrightError::new(
                ErrorCode::StorageFailed,
                Stage::Store,
                format!("Invalid job transition: {current:?} -> {next:?}"),
                "Refresh the job and retry a valid action.",
            ));
        }
        let now = now_unix_ms();
        if next == JobState::Queued {
            reserve_output(&transaction, job_id, Path::new(&row.2), now)?;
        }
        let sequence = row.4.saturating_add(1);
        transaction
            .execute(
                "UPDATE jobs
                 SET state = ?1, sequence = ?2, updated_unix_ms = ?3
                 WHERE id = ?4 AND state = ?5 AND sequence = ?6",
                params![
                    state_name(next),
                    sequence,
                    now,
                    job_id.to_string(),
                    state_name(current),
                    row.4
                ],
            )
            .map_err(storage_error)
            .and_then(|updated| {
                if updated == 1 {
                    Ok(())
                } else {
                    Err(FormatWrightError::new(
                        ErrorCode::StorageFailed,
                        Stage::Store,
                        "Job state changed concurrently",
                        "Refresh the job and retry.",
                    )
                    .retryable(true))
                }
            })?;
        transaction
            .execute(
                "INSERT INTO job_events(
                    job_id, sequence, previous_state, next_state, code, timestamp_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    job_id.to_string(),
                    sequence,
                    state_name(current),
                    state_name(next),
                    code,
                    now
                ],
            )
            .map_err(storage_error)?;
        if is_terminal(next) {
            transaction
                .execute(
                    "DELETE FROM output_reservations WHERE job_id = ?1",
                    [job_id.to_string()],
                )
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;

        Ok(JobRecord {
            id: job_id,
            state: next,
            input_path: PathBuf::from(row.1),
            output_path: PathBuf::from(row.2),
            plan_hash: row.3,
            sequence,
            created_unix_ms: row.5,
            updated_unix_ms: now,
        })
    }

    /// Loads one job by ID.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the row cannot be read or decoded.
    pub fn get_job(&self, job_id: Uuid) -> Result<Option<JobRecord>> {
        self.connection
            .query_row(
                "SELECT state, input_path, output_path, plan_hash, sequence,
                        created_unix_ms, updated_unix_ms
                 FROM jobs WHERE id = ?1",
                [job_id.to_string()],
                |row| {
                    let state_string: String = row.get(0)?;
                    Ok((
                        state_string,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, u64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?
            .map(|row| {
                Ok(JobRecord {
                    id: job_id,
                    state: parse_state(&row.0)?,
                    input_path: PathBuf::from(row.1),
                    output_path: PathBuf::from(row.2),
                    plan_hash: row.3,
                    sequence: row.4,
                    created_unix_ms: row.5,
                    updated_unix_ms: row.6,
                })
            })
            .transpose()
    }

    /// Lists recent jobs in reverse update order.
    ///
    /// # Errors
    ///
    /// Returns a storage error when rows cannot be read or decoded.
    pub fn list_jobs(&self, limit: usize) -> Result<Vec<JobRecord>> {
        self.list_jobs_page(limit, 0)
    }

    /// Lists one bounded page of jobs in reverse update order.
    ///
    /// # Errors
    ///
    /// Returns a storage error when rows cannot be read or decoded.
    pub fn list_jobs_page(&self, limit: usize, offset: usize) -> Result<Vec<JobRecord>> {
        let limit = limit.clamp(1, 10_000);
        let offset = i64::try_from(offset).unwrap_or(i64::MAX);
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, state, input_path, output_path, plan_hash, sequence,
                        created_unix_ms, updated_unix_ms
                 FROM jobs
                 ORDER BY updated_unix_ms DESC, id ASC
                 LIMIT ?1 OFFSET ?2",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![i64::try_from(limit).unwrap_or(10_000), offset],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, u64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;

        rows.into_iter()
            .map(|row| {
                Ok(JobRecord {
                    id: parse_job_id(&row.0)?,
                    state: parse_state(&row.1)?,
                    input_path: PathBuf::from(row.2),
                    output_path: PathBuf::from(row.3),
                    plan_hash: row.4,
                    sequence: row.5,
                    created_unix_ms: row.6,
                    updated_unix_ms: row.7,
                })
            })
            .collect()
    }

    /// Queries one bounded, reverse-update-ordered page using the same batch,
    /// state, and path predicates as stable selection capture.
    ///
    /// # Errors
    ///
    /// Returns an input or storage error when the query is invalid or cannot
    /// be read.
    pub fn query_jobs_page(
        &self,
        query: &JobSelectionQuery,
        limit: usize,
        offset: usize,
    ) -> Result<JobQueryPage> {
        let query = normalize_selection_query(query)?;
        let limit = limit.clamp(1, 500);
        let mut values = Vec::<Value>::new();
        let from_where = job_query_from_where(&query, &mut values)?;
        let total = self
            .connection
            .query_row(
                &format!("SELECT COUNT(DISTINCT jobs.id) {from_where}"),
                params_from_iter(values.iter()),
                |row| row.get::<_, u64>(0),
            )
            .map_err(storage_error)?;
        let mut page_values = values;
        page_values.push(Value::Integer(i64::try_from(limit).unwrap_or(500)));
        page_values.push(Value::Integer(i64::try_from(offset).unwrap_or(i64::MAX)));
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT jobs.id, jobs.state, jobs.input_path, jobs.output_path,
                        jobs.plan_hash, jobs.sequence, jobs.created_unix_ms,
                        jobs.updated_unix_ms
                 {from_where}
                 ORDER BY jobs.updated_unix_ms DESC, jobs.id ASC
                 LIMIT ? OFFSET ?"
            ))
            .map_err(storage_error)?;
        let jobs = statement
            .query_map(params_from_iter(page_values.iter()), stored_job_row)
            .map_err(storage_error)?
            .map(|row| job_record_from_row(row.map_err(storage_error)?))
            .collect::<Result<Vec<_>>>()?;
        Ok(JobQueryPage {
            jobs,
            total,
            limit,
            offset,
        })
    }

    /// Returns durable counts for every state currently present in `SQLite`.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the aggregate cannot be read or decoded.
    pub fn count_jobs_by_state(&self) -> Result<Vec<JobStateCount>> {
        let mut statement = self
            .connection
            .prepare("SELECT state, COUNT(*) FROM jobs GROUP BY state ORDER BY state ASC")
            .map_err(storage_error)?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
            })
            .map_err(storage_error)?
            .map(|row| {
                let (state, count) = row.map_err(storage_error)?;
                Ok(JobStateCount {
                    state: parse_state(&state)?,
                    count,
                })
            })
            .collect()
    }

    /// Lists a bounded FIFO scheduling window for one durable state.
    ///
    /// # Errors
    ///
    /// Returns a storage error when rows cannot be read or decoded.
    pub fn list_jobs_by_state(&self, state: JobState, limit: usize) -> Result<Vec<JobRecord>> {
        let limit = limit.clamp(1, 10_000);
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, state, input_path, output_path, plan_hash, sequence,
                        created_unix_ms, updated_unix_ms
                 FROM jobs
                 WHERE state = ?1
                 ORDER BY created_unix_ms ASC, id ASC
                 LIMIT ?2",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![state_name(state), i64::try_from(limit).unwrap_or(10_000)],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, u64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(JobRecord {
                    id: parse_job_id(&row.0)?,
                    state: parse_state(&row.1)?,
                    input_path: PathBuf::from(row.2),
                    output_path: PathBuf::from(row.3),
                    plan_hash: row.4,
                    sequence: row.5,
                    created_unix_ms: row.6,
                    updated_unix_ms: row.7,
                })
            })
            .collect()
    }

    /// Lists a bounded queued window with round-robin fairness across durable
    /// batches. Jobs outside a batch share one FIFO interactive lane.
    ///
    /// # Errors
    ///
    /// Returns a storage error when rows cannot be read or decoded.
    pub fn list_queued_jobs_fair(&self, limit: usize) -> Result<Vec<JobRecord>> {
        let limit = limit.clamp(1, 10_000);
        let mut statement = self
            .connection
            .prepare(
                "WITH eligible AS (
                     SELECT jobs.id, jobs.state, jobs.input_path, jobs.output_path,
                            jobs.plan_hash, jobs.sequence, jobs.created_unix_ms,
                            jobs.updated_unix_ms,
                            COALESCE(batch_members.batch_id, '__interactive__') AS lane_id,
                            COALESCE(batch_members.ordinal, jobs.created_unix_ms)
                                AS lane_ordinal,
                            COALESCE(batches.created_unix_ms,
                                     (SELECT MIN(created_unix_ms) FROM jobs
                                      WHERE state = 'queued'))
                                AS lane_created_unix_ms
                     FROM jobs
                     LEFT JOIN batch_members ON batch_members.job_id = jobs.id
                     LEFT JOIN batches ON batches.id = batch_members.batch_id
                     WHERE jobs.state = 'queued'
                 ), ranked AS (
                     SELECT *, ROW_NUMBER() OVER (
                         PARTITION BY lane_id
                         ORDER BY lane_ordinal ASC, created_unix_ms ASC, id ASC
                     ) AS lane_rank
                     FROM eligible
                 )
                 SELECT id, state, input_path, output_path, plan_hash, sequence,
                        created_unix_ms, updated_unix_ms
                 FROM ranked
                 ORDER BY lane_rank ASC, lane_created_unix_ms ASC, lane_id ASC,
                          lane_ordinal ASC, id ASC
                 LIMIT ?1",
            )
            .map_err(storage_error)?;
        statement
            .query_map([i64::try_from(limit).unwrap_or(10_000)], stored_job_row)
            .map_err(storage_error)?
            .map(|row| job_record_from_row(row.map_err(storage_error)?))
            .collect()
    }

    /// Atomically claims a queued job for one scheduler process. A competing
    /// process that already claimed the same Job yields `None` without error.
    ///
    /// # Errors
    ///
    /// Returns a storage error for a missing Job or failed transaction.
    #[allow(clippy::too_many_lines)]
    pub fn claim_queued_job(&mut self, job_id: Uuid, code: &str) -> Result<Option<JobRecord>> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let row = transaction
            .query_row(
                "SELECT state, input_path, output_path, plan_hash, sequence,
                        created_unix_ms, updated_unix_ms
                 FROM jobs WHERE id = ?1",
                [job_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, u64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?
            .ok_or_else(|| {
                FormatWrightError::new(
                    ErrorCode::StorageFailed,
                    Stage::Store,
                    format!("Job does not exist: {job_id}"),
                    "Refresh the job list.",
                )
            })?;
        if parse_state(&row.0)? != JobState::Queued {
            transaction.commit().map_err(storage_error)?;
            return Ok(None);
        }
        let now = now_unix_ms();
        let sequence = row.4.saturating_add(1);
        let updated = transaction
            .execute(
                "UPDATE jobs
                 SET state = 'inspecting', sequence = ?1, updated_unix_ms = ?2
                 WHERE id = ?3 AND state = 'queued' AND sequence = ?4",
                params![sequence, now, job_id.to_string(), row.4],
            )
            .map_err(storage_error)?;
        if updated != 1 {
            transaction.commit().map_err(storage_error)?;
            return Ok(None);
        }
        transaction
            .execute(
                "INSERT INTO job_events(
                    job_id, sequence, previous_state, next_state, code, timestamp_unix_ms
                 ) VALUES (?1, ?2, 'queued', 'inspecting', ?3, ?4)",
                params![job_id.to_string(), sequence, code, now],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(Some(JobRecord {
            id: job_id,
            state: JobState::Inspecting,
            input_path: PathBuf::from(row.1),
            output_path: PathBuf::from(row.2),
            plan_hash: row.3,
            sequence,
            created_unix_ms: row.5,
            updated_unix_ms: now,
        }))
    }

    /// Returns the durable number of jobs without hydrating job objects.
    ///
    /// # Errors
    ///
    /// Returns a storage error when `SQLite` cannot read the count.
    pub fn count_jobs(&self) -> Result<u64> {
        self.connection
            .query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))
            .map_err(storage_error)
    }

    /// Lists persistent batches without hydrating their jobs.
    ///
    /// # Errors
    ///
    /// Returns a storage error when batch aggregates cannot be read.
    pub fn list_batches_page(&self, limit: usize, offset: usize) -> Result<Vec<BatchRecord>> {
        let limit = limit.clamp(1, 10_000);
        let offset = i64::try_from(offset).unwrap_or(i64::MAX);
        let mut statement = self
            .connection
            .prepare(
                "SELECT batches.id, batches.name, COUNT(batch_members.job_id),
                        batches.created_unix_ms, batches.updated_unix_ms
                 FROM batches
                 LEFT JOIN batch_members ON batch_members.batch_id = batches.id
                 GROUP BY batches.id
                 ORDER BY batches.created_unix_ms DESC, batches.id ASC
                 LIMIT ?1 OFFSET ?2",
            )
            .map_err(storage_error)?;
        statement
            .query_map(
                params![i64::try_from(limit).unwrap_or(10_000), offset],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .map_err(storage_error)?
            .map(|row| {
                let row = row.map_err(storage_error)?;
                Ok(BatchRecord {
                    id: parse_job_id(&row.0)?,
                    name: row.1,
                    job_count: row.2,
                    created_unix_ms: row.3,
                    updated_unix_ms: row.4,
                })
            })
            .collect()
    }

    /// Lists one stable ordinal page of jobs belonging to a persistent batch.
    ///
    /// # Errors
    ///
    /// Returns a storage error when batch members cannot be read.
    pub fn list_batch_jobs_page(
        &self,
        batch_id: Uuid,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<JobRecord>> {
        let limit = limit.clamp(1, 10_000);
        let offset = i64::try_from(offset).unwrap_or(i64::MAX);
        let mut statement = self
            .connection
            .prepare(
                "SELECT jobs.id, jobs.state, jobs.input_path, jobs.output_path,
                        jobs.plan_hash, jobs.sequence, jobs.created_unix_ms,
                        jobs.updated_unix_ms
                 FROM batch_members
                 JOIN jobs ON jobs.id = batch_members.job_id
                 WHERE batch_members.batch_id = ?1
                 ORDER BY batch_members.ordinal ASC
                 LIMIT ?2 OFFSET ?3",
            )
            .map_err(storage_error)?;
        statement
            .query_map(
                params![
                    batch_id.to_string(),
                    i64::try_from(limit).unwrap_or(10_000),
                    offset
                ],
                stored_job_row,
            )
            .map_err(storage_error)?
            .map(|row| job_record_from_row(row.map_err(storage_error)?))
            .collect()
    }

    /// Captures a stable, persisted set of job IDs for later bulk actions.
    ///
    /// # Errors
    ///
    /// Returns an input or storage error without leaving a partial snapshot.
    pub fn capture_selection(&mut self, query: &JobSelectionQuery) -> Result<SelectionSnapshot> {
        let query = normalize_selection_query(query)?;
        let now = now_unix_ms();
        let id = Uuid::new_v4();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let member_ids = selection_member_ids(&transaction, &query)?;
        if member_ids.len() > 100_000 {
            return Err(FormatWrightError::new(
                ErrorCode::ResourceExhausted,
                Stage::Store,
                "Selection exceeds the 100,000-job snapshot limit",
                "Narrow the batch, state filters, or search query.",
            ));
        }
        let query_json = serde_json::to_string(&query).map_err(|error| {
            FormatWrightError::new(
                ErrorCode::Internal,
                Stage::Store,
                "Unable to serialize the stable selection query",
                "Retry capturing the selection.",
            )
            .with_diagnostic(error.to_string())
        })?;
        transaction
            .execute(
                "INSERT INTO selection_snapshots(id, query_json, member_count, created_unix_ms)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    id.to_string(),
                    query_json,
                    i64::try_from(member_ids.len()).unwrap_or(i64::MAX),
                    now
                ],
            )
            .map_err(storage_error)?;
        for (ordinal, job_id) in member_ids.iter().enumerate() {
            transaction
                .execute(
                    "INSERT INTO selection_members(selection_id, job_id, ordinal)
                     VALUES (?1, ?2, ?3)",
                    params![
                        id.to_string(),
                        job_id,
                        i64::try_from(ordinal).unwrap_or(i64::MAX)
                    ],
                )
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(SelectionSnapshot {
            id,
            query,
            member_count: u64::try_from(member_ids.len()).unwrap_or(u64::MAX),
            created_unix_ms: now,
        })
    }

    /// Lists one ordinal page from a captured selection, independent of later filters.
    ///
    /// # Errors
    ///
    /// Returns a storage error when selection members cannot be read.
    pub fn list_selection_jobs_page(
        &self,
        selection_id: Uuid,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<JobRecord>> {
        self.list_selection_jobs_page_bounded(selection_id, limit.clamp(1, 10_000), offset)
    }

    fn list_selection_jobs_page_bounded(
        &self,
        selection_id: Uuid,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<JobRecord>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT jobs.id, jobs.state, jobs.input_path, jobs.output_path,
                        jobs.plan_hash, jobs.sequence, jobs.created_unix_ms,
                        jobs.updated_unix_ms
                 FROM selection_members
                 JOIN jobs ON jobs.id = selection_members.job_id
                 WHERE selection_members.selection_id = ?1
                 ORDER BY selection_members.ordinal ASC
                 LIMIT ?2 OFFSET ?3",
            )
            .map_err(storage_error)?;
        statement
            .query_map(
                params![
                    selection_id.to_string(),
                    i64::try_from(limit).unwrap_or(i64::MAX),
                    i64::try_from(offset).unwrap_or(i64::MAX)
                ],
                stored_job_row,
            )
            .map_err(storage_error)?
            .map(|row| job_record_from_row(row.map_err(storage_error)?))
            .collect()
    }

    pub(crate) fn apply_bulk_action_persisted<F>(
        &mut self,
        selection_id: Uuid,
        action: BulkJobAction,
        before_transition: F,
    ) -> Result<BulkActionReport>
    where
        F: FnMut(&JobRecord) -> Result<()>,
    {
        apply_bulk_action_transaction(
            &mut self.connection,
            selection_id,
            action,
            before_transition,
        )
    }

    /// Re-resolves the output path and proves that it still maps to the
    /// durable reservation owned by this job.
    ///
    /// # Errors
    ///
    /// Returns an output conflict when a path alias or reparse target changed,
    /// and a storage error when the job or reservation cannot be read.
    pub fn validate_output_reservation(&self, job_id: Uuid) -> Result<()> {
        let job = self.get_job(job_id)?.ok_or_else(|| {
            FormatWrightError::new(
                ErrorCode::StorageFailed,
                Stage::Store,
                format!("Job does not exist: {job_id}"),
                "Refresh the job list.",
            )
        })?;
        let stored_key = self
            .connection
            .query_row(
                "SELECT canonical_output_path FROM output_reservations WHERE job_id = ?1",
                [job_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?
            .ok_or_else(|| {
                FormatWrightError::new(
                    ErrorCode::StorageFailed,
                    Stage::Store,
                    "Active job has no durable output reservation",
                    "Run an integrity check before retrying this job.",
                )
            })?;
        let current_key = reservation_key(&job.output_path)?;
        if current_key != stored_key {
            return Err(FormatWrightError::new(
                ErrorCode::OutputConflict,
                Stage::Store,
                "Output path identity changed after the job was queued",
                "Restore the original link target or cancel and recreate the job for the new location.",
            )
            .with_diagnostic(format!("reserved={stored_key}; current={current_key}")));
        }
        Ok(())
    }

    /// Loads a job together with its immutable Plan and ordered event history.
    ///
    /// # Errors
    ///
    /// Returns a storage or serialization error when the record cannot be
    /// loaded or decoded.
    pub fn get_job_details(&self, job_id: Uuid) -> Result<Option<JobDetails>> {
        let Some(job) = self.get_job(job_id)? else {
            return Ok(None);
        };
        let plan_json = self
            .connection
            .query_row(
                "SELECT plan_json FROM jobs WHERE id = ?1",
                [job_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .map_err(storage_error)?;
        let plan = serde_json::from_str::<Plan>(&plan_json).map_err(|error| {
            FormatWrightError::new(
                ErrorCode::StorageFailed,
                Stage::Store,
                "Stored job Plan is invalid",
                "Restore the database from backup or export unaffected jobs.",
            )
            .with_diagnostic(error.to_string())
        })?;

        let mut statement = self
            .connection
            .prepare(
                "SELECT sequence, previous_state, next_state, code, timestamp_unix_ms
                 FROM job_events WHERE job_id = ?1 ORDER BY sequence ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([job_id.to_string()], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        let events = rows
            .into_iter()
            .map(|row| {
                Ok(JobEventRecord {
                    schema_version: SCHEMA_VERSION,
                    event_id: event_id(job_id, row.0),
                    job_id,
                    sequence: row.0,
                    previous_state: row.1.as_deref().map(parse_state).transpose()?,
                    next_state: parse_state(&row.2)?,
                    code: row.3,
                    timestamp_unix_ms: row.4,
                    progress: None,
                    data: BTreeMap::new(),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Some(JobDetails { job, plan, events }))
    }

    /// Converts active jobs left by a prior process into interrupted jobs and
    /// returns the affected records for staged-output cleanup.
    ///
    /// # Errors
    ///
    /// Returns a storage error when active IDs or transitions cannot be read
    /// and persisted.
    pub fn interrupt_active_jobs(&mut self) -> Result<Vec<JobRecord>> {
        let mut statement = self
            .connection
            .prepare("SELECT id FROM jobs WHERE state IN ('running', 'validating')")
            .map_err(storage_error)?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        drop(statement);

        ids.into_iter()
            .map(|id| {
                self.transition(
                    parse_job_id(&id)?,
                    JobState::Interrupted,
                    "RECOVERED_AFTER_RESTART",
                )
            })
            .collect()
    }

    /// Converts active jobs left by a prior process into interrupted jobs.
    ///
    /// # Errors
    ///
    /// Returns a storage error when active IDs or transitions cannot be read
    /// and persisted.
    pub fn mark_active_jobs_interrupted(&mut self) -> Result<usize> {
        self.interrupt_active_jobs().map(|jobs| jobs.len())
    }
}

type StoredJobRow = (String, String, String, String, String, u64, i64, i64);

fn stored_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredJobRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn normalize_selection_query(query: &JobSelectionQuery) -> Result<JobSelectionQuery> {
    let mut states = Vec::new();
    for state in &query.states {
        if !states.contains(state) {
            states.push(*state);
        }
    }
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if search
        .as_ref()
        .is_some_and(|value| value.chars().count() > 200 || value.chars().any(char::is_control))
    {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Store,
            "Selection search must contain at most 200 visible characters",
            "Shorten the search and remove control characters.",
        ));
    }
    Ok(JobSelectionQuery {
        batch_id: query.batch_id,
        states,
        search,
    })
}

fn selection_member_ids(
    transaction: &Transaction<'_>,
    query: &JobSelectionQuery,
) -> Result<Vec<String>> {
    let mut values = Vec::<Value>::new();
    let mut sql = format!(
        "SELECT jobs.id {}",
        job_query_from_where(query, &mut values)?
    );
    if query.batch_id.is_some() {
        sql.push_str(" ORDER BY batch_members.ordinal ASC, jobs.id ASC");
    } else {
        sql.push_str(" ORDER BY jobs.created_unix_ms ASC, jobs.id ASC");
    }
    sql.push_str(" LIMIT 100001");
    let mut statement = transaction.prepare(&sql).map_err(storage_error)?;
    statement
        .query_map(params_from_iter(values.iter()), |row| {
            row.get::<_, String>(0)
        })
        .map_err(storage_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(storage_error)
}

fn job_query_from_where(query: &JobSelectionQuery, values: &mut Vec<Value>) -> Result<String> {
    let mut sql = String::from("FROM jobs");
    if let Some(batch_id) = query.batch_id {
        sql.push_str(" JOIN batch_members ON batch_members.job_id = jobs.id");
        sql.push_str(" WHERE batch_members.batch_id = ?");
        values.push(Value::Text(batch_id.to_string()));
    } else {
        sql.push_str(" WHERE 1 = 1");
    }
    if !query.states.is_empty() {
        let placeholders = std::iter::repeat_n("?", query.states.len())
            .collect::<Vec<_>>()
            .join(", ");
        write!(sql, " AND jobs.state IN ({placeholders})").map_err(|error| {
            FormatWrightError::new(
                ErrorCode::Internal,
                Stage::Store,
                "Unable to build the bounded selection query",
                "Retry capturing the selection.",
            )
            .with_diagnostic(error.to_string())
        })?;
        values.extend(
            query
                .states
                .iter()
                .map(|state| Value::Text(state_name(*state).to_owned())),
        );
    }
    if let Some(search) = &query.search {
        let escaped = search
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        sql.push_str(
            " AND (jobs.input_path LIKE ? ESCAPE '\\'
                    OR jobs.output_path LIKE ? ESCAPE '\\')",
        );
        values.push(Value::Text(pattern.clone()));
        values.push(Value::Text(pattern));
    }
    Ok(sql)
}

#[allow(clippy::too_many_lines)]
fn apply_bulk_action_transaction<F>(
    connection: &mut Connection,
    selection_id: Uuid,
    action: BulkJobAction,
    mut before_transition: F,
) -> Result<BulkActionReport>
where
    F: FnMut(&JobRecord) -> Result<()>,
{
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    let selection_exists = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM selection_snapshots WHERE id = ?1)",
            [selection_id.to_string()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_error)?;
    if !selection_exists {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Store,
            format!("Selection snapshot does not exist: {selection_id}"),
            "Capture a new stable selection before applying a bulk action.",
        ));
    }
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT jobs.id, jobs.state, jobs.input_path, jobs.output_path,
                        jobs.plan_hash, jobs.sequence, jobs.created_unix_ms,
                        jobs.updated_unix_ms
                 FROM selection_members
                 JOIN jobs ON jobs.id = selection_members.job_id
                 WHERE selection_members.selection_id = ?1
                 ORDER BY selection_members.ordinal ASC",
            )
            .map_err(storage_error)?;
        statement
            .query_map([selection_id.to_string()], stored_job_row)
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?
    };
    let action_id = Uuid::new_v4();
    let now = now_unix_ms();
    transaction
        .execute(
            "INSERT INTO bulk_actions(
                action_id, selection_id, action, matched_count, transitioned_count,
                skipped_state_count, skipped_conflict_count, created_unix_ms
             ) VALUES (?1, ?2, ?3, ?4, 0, 0, 0, ?5)",
            params![
                action_id.to_string(),
                selection_id.to_string(),
                bulk_action_name(action),
                i64::try_from(rows.len()).unwrap_or(i64::MAX),
                now
            ],
        )
        .map_err(storage_error)?;
    let mut transitioned = 0_u64;
    let mut skipped_state = 0_u64;
    let mut skipped_conflict = 0_u64;
    for row in rows.iter().cloned() {
        let job = job_record_from_row(row)?;
        let previous = job.state;
        let Some(next) = action.target_for(previous) else {
            skipped_state = skipped_state.saturating_add(1);
            insert_bulk_member(
                &transaction,
                action_id,
                job.id,
                "skipped-state",
                previous,
                None,
            )?;
            continue;
        };
        if next == JobState::Queued {
            let key = reservation_key(&job.output_path)?;
            let owner = transaction
                .query_row(
                    "SELECT job_id FROM output_reservations
                     WHERE canonical_output_path = ?1",
                    [&key],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(storage_error)?;
            if owner
                .as_deref()
                .is_some_and(|owner| owner != job.id.to_string())
            {
                skipped_conflict = skipped_conflict.saturating_add(1);
                insert_bulk_member(
                    &transaction,
                    action_id,
                    job.id,
                    "skipped-output-conflict",
                    previous,
                    None,
                )?;
                continue;
            }
        }
        before_transition(&job)?;
        if next == JobState::Queued {
            reserve_output(&transaction, job.id, &job.output_path, now)?;
        }
        let sequence = job.sequence.saturating_add(1);
        transaction
            .execute(
                "UPDATE jobs SET state = ?1, sequence = ?2, updated_unix_ms = ?3
                 WHERE id = ?4 AND state = ?5 AND sequence = ?6",
                params![
                    state_name(next),
                    sequence,
                    now,
                    job.id.to_string(),
                    state_name(previous),
                    job.sequence
                ],
            )
            .map_err(storage_error)?;
        transaction
            .execute(
                "INSERT INTO job_events(
                    job_id, sequence, previous_state, next_state, code, timestamp_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    job.id.to_string(),
                    sequence,
                    state_name(previous),
                    state_name(next),
                    bulk_action_event_code(action),
                    now
                ],
            )
            .map_err(storage_error)?;
        if is_terminal(next) {
            transaction
                .execute(
                    "DELETE FROM output_reservations WHERE job_id = ?1",
                    [job.id.to_string()],
                )
                .map_err(storage_error)?;
        }
        insert_bulk_member(
            &transaction,
            action_id,
            job.id,
            "transitioned",
            previous,
            Some(next),
        )?;
        transitioned = transitioned.saturating_add(1);
    }
    transaction
        .execute(
            "UPDATE bulk_actions
             SET transitioned_count = ?1, skipped_state_count = ?2,
                 skipped_conflict_count = ?3
             WHERE action_id = ?4",
            params![
                transitioned,
                skipped_state,
                skipped_conflict,
                action_id.to_string()
            ],
        )
        .map_err(storage_error)?;
    transaction.commit().map_err(storage_error)?;
    Ok(BulkActionReport {
        action_id,
        selection_id,
        action,
        matched: u64::try_from(rows.len()).unwrap_or(u64::MAX),
        transitioned,
        skipped_state,
        skipped_conflict,
        created_unix_ms: now,
    })
}

fn insert_bulk_member(
    transaction: &Transaction<'_>,
    action_id: Uuid,
    job_id: Uuid,
    outcome: &str,
    previous: JobState,
    next: Option<JobState>,
) -> Result<()> {
    transaction
        .execute(
            "INSERT INTO bulk_action_members(
                action_id, job_id, outcome, previous_state, next_state
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                action_id.to_string(),
                job_id.to_string(),
                outcome,
                state_name(previous),
                next.map(state_name)
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

const fn bulk_action_name(action: BulkJobAction) -> &'static str {
    match action {
        BulkJobAction::Cancel => "cancel",
        BulkJobAction::Resume => "resume",
        BulkJobAction::Retry => "retry",
    }
}

const fn bulk_action_event_code(action: BulkJobAction) -> &'static str {
    match action {
        BulkJobAction::Cancel => "BULK_CANCELLED",
        BulkJobAction::Resume => "BULK_RESUMED",
        BulkJobAction::Retry => "BULK_RETRIED",
    }
}

fn insert_jobs(
    transaction: &Transaction<'_>,
    requests: &[JobCreateRequest],
    now: i64,
) -> Result<Vec<JobRecord>> {
    let state = JobState::Planned;
    let mut records = Vec::with_capacity(requests.len());
    for request in requests {
        let id = Uuid::new_v4();
        let plan_json = serialize_plan(&request.plan)?;
        transaction
            .execute(
                "INSERT INTO jobs(
                    id, state, input_path, output_path, plan_hash, plan_json,
                    sequence, created_unix_ms, updated_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?7)",
                params![
                    id.to_string(),
                    state_name(state),
                    request.input_path.to_string_lossy(),
                    request.output_path.to_string_lossy(),
                    request.plan.plan_hash,
                    plan_json,
                    now,
                ],
            )
            .map_err(storage_error)?;
        reserve_output(transaction, id, &request.output_path, now)?;
        transaction
            .execute(
                "INSERT INTO job_events(
                    job_id, sequence, previous_state, next_state, code, timestamp_unix_ms
                 ) VALUES (?1, 0, NULL, ?2, 'JOB_CREATED', ?3)",
                params![id.to_string(), state_name(state), now],
            )
            .map_err(storage_error)?;
        records.push(JobRecord {
            id,
            state,
            input_path: request.input_path.clone(),
            output_path: request.output_path.clone(),
            plan_hash: request.plan.plan_hash.clone(),
            sequence: 0,
            created_unix_ms: now,
            updated_unix_ms: now,
        });
    }
    Ok(records)
}

fn job_record_from_row(row: StoredJobRow) -> Result<JobRecord> {
    Ok(JobRecord {
        id: parse_job_id(&row.0)?,
        state: parse_state(&row.1)?,
        input_path: PathBuf::from(row.2),
        output_path: PathBuf::from(row.3),
        plan_hash: row.4,
        sequence: row.5,
        created_unix_ms: row.6,
        updated_unix_ms: row.7,
    })
}

fn validate_batch_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Store,
            "Batch name must contain 1–120 visible characters",
            "Choose a short descriptive batch name without control characters.",
        ));
    }
    Ok(name.to_owned())
}

fn validate_idempotency_key(key: &str) -> Result<String> {
    let key = key.trim();
    if key.is_empty() || key.chars().count() > 200 || key.chars().any(char::is_control) {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Store,
            "Idempotency key must contain 1–200 visible characters",
            "Use a stable opaque key without control characters.",
        ));
    }
    Ok(key.to_owned())
}

fn serialize_plan(plan: &Plan) -> Result<String> {
    serde_json::to_string(plan).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::Internal,
            Stage::Store,
            "Unable to serialize job Plan",
            "Create the Plan again.",
        )
        .with_diagnostic(error.to_string())
    })
}

fn reserve_output(
    transaction: &Transaction<'_>,
    job_id: Uuid,
    output_path: &Path,
    now: i64,
) -> Result<()> {
    let key = reservation_key(output_path)?;
    let job_id_text = job_id.to_string();
    let owned_key = transaction
        .query_row(
            "SELECT canonical_output_path FROM output_reservations WHERE job_id = ?1",
            [&job_id_text],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_error)?;
    if owned_key.as_deref() == Some(key.as_str()) {
        return Ok(());
    }
    if owned_key.is_some() {
        transaction
            .execute(
                "DELETE FROM output_reservations WHERE job_id = ?1",
                [&job_id_text],
            )
            .map_err(storage_error)?;
    }
    transaction
        .execute(
            "INSERT INTO output_reservations(canonical_output_path, job_id, created_unix_ms)
             VALUES (?1, ?2, ?3)",
            params![key, job_id_text, now],
        )
        .map_err(output_reservation_error)?;
    Ok(())
}

fn reservation_key(path: &Path) -> Result<String> {
    let identity = resolve_output_identity(path, Stage::Store)?;
    #[cfg(windows)]
    {
        let rendered = identity.to_str().ok_or_else(|| {
            invalid_windows_output_path("Output path is not valid Unicode", &identity)
        })?;
        Ok(rendered.to_lowercase())
    }
    #[cfg(not(windows))]
    {
        Ok(identity.to_string_lossy().into_owned())
    }
}

pub(crate) fn resolve_output_identity(path: &Path, stage: Stage) -> Result<std::path::PathBuf> {
    #[cfg(windows)]
    let result = windows_output_identity(path);
    #[cfg(not(windows))]
    let result = posix_output_identity(path);
    result.map_err(|mut error| {
        error.stage = stage;
        error
    })
}

#[cfg(not(windows))]
fn posix_output_identity(path: &Path) -> Result<std::path::PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                FormatWrightError::new(
                    ErrorCode::StorageFailed,
                    Stage::Store,
                    "Unable to resolve the current directory for output reservation",
                    "Choose an absolute output path.",
                )
                .with_diagnostic(error.to_string())
            })?
            .join(path)
    };
    let file_name = absolute.file_name().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Store,
            "Output reservation path has no filename",
            "Choose a complete output path.",
        )
    })?;
    let parent = absolute.parent().unwrap_or_else(|| Path::new("."));
    let canonical_parent = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf());
    Ok(canonical_parent.join(file_name))
}

#[cfg(windows)]
fn windows_output_identity(path: &Path) -> Result<std::path::PathBuf> {
    validate_windows_source_components(path)?;
    let absolute = std::path::absolute(path).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Store,
            "Unable to resolve the absolute Windows output path",
            "Choose an absolute local output path.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let lexical = windows_lexical_disk_path(&absolute, true)?;
    let mut existing_ancestor = lexical.clone();
    let mut missing_suffix = Vec::new();
    loop {
        match std::fs::symlink_metadata(&existing_ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let component = existing_ancestor.file_name().ok_or_else(|| {
                    invalid_windows_output_path(
                        "Output path has no existing local volume ancestor",
                        &lexical,
                    )
                })?;
                missing_suffix.push(component.to_os_string());
                if !existing_ancestor.pop() {
                    return Err(invalid_windows_output_path(
                        "Output path has no existing local volume ancestor",
                        &lexical,
                    ));
                }
            }
            Err(error) => {
                return Err(FormatWrightError::new(
                    ErrorCode::StorageFailed,
                    Stage::Store,
                    "Unable to inspect the Windows output path",
                    "Choose a local output path whose parent directory is accessible.",
                )
                .with_diagnostic(format!("{}: {error}", existing_ancestor.display())));
            }
        }
    }

    let canonical_ancestor = existing_ancestor.canonicalize().map_err(|error| {
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Store,
            "Unable to resolve the final Windows output location",
            "Remove dangling links or choose an accessible local output directory.",
        )
        .with_diagnostic(format!("{}: {error}", existing_ancestor.display()))
    })?;
    let mut resolved = windows_lexical_disk_path(&canonical_ancestor, false)?;
    for component in missing_suffix.into_iter().rev() {
        resolved.push(component);
    }
    if resolved.file_name().is_none() {
        return Err(invalid_windows_output_path(
            "Output reservation path has no filename",
            &resolved,
        ));
    }
    Ok(resolved)
}

#[cfg(windows)]
fn validate_windows_source_components(path: &Path) -> Result<()> {
    use std::path::Component;

    for component in path.components() {
        if let Component::Normal(name) = component {
            let value = name.to_str().ok_or_else(|| {
                invalid_windows_output_path("Output path is not valid Unicode", path)
            })?;
            validate_windows_component(value, path)?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn windows_lexical_disk_path(path: &Path, require_leaf: bool) -> Result<std::path::PathBuf> {
    use std::path::{Component, Prefix};

    let mut components = path.components();
    let (drive, verbatim) = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) => (drive, false),
            Prefix::VerbatimDisk(drive) => (drive, true),
            Prefix::UNC(..) | Prefix::VerbatimUNC(..) => {
                return Err(invalid_windows_output_path(
                    "Network output paths are not allowed",
                    path,
                ));
            }
            Prefix::DeviceNS(_) | Prefix::Verbatim(_) => {
                return Err(invalid_windows_output_path(
                    "Windows device namespace output paths are not allowed",
                    path,
                ));
            }
        },
        _ => {
            return Err(invalid_windows_output_path(
                "Output path is not rooted on a local Windows drive",
                path,
            ));
        }
    };
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(invalid_windows_output_path(
            "Output path is not rooted on a local Windows drive",
            path,
        ));
    }

    let mut names = Vec::new();
    for component in components {
        match component {
            Component::CurDir if !verbatim => {}
            Component::ParentDir if !verbatim => {
                if names.pop().is_none() {
                    return Err(invalid_windows_output_path(
                        "Output path escapes its Windows volume root",
                        path,
                    ));
                }
            }
            Component::Normal(name) => {
                let value = name.to_str().ok_or_else(|| {
                    invalid_windows_output_path("Output path is not valid Unicode", path)
                })?;
                if verbatim && matches!(value, "." | "..") {
                    return Err(invalid_windows_output_path(
                        "Verbatim Windows output paths cannot contain dot components",
                        path,
                    ));
                }
                validate_windows_component(value, path)?;
                names.push(name.to_os_string());
            }
            Component::CurDir | Component::ParentDir => {
                return Err(invalid_windows_output_path(
                    "Verbatim Windows output paths cannot contain dot components",
                    path,
                ));
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(invalid_windows_output_path(
                    "Output path contains an unexpected Windows root component",
                    path,
                ));
            }
        }
    }
    if require_leaf && names.is_empty() {
        return Err(invalid_windows_output_path(
            "Output reservation path has no filename",
            path,
        ));
    }

    let mut normalized =
        std::path::PathBuf::from(format!("{}:\\", char::from(drive).to_ascii_uppercase()));
    normalized.extend(names);
    Ok(normalized)
}

#[cfg(windows)]
fn validate_windows_component(component: &str, path: &Path) -> Result<()> {
    if component.starts_with(' ') || component.ends_with(' ') || component.ends_with('.') {
        return Err(invalid_windows_output_path(
            "Windows output components cannot start with a space or end with a space or period",
            path,
        ));
    }
    if component.chars().any(|character| {
        character == '\0'
            || character <= '\u{1f}'
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
    }) {
        return Err(invalid_windows_output_path(
            "Windows output path contains a reserved character or alternate data stream",
            path,
        ));
    }
    let stem = component.split('.').next().unwrap_or_default();
    if is_reserved_windows_device_name(stem) {
        return Err(invalid_windows_output_path(
            "Windows output path contains a reserved device name",
            path,
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn is_reserved_windows_device_name(stem: &str) -> bool {
    let upper = stem.to_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let Some(suffix) = upper
        .strip_prefix("COM")
        .or_else(|| upper.strip_prefix("LPT"))
    else {
        return false;
    };
    matches!(
        suffix,
        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
    )
}

#[cfg(windows)]
fn invalid_windows_output_path(message: &str, path: &Path) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Store,
        message,
        "Choose a normal local Windows path without device names, aliases, or trailing dots/spaces.",
    )
    .with_diagnostic(path.display().to_string())
}

const fn is_terminal(state: JobState) -> bool {
    matches!(
        state,
        JobState::Completed | JobState::Warning | JobState::Failed | JobState::Cancelled
    )
}

fn output_reservation_error(error: rusqlite::Error) -> FormatWrightError {
    if matches!(
        error,
        rusqlite::Error::SqliteFailure(ref sqlite, _)
            if sqlite.code == SqliteErrorCode::ConstraintViolation
    ) {
        return FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Store,
            "Another active job already reserves this output path",
            "Choose another output path or wait for the owning job to finish.",
        )
        .with_diagnostic(error.to_string());
    }
    storage_error(error)
}

fn parse_job_id(value: &str) -> Result<Uuid> {
    Uuid::parse_str(value).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Store,
            "Stored job ID is invalid",
            "Restore the database from backup or export unaffected jobs.",
        )
        .with_diagnostic(error.to_string())
    })
}

fn event_id(job_id: Uuid, sequence: u64) -> Uuid {
    Uuid::new_v5(&job_id, &sequence.to_be_bytes())
}

fn state_name(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Inspecting => "inspecting",
        JobState::Planned => "planned",
        JobState::Blocked => "blocked",
        JobState::Running => "running",
        JobState::Validating => "validating",
        JobState::Completed => "completed",
        JobState::Warning => "warning",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
        JobState::Interrupted => "interrupted",
    }
}

fn parse_state(value: &str) -> Result<JobState> {
    match value {
        "queued" => Ok(JobState::Queued),
        "inspecting" => Ok(JobState::Inspecting),
        "planned" => Ok(JobState::Planned),
        "blocked" => Ok(JobState::Blocked),
        "running" => Ok(JobState::Running),
        "validating" => Ok(JobState::Validating),
        "completed" => Ok(JobState::Completed),
        "warning" => Ok(JobState::Warning),
        "failed" => Ok(JobState::Failed),
        "cancelled" => Ok(JobState::Cancelled),
        "interrupted" => Ok(JobState::Interrupted),
        _ => Err(FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Store,
            format!("Unknown stored job state: {value}"),
            "Restore the database from backup or migrate it with a compatible release.",
        )),
    }
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

#[allow(clippy::needless_pass_by_value)]
fn storage_error(error: rusqlite::Error) -> FormatWrightError {
    if matches!(
        error,
        rusqlite::Error::SqliteFailure(ref sqlite, _)
            if matches!(
                sqlite.code,
                SqliteErrorCode::DatabaseBusy | SqliteErrorCode::DatabaseLocked
            )
    ) {
        return FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Store,
            "The state database is busy with another writer",
            "Retry after the other FormatWright operation finishes.",
        )
        .retryable(true)
        .with_diagnostic(error.to_string());
    }
    FormatWrightError::new(
        ErrorCode::StorageFailed,
        Stage::Store,
        "SQLite operation failed",
        "Check local storage health and retry.",
    )
    .with_diagnostic(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant};

    use tempfile::tempdir;
    use uuid::Uuid;

    #[cfg(windows)]
    use super::reservation_key;
    use super::{BulkJobAction, JobCreateRequest, JobSelectionQuery, SqliteJobStore};
    use crate::domain::{ChangeSet, JobState, NetworkPolicy, Plan, SCHEMA_VERSION};
    use crate::{BulkJobService, ErrorCode};

    fn plan() -> Plan {
        Plan {
            schema_version: SCHEMA_VERSION,
            plan_id: Uuid::new_v4(),
            plan_hash: "blake3:test".to_owned(),
            input_fingerprint: "fwfp-v1:test".to_owned(),
            target_format: "mp4".to_owned(),
            constraints: BTreeMap::new(),
            steps: Vec::new(),
            changes: ChangeSet::default(),
            validators: Vec::new(),
            network_policy: NetworkPolicy::Deny,
            output_path: Some(PathBuf::from("output.mp4")),
            estimated_output_bytes: None,
        }
    }

    fn durable_plan() -> Plan {
        let mut plan = plan();
        plan.plan_hash = crate::planner::deterministic_plan_hash(&plan).expect("hash durable Plan");
        plan
    }

    #[test]
    fn transitions_are_transactional_and_ordered() {
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let job = store
            .create_job("input.mkv", "output.mp4", &plan())
            .expect("create job");
        assert_eq!(job.state, JobState::Planned);

        let running = store
            .transition(job.id, JobState::Running, "ENGINE_STARTED")
            .expect("start job");
        assert_eq!(running.sequence, 1);
        let validating = store
            .transition(job.id, JobState::Validating, "ENGINE_FINISHED")
            .expect("validate job");
        assert_eq!(validating.sequence, 2);
        let completed = store
            .transition(job.id, JobState::Completed, "VALIDATION_PASSED")
            .expect("complete job");
        assert_eq!(completed.sequence, 3);
    }

    #[test]
    fn concurrent_connections_cannot_reserve_the_same_output() {
        let directory = tempdir().expect("temporary directory");
        let database = directory.path().join("jobs.sqlite3");
        drop(SqliteJobStore::open(&database).expect("initialize database"));
        let output = directory.path().join("shared-output.mp4");
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for index in 0..2 {
            let database = database.clone();
            let output = output.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                let mut store = SqliteJobStore::open(&database).expect("concurrent store");
                barrier.wait();
                store.create_job(format!("input-{index}.mkv"), output, &durable_plan())
            }));
        }
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().expect("reservation thread"))
            .collect::<Vec<_>>();

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let errors = results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, ErrorCode::OutputConflict);
        let store = SqliteJobStore::open(&database).expect("inspect database");
        assert_eq!(store.count_jobs().expect("job count"), 1);
        drop(store);
        assert!(
            crate::MaintenanceService::new(&database)
                .integrity_check()
                .expect("integrity report")
                .ok
        );
    }

    #[test]
    fn concurrent_transitions_commit_only_one_event_sequence() {
        let directory = tempdir().expect("temporary directory");
        let database = directory.path().join("jobs.sqlite3");
        let mut store = SqliteJobStore::open(&database).expect("initialize database");
        let job = store
            .create_job(
                "input.mkv",
                directory.path().join("output.mp4"),
                &durable_plan(),
            )
            .expect("create job");
        let job_id = job.id;
        drop(store);
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let database = database.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                let mut store = SqliteJobStore::open(&database).expect("concurrent store");
                barrier.wait();
                store.transition(job_id, JobState::Running, "CONCURRENT_START")
            }));
        }
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().expect("transition thread"))
            .collect::<Vec<_>>();

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        let store = SqliteJobStore::open(&database).expect("inspect database");
        let details = store
            .get_job_details(job_id)
            .expect("job details")
            .expect("stored job");
        assert_eq!(details.job.state, JobState::Running);
        assert_eq!(details.job.sequence, 1);
        assert_eq!(details.events.len(), 2);
        assert_eq!(details.events[1].code, "CONCURRENT_START");
    }

    #[test]
    fn cancellation_is_valid_at_the_validation_boundary() {
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let job = store
            .create_job("input.mkv", "output.mp4", &plan())
            .expect("create job");
        store
            .transition(job.id, JobState::Running, "ENGINE_STARTED")
            .expect("start job");
        store
            .transition(job.id, JobState::Validating, "ENGINE_FINISHED")
            .expect("enter validation");
        let cancelled = store
            .transition(job.id, JobState::Cancelled, "USER_CANCELLED")
            .expect("cancel during validation");
        assert_eq!(cancelled.state, JobState::Cancelled);
        assert_eq!(cancelled.sequence, 3);
    }

    #[test]
    fn invalid_transition_is_rejected() {
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let job = store
            .create_job("input.mkv", "output.mp4", &plan())
            .expect("create job");
        let error = store
            .transition(job.id, JobState::Completed, "INVALID")
            .expect_err("invalid transition must fail");
        assert!(error.message.contains("Invalid job transition"));
    }

    #[test]
    fn batch_queue_transition_is_atomic() {
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let requests = [
            JobCreateRequest {
                input_path: PathBuf::from("input-a.mkv"),
                output_path: PathBuf::from("output-a.mp4"),
                plan: plan(),
            },
            JobCreateRequest {
                input_path: PathBuf::from("input-b.mkv"),
                output_path: PathBuf::from("output-b.mp4"),
                plan: plan(),
            },
        ];
        let jobs = store.create_jobs(&requests).expect("create batch");
        store
            .queue_jobs(&[jobs[0].id, Uuid::new_v4()], "BATCH_QUEUED")
            .expect_err("missing ID must roll back complete queue operation");
        assert!(jobs.iter().all(|job| {
            store
                .get_job(job.id)
                .expect("load job")
                .is_some_and(|loaded| loaded.state == JobState::Planned)
        }));

        let queued = store
            .queue_jobs(
                &jobs.iter().map(|job| job.id).collect::<Vec<_>>(),
                "BATCH_QUEUED",
            )
            .expect("queue complete batch");
        assert_eq!(queued.len(), 2);
        assert!(queued.iter().all(|job| job.state == JobState::Queued));
    }

    #[test]
    fn persistent_batch_creation_is_atomic_and_preserves_ordinal_membership() {
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let base_plan = durable_plan();
        let requests = (0..3)
            .map(|index| JobCreateRequest {
                input_path: PathBuf::from(format!("batch-input-{index}.mkv")),
                output_path: PathBuf::from(format!("batch-output-{index}.mp4")),
                plan: base_plan.clone(),
            })
            .collect::<Vec<_>>();

        let batch = store
            .create_batch("First durable batch", &requests)
            .expect("create persistent batch");
        assert_eq!(batch.job_count, 3);
        assert_eq!(
            store.list_batches_page(10, 0).expect("list batches"),
            vec![batch.clone()]
        );
        let members = store
            .list_batch_jobs_page(batch.id, 10, 0)
            .expect("list batch members");
        assert_eq!(
            members
                .iter()
                .map(|job| job.input_path.clone())
                .collect::<Vec<_>>(),
            requests
                .iter()
                .map(|request| request.input_path.clone())
                .collect::<Vec<_>>()
        );

        let conflicting = [
            JobCreateRequest {
                input_path: PathBuf::from("rollback-a.mkv"),
                output_path: PathBuf::from("rollback-shared.mp4"),
                plan: base_plan.clone(),
            },
            JobCreateRequest {
                input_path: PathBuf::from("rollback-b.mkv"),
                output_path: PathBuf::from("rollback-shared.mp4"),
                plan: base_plan,
            },
        ];
        let error = store
            .create_batch("Must roll back", &conflicting)
            .expect_err("conflicting batch must fail atomically");
        assert_eq!(error.code, ErrorCode::OutputConflict);
        assert_eq!(store.count_jobs().expect("count jobs"), 3);
        assert_eq!(
            store.list_batches_page(10, 0).expect("list batches").len(),
            1
        );
    }

    #[test]
    fn queued_batch_creation_commits_members_reservations_and_queue_events_atomically() {
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let requests = (0..3)
            .map(|index| JobCreateRequest {
                input_path: PathBuf::from(format!("queued-input-{index}.mkv")),
                output_path: PathBuf::from(format!("queued-output-{index}.mp4")),
                plan: durable_plan(),
            })
            .collect::<Vec<_>>();

        let batch = store
            .create_queued_batch("queued atomically", &requests)
            .expect("create queued batch");
        let jobs = store
            .list_batch_jobs_page(batch.id, 10, 0)
            .expect("batch jobs");

        assert_eq!(jobs.len(), 3);
        assert!(jobs.iter().all(|job| job.state == JobState::Queued));
        assert!(jobs.iter().all(|job| job.sequence == 1));
        for job in jobs {
            let details = store
                .get_job_details(job.id)
                .expect("job details")
                .expect("job exists");
            assert_eq!(details.events.len(), 2);
            assert_eq!(details.events[1].code, "BATCH_QUEUED");
            assert_eq!(details.events[1].previous_state, Some(JobState::Planned));
            assert_eq!(details.events[1].next_state, JobState::Queued);
        }
    }

    #[test]
    fn idempotency_key_reuses_identical_intent_and_rejects_drift() {
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let request = JobCreateRequest {
            input_path: PathBuf::from("idempotent-input.mkv"),
            output_path: PathBuf::from("idempotent-output.mp4"),
            plan: durable_plan(),
        };

        let first = store
            .create_job_idempotent("desktop:drop:0001", &request)
            .expect("create idempotent job");
        let replay = store
            .create_job_idempotent("desktop:drop:0001", &request)
            .expect("replay identical request");
        assert!(first.created);
        assert!(!replay.created);
        assert_eq!(first.job.id, replay.job.id);
        assert_eq!(store.count_jobs().expect("count jobs"), 1);

        let mut drifted = request;
        drifted.output_path = PathBuf::from("different-output.mp4");
        let error = store
            .create_job_idempotent("desktop:drop:0001", &drifted)
            .expect_err("key drift must be rejected");
        assert_eq!(error.code, ErrorCode::PolicyBlocked);
        assert_eq!(store.count_jobs().expect("count after rejected drift"), 1);
    }

    #[test]
    fn idempotent_enqueue_is_atomic_and_concurrent_replays_share_one_job() {
        let directory = tempdir().expect("temporary directory");
        let database = directory.path().join("jobs.sqlite3");
        drop(SqliteJobStore::open(&database).expect("initialize database"));
        let output = directory.path().join("idempotent-output.mp4");
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let database = database.clone();
            let output = output.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                let mut store = SqliteJobStore::open(&database).expect("concurrent store");
                let request = JobCreateRequest {
                    input_path: PathBuf::from("same-input.mkv"),
                    output_path: output,
                    plan: durable_plan(),
                };
                barrier.wait();
                store.enqueue_job_idempotent("api:request:0001", &request)
            }));
        }
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().expect("idempotency thread").expect("enqueue"))
            .collect::<Vec<_>>();

        assert_eq!(results.iter().filter(|result| result.created).count(), 1);
        assert_eq!(results[0].job.id, results[1].job.id);
        assert!(
            results
                .iter()
                .all(|result| { result.job.state == JobState::Queued && result.job.sequence == 1 })
        );
        let store = SqliteJobStore::open(&database).expect("inspect database");
        assert_eq!(store.count_jobs().expect("job count"), 1);
        let details = store
            .get_job_details(results[0].job.id)
            .expect("details")
            .expect("job");
        assert_eq!(details.events.len(), 2);
        assert_eq!(details.events[1].code, "JOB_ENQUEUED");
    }

    #[test]
    fn fair_queued_window_round_robins_batch_lanes() {
        let mut store = SqliteJobStore::open_in_memory().expect("store");
        let batch_plan = durable_plan();
        let make_requests = |prefix: &str, count: usize| {
            (0..count)
                .map(|index| JobCreateRequest {
                    input_path: PathBuf::from(format!("{prefix}-{index}.json")),
                    output_path: PathBuf::from(format!("{prefix}-{index}.yaml")),
                    plan: batch_plan.clone(),
                })
                .collect::<Vec<_>>()
        };
        let first = store
            .create_batch("first", &make_requests("first", 5))
            .expect("first batch");
        let first_jobs = store
            .list_batch_jobs_page(first.id, 10, 0)
            .expect("first jobs");
        store
            .queue_jobs(
                &first_jobs.iter().map(|job| job.id).collect::<Vec<_>>(),
                "TEST_QUEUED",
            )
            .expect("queue first");
        let second = store
            .create_batch("second", &make_requests("second", 2))
            .expect("second batch");
        let second_jobs = store
            .list_batch_jobs_page(second.id, 10, 0)
            .expect("second jobs");
        store
            .queue_jobs(
                &second_jobs.iter().map(|job| job.id).collect::<Vec<_>>(),
                "TEST_QUEUED",
            )
            .expect("queue second");

        let selected = store.list_queued_jobs_fair(4).expect("fair window");

        assert_eq!(
            selected.iter().map(|job| job.id).collect::<Vec<_>>(),
            vec![
                first_jobs[0].id,
                second_jobs[0].id,
                first_jobs[1].id,
                second_jobs[1].id,
            ]
        );
    }

    #[test]
    fn concurrent_connections_claim_one_queued_job_exactly_once() {
        let directory = tempdir().expect("claim suite");
        let database = directory.path().join("jobs.sqlite3");
        let mut seed = SqliteJobStore::open(&database).expect("seed");
        let job = seed
            .create_job("input.json", "output.yaml", &durable_plan())
            .and_then(|job| seed.transition(job.id, JobState::Queued, "TEST_QUEUED"))
            .expect("queued job");
        drop(seed);
        let barrier = Arc::new(Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let database = database.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut store = SqliteJobStore::open(database).expect("claim store");
                    barrier.wait();
                    store
                        .claim_queued_job(job.id, "TEST_CLAIMED")
                        .expect("claim")
                        .is_some()
                })
            })
            .collect::<Vec<_>>();
        let claims = handles
            .into_iter()
            .map(|handle| handle.join().expect("claim thread"))
            .filter(|claimed| *claimed)
            .count();

        assert_eq!(claims, 1);
        let details = SqliteJobStore::open(&database)
            .expect("verify store")
            .get_job_details(job.id)
            .expect("details")
            .expect("job");
        assert_eq!(details.job.state, JobState::Inspecting);
        assert_eq!(details.events.last().expect("event").code, "TEST_CLAIMED");
    }

    #[test]
    fn selection_snapshot_membership_and_order_remain_stable() {
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let base_plan = durable_plan();
        let requests = (0..3)
            .map(|index| JobCreateRequest {
                input_path: PathBuf::from(format!("stable-input-{index}.mkv")),
                output_path: PathBuf::from(format!("stable-output-{index}.mp4")),
                plan: base_plan.clone(),
            })
            .collect::<Vec<_>>();
        let batch = store
            .create_batch("Stable selection", &requests)
            .expect("create batch");
        let jobs = store
            .list_batch_jobs_page(batch.id, 10, 0)
            .expect("list jobs");
        for job in &jobs[..2] {
            store
                .transition(job.id, JobState::Running, "TEST_RUNNING")
                .expect("start job");
            store
                .transition(job.id, JobState::Failed, "TEST_FAILED")
                .expect("fail job");
        }
        let selection = store
            .capture_selection(&JobSelectionQuery {
                batch_id: Some(batch.id),
                states: vec![JobState::Failed],
                search: Some("stable-input".to_owned()),
            })
            .expect("capture failed jobs");
        assert_eq!(selection.member_count, 2);
        store
            .transition(jobs[0].id, JobState::Queued, "TEST_RETRY")
            .expect("mutate state after capture");

        let members = store
            .list_selection_jobs_page(selection.id, 10, 0)
            .expect("list frozen membership");
        assert_eq!(
            members.iter().map(|job| job.id).collect::<Vec<_>>(),
            vec![jobs[0].id, jobs[1].id]
        );
        assert_eq!(members[0].state, JobState::Queued);
        assert_eq!(members[1].state, JobState::Failed);
    }

    #[test]
    fn query_page_filters_by_batch_state_and_escaped_path_search() {
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let requests = ["alpha%one", "alpha_two", "beta"]
            .into_iter()
            .map(|name| JobCreateRequest {
                input_path: PathBuf::from(format!("source/{name}.json")),
                output_path: PathBuf::from(format!("target/{name}.yaml")),
                plan: durable_plan(),
            })
            .collect::<Vec<_>>();
        let batch = store
            .create_batch("query batch", &requests)
            .expect("create batch");
        let jobs = store
            .list_batch_jobs_page(batch.id, 10, 0)
            .expect("batch jobs");
        store
            .transition(jobs[0].id, JobState::Queued, "TEST_QUEUED")
            .expect("queue first");
        store
            .transition(jobs[1].id, JobState::Cancelled, "TEST_CANCELLED")
            .expect("cancel second");

        let page = store
            .query_jobs_page(
                &JobSelectionQuery {
                    batch_id: Some(batch.id),
                    states: vec![JobState::Queued],
                    search: Some("%one".to_owned()),
                },
                25,
                0,
            )
            .expect("filtered page");

        assert_eq!(page.total, 1);
        assert_eq!(page.jobs.len(), 1);
        assert_eq!(page.jobs[0].id, jobs[0].id);
        assert_eq!(page.limit, 25);
        assert_eq!(page.offset, 0);
        let counts = store.count_jobs_by_state().expect("state counts");
        assert_eq!(
            counts
                .iter()
                .find(|entry| entry.state == JobState::Queued)
                .map(|entry| entry.count),
            Some(1)
        );
        assert_eq!(
            counts
                .iter()
                .find(|entry| entry.state == JobState::Cancelled)
                .map(|entry| entry.count),
            Some(1)
        );
    }

    #[test]
    fn bulk_retry_transitions_eligible_jobs_and_audits_skips() {
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let base_plan = durable_plan();
        let requests = (0..4)
            .map(|index| JobCreateRequest {
                input_path: PathBuf::from(format!("bulk-input-{index}.mkv")),
                output_path: PathBuf::from(format!("bulk-output-{index}.mp4")),
                plan: base_plan.clone(),
            })
            .collect::<Vec<_>>();
        let jobs = store.create_jobs(&requests).expect("create jobs");
        store
            .transition(jobs[1].id, JobState::Running, "TEST_RUNNING")
            .expect("start interrupted job");
        store
            .transition(jobs[1].id, JobState::Interrupted, "TEST_INTERRUPTED")
            .expect("interrupt job");
        store
            .transition(jobs[2].id, JobState::Running, "TEST_RUNNING")
            .expect("start failed job");
        store
            .transition(jobs[2].id, JobState::Failed, "TEST_FAILED")
            .expect("fail job");
        store
            .transition(jobs[3].id, JobState::Running, "TEST_RUNNING")
            .expect("start completed job");
        store
            .transition(jobs[3].id, JobState::Validating, "TEST_VALIDATING")
            .expect("validate job");
        store
            .transition(jobs[3].id, JobState::Completed, "TEST_COMPLETED")
            .expect("complete job");
        let selection = store
            .capture_selection(&JobSelectionQuery::default())
            .expect("capture all jobs");

        let report = BulkJobService::apply(&mut store, selection.id, BulkJobAction::Retry)
            .expect("bulk retry");
        assert_eq!(report.matched, 4);
        assert_eq!(report.transitioned, 2);
        assert_eq!(report.skipped_state, 2);
        assert_eq!(report.skipped_conflict, 0);
        assert_eq!(
            store.get_job(jobs[1].id).expect("load").expect("job").state,
            JobState::Queued
        );
        assert_eq!(
            store.get_job(jobs[2].id).expect("load").expect("job").state,
            JobState::Queued
        );
        assert_eq!(
            store
                .get_job_details(jobs[2].id)
                .expect("details")
                .expect("job")
                .events
                .last()
                .expect("event")
                .code,
            "BULK_RETRIED"
        );
    }

    #[test]
    fn bulk_retry_skips_an_output_owned_by_another_active_job() {
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let output = PathBuf::from("bulk-conflict.mp4");
        let failed = store
            .create_job("failed-input.mkv", &output, &durable_plan())
            .expect("create failed candidate");
        store
            .transition(failed.id, JobState::Running, "TEST_RUNNING")
            .expect("start job");
        store
            .transition(failed.id, JobState::Failed, "TEST_FAILED")
            .expect("fail job");
        store
            .create_job("owner-input.mkv", &output, &durable_plan())
            .expect("reserve output with another job");
        let selection = store
            .capture_selection(&JobSelectionQuery {
                states: vec![JobState::Failed],
                ..JobSelectionQuery::default()
            })
            .expect("capture failed job");

        let report = BulkJobService::apply(&mut store, selection.id, BulkJobAction::Retry)
            .expect("bulk retry with conflict");
        assert_eq!(report.matched, 1);
        assert_eq!(report.transitioned, 0);
        assert_eq!(report.skipped_conflict, 1);
        assert_eq!(
            store.get_job(failed.id).expect("load").expect("job").state,
            JobState::Failed
        );
    }

    #[test]
    fn active_jobs_become_interrupted_on_recovery() {
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let job = store
            .create_job("input.mkv", "output.mp4", &plan())
            .expect("create job");
        store
            .transition(job.id, JobState::Running, "ENGINE_STARTED")
            .expect("start job");

        let recovered = store
            .mark_active_jobs_interrupted()
            .expect("recover active jobs");
        assert_eq!(recovered, 1);
        let loaded = store
            .get_job(job.id)
            .expect("load job")
            .expect("job exists");
        assert_eq!(loaded.state, JobState::Interrupted);
    }

    #[test]
    fn list_and_details_preserve_plan_and_event_order() {
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let expected_plan = plan();
        let job = store
            .create_job("input.mkv", "output.mp4", &expected_plan)
            .expect("create job");
        store
            .transition(job.id, JobState::Running, "ENGINE_STARTED")
            .expect("start job");

        let jobs = store.list_jobs(100).expect("list jobs");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, job.id);
        let details = store
            .get_job_details(job.id)
            .expect("load details")
            .expect("job exists");
        assert_eq!(details.plan, expected_plan);
        assert_eq!(details.events.len(), 2);
        assert_eq!(details.events[0].sequence, 0);
        assert_eq!(details.events[0].code, "JOB_CREATED");
        assert_eq!(details.events[1].sequence, 1);
        assert_eq!(details.events[1].code, "ENGINE_STARTED");
    }

    #[test]
    fn active_output_reservation_is_unique_and_terminal_state_releases_it() {
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let first = store
            .create_job("input-a.mkv", "same-output.mp4", &plan())
            .expect("first reservation");
        let error = store
            .create_job("input-b.mkv", "same-output.mp4", &plan())
            .expect_err("second active reservation must fail");
        assert_eq!(error.code, ErrorCode::OutputConflict);

        store
            .transition(first.id, JobState::Cancelled, "USER_CANCELLED")
            .expect("terminal state releases reservation");
        store
            .create_job("input-b.mkv", "same-output.mp4", &plan())
            .expect("released output can be reserved again");
    }

    #[test]
    fn retry_reacquires_output_reservation_atomically() {
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let first = store
            .create_job("input-a.mkv", "retry-output.mp4", &plan())
            .expect("first job");
        store
            .transition(first.id, JobState::Cancelled, "USER_CANCELLED")
            .expect("cancel first job");
        let second = store
            .create_job("input-b.mkv", "retry-output.mp4", &plan())
            .expect("second job owns reservation");

        let error = store
            .transition(first.id, JobState::Queued, "JOB_RETRIED")
            .expect_err("retry must not steal another reservation");
        assert_eq!(error.code, ErrorCode::OutputConflict);
        assert_eq!(
            store
                .get_job(first.id)
                .expect("load first")
                .expect("first exists")
                .state,
            JobState::Cancelled
        );

        store
            .transition(second.id, JobState::Cancelled, "USER_CANCELLED")
            .expect("release second reservation");
        let retried = store
            .transition(first.id, JobState::Queued, "JOB_RETRIED")
            .expect("retry after reservation release");
        assert_eq!(retried.state, JobState::Queued);
    }

    #[cfg(windows)]
    #[test]
    fn windows_reservation_identity_collapses_case_verbatim_and_lexical_aliases() {
        let directory = tempdir().expect("temporary directory");
        let ordinary = directory.path().join("Future").join("Output.MP4");
        let case_alias = directory.path().join("future").join("output.mp4");
        let verbatim_alias = PathBuf::from(format!(r"\\?\{}", ordinary.display()));
        let dot_alias = directory
            .path()
            .join("Future")
            .join("child")
            .join("..")
            .join("Output.MP4");

        let expected = reservation_key(&ordinary).expect("ordinary key");
        assert_eq!(reservation_key(&case_alias).expect("case key"), expected);
        assert_eq!(
            reservation_key(&verbatim_alias).expect("verbatim key"),
            expected
        );
        assert_eq!(reservation_key(&dot_alias).expect("dot key"), expected);
    }

    #[cfg(windows)]
    #[test]
    fn windows_nonexistent_parent_aliases_cannot_hold_two_reservations() {
        let directory = tempdir().expect("temporary directory");
        let first_output = directory
            .path()
            .join("future")
            .join("child")
            .join("..")
            .join("result.mp4");
        let second_output = directory.path().join("FUTURE").join("RESULT.MP4");
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        store
            .create_job("input-a.mkv", &first_output, &plan())
            .expect("first reservation");
        let error = store
            .create_job("input-b.mkv", &second_output, &plan())
            .expect_err("Win32 aliases must share one reservation");
        assert_eq!(error.code, ErrorCode::OutputConflict);
    }

    #[cfg(windows)]
    #[test]
    fn windows_v3_migration_rebuilds_reservations_atomically() {
        let directory = tempdir().expect("temporary directory");
        let database_path = directory.path().join("legacy.sqlite3");
        drop(SqliteJobStore::open(&database_path).expect("initialize schema"));
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let first_output = directory
            .path()
            .join("future")
            .join("child")
            .join("..")
            .join("result.mp4");
        let second_output = directory.path().join("FUTURE").join("RESULT.MP4");
        let mut legacy_plan = plan();
        legacy_plan.plan_hash =
            crate::planner::deterministic_plan_hash(&legacy_plan).expect("hash legacy Plan");
        let legacy_plan_json = serde_json::to_string(&legacy_plan).expect("serialize legacy Plan");
        let legacy = rusqlite::Connection::open(&database_path).expect("open legacy database");
        legacy
            .execute("DELETE FROM schema_migrations WHERE version IN (3, 4)", [])
            .expect("remove migration marker");
        for (job_id, output) in [(first_id, &first_output), (second_id, &second_output)] {
            legacy
                .execute(
                    "INSERT INTO jobs(
                        id, state, input_path, output_path, plan_hash, plan_json,
                        sequence, created_unix_ms, updated_unix_ms
                     ) VALUES (?1, 'planned', 'input.mkv', ?2, ?3, ?4, 0, 1, 1)",
                    rusqlite::params![
                        job_id.to_string(),
                        output.to_string_lossy(),
                        legacy_plan.plan_hash,
                        legacy_plan_json
                    ],
                )
                .expect("insert legacy job");
            legacy
                .execute(
                    "INSERT INTO output_reservations(
                        canonical_output_path, job_id, created_unix_ms
                     ) VALUES (?1, ?2, 1)",
                    rusqlite::params![output.to_string_lossy(), job_id.to_string()],
                )
                .expect("insert legacy reservation");
            legacy
                .execute(
                    "INSERT INTO job_events(
                        job_id, sequence, previous_state, next_state, code, timestamp_unix_ms
                     ) VALUES (?1, 0, NULL, 'planned', 'JOB_CREATED', 1)",
                    [job_id.to_string()],
                )
                .expect("insert legacy event");
        }
        drop(legacy);

        let error = SqliteJobStore::open(&database_path)
            .expect_err("alias collision must stop the migration");
        assert_eq!(error.code, ErrorCode::OutputConflict, "{error:?}");
        let inspected = rusqlite::Connection::open(&database_path).expect("inspect rollback");
        let migration_applied = inspected
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 3)",
                [],
                |row| row.get::<_, bool>(0),
            )
            .expect("read migration marker");
        let reservation_count = inspected
            .query_row("SELECT COUNT(*) FROM output_reservations", [], |row| {
                row.get::<_, u64>(0)
            })
            .expect("count reservations");
        assert!(!migration_applied);
        assert_eq!(reservation_count, 2);
    }

    #[cfg(windows)]
    #[test]
    fn windows_reservation_rejects_trimmed_and_device_components() {
        let directory = tempdir().expect("temporary directory");
        let invalid_components = [
            "output.mp4.",
            "output.mp4 ",
            " output.mp4",
            "CON",
            "nul.txt",
            "COM1.log",
            "LPT¹.report",
            "safe:stream.mp4",
        ];
        for component in invalid_components {
            let error = reservation_key(&directory.path().join(component))
                .expect_err("unsafe Windows component must be rejected");
            assert_eq!(error.code, ErrorCode::InputInvalid, "{component}");
        }

        let nested_device = directory.path().join("AUX").join("output.mp4");
        let error = reservation_key(&nested_device).expect_err("nested device must be rejected");
        assert_eq!(error.code, ErrorCode::InputInvalid);
    }

    #[cfg(windows)]
    #[test]
    fn windows_reservation_rejects_network_and_device_namespaces() {
        let invalid_paths = [
            PathBuf::from(r"\\server\share\output.mp4"),
            PathBuf::from(r"\\?\UNC\server\share\output.mp4"),
            PathBuf::from(r"\\.\PhysicalDrive0"),
            PathBuf::from(r"\\?\GLOBALROOT\Device\HarddiskVolume1\output.mp4"),
        ];
        for path in invalid_paths {
            let error = reservation_key(&path).expect_err("namespace must be rejected");
            assert_eq!(error.code, ErrorCode::InputInvalid, "{}", path.display());
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_reservation_resolves_existing_directory_reparse_points() {
        use std::os::windows::fs::symlink_dir;

        let directory = tempdir().expect("temporary directory");
        let actual = directory.path().join("actual-output");
        let alias = directory.path().join("linked-output");
        std::fs::create_dir(&actual).expect("create actual directory");
        if let Err(error) = symlink_dir(&actual, &alias) {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                eprintln!("skipped reparse-point assertion: {error}");
                return;
            }
            panic!("create directory symlink: {error}");
        }

        let actual_output = actual.join("result.mp4");
        let alias_output = alias.join("result.mp4");
        assert_eq!(
            reservation_key(&actual_output).expect("actual key"),
            reservation_key(&alias_output).expect("alias key")
        );
        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        store
            .create_job("input-a.mkv", &actual_output, &plan())
            .expect("first reservation");
        let error = store
            .create_job("input-b.mkv", &alias_output, &plan())
            .expect_err("reparse aliases must share one reservation");
        assert_eq!(error.code, ErrorCode::OutputConflict);
    }

    #[cfg(windows)]
    #[test]
    fn windows_reparse_retarget_is_detected_before_execution() {
        use std::os::windows::fs::symlink_dir;

        let directory = tempdir().expect("temporary directory");
        let first_target = directory.path().join("first-target");
        let second_target = directory.path().join("second-target");
        let alias = directory.path().join("mutable-link");
        std::fs::create_dir(&first_target).expect("create first target");
        std::fs::create_dir(&second_target).expect("create second target");
        if let Err(error) = symlink_dir(&first_target, &alias) {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                eprintln!("skipped reparse-retarget assertion: {error}");
                return;
            }
            panic!("create directory symlink: {error}");
        }

        let mut store = SqliteJobStore::open_in_memory().expect("in-memory store");
        let job = store
            .create_job("input-a.mkv", alias.join("result.mp4"), &plan())
            .expect("reserve first reparse target");
        std::fs::remove_dir(&alias).expect("remove directory symlink");
        symlink_dir(&second_target, &alias).expect("retarget directory symlink");

        let error = store
            .validate_output_reservation(job.id)
            .expect_err("retargeted output must be blocked");
        assert_eq!(error.code, ErrorCode::OutputConflict);
        store
            .create_job("input-b.mkv", second_target.join("result.mp4"), &plan())
            .expect("new target keeps its independent reservation");
    }

    #[test]
    fn ten_thousand_jobs_are_created_and_read_in_bounded_pages() {
        let directory = tempdir().expect("temporary directory");
        let database_path = directory.path().join("queue.sqlite3");
        let mut store = SqliteJobStore::open(&database_path).expect("disk-backed store");
        let base_plan = plan();
        let requests = (0..10_000)
            .map(|index| JobCreateRequest {
                input_path: PathBuf::from(format!("inputs/{index:05}.mkv")),
                output_path: PathBuf::from(format!("outputs/{index:05}.mp4")),
                plan: base_plan.clone(),
            })
            .collect::<Vec<_>>();
        let started = Instant::now();
        let jobs = store.create_jobs(&requests).expect("create 10,000 jobs");
        let creation_elapsed = started.elapsed();
        assert_eq!(jobs.len(), 10_000);
        assert_eq!(store.count_jobs().expect("count jobs"), 10_000);
        assert!(
            creation_elapsed < Duration::from_secs(30),
            "10,000-job transaction exceeded the architecture gate"
        );

        drop(store);
        let mut store = SqliteJobStore::open(&database_path).expect("reopen durable store");
        assert_eq!(store.count_jobs().expect("count after reopen"), 10_000);

        let mut observed = HashSet::with_capacity(10_000);
        let mut offset = 0;
        let paging_started = Instant::now();
        loop {
            let page = store
                .list_jobs_page(137, offset)
                .expect("read bounded page");
            if page.is_empty() {
                break;
            }
            assert!(page.len() <= 137);
            observed.extend(page.into_iter().map(|job| job.id));
            offset += 137;
        }
        assert_eq!(observed.len(), 10_000);
        eprintln!(
            "FORMATWRIGHT_QUEUE_BENCHMARK jobs=10000 create_ms={} page_size=137 paging_ms={}",
            creation_elapsed.as_millis(),
            paging_started.elapsed().as_millis()
        );

        let queued = store
            .transition(jobs[0].id, JobState::Queued, "JOB_ENQUEUED")
            .expect("enqueue");
        assert_eq!(queued.state, JobState::Queued);
        let cancelled = store
            .transition(jobs[0].id, JobState::Cancelled, "USER_CANCELLED")
            .expect("cancel queued job");
        assert_eq!(cancelled.state, JobState::Cancelled);
        let retried = store
            .transition(jobs[0].id, JobState::Queued, "JOB_RETRIED")
            .expect("retry cancelled job");
        assert_eq!(retried.state, JobState::Queued);
        let scheduling_window = store
            .list_jobs_by_state(JobState::Queued, 7)
            .expect("bounded scheduling window");
        assert_eq!(scheduling_window.len(), 1);
        assert_eq!(scheduling_window[0].id, jobs[0].id);
    }
}
