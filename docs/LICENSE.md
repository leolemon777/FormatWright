# Repository Licensing Map

- Rust core, CLI, desktop application, and engine SDK: Apache License 2.0 (`LICENSE`).
- Documentation authored for this repository: Creative Commons Attribution 4.0 International ([legal code](https://creativecommons.org/licenses/by/4.0/legalcode)).
- Planned self-hosted service: AGPL-3.0-only in a separately licensed package when that package is introduced.
- Generated fixtures: their manifest entry controls; no fixture may be committed without an explicit redistributable license or a reproducible generator.
- Third-party engines: never covered by the repository's Apache license. Their own notices, source offers, hashes, and distribution decisions live under `engines/` and in each engine pack.

SPDX headers or package metadata are authoritative when a subtree has a more specific license.
