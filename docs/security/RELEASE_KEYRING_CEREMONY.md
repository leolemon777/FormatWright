# Release Keyring Ceremony (ADR-0011)

- Status: Runbook ready; awaiting the owner decisions in `docs/release/PRODUCT_DECISIONS.md` #10
- Created: 2026-08-16
- Tooling: `scripts/release_keyring_tool.py` (PyNaCl)

## Principles

- The Ed25519 release seed is the single root of engine-pack trust. It is generated **offline**, never touches a networked machine while unencrypted, and is never committed.
- Public keyrings are embedded in application releases; rotation ships with an application release (ADR-0011).
- Two-person integrity: one person generates, a second verifies and takes custody of one backup share.

## Ceremony steps

1. **Prepare** an offline (air-gapped) machine or a freshly booted live OS. Install python + pynacl from verified media, or copy `release_keyring_tool.py` plus a prepared keyring-entry review sheet.
2. **Generate**:
   ```text
   python release_keyring_tool.py keygen --key-id release-<half-year> \
       --valid-days 540 --seed-out seed.txt --keyring-out keyring.json
   ```
3. **Record** the `key_id`, `public_key`, creation time, location, and participants in the ceremony log (append a row below; this file is the public record — never paste the seed).
4. **Back up the seed** 2-of-3: split the hex seed into three shares (e.g. paper + two hardware tokens) such that any two reconstruct it. Store shares in separate locations/controllers.
5. **Destroy working copies** on the ceremony machine after backups are verified readable (sign a test manifest, verify with `formatwright engines verify --keyring keyring.json`).
6. **Publish** the public keyring entry: embed in the next application release and commit the public keyring to the repository (`keyring.json` is safe to commit; the seed is not).
7. **Sign packs** on the release machine (seed loaded from one backup for the duration, then wiped):
   ```text
   python release_keyring_tool.py sign --manifest <pack>/manifest.json --seed seed.txt --key-id release-<half-year>
   formatwright engines verify <pack>/manifest.json --keyring keyring.json   # must print Trusted
   ```
   Rebuild the Starter bundle afterwards so manifest/SBOM/source hashes stay consistent, and re-run the install smoke.

## Revocation

If a key is compromised: add a `KeyRevocation` entry (key_id, timestamp, reason) to every shipped keyring, ship an application release, and re-sign all packs with the successor key. Verification already fails closed on `Revoked` (ADR-0011); a revoked pack cannot be re-activated by the registry.

## Ceremony log

| Date | key_id | public_key (prefix) | Participants | Location | Backup custody |
|---|---|---|---|---|---|
| — | — | — | — | — | — |
