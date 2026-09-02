# Conversion Matrix Smoke Run

- Status: Phase 2 Windows development evidence
- Updated: 2026-09-01
- Script: `scripts/test_conversion_matrix.sh` (Git Bash; run from any cwd)

## Scope

Generates one fixture per input family (structured data, Markdown/HTML/plain
text, SVG, ODT/ODS/ODP/DOCX/PPTX/XLSX/RTF produced through LibreOffice,
PNG/JPEG, PDF, MP4/WebM, six audio codecs, ZIP) and drives **every supported
conversion route** through the CLI - 59 pairs - recording the validation
status of each. Outputs must carry the target extension: staged partials
preserve the output name, and YAML/CSV/archive recognition is
extension-backed, so an extension-less output fails validation by design.

## Latest run (2026-09-01, this machine)

**59 pass / 0 fail.** Notes:

- Office-family and image-to-PDF rows report `validation: Warning`
  (office-lane advisory checks); everything else Pass.
- Two fixture authoring traps found during the run are worth remembering:
  a CSV written without its header row makes the first data row the header
  (the XML-name validation correctly blocks it), and the structured XML
  lane accepts only the records-v1 shape (element-per-field, no attributes)
  - the lane's own XML output round-trips through all three sibling targets.
- Independent content spot-checks: YAML/JSON/XML round trips carry the exact
  records; PNG->PDF embeds the original pixel data (pdfimages); PDF->PNG
  matches source page dimensions at the requested DPI.
