# FormatWright

**File conversion you can verify.**

FormatWright is an open-source, local-first file conversion platform. It is designed to explain the selected conversion path, prefer remuxing or lossless operations when possible, recover safely from interrupted batch jobs, and validate the result instead of treating a zero exit code as proof of success.

## Status

FormatWright is under active **Windows development Alpha**. The unsigned Windows development installer embeds pinned PDF and Media Starter packs and has passed local real-file conversion, but clean-machine offline certification, complete engine licensing/SBOM work, trusted signatures, upgrade/rollback, and code signing are still pending. It is not Public Beta or Certified, and it is not ready for production data.

- Product scope and release gates: [SPEC_PLAN.md](SPEC_PLAN.md)
- Living completed / pending checklist, architecture, and ordered gates: [docs/MASTER_EXECUTION_PLAN.md](docs/MASTER_EXECUTION_PLAN.md) (see §1.1 progress snapshot)
- Requirement → code → evidence map: [docs/specs/TRACEABILITY.md](docs/specs/TRACEABILITY.md)

**Latest engineering milestones (2026-08-13):**
1. CLI durable-queue execution runs through shared `JobExecutionService` in Core.
2. Desktop binds execution to the visible Plan hash, persists reports before terminal state, and supports recoverable immediate pause plus per-job Resume/Retry.
3. Queue execution closes worker/process-tree failures, normalizes Windows output identity, leaves no cancellation-link tasks, and keeps live history/paging/enqueue available through a separate SQLite connection.
4. Shared SQLite maintenance now provides full integrity checks, validated online WAL-consistent backups, isolated restore preflight, confirmed transactional restore, compaction snapshots, and automatic pre-migration snapshots with five-copy retention.
5. SQLite mutations acquire the writer before reading mutable state, and every output family publishes through the same no-clobber filesystem primitive so a late destination cannot be overwritten.
6. Windows NSIS now owns a classic Explorer file/directory entry; strict local-path parsing and single-instance forwarding pre-fill the existing Convert window without auto-running or starting a competing recovery process.
7. A real current-user install smoke corrected NSIS registry quoting and now proves actual Shell verb cold/hot routing, one-process forwarding, zero auto-created jobs, exact uninstall ownership and byte-for-byte application-state restoration.
8. A real Tauri/WebView2 automated accessibility gate now covers named controls/landmarks, first-Tab skip navigation, selected-state semantics, 200% physical-equivalent layout, bidi paths, reduced motion, forced colors/high contrast and bilingual document semantics.

The next engineering gate moves into format/engine-supply-chain hardening, clean-VM install and release certification, while live screen-reader/physical-DPI/usability evidence remains in the Desktop gate. Release certification still requires a clean offline Windows VM and the supply-chain work above.

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
pwsh -File scripts/test_pdf_sandbox.ps1
pwsh -File scripts/test_office_sandbox.ps1
pwsh -File scripts/test_multi_process_queue.ps1
pwsh -File scripts/test_queue_crash_recovery.ps1
pwsh -File scripts/test_mixed_ten_thousand.ps1
~~~

The sandbox suites generate synthetic fixtures in an isolated `.artifacts` directory and verify media, audio, GIF, image/HEIC, structured-data, document, PDF-rendering, and Office-to-PDF paths plus conflict protection, cancellation, multi-process exact-once ownership, and crash recovery. See `docs/testing/` for each suite's exact evidence boundary.

Start with [the user guide](docs/USER_GUIDE.md), use [troubleshooting](docs/TROUBLESHOOTING.md) for typed recovery actions, and read [the privacy statement](PRIVACY.md) before sharing reports. `scripts/generate_sbom.py` writes the application dependency inventory to ignored `dist/sbom.spdx.json`; third-party conversion-engine SBOMs remain separate pack artifacts.

## Security

Do not use FormatWright on untrusted files until the relevant engine sandbox and threat-model gates are complete. Please follow [SECURITY.md](SECURITY.md) for reporting vulnerabilities.

## License

The Rust core, CLI, desktop application, and engine SDK are licensed under Apache-2.0. The planned self-hosted service will be licensed separately under AGPL-3.0. Documentation is intended to use CC BY 4.0. Third-party engines retain their own licenses and are distributed separately.
