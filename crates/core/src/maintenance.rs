use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::job_store::SqliteJobStore;

pub(crate) const DATABASE_SCHEMA_VERSION: i64 = 4;
const BACKUP_PAGES_PER_STEP: i32 = 128;
const BACKUP_STEP_PAUSE: Duration = Duration::from_millis(5);
const BACKUP_BUSY_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_SNAPSHOT_RETENTION: usize = 5;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IntegrityReport {
    pub database_path: PathBuf,
    pub ok: bool,
    pub sqlite_messages: Vec<String>,
    pub foreign_key_violations: Vec<String>,
    pub application_issues: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MaintenanceStatus {
    pub database_path: PathBuf,
    pub size_bytes: u64,
    pub schema_version: i64,
    pub supported_schema_version: i64,
    pub journal_mode: String,
    pub job_count: u64,
    pub active_job_count: u64,
    pub integrity_ok: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackupReport {
    pub database_path: PathBuf,
    pub backup_path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
    pub schema_version: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RestorePreflightReport {
    pub backup_path: PathBuf,
    pub source_schema_version: i64,
    pub restored_schema_version: i64,
    pub migration_required: bool,
    pub integrity: IntegrityReport,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RestoreReport {
    pub database_path: PathBuf,
    pub restored_from: PathBuf,
    pub safety_backup: Option<PathBuf>,
    pub schema_version: i64,
    pub integrity: IntegrityReport,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompactReport {
    pub database_path: PathBuf,
    pub size_before_bytes: u64,
    pub size_after_bytes: u64,
    pub reclaimed_bytes: u64,
    pub integrity: IntegrityReport,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaintenanceService {
    database_path: PathBuf,
}

impl MaintenanceService {
    #[must_use]
    pub fn new(database_path: impl Into<PathBuf>) -> Self {
        Self {
            database_path: database_path.into(),
        }
    }

    #[must_use]
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    /// Returns lightweight operational and integrity information without changing the database.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the database is absent, unreadable, or malformed.
    pub fn status(&self) -> Result<MaintenanceStatus> {
        let connection = open_read_only(&self.database_path)?;
        let schema_version = schema_version(&connection)?;
        ensure_supported_schema(schema_version)?;
        let journal_mode = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .map_err(storage_error)?;
        let job_count = query_count(&connection, "SELECT COUNT(*) FROM jobs")?;
        let active_job_count = query_count(
            &connection,
            "SELECT COUNT(*) FROM jobs
             WHERE state NOT IN ('completed', 'warning', 'failed', 'cancelled')",
        )?;
        let integrity_ok = integrity_check_connection(&self.database_path, &connection)?.ok;
        Ok(MaintenanceStatus {
            database_path: self.database_path.clone(),
            size_bytes: database_size(&self.database_path)?,
            schema_version,
            supported_schema_version: DATABASE_SCHEMA_VERSION,
            journal_mode,
            job_count,
            active_job_count,
            integrity_ok,
        })
    }

    /// Runs `SQLite` page, foreign-key, and `FormatWright` queue-invariant checks.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the database cannot be read far enough to run the checks.
    pub fn integrity_check(&self) -> Result<IntegrityReport> {
        integrity_check_path(&self.database_path)
    }

    /// Creates a consistent online backup through a same-directory partial file.
    ///
    /// # Errors
    ///
    /// Returns an error for an existing destination, an invalid source, or a failed backup.
    pub fn backup(&self, destination: impl AsRef<Path>) -> Result<BackupReport> {
        backup_database(&self.database_path, destination.as_ref(), false)
    }

    /// Validates a restore on a migrated temporary copy and leaves the live database unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error when the backup is corrupt, too new, or fails post-migration validation.
    pub fn restore_preflight(
        &self,
        backup_path: impl AsRef<Path>,
    ) -> Result<RestorePreflightReport> {
        let prepared = self.prepare_restore(backup_path.as_ref())?;
        let report = prepared.report.clone();
        let _ = remove_database_files(&prepared.stage_path);
        Ok(report)
    }

    /// Restores a prevalidated copy and keeps an automatic safety backup of the current database.
    ///
    /// The caller must ensure no long-running conversion process is using the database.
    ///
    /// # Errors
    ///
    /// Returns an error without replacing the live database if preflight or switching fails.
    pub fn restore(&self, backup_path: impl AsRef<Path>) -> Result<RestoreReport> {
        let backup_path = backup_path.as_ref();
        ensure_distinct_paths(backup_path, &self.database_path)?;
        let prepared = self.prepare_restore(backup_path)?;
        let safety_backup = if self.database_path.exists() {
            Some(self.create_automatic_snapshot("pre-restore")?.backup_path)
        } else {
            None
        };

        if let Some(parent) = self.database_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|error| {
                maintenance_error(
                    ErrorCode::StorageFailed,
                    format!("Cannot create state directory: {}", parent.display()),
                    "Choose a writable state database path.",
                    error,
                )
            })?;
        }
        if let Err(error) = online_copy(&prepared.stage_path, &self.database_path, CopyMode::Live) {
            let _ = remove_database_files(&prepared.stage_path);
            return Err(error);
        }
        remove_database_files(&prepared.stage_path)?;
        let integrity = integrity_check_path(&self.database_path)?;
        if !integrity.ok {
            if let Some(path) = &safety_backup {
                let _ = online_copy(path, &self.database_path, CopyMode::Live);
            }
            return Err(integrity_failure(
                "The restored database failed validation after the transactional switch",
                &integrity,
            ));
        }
        Ok(RestoreReport {
            database_path: self.database_path.clone(),
            restored_from: backup_path.to_path_buf(),
            safety_backup,
            schema_version: prepared.report.restored_schema_version,
            integrity,
        })
    }

    /// Compacts the live database after taking a safety snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when the database is busy, corrupt, or cannot be compacted.
    pub fn compact(&self) -> Result<CompactReport> {
        let before = self.integrity_check()?;
        if !before.ok {
            return Err(integrity_failure(
                "Refusing to compact a database that failed integrity checks",
                &before,
            ));
        }
        let _snapshot = self.create_automatic_snapshot("pre-compact")?;
        let size_before_bytes = database_size(&self.database_path)?;
        let connection = open_read_write_existing(&self.database_path)?;
        connection.execute_batch("VACUUM").map_err(storage_error)?;
        drop(connection);
        let size_after_bytes = database_size(&self.database_path)?;
        let integrity = self.integrity_check()?;
        Ok(CompactReport {
            database_path: self.database_path.clone(),
            size_before_bytes,
            size_after_bytes,
            reclaimed_bytes: size_before_bytes.saturating_sub(size_after_bytes),
            integrity,
        })
    }

    pub(crate) fn create_automatic_snapshot(&self, reason: &str) -> Result<BackupReport> {
        let parent = parent_directory(&self.database_path)?;
        let backup_directory = parent.join("backups");
        fs::create_dir_all(&backup_directory).map_err(|error| {
            maintenance_error(
                ErrorCode::StorageFailed,
                format!(
                    "Cannot create backup directory: {}",
                    backup_directory.display()
                ),
                "Choose a writable application data directory.",
                error,
            )
        })?;
        let file_name = self
            .database_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("jobs.sqlite3");
        let destination =
            backup_directory.join(format!("{file_name}.{reason}.{}.sqlite3", Uuid::new_v4()));
        let report = backup_database(&self.database_path, &destination, false)?;
        prune_automatic_snapshots(
            &backup_directory,
            file_name,
            DEFAULT_SNAPSHOT_RETENTION,
            &destination,
        )?;
        Ok(report)
    }

    fn prepare_restore(&self, backup_path: &Path) -> Result<PreparedRestore> {
        let source_connection = open_read_only(backup_path)?;
        let source_schema_version = schema_version(&source_connection)?;
        ensure_supported_schema(source_schema_version)?;
        let source_integrity = integrity_check_connection(backup_path, &source_connection)?;
        if !source_integrity.ok {
            return Err(integrity_failure(
                "The selected backup failed integrity checks",
                &source_integrity,
            ));
        }
        drop(source_connection);

        let stage_parent = parent_directory(&self.database_path)?;
        fs::create_dir_all(stage_parent).map_err(|error| {
            maintenance_error(
                ErrorCode::StorageFailed,
                format!(
                    "Cannot create restore staging directory: {}",
                    stage_parent.display()
                ),
                "Choose a writable state database path.",
                error,
            )
        })?;
        let stage_path = sibling_temporary_path(&self.database_path, "restore-stage")?;
        if let Err(error) = online_copy(backup_path, &stage_path, CopyMode::Portable) {
            let _ = remove_database_files(&stage_path);
            return Err(error);
        }
        let migration_required = source_schema_version < DATABASE_SCHEMA_VERSION;
        if let Err(error) = SqliteJobStore::open_for_restore_staging(&stage_path) {
            let _ = remove_database_files(&stage_path);
            return Err(error);
        }
        let integrity = match integrity_check_path(&stage_path) {
            Ok(report) => report,
            Err(error) => {
                let _ = remove_database_files(&stage_path);
                return Err(error);
            }
        };
        if !integrity.ok {
            let error = integrity_failure(
                "The migrated restore copy failed integrity checks",
                &integrity,
            );
            let _ = remove_database_files(&stage_path);
            return Err(error);
        }
        let restored_schema_version = {
            let connection = open_read_only(&stage_path)?;
            schema_version(&connection)?
        };
        Ok(PreparedRestore {
            stage_path,
            report: RestorePreflightReport {
                backup_path: backup_path.to_path_buf(),
                source_schema_version,
                restored_schema_version,
                migration_required,
                integrity,
            },
        })
    }
}

#[derive(Debug)]
struct PreparedRestore {
    stage_path: PathBuf,
    report: RestorePreflightReport,
}

pub(crate) fn automatic_snapshot_before_migration(database_path: &Path) -> Result<()> {
    if !database_path.exists() || database_size(database_path)? == 0 {
        return Ok(());
    }
    let connection = open_read_only(database_path)?;
    let version = schema_version(&connection)?;
    drop(connection);
    if version >= DATABASE_SCHEMA_VERSION {
        return Ok(());
    }
    MaintenanceService::new(database_path).create_automatic_snapshot(&format!(
        "pre-migration-v{version}-to-v{DATABASE_SCHEMA_VERSION}"
    ))?;
    Ok(())
}

fn backup_database(source: &Path, destination: &Path, overwrite: bool) -> Result<BackupReport> {
    if !source.exists() {
        return Err(FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Store,
            format!("Database does not exist: {}", source.display()),
            "Choose an existing FormatWright state database.",
        ));
    }
    ensure_distinct_paths(source, destination)?;
    if destination.exists() && !overwrite {
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Store,
            format!(
                "Backup destination already exists: {}",
                destination.display()
            ),
            "Choose a new backup path; existing backups are never overwritten.",
        ));
    }
    let parent = parent_directory(destination)?;
    fs::create_dir_all(parent).map_err(|error| {
        maintenance_error(
            ErrorCode::StorageFailed,
            format!("Cannot create backup directory: {}", parent.display()),
            "Choose a writable backup destination.",
            error,
        )
    })?;
    let partial = sibling_temporary_path(destination, "backup-partial")?;
    let result = (|| {
        online_copy(source, &partial, CopyMode::Portable)?;
        let integrity = integrity_check_path(&partial)?;
        if !integrity.ok {
            return Err(integrity_failure(
                "The new backup failed integrity validation",
                &integrity,
            ));
        }
        sync_file(&partial)?;
        if overwrite && destination.exists() {
            remove_if_exists(destination)?;
        }
        fs::rename(&partial, destination).map_err(|error| {
            maintenance_error(
                ErrorCode::StorageFailed,
                "Cannot commit the validated backup",
                "Choose another writable backup destination.",
                error,
            )
        })?;
        let connection = open_read_only(destination)?;
        let schema_version = schema_version(&connection)?;
        Ok(BackupReport {
            database_path: source.to_path_buf(),
            backup_path: destination.to_path_buf(),
            size_bytes: database_size(destination)?,
            sha256: sha256_file(destination)?,
            schema_version,
        })
    })();
    if result.is_err() {
        let _ = remove_database_files(&partial);
    }
    result
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CopyMode {
    Portable,
    Live,
}

fn online_copy(source: &Path, destination: &Path, mode: CopyMode) -> Result<()> {
    let source_connection = open_read_only(source)?;
    let mut destination_connection = Connection::open(destination).map_err(storage_error)?;
    destination_connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(storage_error)?;
    {
        let backup =
            Backup::new(&source_connection, &mut destination_connection).map_err(storage_error)?;
        let deadline = Instant::now() + BACKUP_BUSY_TIMEOUT;
        loop {
            match backup.step(BACKUP_PAGES_PER_STEP).map_err(storage_error)? {
                StepResult::Done => break,
                StepResult::More => std::thread::sleep(BACKUP_STEP_PAUSE),
                StepResult::Busy | StepResult::Locked if Instant::now() < deadline => {
                    std::thread::sleep(BACKUP_STEP_PAUSE);
                }
                StepResult::Busy | StepResult::Locked => {
                    return Err(FormatWrightError::new(
                        ErrorCode::PolicyBlocked,
                        Stage::Store,
                        "SQLite maintenance could not acquire a database lock within 30 seconds",
                        "Stop queue execution, close other FormatWright processes, and retry.",
                    )
                    .retryable(true));
                }
                _ => {
                    return Err(FormatWrightError::new(
                        ErrorCode::StorageFailed,
                        Stage::Store,
                        "SQLite returned an unknown online-backup result",
                        "Retry with a supported SQLite runtime.",
                    ));
                }
            }
        }
    }
    if mode == CopyMode::Portable {
        destination_connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(storage_error)?;
        let actual_journal_mode = destination_connection
            .query_row("PRAGMA journal_mode=DELETE", [], |row| {
                row.get::<_, String>(0)
            })
            .map_err(storage_error)?;
        if !actual_journal_mode.eq_ignore_ascii_case("DELETE") {
            return Err(FormatWrightError::new(
                ErrorCode::StorageFailed,
                Stage::Store,
                format!(
                    "SQLite refused portable backup journal mode DELETE: {actual_journal_mode}"
                ),
                "Close other FormatWright processes and retry maintenance.",
            ));
        }
    }
    drop(destination_connection);
    drop(source_connection);
    if mode == CopyMode::Portable {
        remove_sqlite_sidecars(destination)?;
    }
    Ok(())
}

fn integrity_check_path(path: &Path) -> Result<IntegrityReport> {
    let connection = open_read_only(path)?;
    integrity_check_connection(path, &connection)
}

fn integrity_check_connection(path: &Path, connection: &Connection) -> Result<IntegrityReport> {
    let sqlite_messages = sqlite_check_messages(connection, "PRAGMA integrity_check")?;
    let foreign_key_violations = foreign_key_violations(connection)?;
    let application_issues = application_issues(connection)?;
    let sqlite_ok = sqlite_messages
        .iter()
        .all(|message| message.eq_ignore_ascii_case("ok"));
    Ok(IntegrityReport {
        database_path: path.to_path_buf(),
        ok: sqlite_ok && foreign_key_violations.is_empty() && application_issues.is_empty(),
        sqlite_messages,
        foreign_key_violations,
        application_issues,
    })
}

fn sqlite_check_messages(connection: &Connection, pragma: &str) -> Result<Vec<String>> {
    let mut statement = connection.prepare(pragma).map_err(storage_error)?;
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(storage_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(storage_error)
}

fn foreign_key_violations(connection: &Connection) -> Result<Vec<String>> {
    let mut statement = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(storage_error)?;
    statement
        .query_map([], |row| {
            let table: String = row.get(0)?;
            let row_id: Option<i64> = row.get(1)?;
            let parent: String = row.get(2)?;
            let foreign_key: i64 = row.get(3)?;
            Ok(format!(
                "table={table}, row_id={row_id:?}, parent={parent}, foreign_key={foreign_key}"
            ))
        })
        .map_err(storage_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(storage_error)
}

#[allow(clippy::too_many_lines)]
fn application_issues(connection: &Connection) -> Result<Vec<String>> {
    let mut issues = Vec::new();
    let versions = {
        let mut statement = connection
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .map_err(storage_error)?;
        statement
            .query_map([], |row| row.get::<_, i64>(0))
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?
    };
    let recorded_version = versions.last().copied().unwrap_or_default();
    let expected = (1..=recorded_version).collect::<Vec<_>>();
    if versions != expected {
        issues.push(format!(
            "schema migration markers are not contiguous: {versions:?}"
        ));
    }

    append_count_issue(
        connection,
        &mut issues,
        "jobs with an unknown state",
        "SELECT COUNT(*) FROM jobs
         WHERE state NOT IN (
             'queued', 'inspecting', 'planned', 'blocked', 'running', 'validating',
             'completed', 'warning', 'failed', 'cancelled', 'interrupted'
         )",
        false,
    )?;
    append_count_issue(
        connection,
        &mut issues,
        "active jobs without exactly one output reservation",
        "SELECT COUNT(*) FROM jobs
         LEFT JOIN output_reservations ON output_reservations.job_id = jobs.id
         WHERE jobs.state NOT IN ('completed', 'warning', 'failed', 'cancelled')
         GROUP BY jobs.id HAVING COUNT(output_reservations.job_id) <> 1",
        true,
    )?;
    append_count_issue(
        connection,
        &mut issues,
        "terminal jobs retaining output reservations",
        "SELECT COUNT(*) FROM jobs
         JOIN output_reservations ON output_reservations.job_id = jobs.id
         WHERE jobs.state IN ('completed', 'warning', 'failed', 'cancelled')",
        false,
    )?;
    append_count_issue(
        connection,
        &mut issues,
        "jobs whose stored sequence disagrees with their latest event",
        "SELECT COUNT(*) FROM jobs
         WHERE sequence <> COALESCE(
             (SELECT MAX(job_events.sequence) FROM job_events
              WHERE job_events.job_id = jobs.id), 0)",
        false,
    )?;
    append_count_issue(
        connection,
        &mut issues,
        "jobs with missing or non-contiguous event sequences",
        "SELECT COUNT(*) FROM jobs
         WHERE NOT EXISTS (
             SELECT 1 FROM job_events
             WHERE job_events.job_id = jobs.id AND job_events.sequence = 0
         ) OR (
             SELECT COUNT(*) FROM job_events WHERE job_events.job_id = jobs.id
         ) <> jobs.sequence + 1",
        false,
    )?;
    append_count_issue(
        connection,
        &mut issues,
        "jobs whose latest event state disagrees with the job state",
        "SELECT COUNT(*) FROM jobs
         WHERE state <> (
             SELECT next_state FROM job_events
             WHERE job_events.job_id = jobs.id
             ORDER BY sequence DESC LIMIT 1
         )",
        false,
    )?;
    append_count_issue(
        connection,
        &mut issues,
        "events whose previous state disagrees with the prior event",
        "SELECT COUNT(*) FROM job_events AS current
         WHERE current.sequence > 0 AND current.previous_state <> (
             SELECT prior.next_state FROM job_events AS prior
             WHERE prior.job_id = current.job_id
               AND prior.sequence = current.sequence - 1
         )",
        false,
    )?;
    if recorded_version >= 4 {
        append_count_issue(
            connection,
            &mut issues,
            "selection snapshots whose stored member count disagrees with membership",
            "SELECT COUNT(*) FROM selection_snapshots
             WHERE member_count <> (
                 SELECT COUNT(*) FROM selection_members
                 WHERE selection_members.selection_id = selection_snapshots.id
             )",
            false,
        )?;
        append_count_issue(
            connection,
            &mut issues,
            "bulk actions whose outcome counts do not sum to matched count",
            "SELECT COUNT(*) FROM bulk_actions
             WHERE matched_count < 0 OR transitioned_count < 0
                OR skipped_state_count < 0 OR skipped_conflict_count < 0
                OR matched_count <> transitioned_count
                    + skipped_state_count + skipped_conflict_count",
            false,
        )?;
        append_count_issue(
            connection,
            &mut issues,
            "bulk actions whose stored matched count disagrees with member outcomes",
            "SELECT COUNT(*) FROM bulk_actions
             WHERE matched_count <> (
                 SELECT COUNT(*) FROM bulk_action_members
                 WHERE bulk_action_members.action_id = bulk_actions.action_id
             )",
            false,
        )?;
        append_count_issue(
            connection,
            &mut issues,
            "bulk action members with an unknown outcome",
            "SELECT COUNT(*) FROM bulk_action_members
             WHERE outcome NOT IN (
                 'transitioned', 'skipped-state', 'skipped-output-conflict'
             )",
            false,
        )?;
    }

    let mut statement = connection
        .prepare("SELECT id, plan_hash, plan_json FROM jobs")
        .map_err(storage_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(storage_error)?;
    for row in rows {
        let (job_id, stored_hash, plan_json) = row.map_err(storage_error)?;
        match serde_json::from_str::<crate::domain::Plan>(&plan_json) {
            Ok(plan)
                if plan.plan_hash == stored_hash
                    && crate::planner::deterministic_plan_hash(&plan)
                        .is_ok_and(|computed| computed == stored_hash) => {}
            Ok(_) => issues.push(format!("job {job_id} has a mismatched Plan hash")),
            Err(error) => issues.push(format!("job {job_id} has invalid Plan JSON: {error}")),
        }
    }
    Ok(issues)
}

fn append_count_issue(
    connection: &Connection,
    issues: &mut Vec<String>,
    label: &str,
    sql: &str,
    grouped: bool,
) -> Result<()> {
    let count = if grouped {
        let wrapped = format!("SELECT COUNT(*) FROM ({sql})");
        query_count(connection, &wrapped)?
    } else {
        query_count(connection, sql)?
    };
    if count > 0 {
        issues.push(format!("{label}: {count}"));
    }
    Ok(())
}

fn schema_version(connection: &Connection) -> Result<i64> {
    let has_migrations = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_schema
                WHERE type = 'table' AND name = 'schema_migrations'
            )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_error)?;
    if !has_migrations {
        return Ok(0);
    }
    connection
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get::<_, Option<i64>>(0)
        })
        .map_err(storage_error)
        .map(Option::unwrap_or_default)
}

fn ensure_supported_schema(version: i64) -> Result<()> {
    if version > DATABASE_SCHEMA_VERSION {
        return Err(FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Store,
            format!(
                "Database schema v{version} is newer than supported v{DATABASE_SCHEMA_VERSION}"
            ),
            "Open this database with an equal or newer FormatWright release.",
        ));
    }
    Ok(())
}

fn query_count(connection: &Connection, sql: &str) -> Result<u64> {
    connection
        .query_row(sql, [], |row| row.get::<_, u64>(0))
        .map_err(storage_error)
}

fn open_read_only(path: &Path) -> Result<Connection> {
    if !path.is_file() {
        return Err(FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Store,
            format!("Database does not exist: {}", path.display()),
            "Choose an existing FormatWright state database.",
        ));
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(storage_error)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(storage_error)?;
    Ok(connection)
}

fn open_read_write_existing(path: &Path) -> Result<Connection> {
    if !path.is_file() {
        return Err(FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Store,
            format!("Database does not exist: {}", path.display()),
            "Choose an existing FormatWright state database.",
        ));
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(storage_error)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(storage_error)?;
    Ok(connection)
}

fn database_size(path: &Path) -> Result<u64> {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| {
            maintenance_error(
                ErrorCode::StorageFailed,
                format!("Cannot read database metadata: {}", path.display()),
                "Check that the database exists and is readable.",
                error,
            )
        })
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|error| {
        maintenance_error(
            ErrorCode::StorageFailed,
            format!("Cannot hash backup: {}", path.display()),
            "Check that the backup is readable.",
            error,
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            maintenance_error(
                ErrorCode::StorageFailed,
                "Cannot read the backup while hashing",
                "Retry the backup on a healthy local disk.",
                error,
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn sync_file(path: &Path) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|mut file| {
            file.flush()?;
            file.sync_all()
        })
        .map_err(|error| {
            maintenance_error(
                ErrorCode::StorageFailed,
                format!("Cannot flush backup to disk: {}", path.display()),
                "Retry on a healthy writable local disk.",
                error,
            )
        })
}

fn ensure_distinct_paths(source: &Path, destination: &Path) -> Result<()> {
    let source = canonical_or_absolute_path(source)?;
    let destination = canonical_or_absolute_path(destination)?;
    if paths_equal(&source, &destination) {
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Store,
            "Backup destination cannot be the live database",
            "Choose a different backup path.",
        ));
    }
    Ok(())
}

fn canonical_or_absolute_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return path.canonicalize().map_err(|error| {
            maintenance_error(
                ErrorCode::StorageFailed,
                format!("Cannot resolve maintenance path: {}", path.display()),
                "Choose a normal readable local path.",
                error,
            )
        });
    }
    absolute_lexical_path(path)
}

fn absolute_lexical_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|directory| directory.join(path))
        .map_err(|error| {
            maintenance_error(
                ErrorCode::StorageFailed,
                "Cannot resolve the current directory",
                "Use an absolute database path.",
                error,
            )
        })
}

#[cfg(windows)]
fn paths_equal(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(not(windows))]
fn paths_equal(left: &Path, right: &Path) -> bool {
    left == right
}

fn sibling_temporary_path(path: &Path, label: &str) -> Result<PathBuf> {
    let parent = parent_directory(path)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("formatwright.sqlite3");
    Ok(parent.join(format!(".{name}.{label}.{}", Uuid::new_v4())))
}

fn parent_directory(path: &Path) -> Result<&Path> {
    if let Some(parent) = path.parent() {
        return if parent.as_os_str().is_empty() {
            Ok(Path::new("."))
        } else {
            Ok(parent)
        };
    }
    if path.file_name().is_some() {
        return Ok(Path::new("."));
    }
    Err(FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Store,
        format!("Path has no parent directory: {}", path.display()),
        "Use a database path with a writable parent directory.",
    ))
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(maintenance_error(
            ErrorCode::StorageFailed,
            format!("Cannot remove maintenance file: {}", path.display()),
            "Close FormatWright and retry maintenance.",
            error,
        )),
    }
}

fn remove_database_files(path: &Path) -> Result<()> {
    remove_if_exists(path)?;
    remove_sqlite_sidecars(path)
}

fn remove_sqlite_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        remove_if_exists(Path::new(&sidecar))?;
    }
    Ok(())
}

fn prune_automatic_snapshots(
    directory: &Path,
    prefix: &str,
    retain: usize,
    preserve: &Path,
) -> Result<()> {
    let mut snapshots = fs::read_dir(directory)
        .map_err(|error| {
            maintenance_error(
                ErrorCode::StorageFailed,
                format!("Cannot read backup directory: {}", directory.display()),
                "Check application-data permissions.",
                error,
            )
        })?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(&format!("{prefix}.")) || !name.ends_with(".sqlite3") {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect::<Vec<_>>();
    snapshots.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    let mut kept_others = 0_usize;
    let other_limit = retain.saturating_sub(1);
    for (_, path) in snapshots {
        if paths_equal(&path, preserve) {
            continue;
        }
        if kept_others < other_limit {
            kept_others = kept_others.saturating_add(1);
        } else {
            remove_if_exists(&path)?;
        }
    }
    Ok(())
}

fn integrity_failure(message: &str, report: &IntegrityReport) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::StorageFailed,
        Stage::Store,
        message,
        "Keep the current database unchanged and inspect the maintenance report.",
    )
    .with_diagnostic(format!(
        "sqlite={:?}; foreign_keys={:?}; application={:?}",
        report.sqlite_messages, report.foreign_key_violations, report.application_issues
    ))
}

#[allow(clippy::needless_pass_by_value)]
fn storage_error(error: rusqlite::Error) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::StorageFailed,
        Stage::Store,
        "SQLite maintenance operation failed",
        "Close other FormatWright processes, verify disk health, and retry.",
    )
    .with_diagnostic(error.to_string())
}

fn maintenance_error(
    code: ErrorCode,
    message: impl Into<String>,
    action: impl Into<String>,
    error: impl std::fmt::Display,
) -> FormatWrightError {
    FormatWrightError::new(code, Stage::Store, message, action).with_diagnostic(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rusqlite::Connection;
    use tempfile::tempdir;

    use super::{DATABASE_SCHEMA_VERSION, MaintenanceService};
    use crate::job_store::SqliteJobStore;

    #[test]
    fn healthy_database_reports_status_and_integrity() {
        let directory = tempdir().expect("temp directory");
        let database = directory.path().join("jobs.sqlite3");
        drop(SqliteJobStore::open(&database).expect("initialize database"));

        let service = MaintenanceService::new(&database);
        let status = service.status().expect("maintenance status");
        assert_eq!(status.schema_version, DATABASE_SCHEMA_VERSION);
        assert_eq!(status.job_count, 0);
        assert!(status.integrity_ok);
        assert!(service.integrity_check().expect("integrity").ok);
    }

    #[test]
    fn online_backup_is_validated_and_never_overwrites() {
        let directory = tempdir().expect("temp directory");
        let database = directory.path().join("jobs.sqlite3");
        drop(SqliteJobStore::open(&database).expect("initialize database"));
        let backup = directory.path().join("manual.sqlite3");
        let service = MaintenanceService::new(&database);

        let report = service.backup(&backup).expect("online backup");
        assert_eq!(report.backup_path, backup);
        assert_eq!(report.sha256.len(), 64);
        assert!(
            MaintenanceService::new(&backup)
                .integrity_check()
                .expect("backup integrity")
                .ok
        );
        let entries = directory
            .path()
            .read_dir()
            .expect("backup directory")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        assert!(!entries.iter().any(|name| name.contains("backup-partial")));
        assert!(!entries.iter().any(|name| name == "manual.sqlite3-wal"));
        assert!(!entries.iter().any(|name| name == "manual.sqlite3-shm"));
        let error = service.backup(&backup).expect_err("must not overwrite");
        assert_eq!(error.code, crate::ErrorCode::OutputConflict);
    }

    #[test]
    fn online_backup_reads_a_committed_snapshot_during_a_wal_write() {
        let directory = tempdir().expect("temp directory");
        let database = directory.path().join("jobs.sqlite3");
        drop(SqliteJobStore::open(&database).expect("initialize database"));
        let mut writer = Connection::open(&database).expect("writer connection");
        writer
            .execute_batch(
                "CREATE TABLE maintenance_probe(value INTEGER NOT NULL);
                 INSERT INTO maintenance_probe VALUES (1);",
            )
            .expect("committed probe");
        let transaction = writer.transaction().expect("writer transaction");
        transaction
            .execute("INSERT INTO maintenance_probe VALUES (2)", [])
            .expect("uncommitted probe");

        let backup = directory.path().join("online.sqlite3");
        MaintenanceService::new(&database)
            .backup(&backup)
            .expect("online backup during WAL write");
        let backed_up = Connection::open(&backup).expect("backup connection");
        assert_eq!(
            backed_up
                .query_row("SELECT COUNT(*) FROM maintenance_probe", [], |row| {
                    row.get::<_, u64>(0)
                })
                .expect("committed row count"),
            1
        );
        transaction.rollback().expect("rollback writer");
    }

    #[test]
    fn application_integrity_detects_non_contiguous_migrations() {
        let directory = tempdir().expect("temp directory");
        let database = directory.path().join("jobs.sqlite3");
        drop(SqliteJobStore::open(&database).expect("initialize database"));
        let connection = Connection::open(&database).expect("raw database");
        connection
            .execute("DELETE FROM schema_migrations WHERE version = 2", [])
            .expect("damage migration markers");
        drop(connection);

        let report = MaintenanceService::new(&database)
            .integrity_check()
            .expect("integrity report");
        assert!(!report.ok);
        assert!(
            report
                .application_issues
                .iter()
                .any(|issue| issue.contains("not contiguous"))
        );
    }

    #[test]
    fn application_integrity_detects_batch_action_count_drift() {
        let directory = tempdir().expect("temp directory");
        let database = directory.path().join("jobs.sqlite3");
        drop(SqliteJobStore::open(&database).expect("initialize database"));
        let connection = Connection::open(&database).expect("raw database");
        connection
            .execute(
                "INSERT INTO selection_snapshots(
                    id, query_json, member_count, created_unix_ms
                 ) VALUES ('selection-drift', '{}', 1, 0)",
                [],
            )
            .expect("insert mismatched selection");
        connection
            .execute(
                "INSERT INTO bulk_actions(
                    action_id, selection_id, action, matched_count,
                    transitioned_count, skipped_state_count,
                    skipped_conflict_count, created_unix_ms
                 ) VALUES ('action-drift', 'selection-drift', 'retry', 1, 1, 0, 0, 0)",
                [],
            )
            .expect("insert mismatched bulk action");
        drop(connection);

        let report = MaintenanceService::new(&database)
            .integrity_check()
            .expect("integrity report");
        assert!(!report.ok);
        assert!(
            report
                .application_issues
                .iter()
                .any(|issue| issue.contains("selection snapshots"))
        );
        assert!(
            report
                .application_issues
                .iter()
                .any(|issue| issue.contains("matched count"))
        );
    }

    #[test]
    fn restore_preflight_migrates_a_copy_without_touching_the_source() {
        let directory = tempdir().expect("temp directory");
        let live = directory.path().join("live.sqlite3");
        drop(SqliteJobStore::open(&live).expect("initialize live"));
        let legacy = directory.path().join("legacy.sqlite3");
        create_legacy_v2_database(&legacy);
        let source_bytes = fs::read(&legacy).expect("legacy bytes");

        let report = MaintenanceService::new(&live)
            .restore_preflight(&legacy)
            .expect("restore preflight");
        assert!(report.migration_required);
        assert_eq!(report.source_schema_version, 2);
        assert_eq!(report.restored_schema_version, DATABASE_SCHEMA_VERSION);
        assert!(report.integrity.ok);
        assert_eq!(fs::read(&legacy).expect("legacy unchanged"), source_bytes);
    }

    #[test]
    fn opening_legacy_database_creates_a_pre_migration_snapshot() {
        let directory = tempdir().expect("temp directory");
        let legacy = directory.path().join("jobs.sqlite3");
        create_legacy_v2_database(&legacy);

        drop(SqliteJobStore::open(&legacy).expect("migrate legacy database"));

        let backups = directory
            .path()
            .join("backups")
            .read_dir()
            .expect("backup directory")
            .map(|entry| entry.expect("backup entry").path())
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        let snapshot = MaintenanceService::new(&backups[0])
            .status()
            .expect("snapshot status");
        assert_eq!(snapshot.schema_version, 2);
        assert_eq!(
            MaintenanceService::new(&legacy)
                .status()
                .expect("migrated status")
                .schema_version,
            DATABASE_SCHEMA_VERSION
        );
    }

    #[test]
    fn opening_v3_database_snapshots_before_batch_schema_migration() {
        let directory = tempdir().expect("temp directory");
        let database = directory.path().join("jobs.sqlite3");
        drop(SqliteJobStore::open(&database).expect("initialize database"));
        let connection = Connection::open(&database).expect("raw database");
        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 DROP TABLE bulk_action_members;
                 DROP TABLE bulk_actions;
                 DROP TABLE selection_members;
                 DROP TABLE selection_snapshots;
                 DROP TABLE job_idempotency_keys;
                 DROP TABLE batch_members;
                 DROP TABLE batches;
                 DELETE FROM schema_migrations WHERE version = 4;",
            )
            .expect("downgrade fixture to schema v3");
        drop(connection);

        drop(SqliteJobStore::open(&database).expect("migrate v3 database"));

        let backups = directory
            .path()
            .join("backups")
            .read_dir()
            .expect("backup directory")
            .map(|entry| entry.expect("backup entry").path())
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            MaintenanceService::new(&backups[0])
                .status()
                .expect("snapshot status")
                .schema_version,
            3
        );
        assert_eq!(
            MaintenanceService::new(&database)
                .status()
                .expect("migrated status")
                .schema_version,
            DATABASE_SCHEMA_VERSION
        );
    }

    #[test]
    fn restore_preflight_rejects_a_newer_schema() {
        let directory = tempdir().expect("temp directory");
        let live = directory.path().join("live.sqlite3");
        drop(SqliteJobStore::open(&live).expect("initialize live"));
        let newer = directory.path().join("newer.sqlite3");
        drop(SqliteJobStore::open(&newer).expect("initialize newer"));
        let connection = Connection::open(&newer).expect("newer connection");
        connection
            .execute(
                "INSERT INTO schema_migrations(version, applied_unix_ms) VALUES (5, 0)",
                [],
            )
            .expect("newer marker");
        drop(connection);

        let error = MaintenanceService::new(&live)
            .restore_preflight(&newer)
            .expect_err("newer schema must be rejected");
        assert!(error.message.contains("newer than supported"));
    }

    #[test]
    fn automatic_snapshot_retention_keeps_five_including_the_newest() {
        let directory = tempdir().expect("temp directory");
        let database = directory.path().join("jobs.sqlite3");
        drop(SqliteJobStore::open(&database).expect("initialize database"));
        let service = MaintenanceService::new(&database);
        let mut newest = None;
        for _ in 0..7 {
            newest = Some(
                service
                    .create_automatic_snapshot("retention-test")
                    .expect("automatic snapshot")
                    .backup_path,
            );
        }
        let backups = directory
            .path()
            .join("backups")
            .read_dir()
            .expect("backup directory")
            .map(|entry| entry.expect("backup entry").path())
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 5);
        assert!(newest.is_some_and(|path| path.is_file()));
    }

    #[test]
    fn restore_replaces_only_after_preflight_and_keeps_safety_backup() {
        let directory = tempdir().expect("temp directory");
        let live = directory.path().join("live.sqlite3");
        drop(SqliteJobStore::open(&live).expect("initialize live"));
        let source = directory.path().join("source.sqlite3");
        drop(SqliteJobStore::open(&source).expect("initialize source"));
        let source_connection = Connection::open(&source).expect("source connection");
        source_connection
            .execute("PRAGMA user_version = 73", [])
            .expect("tag source");
        drop(source_connection);
        let backup = directory.path().join("restore.sqlite3");
        MaintenanceService::new(&source)
            .backup(&backup)
            .expect("source backup");

        let report = MaintenanceService::new(&live)
            .restore(&backup)
            .expect("restore");
        assert!(
            report
                .safety_backup
                .as_ref()
                .is_some_and(|path| path.is_file())
        );
        let restored = Connection::open(&live).expect("restored connection");
        assert_eq!(
            restored
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .expect("tag"),
            73
        );
    }

    #[test]
    fn corrupt_restore_source_never_changes_live_database() {
        let directory = tempdir().expect("temp directory");
        let live = directory.path().join("live.sqlite3");
        drop(SqliteJobStore::open(&live).expect("initialize live"));
        let before = fs::read(&live).expect("live bytes");
        let corrupt = directory.path().join("corrupt.sqlite3");
        fs::write(&corrupt, b"not sqlite").expect("corrupt source");

        assert!(MaintenanceService::new(&live).restore(&corrupt).is_err());
        assert_eq!(fs::read(&live).expect("live unchanged"), before);
    }

    #[test]
    fn restore_rejects_an_alias_of_the_live_database() {
        let directory = tempdir().expect("temp directory");
        let live = directory.path().join("live.sqlite3");
        drop(SqliteJobStore::open(&live).expect("initialize live"));
        let alias = directory.path().join(".").join("live.sqlite3");

        let error = MaintenanceService::new(&live)
            .restore(&alias)
            .expect_err("live alias cannot be its own restore source");
        assert_eq!(error.code, crate::ErrorCode::OutputConflict);
    }

    fn create_legacy_v2_database(path: &std::path::Path) {
        let connection = Connection::open(path).expect("legacy connection");
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_migrations (
                     version INTEGER PRIMARY KEY,
                     applied_unix_ms INTEGER NOT NULL
                 );
                 CREATE TABLE jobs (
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
                 CREATE TABLE job_events (
                     event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                     job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                     sequence INTEGER NOT NULL,
                     previous_state TEXT,
                     next_state TEXT NOT NULL,
                     code TEXT NOT NULL,
                     timestamp_unix_ms INTEGER NOT NULL,
                     UNIQUE(job_id, sequence)
                 );
                 CREATE TABLE output_reservations (
                     canonical_output_path TEXT PRIMARY KEY,
                     job_id TEXT NOT NULL UNIQUE REFERENCES jobs(id) ON DELETE CASCADE,
                     created_unix_ms INTEGER NOT NULL
                 );
                 INSERT INTO schema_migrations VALUES (1, 0), (2, 0);",
            )
            .expect("legacy schema");
    }
}
