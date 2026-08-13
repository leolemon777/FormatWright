# Resource Scheduler Specification

- Status: Normative for Phase 1/3
- Version: 0.1
- Updated: 2026-08-12

## 1. Goals

The scheduler must keep the UI and control plane responsive, bound memory, avoid exhausting temporary disk, and prevent multiple engines from oversubscribing CPU or GPU resources.

## 2. Resource dimensions

Every Plan step declares estimates or unknown values for:

- CPU weight.
- Memory reservation.
- Sequential read/write I/O weight.
- Temporary disk bytes.
- GPU encoder and decoder slots.
- Engine exclusivity key.
- Network requirement, which is zero or denied in v0.1.

Unknown estimates use conservative defaults.

## 3. Work classes

| Class | Examples | Default behavior |
|---|---|---|
| lightweight | Probe, metadata inspection | Higher concurrency, bounded queue |
| io-heavy | Remux, file copy, hashing | Limit per physical disk |
| cpu-heavy | Software video encode, AVIF | Limit by logical CPU budget |
| memory-heavy | Large image/document render | Require explicit memory reservation |
| gpu | Hardware video encode | Limit by detected encoder sessions |
| serial-engine | LibreOffice profile-sensitive work | One task per exclusivity key |

## 4. Initial policy

- In-memory hydrated jobs: maximum 256 by default.
- Runnable process count: maximum 4 by default and never more than half of logical CPUs for cpu-heavy work, rounded up to at least 1.
- Software video encodes: 1 by default.
- LibreOffice conversions: 1 per isolated engine instance until parallel-safety tests pass.
- GPU session count: detected value or 1 when unknown.
- Probe tasks: maximum 8, subject to I/O pressure.

All defaults are configurable within safe caps.

## 5. Disk preflight

Before launch:

- Resolve the destination filesystem.
- Estimate output and temporary bytes with a confidence level.
- Reserve a safety margin of the larger of 1GB or 10% of estimated work.
- Block hard when a declared minimum cannot fit.
- Warn when the estimate is low confidence.

The scheduler rechecks free space before commit.

## 6. Backpressure

- Enumerators write jobs to SQLite in bounded batches.
- The runner pulls only the next scheduling window.
- Progress events are sampled for UI display at most four times per second per active job.
- Durable state changes bypass sampling.
- Log volume has per-job size and rate limits.

## 7. Fairness

- Interactive single-file jobs may receive a bounded priority boost.
- Batch jobs use round-robin fairness across batches.
- The persisted batch ID is the lane; members retain durable ordinal order. Unbatched jobs share one interactive lane ordered by creation time.
- A bounded selection takes the first queued member of every lane before the second member of any lane, with stable creation/ID tie breakers.
- A large batch cannot permanently starve a later interactive job.
- Priority never bypasses disk, security, or engine exclusivity limits.
- Before inspection, a process atomically claims `Queued → Inspecting`; a stale selection that loses ownership is counted as contention and performs no engine work.

## 8. Pause and cancellation

- Pause removes queued jobs from eligibility.
- Finish-current pause lets active steps complete.
- Immediate pause requests cancellation of active steps and restarts them later according to JOB_RECOVERY.md.
- Cancellation tokens propagate from surface to core to process runner.

## 9. Measurement

Scheduler tests record:

- Control-plane RSS.
- Per-engine peak memory.
- CPU utilization.
- Disk throughput and queue latency.
- Time from cancellation request to process-tree termination.
- Number of hydrated jobs.
- Dropped/coalesced display events.

Phase 1 large-file control-plane gates:

- Parent peak working set for a 10 GiB path: at most 160 MiB.
- Parent peak growth from the 1 GiB identity baseline to 10 GiB: at most 32 MiB.
- Engine RSS is measured separately and cannot be hidden inside the control-plane result.
- A sparse fixture is acceptable for the fast architecture gate; release certification also uses a physically allocated sequential-read fixture.

Phase 1 desktop queue-projection gates:

- A synthetic 10,000-job stream is emitted by Rust in no more than 40 batches of 250 jobs.
- The WebView coalesces an event burst into at most one scheduled paint frame.
- The UI keeps no more than a 100-row visible projection; SQLite remains the source of truth for durable jobs.
- Duplicate or out-of-order batches do not regress the projected state.
- A real packaged-development Windows window must finish the benchmark twice without reload, hang, or loss of the final batch.
- This projection gate does not certify persistent creation, pause, resume, or retry of 10,000 SQLite jobs; those remain Phase 3 gates.

Phase 3 durable-queue gates:

- One bounded transaction may create 10,000 jobs, initial events, and output reservations; partial batch insertion is forbidden.
- Closing and reopening the SQLite file must preserve the full count and every job identity.
- Surfaces page jobs with an explicit limit and offset; aggregate counts use SQL rather than hydrated objects.
- Active canonical output reservations are unique, terminal states release them, and retry reacquires them transactionally.
- The fast architecture gate requires the 10,000-row creation transaction to finish within 30 seconds; release baselines record the actual duration, database/WAL size, and RSS.
- `docs/testing/DURABLE_QUEUE.md` owns the reproducible evidence and remaining scheduler certification work.
- `batch-images --pause-after N` and `jobs run --limit N --parallel P` exercise a bounded scheduling window: pause stops admitting queued jobs, resume rechecks engine identity and input fingerprint, and SQLite remains authoritative. `P` is restricted to 1–16 while the deterministic policy independently caps process classes, a 2 GiB reservation budget, GPU slots, and engine exclusivity. `docs/testing/BATCH_SANDBOX.md` and `docs/testing/MIXED_SCHEDULER.md` own the Windows development evidence.
- The opt-in 10,000-real-conversion gate atomically creates and queues 10,000 distinct structured jobs, reopens the database, and executes with no more than 128 hydrated jobs. The Windows release run completed all semantic validations and commits in 88.111 seconds after 48.638 seconds of planning. `docs/testing/TEN_THOUSAND_CONVERSIONS.md` owns the exact evidence and its homogeneous-workload boundary.
- Four independent gated queue processes must reconcile to one engine start/output/report per Job; a real process-tree kill during Running must recover its exact partial and complete after resume. `docs/testing/MULTI_PROCESS_QUEUE.md` owns this Windows evidence.

## 10. Adaptive behavior

v0.1 uses deterministic defaults and explicit configuration. Automatic learning or opaque adaptive scheduling is out of scope. Measured heuristics may be added later through versioned policies and ADRs.
