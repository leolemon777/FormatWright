# Audio Sandbox Tests

- Status: Phase 2 Windows development evidence
- Updated: 2026-08-10
- Workflows: GW-05 and GW-07

## Scope

`scripts/test_audio_sandbox.ps1` generates all fixtures locally and exercises the public CLI through Inspect, Plan, Execute, Validate, and staged commit. It does not turn GW-05 or GW-07 into Certified support; the complete golden corpus and cross-platform matrix remain required.

## Covered assertions

- A video with two language-tagged AAC tracks cannot become one MP3 while preserve-all is active.
- `--audio-stream INDEX --allow-stream-drop` records and converts the selected absolute stream index.
- AAC in a video is transcoded to MP3 and independently detected as MP3.
- AAC is remuxed into M4A without audio re-encoding.
- FLAC to PCM WAV is classified as lossless and independently detected as `pcm_s16le`.
- A FLAC file renamed to `.bin` is still identified by its header and emits an extension mismatch.
- A video without audio is rejected during planning.
- Inputs remain unchanged, validation reports pass, durable jobs complete, and no staged files remain.

## Run

~~~powershell
cargo build -p formatwright-cli
pwsh -NoProfile -File scripts/test_audio_sandbox.ps1
~~~

Machine-readable evidence is written to a unique `.artifacts/audio-suite-*` directory.

The latest recorded Windows run `audio-suite-68c021ddb46945529429fe3841c9b944` passed all assertions: MP3, M4A, and WAV reports were Pass; independent codecs were `mp3`, `aac`, and `pcm_s16le`; and the M4A Plan was a remux.

## Remaining certification work

- MP3, M4A, WAV, FLAC, OGG, Opus, and AAC fixtures on Windows, macOS, and Linux.
- Cover art, tag allowlists, gapless metadata, multichannel layouts, resampling, downmixing, and bit-depth constraints.
- Cancellation and malformed-input cases specific to the audio adapter; the common runner already has Windows cancellation/crash evidence.
- Engine capability preflight for optional `libmp3lame`, `libvorbis`, and `libopus` encoders.
