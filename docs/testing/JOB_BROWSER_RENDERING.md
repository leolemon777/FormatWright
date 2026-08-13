# Bounded Desktop Job Browser Evidence

- Status: Phase 4 implementation verified on Windows
- Updated: 2026-08-12

## Contract

The Desktop job browser never hydrates or mounts an unbounded history:

- the React client requests `JOB_PAGE_SIZE = 100`;
- the Tauri IPC boundary independently clamps every request to `1..=100`, so a modified WebView cannot request the Core store's larger internal page allowance;
- SQLite computes the total separately and returns a deterministic page ordered by update time and Job ID;
- filters and pagination continue to execute in SQLite rather than filtering a full history in JavaScript.

Each mounted row uses `content-visibility: auto` plus an intrinsic block-size estimate. The WebView can skip layout and paint for rows outside the viewport without unmounting their buttons or paths. The page is also layout/style-contained, limiting invalidation outside the job browser.

This deliberately uses strict server-side pagination plus native rendering isolation instead of a manual JavaScript window that would duplicate scrolling, focus restoration, variable-height measurement, and assistive-technology logic. At most 100 rows exist in the page DOM even when SQLite contains 10,000 or more jobs.

## Accessibility contract

The result container and rows expose `list` / `listitem` semantics. Every row carries global `aria-posinset` and `aria-setsize` values, so page 100 of a 10,000-job history announces positions `9901..10000` rather than pretending it is a new 100-item list. Repeated action labels include the durable Job ID, and truncated paths retain the full value in their title.

Native off-screen rendering does not remove rows from the DOM or tab order. A live Narrator/VoiceOver/Orca pass remains part of the broader Phase 4 accessibility gate.

## Direct verification

- Rust regression: missing, zero, ordinary, and oversized IPC limits resolve inside `1..=100`.
- Frontend regression: the page size is 100 and the last row of a 10,000-job result exposes position/size `10000 / 10000`.
- Frontend test, TypeScript build, and Vite production build pass.

