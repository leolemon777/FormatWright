# ADR-0013: Operation-style PDF workflows

- Status: Accepted
- Date: 2026-09-01
- Context: [`COMPETITIVE_GAP_ROADMAP.md`](../COMPETITIVE_GAP_ROADMAP.md) G-12, [`FORMAT_SUPPORT_MATRIX.md`](../specs/FORMAT_SUPPORT_MATRIX.md)

## Context

Every FormatWright workflow today is a *format conversion*: one input artifact
becomes one output artifact in a different format, routed by
(input extension, target format) through engine lanes. PDF tooling needs a
second shape: *operations* such as merge (N inputs → 1 output) and page-range
extraction (1 input → 1 subset output). Stirling-PDF-class tools ship these
without any output proof; our differentiator is proving the result (page-count
conservation), so the design goal is to route operations through the existing
Plan → execute → validate → no-clobber-commit pipeline with as little model
surgery as possible.

## Decision

1. **`PlanRequest.operation: Option<String>`** (serde default `None`). `None`
   keeps the request a format conversion — every existing caller is untouched.
   Launch values: `"pdf-merge"` and `"pdf-extract"`.
2. **Merge is the only multi-input operation.** `PlanRequest.inputs:
   Vec<PathBuf>` (serde default empty) carries the ordered additional inputs;
   `output_path`/`input` semantics are unchanged. The merged Plan's
   `input_fingerprint` is a deterministic joint digest (ordered blake3 of the
   per-input fast fingerprints), and the Plan carries a new optional
   `input_manifest: Vec<ArtifactSummary>` so reports can name every source.
3. **Extraction stays single-input/single-output.** "Split into N files" is
   deferred: a `page_range` argument (e.g. `1-3,7`) selects a subset into one
   new PDF, which covers the dominant use and fits the existing single-output
   commit, conflict, and recovery machinery unchanged.
4. **A separate dispatch entry, not a lane in `required_engines`.** Operations
   are routed by `(operation, input formats)` in `workflow::prepare_operation`,
   parallel to `prepare_conversion`. The format matrix keeps describing
   conversions; operations get their own capability surface later, once more
   than two exist.
5. **Engine lane:** `qpdf` executes (`--empty --pages input1 1-z input2 … --
   out.pdf` for merge, `--pages input RANGE -- out.pdf` for extraction);
   `pdfinfo` validates. qpdf stays system-discovered (Apache-2.0, inventory
   row "preferred default structural PDF candidate"); no bundling change.
6. **Validation is conservation-based, both required:**
   - `PDF_MERGE_PAGE_COUNT`: probed output pages == sum of probed input pages.
   - `PDF_EXTRACT_PAGE_COUNT`: probed output pages == requested range length,
     and page dimensions sample-match the source.
   Both run `pdfinfo` on the *output* after execution — the proof is measured,
   not assumed, mirroring the browser-lane posture of ADR-0012.
7. **Rejection rules:** inputs must all be PDFs with parseable page counts;
   merge refuses a single input; extraction refuses empty/overshooting ranges
   (`Unsupported`/`InputInvalid`), keeping every typed-error contract intact.

## Consequences

- Positive: merge/extraction reuse staging, cancellation, no-clobber commit,
  reports, and the durable queue; each new operation is one plan function plus
  checks, and later operations (rotate, encrypt) extend `operation` instead of
  reshaping the model.
- Negative: two dispatch paths to keep coherent (conversion vs operation), and
   the CLI/UI gain an `operation` surface; the job store's single-input
   identity means a merged job revalidates against the joint digest only.
- Follow-ups: surface operations in the capability snapshot and desktop UI;
   wire `qpdf` into `doctor` discovery (it is already listed); add a
   `test_pdf_ops_sandbox.ps1` before any Certified claim.
