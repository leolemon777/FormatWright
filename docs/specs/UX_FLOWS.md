# Desktop UX Flows and State Matrix

- Status: Phase 4 implementation in progress
- Version: 0.1
- Updated: 2026-08-12

## 1. Information architecture

Primary destinations:

- Convert
- Jobs
- Presets
- Engines
- Reports
- Settings

The default launch destination is Convert unless interrupted jobs require attention.

The current Windows development slice implements Convert, Jobs, Presets, Engines, Reports, and Settings. Native input/output pickers, versioned named preset editing/import/export, and classic Explorer Open-in plus Convert-to-X verbs are implemented. Open-in only pre-fills an existing local absolute path. A named Convert verb is treated as CLI `convert` approval (Plan is still generated and validated). A running single instance receives and focuses later requests. Windows 11 modern top-level, macOS/Linux shell integration, and release usability studies remain pending.

## 2. First-run flow

1. Show the local-first promise and link to the network policy.
2. Run Doctor without downloading anything.
3. Display available workflow categories.
4. Offer user-initiated installation or offline import for missing certified packs.
5. Continue with available capabilities; missing engines do not block the entire app.

## 3. Convert flow states

| State | Required UI |
|---|---|
| empty | Drop zone, choose file/folder, privacy note |
| inspecting | Per-item progress, cancel |
| unsupported | Reason, detected type, plugin/engine guidance |
| target selection | Recommended targets, search, presets |
| plan ready | Steps, loss badge, preserved/dropped properties, disk estimate |
| blocked | Exact blocker and action |
| queued | Queue position and scheduling reason |
| running | Stage, progress units, speed, ETA confidence, pause/cancel |
| validating | Checks in progress |
| completed | Output path, Pass report, open actions |
| warning | Output path, warning summary, report |
| failed | Failed stage, safe retry guidance, partial cleanup state |

## 4. Basic mode

- Target format.
- Quality or size intent.
- Dimensions where relevant.
- Metadata choice.
- Save location.
- Plain-language Plan summary.

The primary action remains disabled until hard blockers are resolved.

## 5. Expert mode

- Container and codec.
- Remux-only or lossless-only constraints.
- Track, subtitle, chapter, metadata, color, frame-rate, sample-rate, and pixel-format controls.
- Engine pinning.
- Resource and temporary-space policy.
- Exact typed command preview.

Expert mode never accepts an arbitrary Shell string.

## 6. Recovery flow

On launch with interrupted jobs:

- Show a non-modal recovery banner.
- Categorize resumable batch, restart-current-file, validation-only, blocked, and cleanup-needed.
- Default safe action is resume completed-state reconciliation and restart interrupted current files.
- Never auto-overwrite a destination created outside Anole.

## 7. Large batch UX

- Virtualized/paginated list.
- Aggregate counts reconcile with database state.
- Search and filter do not load all reports.
- Actions operate on a stable selection query, not only visible rows.
- Partial-success summary separates completed, warning, failed, skipped, and cancelled.

## 8. Secret flow

- Passwords use a dedicated secret prompt.
- Secret values are not placed in command preview, logs, history, reports, or SQLite.
- Remembering a secret is out of scope for v0.1.
- The prompt clearly identifies the file and engine requesting access.

## 9. Insufficient disk flow

- Show destination free space, estimated output, estimated temporary space, and confidence.
- Offer changing destination, reducing target settings, or cancelling.
- Do not offer “continue anyway” when a hard minimum cannot fit.

## 10. Accessibility

- Full keyboard path for every primary workflow.
- Visible focus.
- Screen-reader labels and live regions for state changes.
- Status uses icon/text in addition to color.
- Reduced motion.
- UI zoom and high contrast.
- Chinese and English layouts tested; content fixtures include RTL.

## 11. Usability acceptance

- 80% of first-time Beta participants complete a supported conversion within three minutes without assistance.
- Users can correctly identify whether a Plan is remux, lossless, or lossy.
- Users can locate the reason for a Warning without opening raw logs.
- A 10,000-item batch remains scrollable and controllable.
