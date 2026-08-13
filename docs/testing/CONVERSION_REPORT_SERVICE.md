# Shared ConversionService and ReportService Evidence

- Status: Gate 1 shared service slice verified on Windows
- Updated: 2026-08-12
- Platform observed: Windows 11 x64 (development)

## Surface convergence

The CLI-local conversion router was deleted. CLI and Desktop now use Core `prepare_conversion`; immediate execution uses `ConversionService`; immediate/queued/batch report storage uses `ReportService` before terminal SQLite state.

`ConversionService` tests prove a real JSON→YAML run emits Running → Validating → Completed, commits output, and stores the exact report. A stale approved Plan hash creates neither Job nor output.

`ReportService` tests prove atomic replacement, interrupted-backup recovery, cross-Job ID rejection, a 16 MiB read/write bound, `REPORT_PERSIST_FAILED → Interrupted` before terminal success can be recorded, and a late destination creator cannot be clobbered during report publish.

## Disk-backed CLI evidence

Immediate run `.artifacts/conversion-report-service-e2e-20260812-2200`:

~~~text
state=completed output_exists=true report_exists=true
stdout_report_job=stored_report_job=SQLite_job_id
report_status=pass
~~~

Queued run `.artifacts/shared-services-e2e-20260812-2215`:

~~~text
state=completed completed=1 output_exists=true report_exists=true
report_job=queued_job_id report_status=pass last_event=VALIDATION_FINISHED
~~~

Both use an explicit state DB; reports live in its sibling `reports/` directory. This makes the output/report/job relationship inspectable and suitable for the later application-state bundle.

## Validation-only and export follow-up

- Report export is bounded to 16 MiB, defaults to path redaction, and publishes with no-overwrite semantics. Immutable recipes use the same bounded export boundary.
- Validation-only reopens the original input and existing output, reuses the format validators, and never executes conversion or commit steps. Changed input is rejected; changed output is reported without mutation.
- Each validation-only result is appended to SQLite schema v5. The original report file, original conversion terminal state, and conversion event history remain unchanged; Desktop report view/export select the newest evidence.
- Application-state bundles keep original report files opt-in. Revalidation evidence is an audit table inside SQLite and therefore follows every database backup even when original report files are excluded.

## Remaining work

- Repeat surface-equivalence E2E for every Certified route and platform.
- Add a browsable revalidation-history surface if operators need more than the newest evidence.
- Repeat surface-equivalence E2E for every Certified route and platform.
