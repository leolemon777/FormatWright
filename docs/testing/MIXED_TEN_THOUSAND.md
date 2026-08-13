# 10,000 Mixed Real Conversion Release Gate

- Status: Gate 1 verified on Windows development host
- Updated: 2026-08-12
- Script: `scripts/test_mixed_ten_thousand.ps1`
- Rust gate: `crates/core/tests/mixed_ten_thousand_conversions.rs`

## Workload and contract

The opt-in release gate creates 10,000 distinct local input paths and executes every Job through the shared `JobExecutionService`, normal engine/input reinspection, resource admission, deterministic partial path, validation, no-clobber commit, atomic report persistence, and durable terminal transition:

- 9,600 JSON record arrays → YAML through the native structured adapter;
- 200 PNG still images → WebP through the activated, hash-verified Starter Media pack;
- 200 one-second MPEG-2/MP2 MKV files → MP4 through the same verified pack.

Each workload class is a persisted batch. The first 256-row selection must distribute rows across all three lanes with a maximum one-row difference. The first real engine start in any batch must occur no more than 30 seconds after the first start in another batch.

Twenty inputs are changed after durable enqueue: ten structured, five image, and five media. They must become exactly 20 `Blocked / INPUT_CHANGED` Jobs, create no false outputs, then complete after the fixture bytes are repaired and the same immutable Plans are resumed. The final reconciliation requires 10,000 Completed Jobs, outputs, and reports with zero staged remnants.

The PowerShell observer prebuilds the Release test so compiler memory is excluded, activates the verified Release Starter Media manifest inside the test process, then samples every 100 ms. It records control-plane/process-tree RSS, SQLite WAL, and staged bytes/count. After success it independently opens every one of the 400 image/media outputs with the pack's exact `ffprobe` executable.

Run explicitly:

~~~powershell
pwsh -File scripts/test_mixed_ten_thousand.ps1
~~~

## Recorded Windows result

Case: `mixed-10000-suite-8a0cd2c1a0ad41f6a956dbd7ea415657`

| Measurement | Result |
|---|---:|
| Jobs (structured / image / media) | 10,000 (9,600 / 200 / 200) |
| Planning / execution | 5.178 s / 200.957 s |
| Throughput | 49.762 jobs/s |
| Queue latency P50 / P95 / max | 137.533 / 193.880 / 201.850 s |
| First 256 selection | 86 / 85 / 85 |
| Batch first-start spread | 1.791 s |
| Hydrated / configured parallel / peak active | 256 / 4 / 4 |
| Injected Blocked / repaired completion | 20 / 20 |
| Completed / outputs / reports / final partials | 10,000 / 10,000 / 10,000 / 0 |
| Control-plane / process-tree peak RSS | 70,877,184 / 73,494,528 bytes |
| SQLite DB / peak WAL | 54,312,960 / 48,092,792 bytes |
| Peak staged output | 1 file / 103,272 bytes |
| Input / output / report bytes | 26,503,238 / 21,661,200 / 32,613,200 |
| Independent output probes | 400 / 400 passed |

The execution time includes reinspection, engine version/hash checks, validation, 10,000 atomic report writes, the injected repair/retry window, and durable SQLite events. The original homogeneous 10,000 JSON→YAML gate remains useful for trend comparison; this gate closes the Windows mixed-workload/fairness/RSS/WAL evidence item.

## Boundary

This is host- and engine-build-specific development evidence. The 100 ms observer may miss shorter RSS/WAL/partial peaks, and small synthetic image/media files do not represent a 10,000-item high-resolution production corpus. It does not certify macOS/Linux, low-memory hosts, disk-full/removable-media behavior, physical 10 GiB sequential I/O, long-duration power loss, or signed engine-pack provenance.
