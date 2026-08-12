# Security Policy

## Supported versions

FormatWright has not reached a supported public release. Security fixes currently target the main development branch.

## Reporting a vulnerability

Until a private repository advisory channel is configured, do not publish exploit details. Contact the project maintainers privately and include:

- FormatWright version or commit.
- Operating system and architecture.
- Engine name, version, and build configuration.
- Minimal reproduction steps.
- Whether a crafted file is required.
- Expected impact.

Do not attach sensitive user files. Prefer a synthetic reproducer.

## Security boundaries

The main application treats conversion engines and input files as potentially unsafe. The current development builds do not yet claim a complete OS sandbox. Release claims must match the controls proven by docs/security/THREAT_MODEL.md and the release checklist.

