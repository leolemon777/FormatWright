# Recursive Image Batch Sandbox Tests

- Status: Phase 3 Windows development evidence
- Updated: 2026-08-10
- Workflow: GW-03

## Scope

`scripts/test_batch_sandbox.ps1` builds a three-level image tree and exercises recursive enumeration, directory-symlink refusal, deterministic output naming, transactional reservation, persistent queued states, bounded scheduling, pause, resume through `jobs run` (delegating to shared `JobExecutionService::run_window`), reinspection, validation, and recovery-safe staged commits.

The passing fixture promotes GW-03 to Experimental on Windows. The CLI uses deterministic bounded concurrency with process, CPU, memory, I/O, GPU, and engine-exclusivity limits; SQLite remains the sole durable authority.

## Covered assertions

- Five images across three directory levels are planned while a text file and directory junction cycle are skipped.
- Relative Unicode directory structure is preserved.
- Duplicate stems with different input extensions receive deterministic distinct target names.
- All five output paths and Plans are persisted before the first execution starts. Planned → Queued now uses the same atomic bulk transition exercised by the 10,000-file release gate.
- `--pause-after 2` completes two jobs and leaves three durable jobs queued.
- A separate `jobs run --limit 100 --parallel 4` process selects and completes those three queued jobs. The 2 GiB reservation budget limits the three 1 GiB CPU-heavy image jobs to a peak of two active workers.
- Final completed/job/output counts reconcile at five.
- Every output independently decodes as 48×32 WebP.
- Existing batch outputs block a new batch under the no-overwrite policy.
- A source changed after queueing becomes Blocked during reinspection and produces no output.
- No staged outputs remain.

## Run

~~~powershell
cargo build -p formatwright-cli
pwsh -NoProfile -File scripts/test_batch_sandbox.ps1
~~~

The latest recorded Windows run `batch-suite-399229570e534915a0277a326529a6d8` (2026-08-11, after `JobExecutionService` extraction) passed all assertions: discovered 7, planned 5, skipped 2, paused at 2 completed/3 queued, resumed 3/3 with configured parallelism 4 and peak active 2, and committed 5 validated outputs. Prior development evidence: `batch-suite-1cab33186778486ea19c5803a3a44c67`.

## Current pause/resume semantics

- Pausing stops admission of new queued jobs after the requested boundary.
- Already committed jobs remain terminal and are not repeated.
- Remaining jobs retain immutable Plans and output reservations in SQLite.
- `jobs run` rechecks engine identity and input fingerprint before transitioning queued → inspecting → planned → running.
- Ctrl+C cancels the active process tree and stops further scheduling; untouched jobs remain queued.

## Remaining certification work

- Interactive desktop pause controls and finish-current versus immediate-pause selection.
- 10,000 actual mixed image conversions with fairness and queue-latency measurements; the nine-job mixed RSS/WAL gate now passes.
- Resume after an actual process crash inside this exact batch fixture; common forced-crash recovery is already covered separately.
- Multi-connection reservation races and macOS/Linux filesystem runs.
