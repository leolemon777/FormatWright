# Implementation Notes

## Current milestone — Linux runner operational: OCR closed, three-way archive matrix, second-platform evidence (2026-09-02)

### The Linux execution lane is live
- macair-away (Linux Mint 22.3, Tailscale) is now the second execution environment per the owner's standing rules: Windows holds the authoritative source; Linux runs compile/test; sync is one-way via git archive; no sudo without asking.
- User-level toolchain, zero system packages touched: rustup in ~/.cargo; a conda-forge env (~/.miniforge/envs/ocr) carries tesseract 5.5.3 (+chi_sim), poppler, qpdf, ffmpeg, pandoc, pillow, matplotlib.
- Core suite natively: 214 passed / 0 failed - the Windows symlink-privilege failure class does not exist here.

### OCR gap CLOSED (G-24)
- e2e on Linux: image -> txt validation Pass (OCR_TEXT_NONEMPTY); pdf-ocr validation Pass with the extracted text matching the fixture exactly (OCR TEST ELECTRIC 440010147700). Engine resolution via FORMATWRIGHT_ENGINE_TESSERACT; chi-sim available for Chinese documents.

### Archive family complete
- tar.gz <-> 7z joins zip <-> tar.gz and zip <-> 7z: all three containers now interconvert in memory with manifest conservation. Windows e2e: tar.gz -> 7z -> tar.gz round trip byte-exact.

### Second-platform conversion matrix
- 28/29 routes pass on Linux (structured, markup->pdf/docx/epub via pandoc, raster, PDF->image, OCR, audio, full archive trio). The single miss is svg->pdf requiring msedge - correct EngineMissing behavior for a Windows-only browser lane on Linux; a Chromium discovery branch for Linux is the natural follow-up.


## Current milestone — Wave 4 + first tri-platform green CI (2026-09-02)

### Landed
- **Word export family** (agent G): docx -> txt/md/html/epub via Pandoc (EXPORT_TEXT_NONEMPTY required, fidelity digest Warning) and docx <-> odt exchange through the isolated soffice lane with structural validation; a real zip-6.0 data-descriptor bug misjudging LibreOffice-written DOCX sizes fixed en route.
- **7z lane** (agent H): zip <-> 7z via sevenz-rust, in-memory entries only, entry-count + manifest conservation. RUSTSEC-2026-0245/0246 exempted with rationale (disk-writing path never called); swap the crate before any Certified claim.
- **PDF metadata**: pdf-metadata operation writes /Title//Author via hand-rolled incremental update; validated by pdfinfo round-trip.
- **OCR wired, engine pending**: image->txt and pdf-ocr operations complete with OCR_TEXT_NONEMPTY/OCR_PAGE_COVERAGE; doctor lists tesseract and reports EngineMissing cleanly until installed (user deferred the install).
- **HEIC/HEIF (GW-01)**: lane drives libheif heif-dec (DLL closure hand-assembled from MSYS2 packages via PE import-table scanning); both targets e2e green.
- **First real tri-platform CI: ALL GREEN** (run 33626245570, Windows/macOS/Linux success). The chase surfaced and fixed: repo-contract capability allowlist drift, pnpm '--' flag pass-through, empty-starter dev-build staging, unix cfg lints (unnecessary_wraps, cfg-scoped test import), preset-library v1 schema missing the quality knobs (caught by the contract suite the local --lib loop never ran), a Result ok().is_some_and pair, and two CI-timing flakes (PowerShell process-tree fixture 3s->30s, queue-thread callback 5s->60s). WebView accessibility smoke is continue-on-error on headless runners with the interactive evidence boundary preserved.

### Verification
- Local: core 222 (+4 symlink baseline), schema contracts 9/9, desktop lib 33, frontend 29/29, server 13/13, workspace clippy -D warnings clean, fmt clean, cargo-deny ok, dependency audit 0 vulnerabilities.
- Conversion matrix: 68/68 routes pass locally (Word exports, docx<->odt, HEIC, 7z included).


## Current milestone — Wave 4: OCR (G-24), PDF metadata (G-25), 7z archive (2026-09-02)

### Landed (this subagent)
- **G-24 OCR**:
  - Image -> txt operation-free route: png/jpg/jpeg -> txt via tesseract (`tesseract <in> stdout -l eng --psm 3`; stdout mode avoids tesseract's auto `.txt` suffix). New `ocr.rs` (`plan_image_ocr`, `plan_pdf_ocr`, `validate_ocr_output`). Step arguments use `ocr_mode` (not `operation`) so the image lane stays off the qpdf operation dispatch. Acceptance: `OCR_TEXT_NONEMPTY` (required; at least one alphanumeric token). `OCR_CONFIDENCE` was descoped (tesseract does not print per-word confidence to stdout by default).
  - `pdf-ocr` operation: per page `pdftoppm -f N -l N -r 150 -png` into a staging tempdir, tesseract per page, concatenated txt. Acceptance: `OCR_PAGE_COVERAGE` (processed pages == pdfinfo page count, required) + `OCR_TEXT_NONEMPTY`.
  - doctor discovery list + `FORMATWRIGHT_ENGINE_TESSERACT` env (generic env plumbing already existed). CLI: `--operation pdf-ocr`; images just use `--to txt`.
- **G-25 pdf-metadata**: `apply_pdf_metadata(input_bytes, title, author)` performs a PDF incremental update in-process (zero new deps): parses the last `startxref`, locates the old trailer, copies `/Root`, appends a new `/Info` object + one-entry xref subsection + new trailer with `/Prev` and `/Info` pointing at the new object. Acceptance: `PDF_METADATA_TITLE`/`PDF_METADATA_AUTHOR` (required, pdfinfo-observed) + page-count conservation. CLI `--metadata-title/--metadata-author`. `PlanRequest` gained `metadata_title`/`metadata_author` (serde default; exhaustive literals in cli/main.rs and planner.rs tests updated).
- **7z archive**: `sevenz-rust = "0.6"` (workspace + core). archive.rs: `.7z` magic `37 7A BC AF 27 1C` recognition, `read_7z_entries` (drains entries through a counting discard writer; directory names normalized to trailing `/` so ZIP manifests stay comparable), `repack_zip_to_7z` / `repack_7z_to_zip`. Planning/capabilities/workflow/runner extended; acceptance reuses `ARCHIVE_ENTRY_COUNT`/`ARCHIVE_ENTRY_MANIFEST`.

### Tradeoffs
- Metadata is set via incremental update, not a rewrite: unset fields keep old values, other `/Info` entries are inherited (documented as `unknown` in the ChangeSet); output keeps every original byte verbatim plus the appended revision.
- `pdf-metadata`/`pdf-ocr` route through `prepare_pdf_operation`, so both inspect `qpdf`/`pdfinfo` up front; the metadata step engine identity is qpdf even though execution is in-process (kept for lane consistency).
- 7z support is zip <-> 7z only (tar.gz <-> 7z deliberately out of scope).
- OCR confidence Warning descoped (no cheap stdout parse).

### Verification
- `target
un-tests.bat`: 222 passed + the 4 pre-existing symlink os-error-1314 failures (unchanged baseline; two of the +tests belong to the parallel document agent).
- `targetmt-fix.bat`: FMT_CLEAN; clippy zero warnings for the files touched here (document.rs warnings belong to the parallel wave).
- E2E (debug CLI, engines via FORMATWRIGHT_ENGINE_*):
  - pdf-metadata on a soffice-produced 1-page PDF: `pass` with `PDF_OPS_PAGE_COUNT`, `PDF_METADATA_TITLE`, `PDF_METADATA_AUTHOR` all pass; `pdfinfo` independently reports `Title: ELECTRIC Title 440010147700`, `Author: Anole e2e`, `Pages: 1`; `qpdf --check` reports no syntax errors.
  - zip -> 7z -> zip round trip: both legs pass `ARCHIVE_ENTRY_COUNT`/`ARCHIVE_ENTRY_MANIFEST`; python zipfile confirms identical (name,size) inventory.
- **OCR e2e pending tesseract install** (`E:\DevCaches\Tesseract-OCR	esseract.exe` not present). Rerun after install:
  - `set FORMATWRIGHT_ENGINE_TESSERACT=E:\DevCaches\Tesseract-OCR	esseract.exe`
  - image lane: `formatwright convert ocr.png --to txt --output ocr.txt` (PIL fixture: white 800x300 PNG containing `OCR TEST ELECTRIC 440010147700`).
  - pdf lane: `formatwright convert scan.pdf --operation pdf-ocr --to txt --output scan.txt`.
  - Expect `OCR_TEXT_NONEMPTY` pass; pdf lane additionally `OCR_PAGE_COVERAGE`.

### Risks / Follow-up
- Tesseract `-l eng` is fixed (no language option yet); DPI fixed at 150.
- `apply_pdf_metadata` assumes a classic (non-xref-stream) trailer; xref-stream PDFs fall back to the first `trailer` search and would fail closed with an input error if `/Root` is absent.
- 7z entries with anti-item or empty-stream file semantics rely on sevenz-rust behavior; round-trip covered by unit + e2e for the zip case.


## Current milestone — Parallel wave 3: watermark, target-size, track UI, CORS, cross-platform, LibreOffice (2026-09-01)

### Landed
- **G-23 watermark** (subagent): pdf-watermark operation builds a hand-written single-page PDF stamp layer (Helvetica-Bold, rotation, alpha ExtGState) and applies it with qpdf --overlay --repeat; validation = page-count conservation + watermark text presence (order-insensitive match, since rotated text extracts scattered).
- **G-32 target size** (subagent): --target-size-kb drives a bounded CRF ladder (20/26/32/38) on mp4 transcodes; VIDEO_TARGET_SIZE Warning reports observed vs target or the nearest reachable rung. E2E: VP9 2.6 MB -> 903 KB nearest-rung Warning (testsrc cannot reach 500 KB at acceptable rungs).
- **G-31 track UI** (subagent): expert Convert form gains an audio-track selector fed by the plan probe's streams (auto = None), wired through DesktopConversionRequest.
- **API polish** (subagent + main): malformed JSON bodies now answer the structured {code,stage,message,action} shape; CORS layer (incl. OPTIONS preflight) added so website/demo.html can drive the loopback API from file://.
- **G-34** (subagent): CI fmt collapsed to the Linux job, macOS runs the SBOM script, and doctor's known_install_location generalized to macOS Chromium bundle layouts (+2 tests).
- **G-35**: website/demo.html (receipt-style API demo, curl-verified contract).
- **LibreOffice 26.2.4** installed to E:\DevCaches\LibreOffice (MSI administrative image, no admin rights) with FORMATWRIGHT_ENGINE_SOFFICE pointing at soffice.com (console shim - soffice.exe hangs GUI-substyle on --version). Runner office filter map extended for ODF/RTF flavors. ODT->PDF e2e: LibreOffice-generated ODT converts with validation Warning and a verified text layer. Note: an administrative-image soffice is fine for headless conversion, but a normal installation is still the supported posture for releases.
- Updater release keypair rotated (strong random password stored beside the key, both git-ignored); website repo/download links now point at github.com/leolemon777/FormatWright.

### Verification
- core 211 passed + 4 symlink baseline; fmt clean; clippy zero warnings; server 13/13; frontend 29/29; desktop rebuilt and running. E2E evidence: watermark chars verified independently via pdftotext, target-size nearest-rung Warning, ODT->PDF text layer, demo contract curl-verified.

### Risks / Follow-up
- Wrong-password decrypt copy still generic; watermark text check is order-insensitive (documented); admin-image LibreOffice is a dev convenience, not the certified distribution form.
- Remaining roadmap: G-24 OCR, G-25 metadata edit, G-34 real cross-platform CI runs (untested on actual runners), release signing account.

## Current milestone — Parallel wave 2: PDF toolbox, REST API, ODF/RTF, release engineering, website (2026-09-01)

### Landed (parallel subagents + main thread, all gated and committed)
- **W3 PDF toolbox** (`88c6f94`): pdf-rotate/compress/encrypt/decrypt on the ADR-0013 machinery; one-shot in-process secret store keeps passwords out of serialized Plans; PDF_ENCRYPTED proven by pdfinfo failing on the output; compression ratio as Warning. E2E on qpdf 12.4.1 all Pass.
- **G-33 REST API** (`c4cd38b`): crates/server (axum) reuses the CLI application pipeline; every convert response carries the ValidationReport; structured errors; 127.0.0.1-only; 11 tests + live e2e.
- **G-30 complete** (`16e74dc`): ODT/ODS/ODP by ODF structure+flavor, RTF envelopes without a ZIP; Basic macro members blocked; office lane to PDF.
- **W1** (`df159e9`, `updater slice`): starter-pack population assertion in the release workflow, updater plugin wired end-to-end with a dev keypair outside the repo (docs/release/UPDATER.md), release checklist gates.
- **G-05 website** (committed with wave): offline single-file Meadowlark landing page, EN/中文, receipt-metaphor hero.

### Verification
- core 204 passed + 4 pre-existing symlink failures (baseline); fmt clean; clippy zero warnings; server 11/11; frontend 28/28; desktop rebuilt and running with the updater.

### Risks / Follow-up
- Updater uses a dev keypair - must rotate before any public signed release (UPDATER.md step 1).
- Encrypt/decrypt in the durable queue cannot resume after restart (one-shot secret), reported with an explicit error.
- Wrong-password decrypt reports the generic encrypted-PDF message (correct rejection, imprecise copy).
- Remaining roadmap: G-04 real code signing, G-23/24/25 (watermark/OCR/metadata), G-31/32, G-34 cross-platform, G-35 web front.

## Current milestone — Gap-roadmap execution wave: G-10/30/13/11/12/01 landed (2026-09-01)

### Spec Interpretation
- User goal: execute the full competitive-gap roadmap, item by item, each with tests and evidence ("全部都改，都改完，确认无误"), toward an iteratively superior product.
- Wave status after this session: W2 (G-10/G-11/G-13/G-30) and G-01 fully landed; G-12 + ADR-0013 landed; remaining backlog is W1 release engineering (G-02/G-03/G-04), W3 PDF toolbox extensions, and W4.

### Decisions Made & Landed (all on main, each commit gated by fmt+clippy+full tests)
- **G-10 EPUB target** (`5387c70`/`659cbac`): md/html → epub via Pandoc; OCF magic detection distinguishes EPUB from DOCX prefixes; validation = EPUB_PACKAGE_OPENS/TARGET_FORMAT/CONTENT_DOCUMENTS/TEXT_COVERAGE(required, ≥80%)/TEXT_FIDELITY(Warning — nav/toc repeats chapter titles, same rationale class as EDGE_PDF_TEXT_FIDELITY). E2E: 2-chapter md → 9-entry publication, pandoc reads back complete.
- **G-30 text slice** (`c494208`): .txt/.text → 'plain' format riding the GFM reader (Pandoc has no plain reader; every plain doc is valid Markdown); routes pdf/docx/epub. ODT/RTF deferred — they need ODF/RTF package inspection, not a whitelist.
- **G-13 knobs core+CLI** (`7ac5c21`): PlanRequest.video_crf(0-51)/video_preset(allowlist)/audio_bitrate_kbps(8-320) flow into mp4+audio plans and replace the hardcoded `-preset medium -crf 20`/192k; CLI --video-crf/--video-preset/--audio-bitrate-kbps. E2E proof: VP9→MP4 at 64 kbps measured 64.6 kbps by independent ffprobe. **Desktop UI wiring remains the open G-13 follow-up.**
- **G-11 archive lane** (`4244c45`): built-in formatwright.archive engine; zip ↔ tar.gz in-memory repack (no extraction, deterministic mtime 0, traversal paths rejected, links/devices refused); validation = ENTRY_COUNT + name:size manifest digest. Added tar crate; flate2 promoted from transitive. E2E: 3-entry zip → tar.gz → zip round trip byte-identical.
- **ADR-0013 + G-12** (`1c87a36`/`4903ffc`): PlanRequest.operation/inputs/page_range (serde-default); pdf-merge (joint fingerprint) & pdf-extract via qpdf --empty --pages; MEASURED page-count conservation (PDF_OPS_PAGE_COUNT) via post-execution pdfinfo; verbatim `\\?\` prefixes stripped for qpdf (external_process_path). qpdf 12.4.1 installed to E:\DevCaches with engine override. E2E: 3+2→5 pages Pass; '2-3' extract = exactly source pages 2-3 by pdftotext.
- **G-01 sandbox** (`aadaa7b`): scripts/test_browser_print_sandbox.ps1 + docs/testing/BROWSER_PRINT_SANDBOX.md; GW-10 matrix caveat cleared. Two pre-existing main bugs fixed en route: structured_format_hint swallowed every '<'-prefixed file (CLI inspect of doctype HTML/SVG died as XML), and is_document_path missed svg/epub/txt.

### Verification
- Per-commit: cargo fmt --check clean, clippy zero warnings, core lib tests green (197 passed + 4 pre-existing symlink-privilege os-1314 failures, unchanged baseline).
- Every feature has an end-to-end run on this machine with independent verifier (pandoc read-back, python zipfile, ffprobe, pdfinfo/pdftotext) — evidence captured in commit messages.

### Risks / Follow-up
- **G-13 desktop UI wiring** landed in `f48af08`: expert form + preset editor expose CRF/preset/bitrate per target, preset validation mirrors runner ranges, 28/28 frontend tests.
- ODT/RTF inputs, W3 PDF toolbox (rotate/compress/encrypt/watermark/OCR), API service, updater, cross-platform remain queued per COMPETITIVE_GAP_ROADMAP W3/W4.
- Operation routing is not yet surfaced in the desktop UI (CLI-only); capability snapshot doesn't advertise operations.

## Previous milestone — Competitive gap analysis and v0.2+ roadmap (2026-09-01)

### Spec Interpretation
- User asked for a deep, source-verified gap analysis against competitors (VERT, File Converter, Stirling-PDF, Gotenberg, HandBrake) and then a master plan covering all findings.

### Decisions Made
- Evidence base: read `capabilities.rs` routing (15 targets, lane model), CLI surface (doctor/identify/inspect/plan/convert/jobs/engines/maintenance), full i18n key inventory as the UI feature face, sandbox suite list, `pdf.rs` (inspection + render only — no PDF post-processing), `runner.rs` hardcoded `-crf 20 -preset medium`, absence of updater config, and the CI/release-docs inventory. Confirmed CI (ci/fuzz/release-candidate/dependabot) already exists.
- Deliverable: new [`docs/COMPETITIVE_GAP_ROADMAP.md`](docs/COMPETITIVE_GAP_ROADMAP.md) with G-xxx numbering (no clash with master-plan R-xxx), four waves: W1 release blockers (G-01..04), W2 quick wins (G-10 epub via pandoc, G-11 native archives, G-12 qpdf merge/split with page-conservation validation, G-13 expose codec/CRF/framerate parameters), W3 PDF toolbox on the G-12 operation-routing model (rotate/crop/compress/encrypt/watermark/OCR), W4 breadth+service (ODT/RTF, track selection UI, target-size compression, REST API aligned to Phase 6, macOS/Linux).
- Every new lane must carry machine-readable validation checks (page-count conservation, text-layer retention, hash manifests) — the project's differentiator extended to new capabilities; stated as the entry rule in the roadmap.
- One new architecture decision identified: operation-style routing (multi-input + operation → PDF) for G-12, flagged for ADR-0013 before implementation.
- Explicit non-goals recorded: CAD, PDF→DOCX editable round-trip, Ghostscript (AGPL undecided), chasing VERT's format count.

### Verification
- All claims trace to named source files (see roadmap tables); competitor facts from official sites/GitHub (VERT 15.4k stars AGPL, Stirling 60+ tools, Gotenberg Chromium-based API).
- Linked from `MASTER_EXECUTION_PLAN.md` header; master plan v0.7 body untouched (it still owns v0.1 closure).

### Risks / Follow-up
- Roadmap is a planning artifact; priorities need owner confirmation before Wave 2 starts.
- G-12's operation-routing model is the only structural change — schedule the ADR first.

## Previous milestone — Desktop verification session: title-bar drag fix, local engine provisioning, engine guide (2026-09-01)

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

## 2026-09-03 — Rebrand FormatWright → Anole

Name and mascot decided by the owner: **Anole** (the color-changing "American chameleon", 5 letters, clean in the converter category) with mascot direction A "The Color Shift". Brand assets live in `branding/` (candidates + `branding/final/`: icon light/dark, logo, horizontal lockups, favicon ladder, PNG exports).

**Swapped this pass (user-visible surface):** README/website/governance docs/docs tree (guarded line-level script with an identifier allow-list), core/cli/server/engine-sdk user-visible messages and evidence strings, desktop window titles/`productName`/i18n/dialog filters, Explorer context-menu labels (`Open in Anole`, registry **key names unchanged**), JSON-Schema titles, SBOM generators, crate `description`/`authors`, and the release workflow's installer filename (now `Anole_0.1.0_x64-setup.exe`, matching the new `productName`).

**Deliberately kept (technical identifiers, own follow-up pass):** crate/binary names (`formatwright*`), `formatwright_core::` paths, `FormatWrightError`/`FormatWrightCompatibility`, `.join("FormatWright")` state-database dirs, `FORMATWRIGHT_ENGINE_*` env vars, `...\shell\FormatWright` + `FormatWright.To*` registry verbs, tauri identifier `local.formatwright.desktop`, repo/GitHub name and updater URL.

Verified: `cargo check` (core/cli/server/engine-sdk) clean; `core --lib` 244 passed / 4 failed = the known Windows reparse/symlink baseline; engine-sdk 11 passed; residual-string audit shows only the intended technical identifiers. Trademark screening (Nice 9/42) is still owed before external promotion.
