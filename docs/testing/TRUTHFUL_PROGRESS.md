# Truthful Desktop Progress Evidence

- Status: Phase 4 implementation verified on Windows
- Updated: 2026-08-12

## Contract

Desktop progress is an ephemeral projection of durable Core truth, not a second state machine. Immediate conversions and durable queue windows emit the states they actually commit or enter:

~~~text
Inspecting → Planned → Running → Validating → terminal state
~~~

The queue scheduler also exposes a typed reason when a hydrated job cannot yet be admitted:

- process limit;
- conservative memory budget;
- exclusive engine (for example, the serialized Office worker);
- work-class concurrency limit;
- admission paused;
- duplicate active admission (defensive invariant).

The React view orders updates by durable Job sequence and then event timestamp, ignores an ephemeral update older than the refreshed SQLite sequence, derives elapsed time locally from that stage timestamp, and shows the same live stage in both the status badge and detail line. Repeated actions still use the durable SQLite Job state after a refresh.

## No fabricated precision

`QueueProgressUpdate.eta_milliseconds` is explicitly `null`. Existing adapters expose reliable lifecycle milestones but do not expose a verified total-work counter or stable throughput sample. The UI therefore says that rate/ETA are unavailable instead of mapping states to fixed percentages. The synthetic 10,000-item WebView benchmark remains isolated from real Job progress.

Future engine adapters may populate a rate or ETA only when they provide a typed, bounded, testable progress protocol. Unknown duration must remain Unknown.

## Direct verification

- Scheduler tests distinguish memory, process, class, and exclusive-engine blockers.
- A two-job, one-slot queue test observes a real `Running` stage, a `ProcessLimit` wait for the second Job, and its later `Completed` event; every ETA remains absent.
- Frontend tests reject an older stage update, derive elapsed whole seconds, and assert the absent ETA.
- Desktop Rust check, frontend TypeScript test, and Vite production build pass.
