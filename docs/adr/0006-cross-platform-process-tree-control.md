# ADR-0006: Cross-platform process-tree control

- Status: Accepted for Phase 1
- Date: 2026-08-10
- Owners: FormatWright maintainers
- Related requirements: FW-FR-021, FW-FR-023, FW-NFR-004

## Context

Conversion adapters can create descendants. Killing only the direct adapter process can leave encoders, helpers, or document workers running, and those processes may continue writing staged output after a job is reported cancelled.

## Decision

- Every Unix adapter starts as the leader of a new process group using Tokio's safe `process_group(0)` API.
- Cancellation sends SIGTERM to the entire group, waits 750 ms, then sends SIGKILL to any surviving group before reaping the direct child.
- Windows cancellation targets the exact child PID and descendants with `taskkill /PID <pid> /T /F`, then waits for the owned child. If that mechanism fails, the direct child is force-killed through Tokio.
- Calls are made with typed executable and argument arrays; production adapters never use a shell.
- Final output commit remains impossible after a cancellation request, and the deterministic staged path is cleaned on best effort.

## Consequences

Unix descendants inherit a cancellation boundary without unsafe hooks. Windows relies on an operating-system tree command rather than an in-process Job Object in the Phase 1 slice; forced termination is therefore immediate rather than graceful. Detached processes that deliberately escape an assigned Unix group are outside the v0.1 adapter contract and must be rejected during adapter certification.

## Verification

- The Unix unit test starts a shell-owned descendant inside the adapter group and proves it cannot write a delayed survivor marker after termination.
- The Windows FFmpeg sandbox records the exact CLI process and uses forced-crash injection to prove the process tree and partial output are gone before recovery.
- Linux, macOS, and Windows workspace tests run in CI; Unix-specific tests compile and execute only on Unix runners.

## Revisit when

- A supported Windows adapter spawns descendants before `taskkill` can enumerate them.
- Graceful Windows cancellation becomes necessary for container finalization.
- An adapter legitimately needs a detached helper or creates another session/process group.
- Native sandbox brokers provide a stronger lifecycle primitive on every supported platform.
