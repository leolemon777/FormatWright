# JobRecoveryService Evidence

- Status: Desktop manual staging-cleanup slice verified on Windows
- Updated: 2026-08-12
- Platform observed: Windows 11 x64 (development)

## Boundary

`JobRecoveryService::cleanup_staging` accepts only a durable Job ID. It loads the output path from SQLite, revalidates/canonicalizes it as a local output identity, and derives the existing file/directory and short Office-workspace candidates from the Job ID. The Desktop frontend cannot supply a path to delete.

Cleanup is allowed only in Blocked, Failed, Cancelled, or Interrupted. Planned/Queued/Inspecting/Running/Validating are refused so runnable work cannot be deleted. Completed/Warning are also refused because an unexpected partial beside a successful final output requires inspection/quarantine rather than routine deletion.

SQLite holds an immediate writer transaction while state is checked and deterministic candidates are removed. A concurrent retry in another process therefore cannot move the job to Queued between the state check and cleanup. Every successful command appends an unchanged-state result event: `STAGED_OUTPUT_CLEANED` or `STAGED_OUTPUT_NOT_FOUND`.

Filesystem deletion and SQLite cannot be one cross-resource atomic commit. If deletion succeeds but the later audit write fails (for example, storage failure), the command returns failure instead of claiming an audited success; the next idempotent run records `STAGED_OUTPUT_NOT_FOUND`. The final output is never a cleanup candidate.

## Direct assertions

- an exact staged file is removed while the final output and a similarly named unrelated file remain byte-for-byte/present;
- the durable Job state is unchanged and one same-state cleanup audit event is appended;
- a second cleanup is an idempotent no-op and records `STAGED_OUTPUT_NOT_FOUND`;
- a Queued job is rejected before its staging file is touched;
- Completed output plus an unexpected staging artifact is preserved for future inspection/quarantine instead of routine deletion;
- a staging candidate replaced by a Windows reparse/symlink is refused and its external target remains untouched;
- a real second SQLite connection cannot retry the job until cleanup releases the writer transaction; the cleanup audit precedes the retry event;
- the Desktop surface requires a second confirmation click and disables cleanup during its queue window.

## Remaining gate work

- Add quarantine/inspection for an unexpected partial beside Completed/Warning output.
- Exercise permission loss, locked files, very large partial directories, and another-process contention on clean machines.
