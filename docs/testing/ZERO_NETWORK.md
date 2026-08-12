# Zero-Network Development Gate

- Status: Phase 5 Windows development evidence
- Updated: 2026-08-10

## Enforced controls

- Every first-party planner emits `network_policy: deny`.
- The runner rejects any deserialized or future Plan requesting `explicit-allow` before reading its steps or destination.
- FFprobe and FFmpeg receive `-protocol_whitelist file,pipe`.
- Pandoc receives `--sandbox=true`; Office OOXML external relationships are rejected before LibreOffice execution.
- Raw UNC / `//server/share` input and output paths are policy-blocked before canonical filesystem access. Mapped network drives cannot be identified portably and remain a documented limitation.
- Desktop Doctor and engine import never download automatically.

## Windows observation harness

`scripts/test_zero_network.ps1` generates a synthetic local media fixture, asserts the public Plan is network-denied, launches the real CLI and FFmpeg process tree, and samples every observed descendant with `Get-NetTCPConnection` and `Get-NetUDPEndpoint` at 50 ms intervals. It requires a committed validated output and zero observed TCP/UDP endpoints.

~~~powershell
cargo build -p formatwright-cli
pwsh -NoProfile -File scripts/test_zero_network.ps1
~~~

## Evidence boundary

Polling cannot prove that a socket was not opened and closed entirely between samples. It also cannot distinguish a mapped drive backed by a network filesystem. Public Beta certification therefore still requires an OS-enforced no-network environment (for example an isolated Linux network namespace/firewall policy), syscall or ETW evidence, and repetition for every bundled engine pack on every claimed platform.

## Recorded Windows result

The latest case `zero-network-suite-81864aa583f441fe8181f0bec5b3cd5b` passed with a real MPEG-2/MP2 Matroska → H.264/AAC MP4 transcode and validated commit. The harness took 18 samples at 50 ms, observed three process-tree PIDs at maximum, and found zero TCP connections or UDP endpoints.

The first run exposed and fixed a policy regression: Windows canonical local disk paths such as `\\?\E:\...` were initially mistaken for UNC paths during reinspection. Regression tests now distinguish `VerbatimDisk` from `UNC`/`VerbatimUNC`, so local long paths remain supported while raw network shares are blocked before I/O.
