# Desktop MVP Verification

- Status: Phase 4 Windows development evidence
- Updated: 2026-08-11

## Implemented surface

The Tauri 2 desktop application now invokes the same `formatwright-core::prepare_conversion` path intended for every first-party surface. The React shell exposes Convert, Jobs, Engines, Reports, and Settings rather than only the earlier queue-projection architecture spike.

- Convert accepts a Tauri file drop, a native file chooser, or an absolute local path. A native save chooser selects single-file destinations, while PDF page rendering selects an output directory. The surface recommends targets, builds a real Probe/Plan preview, exposes basic and expert controls, and runs the selected Plan.
- Convert can **Add to queue** (`queue_desktop_conversion`) which persists an immutable Plan as `Queued` without immediate execution.
- Jobs can **Run queue window**, **Pause after current** (finish-current), and **Stop now** (immediate) via shared `JobExecutionService` / `QueueWindowControl`.
- Typed Plan steps show engine, operation, loss class, capability, preserved/changed/dropped/unknown fields, and expert arguments without accepting shell text.
- A cancellation command reaches the active Rust `CancellationToken` and process-tree runner for single-shot converts; queue pause modes use `QueueWindowControl`.
- SQLite jobs live under the Tauri application data directory. On startup, active jobs become interrupted instead of disappearing.
- Validation reports are written for **immediate** converts and for **queue-window** completions through a same-directory partial and rename into the application-data `reports` directory (`run_window_observed` persists before the terminal SQLite transition).
- Engine Doctor performs local discovery only and does not download automatically.
- Engine import selects a local manifest, runs the shared pack verifier off the UI thread, persists an immutable manifest-hash registry entry, re-verifies entries on startup, rejects ambiguous executable-name claims, and activates only exact verified paths. Unsigned or merely signature-bearing packs remain `Unverified` until a trusted release keyring exists.
- Simplified Chinese and English strings live outside React components. Basic/expert mode and language preferences persist in local storage.
- Keyboard focus, text status in addition to color, reduced-motion CSS, high-contrast CSS, responsive layouts, labels, live regions, and alert roles cover the initial accessibility baseline.
- The bounded 10,000-job projection benchmark remains available as an opt-in Jobs diagnostic.

## Direct verification

- `pnpm --dir apps/desktop test -- --run`: two files and six tests pass, covering the 10,000-job coalesced projection, duplicate-batch refusal, target recommendations, non-overwriting output suggestions, PDF page-directory suggestions, and typed error parsing.
- `pnpm --dir apps/desktop build`: TypeScript project build and Vite production bundle pass.
- Current ordinary Rust baseline: 79 tests pass (64 core, 7 schema-contract, 4 desktop and 4 engine-SDK tests), including engine-pack activation, duplicate-claim refusal, atomic batch queueing, cancellation at the validation boundary, deterministic resource admission, versioned preset validation/recovery, network-path policy, durable registry parsing, shared queue execution, and the bounded queue bridge. The separate 10,000-conversion release gate is opt-in.
- Workspace Clippy with warnings denied passes for the desktop IPC and persistence implementation.
- `formatwright-desktop.exe` starts a native window titled `FormatWright`, remains responsive, and is then terminated by the harness.
- Edge headless rendered the local production UI at 1440×1000. The captured Convert page, including native-picker affordances, was visually checked for navigation, hierarchy, clipping, contrast, disabled controls, bilingual typography, and responsive column boundaries.
- The shared-core preparation path has a native Rust test that inspects JSON and produces a runnable YAML Plan, proving the desktop does not depend on CLI-only routing.

Generated screenshots and application data are local development artifacts and are not committed.

## Evidence boundary and remaining work

This is Windows development evidence, not Desktop Beta certification. The picker plugin, its least-privilege capabilities, frontend calls, TypeScript types, Rust registration, and native executable startup are verified. The Orca computer-use runtime was unavailable (`runtime_unavailable`), so automated interaction with the native picker and accessibility tree could not be performed in this run; keyboard/screen-reader behavior still needs a live assistive-technology pass.

Remaining Phase 4 work includes recovery banner, filtered/bulk history actions, report export/redaction controls, open-file/open-folder actions, folder-as-input, a certified-pack download/install experience backed by the Phase 5 signature keyring, Windows context-menu registration, macOS/Linux integration, RTL UI fixtures, signed installers, and the three-minute first-user study.
