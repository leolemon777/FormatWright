# Portable Preset Sandbox Evidence

- Status: Phase 4 Windows development evidence
- Updated: 2026-08-11
- Script: `scripts/test_preset_sandbox.ps1`

## Contract

The desktop Presets destination edits named conversion settings backed by the shared Rust `ConversionPreset` and `PresetLibrary` v1 contract. Each entry has a stable UUID and bounded target, quality, dimensions, DPI, color mode, and stream-preservation policy. The portable JSON envelope is governed by `urn:formatwright:schema:preset-library:v1`.

Imports are limited to 1 MiB, reject unknown fields and unsupported versions, validate all entries before changing the current library, merge by stable ID, and reject case-insensitive name conflicts. A library is limited to 4,096 presets. Writes occur through a same-directory partial and recoverable backup; startup restores a backup left between replacement steps.

## Reproducible gate

~~~powershell
pwsh -NoProfile -File scripts/test_preset_sandbox.ps1 `
  -Cargo E:\Users\Administrator\Desktop\FormatWright\.devtools\cargo\bin\cargo.exe
~~~

The recorded Windows run passed three core mutation/validation tests, the public JSON Schema contract test, and the desktop backup-recovery test. The frontend TypeScript check and production build also pass with the Presets editor, apply/edit, two-step delete, and native import/export controls.

The embedded release executable was then started as a real Tauri/WebView2 window. Windows UI Automation exposed named, keyboard-focusable navigation, edit, combobox, spinner, checkbox, and button controls. The run set the preset name through `ValuePattern`, invoked save through `InvokePattern`, verified the named preset and live confirmation, closed normally, restarted, verified the preset survived, exercised the two-step delete confirmation, confirmed the on-disk library returned to zero entries, and closed normally. Pixel evidence is stored under ignored `.artifacts/preset-native-ui/`; the test preset was removed after the run.

## Boundary

This is development evidence, not a full assistive-technology or usability study. The Orca computer-use runtime was unavailable, so the run used the Windows UI Automation provider directly. Cross-version migration beyond schema v1, signed preset bundles, cloud synchronization, full screen-reader navigation, and macOS/Linux native-dialog runs remain outside this gate. Presets never contain arbitrary shell commands or input/output paths.
