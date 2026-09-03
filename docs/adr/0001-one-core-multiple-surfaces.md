# ADR-0001: One Rust core powers every product surface

- Status: Accepted
- Date: 2026-08-10
- Owners: Anole maintainers
- Related requirements: FW-FR-001 through FW-FR-052

## Context

Anole plans a desktop application, CLI, local REST API, self-hosted worker, and MCP adapter. Duplicating inspection, planning, execution, recovery, or validation logic would produce inconsistent behavior and weaken test evidence.

## Decision

All domain behavior lives in formatwright-core. Surfaces translate input and render events but do not implement conversion rules. The CLI, desktop commands, server handlers, and MCP adapter call the same typed APIs.

The core must not depend on Tauri, HTTP, React, or terminal presentation.

## Consequences

- A conversion plan is identical across supported surfaces.
- Core APIs and event schemas require explicit versioning.
- UI-specific shortcuts cannot bypass policy or validation.
- The core crate must expose testable boundaries for process and storage adapters.

## Verification

- Cross-surface contract tests compare Plan JSON for identical inputs.
- A repository search must not find engine argument construction outside the core or engine adapter crates.

## Revisit when

Only revisit if a platform cannot host the Rust core and a formally versioned remote-core protocol is required.

