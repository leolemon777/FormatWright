# Sandbox Test Harness

- Status: Normative test procedure
- Updated: 2026-08-12

## Purpose

The sandbox harness exercises destructive and failure-prone behavior only in a new, uniquely named directory under `.artifacts/`. It never uses a user document as input and never overwrites an existing test output.

## Windows FFmpeg suite

Prerequisites:

- A debug build at `target/debug/formatwright.exe`.
- `ffmpeg` and `ffprobe` on `PATH`.
- PowerShell 7 or later.

Run:

~~~powershell
cargo build --workspace
pwsh -File scripts/test_ffmpeg_sandbox.ps1
~~~

The script generates its own licensed synthetic fixtures and verifies:

1. H.264/AAC Matroska to MP4 chooses remux and copies both media streams.
2. The committed MP4 receives a passing `ValidationReport` and opens with an independent ffprobe invocation.
3. The input hash is unchanged, the report uses the durable SQLite job ID, and no staged file remains.
4. A pre-existing output is refused with `OUTPUT_CONFLICT` and remains byte-for-byte unchanged.
5. A Matroska file disguised as `.jpg` is detected from its header as MKV and emits `EXTENSION_MISMATCH`.
6. A SubRip stream that cannot be copied into the selected MP4 profile is blocked with `POLICY_BLOCKED` rather than silently dropped.
7. Timeout cancellation returns exit code 130, stores `cancelled`, terminates the child tree, and leaves no target or staged output.
8. Forced process-tree termination leaves a `running` job and staged output; `jobs recover` stores `interrupted`, records `RECOVERED_AFTER_RESTART`, removes the staged output, and never commits a target.
9. The recovered job follows `interrupted → queued → cancelled → queued` through public `resume`, `cancel`, and `retry` commands, with `JOB_RESUMED`, `USER_CANCELLED`, and `JOB_RETRIED` retained in order.

The command prints JSON and writes the same evidence to:

~~~text
.artifacts/sandbox-suite-<random>/summary.json
~~~

Generated `.artifacts` content is intentionally ignored by Git. Release CI must retain the summary and engine logs as workflow artifacts.

## Additional workflow suites

The same isolation and evidence rules apply to the focused audio, GIF, image, HEIC, structured-data, metadata, recursive-batch, document, PDF, and Office scripts listed in the repository README. In particular, `scripts/test_pdf_sandbox.ps1` creates only synthetic PDFs, renders into a unique case directory, independently checks every output page, and records its exact Poppler/ffprobe evidence boundary in `docs/testing/PDF_SANDBOX.md`. `scripts/test_office_sandbox.ps1` creates synthetic DOCX/PPTX/XLSX packages, converts them with an isolated LibreOffice profile, independently renders and decodes every PDF page, and records its boundary in `docs/testing/OFFICE_SANDBOX.md`. `scripts/test_heic_sandbox.ps1` reconstructs a fixed-hash upstream libheif corpus fixture only inside the ignored case directory and records its decoder boundary in `docs/testing/HEIC_SANDBOX.md`.

`scripts/test_multi_process_queue.ps1` uses separate CLI processes and isolated SQLite files to test process-level idempotency plus exact-once queue ownership. `scripts/test_queue_crash_recovery.ps1` targets only the newly launched, path-verified test runner process tree and only after observing its Job in Running with its deterministic partial. Their assertions and recorded cases are in `docs/testing/MULTI_PROCESS_QUEUE.md`.

`scripts/test_mixed_ten_thousand.ps1` is an opt-in Release gate. It creates 10,000 distinct structured/image/media inputs under one ignored case directory, activates the exact hash-verified Starter Media pack, executes the shared queue, injects and repairs 20 input-change blocks, samples RSS/WAL/staging, and independently probes 400 engine outputs. See `docs/testing/MIXED_TEN_THOUSAND.md` for the recorded evidence and small-file/high-resolution boundary.

The opt-in Rust release gate in `docs/testing/TEN_THOUSAND_CONVERSIONS.md` uses an OS temporary directory, creates 10,000 different inputs and outputs, and removes the complete fixture when the test process exits normally. It remains distinct from the smaller per-workflow scripts and is not part of ordinary CI.

## Evidence interpretation

A successful local run is evidence for the exact OS, binary hash, engine build, and assertions in its summary. It is not evidence for another operating system, another engine pack, the 10 GB gate, the 10,000-file gate, or any golden workflow not named by the harness.

## Latest development run

On 2026-08-10 the suite passed on Windows with 8.1.1 Gyan FFmpeg/ffprobe and the locally built debug CLI. The latest case `sandbox-suite-a23cc63b087140c39c800c0ddb33f160` recorded all six scenario groups as `pass` and the complete resume/cancel/retry event chain. This is development evidence only; certification still requires the release platform matrix and a pinned redistributable engine pack.
