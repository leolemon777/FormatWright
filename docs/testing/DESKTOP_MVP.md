# Desktop MVP Verification

- Status: Phase 4 Windows development evidence
- Updated: 2026-08-13

## Implemented surface

The Tauri 2 desktop application now invokes the same `formatwright-core::prepare_conversion` path intended for every first-party surface. The React shell exposes Convert, Jobs, Engines, Reports, and Settings rather than only the earlier queue-projection architecture spike.

- Convert accepts a Tauri file drop, a native file chooser, or an absolute local path. A native save chooser selects single-file destinations, while PDF page rendering selects an output directory. The surface recommends targets, builds a real Probe/Plan preview, exposes basic and expert controls, and runs the selected Plan.
- Convert can **Add to queue** (`queue_desktop_conversion`) which persists an immutable Plan as `Queued` without immediate execution.
- Start and Add-to-queue carry the exact visible preview `plan_hash`; the backend reprepares from the current input, options, and engine identities and rejects missing or stale approval before creating a job.
- Jobs can **Run queue window**, **Pause after current** (finish-current), and **Stop now** (immediate) via shared `JobExecutionService` / `QueueWindowControl`.
- The queue runner uses its own SQLite WAL connection while the UI retains a short-transaction connection. History refresh, bounded paging, and Add-to-queue remain available during a running window; newly queued work waits for the next bounded window. SQLite connections use a five-second busy timeout, and a RAII lease still prevents two queue runners.
- Immediate pause moves admitted work to recoverable `Interrupted / QUEUE_PAUSED_IMMEDIATE`; Jobs shows **Resume** for interrupted/blocked jobs and **Retry** for failed/cancelled jobs. Requeued work is re-inspected and runs in the next queue window; successful terminal jobs cannot be requeued.
- Typed Plan steps show engine, operation, loss class, capability, preserved/changed/dropped/unknown fields, and expert arguments without accepting shell text.
- A cancellation command reaches the active Rust `CancellationToken` and process-tree runner for single-shot converts; queue pause modes use `QueueWindowControl`.
- SQLite jobs live under the Tauri application data directory. On startup, active jobs become interrupted instead of disappearing.
- Validation reports are written for **immediate** converts and for **queue-window** completions through a same-directory partial, recoverable backup, and rename into the application-data `reports` directory before the terminal SQLite transition. Report-write failure leaves an immediate job `Interrupted`, never falsely `Completed`/`Warning`.
- Reports can be exported with paths redacted by default, immutable job recipes can be exported for reproducibility, and both use a 16 MiB bound plus atomic no-clobber publication. The file-browser action resolves the output from a trusted `job_id` in SQLite instead of accepting an arbitrary frontend path.
- Validation-only reopens the original input and existing output through the format validators without rerunning conversion steps or modifying output bytes. Each result is appended atomically to SQLite schema v5; the original conversion report and terminal state remain immutable, while report view/export show the newest evidence.
- The Jobs surface offers two-step manual staging cleanup only for Blocked/Failed/Cancelled/Interrupted jobs. The backend resolves deterministic candidates from the trusted SQLite Job ID/output pair under an immediate writer transaction, never accepts a frontend path, never targets final output, and appends a cleaned/not-found result event.
- Job history uses SQLite filtering/counting plus a hard 100-row IPC/page bound. Each mounted row uses native off-screen render isolation while remaining in the DOM, and exposes global list position/size semantics for keyboard and assistive-technology continuity across pages.
- Immediate and queue conversions stream truthful Core stages into the Desktop. Queue rows explain typed scheduler waits (process, memory, work class, exclusive engine, or paused admission) and show stage elapsed time. Rate and ETA remain explicitly unavailable because current engines do not expose verifiable total-work timing; the synthetic benchmark percentages never feed real Jobs.
- Engine Doctor performs local discovery only and does not download automatically.
- Engine import selects a local manifest, runs the shared pack verifier off the UI thread, persists an immutable manifest-hash registry entry, re-verifies entries on startup, rejects ambiguous executable-name claims, and activates only exact verified paths. Unsigned or merely signature-bearing packs remain `Unverified` until a trusted release keyring exists.
- The Windows installer owns classic Explorer entries for files and directories. They pass one quoted local absolute path behind the explicit `--shell-open` marker; the backend rejects missing, relative, UNC, device-namespace, and bare arguments, then only pre-fills Convert. The single-instance plugin is registered first, queues rapid follow-up paths for the existing window, and focuses it instead of opening a second recovery-capable process.
- Simplified Chinese and English strings live outside React components. Basic/expert mode and language preferences persist in versioned application settings; the document language and accessible navigation names update with the selected language.
- Keyboard focus, a first-stop skip link, main/navigation landmarks, active/pressed state semantics, explicit path labels, bidi-isolated paths, text status in addition to color, reduced-motion CSS, high-contrast CSS, responsive layouts, live regions, and alert roles cover the automated accessibility baseline.
- The bounded 10,000-job projection benchmark remains available as an opt-in Jobs diagnostic.

## Direct verification

- `pnpm --dir apps/desktop test -- --run`: two files and eight tests pass, covering the 10,000-job coalesced projection, duplicate-batch refusal, target recommendations, non-overwriting output suggestions, PDF page-directory suggestions, typed error parsing, global list semantics, and monotonic truthful progress with no invented ETA.
- `pnpm --dir apps/desktop build`: TypeScript project build and Vite production bundle pass.
- Current ordinary Rust baseline: 188 tests pass (153 core, 9 schema-contract, 22 desktop and 4 engine-SDK tests). Coverage includes shared Conversion/Report/Revalidation/JobRecovery services, versioned application-state bundles/settings, exact preview approval, truthful stages/scheduler wait reasons, in-flight pause and process-tree recovery, Desktop per-job and stable-selection bulk actions, idempotent enqueue, live bounded list/paging/enqueue, queue failure drain, report-before-terminal persistence/recovery, trusted exact staging cleanup, bounded redacted no-clobber export, safe Explorer argument parsing, Windows output identity, SQLite v5 maintenance/restore/concurrency/atomic claim, round-robin batch selection, no-clobber output publish, engine-pack activation, atomic durable batches, deterministic resource admission, and the bounded queue bridge. The homogeneous and mixed 10,000-conversion release gates are opt-in.
- Workspace Clippy with warnings denied passes for the desktop IPC and persistence implementation.
- `formatwright-desktop.exe` starts a native window titled `FormatWright`, remains responsive, and is then terminated by the harness.
- Edge headless rendered the local production UI at 1440×1000. The captured Convert page, including native-picker affordances, was visually checked for navigation, hierarchy, clipping, contrast, disabled controls, bilingual typography, and responsive column boundaries.
- The shared-core preparation path has a native Rust test that inspects JSON and produces a runnable YAML Plan, proving the desktop does not depend on CLI-only routing.
- `scripts/test_windows_explorer_integration.ps1` passed a real current-user silent install, exact registry command quoting, actual Shell verb cold/hot launches, UIA path observation, one-PID forwarding, zero durable jobs, negative missing path, owned-key cleanup, unrelated-key preservation, install-root removal, and byte-for-byte state restoration.
- `scripts/test_desktop_accessibility.ps1` passed against a real accessibility-instrumented Tauri/WebView2 debug build: 198 AX nodes, zero unnamed focusable controls, localized landmarks/skip link, first-Tab skip behavior, no horizontal document overflow at a 200% physical-equivalent viewport, Arabic/Hebrew/CJK path retention, reduced motion, forced colors/high contrast, and live Chinese→English language semantics. See `DESKTOP_ACCESSIBILITY.md`.

Generated screenshots and application data are local development artifacts and are not committed.

## Evidence boundary and remaining work

This is Windows development evidence, not Desktop Beta certification. The picker plugin, its least-privilege capabilities, frontend calls, TypeScript types, Rust registration, native executable startup, installed classic Shell path, Chromium accessibility tree, skip navigation, equivalent 200% layout and media preferences are verified. The Orca computer-use runtime was unavailable (`runtime_unavailable`), so Windows Narrator speech and physical-monitor/high-DPI behavior still need a live assistive-technology pass.

Remaining Phase 4 work includes a certified-pack download/install experience backed by the Phase 5 signature keyring, an optional Windows 11 modern top-level menu extension, macOS/Linux integration, a live Narrator/VoiceOver/Orca pass, signed installers, clean-VM and physical-monitor validation, and the three-minute first-user study. The classic Explorer menu normally appears under **Show more options** on Windows 11.
