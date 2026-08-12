# PDF-to-Image Sandbox Tests

- Status: Phase 2 Windows development evidence
- Updated: 2026-08-10
- Workflow: GW-09 alpha slice

## Scope

`scripts/test_pdf_sandbox.ps1` generates a synthetic three-page PDF, an encrypted copy, a wrong-extension copy, and a truncated copy. It exercises content-first PDF inspection, typed Poppler planning, all-page PNG/JPEG rendering, per-page validation, directory-level atomic commit, and negative policy paths through the public CLI.

The passing fixture promotes the covered GW-09 all-pages path to Experimental on Windows. It does not certify selective page ranges, transparent-background preservation, malformed individual-page continuation, hostile-PDF isolation, or another platform.

## Security and resource contract

- `pdfinfo` and `pdftoppm` are launched by exact hashed paths without a shell.
- The Plan records the selected engine version/hash, page count, DPI, color mode, JPEG quality, opaque-white background, deterministic naming, and Network Deny policy.
- Encrypted PDFs are blocked without accepting, storing, or reporting a password in the alpha path.
- Inspection is capped at 10,000 pages; DPI is 36–600; each page is capped at 16,384 pixels per axis and 100 megapixels.
- Rendered files remain in a deterministic hidden same-parent directory until every page passes validation. The entire directory is then committed with one rename.
- Cancellation, engine failure, input mutation, validation failure, and job recovery remove the exact staged directory.
- Native pixel decoding is bounded to 512 MiB per page.

## Directly verified assertions

- A three-page PDF with Letter, A4, and landscape Letter pages is inspected as three ordered page streams.
- A `.bin` copy is detected from `%PDF-` content and reports an extension mismatch.
- RGB PNG at 144 DPI produces exactly `page-000001.png` through `page-000003.png` with dimensions 1224×1584, 1191×1684, and 1584×1224.
- Grayscale JPEG at 96 DPI and quality 77 produces three validated pages with dimensions 816×1056, 794×1123, and 1056×816.
- Each page is independently reopened by ffprobe and by Pillow; native bounded pixel samples verify grayscale policy and opaque output.
- The report records exact page count, target formats, dimensions, color evidence, alpha policy, engine identities, and a deterministic page-set fingerprint.
- Encrypted and truncated PDFs, invalid DPI, PNG quality, and an existing output directory are rejected with typed errors.
- A 600-DPI render cancelled at timeout zero commits nothing, persists `cancelled`, retries through the public durable queue, revalidates engine/input identity, and completes the same immutable Plan.
- Source bytes remain unchanged and no hidden staged directory remains.
- Representative RGB and grayscale renders were visually inspected for legibility, orientation, spacing, and color behavior.

## Run

~~~powershell
cargo build -p formatwright-cli
pwsh -NoProfile -File scripts/test_pdf_sandbox.ps1 `
  -Python <python-with-reportlab-pypdf-pillow> `
  -PdfInfo <pdfinfo> `
  -PdfToPpm <pdftoppm> `
  -Ffprobe <ffprobe>
~~~

The recorded Windows run `pdf-suite-fc5643a77c4e4251842b521fd358b110` passed every assertion with Poppler 26.05.0 and FFmpeg/ffprobe 8.1.1.

## Remaining certification work

- Mixed rotation metadata, CropBox/MediaBox policy, transparency-preserving PNG, ICC/display-profile fixtures, annotations, forms, very large pages, and PDFs with one damaged page.
- Explicit page selection/ranges and individually reported partial-page failures; the alpha adapter intentionally commits only complete all-page sets.
- Forced-crash injection while a long render is active (timeout cancellation and immutable-Plan queue retry are covered).
- Malicious corpus, parser/render fuzzing, OS sandbox containment, zero-network canary, certified signed Poppler pack, and Windows/macOS/Linux golden runs.
