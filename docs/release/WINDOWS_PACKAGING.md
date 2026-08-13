# Windows Packaging Evidence

- Status: unsigned self-contained release candidate; clean-machine certification pending
- Updated: 2026-08-12
- Host: Windows x86-64

## Configuration

Tauri bundling is enabled in `apps/desktop/src-tauri/tauri.conf.json`. The Windows override builds a current-user NSIS installer with English and Simplified Chinese UI, embeds the full WebView2 offline installer, and maps the generated Windows x86-64 Starter resources to `engine-packs/starter/`. It does not use Tauri's network-dependent WebView bootstrapper.

`windows-explorer-hooks.nsh` adds classic Explorer **Open in FormatWright** entries for all files and directories under the current user's `Software\Classes` view. Each command quotes the installed executable and selected path and uses the explicit `--shell-open` marker. `NSIS_HOOK_PREUNINSTALL` deletes only FormatWright's two owned keys. Windows 11 normally places these classic registrations under **Show more options**; a modern top-level shell extension is not claimed.

The application registers Tauri's official single-instance plugin before every other plugin. If FormatWright is already open, a context invocation forwards its argument to the existing process, queues it until the frontend consumes it, restores/focuses the main window, and exits before Desktop setup can run recovery a second time. The backend accepts only an existing local-drive absolute file/directory path and never starts conversion automatically.

On first startup, the Release backend verifies each embedded manifest, executable, runtime file and license notice; copies declared files into the versioned application-data engine store; atomically updates one active registry pointer per engine ID; and activates exact installed paths. Release never substitutes a tool discovered from the user's `PATH`.

Build from the repository root:

```text
pnpm --filter @formatwright/desktop tauri build --bundles nsis
```

Generate release checksums from an explicit artifact list:

```text
python scripts/generate_checksums.py target/release/formatwright-desktop.exe target/release/bundle/nsis/FormatWright_0.1.0_x64-setup.exe
```

The checksum generator hashes files in 1 MiB chunks, rejects missing/non-file inputs, duplicate basenames, and attempts to include the manifest itself.

## Current recorded build

The final 2026-08-12 local rebuild after the Explorer/single-instance slice produced:

| Artifact | Bytes | SHA-256 | Signature |
|---|---:|---|---|
| `FormatWright_0.1.0_x64-setup.exe` | 279,369,285 | `9ff39c4dfc888e544c911c5cb3b4d3a334f7721cb786e17e881f1993fa8cb21b` | NotSigned |
| `formatwright-desktop.exe` | 15,524,352 | `3e7211258f72b8d85bca2e490017bcef0734ec5d26ce48e0b6a87d35964fa3d7` | NotSigned |

The embedded resource directory contains bundle hash `21f46f92f63ae9fc31a059b3139b4edcf27d2fa9b7b6522fc34f13cb43c48823`, PDF manifest hash `e047b5e81f3f8abbc2329a91850ec570a718c7d0aed84c1016b9feefc88e894b`, and Media manifest hash `5bc2643953fc4f80ed7ad5abd5e74a20b0270e67ab9e04c8aa527e6e4ddebc73`.

## Sandbox smoke

The earlier application-shell installer was run silently with `/S` and an explicit `/D=` path inside the ignored project `.artifacts/installer-smoke` directory. The installed application:

- reported file/product version 0.1.0 / FormatWright;
- opened a native window titled `FormatWright`;
- remained alive and responsive during the observation;
- closed on a normal main-window request.

The installed `uninstall.exe /S` returned exit code 0. After two seconds, the explicit install root did not exist and contained zero remnants.

The current unpackaged Release candidate was then started with embedded resources. Startup installed both `formatwright-pdf` and `formatwright-media` into the versioned application-data store and wrote active registry records. Exact-pack local E2E passed PDF→PNG, PDF→JPEG, GIF, and built-in structured conversion. See `docs/testing/WINDOWS_STARTER.md`.

The 2026-08-12 final source build completed Release linking and NSIS bundling with the Explorer hooks and pinned single-instance plugin. Registry installation, cold launch, hot-instance forwarding, and uninstall cleanup remain an isolated-VM manual test and are not inferred from a successful bundle build.

## Release boundary

This is not signed-release or clean-machine certification evidence. Both generated PE files remain unsigned. The embedded packs are pinned and hash-verified but are still `Unverified`: trusted pack signatures/keyring, transitive engine SBOMs, final license/source-offer review, revocation and upgrade/rollback are incomplete. The final NSIS artifact has not yet been installed and exercised in an isolated clean VM.

Public Beta remains blocked until offline clean-machine installed conversions pass, an authorized code-signing identity and timestamp service are configured, signatures are verified after bundling, engine supply-chain and upgrade/rollback matrices pass, R-001–R-007 close, and equivalent claimed-platform artifacts are built and tested.
