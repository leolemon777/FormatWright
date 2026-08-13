# ADR-0010: Versioned Application-State Bundles

- Status: Accepted
- Date: 2026-08-12

## Context

SQLite online backup alone cannot migrate a long-lived FormatWright installation. Presets, UI settings, engine-registry identities, and ValidationReports live beside the database. Copying that directory while WAL writes or atomic file replacements are active can produce a mixed snapshot, and extracting an untrusted ZIP directly into application data creates traversal, link, size-amplification, overwrite, and partial-restore risks.

## Decision

`ApplicationStateService` owns `.fwstate` backup, preflight, restore, and interrupted-restore recovery.

- Bundle v1 is a ZIP with a public manifest. Every declared payload has a fixed component, exact byte count, and SHA-256. Undeclared, duplicate, unsafe, nested, non-regular, oversized, or hash-mismatched members are rejected.
- SQLite is captured through `MaintenanceService` online backup. A just-created bundle is reopened and fully preflighted before no-clobber publication.
- Presets, settings, registry identities, and reports are validated against bounded typed contracts. Reports must reference a Job and matching Plan hash in bundled SQLite.
- Reports are opt-in. Restoring a bundle without reports clears live reports so they cannot silently mismatch the restored database; the pre-restore safety bundle preserves them.
- Engine registry identity/path records are portable metadata; third-party engine binaries are not copied blindly.
- Restore extracts to a same-volume temporary directory, completes all validation and SQLite migration preflight, then creates a full safety bundle and database safety copy before switching live components.
- A durable journal records each swap, database-switch intent, and final commit. Startup rolls an uncommitted switch back or completes cleanup for a committed switch. A bundle can restore onto a machine with no existing state.
- Desktop settings move from WebView-only local storage to a versioned backend file. Existing language/expert preferences migrate once.

## Consequences

- Backup and restore remain local and network-free, have explicit limits, and never overwrite a selected bundle destination.
- The recovery protocol can return to a consistent old state after interruption and retains an operator-visible safety bundle after successful replacement of existing state.
- Full bundle restore must run without queue workers or other FormatWright processes; a future server mode needs a cross-process maintenance lease.
- Missing third-party engine paths require re-import on a new machine.

## Evidence

Core tests cover full component/content round-trip, new-machine restore, member tampering, traversal, orphan reports, uncommitted rollback, and committed cleanup continuation. The disk-backed CLI exercise is recorded in `docs/testing/APPLICATION_STATE_BUNDLE.md`.

## Revisit when

Bundles are signed/encrypted, secrets enter settings, report storage moves into SQLite, remote backups stream without local seek, or multi-process service mode ships.
