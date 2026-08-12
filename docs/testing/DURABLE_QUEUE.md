# Durable 10,000-Job Queue Test

- Status: Phase 3 partial evidence
- Updated: 2026-08-10
- Platform observed: Windows 11 x64

## Claim under test

The Rust control plane can transactionally persist 10,000 jobs and output reservations in a disk-backed SQLite database, close and reopen the database, page every job without hydrating the complete queue, and preserve retry/cancel/resume transitions as ordered events.

This is separate from `QUEUE_BRIDGE.md`, which tests bounded delivery from Rust into a real WebView.

## Automated assertions

`job_store::tests::ten_thousand_jobs_are_created_and_read_in_bounded_pages` proves:

- One transaction inserts 10,000 Jobs, 10,000 initial events, and 10,000 unique output reservations.
- The database is closed and reopened before the read pass.
- `COUNT(*)` returns 10,000 without hydrating records.
- Every ID is recovered exactly once through pages of 137 records.
- No page exceeds its requested bound.
- Planned → queued → cancelled → queued transitions succeed and remain transactional.
- The architecture gate fails if the creation transaction exceeds 30 seconds.

Additional tests prove that two active jobs cannot reserve the same canonical output path, terminal states release reservations, and retry cannot steal another job's reservation.

## Recorded Windows evidence

~~~text
FORMATWRIGHT_QUEUE_BENCHMARK jobs=10000 create_ms=612 page_size=137 paging_ms=1073
test result: ok. 1 passed; finished in 1.73s
~~~

The latest CLI crash sandbox case `sandbox-suite-a23cc63b087140c39c800c0ddb33f160` additionally proves:

- A killed running job recovers to `interrupted`.
- Its staged file is removed without committing the target.
- `jobs resume` appends `JOB_RESUMED` and returns it to `queued`.
- `jobs cancel` appends `USER_CANCELLED`.
- `jobs retry` reacquires the output reservation, appends `JOB_RETRIED`, and returns it to `queued`.

The latest recursive batch run `batch-suite-399229570e534915a0277a326529a6d8` additionally proves that the real resource scheduler can atomically queue its Plans, stop after two commits, leave three jobs queued across CLI processes, and finish those three through `jobs run --parallel 4` after engine and input reinspection via shared `JobExecutionService::run_window`. The 2 GiB reservation budget bounded its peak active image jobs at two. See `BATCH_SANDBOX.md` and `JOB_EXECUTION_SERVICE.md`.

The mixed scheduler run `mixed-scheduler-suite-402efe46745d4aeaa1a1319ea1f0d304` queued nine structured, image, and video jobs through public CLI commands, completed 9/9, observed two simultaneous path-scoped FFmpeg processes, parent RSS 16,125,952 bytes, process-tree RSS 2,325,934,080 bytes, WAL peak 1,285,472 bytes, and no staged remnants. See `MIXED_SCHEDULER.md`.

The opt-in release gate in `TEN_THOUSAND_CONVERSIONS.md` now proves 10,000 distinct structured inputs are planned, atomically queued, reopened, executed in windows of 128, semantically validated, and committed to 10,000 distinct outputs. The recorded Windows release run completed planning in 48.638 seconds and execution in 88.111 seconds. A new atomic `queue_jobs` transition removes the prior per-job transaction bottleneck and rolls the complete enqueue operation back on any invalid ID or state.

## Remaining certification work

- Add desktop pause controls; bounded parallel CLI pause/resume now passes.
- Run and retry a mixed successful/failed media/document distribution across the full 10,000-job corpus; the nine-job mixed and homogeneous 10,000-job gates now pass separately.
- Extend RSS and WAL measurements from the nine-job mixed scheduler gate to the full 10,000-job corpus.
- Exercise output-reservation races from multiple database connections.
- Repeat on macOS and Linux filesystems.
