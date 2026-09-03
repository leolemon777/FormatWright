# ADR-0009: Centralize conversion and report lifecycle in Core

- Status: Accepted
- Date: 2026-08-12
- Owners: Anole maintainers
- Related requirements: FW-FR-020 through FW-FR-034; R-002/R-003

## Context

CLI and Desktop shared low-level planners and runners but still carried separate orchestration. CLI duplicated the full `prepare_conversion` route table, Desktop owned the only recoverable report writer, and some CLI success paths could reach terminal SQLite state without persisting a report. That is unsafe for a long-lived product and would multiply again for API/MCP.

## Decision

- `workflow::prepare_conversion` is the only first-party route planner. The duplicate CLI implementation is removed.
- `ConversionService` owns immediate conversion: exact Plan approval, durable Job creation, Running/Validating milestones, execution, report-before-terminal persistence, terminal transition, and normalized Failed/Cancelled recovery.
- `ReportService` owns bounded report serialization/read, unique partial files, recoverable same-directory backup replacement, Job-ID validation, and `REPORT_PERSIST_FAILED → Interrupted` recovery.
- CLI and Desktop immediate conversion call `ConversionService`.
- CLI/Desktop queue windows persist reports through `ReportService` before `JobExecutionService` writes terminal state.
- CLI `batch-images` persists every produced report before terminal state.
- CLI report storage is the `reports/` sibling of its selected state database. Desktop keeps its application-data `reports/` directory.

## Consequences

- A successful first-party immediate, queued, or image-batch job has a durable report before its terminal state.
- CLI and Desktop produce the same Plan hash and state-event semantics for immediate conversion.
- Report reads reject malformed, oversized (>16 MiB), or cross-Job files instead of hydrating unbounded/corrupt content.
- The report file and SQLite terminal transition are not one filesystem transaction. The ordered recovery contract is: report first; if report storage fails, active state becomes Interrupted; a report may exist before a later SQLite transition failure and is safe to replace on retry.
- Report export/redaction variants now use a bounded no-overwrite boundary. Validation-only reuses the immutable Plan and format validators, records append-only evidence in SQLite schema v5, and does not replace the original conversion report or terminal state.

## Verification

- Real structured conversion through `ConversionService` reaches Completed with matching stored report.
- Stale approval creates no Job and no output.
- Report replacement/recovery leaves one active report.
- Cross-Job report read is rejected.
- Report storage failure leaves the validating Job Interrupted.
- Disk CLI immediate and queue-window E2E each prove matching Job/output/report/terminal event.

## Implementation update

Validation-only and safe report/recipe export shipped on 2026-08-12. Original report files remain the immutable conversion record. Revalidation reports are an append-only SQLite audit: application-state bundles therefore always include them as part of the database, while inclusion of original report files remains optional.

## Revisit when

A remote API needs streaming report persistence, full revalidation-history browsing becomes a product requirement, or report indexing must move beyond the append-only audit table.
