# Structured-data Sandbox Tests

- Status: Phase 2 Windows development evidence
- Updated: 2026-08-10
- Workflow: GW-11

## Scope

`scripts/test_structured_sandbox.ps1` creates CSV, JSON, YAML, and XML fixtures locally and exercises the public CLI through Inspect, Plan, Execute, independent re-inspection, Validate, and same-directory staged commit. The native Rust adapter does not invoke a shell or require an external format engine.

This evidence promotes GW-11 to Experimental on Windows. It is not Certified until the complete golden corpus and cross-platform matrix pass.

## Covered assertions

- JSON to YAML preserves a canonical semantic digest containing a 64-bit integer beyond JavaScript's exactly representable range, booleans, nulls, missing fields, and Unicode.
- CSV to JSON preserves quoted commas, escaped quotes, embedded newlines, empty strings, Unicode, record count, and field inventory.
- XML to JSON preserves predefined entity text and Unicode under the strict `records/record/field` mapping.
- Scalar/null/missing distinctions cannot be mapped to CSV by default; `--allow-lossy-data` produces a Warning report rather than a silent Pass.
- Nested JSON cannot be flattened without an explicit mapping, even when lossy scalar mapping is authorized.
- Duplicate JSON keys and XML DTDs are rejected deterministically.
- UTF-8 BOM input parses, while unmapped XML attributes are blocked instead of silently dropped.
- JSON renamed to `.bin` is detected from content and reports the extension mismatch.
- Existing outputs are not overwritten, inputs remain unchanged, and no staged files remain.

## Run

~~~powershell
cargo build -p formatwright-cli
pwsh -NoProfile -File scripts/test_structured_sandbox.ps1
~~~

Machine-readable evidence is written under a unique `.artifacts/structured-suite-*` directory.

The latest recorded Windows run `structured-suite-82ccfbacaed5421a93c09b69956f0e8a` passed every assertion. JSON→YAML, CSV→JSON, and XML→JSON reported Pass; the explicitly authorized JSON→CSV scalar mapping reported Warning.

## Current contract and remaining certification work

- JSON and YAML inputs are top-level arrays of record objects.
- The alpha parser has a hard 64 MiB input limit because its canonical semantic digest currently materializes records in memory; larger inputs fail with `RESOURCE_EXHAUSTED` rather than risking uncontrolled memory use.
- XML uses the deliberately narrow `records/record/field` shape. DTDs, custom entities, attributes, and implicit nesting are blocked.
- CSV uses UTF-8, a header row, comma delimiter, RFC-style quoting, and string values. Duplicate or empty headers are blocked.
- JSON/YAML field order is deterministic through lexicographic maps; record order is preserved.
- CSV/XML flattening of arrays or objects is unsupported. Future flattening requires a versioned mapping contract.
- Complete alternate-delimiter, decimal/date, malformed YAML/XML, non-UTF-8 encoding, cancellation, resource-limit, fuzz, and macOS/Linux coverage remains required.
