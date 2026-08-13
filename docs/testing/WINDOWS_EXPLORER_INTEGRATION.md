# Windows Explorer Integration

- Status: current-user installed smoke passes; clean-VM certification pending
- Updated: 2026-08-13
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
- The first real install exposed an NSIS quoting defect: the registry contained literal `$"` tokens. The hook now emits native NSIS quotes. The final rebuild containing the accessibility fixes is 279,373,840 bytes with SHA-256 `f5e18960f7e3f30c12d4b5d1b7a0f29ced88f5b72262f97c3821b37f4d0ea961`.
- `scripts/test_windows_explorer_integration.ps1` installs silently under ignored artifacts, exercises the actual Windows shell verb, inspects the native window through UI Automation, and always restores pre-existing application state.

The installed smoke writes only the two owned current-user keys plus a uniquely named sibling fixture and removes them in `finally`.

## Installed smoke result

The 2026-08-13 current-user run passed all of these assertions:

1. Snapshot both owned keys as absent, install the current unsigned candidate, and verify both keys and their quoting.
2. Right-click a local Unicode/space-bearing file with the app closed; verify one window opens on Convert with the exact path and no job is created.
3. With that instance open and a durable job running, invoke a second local file and then a directory; verify the same process is focused, the latest accepted path appears, and no active job is marked interrupted.
4. Try a missing path, relative path, UNC share, and ordinary bare argument; verify none pre-fills Convert.
5. Uninstall and verify both owned keys are absent while an unrelated sibling verb remains intact.
6. Existing Roaming and Local application-state directories were isolated and restored with exact tree hashes; zero durable jobs were created.

The observed cold file and hot directory paths both contained Unicode and spaces. The hot launch exited successfully while the original PID remained the only Desktop process. A missing path was rejected without replacing the selected path. Uninstall returned zero, removed both owned keys and the install root, and preserved the unrelated sibling verb. A clean offline VM and an explicit Windows 11 **Show more options** observation remain release-certification gates.
