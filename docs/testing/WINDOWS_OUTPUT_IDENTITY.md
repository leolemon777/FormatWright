# Windows Output Identity Evidence

- Status: R-005 closed on Windows development host
- Updated: 2026-08-12
- Scope: durable output reservation, queue preflight, and commit-path resolution

## Contract

One Win32 destination can have only one active durable reservation, even when callers spell it through another case, a regular versus `\\?\` disk path, an existing 8.3 ancestor, lexical dot components, or a directory reparse point. The commit path uses the same resolver as the reservation path.

Anole checks the original components before asking Windows for an absolute path, because Win32 can trim ASCII spaces and final periods during path handling. Components containing trailing dot/space, leading ASCII space, reserved device stems (`CON`, `NUL`, `COM1`… including superscript digits), alternate data streams/reserved characters, UNC paths, and device namespaces are rejected.

The resolver lexically removes regular `.`/`..`, finds the deepest existing ancestor, asks the filesystem for its final canonical path, then appends the validated nonexistent suffix. This expands existing short names and resolves symlink/junction ancestors without requiring the complete future parent path to exist.

Before a queued job enters `Inspecting` or starts a worker, `validate_output_reservation` re-resolves the path. A changed link target becomes `Blocked / OUTPUT_IDENTITY_CHANGED`; no output is written. A narrow filesystem race can still occur after this preflight and before the atomic no-overwrite rename, so commit retains its destination-exists check.

## Migration

SQLite migration v3 rebuilds every nonterminal reservation from the prior durable key inside one transaction. If two v2 spellings collapse to one Windows identity, startup fails safely with `OUTPUT_CONFLICT`; the migration marker and reservation table roll back together instead of silently choosing an owner.

## Automated evidence

The Windows tests cover:

- case, regular/verbatim disk path, lexical alias, and nonexistent-parent collisions;
- rejection of leading/trailing trimming aliases, reserved device names, ADS, UNC, and device namespaces;
- a real directory symlink resolving to the same reservation;
- retargeting that symlink after enqueue, both at the Job Store boundary and through a real queue window before worker execution;
- v2→v3 collision rollback;
- the same identity policy in `runner::resolve_output_path` before commit.

The behavior follows Microsoft's [Naming Files, Paths, and Namespaces](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file) rules and uses final-path canonicalization semantics described for [GetFinalPathNameByHandle](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlea).
