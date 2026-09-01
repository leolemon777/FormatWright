# Release Checklist

## Product scope

- [ ] SPEC_PLAN and supporting specs match implemented behavior.
- [ ] Support matrix contains no unproven Certified claim.
- [ ] All 12 golden workflows pass on each claimed platform.
- [ ] Extended corpus meets the published success target.
- [ ] No P0/P1 defect remains open.

## Correctness and recovery

- [ ] Inspect, Plan, Execute, Validate evidence saved.
- [ ] Remux preference tests pass.
- [ ] Destination race and no-silent-overwrite tests pass.
- [ ] Crash matrix has no false completion.
- [ ] 10GB and 10,000-file gates pass.
- [ ] Schema compatibility tests pass.

## Security and privacy

- [ ] Threat model reviewed against current code.
- [ ] Command injection and path traversal tests pass.
- [ ] Secret redaction tests pass.
- [ ] Zero-network suite passes.
- [ ] Dependency and engine vulnerability review complete.
- [ ] `python scripts/audit_dependencies.py` reports zero locked-dependency vulnerabilities.
- [ ] `cargo deny check` passes advisory, ban, license, and source policy.
- [ ] No revoked engine pack included.

## Licensing and supply chain

- [ ] Application license and notices included.
- [ ] Every engine pack has license, source, build flags, hashes, signature, and SBOM.
- [ ] FFmpeg build configuration reviewed.
- [ ] No nonfree or unapproved component included.

## Packaging

- [ ] Windows Starter Core/PDF/Media pack is staged from locked manifests and activated through the verified registry.
- [ ] Production locator rejects ambient PATH, development cache, `.cmd`, and `.bat`; polluted-PATH negative tests pass.
- [ ] UI recommendations, Planner and backend share the same identity-bound capability snapshot.
- [ ] Windows artifact built and signed.
- [ ] Offline NSIS install/start/uninstall smoke passes and `SHA256SUMS` is generated.
- [ ] macOS artifact signed and notarized.
- [ ] Linux artifact and checksums built.
- [ ] Portable and offline-bundle behavior verified.
- [ ] Upgrade and rollback tests pass.
- [ ] On a clean machine with no system engines and no network, installation plus Core/PDF/Media real conversions pass; reports and outputs are verified.

## Documentation

- [ ] Install, first-run, offline, Doctor, conversion, recovery, and troubleshooting documented.
- [ ] Privacy and network behavior documented.
- [ ] Supported formats and limitations published.
- [ ] Changelog and migration notes published.

## Release evidence

- [ ] Git tag signed.
- [ ] Release SHA-256 published.
- [ ] SBOM attached.
- [ ] Test report attached.
- [ ] Engine certification records attached.

Generate the application dependency SBOM with `python scripts/generate_sbom.py`. Set `SOURCE_DATE_EPOCH` to the signed release commit timestamp for reproducible creation metadata. This application SBOM does not replace the separate SBOM required inside every engine pack.
- [ ] Updater release keypair (not the dev keypair) is set via CI secrets and the matching `pubkey` is pinned in `tauri.conf.json` before signing; see [UPDATER.md](UPDATER.md).
