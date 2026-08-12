# 10 GiB Large-File Gate

- Status: Windows development gate implemented
- Updated: 2026-08-10

## Procedure

`scripts/test_large_file.ps1` creates new NTFS sparse fixtures under `.artifacts/` and compares bounded artifact identification for 1 GiB and 10 GiB logical files. It then extends a reproducibly generated H.264/AAC Matroska fixture to 10 GiB and runs:

~~~text
identify → inspect → plan → remux → independent inspect → validate → commit
~~~

The harness samples only the FormatWright parent process for the control-plane gate. Engine memory is a separate metric and must be bounded per certified engine profile.

Run:

~~~powershell
cargo build --workspace
pwsh -File scripts/test_large_file.ps1
~~~

## Gates

- Logical input size is exactly 10,737,418,240 bytes.
- Parent peak working set is at most 160 MiB.
- A 10 GiB `identify` run may use at most 32 MiB more than the 1 GiB baseline.
- The input Probe preserves the 10 GiB size and detects Matroska.
- The Plan selects remux for compatible H.264/AAC.
- The output independently opens as MP4 and the stored job is `completed`.
- No staged output remains.

## Latest development evidence

The 2026-08-10 Windows run recorded:

| Measurement | Result |
|---|---:|
| 1 GiB identity parent peak | 2,584,576 bytes |
| 10 GiB identity parent peak | 7,929,856 bytes |
| Growth from 1 GiB to 10 GiB | 5,345,280 bytes |
| 10 GiB conversion parent peak | 10,702,848 bytes |
| 10 GiB end-to-end elapsed | 39,575 ms |
| Validation / independent format | pass / MP4 family |
| Durable job state | completed |

This proves the Windows control-plane path for the tested sparse fixture and binary hash. Release certification still requires physical-media fixtures, low-memory hardware, Linux/macOS runs, and engine-process memory reporting.
