# Multi-Process Queue and Crash-Recovery Evidence

- Status: Gate 1 verified on Windows development binary
- Updated: 2026-08-12
- Platform observed: Windows 11 x64

## Contract under test

Independent CLI processes may open the same WAL database, but a queued Job has exactly one execution owner. `SqliteJobStore::claim_queued_job` acquires an immediate writer transaction and changes `Queued → Inspecting` with its event in that transaction. A process that selected the same row but lost the claim records the item as `contended`; it does not inspect, launch an engine, validate, publish, or mark the Job failed.

Bounded queue windows are selected with durable round-robin lanes. Each persisted batch is one lane ordered by member ordinal; jobs without a batch share one interactive lane ordered by creation time. Selection takes rank 1 from every lane before rank 2, then applies deterministic creation/ID tie breakers. Resource admission remains a second independent bound after fair selection.

## Reproducible gates

~~~powershell
pwsh -File scripts/test_multi_process_queue.ps1
pwsh -File scripts/test_queue_crash_recovery.ps1
~~~

`test_multi_process_queue.ps1` first launches several independent queue-only commands with one idempotency key and verifies that every process receives the same Job ID and SQLite contains one Job. It then starts four queue runners behind a test-only start gate so every runner observes the same 24 queued rows. Assertions require:

- 24 total completions and 72 losing claims reported as `contended`;
- exactly 24 `ENGINE_STARTED` events, outputs, and reports;
- zero failed Jobs and zero staged-output remnants;
- every final Job state is `Completed`.

`test_queue_crash_recovery.ps1` creates a real two-minute media source, waits until the Job is durably `Running` and its partial contains bytes, verifies the exact runner executable identity, and force-terminates that process tree. It then requires recovery to mark exactly one Job interrupted and remove exactly one owned partial. Resume must execute and validate the Job once, preserve the source hash, create its report, and leave no partial.

The `--start-gate` argument is hidden and test-only. It only aligns queue selection timing; all ownership safety comes from the SQLite claim transaction.

## Recorded Windows evidence

- `multi-process-queue-suite-c18914264e4f423a9f845499a305e68d`: four runners, 24 Jobs, 96 selections, 24 completions, 72 contentions, 24 engine starts, 24 outputs, 24 reports, zero failures/partials.
- `multi-process-queue-suite-be83bfaa62a646d6898760d3e68959ef`: the repository gate plus four concurrent idempotency submissions; one unique Job, then the same 24/72/24 exact-once reconciliation.
- Short soak `multi-process-queue-suite-88f86439537442d0be781cdfef48b73d`, `multi-process-queue-suite-14ea8f11507c4f8c96a0ae82b2b3f4b2`, and `multi-process-queue-suite-4503ca8b082a45b7b239427ce09ad819`: three consecutive 32-Job/four-runner passes; each produced 32 completions, 96 contentions, 32 engine starts, zero failures, and one unique idempotent Job.
- `queue-crash-recovery-suite-8c69c12d2c27446da670f553a8bce9e1`: killed while Running with a non-empty partial; one interrupted Job and one partial recovered; resumed Job completed with unchanged input and independent `ffprobe` success.

Generated evidence stays under `.artifacts/` and is excluded from Git.

## Scope boundary

This closes the Windows multi-process exact-once claim and one real kill/restart development gate. It is not a distributed scheduler lease: SQLite still targets one local machine, active maintenance remains separately coordinated, and a process paused after claiming is recovered on a later startup. Repeated long-duration power-loss campaigns, macOS/Linux filesystems, full 10,000-item mixed media/document load, latency percentiles, and peak RSS/WAL certification remain open.
