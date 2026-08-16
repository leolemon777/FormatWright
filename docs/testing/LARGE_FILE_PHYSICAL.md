# Physical Large-File Evidence (FW-NFR-001 physical variant)

- Status: Pass on Windows development host (2026-08-16); cross-platform pending
- Suite: `scripts/test_large_file_physical.ps1`
- Related: `docs/testing/LARGE_FILE.md` (sparse logical-size harness), SPEC_PLAN FW-NFR-001

## What this adds over the sparse harness

`test_large_file.ps1` proves control-plane memory does not grow with **logical** size using sparse files (only the prefix carries real bytes, and the remux effectively streams the small real prefix). This suite builds a **genuinely allocated** ≥ 10 GiB valid MKV by stream-copying a real 60-second chunk 1,728 times, asserts on-disk allocation equals logical size (locale-independent `GetCompressedFileSizeW` check), and pushes every real byte through the CLI identity/inspect/remux path with a parent-process RSS gate and an independent ffprobe verdict.

## Recorded run (2026-08-16)

Evidence: `.artifacts/large-file-physical/physical-1a177b08dfa64dcf8fddb72070460987/physical-large-file-summary.json`

| Metric | Value | Gate |
|---|---:|---|
| Physical input (allocated = logical) | 10,747,106,438 bytes | ≥ 10 GiB |
| Assembly throughput | 295.8 MiB/s over 34.7 s | — |
| `identify` peak control-plane RSS | 2,584,576 bytes (2.5 MiB) | ≤ 160 MiB |
| `convert` remux peak control-plane RSS | 14,934,016 bytes (14.2 MiB) | ≤ 160 MiB |
| Remux end-to-end | 254.7 s at 80.6 MiB/s combined R/W | — |
| Output | 10,780,283,175 bytes MP4, independent ffprobe `mov,mp4,…`, duration 103,716.278 s (1728 × 60 s ± 2%) | Pass |
| Job state | `completed`, no staged partial left | Pass |

The parent process stays below 15 MiB while streaming ~21.5 GiB through the engine subprocess, which is the FW-NFR-001 claim in its strongest form: memory is bounded by control-plane state, not input size.

## Boundary

- One development host, one run; the sparse harness remains the cheap CI-side regression, this one is the expensive physical gate to rerun per release candidate.
- Remux only: transcode at 10 GiB is a v0.2 concern once encoding profiles land.
- Disk-full, removable-volume, and low-memory variants remain open under batch E.
