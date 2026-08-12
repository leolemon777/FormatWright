# ADR-0002: Conversion engines run as isolated subprocesses first

- Status: Accepted
- Date: 2026-08-10
- Owners: FormatWright maintainers
- Related requirements: FW-FR-020 through FW-FR-023, FW-FR-050 through FW-FR-052

## Context

FFmpeg, libvips, LibreOffice, Pandoc, PDF tools, and ExifTool have different release cadences, crash behavior, licenses, and platform packaging. Linking every native API into one process would expand the crash and license boundary before the product has evidence that FFI is necessary.

## Decision

The first implementation invokes exact executables with structured argument arrays and a minimal explicit environment. It never constructs a Shell command. Each task receives an isolated working directory and process group.

First-party adapters map typed plans to arguments and parse progress. Third-party plugins use a versioned JSON/NDJSON stdio protocol. Arbitrary native dynamic libraries are not loaded in v0.1.

## Consequences

- Engine crashes can be contained and reported.
- Cancellation and process-tree cleanup require platform-specific implementations.
- Process startup overhead is accepted.
- Engine provenance and build configuration remain visible.
- Hot paths may move to FFI only after profiling and a new ADR.

## Verification

- Injection tests use hostile filenames and options.
- Windows Job Object and Unix process-group tests prove descendant cleanup.
- Repository policy forbids Shell invocation in production adapters.

## Revisit when

Profiling proves subprocess overhead prevents a published performance requirement, and a specific FFI boundary has an acceptable safety and license design.

