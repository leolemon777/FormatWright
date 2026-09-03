# Public Beta Validation and Launch Plan

- Status: Planned
- Version: 0.1
- Updated: 2026-08-10

## 1. Purpose

Desk research proves demand but not product usability. Public Beta requires primary user evidence before the project expands its format count.

## 2. Cohort

Target 24 participants:

- 8 creators or media-heavy desktop users.
- 6 developers or automation users.
- 6 archivists, data hoarders, or metadata-sensitive users.
- 4 privacy-sensitive occasional users.

At least:

- 8 Windows users.
- 6 macOS users.
- 6 Linux users.
- 4 participants with a real file above 5GB.
- 6 participants with a real batch above 1,000 files.

Participants must not submit confidential source files to maintainers.

## 3. Core tasks

1. Complete one recommended basic-mode conversion.
2. Explain the proposed Plan in the participant's own words.
3. Run or resume a batch.
4. Interpret a Warning report.
5. Use the CLI or export a reproducible recipe where relevant.
6. Verify that conversion works while offline.

## 4. Evidence collection

Default builds have no telemetry. Evidence is collected through:

- Moderator observation.
- A local diagnostics summary the user explicitly previews and exports.
- Post-task survey.
- Structured interview.
- Opt-in crash report with mandatory redaction preview.

No raw path, filename, file content, metadata value, or secret is included by default.

## 5. Exit targets

- 80% first supported conversion within three minutes.
- 90% correctly identify lossy versus remux/lossless Plan.
- 90% understand the primary Warning.
- 100% of interrupted-job exercises recover without false success.
- Zero unexpected network activity in observed offline sessions.
- No P0 or P1 open defect.

## 6. Defect severity

- P0 Critical: data loss, silent corruption, unauthorized overwrite, secret disclosure, remote code execution, or privacy promise violation. Release stops immediately.
- P1 High: supported workflow consistently fails, recovery loses job truth, cancellation leaves dangerous output, or installer cannot operate on a supported platform. Release is blocked.
- P2 Medium: workaround exists and no data-integrity/security promise is broken. May ship only with documented triage.
- P3 Low: cosmetic, minor usability, documentation, or enhancement.

Waiving P0 or P1 is not permitted for Public Beta.

## 7. Continue, change, stop gates

- Continue: exit targets pass and participants value plan/validation or recovery.
- Change positioning: conversion succeeds but users do not understand or value the differentiators.
- Narrow scope: fewer than 80% of golden workflows are reliable during internal pre-Beta.
- Stop adding formats: any P0/P1 reliability or license gate remains open.

## 8. Launch surfaces

- GitHub repository and signed releases.
- Documentation site with a truthful support matrix.
- Short demonstrations of remux planning, crash recovery, and validation.
- Announcements in self-hosted, privacy, data-hoarding, creator, and developer communities.
- Search pages may explain specific workflows, but may not advertise unsupported route counts.

## 9. Feedback triage

Every report requests:

- Workflow and requirement ID.
- Platform.
- Anole version.
- Engine manifest identity.
- Redacted Plan and ValidationReport.
- Reproduction using a redistributable sample where possible.

Format requests without a user workflow and legal fixture remain P3 discovery items.

