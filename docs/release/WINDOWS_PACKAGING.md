# Windows Packaging Evidence

- Status: unsigned self-contained release candidate; clean-machine certification pending
- Updated: 2026-08-12
- Host: Windows x86-64

## Configuration

Tauri bundling is enabled in `apps/desktop/src-tauri/tauri.conf.json`. The Windows override builds a current-user NSIS installer with English and Simplified Chinese UI, embeds the full WebView2 offline installer, and maps the generated Windows x86-64 Starter resources to `engine-packs/starter/`. It does not use Tauri's network-dependent WebView bootstrapper.

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

The final 2026-08-12 local rebuild after R-010 produced:

| Artifact | Bytes | SHA-256 | Signature |
|---|---:|---|---|
| `FormatWright_0.1.0_x64-setup.exe` | 278,978,992 | `3d48f79168eda1eb7672be121e8d9cef73f50792a64ead414941281959605ee2` | NotSigned |
| `formatwright-desktop.exe` | 13,946,368 | `70f3d03ca452b5f0a300372b1d953fafd1c799ce60cc80bdd90d978545f161e5` | NotSigned |

The embedded resource directory contains bundle hash `21f46f92f63ae9fc31a059b3139b4edcf27d2fa9b7b6522fc34f13cb43c48823`, PDF manifest hash `e047b5e81f3f8abbc2329a91850ec570a718c7d0aed84c1016b9feefc88e894b`, and Media manifest hash `5bc2643953fc4f80ed7ad5abd5e74a20b0270e67ab9e04c8aa527e6e4ddebc73`.

## Sandbox smoke

The earlier application-shell installer was run silently with `/S` and an explicit `/D=` path inside the ignored project `.artifacts/installer-smoke` directory. The installed application:

- reported file/product version 0.1.0 / FormatWright;
- opened a native window titled `FormatWright`;
- remained alive and responsive during the observation;
- closed on a normal main-window request.

The installed `uninstall.exe /S` returned exit code 0. After two seconds, the explicit install root did not exist and contained zero remnants.

The current unpackaged Release candidate was then started with embedded resources. Startup installed both `formatwright-pdf` and `formatwright-media` into the versioned application-data store and wrote active registry records. Exact-pack local E2E passed PDF→PNG, PDF→JPEG, GIF, and built-in structured conversion. See `docs/testing/WINDOWS_STARTER.md`.

## Release boundary

This is not signed-release or clean-machine certification evidence. Both generated PE files remain unsigned. The embedded packs are pinned and hash-verified but are still `Unverified`: trusted pack signatures/keyring, transitive engine SBOMs, final license/source-offer review, revocation and upgrade/rollback are incomplete. The final NSIS artifact has not yet been installed and exercised in an isolated clean VM.

Public Beta remains blocked until offline clean-machine installed conversions pass, an authorized code-signing identity and timestamp service are configured, signatures are verified after bundling, engine supply-chain and upgrade/rollback matrices pass, R-001–R-007 close, and equivalent claimed-platform artifacts are built and tested.
