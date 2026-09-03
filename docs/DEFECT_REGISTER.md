# Anole Defect Register

- Status: Active
- Updated: 2026-08-15
- Execution order source: [`MASTER_EXECUTION_PLAN.md`](MASTER_EXECUTION_PLAN.md)

## Rules

- `Open` means reproduced or supported by direct code-path evidence but not fixed.
- `Fixed` requires implementation plus the named regression evidence.
- `Closed` additionally requires full ordinary CI and affected sandbox gates to pass.
- P0/P1 blocks the next release. A new format or surface never outranks an open P0/P1.
- One fix should normally map to one reviewable commit and must not silently broaden scope.

## Active defects

| ID | Severity | Status | Summary | Primary evidence | Closure evidence |
|---|---|---|---|---|---|
| R-008 | P1 | Fixed | Windows release package contained no conversion engine, while the UI recommended/enabled external-engine routes; the installed app was not out-of-box usable | Release artifact inspection; user reproduction of PDF→PNG/JPG; local Doctor could not run Poppler | Implemented pinned PDF/Media Starter resources, runtime-file hashes, versioned copy/install, atomic active registry, first-start activation, and frontend/backend capability gating. Local Release startup installed both packs; real 15-page PDF→PNG/JPG, GIF, and Core E2E pass; the 2026-08-15 standard-configuration installer (282,440,776 bytes, `a2d38614…b5828`) passed the enhanced real-install smoke including per-pack SBOM/sources sidecar verification, and the Release UI gate now proves PDF→PNG and PDF→JPEG `Pass` through the real interface from per-format isolated processes (`docs/testing/DESKTOP_RELEASE_CONVERSION.md`). Remains Fixed until offline clean-VM UI execution, transitive engine SBOM/license/signature review, upgrade/rollback, and signed-release evidence pass |
| R-009 | P1 | Fixed | Release engine resolution fell back to ambient `PATH` and accepted `.cmd`/`.bat`, allowing broken or unrelated development wrappers to become the selected engine | `doctor.rs` lookup order and Windows executable-extension handling; Doctor selected Codex `override\\pdfinfo.cmd` and version check failed | Implemented explicit `VerifiedPacksOnly`/`Development` policy; Release defaults to verified packs only; Windows pack verification rejects non-native wrappers. Debug full suite, Clippy and Release `production_policy_*` regression tests pass. Remains Fixed until Starter clean-machine E2E closes the affected release gate |
| R-001 | P1 | Closed | Queue control-plane errors could return while other workers remained active in SQLite and were aborted on `JoinSet` drop | `crates/core/src/application/job_execution.rs` report/milestone/prepare/join error paths | `abort_active_window` stops admission, cancels and drains all workers, releases scheduler resources, and persists unfinished jobs as `Interrupted / CONTROL_PLANE_FAILED`. Injected report-storage failure and worker panic tests prove peer drain, recoverable requeue, staged cleanup, and reservation release; ordinary workspace tests and Clippy pass |
| R-002 | P1 | Closed | Desktop preview was not bound to execution/enqueue by approved `plan_hash` | `apps/desktop/src-tauri/src/lib.rs` independently called `prepare_conversion` for preview/run/queue without comparing the result | Run/enqueue must carry the visible preview hash; backend reprepares and calls shared Core `ensure_plan_approved`. Missing approval is `POLICY_BLOCKED`; changed input is `INPUT_CHANGED`; unchanged preview passes; engine identity changes alter the deterministic hash. Core/Desktop tests, all 101 ordinary Rust tests, frontend tests/typecheck/build, and Clippy pass |
| R-003 | P1 | Closed | Immediate conversion committed terminal job state before ValidationReport persistence | `apps/desktop/src-tauri/src/lib.rs` transitioned before `save_report` | Immediate and queued paths now persist reports before terminal transition. Report failure moves an active immediate job to `Interrupted / REPORT_PERSIST_FAILED`; successful return has an already readable report; replacement is atomic and an interrupted replacement backup is recovered. Four Desktop report tests, all 98 ordinary Rust tests, and Clippy pass |
| R-004 | P1 | Closed | Immediate queue pause produced Cancelled jobs without Desktop retry/resume, so a pause was not recoverable in the same surface | `job_execution.rs` Cancelled transition and Jobs UI action set | An admitted delayed worker paused in flight becomes `Interrupted / QUEUE_PAUSED_IMMEDIATE`, leaves no output, is resumed from Desktop to `Queued`, and completes in the next window. Jobs exposes Resume for interrupted/blocked and Retry for failed/cancelled; successful terminal jobs are rejected. Core/Desktop regressions, ordinary workspace tests, frontend tests/typecheck/build, and Clippy pass |
| R-005 | P1 | Closed | Windows reservation identity only lowercased paths and did not normalize/reject Win32 aliases | `crates/core/src/job_store.rs::reservation_key` | Reservation and commit now share the same local output identity resolver: raw components are checked before Win32 trimming; case, `.`/`..`, verbatim disk paths, 8.3 ancestors and deepest existing reparse ancestors normalize together; trailing dot/space, device names, ADS/reserved characters, UNC and device namespaces are rejected. Queue execution rechecks the durable key and blocks retargeted links before worker start. Windows tests cover nonexistent parents, real directory symlinks/retarget, v2→v3 atomic collision rollback, and commit-path parity; ordinary workspace tests and Clippy pass |
| R-006 | P2 | Closed | Cancellation-link task and pause tests did not prove long-lived/in-flight behavior | `JobExecutionService::run_window` spawned a detached linker; pause tests cancelled before execution started | `run_window` now uses structured `tokio::select!` without a detached linker and awaits the queue drain after external cancellation. An isolated runtime runs 64 windows with zero live tasks after every return; admitted finish-current drains exactly one worker while leaving the next queued; admitted external/immediate cancellation is persisted before return. A real Windows parent/descendant PowerShell fixture proves `taskkill /T` prevents delayed descendant writes and partial cleanup completes. Ordinary workspace tests and Clippy pass |
| R-007 | P2 | Closed | Desktop removed the only Job Store for the full queue window, preventing live reads and enqueue | `run_desktop_queue_window` took `state.store`; `require_store` rejected callers | Desktop retains a short-transaction UI connection and opens a dedicated WAL/busy-timeout queue connection. A concurrency regression holds the queue window in its report callback while the UI connection lists live state, reads two bounded pages, and creates/enqueues another job; the original completes and the new job remains queued for the next window. RAII queue-window lease preserves single-runner exclusivity and clears it on every drop/error path; ordinary workspace tests, frontend checks/build, and Clippy pass |
| R-010 | P1 | Closed | PDF raster validation rounded fractional point×DPI dimensions to nearest, but Poppler rounds raster bounds up; correct A4 renders were rejected as 595 px expected versus 596 px observed | Real ST508S PDF→PNG failure: all 15 pages opened and matched format/color, but every width failed `PDF_PAGE_DIMENSIONS` | Validator now uses Poppler ceiling semantics; 36/72/144 DPI observations agree; `poppler_dimensions_round_fractional_pixels_up` and all 94 ordinary Rust tests pass; the same 15-page PDF passes both PNG and JPEG validation; final embedded-Release rebuild succeeds |

## Required fix order

1. R-008/R-009 — establish a self-contained, deterministic Windows conversion vertical slice so real end-to-end verification is possible.
2. R-001 — establish safe error unwinding first.
3. R-003 — make terminal truth and report truth consistent.
4. R-002 — enforce the product's Plan-first approval boundary.
5. R-004 — complete pause/recovery semantics and Desktop controls.
6. R-005 — close Windows output identity collisions before concurrent/bulk expansion.
7. R-006 and R-007 — prove long-lived execution and remove the single-store UI bottleneck.

## Closeout template

For each defect record:

- Fix commit:
- Files changed:
- Regression test names:
- Commands and results:
- Sandbox/release evidence:
- Remaining limitations:
- Documentation/ADR/traceability updates:
