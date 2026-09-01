# Browser Print Sandbox Tests

- Status: Phase 2 Windows development evidence (GW-10 browser lane)
- Updated: 2026-09-01
- Workflow: GW-10 HTML/SVG → vector PDF (ADR-0012, ADR-0013 neighbor)

## Scope

`scripts/test_browser_print_sandbox.ps1` generates a carton-drawing-shaped
HTML page (void `<meta>`, CJK company/heading text, barcode digits, a rotated
watermark, and an inline SVG vector panel), a pure-vector SVG, and a
raster-referencing SVG. It exercises markup inspection, browser-lane planning
(msedge selected, Network Deny), full HTML→PDF and SVG→PDF printing, all
required `EDGE_PDF_*` validation checks, the policy-blocked raster path, and
independent text-layer verification through the pinned `pdftotext`.

The passing run promotes the GW-10 browser lane to Experimental on Windows
with formal evidence. It does not certify other browsers, non-Windows
platforms, `@media print` corner cases beyond the fixture, or release-build
engine packs (ADR-0012 release activation remains a separate gate).

## Security and resource contract

- The browser is launched headless with an isolated temporary
  `--user-data-dir`, `--host-resolver-rules` network denial, and a 180 s
  timeout with process-tree termination; identity comes from the versioned
  install directory (no browser subprocess for version probing).
- Poppler tools resolve by explicit parameter > `FORMATWRIGHT_ENGINE_*` >
  `PATH`; the script pins all four and intentionally leaves msedge to
  ADR-0012 discovery.
- The Plan records the engine identity, page count/sizes, and Network Deny.
- A raster `<image>` reference inside SVG is flagged at inspection and
  policy-blocked at planning — the vector promise is enforced, not assumed.
- Output commits only after every required validation check passes
  (no-clobber, staged partials).

## Directly verified assertions

- The HTML fixture (doctype + void `<meta>`) inspects as `html` with a
  non-empty text inventory; the SVG fixtures inspect as `svg` with
  `image/svg+xml`.
- Planning selects `msedge` with `network_policy: deny` and declares the
  `EDGE_PDF_*` validator set.
- HTML → PDF: `status` pass/warning, every **required** check passes, and
  `EDGE_PDF_OPENS / PAGE_COUNT / PAGE_SIZES / ALL_PAGES_RENDER /
  TEXT_EXTRACTABLE / FONTS_EMBEDDED` are all present in the report.
- Independent `pdftotext` re-reads the committed PDF and finds the barcode
  digit sequence `440010147700`, the CJK heading `电子元件外箱标签`, and
  the inline SVG panel text `VECTOR PANEL 440`.
- SVG → PDF: required checks pass and the text layer carries both the Latin
  and CJK vector text.
- The raster `<image>` SVG is flagged at inspection
  (`has_external_resource`) and planning exits non-zero (`POLICY_BLOCKED`).

## Evidence

Latest local run (2026-09-01, Windows, msedge system-discovered, Poppler
26.02.0 pinned): `BROWSER PRINT SANDBOX PASS browser-print-suite-8ea0cc08…`,
HTML PDF 42,497 bytes, SVG PDF 56,083 bytes, both with pass/warning status
and zero required-check failures.
