# Job Identity, Commit, and Recovery Specification

- Status: Normative for v0.1
- Version: 0.1
- Updated: 2026-08-10
- Related requirements: FW-FR-020 through FW-FR-034

## 1. Resume terminology

FormatWright v0.1 promises **job and file-level recovery**, not arbitrary byte-level continuation.

- A completed and validated file is skipped after restart when its identity still matches.
- A file interrupted during an engine step is restarted from the beginning of that step.
- Byte-level continuation is permitted only when an adapter explicitly declares a checkpoint capability and has dedicated tests.
- Pause means no new jobs start. In-flight jobs follow the selected policy: finish current step or cancel and restart later.

The UI and documentation must not use wording that implies universal byte-level resume.

## 2. Input identity

### 2.1 Fast identity

Planning uses:

- Canonical resolved path.
- Filesystem identity when available.
- File size.
- High-resolution modified time.
- A BLAKE3 sample fingerprint over a versioned set of prefix, middle, and suffix regions for files above the full-hash threshold.

### 2.2 Full hash

A full BLAKE3 hash is required when:

- The workflow is metadata-clean and must prove payload stability.
- A released golden fixture is evaluated.
- A report is exported with full provenance enabled.
- A policy explicitly requires it.

A full hash is not mandatory before every 10GB conversion.

### 2.3 Change detection

Input identity is checked:

1. After initial inspection.
2. Immediately before engine launch.
3. Before final output commit when the engine reads a live source path.

If the source changes, the job becomes blocked with reason INPUT_CHANGED. Automatic re-plan requires explicit policy.

## 3. Plan snapshot

A runnable job stores:

- Probe schema version and snapshot.
- User constraints.
- Deterministic Plan and Plan hash.
- Exact engine ID, version, binary hash, and capability manifest hash.
- Expected output reservation.
- Validation policy version.

An application or engine upgrade does not silently change an existing job. A retry using new capabilities creates a new Plan revision linked to the original job.

## 4. Output reservation

Before execution, the scheduler reserves the canonical output path in SQLite. Two jobs cannot own the same path simultaneously.

Conflict policies:

- ask: job remains blocked until a decision exists.
- skip: existing output is inspected; it is not assumed equivalent.
- rename: deterministic suffix allocation occurs transactionally.
- overwrite: requires explicit recorded authorization.

Case-insensitive collision checks apply on case-insensitive filesystems. Windows reserved names and trailing dot/space normalization are handled before reservation.

## 5. Staging and commit

### 5.1 Same-filesystem staging

The preferred partial path is a hidden or clearly marked sibling of the destination file or output directory:

~~~text
destination-parent/.formatwright-partial-JOBID-FILENAME.ext
destination-parent/.formatwright-partial-JOBID-PAGE-DIRECTORY/
~~~

This preserves same-filesystem rename semantics. A multi-file workflow must validate the complete staged directory and commit it with one same-parent rename; it must not expose pages incrementally at the final path. If the destination parent cannot host staging, execution is blocked or uses a documented non-atomic fallback that requires explicit policy.

Office-to-PDF uses a short, unpredictable same-parent workspace named `.fw-<12-hex>` containing both the isolated LibreOffice user profile and generated PDF. The shorter adapter-specific name is required because LibreOffice may exit successfully without producing an output when its profile URL is too long on Windows. Recovery and cancellation enumerate only the exact deterministic candidate for the job; they do not glob-delete arbitrary `.fw-*` directories.

### 5.2 Commit sequence

1. Engine closes output successfully.
2. FormatWright flushes and closes owned handles.
3. Output, or every member of a multi-file output set, is independently re-probed.
4. Required validation completes.
5. Existing destination handling is rechecked.
6. Partial is renamed within the same filesystem.
7. Directory metadata is flushed where the platform offers a reliable primitive.
8. Job transition to completed or warning is committed.

The database must never record completed before the destination exists at its final path.

### 5.3 Cross-volume behavior

A cross-volume move is not called atomic. If a future workflow requires it, FormatWright copies to a destination-side partial, verifies the copy, and then performs the same-filesystem final rename.

## 6. Recovery matrix

| Observed state | Partial | Final output | Recovery action |
|---|---|---|---|
| queued/planned | none | none | Requeue after identity check |
| running | none | none | Mark interrupted; restart step |
| running | present | none | Validate only if adapter guarantees complete marker; otherwise remove/quarantine and restart |
| validating | present | none | Resume validation when Plan and engine snapshot match |
| validating | none | present | Inspect final; reconcile only with valid commit journal |
| completed | none | present | Revalidate lightweight identity on demand |
| completed | present | present | Quarantine unexpected partial and raise maintenance warning |
| any | none | conflicting final | Apply conflict policy; never assume ownership |

## 7. Cancellation

- Cancellation transitions first to a durable cancellation-requested event.
- The runner stops progress intake, requests graceful termination, waits a bounded period, then kills the process tree.
- A cancelled output is never committed.
- Partial cleanup failure produces a maintenance warning and records the exact path.
- Secrets and temporary profile directories are deleted on best effort without claiming secure erase on SSDs.

## 8. Batch memory bound

- SQLite holds the full queue.
- The scheduler hydrates a bounded window of jobs.
- UI queries are paginated.
- Events may be coalesced for display but durable state transitions are never dropped.

## 9. Symlinks and links

- Directory symlinks are not traversed by default.
- File symlinks are allowed only when the resolved target is inside the authorized input root.
- Output is a new regular file; hardlink identity is not preserved by default.
- Link decisions are included in the Plan.

## 10. Tests

The test suite must terminate the application at every numbered commit step and prove one of:

- The old destination remains intact.
- A valid new destination exists with matching validation evidence.
- A recognizable partial remains and recovery gives an actionable state.

No crash point may produce a false completed state.
