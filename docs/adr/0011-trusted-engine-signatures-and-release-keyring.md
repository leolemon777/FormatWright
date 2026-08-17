# ADR-0011: Trusted engine signatures with an embedded release keyring

- Status: Accepted
- Date: 2026-08-15
- Owners: FormatWright maintainers
- Related requirements: FW-FR-051, FW-FR-052, FW-FR-053, SPEC_PLAN §9.4, `docs/security/ENGINE_SUPPLY_CHAIN.md`, R-008/R-009 closure path

## Context

Every engine pack file is already hash-pinned by its manifest and re-verified before and after atomic installation. Nothing, however, proves that a *manifest itself* was produced by FormatWright: `signature` on `EngineManifest` and `signature_present` on `VerifiedEnginePack` are informational only, so a hand-crafted or attacker-modified manifest with self-consistent hashes still verifies. The Public Beta gate (`docs/MASTER_EXECUTION_PLAN.md` Gate 3) requires trusted signature verification, a release keyring with revocation, downgrade blocking, and activation rollback.

The engine store already keeps content-addressed copies per `(engine_id, version, manifest_sha256)`, so multiple versions physically coexist. The desktop registry, however, keeps exactly one active entry per engine and deletes superseded entries, and startup activation failures are ignored silently — there is no rollback path and no failure evidence.

Constraints:

- Verification must work fully offline inside the local application.
- The signing scheme must not add Shell, network, or native (C) dependencies.
- Dev-discovered and imported packs without signatures must keep working, but must never be promoted to a trusted state.
- Real release key material and the signing ceremony are not available yet (frozen product decision); the implementation must be testable with generated keys.

## Decision

1. **Canonical signing payload.** The signed bytes are `formatwright_engine_sdk::canonical_manifest_bytes(manifest)`: the compact `serde_json` serialization of the manifest struct with `signature` set to `None` (the payload therefore contains a `"signature":null` placeholder and never any signature value). Struct field order is the schema order and is stable — note this may differ from the field order a builder happens to write into the manifest file; map-typed fields (for example capability `constraints`) serialize with sorted keys, matching `serde`'s `BTreeMap` behavior. The payload never contains an install absolute path (manifests already reject absolute paths).

2. **Signature envelope.** The existing v1 shape is kept: `signature: { algorithm: "ed25519", key_id, value }`, where `value` is the lowercase hex encoding of the 64-byte Ed25519 signature over the canonical bytes. No timestamp lives in the manifest; validity is a property of the key.

3. **Release keyring.** A versioned JSON document `ReleaseKeyring { schema_version: 1, keys, revocations }` with `ReleaseKey { key_id, algorithm: "ed25519", purpose: "engine-manifest", public_key: hex(32 bytes), valid_from_unix_ms, valid_until_unix_ms }` and `KeyRevocation { key_id, revoked_unix_ms, reason }`. For v0.1 the keyring is compiled into the application and rotates with application releases; a runtime keyring file is accepted only through an explicit development argument and never silently overrides the embedded keyring.

4. **Trust states.** `formatwright_engine_sdk::verify_manifest_signature(manifest, &keyring, now_unix_ms)` returns exactly one of: `Trusted { key_id }`, `Unsigned`, `UnknownKey`, `Revoked { key_id }`, `Expired { key_id }`, `InvalidSignature`. Evaluation is deterministic and ordered: missing signature → `Unsigned`; key absent from keyring → `UnknownKey`; key revoked → `Revoked`; now outside key validity → `Expired`; signature mismatch (tamper or a signature reused over a different manifest, which covers wrong-target reuse) → `InvalidSignature`; otherwise `Trusted`. `DowngradeBlocked` is not a crypto state; it is produced by the versioned registry (item 6).

5. **Promotion rule.** A `Trusted` signature alone does **not** set capability `Certification::Certified` and does not modify `sources.json.review_status`. Certification requires a trusted signature **and** the human-signed supply-chain review (ADR gate from `docs/security/ENGINE_SUPPLY_CHAIN.md`, batch C). Packs display as "signature trusted, review incomplete" until both hold. Hash completeness or `signature_present` alone never promotes any state.

6. **Versioned activation and rollback** (implemented in `crates/core/src/engine_registry.rs`). The registry keeps one atomic active pointer per engine in the existing `EngineRegistryIdentity` format; rollback history is derived from the content-addressed store, which retains every installed `<engine_id>/<version>/<manifest_sha256>` directory, so no history files or migrations exist. At startup, `EngineRegistry::recover` re-verifies each active version, falls back automatically to the newest still-verifiable installed version instead of skipping silently, promotes the fallback to the active pointer, and reports per-engine `Activated`/`FellBack`/`Failed` outcomes into the desktop recovery summary. `EngineRegistry::rollback` re-verifies the target first and blocks downgrades unless explicitly authorized; running jobs keep the exact in-process paths their immutable plans already resolved.

## Consequences

- Manifest tampering, signature reuse across manifests, revoked keys, expired keys, and unknown keys each produce a distinct, testable state instead of a boolean.
- Ed25519 verification is pure Rust (`ed25519-dalek`), offline, and fits the existing cargo-deny license allowlist; signing requires no online service.
- Until real release keys exist, all shipped packs remain `Unsigned`/`Unverified`; the Starter is unaffected but also not trusted, which is today's truth.
- Key rotation requires an application release; an updateable keyring channel is deliberately out of scope for v0.1 and recorded under "Revisit when".
- The registry change removes silent startup skipping, which surfaces previously hidden verification failures as user-visible recovery actions.

## Verification

- engine-sdk unit tests: sign/verify roundtrip; tampered canonical bytes → `InvalidSignature`; signature replayed over a different manifest → `InvalidSignature`; revoked key → `Revoked`; expired key → `Expired`; unknown key → `UnknownKey`; absent signature → `Unsigned`; `derive_engine_certification` requires Trusted + complete review.
- core: `verify_engine_pack_with_keyring` threads the trust state into `VerifiedEnginePack` while preserving all existing hash/supply-chain checks. Activation evaluates the embedded keyring (currently empty) and registers certification/trust/review for Doctor, Planner, and reports.
- CLI: `formatwright engines verify <manifest> --keyring <keyring.json>` prints trust, review, and derived certification, and fails closed on non-trusted states when a keyring is supplied. Doctor prints the same derived certification.
- Desktop Engines/Plan/Report surfaces show localized certification and never promote a trusted-but-unreviewed pack.

## Revisit when

- Real release key material and a signing ceremony exist (key generation, storage, dual control).
- A signed keyring update channel (with its own trust anchor) is needed for rotation without an app release.
- Hardware/HSM signing or a post-quantum scheme becomes a requirement.
- The engine pack format gains fields that cannot serialize deterministically.
