# FormatWright Threat Model

- Status: Initial
- Version: 0.1
- Updated: 2026-08-10
- Review gate: Public Beta

## 1. Protected assets

- User source files and outputs.
- File contents, names, paths, metadata, and secrets.
- Existing destination files.
- Host availability and resources.
- Job and validation integrity.
- Engine pack and update integrity.
- FormatWright signing identities.

## 2. Trust boundaries

1. User interface to Rust core.
2. Rust core to SQLite and task directories.
3. Rust core to conversion subprocess.
4. Core to third-party plugin protocol.
5. Application to optional update/engine download service.
6. MCP/API caller to authorized filesystem roots.

Input files and conversion engines are treated as potentially hostile. The Rust core is trusted only after its own input validation and tests.

## 3. Threats and required controls

| Threat | Example | Required control | Release evidence |
|---|---|---|---|
| Command injection | Filename contains quotes or Shell metacharacters | No Shell; typed argument arrays | Hostile-path integration tests |
| Path traversal | Plugin writes outside output root | Canonical path authorization and output reservation | Traversal/symlink tests |
| Existing-file loss | Race creates target after planning | Commit-time conflict recheck | Race tests |
| Malformed parser exploit | Crafted media/document | Current engines, process isolation, resource bounds | Fuzz corpus and security scan |
| Resource exhaustion | Huge dimensions, decompression bomb | Probe limits, scheduler reservations, timeout | Bomb simulations |
| Network exfiltration | HTML fetches remote image | Default protocol deny and network canary | Offline golden suite |
| Secret leakage | PDF password enters logs | Dedicated secret channel and redaction | Log/report assertions |
| Pack tampering | Replaced FFmpeg binary | Hash and signed manifest | Tamper tests |
| Malicious plugin | Arbitrary executable or capability claim | Explicit install, signature/status, permissions, runtime intersection | Plugin conformance tests |
| Confused deputy | MCP accesses arbitrary user path | Per-root allowlist, plan-first, confirmation | MCP authorization tests |
| Update compromise | Malicious release metadata | Signed release index, rollback protection, revocation | Update integration test |

## 4. Subprocess boundary

Minimum v0.1 controls:

- Exact executable path.
- Structured arguments.
- Minimal environment.
- Per-job working/profile directory.
- Process group or Job Object.
- Wall-time timeout and cancellation escalation.
- Resource policy.
- Protocol/network restrictions passed to engines.
- Bounded stdout/stderr capture.

Public Beta documentation must not claim a complete OS sandbox unless native platform sandbox tests prove it. Stronger sandboxing remains a Phase 5 objective:

- Windows restricted token/AppContainer feasibility.
- macOS sandbox profile and hardened runtime.
- Linux namespaces/seccomp/bubblewrap feasibility.

## 5. Secret handling

- Secrets enter through a dedicated in-memory type.
- Secret values are never serialized to Plan, Job, Event, history, report, command preview, or crash data.
- When an engine only accepts command-line secrets, the adapter is not certified until exposure is assessed; stdin or secure file descriptor is preferred.
- Temporary secret files use restrictive permissions and are deleted after engine start/exit.
- Secure erase is not promised on modern filesystems or SSDs.

## 6. Network policy

Default policy is deny:

- No URL inputs in v0.1.
- Document engines cannot retrieve HTTP resources.
- FFmpeg protocols are allowlisted to local file/pipe needs.
- Tauri Content Security Policy blocks external connections.
- Update and engine download paths are separate user actions.

Tests capture DNS, TCP, UDP, and proxy attempts where the platform permits.

## 7. Logging and diagnostics

- Stable error codes instead of dumping raw engine commands.
- Paths are redacted by default in export bundles.
- Engine stderr is size-limited and scrubbed for secrets.
- File contents and metadata values are not logged.
- Users preview diagnostic exports.

## 8. Residual risks

- Third-party native engines may contain vulnerabilities.
- Subprocess isolation is weaker than a proven OS sandbox.
- System-installed engines may be modified or built with unexpected features.
- Network filesystems may not provide expected atomic semantics.
- Format-level validation cannot prove semantic equivalence for every document.

Each residual risk must be visible in the support matrix and release notes.

## 9. Security release gates

- No open P0/P1 vulnerability.
- Dependency and engine vulnerability scan completed.
- Signed artifacts and SBOM generated.
- Zero-network golden tests pass.
- Command injection, traversal, symlink, overwrite-race, and tamper tests pass.
- Threat model reviewed against actual implementation.

