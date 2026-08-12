# Windows Packaging Evidence

- Status: unsigned application-shell Alpha evidence; not conversion-usable on a clean machine
- Updated: 2026-08-12
- Host: Windows x86-64

## Configuration

Tauri bundling is enabled in `apps/desktop/src-tauri/tauri.conf.json`. The Windows override builds a current-user NSIS installer with English and Simplified Chinese UI and embeds the full WebView2 offline installer. It does not use Tauri's smaller network-dependent bootstrapper. The recorded build contains no conversion engine or verified Starter pack; it therefore proves only application installation/startup, not a usable conversion release.

The target release may embed a reviewed Starter pack or place the same-version pack beside the installer in an Offline Bundle. In both cases installation must verify and activate it through the Engine Registry; Release must never substitute a tool discovered from the user's PATH.

Build from the repository root:

```text
pnpm --filter @formatwright/desktop tauri build --bundles nsis
```

Generate release checksums from an explicit artifact list:

```text
python scripts/generate_checksums.py target/release/formatwright-desktop.exe target/release/bundle/nsis/FormatWright_0.1.0_x64-setup.exe
```

The checksum generator hashes files in 1 MiB chunks, rejects missing/non-file inputs, duplicate basenames, and attempts to include the manifest itself.

## Recorded build

The 2026-08-10 local build produced:

| Artifact | Bytes | SHA-256 | Signature |
|---|---:|---|---|
| `FormatWright_0.1.0_x64-setup.exe` | 215,904,990 | `c2078e0b96c530e3abc35302e93a305893fb41d753943fe91095b66ba82c0998` | NotSigned |
| `formatwright-desktop.exe` | 13,133,824 | `052a2b1eea2172285bbff1543334b9ac76d63a1ea5ea8e049fd7772f24ae88bb` | NotSigned |

The NSIS payload installed an executable of the same size and version but a distinct SHA-256 (`47e93c5271dd386bc71ffa0b97e84c97936f5d89f9fdf24179c37ed0db621601`). Installer and standalone executable are therefore treated as separate artifacts; portable/payload byte-equivalence is not claimed.

## Sandbox smoke

The installer was run silently with `/S` and an explicit `/D=` path inside the ignored project `.artifacts/installer-smoke` directory. The installed application:

- reported file/product version 0.1.0 / FormatWright;
- opened a native window titled `FormatWright`;
- remained alive and responsive during the observation;
- closed on a normal main-window request.

The installed `uninstall.exe /S` returned exit code 0. After two seconds, the explicit install root did not exist and contained zero remnants.

## Release boundary

This is not usable-release or signed-release evidence. Both generated PE files returned `NotSigned` from `Get-AuthenticodeSignature`, and artifact inspection confirms that Poppler, FFmpeg and the other conversion engines are absent. The current runtime can fall back to ambient PATH and selected a broken Codex `pdfinfo.cmd`; that behavior is tracked as R-008/R-009.

Public Beta remains blocked until the verified Starter pack and exact-path production locator ship, offline clean-machine conversions pass, an authorized code-signing identity and timestamp service are configured, signatures are verified after bundling, upgrade/rollback matrices pass, and equivalent claimed-platform artifacts are built and tested.
