# Desktop Accessibility Verification

- Status: automated Windows WebView baseline passes; live Narrator and user study pending
- Updated: 2026-08-13
- Scope: real Tauri/WebView2 window, Simplified Chinese and English

## Implemented contract

- The first keyboard stop is a localized skip link that moves focus to the `main` landmark.
- The primary navigation has a localized accessible name and exposes the active page with `aria-current="page"`.
- File/folder, Basic/Expert, recommendation and state-filter selections expose pressed state instead of relying only on color.
- Path controls use explicit `for`/`id` labels; picker buttons are no longer nested inside those labels.
- Path inputs use `dir="auto"` and isolated bidirectional text. Rendered job, mapping, engine-manifest and report paths use `bdi`.
- Existing alert/live-region, visible focus, reduced-motion, increased-contrast and responsive rules remain active.

## Reproducible Windows smoke

The release configuration does not expose a debug port. Build the test-only merge configuration and run the harness:

```powershell
pnpm --filter @formatwright/desktop tauri build -- --debug --no-bundle --config src-tauri/tauri.accessibility.conf.json
./scripts/test_desktop_accessibility.ps1
```

`tauri.accessibility.conf.json` exists only to add a fixed loopback DevTools port and forced renderer accessibility to the debug test binary. It is not used by production or installer builds. The harness moves both authoritative application-state directories to same-volume isolated names, launches the real Tauri executable, audits it over the WebView2 DevTools protocol, force-closes only the test process, deletes test state, restores the original directories and compares full file hashes.

The 2026-08-13 run reported:

- 198 accessibility-tree nodes, zero unnamed focusable controls, and three selected buttons exposing pressed state in both the DOM and AX tree;
- `main`, localized `navigation`, and localized skip-link semantics present;
- first Tab stop was `跳到主要内容`, and Enter focused `main-content`;
- a 590×390 CSS viewport at device scale 2 (1180×780 physical equivalent) had 575 px document width and 575 px client width: no horizontal document overflow;
- an Arabic/Hebrew/CJK path remained intact in the real WebView and used isolated bidi rendering;
- reduced motion, increased contrast and forced colors all matched; transition duration became `0s` and the high-contrast root became black;
- switching Chinese to English changed `document.lang`, the navigation accessible name, and skip-link text;
- both original application-state trees were restored byte-for-byte and no FormatWright test process remained.

The harness saves `accessibility-audit.json`, `desktop-200-percent.png`, and `desktop-forced-colors.png` under the ignored `.artifacts/desktop-accessibility` directory. Both screenshots were visually inspected for clipping, overlap, readable navigation, focus affordance, and text retention.

## Evidence boundary

This automated gate proves DOM semantics, the Chromium accessibility tree, keyboard skip navigation, equivalent 200% responsive behavior, bidirectional path rendering and media-query behavior on this Windows host. It does not prove Narrator speech phrasing, Windows 200% physical-monitor behavior, every tab workflow, VoiceOver/Orca behavior, or usability with real users. Those remain certification gates.
