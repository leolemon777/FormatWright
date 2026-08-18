---
version: alpha
name: Chicago 95
description: A retro desktop-OS design system rebuilt as a portable CSS component language. Beveled silver chrome, navy title bars, and pixel-tight typography sit on a teal desktop.
theme: light
colors:
  primary: "#000080"
  primary-hover: "#1084d0"
  secondary: "#c0c0c0"
  tertiary: "#008080"
  neutral: "#808080"
  surface: "#c0c0c0"
  surface-raised: "#dfdfdf"
  surface-sunken: "#a8a8a8"
  surface-field: "#ffffff"
  on-surface: "#000000"
  on-primary: "#ffffff"
  on-surface-muted: "#404040"
  on-surface-disabled: "#808080"
  border-highlight: "#ffffff"
  border-shadow: "#808080"
  border-deep: "#000000"
  focus: "#000000"
  accent: "#ffff00"
  error: "#c00000"
  syntax-comment: "#008000"
  syntax-keyword: "#0000ff"
  syntax-string: "#808080"
  syntax-variable: "#800080"
  syntax-variable-2: "#c00000"
typography:
  display-xl:
    fontFamily: "VT323, 'Courier New', monospace"
    fontSize: "56px"
    lineHeight: 1.15
    fontWeight: 400
  display-md:
    fontFamily: "VT323, 'Courier New', monospace"
    fontSize: "28px"
    lineHeight: 1.15
    fontWeight: 400
  headline-lg:
    fontFamily: "'Pixelify Sans', 'MS Sans Serif', sans-serif"
    fontSize: "22px"
    lineHeight: 1.15
    fontWeight: 700
  headline-md:
    fontFamily: "'Pixelify Sans', 'MS Sans Serif', sans-serif"
    fontSize: "18px"
    lineHeight: 1.3
    fontWeight: 700
  headline-sm:
    fontFamily: "'Pixelify Sans', 'MS Sans Serif', sans-serif"
    fontSize: "15px"
    lineHeight: 1.3
    fontWeight: 700
  body-md:
    fontFamily: "'Pixelify Sans', 'MS Sans Serif', sans-serif"
    fontSize: "14px"
    lineHeight: 1.45
    fontWeight: 400
  body-sm:
    fontFamily: "'Pixelify Sans', 'MS Sans Serif', sans-serif"
    fontSize: "13px"
    lineHeight: 1.45
    fontWeight: 400
  label-sm:
    fontFamily: "'Pixelify Sans', 'MS Sans Serif', sans-serif"
    fontSize: "12px"
    lineHeight: 1.15
    fontWeight: 500
  caption:
    fontFamily: "'Pixelify Sans', 'MS Sans Serif', sans-serif"
    fontSize: "11px"
    lineHeight: 1.15
    fontWeight: 400
  mono-md:
    fontFamily: "'JetBrains Mono', 'Courier New', monospace"
    fontSize: "13px"
    lineHeight: 1.3
    fontWeight: 400
spacing:
  2xs: "2px"
  xs: "4px"
  sm: "6px"
  md: "10px"
  lg: "16px"
  xl: "24px"
  2xl: "32px"
  3xl: "48px"
rounded:
  none: "0"
  sm: "0"
  md: "0"
  lg: "0"
  xl: "0"
  full: "0"
elevation:
  bevel-out: "inset 1px 1px 0 0 #ffffff, inset -1px -1px 0 0 #000000, inset 2px 2px 0 0 #dfdfdf, inset -2px -2px 0 0 #808080"
  bevel-in: "inset 1px 1px 0 0 #000000, inset -1px -1px 0 0 #ffffff, inset 2px 2px 0 0 #808080, inset -2px -2px 0 0 #dfdfdf"
  bevel-out-thin: "inset 1px 1px 0 0 #ffffff, inset -1px -1px 0 0 #808080"
  bevel-in-thin: "inset 1px 1px 0 0 #808080, inset -1px -1px 0 0 #ffffff"
  bevel-field: "inset 1px 1px 0 0 #808080, inset -1px -1px 0 0 #ffffff, inset 2px 2px 0 0 #000000, inset -2px -2px 0 0 #dfdfdf"
  focus-marquee: "1px dotted #000000"
components:
  button-secondary:
    backgroundColor: "{colors.secondary}"
    textColor: "{colors.on-surface}"
    typography: "{typography.body-sm}"
    rounded: "{rounded.none}"
    padding: "0 10px"
    height: "28px"
    elevation: "{elevation.bevel-out}"
  button-secondary-active:
    backgroundColor: "{colors.secondary}"
    textColor: "{colors.on-surface}"
    elevation: "{elevation.bevel-in}"
  button-primary:
    backgroundColor: "{colors.secondary}"
    textColor: "{colors.on-surface}"
    typography: "{typography.body-sm}"
    rounded: "{rounded.none}"
    padding: "0 10px"
    height: "28px"
    elevation: "{elevation.bevel-out}"
    border: "1px solid {colors.border-deep}"
  button-disabled:
    backgroundColor: "{colors.secondary}"
    textColor: "{colors.on-surface-disabled}"
    elevation: "{elevation.bevel-out}"
  input-field:
    backgroundColor: "{colors.surface-field}"
    textColor: "{colors.on-surface}"
    typography: "{typography.body-sm}"
    rounded: "{rounded.none}"
    padding: "4px 6px"
    height: "28px"
    elevation: "{elevation.bevel-field}"
  input-field-focus:
    backgroundColor: "{colors.surface-field}"
    elevation: "{elevation.bevel-field}"
    border: "inset 0 0 0 1px {colors.primary}"
  card-window:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.on-surface}"
    rounded: "{rounded.none}"
    padding: "2px"
    elevation: "{elevation.bevel-out}"
  card-window-titlebar:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.on-primary}"
    typography: "{typography.label-sm}"
    height: "22px"
    padding: "0 4px 0 6px"
  checkbox-base:
    backgroundColor: "{colors.surface-field}"
    size: "14px"
    rounded: "{rounded.none}"
    elevation: "{elevation.bevel-field}"
  checkbox-checked:
    backgroundColor: "{colors.surface-field}"
    textColor: "{colors.on-surface}"
    elevation: "{elevation.bevel-field}"
  tabs-tab:
    backgroundColor: "{colors.secondary}"
    textColor: "{colors.on-surface}"
    typography: "{typography.label-sm}"
    rounded: "{rounded.none}"
    padding: "4px 10px"
    elevation: "{elevation.bevel-out-thin}"
  tabs-active:
    backgroundColor: "{colors.secondary}"
    textColor: "{colors.on-surface}"
    typography: "{typography.label-sm}"
    rounded: "{rounded.none}"
    padding: "6px 10px 5px"
    elevation: "{elevation.bevel-out-thin}"
    border: "1px solid {colors.primary}"
  terminal-screen:
    backgroundColor: "#000000"
    textColor: "#33ff66"
    typography: "{typography.display-md}"
    rounded: "{rounded.none}"
    padding: "10px"
    elevation: "{elevation.bevel-in-thin}"
  tag:
    backgroundColor: "{colors.secondary}"
    textColor: "{colors.on-surface}"
    typography: "{typography.caption}"
    rounded: "{rounded.none}"
    padding: "2px 6px"
    elevation: "{elevation.bevel-out-thin}"
  tag-alert:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.border-deep}"
    typography: "{typography.caption}"
    rounded: "{rounded.none}"
    padding: "2px 6px"
    elevation: "{elevation.bevel-out-thin}"
---

## Overview

Chicago 95 is a skeuomorphic, desktop-OS-flavored design system that translates the chunky 90s workstation vocabulary into a portable, framework-agnostic CSS kit. Every component reads as a tiny window: silver chrome with a 2-pixel composite bevel, a navy title bar, square corners, hairline dividers, and pixel-tight typography sitting on a teal desktop.

The system is built for dense, information-rich product surfaces — admin panels, dashboards, developer tools, and editorial UIs that benefit from a strong visual personality without sacrificing legibility. Density, mechanical interaction, and zero radius are the load-bearing decisions; everything else (palette, type, motion) flows from them.

## Colors

The palette is strict and intentional. Eight tokens cover the entire system, with three of them dedicated to the 3D bevel that defines every surface.

- **Desktop Teal `#008080`** — the page background. Every screen "boots up" on the desktop.
- **Window Silver `#c0c0c0`** — the chrome of every panel, button face, tab strip, and status bar.
- **Title Navy `#000080`** — active window title bars and the primary accent. Also used for syntax keywords.
- **Bevel Highlight `#ffffff`** — top + left edges of raised surfaces; bottom + right edges of sunken ones.
- **Bevel Shadow `#808080`** — bottom + right edges of raised surfaces; top + left of sunken ones.
- **Deep Shadow `#000000`** — outer frame edge, body text, inset field shadows.
- **Workspace Paper `#ffffff`** — input fields, code wells, document surfaces.
- **Alert Yellow `#ffff00`** — reserved as a single signature accent for warnings and highlight chips. Used sparingly so it never reads decorative.

Foreground on silver is always Deep Shadow; on navy it is on-primary white. Disabled text uses Bevel Shadow with a 1px white text-shadow to create the etched OS feel.

## Typography

Three faces, each with a clear role. All sizes are small by modern web standards — 11-14px is the working range — to preserve the workstation density.

- **VT323** — display headings and the signature terminal screen. Large pixel-bitmap face used for hero numerals, the terminal output, and oversize section markers.
- **Pixelify Sans** — every piece of UI chrome: button labels, tabs, menu items, captions, body. Crisp pixel sans that holds up at small sizes.
- **JetBrains Mono** — code blocks, line numbers, mono data fields. Replaces the original `Courier New` mood with something sharper.

Line-height is tight (`1.15` for headings, `1.3` for snug UI, `1.45` for body). Letter-spacing is left at zero except for VT323 display, where `0.02em` keeps the bitmap rhythm. Avoid anti-aliasing where possible — `-webkit-font-smoothing: none` is applied at the root to preserve the pixel character.

## Layout

Layout is dense, pixel-aligned, and rectangular. The system uses an 8-step spacing scale (`2-4-6-10-16-24-32-48`) where the small end (2-10px) handles intra-component breathing and the larger end (16-48px) handles section rhythm. Window padding is intentionally tight (`10px`) so content dominates the chrome.

Surfaces stack as nested windows. The `.c95-desktop` wrapper paints the teal background; `.c95-window` provides the chunky outer frame; `.c95-window__body` carries content; an optional `.c95-window__body--inset` creates the recessed paper well used for documents, code, and forms. Tabs and panels follow the same anatomy.

Controls share a fixed vertical rhythm — `20px / 24px / 28px` for sm/md/lg — so buttons, inputs, selects, and checkboxes line up on a single baseline in dense toolbars.

## Elevation & Depth

There are no blurred drop-shadows in this system. Depth is communicated entirely with a composite bevel built from four `inset box-shadow` layers, encoded once as `--c95-bevel-out` and inverted as `--c95-bevel-in`.

- **Raised** (`bevel-out`): white top + left, gray bottom + right, with a black outer-edge cue on primary buttons.
- **Sunken / pressed** (`bevel-in`): the inverse — used for active buttons, input fields, code wells, and inset content panels.
- **Field** (`bevel-field`): a deeper variant that adds a black shadow for input wells so the carpet of paper feels recessed two layers down.
- **Thin** (`bevel-out-thin` / `bevel-in-thin`): 1px versions for small chrome — tab caps, title-bar controls, status-bar cells.

Hover is intentionally flat (no color change, no shadow shift) — the OS feedback is the cursor change. Active state always swaps `out` for `in` and nudges content one pixel down-right, giving controls a real mechanical click.

## Shapes

Zero radius across the entire system. Every corner is square, including buttons, inputs, checkboxes, tags, scrollbar thumbs, and icon containers. The radius scale exists in the tokens (`none/sm/md/lg/xl/full`) but every value resolves to `0` so downstream code that references a radius token cannot accidentally introduce a curve.

Iconography stays squared and outlined — Tabler Icons in their default line weight — and ornament is limited to bevels, hairlines, dotted focus marquees, and the optional title-bar gradient (navy → light navy) on active windows.

Focus is a 1px dotted black marquee drawn just inside the element, not a soft ring. It deliberately echoes the OS keyboard-focus dotted outline.

## Components

All component classes are prefixed `c95-`. Tokens, class names, and `metadata.json` use consistent semantic naming.

**Button** (`.c95-button`)
Silver face, 2px outset bevel, square corners, Pixelify Sans label. The `--primary` modifier adds a 1px black outline to mark the default action. Active state swaps to inset bevel and nudges content one pixel. A `--ghost` variant strips the chrome for inline secondary actions. An `--icon` modifier squares the button to host a single Tabler icon, useful for title-bar controls and toolbar groups.

**Input** (`.c95-input`, `.c95-textarea`, `.c95-select`)
White paper field with the deep `bevel-field` recess, sitting inside a silver panel. Labels (`.c95-field__label`) sit above the field in Pixelify Sans. Focus draws an inset 1px navy border in addition to the field bevel. The `.c95-input-group` composite supports inline addons (units, helper buttons) that retain their own raised bevel inside the recessed field.

**Card / Window** (`.c95-window`)
The window is the canonical card. It carries a title bar with optional icon and a control cluster (`min`, `max`, `close`) implemented as small beveled mini-buttons, an optional menu strip, a body, and an optional status bar with cell-style sunken slots. Use the `--inset` body modifier when the card hosts a document or code well.

**Checkbox & Radio** (`.c95-check`, `.c95-radio`)
14×14 square with the field bevel and a black pixel-check glyph when on. The radio uses the same square treatment with a filled inner square instead of a circle — no rounded radios anywhere in the system. Both inherit the marquee focus ring.

**Tabs** (`.c95-tabs`)
Folder-tab metaphor. Tab caps sit on top of an outset panel; the active tab raises 1px, hides its bottom edge so it merges with the panel body, and shows a 1px navy underline cue along its top. The tab strip is keyboard-friendly via `aria-selected`.

**Signature: CRT Terminal** (`.c95-terminal`)
The signature element. A full window with a navy title bar wraps a black `screen` painted in VT323 phosphor-green type with a faint scanline overlay and a blinking block caret. Inline token classes (`__keyword`, `__comment`, `__string`, `__prompt`) carry the same syntax DNA as the silver code block but recoded for the CRT palette. Use it for terminal demos, hero animations, and live-output panels.

**Code block** (`.c95-code`)
A sunken paper well for inline code, with the original mid-90s syntax palette preserved as `tok-*` classes (comment green, keyword blue, string gray, two magenta variable tints).

**Icons**
Exactly one icon library is used system-wide: **Tabler Icons** (https://tabler.io/icons, MIT). Outline weight, currentColor stroke. Icons sit inside `.c95-window__ctrl`, `.c95-button--icon`, status-bar cells, menu items, and signature `.c95-window__title` markers. Do not invent SVG paths — only copy official Tabler markup. Size with `.c95-icon`, `.c95-icon--md`, `.c95-icon--lg`.

**Accessibility**
Contrast ratios pass AA on the core combinations (black-on-silver, white-on-navy, black-on-paper). All interactive components expose `focus-visible` states. Real semantic elements are used (`<button>`, `<input>`, `<label>`, `<section>`, `<article>`, headings) so screen readers receive proper roles even though the visual treatment is heavy. Keep target sizes at the `lg` control height (28px+) for primary actions.

## Do's and Don'ts

**Do**
- Treat every container as a window: outer bevel, optional title bar, body, optional status bar.
- Keep type small (11-14px) and tightly leaded — the density is the personality.
- Use the bevel tokens consistently: raised = action available; sunken = data/document/pressed.
- Reserve Alert Yellow for one or two surfaces per screen — warnings, focus chips, marquee callouts.
- Use Tabler Icons in outline weight at 14-18px to match the chrome rhythm.

**Don't**
- Don't introduce border-radius, soft shadows, blur, or gradients beyond the navy title-bar sweep.
- Don't use color as the hover signal — hover stays flat; active inverts the bevel.
- Don't mix icon libraries or invent custom SVG glyphs.
- Don't enlarge body type past 14px to "modernize" the look — the workstation feel depends on density.
- Don't apply Alert Yellow to large surfaces or use it as a brand color — it must read as a single accent.
