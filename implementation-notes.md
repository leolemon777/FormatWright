# Implementation Notes

## Current milestone — Desktop verification session: title-bar drag fix, local engine provisioning, engine guide (2026-09-01)

### Spec Interpretation
- User smoke-tested the merged browser-lane build and reported two gaps: the window could not be dragged after resizing, and the doctor page showed missing engines. Goal: fix the drag bug, provision this machine's engines, and document engine acquisition for the upcoming open-source release.

### Decisions Made
- Title-bar drag fix: Tauri's `data-tauri-drag-region` only fires when the event target is the attributed element itself (no ancestor/child walk). The old markup attributed only the content-width `.c95-window__title` span, so after enlarging the window most of the title bar (the `header` padding area and the SVG icon) was undraggable. A stray `-webkit-app-region: drag` (an Electron-ism, inert in WebView2) had masked the gap in review. Fix: attribute the whole `c95-window__titlebar` header, `pointer-events: none` on the title icon (both themes), `user-select: none` on the titlebar. Window control buttons remain unaffected (no attribute on them), and double-click maximize now works on the whole bar via Tauri's built-in behavior.
- Local engine provisioning (machine config, not repo): Poppler 26.02.0 installed to `E:\DevCaches\poppler-26.02.0` from the same pinned poppler-windows archive as the starter-pack script (sha256 `993e4a…cda5` verified), user PATH extended, and per-engine `FORMATWRIGHT_ENGINE_PDFINFO/PDFTOPPM/PDFTOTEXT/PDFFONTS` overrides set. The overrides matter: Git for Windows ships an Xpdf 4.00 `pdftotext` whose `-v` exits 99, which doctor (correctly) rejects; env overrides outrank PATH so the real Poppler wins regardless of PATH order.
- Debug desktop build: the Windows resource map requires `dist/engine-packs/windows-x86_64/starter/` to exist; an empty directory is a supported degraded state (`bundled_manifest_paths` returns an empty list without `bundle.json`), so no 100+ MB starter-pack download was needed for a system-discovery machine. Release builds must run `prepare/build_windows_starter_pack.ps1` instead.
- README gained an `Engines` section: why nothing is bundled (license/supply-chain, links to engines/README + ADR-0011/0012), the discovery order (pack > `FORMATWRIGHT_ENGINE_*` > PATH > canonical locations), a per-engine acquisition table, and the starter-pack expectation for releases.

### Verification
- Desktop rebuild (`tsc -b` + `vite build` + `tauri build --debug --no-bundle`) green; app relaunched.
- Doctor page re-read via UIA after restart: `msedge` 152.0.4191.53, `pdfinfo`/`pdftoppm`/`pdftotext`/`pdffonts` 26.02.0, `pandoc` 3.8, `ffmpeg`/`ffprobe` 8.1.1 all `✓ 可用`; remaining missing (`soffice`, `qpdf`, `vips`, `heif-convert`) are unwired or optional in v0.1. Browser lane is now fully available on this machine.
- Title-bar drag: fix verified against Tauri's documented drag-region targeting rules; rebuild + relaunch handed to the user for tactile confirmation.

### Risks / Follow-up
- `qpdf`/`vips`/`heif-convert` remain doctor-only inventory entries with no route; keep them documented as optional to avoid "must turn everything green" pressure.
- LibreOffice intentionally not installed yet (user decision pending; ~400 MB, E-drive constraint noted).
- The starter-pack empty-directory workaround must not leak into release builds — release checklist should assert a non-empty `dist/engine-packs` tree.

## Previous milestone — Browser print engine lane: HTML/SVG → vector PDF (2026-08-31)

### Spec Interpretation
- GW-10 names "Pandoc；PDF 引擎可选" for Markdown/HTML → PDF. This milestone fills that open PDF-engine slot with a system-discovered headless Edge print adapter and adds SVG as a new document input, per ADR-0012.
- "可编辑矢量 PDF" is a *validated* product claim, not a marketing one: independent Poppler utilities must prove the text layer and font embedding before commit.

### Decisions Made
- Engine id `msedge`, resolved pack > `FORMATWRIGHT_ENGINE_MSEDGE` > PATH > canonical vendor install locations (`doctor.rs::known_install_location`), the latter three under `Development` policy only. Doctor never launches the browser: version comes from the versioned install directory on Windows, else `unknown`.
- Routing gained a lane concept (`capabilities.rs::route_engine_lanes`): HTML→PDF prefers the browser lane `[msedge, pdfinfo, pdftoppm, pdftotext, pdffonts]` and falls back to the existing Pandoc lane; SVG→PDF is browser-lane only; Markdown→PDF is unchanged. Route availability is satisfied when *any* lane is fully available.
- Plan (`edge_pdf.rs::plan_edge_print_to_pdf`): 5 steps — Edge vector print (`LossClass::None`), pdfinfo structural, pdftoppm render, pdftotext text-layer, pdffonts embedding — with `text_must_remain_extractable` as a plan constraint.
- Execution (`runner.rs::execute_edge_print_plan`): staged workspace with isolated `--user-data-dir`, `--host-resolver-rules=MAP * ~NOTFOUND` as network-deny reinforcement, 180 s print timeout + process-tree termination, `office_staged_work_path` staging, no-clobber commit, scheduler treats `msedge` as `SerialEngine`.
- Validation (`validate_edge_pdf_output`): required `EDGE_PDF_OPENS / PAGE_COUNT / PAGE_SIZES / ALL_PAGES_RENDER / TEXT_EXTRACTABLE / FONTS_EMBEDDED`; non-required Warning `EDGE_PDF_TEXT_FIDELITY` (extracted-vs-input character ratio; extraction loses hyphenation/ligatures so it never blocks).
- SVG inspection: prefix/`<?xml`+`<svg` detection, `image/svg+xml`, any raster `<image>` denied under deny-all (breaks the vector promise), text extracted via the XML reader like HTML.
- `pdffonts` embedding parsed from the right (fixed `emb sub uni object ID` tail); font name from the first token because variable-width `type` values make an exact left split unreliable.

### Changes From Spec
- No manifest template for Edge: `engines/manifests/templates` is for reviewable shipped packs, and Edge cannot be redistributed. Instead: `engines/README.md` inventory row + ADR-0012, mirroring the LibreOffice discovery posture.
- Desktop UI/CLI surfaces unchanged: no new target id (`pdf` exists), capability snapshot picks up the lane automatically; no `PlanRequest` field added, so plan/JSON schemas are untouched.

### Verification
- `cargo check -p formatwright-core --locked` ✓; `cargo clippy -p formatwright-core --all-targets` zero warnings ✓; `cargo fmt --check` clean for every touched file ✓; `cargo test -p formatwright-core --lib` 181 passed (4 pre-existing failures: symlink-privilege `os error 1314` tests in `job_store`/`application`, reproduced independent of this branch) ✓; schema contract suite 9/9 ✓; `scripts/check_repository.py` reports only the pre-existing `capabilities/main.json` allowlist error (present on `main`).
- **End-to-end sandbox evidence (2026-08-31, dev build, Windows)**: `formatwright convert carton.html --to pdf` — a real 291-line HTML/SVG carton-drawing fixture — routed to the browser lane (doctor resolved `msedge` 152.0.4191.53 via canonical install location, Poppler 26.02.0 via PATH), completed in ~10 s with `validation: Pass`. Independent re-inspection of the committed PDF: 1 page at 420×293 mm, 0 raster image objects, 5 embedded font subsets (Arial/Arial-Bold/MicrosoftYaHei±Bold/SimSun), 789 extractable characters including the watermark, barcode digits, and the Chinese company name. The plan hash and every required validator (`EDGE_PDF_OPENS/PAGE_COUNT/PAGE_SIZES/ALL_PAGES_RENDER/TEXT_EXTRACTABLE/FONTS_EMBEDDED`) passed before commit.
- Build environment note: this machine's MSVC 14.51 install lacks the CRT headers; compilation required `LIB`/`INCLUDE` for onecore libs + SDK 10.0.22621.0 (plus the bundled vc15 headers from `SDK/ScopeCppSDK` for `libsqlite3-sys`'s C build — a machine-specific workaround, not a repo change).

### Bug fixed en route (pre-existing, main)
- `document.rs::html_text` ran quick-xml with default `check_end_names`, so any real-world HTML containing void elements (`<meta>`, `<br>`, `<img>`) failed inspection with "Malformed HTML", silently fell through to the ffprobe media branch, and reported "ffprobe could not recognize or open the input". Discovered when the carton fixture (contains `<meta charset="UTF-8">`) misrouted while minimal fixtures passed. Fixed by disabling `check_end_names` for the HTML extractor only (DOCX keeps strict XML matching); regression test `html_with_void_elements_is_still_inspectable` added. This also un-breaks GW-10's existing Pandoc lane for ordinary HTML.

### Risks / Follow-up
- Formal sandbox artifacts (`scripts/test_*_sandbox.ps1` + `docs/testing/*_SANDBOX.md` with a committed fixture and pinned engine identities) still owed before the matrix row drops its evidence caveat; the manual end-to-end run above is the interim evidence.
- Edge print fidelity across browser versions is environment-dependent by design (plan hash embeds engine identity); golden fixtures must pin a browser version or tolerate substitution warnings.
- `--headless=new` requires Edge ≥ 108 (2022); older LTS images may need the legacy `--headless` fallback — decide when a real corpus machine fails.
- `known_install_location` currently special-cases only `msedge`; generalize if another canonical-layout engine (e.g. Chrome, WebView2 runtime) joins the inventory.

## Previous milestone — Chicago 95 desktop chrome (2026-08-18)

### Spec Interpretation
- User asked to restyle the entire desktop UI from `plastic-fly-44-2a81bc35` (Chicago 95). Product behavior stays: Plan, queue, Explorer convert, no new formats.

### Decisions Made
- Vend `system.css` as `apps/desktop/src/chicago95.css`. Strip Google Font `@import` because Tauri CSP is `style-src 'self'`; UI uses Tahoma / MS Sans Serif / Courier New fallbacks offline.
- Main window `decorations: false` with a real Win95 title bar (min/max/close via `core:window:default`).
- Existing convert/jobs/presets/engines/reports/maintenance/settings flows keep their logic; chrome is windows, folder tabs, beveled controls, teal desktop.

### Changes From Spec
- Daily-use spec did not require this visual language. Native OS title bar is gone in desktop/e2e/accessibility window configs.

### Verification
- Desktop vitest + `tsc -b` after markup wrap.

### Risks / Follow-up
- Pixelify Sans / VT323 not bundled; look is workstation-like on Windows, not pixel-perfect vs the marketing preview.
- Accessibility snapshots that assumed a dark modern shell will need a re-run.

## Previous milestone — HowToConvert live-site snapshot (2026-08-18)

### Spec Interpretation
- Docs-only: refresh competitor facts from howtoconvert.co. Do not change product scope or start Wave 5 items.

### Decisions Made
- SPEC_PLAN §1.1/§1.3 record WASM upsell, shell-to-user-installed engines, no Explorer, 5 devices, still-Beta pricing.
- VOC Wave 1 remains the UX steal; Wave 5 now names crop/trim/WASM/PATH-install as forbidden.
- FORMAT_SUPPORT_MATRIX GW-04: Architecture Spike → Experimental on Windows. Certified stays empty.

### Changes From Spec
- None. Plan allowed the matrix status fix as optional; it is included.

### Verification
- Read-back of the three edited sections.

### Risks / Follow-up
- GOLDEN_WORKFLOWS.md GW-04 contract text was not rewritten; TRACEABILITY still says the slice is in progress.

## Previous milestone — Wave 1 ingest / toast / plain copy + testdrive (2026-08-17)

### Spec Interpretation
- Remaining Wave-1: 800ms file-list ingest (PR-06), toast without tray (PR-07), plain-language Plan/errors (PR-08). Then hand a Release testdrive, no commit.

### Decisions Made
- Convert verbs go through `ShellConvertCoordinator` (800ms same-target reset, mix-target flush) and `ingest_shell_convert_paths`. Open-in stays on the old FIFO.
- Toast is a Win32 toast via PowerShell (`show_desktop_toast`). No tray, no keep-alive, no settings schema, no new npm package.
- Basic-mode Plan uses `plainLossSummary`; banners use `basicModeFailureCopy` instead of `route.message`.

### Changes From Spec
- Toast is not `tauri-plugin-notification` (avoids a lockfile/plugin surface for testdrive). Click-to-focus is “show main window” after the toast command.

### Verification
- desktopModel + shell_convert + ingest unit tests; Release desktop sequential launch; two CLI JSON→YAML converts.

### Risks / Follow-up
- Installed NSIS smoke not re-run. R-008/R-009 still not Closed.

## Previous milestone — Wave 1 daily-use (PR-01b through PR-05 + PR-02) (2026-08-17)

### Spec Interpretation
- `WINDOWS_DAILY_USE_SPEC_PLAN.md` Wave 1 exit: PR-02 + PR-03 + PR-05, recommended PR-04, plus PR-01b pending pin.
- Convert to X still uses the existing `pendingShellConvert` preview+run effect (PR-06 ingest is later). PR-01b only stops that pending from leaking across user edits and capability auto-target.

### Decisions Made
- `defaultPlanConstraints` resets quality/width/dpi/colorMode to null on new input and shell convert.
- Capability snapshot keeps a pending wanted target; unavailable wanted clears pending and does not jump to `firstRecommended`.
- Success stays on Convert; `setTab("reports")` remains only for explicit report browsing.
- Empty-state cards probe `C:\formatwright-probe.pdf` / `.mkv` through the existing snapshot command (extension-only).
- Drop folders go through `classify_desktop_drop_path` (same local-disk rules as shell).
- Explorer verbs come from `explorer-verbs.json` via `scripts/generate_explorer_verbs.ps1`.

### Changes From Spec
- PR-06 800ms ingest / `ingest_shell_convert_paths` not implemented. N=1 Explorer convert still uses the frontend effect.
- Installed Explorer convert smoke is in the script contract but was not executed here (needs a fresh NSIS build + isolated install).

### Verification
- Targeted desktopModel vitest, desktop `shell_` / classify Rust tests, `generate_explorer_verbs.ps1 -Check`.

### Risks / Follow-up
- PR-06 still required to merge Explorer multi-select and to honor queue-window busy.
- Do not mark R-008/R-009 Closed without a clean VM.

## Previous milestone — PR-01 dirty-tree snapshot (2026-08-17)

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
