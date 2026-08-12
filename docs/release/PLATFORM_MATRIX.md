# Platform Test Matrix

- Status: Initial candidate matrix
- Updated: 2026-08-10

## 1. Required environments

| ID | OS | Architecture | Purpose |
|---|---|---|---|
| WIN-PRIMARY | Windows 11 | x86_64 | Primary development, CLI, desktop, installer, Job Object |
| MAC-ARM | macOS supported baseline | arm64 | CLI, desktop, signing/notarization, Finder action |
| MAC-X64 | macOS supported baseline | x86_64 | Build and golden smoke |
| LINUX-X64 | Ubuntu LTS | x86_64 | CLI, desktop, AppImage, process groups |

Exact minimum versions are frozen by ADR after Phase 1 engine and Tauri testing.

## 2. Per-platform checks

Common:

- Build, fmt, lint, unit and integration tests.
- Unicode and long paths.
- Same-filesystem staging and commit.
- Cancellation and descendant cleanup.
- 10,000-job database behavior.
- Offline conversion.

Windows:

- Job Object.
- Case-insensitive conflicts.
- Reserved filenames.
- Extended-length paths.
- MSI/NSIS and signing.
- Explorer context integration.

macOS:

- App signing and notarization.
- Hardened runtime.
- App translocation behavior.
- Finder Quick Action.
- APFS case-sensitive and default case-insensitive variants where available.

Linux:

- Process group signals.
- AppImage.
- Wayland/X11 smoke where relevant.
- File-manager action.
- ext4 and a case-sensitive filesystem.

## 3. Hardware coverage

- CPU-only conversion.
- One supported hardware video encoder path per primary platform when available.
- Low-memory reference environment.
- High-DPI display.
- Removable drive.

Hardware acceleration is Experimental until its output validation matches software-path requirements.

