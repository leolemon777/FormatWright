# Desktop Queue Bridge Test

- Status: Phase 1 architecture evidence
- Date: 2026-08-10
- Platform observed: Windows 11 x64

## 1. Claim under test

The desktop control plane can receive a 10,000-job Rust event stream without hydrating or rendering the entire queue in the WebView. This test covers the bounded Rust-to-WebView projection, not the later Phase 3 requirement for 10,000 durable SQLite jobs with pause, resume, and retry.

## 2. Build contract

A real embedded-asset Tauri binary must be built through the Tauri CLI:

~~~powershell
pnpm install --frozen-lockfile
pnpm --dir apps/desktop tauri build --debug --no-bundle
~~~

Directly running a binary produced by plain `cargo build` is not a distribution test: in a development build Tauri may retain `devUrl` and attempt to open the Vite server. The real-window check therefore treats the Tauri CLI build path as part of the test contract.

## 3. Automated assertions

Backend tests in `apps/desktop/src-tauri/src/queue_bridge.rs` assert:

- Exactly 10,000 jobs are emitted.
- Batch size is 250 or less.
- Exactly 40 monotonically numbered batches reach the final sequence.
- Invalid zero or oversized requests are rejected.

Frontend tests in `apps/desktop/src/queueProjection.test.ts` assert:

- A 40-batch burst schedules one paint callback.
- The final projected total is 10,000.
- At most 100 rows are retained for rendering.
- Duplicate and out-of-order batches do not regress state.

## 4. Real-window procedure

1. Build using the command in section 2.
2. Launch `target/debug/formatwright-desktop.exe` as a normal Windows desktop application.
3. Verify that the embedded Anole page appears; a localhost error page is a failure.
4. Invoke **Run 10,000-job benchmark** through the accessible button.
5. Verify the visible summary, aggregate counts, final batch number, and 100-row preview.
6. Invoke the benchmark a second time without reloading the window to detect listener leaks or stale state.
7. Close the application and confirm no Anole window remains.

## 5. Recorded evidence

The 2026-08-10 Windows run produced:

| Observation | First run | Second run |
|---|---:|---:|
| Jobs | 10,000 | 10,000 |
| Batches | 40 | 40 |
| Backend emit time | 19 ms | 21 ms |
| Final batch visible | 40 | 40 |
| Projected count | 10,000 | 10,000 |
| Visible rows | first 100 | first 100 |

The accessible document also exposed Completed 1,000, Active 2,000, Failed 1,000 and individual rows `bench-00000` through `bench-00099`. The second invocation completed in the same window, providing direct evidence that the UI remained interactive after the first burst.

## 6. Remaining certification work

- Repeat the real-window check on macOS and Linux.
- Add a packaged installer smoke test once bundling is enabled.
- Exercise 10,000 real SQLite jobs, paging, pause, resume, retry, crash recovery, and bounded RSS in Phase 3.
- Capture release-grade machine-readable timing and memory artifacts rather than relying on this architecture-spike observation.
