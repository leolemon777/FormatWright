# Desktop Release Conversion Evidence

- Status: local Windows release-candidate evidence; clean-VM certification pending
- Updated: 2026-08-15
- Suite: `scripts/test_desktop_release_conversion.ps1` + `scripts/cdp_desktop_conversion_e2e.mjs`
- Related defects: R-008, R-010

## Purpose

Prove that a real Release desktop build converts a real user PDF to PNG and JPEG **through the actual visible interface** — typed input path, capability-gated target selection, plan preview approval, conversion start, and a persisted Pass validation report — without any CLI shortcut, and restore the host application state afterwards.

## Harness design

- The desktop binary is built with `apps/desktop/src-tauri/tauri.release-e2e.conf.json`, which only adds a fixed loopback DevTools port (`--remote-debugging-port=9338`) to the WebView2 arguments. Production and installer builds never use this overlay.
- Each target format runs in **its own application process with its own isolated application state**. The harness moves both authoritative application-state roots (`%APPDATA%` and `%LOCALAPPDATA%` `local.formatwright.desktop`) to same-volume isolated names before each round, launches the Release executable with `--shell-open` and the Unicode/space input path, drives one conversion over CDP, verifies the deterministic page outputs, force-closes the process, removes the test state, and restores the original directories byte-for-byte. One format can therefore never inherit another format's React, engine-store, or job state.
- The CDP driver sets React-controlled values through native prototype setters plus real `input`/`change` events, waits for the target option to be capability-enabled, sets the output directory, clicks the real Plan preview button, asserts the plan card targets the requested format, clicks the real conversion button, and then requires: report status `pass`, zero non-pass checks, and the report output path equal to the requested directory. Timeouts dump a full form/button/notice diagnostic and a screenshot into the evidence directory.
- Input is a real 15-page user PDF (`ST508S`); outputs must be exactly `page-000001.<fmt>` … `page-000003.<fmt>` with three pages.

## Commands

~~~text
pnpm --filter @formatwright/desktop tauri build --no-bundle --config src-tauri/tauri.release-e2e.conf.json
pwsh -File scripts/test_desktop_release_conversion.ps1 -SourcePdf <real-pdf>
~~~

The standard installer is rebuilt separately without the overlay and must not embed the DevTools argument.

## Recorded run

2026-08-15, standard Starter resources, e2e-overlay Release binary, real 3-page Unicode/space-named user PDF:

- Suite: `.artifacts/desktop-release-conversion/suite-433c9b950b894deda251b353f7e95a98`
- PDF→PNG: report `pass`, 6/6 required checks, exactly `page-000001.png` … `page-000003.png`, screenshot `png-report.png`.
- PDF→JPEG: report `pass`, 6/6 required checks, exactly `page-000001.jpg` … `page-000003.jpg`, screenshot `jpg-report.png`.
- Each format ran in its own process and isolated state; both authoritative application-state roots were restored afterwards (`application_state_restored: true`).

Diagnostic note: an earlier failure of the JPEG round was a harness bug, not a product failure — the plan card renders the normalized target name (`PDF → JPEG`), while the driver waited for the raw id (`PDF → JPG`). The driver now accepts the normalized spelling, and the timeout diagnostics (form values, button states, plan heading, notices) that identified this are part of the suite.

## Boundary

- This is local Windows development-host evidence, not clean-machine certification: the host has development caches and system engines, and the Release binary is an unsigned Alpha build with a test-only DevTools port.
- It does not cover folder mode, queue/recovery flows, or non-PDF families; those have their own suites.
- `review_status=incomplete` engine supply-chain limits and all Public Beta gates remain open.
