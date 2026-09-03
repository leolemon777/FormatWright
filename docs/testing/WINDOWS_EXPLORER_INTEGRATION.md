# Windows Explorer Integration

- Status: current-user installed smoke contract covers Open-in and Convert verbs
- Updated: 2026-08-17
- Scope: Windows current-user NSIS package
- Verb table: `apps/desktop/src-tauri/explorer-verbs.json` (generated into `windows-explorer-hooks.nsh` and `scripts/register_dev_explorer_convert.ps1`)

## Contract

The installer owns **2 Open-in keys + 17 Convert keys** (19 total). Open-in:

- `Software\Classes\*\shell\FormatWright`
- `Software\Classes\Directory\shell\FormatWright`

Convert keys live under `Software\Classes\SystemFileAssociations\<ext>\shell\FormatWright.To*` and invoke `--shell-convert --to <format> "%1"`. Uninstall deletes only this generated set. Windows 11 normally shows classic verbs under **Show more options**; a modern top-level extension is out of Wave 1.

**Open in Anole** is navigation only: it may pre-fill an existing local file or directory and must create **0 Jobs**.

**Convert to X** is CLI-equivalent approval (`convert INPUT --to X`). A cold-start Convert of a small PDF must create **1 Job**, persist a Pass (or Warning) ValidationReport, leave the source hash unchanged, and refuse overwrite.

The backend still rejects:

- missing paths and incomplete markers;
- relative paths and bare positional arguments;
- UNC/network shares and device namespaces;
- Convert requests whose path is not a file (Wave 1).

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

The installed smoke writes the 2+17 owned current-user keys plus a uniquely named sibling fixture and removes them in `finally`. Open-in and Convert are asserted separately so Convert cannot be greenwashed by a zero-job check.

## Installed smoke result

The 2026-08-13 current-user run passed the Open-in assertions. The 2026-08-17 contract additionally requires:

1. After install, verify Open-in quoting **and** all 17 Convert commands (`--shell-convert --to`).
2. Right-click / `--shell-open` a local Unicode/space-bearing file; one window; **0 Jobs**.
3. Hot directory Open-in keeps the same PID and still creates **0 Jobs**.
4. After that process exits, cold `--shell-convert --to png` on a **small PDF** creates **1 Job**, a Pass/Warning report, output on disk, and an unchanged source hash.
5. Uninstall removes the 2+17 owned keys while an unrelated sibling verb remains.
6. Existing Roaming and Local application-state directories are isolated and restored with exact tree hashes.

The observed cold file and hot directory paths both contained Unicode and spaces. The hot launch exited successfully while the original PID remained the only Desktop process. A missing path was rejected without replacing the selected path. Uninstall returned zero, removed both owned keys and the install root, and preserved the unrelated sibling verb. A clean offline VM and an explicit Windows 11 **Show more options** observation remain release-certification gates.
