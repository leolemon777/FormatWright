# Implementation Notes

## Current milestone — PR-01 dirty-tree snapshot (2026-08-17)

### Spec Interpretation
- `docs/specs/WINDOWS_DAILY_USE_SPEC_PLAN.md` KD-13: PR-01 is a rollback snapshot of work already in the dirty tree. No Wave-1 behavior.

### Decisions Made
- Snapshot includes certification threading, Gate U host-side negatives, convert-page honesty, `--shell-convert`, NSIS/dev Convert verbs, VOC backlog, and the daily-use spec.
- **Explorer / clean-VM test-contract migration is PR-02, not this snapshot.** `scripts/test_windows_explorer_integration.ps1` and `scripts/test_clean_vm_certification.ps1` still encode Open-in / navigation-only (including the existing `FormatWrightConvert` key-name mistake). Do not flip them to Convert = 1 Job + Pass + source hash here.

### Changes From Spec
- None in this snapshot. PR-01b (pending-clear / pin wanted), PR-03 (success CTA), PR-04 (empty-state cards), PR-05 (`classify_desktop_drop_path`), and PR-06 (800ms ingest) stay later.

### Verification
- Targeted desktop-model, shell parse/validate, and related core/engine-sdk unit tests. See `{SCRATCH}/targeted-tests.log` when the goal runner captures it.

### Risks / Follow-up
- Installed-smoke and CLEAN_VM still assert the old contract until PR-02.

## Previous milestone — HowToConvert simplicity + FileConverter right-click (2026-08-16)

### Spec Interpretation
- Owner wants both: drag-and-drop simplicity and Explorer one-click convert.
- This is not format-count parity. Only already-supported golden-route families get Convert verbs.
- Right-click **Convert to X** is explicit approval, identical to CLI `convert`. Open-in remains review-only. No overwrite; validation still required.

### Decisions Made
- Convert dropdown shows only available routes plus missing-engine routes for this input.
- Quality field only for lossy targets.
- `--shell-convert --to FORMAT PATH` + per-extension Explorer verbs.
- Directory convert requests are rejected.

### Changes From Spec
- USER_GUIDE previously said the shell never starts a conversion. Convert verbs now may start after a named target is chosen.

### Tradeoffs
- Classic Explorer menu only (Windows 11 modern top-level still later).
- Portable `target/release` exe does not get verbs until install or `register_dev_explorer_convert.ps1`.

### Verification
- Frontend target/shell unit tests; desktop Rust parse/validate tests.

### Risks / Follow-up
- Auto-run from the UI after a shell convert still requires a rebuilt desktop binary.
- Need Media/PDF packs for those verbs to succeed.

## Previous milestone — Gate U engine negative matrix (2026-08-16)

### Spec Interpretation
- Gate U requires automated negatives for missing pack, hash tamper, version incompatibility, revoke, half-install, failed upgrade, and malicious PATH.
- `formatwright_compatibility` is a hard install/verify bound, not documentation.

### Decisions Made
- Enforce `[minimum, maximum_exclusive)` against `CARGO_PKG_VERSION` during `verify_engine_pack`.
- Do not mutate process `PATH` (workspace forbids `unsafe`); prove override/PATH losing to a registered pack via a pure `choose_engine_path` helper.

### Changes From Spec
- None. Clean-VM still required to close R-008/R-009.

### Tradeoffs
- Version compare uses dotted numeric prefixes only; `1.0.0-alpha` compares as `1.0.0`.

### Verification
- Targeted engine-sdk / engine_pack / engine_registry / doctor tests and Clippy `-D warnings`.

### Risks / Follow-up
- Host-side negatives are not a substitute for the offline clean VM.

## Previous milestone — certification status threading (2026-08-16)

### Spec Interpretation
- ADR-0011: `Certified` requires a trusted release signature **and** completed human `sources.json` review.
- Hash completeness or `signature_present` must never promote.
- Display trusted-but-incomplete honestly; do not invent a new Plan/Report schema field.

### Decisions Made
- Keep `EngineIdentity.certification` as the only persisted Plan/Report field (schema v1 unchanged).
- Add `SupplyChainReviewStatus` + derive helpers in `engine-sdk`.
- Activation evaluates the compiled-in (currently empty) keyring so Doctor/UI see `Unsigned` instead of “trust not evaluated”.
- Registered pack provenance feeds `inspect_engine` so Planner and reports inherit the same certification.

### Changes From Spec
- None. Official key ceremony still blocked on owner decision.

### Tradeoffs
- Empty embedded keyring makes every current signed-but-unknown key `UnknownKey`. Starter packs are unsigned, so they become `Unsigned`.
- Time-varying trust is **not** hashed into `plan_hash` except via the derived three-state `certification` captured at inspect time.

### Verification
- Targeted Rust + frontend tests listed in `docs/testing/ENGINE_RESOLUTION.md`.

### Risks / Follow-up
- Clean-VM, official key ceremony, and legal review still block R-008/R-009 closure.

## Previous milestone — Windows usable vertical slice, then reliability remediation (2026-08-12)

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
