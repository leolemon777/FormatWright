# Windows Starter Pack Evidence

- Status: implementation complete; release certification pending
- Updated: 2026-08-15
- Related defects: R-008, R-009, R-010

## Implemented vertical slice

The Windows x86-64 desktop bundle now embeds separate PDF and Media engine packs. On first Release startup, the desktop backend verifies every declared executable, runtime file, license notice, target, protocol, and manifest invariant; copies the pack into the versioned application-data engine store; atomically updates one active registry record per engine ID; and activates only the exact installed executable paths.

The frontend and backend both consume the same capability snapshot. Unsupported routes and routes with missing packs are disabled before Plan or execution. Release engine discovery ignores ambient `PATH` and development environment overrides.

## Pinned development inputs

| Pack | Upstream binary distribution | Archive SHA-256 | Manifest SHA-256 |
|---|---|---|---|
| PDF | Poppler Windows `26.02.0-0` | `993e4a94376ed712fafc7058d724ea0b943d118bbd2305cd9ed55174eb85cda5` | `e047b5e81f3f8abbc2329a91850ec570a718c7d0aed84c1016b9feefc88e894b` |
| Media | Gyan FFmpeg essentials `9.0` | `e6b54767a6065919048f1a098eb27211ca4e12b4348a05d88777a5855d0b6e71` | `5bc2643953fc4f80ed7ad5abd5e74a20b0270e67ab9e04c8aa527e6e4ddebc73` |

`scripts/prepare_windows_starter_pack.ps1` downloads or reuses only the pinned archives, verifies their hashes before extraction, and calls `scripts/build_windows_starter_pack.ps1`. The builder now creates per-pack SPDX 2.3 file inventories and explicit incomplete-review `sources.json` records, pins both in the Engine Manifest, and verifies the final SBOM before publication. A full rebuilt Starter contains 311 files and 243,777,389 bytes.

Two consecutive builds produced byte-identical supply-chain artifacts:

| Pack | Manifest SHA-256 | SBOM SHA-256 | sources.json SHA-256 | SBOM files |
|---|---|---|---|---:|
| PDF | `f5856f84c902dc45b038109f529ae8721b33da58ff6c9e9cdd93b0928ca9a498` | `8a8ae7125871893dbd86b18a0b702ea282cbe129d793669520de88cd3b812f63` | `7f5ccbe63e841bb4025d95c664bfd4876e1eb0b7b340dfbbb0735e3759ea60db` | 300 |
| Media | `1af7d4e7d2f8a229d7326ba821b9a2ba439102bc76f60866b639b7a824889810` | `715e474e6efe89bdd0984de059a62124269aa179e158e13bf562e09e30c977cc` | `0b343967f57c25c8152a44d72d46fe69263b1ac3d0aa0942df1a1fb77e4b0979` | 6 |

The bundle manifest remained `21f46f92f63ae9fc31a059b3139b4edcf27d2fa9b7b6522fc34f13cb43c48823`. Both supply-chain-bearing Engine Manifests passed the real CLI verifier. Core tests prove SBOM/source sidecars survive atomic installation and reject tampered hashes and wrong engine identities.

## Local evidence

- Both generated manifests pass `formatwright engines verify`.
- A Release desktop startup installed `formatwright-pdf` and `formatwright-media` into the versioned application-data store and wrote one active registry entry for each pack.
- The real 15-page ST508S manual converted to PNG at 72 DPI: 15 outputs, 1,974,527 bytes, validation `Pass`.
- The same manual converted to JPEG at quality 78 and 72 DPI: 15 outputs, 755,647 bytes, validation `Pass`.
- The pinned Media pack passed the GIF sandbox: 18 frames, 240×136, 1.5 seconds, independent ffprobe validation, source unchanged, and no staged output remaining.
- The built-in structured sandbox passed JSON→YAML, CSV→JSON, XML→JSON, semantic preservation, typed lossy authorization, hostile-input rejections, conflict handling, source immutability, and staged-output cleanup.
- The 2026-08-15 standard-configuration NSIS rebuild embedded the same Starter resources: installer 282,440,776 bytes, SHA-256 `a2d3861472b949f58fe9e6b17317f45d0d98d0102524432f8850074104ab5828`, byte-scan negative for test DevTools arguments, and the enhanced real-install smoke re-verified both packs and all four supply-chain sidecars before clean uninstall (`.artifacts/windows-explorer-installed-smoke/suite-14dd1eebb65946968758aeb25ebba1b3`).
- The Release UI conversion gate converted the same family PDF to PNG and JPEG through the real interface from per-format isolated processes: both `Pass` with 6/6 checks and deterministic `page-00000N` naming (`.artifacts/desktop-release-conversion/suite-433c9b950b894deda251b353f7e95a98`, see `DESKTOP_RELEASE_CONVERSION.md`).

The PDF run initially exposed R-010: Poppler raster dimensions use ceiling for fractional pixel sizes, while the validator used nearest rounding. The A4 page width at 72 DPI was incorrectly expected as 595 rather than the observed 596. The model now matches Poppler at 36, 72, and 144 DPI and has a regression test.

## Commands

~~~powershell
pwsh -NoProfile -File scripts/prepare_windows_starter_pack.ps1
cargo run -p formatwright-cli -- engines verify dist/engine-packs/windows-x86_64/starter/pdf/manifest.json
cargo run -p formatwright-cli -- engines verify dist/engine-packs/windows-x86_64/starter/media/manifest.json
pnpm --dir apps/desktop tauri build
pwsh -NoProfile -File scripts/test_gif_sandbox.ps1
pwsh -NoProfile -File scripts/test_structured_sandbox.ps1
~~~

## Certification limits

This is Windows development/release-candidate evidence, not a certified public release. R-008 and R-009 remain `Fixed`, not `Closed`, until all of the following pass:

- Offline installation and first launch in a clean Windows VM with no system conversion engines or development caches and with deliberately polluted `PATH`.
- PDF→PNG, PDF→JPEG, Media, and Core conversions through the installed Release UI in that VM.
- Complete transitive component attribution and license/source-offer review, including regional codec/patent review. The file-level SPDX inventory is implemented but is intentionally not labeled full legal certification.
- Trusted pack signatures, keyring verification, revocation, downgrade, upgrade, rollback, and half-install failure tests.
- Authenticode-signed application/installer verification and retained release evidence.
