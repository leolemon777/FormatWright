# Application State Bundle Evidence

- Status: Implemented development evidence
- Updated: 2026-08-12

`ApplicationStateService` creates a versioned `.fwstate` ZIP containing an online SQLite snapshot plus optional `presets.json`, persisted `settings.json`, engine-registry identity JSON, and optionally ValidationReports. The public v1 manifest lists the exact member set, component, byte count, and SHA-256. Third-party engine binaries are excluded.

Creation uses a same-directory unique partial, flushes the ZIP, then reopens and fully preflights the completed partial before no-clobber publication. Restore extracts only regular declared members into a bounded same-volume staging directory, validates their hashes/contracts, and runs `MaintenanceService::restore_preflight` against staged SQLite before touching live state.

Confirmed restore first creates a full safety bundle and SQLite safety copy. A durable journal records each file/directory swap and whether the database switch started. Failure rolls swaps back; CLI and Desktop startup call `recover_interrupted_restore` before opening SQLite. The safety bundle remains after success for operator rollback.

Core tests cover:

- full SQLite/presets/settings/registry/reports round trip with pre/post job-count proof;
- restore onto a new machine with no existing application-state database;
- tampered-member SHA-256 refusal before restore;
- unsafe traversal path refusal and orphan report/SQLite reference rejection;
- simulated interrupted file switch recovered from the journal;
- committed restore with interrupted cleanup resumed without rolling live state back.

The real disk-backed CLI exercise is recorded in `.artifacts/application-state-cli-e2e-20260812-2242`. It performed a real JSON→YAML conversion, bundled its SQLite/report state, passed bundle preflight, restored with `--yes`, retained a safety bundle, and passed post-restore full database integrity. `bundle_id` matched backup, preflight, and restore output.

The separate new-machine exercise in `.artifacts/application-state-new-machine-e2e-20260812-2306` restored that same class of SQLite/report bundle to a destination with no prior database. It created the destination, restored exactly one Job and its report, passed integrity, and correctly returned no pre-restore safety bundle because no prior state existed.

This closes the Gate 1 portable application-state implementation slice. Clean-VM cross-version upgrade/downgrade refusal, Desktop maintenance UI, cancellation/progress, and release-signing evidence remain open.
