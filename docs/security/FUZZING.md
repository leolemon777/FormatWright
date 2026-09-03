# Fuzzing

- Status: Phase 5 development harness
- Updated: 2026-08-10

Anole uses `cargo-fuzz`/libFuzzer for two untrusted-input boundaries:

- `engine_manifest`: arbitrary JSON deserialization followed by complete protocol/schema/path/capability invariant validation.
- `structured_file`: arbitrary JSON, YAML, CSV, or XML bytes through the bounded native inspector, including strict duplicate-key and XML DTD/entity policy.

The fuzz workspace is isolated from the release Cargo workspace so libFuzzer and `unsafe` sanitizer support cannot become application dependencies.

## Run

On a supported nightly Rust toolchain with LLVM sanitizer support:

~~~text
cargo install cargo-fuzz --version 0.13.2 --locked
cargo +nightly fuzz run engine_manifest -- -runs=100000 -max_len=65536
cargo +nightly fuzz run structured_file -- -runs=20000 -max_len=65536
~~~

Seed corpora are committed under `fuzz/corpus`. Crashes and generated corpus growth remain local artifacts unless a minimized, non-sensitive regression fixture is reviewed and committed.

## CI policy

The scheduled/manual fuzz workflow runs bounded campaigns on Linux. Ordinary pull-request CI still compiles and tests deterministic regressions; it does not pretend a short fuzz budget proves absence of parser defects.

## Recorded Windows development campaigns

On 2026-08-10, cargo-fuzz 0.13.2 ran with Rust nightly `1.99.0-nightly (12c36e253 2026-08-10)` and the Visual Studio 2022 x64 AddressSanitizer runtime:

| Target | Runs | Final coverage/features | Peak RSS | Result |
|---|---:|---:|---:|---|
| `engine_manifest` | 10,000 | 1,043 / 1,765 | 100 MiB | No crash, timeout, or OOM |
| `structured_file` | 20,000 | 4,357 / 7,155 | 397 MiB | No crash, timeout, or OOM; 90 seconds |

libFuzzer's generated corpus growth was removed after the run; only the two reviewed seeds remain committed. This keeps the repository deterministic while the campaign can regenerate coverage-oriented inputs. A future release should retain a content-hashed minimized corpus artifact in CI rather than relying only on this development record.

Before Public Beta, retain campaign duration, executions per second, sanitizer configuration, corpus hashes, crashes/timeouts/OOMs, and minimized regressions as release evidence. The scheduled Linux campaign, longer Windows budgets, and macOS runs remain platform-matrix work.
