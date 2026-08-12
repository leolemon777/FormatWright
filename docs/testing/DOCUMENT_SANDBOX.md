# Markup-to-DOCX/PDF Sandbox Tests

- Status: Phase 2 Windows development evidence
- Updated: 2026-08-10
- Workflow: GW-10 alpha slice

## Scope

`scripts/test_document_sandbox.ps1` exercises Markdown and HTML inspection, offline Pandoc planning, subprocess execution, native DOCX ZIP/OPC inspection, semantic token validation, isolated LibreOffice PDF rendering, all-page Poppler validation, atomic commit, cancellation, and durable retry through the public CLI.

The passing fixture promotes Markdown/HTML → DOCX and PDF to Experimental on Windows. PDF output remains Warning even when all required checks pass because visual fidelity is not calibrated.

## Security and validation contract

- Pandoc runs with `--sandbox=true`, a typed reader, standalone DOCX writer, no shell, and network policy Deny.
- Markup containing image/resource syntax is blocked during planning under the current deny-all resource policy.
- Markup inputs are limited to 16 MiB in the alpha materializing inspector.
- DOCX expanded size and `word/document.xml` size are bounded before extraction.
- `[Content_Types].xml`, `_rels/.rels`, and `word/document.xml` are mandatory.
- Normalized Unicode token digests must match between source markup text and extracted DOCX text.
- The validator does not yet certify pixel-level layout fidelity and the Plan says so explicitly.
- PDF plans pin Pandoc, LibreOffice, pdfinfo, and pdftoppm identities. Pandoc first produces a semantically validated intermediate DOCX; LibreOffice then runs with a macro-disabled isolated profile and Network Deny.
- Every staged PDF page must have positive dimensions, render through pdftoppm, and decode through the native bounded image path before the final same-filesystem commit.

## Covered assertions

- Markdown with headings, list items, Unicode, and spaces converts with a Pass report.
- HTML headings and inline structure convert with a Pass report.
- .NET independently opens the ZIP and finds every required OPC part.
- The native inspector reopens the output and matches the semantic token digest.
- Remote image syntax is rejected before Pandoc execution.
- Existing outputs are not overwritten, source bytes remain unchanged, and no staged output remains.
- Markdown and HTML each convert to PDF with every required DOCX/PDF check passing and the expected overall Warning.
- Independent Poppler rendering and Pillow decoding reopen every final PDF page; representative Markdown (including Chinese text) and HTML pages were visually reviewed.
- Timeout-zero cancellation commits no PDF, and the public durable queue retries the same immutable four-engine Plan to validated Warning.

## Run

~~~powershell
cargo build -p formatwright-cli
pwsh -NoProfile -File scripts/test_document_sandbox.ps1 `
  -Python <python-with-pillow> `
  -Pandoc <pandoc> `
  -Soffice <soffice.com> `
  -PdfInfo <pdfinfo> `
  -PdfToPpm <pdftoppm>
~~~

The recorded Windows run `document-suite-4def7caf51b84803ad35bafeb6540f31` passed every assertion for Markdown and HTML to both DOCX and PDF.

## Remaining certification work

- Tables, footnotes, code, math, links, local images under an explicit authorized resource root, reference DOCX templates, and malicious archive corpus.
- Calibrated visual rendering comparison, font substitution diagnostics, deeper relationship validation, embedded-object rejection, and accessibility checks.
- Certified Pandoc/LibreOffice/Poppler packs and cross-platform runs.
