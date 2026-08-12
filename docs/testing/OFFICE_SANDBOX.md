# Office-to-PDF Sandbox Tests

- Status: Phase 2 Windows development evidence
- Updated: 2026-08-10
- Workflow: GW-08 alpha slice

## Scope

`scripts/test_office_sandbox.ps1` generates synthetic DOCX, PPTX, and XLSX fixtures and exercises content-first OOXML inspection, policy screening, typed LibreOffice planning and execution, PDF validation, atomic commit, cancellation, and durable retry through the public CLI.

The passing fixture promotes the covered Office-to-PDF path to Experimental on Windows. It does not certify high-fidelity rendering, arbitrary fonts or scripts, embedded objects, password-protected documents, hostile-package containment, or another platform.

## Security and execution contract

- The native inspector opens the OOXML ZIP package with bounds of 10,000 entries, 1 GiB expanded data, and 8 MiB per relationship XML document.
- Required OPC parts determine DOCX, PPTX, or XLSX from content before the extension is considered.
- Packages containing `vbaProject.bin`, external OOXML relationships, or DTD-bearing relationship XML are blocked before LibreOffice starts.
- `soffice` and `pdftoppm` are launched by exact hashed paths with typed arguments and no shell; the Plan sets Network Deny, macros disabled, and an isolated per-job LibreOffice user profile.
- The isolated profile and engine output live in a short, unpredictable `.fw-<12-hex>` same-parent workspace. The name avoids a verified LibreOffice-on-Windows long-profile failure while preserving same-filesystem commit semantics.
- The PDF remains staged until `pdfinfo` opens it, every page has a positive size, every page renders through `pdftoppm`, and the native bounded image decoder reopens every validation render.
- Cancellation terminates the process tree, commits no output, removes the exact workspace, and leaves a durable retryable job.

## Directly verified assertions

- DOCX with a table and explicit page break converts to a validated two-page PDF.
- PPTX with two colored slides converts to a validated two-page PDF.
- XLSX with a styled header, 24 data rows, print area, and fit-to-page settings converts to a validated one-page PDF.
- A DOCX copied to `.bin` is detected from OPC content and reports an extension mismatch.
- An external relationship, a macro-bearing package, and a truncated package are rejected with typed policy/input errors.
- A pre-existing target is not overwritten.
- Independent Poppler rendering at 96 DPI produces exactly 2, 2, and 1 pages; Pillow independently decodes every rendered page and verifies positive dimensions.
- Representative DOCX, PPTX, and XLSX pages were visually inspected for expected content, ordering, color, legibility, and absence of obvious clipping or blank output.
- Timeout-zero cancellation commits nothing; public `jobs retry` and `jobs run` recheck input plus every pinned engine identity and finish the immutable Plan with the expected Warning.
- All mandatory checks Pass, sources remain byte-for-byte unchanged, and no staged workspace remains.

The overall result intentionally remains Warning because this alpha validator does not yet compare source and output visual layout. Engine exit zero and a structurally valid PDF are not described as proof of Office fidelity.

## Run

~~~powershell
cargo build -p formatwright-cli
pwsh -NoProfile -File scripts/test_office_sandbox.ps1 `
  -Python <python-with-python-docx-python-pptx-openpyxl-pillow> `
  -Soffice <soffice.com> `
  -PdfInfo <pdfinfo.exe> `
  -PdfToPpm <pdftoppm.exe>
~~~

The recorded Windows run `office-suite-7cc1e0f4084a4a3e8dca0c11b3ec7440` passed every assertion with LibreOffice 26.2.5.2 and Poppler 26.05.0. The official LibreOffice 26.2.5 x86-64 MSI used for this development run had SHA-256 `f15ba07bfcb0186986cf3171063506f5d207c11f8cc051ba0d135209e9e915f9`.

## Remaining certification work

- Golden reference documents covering font substitution, complex scripts and RTL, tables/charts, formulas, headers/footers, tracked changes, notes, hidden sheets/slides, hyperlinks, accessibility, and embedded objects.
- Calibrated source-reference and output-page visual comparison with explicit tolerance; the current result correctly reports Warning instead of claiming high fidelity.
- Password/encryption policy, malformed and decompression-bomb corpus, parser fuzzing, OS sandbox containment, zero-network canary, and forced-crash injection during active rendering.
- Certified signed LibreOffice/Poppler engine packs with complete transitive hashes/licenses/SBOM and Windows/macOS/Linux golden runs.
