# 10,000-Conversion Release Soaks

## Linux runner (2026-09-03, conda-forge engines)

- `ten_thousand_conversions` (structured): **pass**, wall 530.93 s.

## Windows development host (2026-09-03)

- `mixed_ten_thousand_conversions` (fair bounded scheduling, structured+image+media
  with injected blocks and repairs): **pass** - completed 10,000/10,000,
  outputs 10,000, reports 10,000, staged outputs remaining 0, resumed-after-repair
  20/20, peak control-plane RSS ~70 MiB, queue latency p95 ~341 s over the full
  bounded schedule. Starter media pack at FFmpeg 9.0.1-essentials (repinned; the
  9.0 archive now 404s upstream) with hash-verified manifests.
