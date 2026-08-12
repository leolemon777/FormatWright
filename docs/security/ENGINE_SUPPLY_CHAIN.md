# Engine Supply Chain and Certification

- Status: Initial
- Version: 0.2
- Updated: 2026-08-12

## 1. Pack contents

~~~text
engine-pack/
  manifest.json
  manifest.sig
  bin/
  licenses/
  sources.json
  sbom.spdx.json
  README.txt
~~~

The manifest lists every executable and shared library hash.

For development and system-engine discovery, an exact executable can be selected with `FORMATWRIGHT_ENGINE_<NAME>` (for example `FORMATWRIGHT_ENGINE_PDFTOPPM`). Doctor resolves, version-probes, and hashes that exact file; queued jobs block if its version or SHA-256 differs from the immutable Plan. Release builds still require certified pack manifests rather than trusting an environment override.

Release resolution has one allowed source: the exact executable path within an activated, versioned Engine Registry pack. Ambient PATH, development caches, `.cmd`, and `.bat` wrappers are not production capabilities. Developer discovery may present an external executable as an import candidate, but it does not enter the production capability snapshot until pack identity, hashes, license metadata, architecture and protocol are verified.

The desktop development surface also supports an import-by-reference flow. It selects `manifest.json`, verifies schema/protocol, host target, safe canonical paths, executable hashes, and declared license files, then writes one immutable registry record per manifest hash under application data. On every application start, each registered manifest and binary is verified again before its exact paths are activated for Doctor or planning. Two manifests may not claim the same executable name. Moving or tampering with a pack makes that registry entry invalid instead of silently falling back. A present signature remains `Unverified` until the Phase 5 keyring performs cryptographic validation.

## 2. Provenance

For each artifact record:

- Upstream project and canonical source URL.
- Upstream version and source commit/tag.
- Download URL and original hash.
- Build environment and reproducible command where available.
- Enabled and disabled features.
- License and required notices/source offer.
- Platform signing result.
- Pack builder identity.

## 3. FFmpeg policy

- Default certified builds avoid nonfree configuration.
- Build configuration is captured from ffmpeg -buildconf.
- Runtime capabilities are captured from formats, codecs, encoders, decoders, filters, and protocols.
- GPL-enabled packs, if ever offered, are labeled and reviewed separately.
- Patent and regional notes are part of release metadata.

## 3.1 LibreOffice development evidence

The GW-08 Windows development run used the official LibreOffice 26.2.5 x86-64 MSI, administratively extracted into the ignored local `.devtools` directory rather than installed system-wide. The downloaded MSI SHA-256 was `f15ba07bfcb0186986cf3171063506f5d207c11f8cc051ba0d135209e9e915f9`, matching the upstream checksum. The selected `soffice.com` reported `LibreOffice 26.2.5.2` and SHA-256 `fe41a4eb77ba51610f10bcbba2d849fbaec9f63a1fcbeda5f32d629bb8c49316`.

This is reproducible development provenance, not a certified engine pack. Release packaging still requires the complete manifest, transitive library hashes, MPL notices/source obligations, signature, SBOM, and platform fixture matrix described in this document.

## 3.2 HEIC development evidence

The GW-01 Windows run used libheif 1.23.0 `heif-convert` as an explicit development fallback. The selected executable SHA-256 was `032d453e10a26c71051bddf77d79ea954b70cc5abbf2dbbb7c8d92bd9c5bc3e5`; runtime preflight reported the libde265 1.1.1 HEVC decoder. libheif is LGPL-3.0, and release redistribution must include the corresponding notices/source mechanism plus review of HEVC patent and regional obligations.

The official libvips 8.18.4 Windows x64 “all” asset was also checked. Its archive SHA-256 `95a56455ac525c9cb64865804322bbacad07021ded8ec49327fa3e392b91935b` matched the GitHub release digest, and `vips.exe` SHA-256 was `46e25b86a0695f4b03c66108d30eb68b2a8a36fb551e384ff28c3b8c06caccab`. Although that build advertised `heifload`, a real HEVC HEIC decode failed because HEVC support was not built in. It is therefore not claimed as the GW-01 Windows engine until capability preflight and the real fixture pass.

## 4. Pack lifecycle

States:

- draft
- tested
- certified
- deprecated
- revoked

A job pins a certified pack identity. New jobs may migrate after Doctor and fixture checks. Interrupted jobs never silently switch packs.

The Windows v0.1 Starter implementation now contains Core (built-in structured conversions), PDF (pinned Poppler tools and declared runtime files), and Media (pinned FFmpeg/ffprobe). Document and Image remain optional packs until their redistribution and capability matrices pass. The installer carries Starter resources directly; first startup verifies, copies, and activates them through the versioned atomic registry path. These packs remain development/unverified until the license, source-offer, engine SBOM, signature/keyring, clean-VM, upgrade, rollback, and revocation gates pass.

## 5. Updates and rollback

- Security revocation blocks new execution and explains the reason.
- Offline environments can import a signed revocation/update bundle.
- At least one prior compatible certified pack remains available unless revoked.
- Downgrade cannot read a newer incompatible job schema without an explicit safe path.

## 6. License gate

Before certification:

- License identified.
- Distribution mode reviewed.
- Required source and notices included.
- Build flags reviewed.
- Interaction with Apache-licensed application documented.
- Commercial/patent uncertainty recorded.

Ghostscript is excluded from the default v0.1 pack pending a specific license decision.

## 7. Runtime verification

At engine launch:

- Resolve the exact certified path.
- Verify manifest and binary hash according to cache policy.
- Compare runtime version/build output.
- Intersect declared and observed capabilities.
- Record identity in Plan and ValidationReport.

## 8. CI evidence

- Tampered manifest rejected.
- Tampered binary rejected.
- Wrong architecture rejected.
- Unsupported protocol version rejected.
- Revoked pack rejected.
- License files present.
- SBOM generated and parseable.

The Apache application dependency inventory is generated separately with `scripts/generate_sbom.py` as SPDX 2.3 JSON. It covers locked Cargo packages and production pnpm packages without embedding local installation paths. Every distributable engine pack still requires its own transitive binary/library SBOM; the application inventory must not be presented as engine coverage.
