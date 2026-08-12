# FormatWright Name and Namespace Preflight

- Status: Technical preflight only
- Checked: 2026-08-10
- Public release gate: Open

## Findings

At the time of the check:

- GitHub repository search returned no exact FormatWright repository name.
- crates.io returned HTTP 404 for formatwright.
- npm returned HTTP 404 for formatwright.
- General web search did not identify a file-conversion product using the exact name.
- formatwright.com, formatwright.dev, formatwright.org, and formatwright.io all resolved in DNS and must be treated as owned or controlled by someone else until proven otherwise.

## Consequences

- The codebase may continue using FormatWright as its working product and binary name.
- No repository metadata or Schema ID may claim an unowned domain.
- Schema IDs use the domain-independent urn:formatwright namespace.
- Public release remains blocked on a proper trademark search, jurisdiction decision, and selected domain/organization ownership.

## Before the first public release

- Perform a trademark search in intended markets.
- Confirm GitHub organization availability and reserve the selected organization.
- Reserve crates.io and npm names if still available.
- Choose and acquire a domain or update all public metadata to an owned domain.
- Search major app stores and package managers.
- Record the decision in a new ADR.

This document is an engineering preflight and is not legal advice.

