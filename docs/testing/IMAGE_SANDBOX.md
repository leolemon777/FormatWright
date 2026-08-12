# Image Sandbox Tests

- Status: Phase 2 Windows development evidence
- Updated: 2026-08-10
- Workflows: GW-01 implementation path, GW-02 verified slice

## Scope

`scripts/test_image_sandbox.ps1` generates deterministic images locally and drives the public CLI through content-first Inspect, explainable Plan, FFmpeg execution, independent ffprobe validation, and atomic staged commit. FFmpeg is an Experimental development adapter; libvips remains the intended primary certified image engine.

The passing fixture promotes the covered GW-02 PNG/JPEG → WebP/AVIF path to Experimental on Windows. HEIC/HEIF decoding is implemented through the same planner/runner contract but is not promoted without a licensed golden fixture. Directory batching (GW-03) remains separate scheduler work.

## Covered assertions

- PNG is classified as an image rather than video and PNG renamed to `.bin` is still detected from its signature.
- PNG → WebP records quality 88, scales 320×180 to 160×90, and validates codec and dimensions independently.
- JPEG → AVIF validates as an AVIF container carrying AV1.
- JPEG → PNG is classified as a lossless decoded-pixel encoding step; it does not claim to restore source quality.
- A transparent PNG cannot become JPEG without a future explicit background-composite policy.
- Transparent PNG → WebP preserves an alpha-bearing pixel format.
- Zero width and zero quality are blocked during planning.
- Existing outputs are not overwritten, source bytes remain unchanged, and no staged output remains.

## Run

~~~powershell
cargo build -p formatwright-cli
pwsh -NoProfile -File scripts/test_image_sandbox.ps1
~~~

The recorded Windows run `image-suite-9884309e7249436bbe2a47a637cd19d8` passed all assertions. WebP was independently detected at 160×90, AVIF carried AV1, and each committed output received a Pass report.

## Remaining certification work

- Licensed HEIC/HEIF inputs, EXIF orientation, ICC profiles, wide-gamut/HDR data, 16-bit images, animation rejection, malformed inputs, and metadata policy fixtures.
- User-selectable fit/crop, height, background composite, chroma subsampling, lossless WebP, and AVIF speed controls.
- libvips adapter parity and engine-pack capability preflight.
- macOS/Linux runs and a larger perceptual-quality corpus.
