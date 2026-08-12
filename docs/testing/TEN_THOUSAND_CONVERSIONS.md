# 10,000 Real Conversion Release Gate

- Status: Phase 3 Windows development evidence
- Updated: 2026-08-10
- Platform observed: Windows 11 x64

## Claim under test

The release-gate test creates 10,000 distinct JSON record-array files, creates 10,000 immutable JSON-to-YAML Plans and unique output reservations in `SQLite`, closes and reopens the database, and executes queued work in windows of at most 128 hydrated jobs. Every item follows Inspect → Plan → Queued → Reinspect → Running → Validating → Completed and produces a distinct validated YAML file through the normal partial-and-commit runner.

This is a real homogeneous structured-data workload, not a synthetic queue projection and not 10,000 references to one input. It complements the control-plane tests in `DURABLE_QUEUE.md` and `QUEUE_BRIDGE.md`.

## Reproduce

~~~powershell
cargo test -p formatwright-core --test ten_thousand_conversions --release -- --ignored --nocapture
~~~

The test is ignored during ordinary CI because it deliberately creates 20,000 files and tens of thousands of durable state events. Release candidates run it explicitly.

## Recorded result

~~~text
FORMATWRIGHT_10000_PROGRESS completed=1000 elapsed_ms=8815
FORMATWRIGHT_10000_PROGRESS completed=5000 elapsed_ms=43634
FORMATWRIGHT_10000_PROGRESS completed=10000 elapsed_ms=88111
FORMATWRIGHT_10000_CONVERSIONS jobs=10000 window=128 planning_ms=48638 execution_ms=88111
test result: ok. 1 passed; finished in 139.65s
~~~

Assertions cover:

- Exactly 10,000 distinct inputs, Plans, jobs, reservations, outputs, and terminal `Completed` states.
- Atomic bulk creation followed by atomic bulk Planned → Queued migration; a unit test proves a missing ID rolls the complete migration back.
- Database close/reopen before scheduling.
- No scheduling window above 128 hydrated jobs.
- Input fingerprint recheck before every execution.
- Required semantic validation is `Pass` for every output.
- Exactly 10,000 `.yaml` outputs and no remaining partial filenames.
- No overwrite path: every output is unique and the normal runner rejects a pre-existing destination.

The first development attempt used top-level JSON objects and was correctly rejected by the GW-11 record-array contract before queue creation. The passing fixture uses valid one-record arrays; the rejection was not weakened to make the benchmark pass.

## Evidence boundary

This satisfies the 10,000-real-file homogeneous batch architecture gate on Windows. It does not yet certify mixed media/document failure and retry distribution, bounded parallel workers, fairness across batches, peak RSS, `SQLite`/WAL growth, physically separate disks, or macOS/Linux filesystem behavior. Those remain release-matrix work.
