# Metadata-clean Sandbox Tests

- Status: Phase 2 Windows development evidence
- Updated: 2026-08-10
- Workflow: GW-12 media slice

## Scope

`scripts/test_metadata_sandbox.ps1` generates a tagged H.264/AAC Matroska fixture and exercises Inspect, a redacted metadata-clean Plan, stream-copy execution, independent ffprobe validation, and atomic commit through the public CLI.

The result promotes only the covered media-container slice of GW-12 to Experimental on Windows. Photo EXIF/GPS/ICC and PDF metadata need their type-specific adapters and fixtures before the full workflow can be promoted.

## Policy

- Known private or secret format-level keys are named in the Plan and removed.
- Container-structural keys are classified Public and retained or safely regenerated.
- Unrecognized keys are classified Unknown and retained by default.
- Values of removed metadata do not enter the Plan or validation report.
- Encoded payload streams use copy mode; in-place cleaning is prohibited.
- Chapters are removed by the current media policy and declared in the step arguments.

## Covered assertions

- Title, artist, and comment are classified private, explicitly listed, and absent after cleaning.
- A custom Unknown tag remains present with its value.
- Plan JSON does not contain removed title or artist values.
- H.264/AAC codecs and 320×180 dimensions remain unchanged.
- Output independently opens and the ValidationReport is Pass.
- In-place mutation and an existing output are blocked.
- Source bytes remain unchanged and no staged file remains.

## Run

~~~powershell
cargo build -p formatwright-cli
pwsh -NoProfile -File scripts/test_metadata_sandbox.ps1
~~~

The recorded Windows run `metadata-suite-4077fdba74ba44d0966102c2e95c0968` passed all assertions. It removed `ARTIST`, `COMMENT`, and `title`, retained `CUSTOM_TAG`, and preserved H.264/AAC payload identities.

## Remaining certification work

- EXIF, GPS, ICC, XMP, thumbnails, rotation/orientation, stream-level tags, cover art, and attachment policies.
- PDF document information and XMP through a PDF-specific adapter.
- Chapter fixtures and an explicit keep/remove chapter option.
- Malformed metadata, hostile keys, cross-platform behavior, and a broader privacy corpus.
