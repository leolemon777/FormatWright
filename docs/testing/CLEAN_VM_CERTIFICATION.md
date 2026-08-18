# Clean Offline Windows VM Certification (Batch D)

- Status: runbook + automation ready; **awaiting VM environment**
- Created: 2026-08-16
- Suite: `scripts/test_clean_vm_certification.ps1`
- Blocking: closes R-008/R-009 from Fixed → Closed; Public Beta gate

## Purpose

Prove the shipped installer works on a machine that has **no** conversion engines, **no** FormatWright development caches, a **deliberately polluted PATH**, and (for the manual steps) **no network**. Local host evidence can never substitute: this machine has system engines and development state everywhere.

## VM preparation checklist

1. Windows 11 x64 VM, fresh image, no snapshots of prior FormatWright testing.
2. Do **not** install FFmpeg, Poppler, LibreOffice, Pandoc, libvips, or any codec pack.
3. Install PowerShell 7, Node.js (for the CDP driver), and Python (optional, for checksum tooling) — nothing else.
4. Copy in from the host: the standard NSIS installer, the e2e-overlay desktop binary (`tauri build --no-bundle --config src-tauri/tauri.release-e2e.conf.json` output), and a real multi-page PDF.
5. Take a **clean snapshot** before the first run so the suite can be re-run from scratch.
6. For the offline phase: disconnect the virtual network adapter after file transfer.

## Automated suite

From an elevated-or-normal pwsh inside the VM:

~~~text
pwsh -File scripts/test_clean_vm_certification.ps1 `
    -Installer <FormatWright_0.1.0_x64-setup.exe> `
    -E2EBinary <formatwright-desktop.exe> `
    -SourcePdf <real.pdf>
~~~

The suite asserts, in order:

- VM cleanliness: none of ffmpeg/ffprobe/pdftoppm/pdfinfo/soffice/pandoc/vips on PATH, no prior app state, no prior install root.
- Silent `/S` install succeeds; the installed standard binary does **not** embed the test DevTools argument.
- First launch of the **installed** app installs both Starter packs from embedded resources while hostile `.cmd` wrappers sit earlier on PATH (Release must resolve exact pack paths, never PATH).
- Real UI PDF→PNG and PDF→JPEG conversions pass through `scripts/test_desktop_release_conversion.ps1` with per-format isolated processes (evidence under `.artifacts/clean-vm-certification/vm-*/ui-conversions/`).
- `uninstall.exe /S` removes the install root, both app-state roots, and FormatWright's owned shell keys (`FormatWright` Open-in plus the 17 generated Convert verbs). It must **not** look for the obsolete `FormatWrightConvert` name.

## Manual checklist (not yet automated — record screenshots + notes)

- [ ] With the network adapter disconnected, repeat one PDF→PNG and one JSON→YAML conversion from the installed UI; confirm no network prompts or failures (`test_zero_network.ps1` philosophy).
- [ ] A JSON→YAML structured conversion from the installed UI (Core built-in path).
- [ ] One audio/video conversion (Media pack) from the installed UI.
- [ ] Unicode/space/long-path (>260 chars) files through the UI.
- [ ] Windows Explorer **Open in FormatWright** on a file and a directory (cold and hot instance).
- [ ] Cancel a long conversion mid-run; no partial output is committed.
- [ ] Force-kill the app during a queued batch; relaunch; the recovery banner appears and jobs resume/retry correctly (engine fallback notice path from ADR-0011 B3 is visible if a pack breaks).
- [ ] Upgrade: install a newer RC over this one; state migrates; downgrade attempt is refused or clean.
- [ ] Save VM snapshot, evidence directory, installer hash, and screen recordings into the release evidence folder.

## Closure rule

R-008/R-009 move Fixed → Closed only when this suite plus the manual checklist pass on the final RC, and the supply-chain gate (batch C sign-off) is also recorded. One clean-VM pass against a superseded installer does not count.
