# GIF Sandbox Tests

- Status: Phase 2 Windows development evidence
- Updated: 2026-08-10
- Workflow: GW-06

## Scope

`scripts/test_gif_sandbox.ps1` generates a four-second video/audio fixture, runs the public CLI through Inspect, Plan, Execute, Validate, and commit, and independently probes the resulting GIF.

The Plan makes start, duration, width, frame rate, loop count, palette size, and dithering explicit. Numeric values are validated before FFmpeg starts and are mapped through typed argument positions rather than a shell command.

## Covered assertions

- A 500 ms start and 1,500 ms duration produce a GIF within the 250 ms transcode tolerance.
- A requested 240-pixel width preserves aspect ratio at the expected even height of 136 pixels.
- Twelve frames per second produces 17–19 independently counted frames for the selected duration.
- The output independently opens as a GIF with the GIF video codec and no audio stream.
- Zero duration, more than 60 fps, and a start beyond input duration are blocked in planning.
- A GIF renamed to `.bin` is still detected by its header.
- Source hash remains unchanged and no staged output remains.

## Run

~~~powershell
cargo build -p formatwright-cli
pwsh -NoProfile -File scripts/test_gif_sandbox.ps1
~~~

Machine-readable evidence is written to `.artifacts/gif-suite-*/gif-sandbox-result.json`.

The latest recorded Windows run `gif-suite-f2500b5580e848cd9a82f11830f27266` passed all assertions with a 240×136 output, 18 independently counted frames, and a 1.5-second independently measured duration.

## Remaining certification work

- Crop, transparency-capable inputs, target-size bounded iteration, and target-size failure classification.
- Landscape/portrait/high-frame-rate golden fixtures on all supported platforms.
- Verification of the encoded loop extension with a second independent decoder.
- Cancellation and temporary palette cleanup under injected failure.
