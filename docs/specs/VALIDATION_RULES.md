# Validation Rules v0.1

- Status: Normative
- Version: 0.1
- Updated: 2026-08-10
- Related requirements: FW-FR-040 through FW-FR-042

## 1. Status model

- Pass: all hard checks pass and no unplanned material change is detected.
- Warning: output is usable but a soft constraint, expected loss, missing verifier, or material uncertainty remains.
- Fail: output cannot be opened, violates a hard constraint, lacks required content, or cannot be safely committed.
- Unknown: the check could not run. Unknown never upgrades to Pass.

Aggregate status is the worst required check. A planned lossy operation can Pass only when every loss is disclosed and all requested constraints hold.

## 2. Common checks

Every output:

1. Exists and is non-empty unless the target format permits empty output.
2. Opens with the primary probe.
3. Uses the requested target format, not only the requested extension.
4. Satisfies output path and overwrite policy.
5. Records engine and Plan identity.
6. Uses an independent secondary parser for release-gate fixtures where available.

## 3. Image rules

| Check | Pass | Warning | Fail |
|---|---|---|---|
| Dimensions | Exact requested dimensions | None | Any unplanned mismatch |
| Orientation | Displayed orientation matches | Metadata representation changed as planned | Visible orientation mismatch |
| Alpha | Preserved when target and policy require | Flattened as explicitly planned | Unplanned alpha loss |
| ICC | Byte-identical or declared conversion | Profile unsupported but disclosed | Required profile missing or wrong |
| Animation | Frame behavior matches Plan | Planned first-frame extraction | Silent frame loss |
| Decode | Full decode succeeds | Independent decoder unavailable | Decode error |

Lossy image visual metrics are diagnostic evidence, not a universal fidelity guarantee. Perceptual thresholds must be calibrated by fixture class before they can block a release.

## 4. Video rules

### 4.1 Duration tolerance

- Remux: absolute difference no greater than the larger of 50 milliseconds or one nominal frame.
- Transcode: absolute difference no greater than the larger of 250 milliseconds or two nominal frames.
- A larger intentional trim must be represented as a constraint, not tolerated as drift.

### 4.2 Stream rules

- Selected video, audio, subtitle, attachment, and data stream counts match the Plan.
- Any dropped stream is listed before execution and in the report.
- Resolution is exact unless a scaling constraint exists.
- Rational frame rate is exact for remux; transcoded output allows 0.1% relative drift only when variable-rate interpretation requires it.
- Rotation, HDR transfer, primaries, matrix, mastering data, and color range are preserved or explicitly transformed.
- Chapters must match count and time tolerance when preservation is required.

## 5. Audio rules

- Remux duration tolerance is the larger of 100 milliseconds or one codec frame.
- Transcode duration tolerance is 250 milliseconds unless the selected encoder has a documented priming allowance.
- Channel count and layout match the Plan.
- Sample rate and bit depth match exact hard constraints.
- Required tags and cover art are present.
- Lossy-to-lossless conversion never claims quality restoration.

## 6. GIF rules

- Dimensions are exact.
- Duration drift is at most one output frame.
- Frame count matches the planned frame-rate and duration within one frame.
- Loop behavior matches the Plan.
- A hard target-size limit must be met; a soft target-size miss produces Warning with achieved size.

## 7. PDF rules

- Output opens with the primary parser.
- Every selected page renders fully.
- Page count is exact unless page selection is explicit.
- Page size and rotation match expected values within renderer rounding.
- Required text extraction, links, forms, annotations, or fonts are checked only when the workflow promises them.
- Encrypted output policy is explicit.

For PDF-to-image page sets, required checks are exact selected/all-page count, deterministic ordered filenames, independent decode of every page, target format, point-size × DPI dimensions using the renderer's rounding rule, declared color mode, and the declared alpha/background policy. The page set is a single job output: any missing, extra, undecodable, or nonconforming page fails the set before directory commit.

## 8. Office-to-PDF rules

Hard failures:

- Output cannot open.
- Page count differs from the fixture or explicit expected range.
- A page fails to render.
- Content bounding boxes are clipped outside the page.

Warnings:

- Missing or substituted fonts.
- Changed pagination allowed by an unpinned environment.
- Material perceptual drift.
- Unsupported embedded object or animation.

Visual comparison is Warning-only in v0.1 until thresholds are calibrated per fixture class. Structural failures remain blocking.

## 9. Structured-data rules

- Output parses with an independent parser.
- Record count and required field count reconcile with the declared mapping.
- Null, missing, empty string, zero, and false remain distinguishable when the target can express them.
- Integer precision is not reduced silently.
- Date/time normalization requires an explicit policy.
- Duplicate keys and ordering behavior are reported.
- A lossy nested-to-tabular mapping requires explicit confirmation.

## 10. Metadata-clean rules

- The removed field set exactly matches the Plan.
- Required pixel, media, or document payload properties remain unchanged.
- Payload stability uses a format-aware hash or canonical comparison; whole-file hash equality is not expected when container metadata changes.
- Removed secret values are redacted from default reports.

## 11. Golden versus extended corpus

- Golden corpus: 100% required checks pass on supported platforms.
- Extended corpus: at least 95% complete successfully; every failure is explicit.
- Silent corruption target: zero known cases in both corpora.

## 12. Report stability

Every check has a stable code, severity, expected value, observed value, evidence source, and remediation message. Human text may be localized; codes and machine fields remain stable within a schema major version.
