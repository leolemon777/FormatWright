# Core Contract and Schema Specification

- Status: Normative baseline
- Version: 0.1
- Updated: 2026-08-10

## 1. Contract set

Anole publishes these machine contracts:

| Contract | Schema ID | Producer | Main consumers |
|---|---|---|---|
| Probe | urn:formatwright:schema:probe:v1 | Inspector | Planner, UI, CLI |
| Plan | urn:formatwright:schema:plan:v1 | Planner | Runner, UI, reports |
| Job Event | urn:formatwright:schema:job-event:v1 | Queue/Runner | UI, CLI, API |
| Validation Report | urn:formatwright:schema:validation-report:v1 | Validators | UI, CLI, audit |
| Engine Manifest | urn:formatwright:schema:engine-manifest:v1 | Pack builder/plugin | Registry, Doctor |
| Preset Library | urn:formatwright:schema:preset-library:v1 | Desktop/editor | Desktop, CLI/API later |
| Application State Manifest | urn:formatwright:schema:application-state-manifest:v1 | MaintenanceService | CLI/Desktop restore, migration tools |
| Application Settings | urn:formatwright:schema:application-settings:v1 | Desktop settings | Desktop, application-state bundle |

Canonical JSON Schemas live under schemas/. Rust types and examples must validate against them in CI.

## 2. Compatibility

- Schema IDs contain a major version.
- Adding an optional field is backward compatible.
- Removing a field, changing meaning, changing type, or tightening an accepted enum requires a new major version.
- Consumers ignore unknown fields but preserve them when acting as a pass-through.
- Every serialized object includes schema_version.
- Human-readable messages are not stable API; stable codes are.

## 3. Probe requirements

Probe includes:

- Artifact identity and observed path.
- Observed format and confidence.
- Container properties.
- Typed streams/pages/records.
- Metadata keys with redaction classification.
- Probe engine evidence.
- Warnings and inconsistencies, including extension mismatch.

Probe does not claim properties it did not inspect.

## 4. Plan requirements

Plan includes:

- Plan ID and deterministic hash.
- Input Probe reference.
- Target constraints.
- Ordered typed steps.
- Exact engine selector.
- Loss class for each step.
- Preserved, changed, dropped, and unknown properties.
- Estimated resources and temporary space.
- Required validators.
- Security/network policy.
- Output reservation proposal.

The hash excludes volatile human text and timestamps.

## 5. Job Event requirements

Event includes:

- Event ID.
- Job ID.
- Monotonic sequence number per job.
- Timestamp.
- Stable event code.
- Previous and next state when applicable.
- Progress snapshot with explicit units.
- Redacted diagnostic fields.

UI event coalescing cannot alter durable state-transition events.

## 6. Validation Report requirements

Report includes:

- Input and output artifact summaries.
- Plan and engine provenance.
- Ordered validation checks.
- Aggregate status.
- Expected and observed values.
- Evidence source.
- Intentional changes.
- Unknown or unavailable checks.
- Reproduction information.
- Redaction policy.

## 7. Engine Manifest requirements

Manifest includes:

- Engine ID, version, platform, architecture.
- Executable relative paths and hashes.
- Source and license metadata.
- Build configuration.
- Capability declarations.
- Protocol version.
- Anole compatibility range.
- Pack signature and manifest hash.

Capability claims are intersected with runtime Doctor inspection. A manifest cannot force support for a capability the binary does not expose.

## 8. Preset Library requirements

The portable preset library includes a versioned envelope and bounded named entries with stable UUIDs. Presets contain typed conversion settings only: target, quality, dimensions, DPI, color mode, and stream-preservation policy. Paths, secrets, arbitrary engine arguments, and shell strings are forbidden. Imports validate completely before merge and reject duplicate IDs or case-insensitive names.

## 9. Application State Manifest requirements

The application-state manifest enumerates the exact ZIP member set, component type, bounded byte size, and lowercase SHA-256 for every payload. Archive paths are fixed or single-level JSON names; undeclared, duplicate, nested, absolute, traversal, link, oversized, or hash-mismatched members are rejected before live state changes. Engine registry entries carry identity/path references only; third-party binaries are never copied blindly.

## 10. Stable error model

Errors contain:

- code
- category
- stage
- retryable
- user_action
- redacted diagnostic context
- underlying engine code where safe

Categories:

- INPUT_INVALID
- INPUT_CHANGED
- UNSUPPORTED
- ENGINE_MISSING
- ENGINE_INCOMPATIBLE
- POLICY_BLOCKED
- RESOURCE_EXHAUSTED
- EXECUTION_FAILED
- CANCELLED
- VALIDATION_FAILED
- OUTPUT_CONFLICT
- STORAGE_FAILED
- INTERNAL

## 11. Traceability

Every schema field must map to:

- A Rust field or explicit computed field.
- At least one producer test.
- At least one consumer test.
- A redaction classification.
