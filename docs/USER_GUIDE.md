# FormatWright User Guide

- Status: Development guide; no Public Beta support claim
- Updated: 2026-08-12

> Current Alpha limitation: the recorded Windows installer contains no conversion engines and may discover unrelated development tools from PATH. It is not an out-of-box usable build. Use only a known development setup or a manually verified/imported pack until R-008/R-009 close; do not use irreplaceable originals.

## First local conversion

1. Open the desktop application and drop a file or choose **选择文件 / Choose file**.
2. Review the recommended target and the non-overwriting output suggestion.
3. Select **检查并预览计划 / Inspect and preview Plan**.
4. Read the detected format, engine steps, and preserved/changed/dropped/unknown fields. Expert mode shows typed arguments, never shell text.
5. Start conversion. A successful destination is committed only after required validation passes.
6. Open **Reports** for checks or **Jobs** for durable history. Closing and reopening the UI recovers active jobs as interrupted rather than pretending they completed.

PDF-to-image conversion selects an output directory because every page is rendered. Other current workflows select one output file. Existing destinations are refused; silent overwrite is not supported.

## Presets

Open **Presets** to name and save the current target, quality, width, DPI, color mode, and compatible-stream policy. Applying a preset updates conversion settings but never stores or changes input/output paths. Edit uses the preset's stable ID; delete requires a second confirmation click.

**Import presets** and **Export presets** use a versioned JSON library suitable for migration between machines. Import validates the entire file before changing local settings, refuses unknown fields and duplicate names, and never accepts shell commands. Keep a separate copy before moving to a future incompatible major schema.

## Engines

**Engines** runs Doctor without downloading anything. A local engine pack can be imported by selecting its `manifest.json`. FormatWright verifies protocol, platform/architecture, canonical paths, executable hashes, and declared license files, then stores only a reference. It re-verifies the pack at startup. Imported packs remain `Unverified` until the release keyring cryptographically trusts their signatures.

For development only, an exact system executable can be selected before startup with `FORMATWRIGHT_ENGINE_<NAME>`, such as `FORMATWRIGHT_ENGINE_FFMPEG` or `FORMATWRIGHT_ENGINE_PDFTOPPM`. The target Release will ignore PATH and these overrides unless explicitly running in a developer mode; production capability comes only from an activated verified pack.

## CLI essentials

~~~text
formatwright inspect INPUT
formatwright plan INPUT --to FORMAT --output PATH
formatwright convert INPUT --to FORMAT --output PATH
formatwright doctor
formatwright batch-images INPUT_DIRECTORY --output-dir DIRECTORY --to webp
formatwright jobs list
formatwright jobs recover
formatwright jobs run --limit 100
formatwright engines verify PACK/manifest.json
~~~

Add `--json` for machine-readable output and `--state-db PATH` for an explicit durable queue. Use `convert --dry-run` to inspect a Plan without running it. `Ctrl+C` requests cancellation and prevents admission of further queued work.

## Supported development workflows

The 12 golden workflows and their current platform/certification status live in `docs/specs/FORMAT_SUPPORT_MATRIX.md`. “Experimental” means a real fixture passed on the named platform; it is not a cross-platform Certified claim.

## Recovery

- A process killed while running becomes `interrupted` after `jobs recover` or desktop restart.
- Staged partial outputs are removed; the destination is never reported complete early.
- Use `jobs resume JOB_ID` for blocked/interrupted work or `jobs retry JOB_ID` for failed/cancelled/interrupted work.
- Input fingerprints and every engine version/hash are checked again before resumed execution.

See `docs/TROUBLESHOOTING.md` for typed errors and recovery actions, and `PRIVACY.md` before sharing reports.
