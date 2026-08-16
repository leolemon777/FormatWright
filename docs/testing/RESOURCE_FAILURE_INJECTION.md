# Resource Failure Injection Evidence (Batch E)

- Status: Pass on Windows development host (2026-08-16); cross-platform pending
- Suite: `scripts/test_resource_failure_injection.ps1` (elevation required for the VHD scenarios; `-SkipVhd` runs the permission scenario only)
- Related: `docs/testing/SANDBOX_TESTS.md` (conflict/cancel coverage), SPEC_PLAN FW-NFR-005/008

## What is injected

| Scenario | Injection | Expected contract |
|---|---|---|
| `permission-denied` | `icacls /deny` write-data/append-data on the output directory | typed non-zero exit, no committed output, no staged partial, job `failed` (never `completed`) |
| `disk-full` | Dedicated 96 MiB VHD volume (diskpart, selected **only by vdisk file path** — never by disk number) squeezed to < 1 MiB free via `fsutil file createnew` fillers | same contract |
| `volume-removed` | VHD detached between a successful control conversion and the next run | typed exit 2 (`INPUT_INVALID`), nothing persisted (`not-persisted` is the honest outcome for a validation-stage rejection), no output/partial |

Every scenario also asserts the input file hash is unchanged after the failure, and the shared failure contract accepts only documented exit codes (1/2/4/5/8) — never 0.

## Recorded run (2026-08-16)

Evidence: `.artifacts/resource-failure-injection/suite-aa0bcf5eb3a9465f96079a8f62cb17d9/summary.json`

- `permission-denied`: exit 5, job `failed`
- `disk-full`: volume at 512,000 bytes free (input 7,940,813 bytes), exit 5, job `failed`
- `volume-removed`: exit 2, `not-persisted`
- VHD detached and deleted afterwards; drive letter reclaimed; input hash identical before/after all scenarios.

## Boundary

- Engine-level failure surfaces as `EXECUTION_FAILED` (5); validation-stage rejections correctly never create a job. No fake success, no orphaned partial, no input mutation observed in any scenario.
- Not covered here (separate gates): power loss mid-write, SQLite WAL/backup/restore matrix, long soak, removable volume yanked **during** a write (only between runs is deterministic).

## Operational notes

- `delete vdisk` is unreliable on some diskpart builds when combined with `detach vdisk` in one script; the suite detaches first, retries delete, then falls back to plain file removal of the detached VHD (verified safe).
- The elevation check uses the Windows principal API (`WindowsPrincipal.IsInRole`), not `whoami`, so the suite works from any shell.
