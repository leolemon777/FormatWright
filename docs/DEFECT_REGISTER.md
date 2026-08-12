# FormatWright Defect Register

- Status: Active
- Updated: 2026-08-12
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
| R-008 | P1 | Open | Windows release package contains no certified conversion engine, while the UI still recommends/enables PDF→PNG/JPG and other external-engine routes; the installed app is therefore not out-of-box usable | Release artifact inspection; Tauri config has no engine resources/external binaries; user reproduction of PDF→PNG/JPG; local Doctor cannot run Poppler | Offline clean-VM install; first launch exposes only actually runnable routes; bundled/imported Starter engine pack completes PDF→PNG/JPG and media smoke tests without system PATH |
| R-009 | P1 | Fixed | Release engine resolution fell back to ambient `PATH` and accepted `.cmd`/`.bat`, allowing broken or unrelated development wrappers to become the selected engine | `doctor.rs` lookup order and Windows executable-extension handling; Doctor selected Codex `override\\pdfinfo.cmd` and version check failed | Implemented explicit `VerifiedPacksOnly`/`Development` policy; Release defaults to verified packs only; Windows pack verification rejects non-native wrappers. Debug full suite, Clippy and Release `production_policy_*` regression tests pass. Remains Fixed until Starter clean-machine E2E closes the affected release gate |
| R-001 | P1 | Open | Queue control-plane errors can return while other workers remain active in SQLite and are aborted on `JoinSet` drop | `crates/core/src/application/job_execution.rs` report/milestone/prepare/join error paths | Failure-injection tests for callback/store/join failures; all workers drained; jobs become recoverable; no leaked reservation |
| R-002 | P1 | Open | Desktop preview is not bound to execution/enqueue by approved `plan_hash` | `apps/desktop/src-tauri/src/lib.rs` independently calls `prepare_conversion` for preview/run/queue | Input/engine change after preview is rejected; unchanged preview executes; CLI/Desktop contract test |
| R-003 | P1 | Open | Immediate conversion commits terminal job state before ValidationReport persistence | `apps/desktop/src-tauri/src/lib.rs` transitions before `save_report` | Read-only/full report directory tests; no Completed/Warning without report; retry can atomically replace/recover report |
| R-004 | P1 | Open | Immediate queue pause produces Cancelled jobs without Desktop retry/resume, so a pause is not recoverable in the same surface | `job_execution.rs` Cancelled transition and Jobs UI action set | In-flight immediate pause test; Desktop retry/resume; next run continues work without CLI; terminology matches state semantics |
| R-005 | P1 | Open | Windows reservation identity only lowercases paths and does not normalize/reject all Win32 aliases | `crates/core/src/job_store.rs::reservation_key` | Table-driven trailing dot/space, reserved device, case, extended path, reparse point and nonexistent-parent collision tests |
| R-006 | P2 | Open | Cancellation-link task and pause tests do not prove long-lived/in-flight behavior | `JobExecutionService::run_window` spawned linker; pause tests cancel before execution starts | Repeated-window task-count/lifecycle test; pause after worker admission; process-tree and partial cleanup assertions |
| R-007 | P2 | Open | Desktop removes the only Job Store for the full queue window, preventing live reads and enqueue | `run_desktop_queue_window` takes `state.store`; `require_store` rejects callers | Queue runs while list/paging/queue-only remain available through actor or read connection; concurrency tests |

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
