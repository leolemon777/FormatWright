# ADR-0003: SQLite is the local job source of truth

- Status: Accepted
- Date: 2026-08-10
- Owners: FormatWright maintainers
- Related requirements: FW-FR-030 through FW-FR-034

## Context

FormatWright must recover large batch queues after crashes and must not treat UI memory as durable state. A separate database service would make a local desktop application harder to install and operate.

## Decision

SQLite in WAL mode stores job identity, plan snapshots, step state, engine identity, output reservation, and validation summaries. Large logs, source files, outputs, and report bodies stay in the filesystem.

All state transitions are transactional and validated by a state machine. Read models and queue workers may use separate WAL connections. Each mutation begins an immediate transaction so the SQLite writer is acquired before mutable state/reservations are read; a five-second busy timeout bounds contention, and a timeout is reported as retryable rather than silently replayed.

Output reservations prevent first-party jobs from sharing a normalized destination. Final filesystem publication independently uses a no-clobber move so an external destination created after validation cannot be overwritten by a check/rename race.

Database backup, restore validation, migration snapshots, integrity checks, and compaction are defined by [ADR-0007](0007-transactional-sqlite-maintenance.md).

## Consequences

- Desktop operation requires no database server.
- Schema migrations and downgrade behavior are release-critical.
- Very large queues must be paginated and streamed from the database.
- The future distributed server may implement a separate Postgres adapter without changing domain semantics.

## Verification

- Crash tests terminate the process at every state transition.
- Migration tests cover the previous supported schema.
- A 10,000-job test proves bounded in-memory hydration.
- Barrier-synchronized independent connections prove one reservation winner and one transition/event winner; direct file/directory publish tests prove no-clobber behavior.

## Revisit when

The single-machine product has measured write contention that cannot be solved with batching or the self-hosted distributed worker becomes a committed release.
