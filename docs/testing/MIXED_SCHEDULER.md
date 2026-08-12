# Mixed Resource Scheduler Evidence

- Status: Phase 3 Windows development evidence
- Updated: 2026-08-10
- Script: `scripts/test_mixed_scheduler.ps1`

## Gate

The script creates one durable SQLite queue containing nine real conversions:

- three JSON record arrays to YAML (`lightweight`, 64 MiB reservations);
- three 1600×1000 PNG images to WebP (`cpu-heavy`, 1 GiB reservations);
- three 180-second 1920×1080 MPEG-2/MP2 MKV inputs to H.264/AAC MP4 (`cpu-heavy`, 1 GiB reservations).

Every job is planned and queued through the public `convert --queue-only` command. `jobs run --limit 9 --parallel 4` then delegates to shared `JobExecutionService::run_window`, which rechecks each engine identity and input fingerprint before execution. The deterministic scheduler has a 2 GiB default reservation budget, a maximum of four processes, a half-logical-CPU limit for CPU-heavy work, two I/O-heavy slots, one GPU slot, and an exclusivity key for LibreOffice.

The Windows observer samples every 50 ms. It measures the CLI parent RSS, descendant-tree RSS, SQLite WAL size, and FFmpeg processes whose command line contains the unique sandbox path. The command-line scope excludes unrelated FFmpeg processes and version probes.

## Recorded run

Run ID: `mixed-scheduler-suite-402efe46745d4aeaa1a1319ea1f0d304` (2026-08-11, after `JobExecutionService` extraction)

| Measurement | Result |
|---|---:|
| Queued / selected / completed | 9 / 9 / 9 |
| Configured / scheduler peak active | 4 / 4 |
| Real FFmpeg process peak | 2 |
| Durable running-state interval peak | 4 |
| Samples / interval | 248 / 50 ms |
| Parent peak RSS | 16,125,952 bytes |
| Process-tree peak RSS | 2,325,934,080 bytes |
| WAL peak / final database | 1,285,472 / 98,304 bytes |
| Outputs / staged remnants | 9 / 0 |

All jobs reached `completed`; none were blocked, failed, warned, or cancelled. The real process observation proves engine overlap without exceeding the effective two-job CPU/memory budget. This recorded run uses the conservative 1 GiB CPU-heavy reservation adopted after an earlier 512 MiB measurement proved too optimistic.

Prior development evidence: `mixed-scheduler-suite-e24e94268b30432c8b9375a67306462b` (parent RSS 14,602,240; tree RSS 2,321,453,056; WAL 1,318,432).

The durable running-state overlap is an upper bound, not a process count: milestone messages can wait while the control plane reinspects the next job. Real engine concurrency is therefore certified only by the separate command-line-scoped OS observation.

## Boundary

RSS and 50 ms polling are host-specific and may miss shorter peaks. This gate does not replace low-memory-machine tests, platform-specific process containment, GPU session tests, disk-full tests, or macOS/Linux campaigns. The 180-second source is generated under ignored `.artifacts` storage and is not committed.
