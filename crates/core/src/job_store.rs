use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, ErrorCode as SqliteErrorCode, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::{JobState, Plan, SCHEMA_VERSION};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};

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
        let connection = Connection::open(path).map_err(storage_error)?;
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
            .map_err(storage_error)
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
        let state = JobState::Planned;
        let transaction = self.connection.transaction().map_err(storage_error)?;
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
            reserve_output(&transaction, id, &request.output_path, now)?;
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
        transaction.commit().map_err(storage_error)?;
        Ok(records)
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
        let transaction = self.connection.transaction().map_err(storage_error)?;
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
        let transaction = self.connection.transaction().map_err(storage_error)?;
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
    let rendered = canonical_parent
        .join(file_name)
        .to_string_lossy()
        .into_owned();
    #[cfg(windows)]
    {
        Ok(rendered.to_lowercase())
    }
    #[cfg(not(windows))]
    {
        Ok(rendered)
    }
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
    use std::time::{Duration, Instant};

    use tempfile::tempdir;
    use uuid::Uuid;

    use super::{JobCreateRequest, SqliteJobStore};
    use crate::ErrorCode;
    use crate::domain::{ChangeSet, JobState, NetworkPolicy, Plan, SCHEMA_VERSION};

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
