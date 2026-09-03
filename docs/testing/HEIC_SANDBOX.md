# HEIC-to-JPEG/PNG Sandbox Tests

- Status: Phase 2 Windows development evidence
- Updated: 2026-08-10
- Workflow: GW-01 alpha slice

## Scope

`scripts/test_heic_sandbox.ps1` reconstructs a fixed-hash 64×64 HEVC HEIC fixture from the upstream libheif fuzz corpus inside a unique ignored case directory. It exercises content-first inspection, an explainable libheif fallback Plan, JPEG/PNG conversion, independent output validation, typed negative paths, atomic commit, cancellation, and durable retry through the public CLI.

This promotes only the covered libheif fallback to Experimental on Windows. The intended libvips primary adapter is not claimed: the official Windows 8.18.4 “all” package advertised `heifload`, but a real HEVC fixture failed with “compression format has not been built in.”

## Security and execution contract

- `heif-convert` and ffprobe are selected by exact hashed paths and launched with typed arguments and no shell.
- The Plan records single-primary selection, JPEG quality or lossless PNG, HEIF transformation handling, metadata drop, no resize, and Network Deny.
- EXIF/XMP sidecars, auxiliary images, thumbnails, depth data, and additional image items are not requested. A conversion producing anything other than the single expected staged image is blocked.
- Output stays in a deterministic same-parent staged directory until ffprobe validation completes; cancellation, failure, and recovery remove that exact directory.
- libheif security limits remain enabled; Anole never passes `--disable-limits`.

## Directly verified assertions

- A real HEVC Main Still Picture HEIC is content-detected at 64×64; a `.bin` copy is also detected and reports extension mismatch.
- JPEG quality 82 and lossless PNG each complete with a Pass report, expected codec/format, one stream, preserved dimensions, and no alpha-policy violation.
- Independent ffprobe reopens both outputs; Pillow decodes every pixel, confirms format/dimensions, and proves the fixture is not a constant-color blank.
- Representative JPEG and PNG outputs were visually reviewed and preserve the source color gradient.
- Invalid JPEG quality, PNG quality, unsupported resize, truncated input, and a pre-existing output are rejected with typed errors.
- Timeout-zero cancellation commits nothing; public queue retry revalidates input and the pinned libheif identity, then completes the same Plan.
- Source bytes remain unchanged and no staged workspace remains.

## Fixture provenance

The 499-byte fixture is `strukturag/libheif/fuzzing/data/corpus/colors-no-alpha.heic`, reconstructed only at test time from base64. Its SHA-256 is `76f82ffc717a647b1c9c2551e5ea0545832a2d3216c7540f7e5b092282a04b63`. The libheif library is LGPL-3.0; its sample applications are MIT. Release corpus licensing must remain recorded separately from engine licensing.

## Run

~~~powershell
cargo build -p formatwright-cli
pwsh -NoProfile -File scripts/test_heic_sandbox.ps1 `
  -Python <python-with-pillow> `
  -HeifConvert <heif-convert.exe> `
  -Ffprobe <ffprobe.exe>
~~~

The recorded Windows run `heic-suite-485d9439e86e4dc59eda6642540f1569` passed every assertion with libheif 1.23.0/libde265 1.1.1 and ffprobe 8.1.1.

## Remaining certification work

- A libvips Windows/macOS/Linux pack whose capability preflight and real HEVC fixture both pass; explicit engine fallback selection in the UI.
- Rotation/mirror/crop properties, alpha, HDR/10/12-bit, ICC/NCLX, multiple images, Live Photo-related metadata, depth/auxiliary data, thumbnails, malformed corpus, and very large images.
- Pixel-reference thresholds, metadata-policy expansion, resize support, OS sandbox containment, zero-network canary, signed packs, SBOM/license notices, and HEVC patent/regional review.
