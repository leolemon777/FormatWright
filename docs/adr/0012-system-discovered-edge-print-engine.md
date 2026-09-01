# ADR-0012: System-discovered Microsoft Edge as the browser print engine

- Status: Proposed
- Date: 2026-08-31
- Owners: FormatWright maintainers
- Related requirements: GW-10 (HTML/SVG → vector PDF lane)

## Context

GW-10 currently renders Markdown/HTML to PDF through Pandoc → DOCX → LibreOffice, which normalizes markup through two intermediate representations and cannot preserve a vector text layer from browser-authored layouts (SVG input is unsupported entirely). A browser print engine produces vector PDFs directly: text stays selectable, drawing commands stay vector, and page description comes from one rendering engine instead of a document-converter chain.

Microsoft Edge is Chromium-based, present on every supported Windows install, and exposes a stable headless `--print-to-pdf` path. Redistribution is not permitted under the Microsoft Edge license terms, and bundling Chromium separately would add a large supply-chain surface. The engine inventory (`engines/README.md`) already distinguishes shipped packs from system-discovered executables, and `EngineDiscoveryPolicy` already confines PATH/env discovery to development builds.

Poppler's `pdftotext` and `pdffonts` utilities join `pdfinfo`/`pdftoppm` as validators so the "editable vector PDF" claim is provable: a text layer that an independent tool can extract, and fonts that are embedded.

## Decision

1. The browser print engine is identified by engine id `msedge` and is resolved only through (a) a registered verified pack, (b) the `FORMATWRIGHT_ENGINE_MSEDGE` development override, (c) PATH, or (d) the canonical vendor install locations (`%ProgramFiles(x86)%`/`%ProgramFiles%` on Windows, `/Applications/...` on macOS, `/usr/bin/microsoft-edge*` on Linux) — the last three only under `EngineDiscoveryPolicy::Development`. It is never bundled or redistributed by FormatWright.
2. Doctor never launches the browser to probe it. Identity is the executable hash plus the version derived from the versioned install directory (Windows) or `unknown`.
3. The engine is executed headless with an isolated staged `--user-data-dir`, `--host-resolver-rules=MAP * ~NOTFOUND` as a network-deny reinforcement, a bounded print timeout, process-tree termination on cancel/timeout, and `LossClass::None` on the print step because vector printing rasterizes nothing.
4. HTML→PDF keeps the Pandoc lane as an explicit fallback lane; route availability is per-lane, so a machine with only one lane still converts. SVG→PDF is browser-lane only.
5. Vector claims are validated, not asserted: `pdftotext` must extract a text layer whenever the input declares text, and `pdffonts` must report every declared font embedded; both run against the staged output before commit.

## Consequences

- Windows machines convert HTML/SVG → vector PDF with zero extra engine installs in development builds; release builds still require a verified pack that ships an Edge-compatible executable, keeping the supply-chain boundary intact.
- Conversion output now depends on the user's installed browser version; plan hashes embed the resolved engine identity so a browser upgrade invalidates stale approvals by design.
- `pdftotext` and `pdffonts` become required Poppler utilities for this lane; Poppler distributions ship them together, so no new supplier is introduced.
- A rasterized fallback (printing pages as images) is explicitly not offered; documents that need pixel reproduction should use the Office lane or PDF render targets instead.

## Verification

- `crates/core` unit tests: lane routing (`route_engine_lanes`), SVG inspection (`document.rs`), plan shape and engine guards (`edge_pdf.rs`), `pdffonts` table parsing from the right, and report aggregation.
- `cargo check -p formatwright-core`, `cargo clippy -p formatwright-core --all-targets` (zero warnings), `cargo test -p formatwright-core --lib`, and the schema contract suite pass on Windows.
- An end-to-end sandbox script for a real HTML fixture (browser lane with validation report) is the remaining evidence item before the lane can be marked Experimental in the support matrix.

## Revisit when

- A signed, redistributable Chromium-family pack passes supply-chain review; then the canonical-location discovery can be retired in favor of pack activation.
- Engine SDK adopts the ADR-0002-reserved versioned stdio protocol; the print adapter should migrate off argv-only control.
- PDF/A or tagged-PDF output becomes a product promise; the print arguments and validators must extend together.
