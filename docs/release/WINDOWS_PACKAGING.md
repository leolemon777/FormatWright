# Windows Packaging Evidence

- Status: unsigned self-contained release candidate; clean-machine certification pending
- Updated: 2026-08-16
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

The final 2026-08-16 standard-configuration rebuild — including the trusted-signature verification, versioned engine registry with startup fallback, and desktop recovery-outcome surfacing batches (ADR-0011 B1–B3) — produced:

| Artifact | Bytes | SHA-256 | Signature |
|---|---:|---|---|
| `FormatWright_0.1.0_x64-setup.exe` | 282,479,337 | `016c7cc657839560ae1b41a99c800bae865990041c71154b839cfe1ce55233be` | NotSigned |
| `formatwright-desktop.exe` (standard config) | 15,787,008 | `3af8702b0c975db109001cf8c163d39556dcc954b381d2842ed9d86a88faf1b8` | NotSigned |

The enhanced current-user install smoke passed against this installer (evidence `.artifacts/windows-explorer-installed-smoke/suite-ba4135da354340c798bbf511f94a66b3`).

A byte scan of both standard artifacts is negative for the release-e2e DevTools arguments (`remote-debugging-port`, `force-renderer-accessibility`); those exist only in the separately built test binary documented in `docs/testing/DESKTOP_RELEASE_CONVERSION.md`. The embedded Starter resource tree remains bundle hash `21f46f92f63ae9fc31a059b3139b4edcf27d2fa9b7b6522fc34f13cb43c48823` with the PDF and Media manifest/SBOM/sources hashes recorded in `docs/testing/WINDOWS_STARTER.md`. The installer also embeds the WebView2 offline runtime fetched at build time, so its hash depends on that download as well as the pinned packs.

## Sandbox smoke

The earlier application-shell installer was run silently with `/S` and an explicit `/D=` path inside the ignored project `.artifacts/installer-smoke` directory. The installed application:

- reported file/product version 0.1.0 / FormatWright;
- opened a native window titled `FormatWright`;
- remained alive and responsive during the observation;
- closed on a normal main-window request.

The installed `uninstall.exe /S` returned exit code 0. After two seconds, the explicit install root did not exist and contained zero remnants.

The current unpackaged Release candidate was then started with embedded resources. Startup installed both `formatwright-pdf` and `formatwright-media` into the versioned application-data store and wrote active registry records. Exact-pack local E2E passed PDF→PNG, PDF→JPEG, GIF, and built-in structured conversion. See `docs/testing/WINDOWS_STARTER.md`.

The 2026-08-13 current-user installed harness found and prevented a false-positive build-only result: literal NSIS `$"` tokens were present in the first registry command. After correction and rebuild, exact native quoting, actual Windows Shell verb cold launch, hot-instance forwarding, UIA path observation, zero-job behavior, negative missing-path handling, owned-key cleanup, unrelated-key preservation and install-root removal all passed. Both authoritative application-state roots were isolated and restored byte-for-byte. A clean offline VM remains required for release certification.

The enhanced smoke was rerun on 2026-08-15 against the current installer (evidence `.artifacts/windows-explorer-installed-smoke/suite-14dd1eebb65946968758aeb25ebba1b3`): both Starter packs installed from the embedded resources, all four supply-chain sidecar hashes re-verified with the real CLI verifier and `review_status=incomplete` asserted, installed Shell verbs and single-instance forwarding re-checked, and uninstall again left no owned keys or install-root remnants with application state restored byte-for-byte.

## Release boundary

This is not signed-release or clean-machine certification evidence. Both generated PE files remain unsigned. The embedded packs are pinned and hash-verified but are still `Unverified`: trusted pack signatures/keyring, transitive engine SBOMs, final license/source-offer review, revocation and upgrade/rollback are incomplete. The final NSIS artifact passed an isolated current-user host smoke but has not yet been exercised in a clean offline VM.

Public Beta remains blocked until offline clean-machine installed conversions pass, an authorized code-signing identity and timestamp service are configured, signatures are verified after bundling, engine supply-chain and upgrade/rollback matrices pass, R-001–R-007 close, and equivalent claimed-platform artifacts are built and tested.
