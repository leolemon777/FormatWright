# Requirement Traceability

- Status: Living document
- Updated: 2026-08-12

## 1. Rule

No requirement is complete until implementation and direct verification evidence are linked. A green broad test command is insufficient when it does not exercise the named invariant.

Progress snapshot for completed vs planned engineering work lives in [`docs/MASTER_EXECUTION_PLAN.md`](../MASTER_EXECUTION_PLAN.md) §1.1. This matrix stays requirement-centric.

## 2. Current map

| Requirement | Specification | Implementation target | Required evidence | Status |
|---|---|---|---|---|
| FW-FR-001/002 | CORE_SCHEMAS | `crates/core/src/inspect.rs` | Header-sniff unit test; wrong-extension FFmpeg sandbox | Verified on one platform for media slice |
| FW-FR-010–013 | CORE_SCHEMAS, GOLDEN_WORKFLOWS | `crates/core/src/planner.rs` | Determinism, hard subtitle constraint, remux/transcode unit and sandbox tests | Verified on one platform for GW-04 slice |
| FW-FR-020–023 | JOB_RECOVERY, THREAT_MODEL | `crates/core/src/runner.rs` | Typed argv, cancel, staged commit, Windows tree sandbox, Unix descendant test | In progress; injection corpus and cross-platform CI execution pending |
| FW-FR-030–034 | JOB_RECOVERY | `crates/core/src/job_store.rs`, `application/job_execution.rs`, `application/bulk_jobs.rs`, `application/revalidation.rs`, `maintenance.rs`, `application_state.rs`, runner no-clobber publish, CLI jobs/maintenance/batch-images, Desktop queue/bulk commands | 10k disk-backed transaction/paging, batch/idempotency/selection/bulk regressions, append-only revalidation evidence, reservation/transition/claim races, four-process exact-once execution, real forced crash/recovery, no-clobber publish, shared executor failures, online backup/restore/v3→v4→v5 snapshots, full-state bundle round-trip/tamper/journal recovery, disk CLI E2E | In progress; R-001–R-007 are Closed. Schema v5 durable batch/selection/bulk/revalidation audit, SQLite maintenance/concurrency, round-robin selection, atomic queue claim, no-clobber publish, and application-state bundle have direct Windows evidence. Long power-loss soak and cross-platform recovery remain |
| FW-FR-040–042 | VALIDATION_RULES | shared validation/report model plus media, image, structured, PDF, Office, and document validators | Public ValidationReport schema, unit tests, and independent Windows sandbox probes/renders | In progress; report schema and Windows experimental validators exist, while full licensed corpus, fidelity calibration, and cross-platform certification remain |
| FW-FR-050–053 | ENGINE_SUPPLY_CHAIN | `crates/engine-sdk`, Doctor, `crates/core/src/engine_pack.rs`, versioned pack store, production `EngineLocator`, Desktop capability snapshot | Live hashes/build flags, missing-engine diagnostics, pack integrity/runtime tamper tests, polluted-PATH negative test, `docs/testing/ENGINE_RESOLUTION.md`, `docs/testing/WINDOWS_STARTER.md`, offline clean-VM Starter conversions | In progress; R-008/R-009 are Fixed: Release uses exact activated paths, Windows packs reject script wrappers, the installer embeds pinned PDF/Media packs, first startup installs them atomically, and UI/backend gate routes from the same snapshot. Clean-VM conversion, trusted keyring, revocation, legal review and engine SBOMs remain |
| FW-NFR-001 | RESOURCE_SCHEDULER | runner and `scripts/test_large_file.ps1` | 1/10 GiB parent RSS comparison plus 10 GiB E2E report | Verified on Windows sparse fixture; physical/cross-platform runs pending |
| FW-NFR-002 | RESOURCE_SCHEDULER | SQLite queue, deterministic resource scheduler, `JobExecutionService`, `MaintenanceService`, bounded desktop projection | `DURABLE_QUEUE.md`, `QUEUE_BRIDGE.md`, `BATCH_SANDBOX.md`, `MIXED_SCHEDULER.md`, `MIXED_TEN_THOUSAND.md`, `JOB_EXECUTION_SERVICE.md`, `MAINTENANCE_SERVICE.md`, `SQLITE_CONCURRENCY.md`, `MULTI_PROCESS_QUEUE.md` | 10k mixed real conversion, P50/P95/RSS/WAL/staging, 20 recoverable failures, round-robin lanes, four-process exact-once claim, SQLite maintenance/concurrency, real-window projection, and larger-engine overlap verified on Windows; high-resolution/PDF/Office and cross-platform gates pending |
| FW-NFR-004 | JOB_RECOVERY | process runner | Timeout cancellation and exact process-tree crash injection | Verified on Windows development engine |
| FW-NFR-006 | THREAT_MODEL | network-denied Plan/runner policy and `scripts/test_zero_network.ps1` | Path-scoped process-tree TCP/UDP observation plus OS-enforced denial campaign | In progress; Windows observational zero-socket run passes, while OS-enforced isolation and macOS/Linux campaigns remain |
| Desktop E10 | UX_FLOWS | Tauri commands plus React workflow shell; queue via `JobExecutionService`/`QueueWindowControl`, bulk via `BulkJobService`, recovery via `JobRecoveryService`, validation-only via `RevalidationService` | `DESKTOP_MVP.md`, `JOB_BROWSER_RENDERING.md`, `JOB_RECOVERY_SERVICE.md`, `BATCH_SELECTION_BULK.md`, preset/execution evidence, Rust/TypeScript tests, production build | In progress; exact preview approval, report-before-terminal persistence, recovery banner, recoverable pause, per-job and stable-filter bulk actions, hard-bounded/native-isolated list rendering, live paging/enqueue, audited exact staging cleanup, bounded export, trusted output reveal, and validation-only are verified. Shell integration, screen-reader audit, and usability study remain |
| 10,000 real conversions | RESOURCE_SCHEDULER §9, v0.1 DoD | Atomic bulk queue transition plus bounded 128/256-job execution windows | `crates/core/tests/ten_thousand_conversions.rs`, `crates/core/tests/mixed_ten_thousand_conversions.rs`, `docs/testing/TEN_THOUSAND_CONVERSIONS.md`, `docs/testing/MIXED_TEN_THOUSAND.md`, `docs/testing/MIXED_SCHEDULER.md` | Pass on Windows for homogeneous 10,000 distinct JSON→YAML and mixed 9,600 structured + 200 image + 200 media; mixed fairness/P50/P95/RSS/WAL, 20 recovery cases, 400 independent probes and larger-engine overlap recorded. High-resolution/PDF/Office and cross-platform certification remain |
| Security/Packaging E11 | THREAT_MODEL, ENGINE_SUPPLY_CHAIN, release checklist | Network-denied runner/path policy, zero-socket observation harness, isolated fuzz workspace, scheduled sanitizer workflow, SPDX generator, locked-dependency audit, offline NSIS packaging/checksums, verified engine staging, privacy/user/recovery docs | `docs/testing/ZERO_NETWORK.md`, `docs/testing/WINDOWS_STARTER.md`, `docs/security/FUZZING.md`, `docs/security/DEPENDENCY_AUDIT.md`, `docs/release/SBOM.md`, `docs/release/WINDOWS_PACKAGING.md`, `PRIVACY.md` | In progress; Windows bounded fuzz/application SBOM/zero-vulnerability dependency/observational zero-network gates pass, and an unsigned installer with embedded Starter resources builds locally. Clean-VM installed conversion, engine SBOM/legal/signature gates, OS-enforced isolation, signed installers and cross-platform campaigns remain |
| GW-01 | GOLDEN_WORKFLOWS | libheif development adapter; libvips certification target | `scripts/test_heic_sandbox.ps1`, real HEVC HEIC fixture, independent ffprobe/Pillow checks and visual review | Experimental; Windows JPEG/PNG, content detection, typed constraints, cancellation/retry implemented; official libvips Windows HEVC preflight failed and certified libvips pack remains pending |
| GW-08 | GOLDEN_WORKFLOWS | native OOXML inspector, LibreOffice renderer, Poppler/native PDF validator | `scripts/test_office_sandbox.ps1`, independent all-page Poppler/Pillow checks, visual render review | Experimental; Windows DOCX/PPTX/XLSX path, isolated profile, macro/external-relationship policy, cancellation and immutable-Plan retry implemented; fidelity calibration/cross-platform corpus pending |
| GW-09 | GOLDEN_WORKFLOWS | Poppler inspector/renderer plus native pixel validator | `scripts/test_pdf_sandbox.ps1`, independent ffprobe/Pillow checks, visual render review | Experimental; Windows all-page PNG/JPEG, DPI/color/alpha policy, encrypted/malformed paths and atomic directory commit implemented; selection/transparency/cross-platform corpus pending |
| GW-04 | GOLDEN_WORKFLOWS | FFmpeg adapter | FFmpeg sandbox, 10 GiB harness, and unit tests | In progress; Windows remux/10 GiB/negative paths verified, multitrack and other platforms missing |
| GW-05/GW-07 | GOLDEN_WORKFLOWS | FFmpeg audio planner/runner/validator | `scripts/test_audio_sandbox.ps1`, planner tests, independent ffprobe | Experimental; Windows core paths implemented, full metadata/layout/cross-platform corpus pending |
| GW-06 | GOLDEN_WORKFLOWS | FFmpeg GIF planner/runner/validator | `scripts/test_gif_sandbox.ps1`, constraint tests, independent ffprobe | Experimental; Windows time/scale/fps/palette path implemented, crop/target-size/cross-platform corpus pending |
| GW-02 | GOLDEN_WORKFLOWS | FFmpeg development image adapter; planned libvips adapter | `scripts/test_image_sandbox.ps1`, planner tests, independent ffprobe | Experimental; Windows PNG/JPEG → WebP/AVIF path implemented, libvips and full color/metadata corpus pending |
| GW-03 | GOLDEN_WORKFLOWS | recursive image enumerator, SQLite queue, bounded resource scheduler | `scripts/test_batch_sandbox.ps1`, `scripts/test_mixed_scheduler.ps1`, `scripts/test_mixed_ten_thousand.ps1`, persistent job events, independent ffprobe/process observation | Experimental; Windows recursive pause/resume, changed-input, bounded concurrency and 10k small-file structured/image/media mix implemented; high-resolution/PDF/Office and cross-platform corpus pending |
| GW-11 | GOLDEN_WORKFLOWS | native Rust structured adapter | `scripts/test_structured_sandbox.ps1`, strict-parser unit tests, independent native re-inspection | Experimental; Windows JSON/YAML typed and CSV/XML flat-record paths implemented, mapping/encoding/cross-platform corpus pending |
| GW-12 | GOLDEN_WORKFLOWS | FFmpeg metadata-clean media adapter | `scripts/test_metadata_sandbox.ps1`, redacted Plan, independent ffprobe | Experimental media slice; private keys removed and unknown retained on Windows, image/PDF/type-specific corpus pending |
| GW-10 | GOLDEN_WORKFLOWS | Pandoc subprocess, native DOCX inspector, isolated LibreOffice renderer, Poppler/native PDF validator | `scripts/test_document_sandbox.ps1`, native/independent OPC checks, token digest, independent all-page PDF render and visual review | Experimental; Windows Markdown/HTML → DOCX/PDF, four-engine immutable Plan, cancellation/retry implemented; authorized resources/layout calibration/cross-platform corpus pending |

## 3. Evidence storage

Local generated evidence is written under .artifacts/ and excluded from Git. Release summaries and hashes are attached to release artifacts. Small deterministic snapshots may be committed under tests/snapshots/.

The reproducible Windows procedure and assertion inventory are in `docs/testing/SANDBOX_TESTS.md`.

The 10,000-job Rust-to-WebView projection evidence and the required Tauri build path are in `docs/testing/QUEUE_BRIDGE.md`.

The disk-backed 10,000-job transaction, paging, output reservation, and CLI action evidence is in `docs/testing/DURABLE_QUEUE.md`.

The strict structured-data mapping, semantic-digest validation, lossy-policy cases, and recorded Windows evidence are in `docs/testing/STRUCTURED_SANDBOX.md`.

The experimental image codec, resize, alpha, constraint, and independent-probe evidence is in `docs/testing/IMAGE_SANDBOX.md`.

The real HEVC HEIC fixture, libheif fallback, JPEG/PNG validation, wrong-extension/truncation paths, cancellation, durable retry, and failed Windows libvips HEVC capability preflight are in `docs/testing/HEIC_SANDBOX.md`.

The metadata classification, value-redaction, stream-copy, retained-unknown, and independent-probe evidence is in `docs/testing/METADATA_SANDBOX.md`.

The recursive enumeration, directory-junction refusal, pause/resume, output naming, and changed-input evidence is in `docs/testing/BATCH_SANDBOX.md`.

The offline Pandoc, bounded DOCX package inspection, required OPC parts, semantic token digest, remote-resource policy, isolated PDF pipeline, all-page render, cancellation, and durable-retry evidence is in `docs/testing/DOCUMENT_SANDBOX.md`.

The content-first PDF inspection, all-page Poppler rendering, deterministic directory commit, per-page dimensions, decoded color/alpha checks, and encrypted/malformed-input evidence is in `docs/testing/PDF_SANDBOX.md`.

The bounded OOXML inspection, macro and external-relationship refusal, isolated LibreOffice profile, all-page PDF validation, short same-parent workspace, cancellation, durable retry, and representative visual evidence is in `docs/testing/OFFICE_SANDBOX.md`.

The shared-core desktop commands, persistent job/report storage, bilingual basic/expert workflow, native startup, production build, frontend state tests, and pixel-layout review are in `docs/testing/DESKTOP_MVP.md`.

The shared durable-queue executor extraction evidence is in `docs/testing/JOB_EXECUTION_SERVICE.md`.

The SQLite status, full integrity, online backup, migration snapshot, isolated restore preflight, transactional restore, and CLI disk-backed evidence is in `docs/testing/MAINTENANCE_SERVICE.md`.

The immediate-writer reservation/transition races and cross-platform no-clobber file/directory publish evidence is in `docs/testing/SQLITE_CONCURRENCY.md`.

The schema v4 durable batch, idempotency, stable selection, audited bulk-action, migration, CLI E2E, and Desktop surface evidence is in `docs/testing/BATCH_SELECTION_BULK.md`.

The single Core planner, immediate conversion lifecycle, bounded atomic report persistence, CLI/Desktop convergence, and disk E2E evidence is in `docs/testing/CONVERSION_REPORT_SERVICE.md`.

The round-robin batch selection, process-level idempotent replay, four-process exact-once queue claim, and real force-kill/recover/resume evidence is in `docs/testing/MULTI_PROCESS_QUEUE.md`.

The trusted-Job-ID manual staging cleanup, state gate, exact candidate isolation, final-output preservation, idempotency, and audit-event evidence is in `docs/testing/JOB_RECOVERY_SERVICE.md`.

The 10,000 mixed structured/image/media execution, recoverable failure distribution, fairness, latency, RSS, WAL, staging, artifact reconciliation, and independent probe evidence is in `docs/testing/MIXED_TEN_THOUSAND.md`.

## 4. Status values

- Not started
- In progress
- Implemented, unverified
- Verified on one platform
- Certified
- Blocked
