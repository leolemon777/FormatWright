# FormatWright User Guide

- Status: Development guide; no Public Beta support claim
- Updated: 2026-08-12

> Current Alpha limitation: the local Windows candidate now includes a hash-verified PDF/Media Starter pack and Release ignores unrelated tools from `PATH`, but the packs and installer are not yet signed or release-certified. Clean-VM, SBOM, license/source-offer, upgrade/rollback, and Authenticode gates remain open. Do not use the Alpha on the only copy of irreplaceable data.

## First local conversion

1. Open the desktop application and drop a file or choose **选择文件 / Choose file**.
2. Review the recommended target and the non-overwriting output suggestion.
3. Select **检查并预览计划 / Inspect and preview Plan**.
4. Read the detected format, engine steps, and preserved/changed/dropped/unknown fields. Expert mode shows typed arguments, never shell text.
5. Start conversion. A successful destination is committed only after required validation passes.
6. Open **Reports** for checks or **Jobs** for durable history. Closing and reopening the UI recovers active jobs as interrupted rather than pretending they completed.

PDF-to-image conversion selects an output directory because every page is rendered. Other current workflows select one output file. Existing destinations are refused; silent overwrite is not supported.

On an installed Windows development build, Explorer's classic context menu (normally **Show more options** on Windows 11) has two kinds of FormatWright actions:

- **Open in FormatWright** on any file or folder only fills the Convert page. It does not start work.
- **Convert to …** on supported extensions (PDF→PNG/JPG, images→WebP, JSON/CSV/YAML/XML, common media) is an explicit approval, same as a one-shot CLI `convert`. FormatWright still builds a Plan, validates, and refuses to overwrite. Unsupported files stay review-only.

Network shares are rejected by the current local-path policy. Choosing a Convert verb on a folder is ignored.

## Presets

Open **Presets** to name and save the current target, quality, width, DPI, color mode, and compatible-stream policy. Applying a preset updates conversion settings but never stores or changes input/output paths. Edit uses the preset's stable ID; delete requires a second confirmation click.

**Import presets** and **Export presets** use a versioned JSON library suitable for migration between machines. Import validates the entire file before changing local settings, refuses unknown fields and duplicate names, and never accepts shell commands. Keep a separate copy before moving to a future incompatible major schema.

## Engines

**Engines** runs Doctor without downloading anything. The Windows candidate installs its embedded Starter packs into a versioned application-data store on first launch. A local engine pack can also be imported by selecting its `manifest.json`; FormatWright verifies protocol, platform/architecture, canonical paths, executable/runtime hashes, and declared license files, then copies only declared files into the same store and atomically switches the active registry record. Every pack is re-verified at startup. Doctor, Plan steps, and ValidationReport all show the same derived certification. A trusted signature alone is displayed as “signature trusted, review incomplete”; `Certified` requires that trust **and** a completed human supply-chain review. Hash completeness or a present signature never promote a pack.

For development only, an exact system executable can be selected before startup with `FORMATWRIGHT_ENGINE_<NAME>`, such as `FORMATWRIGHT_ENGINE_FFMPEG` or `FORMATWRIGHT_ENGINE_PDFTOPPM`. Release ignores `PATH` and these overrides; production capability comes only from an activated verified pack.

## CLI essentials

~~~text
formatwright inspect INPUT
formatwright plan INPUT --to FORMAT --output PATH
formatwright convert INPUT --to FORMAT --output PATH
formatwright doctor
formatwright batch-images INPUT_DIRECTORY --output-dir DIRECTORY --to webp
formatwright jobs list
formatwright jobs batches
formatwright jobs select --state failed --search TEXT
formatwright jobs selection SELECTION_ID
formatwright jobs bulk SELECTION_ID --action retry
formatwright jobs recover
formatwright jobs run --limit 100
formatwright engines verify PACK/manifest.json
formatwright --state-db PATH maintenance status
formatwright --state-db PATH maintenance backup BACKUP.sqlite3
formatwright --state-db PATH maintenance bundle-backup BACKUP.fwstate --include-reports
formatwright --state-db PATH maintenance integrity-check
formatwright --state-db PATH maintenance restore BACKUP.sqlite3
formatwright --state-db PATH maintenance bundle-restore BACKUP.fwstate
formatwright --state-db PATH maintenance bundle-restore BACKUP.fwstate --yes
formatwright --state-db PATH maintenance compact
~~~

Add `--json` for machine-readable output and `--state-db PATH` for an explicit durable queue. Use `convert --dry-run` to inspect a Plan without running it. `Ctrl+C` requests cancellation and prevents admission of further queued work.

Automation may add `convert ... --queue-only --idempotency-key KEY`. Repeating the same key with the same input, Plan, and normalized output returns the original queued Job; reusing it for a different intent is rejected. The Desktop generates and reuses a submission key while retrying a failed queue request.

`jobs select` freezes the ordered matching job IDs before a bulk action. `jobs bulk` rechecks each current state and reports transitioned, state-skipped, and output-conflict-skipped counts; completed jobs are never silently retried. The Desktop Jobs page uses the same snapshot-and-action path for its filtered bulk buttons. `batch-images` reports a durable batch ID that can be listed with `jobs batches`.

Every successful immediate conversion, `jobs run` item, and image-batch item stores `reports/JOB_ID.json` beside the selected SQLite state database before recording its terminal state. Desktop uses its application-data Reports directory. A missing or invalid report is a recovery problem, not a successful conversion claim.

`maintenance restore BACKUP` is the SQLite-only path. `maintenance bundle-backup BACKUP.fwstate` packages a consistent SQLite copy, presets, persisted UI settings, and engine-registry identities; `--include-reports` also includes bounded report JSON. Third-party engine binaries are intentionally excluded, so missing paths must be re-imported on another machine.

`maintenance bundle-restore BACKUP.fwstate` is preflight-only: it rejects unsafe/undeclared paths, links, duplicate members, size violations, SHA-256 mismatches, invalid component JSON, and an incompatible SQLite copy before live state changes. Stop queue execution and close other FormatWright processes, review the warnings, then rerun with `--yes`. Confirmed restore retains a full pre-restore safety bundle under `backups` and uses a recovery journal; CLI/Desktop startup rolls back an interrupted multi-component switch before opening the queue. Bundle and manual database backup destinations are never overwritten.

## Supported development workflows

The 12 golden workflows and their current platform/certification status live in `docs/specs/FORMAT_SUPPORT_MATRIX.md`. “Experimental” means a real fixture passed on the named platform; it is not a cross-platform Certified claim.

## Recovery

- A process killed while running becomes `interrupted` after `jobs recover` or desktop restart.
- Staged partial outputs are removed; the destination is never reported complete early.
- Use `jobs resume JOB_ID` for blocked/interrupted work or `jobs retry JOB_ID` for failed/cancelled/interrupted work.
- Input fingerprints and every engine version/hash are checked again before resumed execution.

See `docs/TROUBLESHOOTING.md` for typed errors and recovery actions, and `PRIVACY.md` before sharing reports.
