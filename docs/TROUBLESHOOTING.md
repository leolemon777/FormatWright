# Troubleshooting

- Status: Development guide
- Updated: 2026-08-10

## Start with Doctor

Run `formatwright doctor --json` or open **Engines** in the desktop app. A missing engine is not downloaded automatically. Import a reviewed local pack, configure an exact `FORMATWRIGHT_ENGINE_<NAME>` path before startup, or install the engine through its official distribution channel.

If an imported pack becomes invalid, restore it at the recorded path or import an intact compatible pack. FormatWright rejects wrong architecture/protocol, missing license files, path escapes, tampered binaries, and a second manifest claiming an already registered executable name.

## Common typed errors

| Code or symptom | Meaning | Safe action |
|---|---|---|
| `OUTPUT_CONFLICT` | Destination exists or is reserved by active work | Choose another destination; do not delete the existing file unless you have verified it yourself |
| `ENGINE_MISSING` | Required local engine was not found | Run Doctor and select/import the exact engine |
| `ENGINE_INCOMPATIBLE` | Version, hash, platform, manifest, or runtime check failed | Do not bypass the check; restore or replace the engine pack |
| `INPUT_INVALID` | Content is malformed or outside a bounded parser contract | Correct the source; changing its extension does not change the detected format |
| `POLICY_BLOCKED` | Requested conversion would silently lose data or violate policy | Review the Plan and choose an explicit compatible target/policy |
| `OUTPUT_INVALID` | Engine exited but required independent validation failed | Keep the original; inspect the validation report and engine identity |
| `CANCELLED` | Cancellation reached the runner | Retry from the durable job only if the source and engine identities remain valid |

## Office and document conversion

LibreOffice runs with an isolated temporary profile. Close unrelated profile locks, confirm `soffice`, `pdfinfo`, and `pdftoppm` are available, and retry. External OOXML relationships are blocked rather than fetched. Markdown/HTML conversion also requires Pandoc. A PDF is accepted only after every page can be inspected and rendered.

## HEIC on Windows

The currently tested development fallback is libheif `heif-convert`. A libvips build advertising `heifload` may still lack an HEVC decoder; Doctor/version output is not enough. Use the real fixture gate in `scripts/test_heic_sandbox.ps1` before claiming support.

## Interrupted jobs and partial files

Run `formatwright jobs recover --state-db PATH` after an abnormal CLI exit. Desktop startup performs the equivalent interruption step automatically. Recovery deletes only deterministic staged files belonging to known job IDs and never the selected destination.

## Database integrity and restore

Run `formatwright --state-db PATH maintenance integrity-check` before attempting state repair. The check covers SQLite pages, foreign keys, migrations, queue reservations/events, and stored Plan hashes. Do not delete or recreate the database after a read error.

Create a portable copy with `maintenance backup BACKUP.sqlite3`; an existing backup path is refused. `maintenance restore BACKUP.sqlite3` only validates and migrates a temporary copy. Stop queue execution, close other FormatWright processes, and add `--yes` only after preflight succeeds. A confirmed restore first stores a pre-restore safety snapshot under `backups`. A schema newer than the running application is intentionally refused; install an equal or newer FormatWright release instead of forcing a downgrade.

## Reports and sensitive paths

Local reports intentionally include paths so recovery is explainable. Metadata values classified private/secret are redacted, but paths are not yet redacted. Do not attach raw reports to public issues; reproduce with a synthetic file and follow `SECURITY.md` for vulnerabilities.

## Development evidence

Run `python scripts/check_repository.py`, the Rust/TypeScript test suites, then only the relevant isolated script under `scripts/`. Test fixtures are created under ignored `.artifacts` directories. A passing Windows development script is not proof for another OS or engine build.
