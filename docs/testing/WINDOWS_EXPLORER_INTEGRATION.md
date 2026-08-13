# Windows Explorer Integration

- Status: implemented; source/build verification complete; isolated installed smoke pending
- Updated: 2026-08-12
- Scope: Windows current-user NSIS package

## Contract

The installer owns two classic Explorer entries named **Open in FormatWright**:

- `Software\Classes\*\shell\FormatWright`
- `Software\Classes\Directory\shell\FormatWright`

Their commands invoke the installed executable with exactly one quoted selection after `--shell-open`. The uninstall pre-hook deletes only these two owned subtrees. Because these are classic verb registrations, Windows 11 normally displays them under **Show more options**; FormatWright does not yet claim a modern top-level Explorer extension.

The shell request is deliberately a navigation action, not conversion authority. It may pre-fill only an existing file or directory on a local drive. The backend rejects:

- missing paths and incomplete markers;
- relative paths and bare positional arguments;
- UNC/network shares and device namespaces;
- anything that is not currently a file or directory.

No target, preset, output, approval hash, or automatic execution can enter through the shell command.

## Single-instance behavior

`tauri-plugin-single-instance` is registered before all other plugins. A second launch forwards its full argument vector to the first instance and exits before `setup_desktop` can run startup recovery against the shared SQLite database. Accepted paths enter a 32-item FIFO that retains the newest requests under abuse; the frontend installs its event listener first and then drains the FIFO through a typed command, so initial launch, rapid repeated requests, and event/listener timing do not lose an accepted in-bound request. The existing window is shown, restored, and focused.

## Direct automated evidence

- Desktop Rust tests accept an explicit existing Unicode/space-bearing local absolute path.
- Desktop Rust tests reject missing, incomplete, bare, and relative requests.
- The Desktop crate and official single-instance plugin compile with Rust 1.88 and pass Clippy with warnings denied.
- Frontend TypeScript check, eight unit tests, and the production build pass with the FIFO consumer.
- The full NSIS build preprocesses `windows-explorer-hooks.nsh` and publishes a fresh setup executable.
- The final unsigned setup build completed with 279,369,285 bytes and SHA-256 `9ff39c4dfc888e544c911c5cb3b4d3a334f7721cb786e17e881f1993fa8cb21b`; generated `installer.nsi` includes the hook file and inserts both post-install and pre-uninstall macros.

These checks do not write the development machine's registry.

## Required isolated installed smoke

Run this only in a disposable Windows VM or an explicitly approved test profile:

1. Snapshot both owned keys as absent, install the current unsigned candidate, and verify both keys and their quoting.
2. Right-click a local Unicode/space-bearing file with the app closed; verify one window opens on Convert with the exact path and no job is created.
3. With that instance open and a durable job running, invoke a second local file and then a directory; verify the same process is focused, the latest accepted path appears, and no active job is marked interrupted.
4. Try a missing path, relative path, UNC share, and ordinary bare argument; verify none pre-fills Convert.
5. Uninstall and verify both owned keys are absent while an unrelated sibling verb remains intact.
6. Repeat on Windows 11 and record that the classic entry is under **Show more options**.

Until this matrix passes, the feature is implemented but not installed-environment certified.
