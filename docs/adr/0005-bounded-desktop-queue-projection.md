# ADR-0005: Bounded desktop queue projection

- Status: Accepted
- Date: 2026-08-10
- Owners: Anole maintainers
- Related requirements: FW-FR-030, FW-NFR-002, FW-NFR-003

## Context

The durable job queue may contain at least 10,000 items. Sending a complete hydrated queue after every change would make WebView memory and rendering cost grow with total queue size. UI state must not become a second source of truth beside SQLite.

## Decision

- SQLite and the Rust core own durable job state.
- Rust sends monotonic, bounded queue-delta batches; the initial architecture gate uses at most 250 jobs per batch.
- The WebView rejects duplicate or older batches, coalesces bursts to one paint request, and keeps only a 100-row visible projection.
- Full queue queries use paging; they are not represented as one permanent React object graph.
- Tauri event permissions are limited to the event capability needed by the bridge.

## Consequences

The UI remains responsive and memory-bounded as the durable queue grows. Consumers must handle ordering and replay explicitly. Filtering, selection across pages, and aggregate counts require Rust-side queries instead of scanning browser state.

## Verification

- Rust tests prove 10,000 synthetic jobs produce exactly 40 batches of 250.
- Frontend tests prove a 40-batch burst schedules one render frame, limits the visible projection to 100, and ignores duplicate/out-of-order batches.
- A real Windows Tauri window must complete two consecutive 10,000-job bridge runs and display the final batch and aggregate counts.
- Evidence is recorded in `docs/testing/QUEUE_BRIDGE.md`.

## Revisit when

- Real queue filtering or selection requires more than the bounded projection contract provides.
- Profiling shows the 250/100 limits are poor defaults on a supported platform.
- Tauri event delivery changes semantics or a lower-overhead typed channel becomes available.
