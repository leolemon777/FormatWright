# Implementation Notes

## Current milestone — Windows usable vertical slice, then reliability remediation (2026-08-12)

**Verified baseline**
- Shared `JobExecutionService` is used by CLI and the Desktop durable queue window.
- `QueueWindowControl` exposes finish-current and immediate controls.
- 79 ordinary Rust tests, 6 frontend tests, TypeScript, production build, Rustfmt, Clippy, repository contracts and pnpm production audit pass.
- The repository currently has no first commit; the current snapshot must be preserved before implementation continues.
- The current Windows release package contains no conversion engines. Production discovery falls back to ambient PATH and selected a broken Codex `pdfinfo.cmd`; PDF→PNG/JPG therefore cannot run out of the box.

**Next, in strict order**
1. Create the recoverable Git baseline; `docs/DEFECT_REGISTER.md` tracks R-001–R-009.
2. R-008/R-009: ship/import a verified Windows Starter pack, resolve exact registered paths only in Release, gate UI routes from the same capability snapshot, and prove offline conversion on a clean VM.
3. R-001: cancel/drain workers and reconcile active job state on every control-plane failure.
4. R-003: persist ValidationReport before the terminal state for immediate and queued conversions.
5. R-002: bind execution/enqueue to the user-approved `plan_hash`.
6. R-004: make immediate pause recoverable from Desktop and add retry/resume actions.
7. R-005/R-006/R-007: Windows output identity, in-flight pause/failure injection, cancellation-link task lifetime, and live queue reads.
8. Only then extract full `ConversionService`, `ReportService`, and the minimum `MaintenanceService`.

The authoritative checklist, long-term module design, 12-week route and maintenance cadence live in `docs/MASTER_EXECUTION_PLAN.md`.
