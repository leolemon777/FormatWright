# ADR-0007: Validate SQLite maintenance on isolated copies

- Status: Accepted
- Date: 2026-08-12
- Owners: Anole maintainers
- Related requirements: FW-FR-030 through FW-FR-034; long-term operations §20.1

## Context

SQLite is the durable source of truth for jobs, Plans, events, and output reservations. A long-lived local application needs backups and upgrades that remain consistent while WAL readers or writers exist. Copying only the main database file, clearing state after a read error, or testing a restore against the live database would violate the recovery contract.

## Decision

`MaintenanceService` owns database status, full integrity checking, online backup, restore preflight, restore, compaction, and automatic migration snapshots.

- Backups use SQLite's Online Backup API, write to a unique same-directory partial, convert the result to portable rollback-journal mode, run full validation, flush it, and only then rename it to the requested non-overwriting destination.
- Full validation combines `PRAGMA integrity_check`, `PRAGMA foreign_key_check`, contiguous migration markers, known states, reservation cardinality, event sequence/latest-state agreement, Plan JSON parsing, and deterministic Plan-hash verification.
- Restore first copies the selected backup into a temporary database, rejects schemas newer than the running release, migrates the temporary copy, and validates it. The live database is not changed during this preflight.
- An explicitly confirmed restore creates a validated safety snapshot, then uses SQLite's transactional backup mechanism to replace the live database from the validated temporary copy. A post-switch validation failure triggers a best-effort rollback from the safety snapshot.
- Before a disk-backed schema migration, the current database is snapshotted. Automatic snapshots share a default retention limit of five, and the snapshot just created is never pruned by timestamp ties.
- Online backup/restore lock acquisition is bounded to 30 seconds. Restore callers must stop queue execution and close other Anole processes.
- `compact` is explicit and creates a safety snapshot before `VACUUM`.

The implementation follows the [SQLite Online Backup API](https://sqlite.org/backup.html) and uses the checks defined by [SQLite PRAGMA integrity_check and foreign_key_check](https://sqlite.org/pragma.html#pragma_integrity_check).

## Consequences

- WAL activity is included in a consistent backup instead of depending on sidecar-file copying.
- Corrupt, logically inconsistent, or too-new backups cannot reach the live switch.
- Manual backup destinations are never silently overwritten and contain no temporary WAL/SHM sidecars.
- Migration safety is enforced by the same Core path for CLI and Desktop database opens.
- SQLite backup remains independently usable. The later `ApplicationStateService` bundle composes this online snapshot with presets, settings, engine registry identity, and optional reports under a hashed manifest and recovery journal.
- Full integrity checks and compaction are synchronous. A cancellable Desktop maintenance workflow remains follow-up work.

## Verification

- Healthy status/integrity and logical-corruption detection.
- Validated non-overwriting online backup without partial/sidecar leakage.
- v2 restore preflight migrates only a temporary copy; the source bytes remain unchanged.
- v2 normal open creates a pre-migration snapshot before current migrations are applied; a true v3 fixture is also snapshotted before v4 batch/selection migration.
- Newer schema and corrupt restore sources are rejected without changing the live database.
- Confirmed restore preserves a safety backup and transactionally installs the validated source.
- CLI status, backup, preflight, confirmed restore, integrity JSON, and compact are exercised against a temporary disk-backed database.

## Revisit when

Application-state bundles add presets/settings/registry/reports, maintenance moves to a background cancellable task, or multi-process server mode requires an explicit cross-process maintenance lease.
