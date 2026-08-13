# ADR-0008: Persist batches, selections, idempotency, and bulk-action outcomes

- Status: Accepted
- Date: 2026-08-12
- Owners: FormatWright maintainers
- Related requirements: FW-FR-030 through FW-FR-034; Gate 1 durable queue

## Context

A long-lived queue cannot model a folder conversion as an ephemeral `Vec<Job>`. Filters can drift while the queue is running, retries can be submitted twice, and a bulk button must not silently act on a different set than the user reviewed. Multiple CLI/Desktop processes also need one durable account of which jobs transitioned or were skipped.

## Decision

SQLite schema v4 adds:

- `batches` and ordinal `batch_members` created atomically with every job, initial event, and output reservation;
- `job_idempotency_keys`, binding a bounded key to one input, deterministic Plan hash, and normalized output identity;
- `selection_snapshots` and ordinal `selection_members`, freezing at most 100,000 IDs from an optional batch, state set, and escaped path search;
- `bulk_actions` and `bulk_action_members`, recording one outcome for every selected job.

Bulk actions are intentionally state-aware:

- Cancel: Planned, Queued, Blocked, or Interrupted to Cancelled.
- Resume: Blocked or Interrupted to Queued.
- Retry: Failed, Cancelled, or Interrupted to Queued.

The action re-reads current state under an immediate SQLite writer transaction. Ineligible jobs are `skipped-state`; a destination reserved by another active job is `skipped-output-conflict`. Eligible staged outputs are cleaned while the same writer lock prevents another FormatWright process from starting that job. Normal job events (`BULK_CANCELLED`, `BULK_RESUMED`, or `BULK_RETRIED`) remain the canonical state history.

CLI and Desktop call the same `BulkJobService`. Desktop filtering is only presentation; every button first persists a selection snapshot and then applies the action to that immutable membership.

## Consequences

- Replaying an identical idempotency key returns the original job; changing its intent fails closed.
- Batch creation and bulk audit are all-or-nothing database transactions.
- Selection membership stays fixed even when current job states change; action eligibility is evaluated at commit time.
- Image batch reports now include a durable `batch_id` and use report schema version 2.
- Schema v3 databases receive a validated automatic snapshot before v4 migration.
- Selection snapshots and bulk audit history currently have no archival retention policy; history compaction is later work.

## Verification

- Atomic batch rollback and ordinal member order.
- Idempotent replay and intent-drift refusal.
- Stable selection order after state mutation.
- Mixed eligible/ineligible bulk retry, output-conflict skip, and event codes.
- Integrity detection for snapshot/action count drift.
- v3 pre-migration snapshot and v4 migration.
- Disk-backed CLI queue → select → bulk cancel → schema/integrity E2E.

## Revisit when

Selection history needs retention/archival, folder mapping becomes a Desktop first-class preview, API clients receive their idempotency contract, or bulk execution needs a resumable filesystem cleanup journal.
