# Application SBOM

- Status: Phase 5 development evidence
- Updated: 2026-08-10
- Format: SPDX 2.3 JSON

## Generation

~~~text
python scripts/generate_sbom.py
~~~

The generator reads the locked Cargo metadata graph and pnpm's production-license inventory. It emits stable package SPDX IDs, package URLs, declared licenses when available, Cargo dependency relationships, workspace `DESCRIBES` relationships, and a document namespace derived from both lockfiles. Set `SOURCE_DATE_EPOCH` to the signed release commit timestamp to make creation metadata reproducible.

The output defaults to ignored `dist/sbom.spdx.json`; release CI attaches it rather than committing a timestamped build artifact. The generator rejects duplicate package IDs, relationships to unknown IDs, and leaked Windows installation paths.

## Recorded Windows evidence

The 2026-08-10 development run after the dependency security update generated 527 unique packages and 1,571 valid relationships. PowerShell independently parsed the JSON and observed:

~~~text
SPDX-2.3
packages=527 unique_ids=527 relationships=1571 broken_relationships=0
contains_windows_absolute_path=false
~~~

## Boundary

This is a conservative workspace/application dependency inventory. Cargo metadata can include build or test-only packages; over-reporting is preferred to silently omitting a linked dependency. A release pipeline should eventually reconcile the inventory against each final target artifact.

Third-party conversion engines remain separate programs even when a Starter pack is embedded in the Windows installer. The current development candidate includes pinned PDF and Media packs with manifests, file hashes, provenance, and notices, but their complete transitive library inventory, source-offer review, trusted signature, and pack SBOM remain release gates under `docs/security/ENGINE_SUPPLY_CHAIN.md`.
