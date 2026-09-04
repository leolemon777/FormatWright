# Anole

<p align="center"><img src="branding/final/png/lockup-light.png" width="420" alt="Anole — one file, any form." /></p>

<p align="center">
  <a href="https://github.com/leolemon777/FormatWright/actions/workflows/ci.yml"><img src="https://github.com/leolemon777/FormatWright/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <a href="https://github.com/leolemon777/FormatWright/releases"><img src="https://img.shields.io/github/v/release/leolemon777/FormatWright?include_prereleases" alt="Release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue" alt="License: Apache-2.0" /></a>
  <a href="https://leolemon777.github.io/FormatWright/"><img src="https://img.shields.io/badge/website-leolemon777.github.io%2FFormatWright-2ea44f" alt="Website" /></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux-9cf" alt="Platforms: Windows, Linux" />
  <img src="https://img.shields.io/badge/routes-252%20canonical-orange" alt="252 canonical reachable conversion routes" />
</p>

**File conversion you can verify.**

Anole (formerly FormatWright) is an open-source, local-first file conversion platform. It is designed to explain the selected conversion path, prefer remuxing or lossless operations when possible, recover safely from interrupted batch jobs, and validate the result instead of treating a zero exit code as proof of success.

## Status

**v0.1.0 Public Beta (Unsigned Alpha) — released 2026-09-04.** Download the Windows x64 installer from the [latest release](https://github.com/leolemon777/FormatWright/releases/latest) (see `SHA256SUMS`; the installer is unsigned, so SmartScreen will warn), or browse the [website](https://leolemon777.github.io/FormatWright/). What shipped: 252 canonical reachable conversion routes (133 direct + 119 chained, counted by `scripts/count_routes.py`; TIFF/BMP/PSD and camera-RAW (DNG/CR2/...) joined the raster family post-release through ffmpeg and the discovered ImageMagick engine), every hop with validation receipts, durable SQLite queue with crash recovery, plan-first approval, sandboxed inputs, CLI + desktop GUI + REST API, tri-platform CI, and a 10,000-job soak evidence trail. Known gaps: no code-signing certificate yet (v0.1.1 will be signed), OCR needs a host Tesseract on Windows, clean-VM certification evidence is still pending, and macOS has CI coverage only.

- Product scope and release gates: [SPEC_PLAN.md](SPEC_PLAN.md)
- Living completed / pending checklist, architecture, and ordered gates: [docs/MASTER_EXECUTION_PLAN.md](docs/MASTER_EXECUTION_PLAN.md) (see §1.1 progress snapshot)
- Requirement → code → evidence map: [docs/specs/TRACEABILITY.md](docs/specs/TRACEABILITY.md)
- Release engineering records: [implementation-notes.md](implementation-notes.md), [docs/release/](docs/release/)

The historical engineering milestone log through the alpha phase lives in [implementation-notes.md](implementation-notes.md). The next gates are the code-signing certificate, clean-VM evidence, and the post-release format long tail (TIFF/BMP, RAW, PSD, MSG, MBOX).

## Product promises

- Local by default, with no conversion telemetry.
- One Rust core shared by desktop, CLI, API, and future MCP surfaces.
- Large-file execution without buffering the entire file in the application.
- Persistent batch jobs with pause, retry, and crash recovery.
- Explainable conversion plans.
- Human-readable and machine-readable validation reports.
- No silent overwrite.
- Auditable engine versions, build flags, licenses, and hashes.
- Desktop import-by-reference re-verifies engine manifests and binaries at every start; imported packs remain unverified until the release keyring trusts their signatures.

## Initial architecture

- Rust stable + Tokio for the control plane.
- Clap for the CLI.
- SQLite for persistent jobs.
- Tauri 2 + React/TypeScript for the desktop application.
- FFmpeg/ffprobe, libvips, LibreOffice, Pandoc, and PDF engines as isolated subprocesses.

## Repository layout

~~~text
crates/core          Domain, inspection, planning, execution, queue, validation
crates/core/src/application  Shared use cases (JobExecutionService; ConversionService planned)
crates/engine-sdk    Engine manifests and versioned adapter protocol
crates/cli           formatwright command-line interface (thin surface over Core)
apps/desktop         Tauri desktop application and shared-core workflow surface
docs/adr             Architecture decisions
docs/specs           Executable supporting specifications
docs/testing         Reproducible sandbox evidence and suite IDs
docs/security        Threat model and engine supply chain
docs/release         Platform matrix and release gates
test-corpus          Licensed golden corpus manifests and generated fixtures
~~~

## Engines

Anole deliberately ships **without bundled conversion engines**. Third-party binaries carry their own license and supply-chain obligations (GPL/LGPL/MPL components, and Microsoft Edge may not be redistributed at all), so the application discovers engines on the host instead. See [the engine inventory](engines/README.md), [ADR-0011](docs/adr/0011-trusted-engine-signatures-and-release-keyring.md), and [ADR-0012](docs/adr/0012-system-discovered-edge-print-engine.md) for the full rationale.

Discovery order per engine: an activated engine pack, then a `FORMATWRIGHT_ENGINE_<NAME>` environment variable (full path to the executable, e.g. `FORMATWRIGHT_ENGINE_PDFINFO`), then `PATH`, then known vendor install locations (`msedge` only). Discovered engines are reported as `unverified` by design; the doctor never downloads anything.

| Engine | Unlocks | Where to get it |
|---|---|---|
| `msedge` | Browser print lane: HTML/SVG → vector PDF | Preinstalled on Windows 10/11; nothing to do |
| Poppler (`pdfinfo`, `pdftoppm`, `pdftotext`, `pdffonts`) | PDF output validation for the browser and document lanes | [poppler-windows releases](https://github.com/oschwartz10612/poppler-windows/releases); verify the archive hash, extract, and add `Library\bin` to `PATH` |
| `soffice` (LibreOffice) | Office → PDF (docx/xlsx/pptx) | [libreoffice.org](https://www.libreoffice.org/) or `winget install TheDocumentFoundation.LibreOffice` |
| `pandoc` | Markdown/document conversion lane | [pandoc.org](https://pandoc.org/installing.html) |
| `ffmpeg` / `ffprobe` | Media inspect, remux, transcode | [ffmpeg.org](https://www.ffmpeg.org/download.html) |

`qpdf`, `vips`, and `heif-convert` appear in the doctor inventory but have no conversion route wired in v0.1; installing them is optional.

Official release builds additionally carry a reviewed **starter engine pack** (Poppler + FFmpeg with manifests, hashes, license files, and signatures) built by `scripts/prepare_windows_starter_pack.ps1`, so end users get PDF validation and media conversion out of the box without the application redistributing unreviewed binaries. When an engine is missing, the doctor page shows exactly which one and why; install it through the upstream channel and rerun the check.

## Development

Prerequisites:

- Rust stable with rustfmt and clippy.
- Git.
- FFmpeg and ffprobe for media/image paths; libheif `heif-convert` for the Windows HEIC development fallback; Poppler `pdfinfo`/`pdftoppm` for PDF rendering and validation; LibreOffice `soffice` for Office-to-PDF.
- Node.js and pnpm when desktop work begins.

Common checks:

~~~text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo run -p formatwright-cli -- doctor
cargo run -p formatwright-cli -- --state-db PATH maintenance integrity-check
pnpm --dir apps/desktop test -- --run
pnpm --dir apps/desktop build
cargo test -p formatwright-desktop --all-targets
cargo test -p formatwright-core --test ten_thousand_conversions --release -- --ignored --nocapture
python scripts/generate_sbom.py
pwsh -File scripts/test_ffmpeg_sandbox.ps1
pwsh -File scripts/test_large_file.ps1
pwsh -File scripts/test_audio_sandbox.ps1
pwsh -File scripts/test_gif_sandbox.ps1
pwsh -File scripts/test_structured_sandbox.ps1
pwsh -File scripts/test_image_sandbox.ps1
pwsh -File scripts/test_heic_sandbox.ps1
pwsh -File scripts/test_metadata_sandbox.ps1
pwsh -File scripts/test_batch_sandbox.ps1
pwsh -File scripts/test_document_sandbox.ps1
pwsh -File scripts/test_browser_print_sandbox.ps1
pwsh -File scripts/test_pdf_sandbox.ps1
pwsh -File scripts/test_office_sandbox.ps1
pwsh -File scripts/test_multi_process_queue.ps1
pwsh -File scripts/test_queue_crash_recovery.ps1
pwsh -File scripts/test_mixed_ten_thousand.ps1
~~~

The sandbox suites generate synthetic fixtures in an isolated `.artifacts` directory and verify media, audio, GIF, image/HEIC, structured-data, document, PDF-rendering, and Office-to-PDF paths plus conflict protection, cancellation, multi-process exact-once ownership, and crash recovery. See `docs/testing/` for each suite's exact evidence boundary.

Start with [the user guide](docs/USER_GUIDE.md), use [troubleshooting](docs/TROUBLESHOOTING.md) for typed recovery actions, and read [the privacy statement](PRIVACY.md) before sharing reports. `scripts/generate_sbom.py` writes the application dependency inventory to ignored `dist/sbom.spdx.json`; third-party conversion-engine SBOMs remain separate pack artifacts.

## Security

Do not use Anole on untrusted files until the relevant engine sandbox and threat-model gates are complete. Please follow [SECURITY.md](SECURITY.md) for reporting vulnerabilities.

## License

The Rust core, CLI, desktop application, and engine SDK are licensed under Apache-2.0. The planned self-hosted service will be licensed separately under AGPL-3.0. Documentation is intended to use CC BY 4.0. Third-party engines retain their own licenses and are distributed separately.
