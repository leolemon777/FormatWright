# SQLite Concurrency and No-Clobber Commit Evidence

- Status: Gate 1 multi-connection and multi-process execution slice verified on Windows
- Updated: 2026-08-12
- Platform observed: Windows 11 x64 (development)

## Write coordination contract

Every `SqliteJobStore` mutation starts an immediate transaction. The writer lock is therefore acquired before reading mutable job/reservation state, instead of upgrading a deferred read after another connection may have committed. Connections retain the five-second SQLite busy timeout.

- `create_jobs` owns job rows, initial events, and output reservations in one transaction.
- `queue_jobs` and `transition` read state, compare sequence/state, write the event, and change reservations under the same immediate writer transaction.
- The v3 output-identity migration also acquires the writer before reading and rebuilding active reservations.
- A lock timeout is a retryable `STORAGE_FAILED`; an already-owned normalized destination is the stable non-retryable `OUTPUT_CONFLICT` result.

Two barrier-synchronized disk-backed tests open independent SQLite connections:

- `concurrent_connections_cannot_reserve_the_same_output`: exactly one connection creates the job; the loser receives `OUTPUT_CONFLICT`; one job remains and the full maintenance integrity check passes.
- `concurrent_transitions_commit_only_one_event_sequence`: two writers race `Planned → Running`; exactly one succeeds, and the durable job has sequence 1 with exactly one `CONCURRENT_START` event.

The process-level extension starts four gated CLI runners against the same WAL database. Every process deliberately selects the same 24 rows, yet the immediate-transaction queue claim produces 24 winners, 72 reported contentions, exactly 24 engine starts/outputs/reports, and zero failures or partials. Concurrent process-level idempotency replay also resolves to one Job. See `MULTI_PROCESS_QUEUE.md`.

## Destination publish contract

The final filesystem publish no longer relies on `exists()` followed by platform-default `rename`. All media, Office PDF, PDF page-directory, HEIC, DOCX, and structured outputs use one no-clobber persistence helper. The helper uses the platform implementation behind `TempPath::persist_noclobber`; a destination created after the earlier check produces `OUTPUT_CONFLICT`, never replacement. The caller removes its own deterministic stage after a conflict.

Direct tests prove:

- an existing file keeps its original bytes and the validated source is not published;
- an existing non-empty page directory and marker remain unchanged;
- an absent file destination receives the validated bytes and the staging name disappears.

A disk-backed CLI JSON→YAML run also completed with Validation `Pass`; a second conversion to the same destination exited non-zero and the first YAML bytes remained unchanged.

This closes the application-level check/rename race for local filesystems. Unsupported filesystem primitives fail closed as `STORAGE_FAILED`. Network destinations are already outside the local-path policy; removable/legacy filesystem campaigns remain Gate 3 work.

Schema v4 batch creation, selection capture, and bulk actions follow the same immediate-writer rule. Bulk stage cleanup runs only after current-state and reservation-conflict checks while that lock is held, preventing a second FormatWright process from admitting the same job between check and cleanup.

## Remaining work

- Longer power-loss/reboot soak beyond the verified process-tree force-kill/restart case.
- A real worker hook that creates the destination precisely between validation and publish, in addition to the direct publish primitive tests.
- 10k mixed-format concurrency, latency, WAL, RSS, and fairness evidence.
