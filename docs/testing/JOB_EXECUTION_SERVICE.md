# Shared JobExecutionService Evidence

- Status: Gate 1 shared execution and R-001 failure-unwind evidence
- Updated: 2026-08-11
- Platform observed: Windows 11 x64 (development)

## Claim under test

The durable queue scheduling loop that previously lived only in the CLI now executes through `formatwright_core::JobExecutionService::run_window`. CLI `jobs run` retains argument parsing, Ctrl+C cancellation wiring, and JSON/text report printing. Core does not depend on Clap, Tauri, React, or Axum.

## Contract preserved

- `--parallel` rejected outside 1–16
- `--limit` rejected above 256
- Default `SchedulerPolicy::bounded` memory budget remains 2 GiB
- Queued jobs recheck engine identity and input fingerprint before Running
- Cancellation stops new admissions; active workers receive the same token
- Central control plane persists `QUEUE_REINSPECTING`, `ENGINE_FINISHED`, `VALIDATION_FINISHED`, `QUEUE_CANCELLED`, and related events
- `QueueRunReport` schema_version 1 fields are unchanged for CLI JSON output
- Any prepare, milestone, report callback, terminal transition, or worker-join failure first stops admission, cancels the shared worker token, drains the complete `JoinSet`, releases scheduler resources, and persists unfinished active jobs as recoverable `Interrupted / CONTROL_PLANE_FAILED` before returning the original error
- A cleanup persistence failure is attached to the original diagnostic after worker drain; the control plane does not hide the initiating failure

## Automated assertions

`application::job_execution::tests` proves:

- Invalid parallelism and oversized windows return `InputInvalid`
- A structured JSON→YAML queued job completes with `peak_active == 1` and a second window can run after resource release
- `run_window_observed` invokes the report callback before the terminal SQLite transition and can persist a report file
- `QueueWindowControl::pause_finish_current` leaves hydrated jobs `queued` without cancelling
- `QueueWindowControl::pause_immediate` stops admission without mutating unstarted queued jobs; an admitted delayed worker cancelled in flight becomes recoverable `Interrupted / QUEUE_PAUSED_IMMEDIATE`, leaves no output, and completes after requeue in the next window
- A blocked stale-fingerprint job releases its scheduler slot so a later valid job in the same window can complete
- A plan with no steps transitions to `Failed` via `PLAN_INVALID`
- An injected report-storage callback failure with two admitted workers cancels and drains the peer, marks both jobs `Interrupted / CONTROL_PLANE_FAILED`, leaves no staged output, permits requeue, and releases reservations after terminal cleanup
- An injected worker panic with an admitted delayed peer returns `Internal` only after peer drain and leaves both jobs recoverable rather than silently aborting the `JoinSet`

## Sandbox continuity

Post-extraction Windows reruns (pwsh):

- Batch: `batch-suite-399229570e534915a0277a326529a6d8` — pause 2/3, resume selected 3 completed 3, parallelism 4, peak_active 2, outputs 5, staged 0
- Mixed: `mixed-scheduler-suite-402efe46745d4aeaa1a1319ea1f0d304` — 9/9/9, peak_active 4, FFmpeg processes 2, parent RSS 16,125,952, tree RSS 2,325,934,080, WAL peak 1,285,472, staged 0

## Desktop wiring

As of 2026-08-11 the Tauri surface also calls `JobExecutionService`:

- `queue_desktop_conversion` → Planned → Queued
- `run_desktop_queue_window` → exclusive store take + `run_window_observed` (persists `reports/{job_id}.json` before terminal transition)
- `pause_desktop_queue_window` with `finish-current` or `immediate` via `QueueWindowControl`
- `cancel_desktop_queue_window` → alias for immediate pause
- `requeue_desktop_job` → Resume for `Interrupted`/`Blocked`, Retry for `Failed`/`Cancelled`; other states are rejected

Known gaps: recovery banner and bulk retry UI are not implemented.

## Pause semantics

- **finish-current**: cancel admission only; hydrated but not-yet-admitted jobs remain `queued`; active workers finish.
- **immediate**: cancel admission and active workers; unstarted hydrated jobs remain `queued`; cancelled actives become `Interrupted / QUEUE_PAUSED_IMMEDIATE`, retain their output reservation, and may be resumed from Desktop for the next window.

## Remaining work

- Desktop recovery banner and bulk retry (single-job Resume/Retry is implemented)
- ConversionService extraction / remove CLI duplicate `prepare_conversion`
- Multi-connection reservation races and 10k mixed workload certification
- Cancellation-link task lifecycle and real subprocess in-flight pause evidence (R-006)
