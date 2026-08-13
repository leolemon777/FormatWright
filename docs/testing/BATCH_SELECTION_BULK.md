# Persistent Batch, Selection, and Bulk-Action Evidence

- Status: Gate 1 slice verified on Windows
- Updated: 2026-08-12
- Platform observed: Windows 11 x64 (development)

## Verified contract

SQLite schema v4 persists batch identity, ordinal membership, job idempotency keys, stable selection snapshots, bulk actions, and per-job outcomes. All writes use immediate transactions and retain existing output-reservation rules.

Core regressions prove:

- a three-job batch preserves request order; an internal output collision rolls back the entire second batch;
- identical key/input/Plan/output replay returns one job, while key reuse with a different output returns `POLICY_BLOCKED`;
- a batch/state/search selection retains captured IDs and order after one member changes state;
- bulk retry transitions only Failed/Interrupted/Cancelled members, records `BULK_RETRIED`, and audits every state skip;
- a retry whose normalized output is owned by another active job records an output-conflict skip without changing that job;
- maintenance detects selection member-count and bulk matched-count drift;
- opening a true schema v3 fixture creates one v3 portable snapshot before migration to v4.

The filesystem cleanup hook executes only for a currently eligible job while the same immediate writer transaction prevents another FormatWright process from starting it. Database failure rolls the action back; cleanup is safe to repeat because staging names are deterministic and owned by job ID.

## Surface evidence

CLI now exposes:

~~~text
formatwright jobs batches
formatwright jobs select --state failed --search TEXT
formatwright jobs selection SELECTION_ID
formatwright jobs bulk SELECTION_ID --action retry
~~~

`batch-images` creates a persistent batch and reports `batch_id`. Desktop Jobs provides path filtering plus stable-snapshot bulk Retry, Resume, and Cancel through the same Core service.

A disk-backed CLI E2E under `.artifacts/cli-bulk-e2e-20260812-2128` queued one JSON→YAML job, captured one queued member, bulk-cancelled it, left no output, and reported:

~~~text
member_count=1 matched=1 transitioned=1 skipped_state=0 skipped_conflict=0
schema_version=4 integrity_ok=true output_exists=false
~~~

A second disk-backed CLI E2E under `.artifacts/cli-idempotency-e2e-20260812-2145` submitted the same `--queue-only --idempotency-key e2e:queue:1` request twice. Both responses returned Job `8178614f-835d-4656-8af2-1289d0562ae5`; the database contained exactly one Queued Job at sequence 1. The create/key/Planned→Queued event sequence is one immediate transaction, so a failed enqueue cannot strand a key-bound Planned submission.

## Remaining work

- Desktop batch browser and folder mapping preview.
- Paginated/virtualized query view beyond the current 100-row Desktop projection.
- Selection/action retention and historical audit viewer.
- Multi-process soak/kill injection and full 10,000 mixed-format fairness/latency evidence.
