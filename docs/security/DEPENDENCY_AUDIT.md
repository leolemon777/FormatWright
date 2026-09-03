# Dependency Vulnerability Audit

Updated: 2026-08-10

Anole treats a known vulnerability in any locked Rust application, Rust fuzzing, or production JavaScript dependency as a failing release gate. The gate does not use advisory suppressions.

Run it from the repository root:

```text
cargo install cargo-audit --version 0.22.2 --locked
python scripts/audit_dependencies.py
cargo install cargo-deny --version 0.20.2 --locked
cargo deny check
```

The script checks `Cargo.lock` and `fuzz/Cargo.lock` against RustSec, then runs `pnpm audit --prod`. An unavailable registry, malformed response, missing tool, or nonzero audit failure cannot silently pass.

## Current evidence

The 2026-08-10 audit used RustSec database commit `2ae3ea41b89902e846595002ca29a91df471097d` and found:

- application Rust lock: 0 vulnerabilities across 522 dependencies;
- fuzz Rust lock: 0 vulnerabilities across 115 dependencies;
- production pnpm graph: 0 vulnerabilities at every reported severity.

The audit initially found `RUSTSEC-2026-0194`, `RUSTSEC-2026-0195`, and `RUSTSEC-2026-0009`. Updating `plist` from 1.8.0 to 1.10.0 removed `quick-xml` 0.38.4 and updated `time` to 0.3.55. Because that secure dependency set requires Rust 1.88, the workspace MSRV was raised from 1.85 to 1.88.

RustSec also reports informational warnings for the GTK3 dependency family and one affected `glib` API in Tauri's Linux WebView dependency graph. These are not vulnerability findings and are not suppressed. Anole does not call the affected `glib::VariantStrIter` APIs directly. They remain an upstream migration risk to review before a Linux release claim; a Windows-only build does not link that target-specific GTK graph.

`deny.toml` independently enforces an explicit SPDX license allowlist, denies unknown registries and Git sources, and rejects wildcard dependencies. It fails unmaintained direct workspace dependencies while reporting transitive duplicates and maintenance notices for review. The 2026-08-10 run passed all four cargo-deny checks: advisories, bans, licenses, and sources.

## Boundary

This gate covers repository lock files only. Imported engine packs have separate binaries and dependencies; each release candidate still requires the engine vulnerability review, SBOM, hashes, and revocation checks defined in `ENGINE_SUPPLY_CHAIN.md` and `RELEASE_CHECKLIST.md`.
