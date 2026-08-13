# MaintenanceService Evidence

- Status: Gate 1 SQLite maintenance slice verified on Windows
- Updated: 2026-08-12
- Platform observed: Windows 11 x64 (development)

## Contract under test

`formatwright_core::MaintenanceService` is the shared SQLite maintenance boundary for future Desktop and service surfaces. The CLI exposes the same operations:

~~~text
formatwright --state-db PATH maintenance status
formatwright --state-db PATH maintenance backup OUTPUT
formatwright --state-db PATH maintenance integrity-check
formatwright --state-db PATH maintenance restore BACKUP
formatwright --state-db PATH maintenance restore BACKUP --yes
formatwright --state-db PATH maintenance compact
~~~

Restore without `--yes` is preflight-only and does not change live data. Confirmed restore should be run only after queue execution is stopped and other FormatWright processes are closed.

## Direct automated assertions

The fifteen `maintenance::tests` cover:

- healthy schema/status and full integrity;
- a logically damaged migration sequence that still passes SQLite page checks but fails application invariants;
- a validated online backup, SHA-256 output, no overwrite, and no partial/WAL/SHM leakage beside the portable backup;
- an online backup taken while a WAL writer has an uncommitted row; the portable backup contains only the committed snapshot;
- restore preflight of a v2 database, migration of only the temporary copy, and byte-for-byte preservation of the selected source;
- automatic pre-migration snapshots containing schema v2/v3/v4 while the live database reaches current schema v5;
- detection of mismatched/tampered append-only revalidation identity, Plan hash, status, and terminal-job linkage;
- refusal of a newer schema by an older application;
- refusal of a non-SQLite restore source without changing the live file;
- confirmed transactional restore plus a readable pre-restore safety snapshot.
- five-copy automatic retention that always preserves the snapshot just created, plus refusal to restore from a lexical alias of the live database.

Application integrity checks migration continuity, known job states, active/terminal reservation cardinality, event count/latest state, Plan parsing, and deterministic Plan hash. `ApplicationStateService` separately validates presets, settings, engine-registry identities, optional report identity, archive path/type/size, and every payload hash before restore.

## Recorded CLI disk-backed run

A temporary database completed initialization, portable backup, preflight, confirmed restore, JSON integrity check, and compact. Current automated status/preflight assertions report schema v5, including v3→v4 and v4→v5 snapshot coverage. Recursive inspection found no `backup-partial` or `restore-stage` artifact; the live WAL/SHM pair remained expected for the active WAL database.

## Remaining gate work

- Desktop maintenance progress/cancellation UI and cross-version clean-machine bundle restore evidence.
- Add a Desktop maintenance/recovery surface and background cancellation for full check/compact.
- Add explicit multi-process maintenance leasing and concurrent reservation stress.
- Exercise old-version upgrade, failed migration rollback, restore, and downgrade refusal in a clean offline Windows VM.
