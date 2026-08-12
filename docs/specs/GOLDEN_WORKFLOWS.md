# Golden Workflows v0.1

- Status: Normative
- Version: 0.1
- Updated: 2026-08-10
- Parent: ../../SPEC_PLAN.md

## 1. Purpose

This document turns GW-01 through GW-12 into executable acceptance contracts. A workflow is not supported merely because an engine advertises the format. It becomes supported only when its required fixtures pass inspection, planning, execution, and validation on every platform listed in the support matrix.

## 2. Common contract

Every golden test must save:

- Input fixture ID, source, license, and hash.
- Input Probe JSON.
- User constraints.
- Expected Plan class and forbidden Plan classes.
- Pinned engine identity.
- Process exit evidence.
- Output Probe JSON.
- ValidationReport JSON.
- Expected final status.

Every workflow must include:

1. A normal small input.
2. A path containing spaces and Unicode.
3. A wrong-extension input.
4. A truncated or malformed input.
5. A destination conflict.
6. A user cancellation test.
7. An insufficient temporary-space simulation where practical.

Global invariants:

- No source file is modified.
- No existing output is overwritten without explicit policy.
- A failed or cancelled task never leaves a committed target.
- A successful output can be reopened by an independent probe where available.
- The report identifies every engine and build used.
- Conversion does not access the network under the default policy.

## 3. Fixture naming

~~~text
GW-01-HEIC-001-normal.heic
GW-01-HEIC-002-rotated.heic
GW-01-HEIC-003-icc.heic
GW-01-HEIC-004-wrong-extension.bin
GW-01-HEIC-005-truncated.heic
~~~

Generated fixtures use reproducible scripts. External fixtures require a manifest entry with a redistribution license. Large fixtures are generated or downloaded by an explicit test setup command and are not stored directly in Git.

## 4. Workflow contracts

### GW-01 — HEIC/HEIF to JPG or PNG

Required fixtures:

- Normal sRGB still image.
- EXIF orientation other than 1.
- Embedded ICC profile.
- Image with metadata fields selected for preservation and removal.
- HEIF image sequence or unsupported variant to prove an honest failure.

Expected planning:

- JPG is classified as lossy and cannot preserve alpha.
- PNG is selected when alpha preservation is a hard constraint.
- Orientation may be normalized into pixels only if the Plan records the metadata change.
- ICC preservation is enabled unless the user requests color conversion.

Acceptance:

- Output dimensions and displayed orientation match the input.
- ICC is preserved or a planned conversion is reported.
- EXIF behavior matches the selected metadata policy.
- A target incapable of a required property is blocked before execution.

Negative tests:

- Malformed HEIC fails during inspection or execution without a committed output.
- Unsupported sequence behavior returns an actionable error rather than exporting a single frame silently.

### GW-02 — PNG/JPG to WebP or AVIF

Required fixtures:

- Opaque photograph.
- Transparent PNG with soft alpha.
- Wide-gamut ICC image.
- Very large image that exercises bounded-memory processing.

Expected planning:

- Lossy and lossless modes are distinct capabilities.
- Alpha preservation is a hard constraint when present unless explicitly discarded.
- Resize occurs in the same image pipeline where the engine can avoid extra decode/encode cycles.

Acceptance:

- Dimensions match requested dimensions exactly.
- Alpha is preserved for supporting targets.
- Output decodes successfully and preserves the declared color behavior.
- Target-size mode reports whether the requested size was achieved.

Negative tests:

- JPG output is blocked when alpha preservation is required.
- Unsupported AVIF encoder is reported by Doctor and planning before execution.

### GW-03 — Recursive image batch resize/compress

Required fixtures:

- A nested directory with at least three levels.
- Duplicate stems with different extensions.
- Unicode and reserved-name edge cases.
- A generated 10,000-file corpus.
- A directory symlink cycle fixture.

Expected planning:

- The default does not traverse directory symlinks.
- Relative directory structure is preserved.
- Every output path is reserved before execution.
- Concurrency is assigned by the resource scheduler, not by loading all files into memory.

Acceptance:

- Input count, planned count, skipped count, completed count, warning count, and failed count reconcile.
- Pause stops scheduling new work and allows defined in-flight behavior.
- Resume skips outputs whose identity and validation evidence still match.
- A crash does not lose completed item state.

Negative tests:

- Conflicting output names follow the chosen conflict policy.
- An input changed after planning is re-inspected or blocked.

### GW-04 — MOV/MKV/AVI/WebM to MP4

Required fixtures:

- H.264 + AAC input compatible with MP4 remux.
- HEVC + AAC input with color metadata.
- VP9 or AV1 input requiring video transcode for the selected MP4 profile.
- Multiple audio tracks.
- Subtitle and chapter tracks.
- Rotation metadata.
- Generated 10GB media file.

Expected planning:

- Compatible H.264/H.265 and audio tracks use remux when all selected streams fit target constraints.
- Incompatible streams produce an explicit transcode or drop decision.
- Subtitle and chapter treatment is shown before execution.
- The 10GB path does not require a full application buffer or mandatory full pre-hash.

Acceptance:

- Remux plans contain zero video re-encode steps.
- Duration, selected track count, chapters, rotation, and color metadata satisfy VALIDATION_RULES.md.
- Cancellation terminates the full process tree and leaves no committed MP4.
- The control plane stays within the published memory gate.

Negative tests:

- An incompatible subtitle cannot disappear silently.
- A target on a different volume cannot be described as atomically committed unless the staging policy proves it.

### GW-05 — Video to MP3/M4A/WAV

Required fixtures:

- Single audio track video.
- Multiple language audio tracks.
- Video with attached cover art and metadata.
- Video with no audio.

Expected planning:

- The selected audio stream is explicit.
- Preserve-all mode produces separately named outputs when the target cannot contain multiple desired streams.
- WAV is identified as uncompressed PCM output.

Acceptance:

- Duration and channel layout satisfy the audio tolerance.
- Selected language and stream identity are recorded.
- Metadata and cover handling match the Plan.

Negative tests:

- A video with no audio is blocked during planning.
- Ambiguous multiple-audio selection prompts or uses a documented deterministic default.

### GW-06 — Video to GIF

Required fixtures:

- Short landscape clip.
- Clip requiring crop and scale.
- High-frame-rate clip.
- Clip with transparency where the engine supports it.

Expected planning:

- Start, end, crop, dimensions, frame rate, palette strategy, and looping are explicit.
- Target-size mode is represented as an iterative strategy with a bounded attempt count.

Acceptance:

- Output dimensions, approximate duration, frame count range, and loop setting match constraints.
- Target-size result reports achieved size and any quality compromise.
- Temporary palette artifacts are cleaned.

Negative tests:

- An invalid time range is rejected before engine launch.
- Target-size failure produces Warning or Fail according to the hard/soft constraint.

### GW-07 — Audio format conversion

Required fixtures:

- Tagged FLAC with cover art.
- Multichannel WAV.
- Variable-bitrate MP3.
- Gapless album track metadata.

Expected planning:

- Lossless-to-lossless and lossy-to-lossy paths are classified honestly.
- The planner does not label transcoding from MP3 to FLAC as quality recovery.
- Tags, cover art, sample rate, bit depth, and channel behavior are explicit.

Acceptance:

- Duration and channel layout satisfy the tolerance.
- Required tags and cover art are present.
- Planned resampling or downmixing is reported.

Negative tests:

- Unsupported bit-depth or channel constraints block before execution.
- Invalid audio fails without a committed target.

### GW-08 — DOCX/PPTX/XLSX to PDF

Required fixtures:

- Simple document using bundled fonts.
- Complex tables and page breaks.
- Missing-font case.
- RTL text.
- Presentation with transparency and embedded media poster frames.
- Spreadsheet with multiple print areas.

Expected planning:

- LibreOffice uses a per-job profile directory.
- Macros and external resources are disabled or explicitly controlled.
- Missing fonts are identified before or during conversion where possible.

Acceptance:

- Output opens and page count is consistent with the expected fixture contract.
- Page dimensions and orientation match.
- Missing fonts produce Warning.
- Visual drift evidence is attached where a comparable reference render exists.

Negative tests:

- Password-protected documents request a secret through the approved secret channel or fail clearly.
- A document requiring external resources cannot access the network under default policy.

### GW-09 — PDF to PNG or JPG

Required fixtures:

- Multi-page RGB PDF.
- Transparent page content.
- Mixed page sizes and rotations.
- Encrypted PDF.
- Malformed PDF.

Expected planning:

- Page selection, DPI, target dimensions, color mode, background, and naming are explicit.
- JPG warns when transparency is flattened.

Acceptance:

- Number of outputs equals selected page count.
- Per-page dimensions derive from page size and requested DPI within rounding rules.
- Rotation and background behavior match the Plan.

Negative tests:

- Incorrect PDF password never appears in logs or reports.
- A malformed page is reported individually without misreporting a complete batch.

### GW-10 — Markdown/HTML to PDF or DOCX

Required fixtures:

- Markdown with headings, tables, footnotes, code, math, and local images.
- HTML with local CSS and assets.
- Document referencing an external URL.
- Unicode and RTL document.

Expected planning:

- Input base directory and allowed resources are explicit.
- External HTTP resources are blocked by default.
- PDF engine choice and availability are shown.

Acceptance:

- Heading structure, local resources, links, and selected metadata are preserved.
- Missing resources produce actionable diagnostics.
- Output is validated by the appropriate document or PDF validator.

Negative tests:

- Path traversal outside the authorized resource root is blocked.
- External resources do not trigger network access under default policy.

### GW-11 — CSV/JSON/YAML/XML conversion

Required fixtures:

- Flat tabular data.
- Nested objects and arrays.
- Null, empty string, zero, false, and missing fields.
- Large integers, decimals, dates, Unicode, BOM, and alternate delimiters.
- Malformed and duplicate-key inputs.

Expected planning:

- Nested mapping, array expansion, type inference, delimiter, encoding, and null policy are explicit.
- Lossy flattening requires confirmation or a saved preset.

Acceptance:

- Record and field counts reconcile according to the declared mapping.
- Numeric precision and null distinctions satisfy the contract.
- Output parses independently.
- Ordering guarantees are stated, not implied.

Negative tests:

- Ambiguous nested conversion is blocked without a mapping policy.
- Duplicate-key behavior is deterministic and reported.

### GW-12 — Inspect and metadata clean

Required fixtures:

- Photo with EXIF, GPS, ICC, orientation, and thumbnail.
- Media with title, artist, chapters, and attached art.
- PDF with document metadata.
- File with no recognized metadata.

Expected planning:

- Inspect never modifies the source.
- Clean lists every field or category to remove.
- Pixel/media payload and structural metadata are distinguished.

Acceptance:

- Removed fields exactly match the Plan.
- Required content properties remain unchanged.
- The report records before/after metadata without exposing removed secrets by default.

Negative tests:

- Unknown metadata is retained unless an explicit strip-all mode is selected.
- In-place mutation is not available in v0.1; output uses normal conflict and commit rules.

## 5. Release aggregation

- Golden fixtures: 100% pass on every supported platform.
- Extended real-world corpus: at least 95% successful conversion, with zero known silent corruption.
- Any unsupported engine/platform pair remains marked Experimental and is excluded from the supported count.
- A fixture expectation change requires review and a linked rationale; tests may not be weakened solely to make a release pass.

