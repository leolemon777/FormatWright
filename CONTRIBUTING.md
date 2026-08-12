# Contributing to FormatWright

FormatWright is early-stage software. Contributions should preserve the project principles in SPEC_PLAN.md: local-first operation, explicit loss, safe recovery, deterministic planning, and validation of every successful output.

## Before opening a change

1. Search existing issues and ADRs.
2. For a new format or engine, provide a real user workflow and licensed test samples.
3. For a behavior change, identify the affected requirement IDs.
4. Large architecture changes require an ADR or RFC before implementation.

## Local verification

Run:

~~~text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
~~~

Engine integration changes must also run the relevant golden-corpus tests.

## Commit policy

- Use focused commits.
- Use Conventional Commits where practical.
- Sign off commits to certify the Developer Certificate of Origin.
- Never commit proprietary, private, personal, or ambiguously licensed sample files.

## Security

Do not open public issues for suspected vulnerabilities. Follow SECURITY.md.

