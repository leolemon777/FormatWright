# ADR-0004: Certified engine packs are separate from the application

- Status: Accepted; amended after Windows release usability audit on 2026-08-12
- Date: 2026-08-10
- Owners: FormatWright maintainers
- Related requirements: FW-FR-050 through FW-FR-053

## Context

Conversion engines are large and carry different licenses, codecs, patent considerations, build flags, and update urgency. Bundling every engine into the main installer would increase size and make provenance difficult to explain.

## Decision

FormatWright treats certified engine packs as a separate trust and versioning boundary even when a Starter pack is embedded in, or shipped beside, the desktop installer. A pack contains a signed manifest, hashes, platform and architecture, source URL, source-offer information when required, build configuration, license notices, capabilities, and compatibility range.

Network downloads are always user initiated. Offline import and same-media Starter delivery are supported. Production Release resolves only exact binaries from an activated, verified pack; unknown system engines, ambient PATH, `.cmd` and `.bat` wrappers are never executed, including in expert mode. Development builds may discover system engines as explicit import candidates.

Doctor produces one identity-bound capability snapshot. Planner, UI route recommendations and the execution backend all use that snapshot; missing capabilities are disabled with an installation/import action.

## Consequences

- The base application can remain smaller while a Windows Starter Offline Bundle is still genuinely usable.
- Offline bundles may be one installer payload or a matched application + pack artifact set, but activation semantics are identical.
- A pack registry and revocation mechanism are required before Public Beta.
- Job plans pin the exact engine identity used.
- Release behavior is deterministic and cannot change because another development tool modified PATH.

## Verification

- Tampered pack tests must fail before execution.
- Release automation generates and checks pack manifests.
- License review is a blocking release checklist item.
- A clean-machine offline test must finish Core, PDF and Media Starter conversions with a polluted/empty PATH.
- UI route availability must match backend capability checks.

## Revisit when

Measurement proves that a different pack/install boundary improves first-run success without weakening provenance, rollback, license or capability guarantees.
