# Frozen Product Decisions — Decision Memo

- Status: **Awaiting owner approval** — every item below carries a recommendation
- Created: 2026-08-16
- Blocking: Public Beta (SPEC_PLAN §20.2), code signing, first public repository/name claim
- Rule: none of these may be silently defaulted by code; each needs an explicit owner decision recorded in this file

## How to decide

Review each row, edit the **Decision** field (or write an alternative), and date it. Items marked 🔴 block release engineering immediately once overdue; 🟡 blocks before Public Beta; 🟢 is reversible later.

| # | Decision | Options | Recommendation | Rationale | Blocks |
|---|---|---|---|---|---|
| 1 | Product name | keep `FormatWright` / rename | **Keep FormatWright** after a paid trademark search in target markets (US/EU/CN); `formatwright.com/.dev/.org/.io` are all taken, so pick `formatwright.app` / `getformatwright.com` or similar and update public metadata | Engineering preflight (2026-08-10, `NAME_CLEARANCE.md`) found no exact-name collision on GitHub/crates.io/npm/web; domains are taken but discoverable alternatives exist. Renaming later costs more the longer we wait | 🔴 |
| 2 | GitHub organization + package names | reserve `formatwright` org / alternative | Reserve the org and `formatwright` on crates.io + npm **before** any public mention | Both were free at preflight; squatting risk grows with visibility | 🔴 |
| 3 | Minimum Windows/macOS/Linux versions | Win11-only / Win10+ / older | **Windows 10 21H2+, macOS 13+, Ubuntu 24.04 LTS** for Beta | WebView2 supports Win10; Win11-only needlessly halves the audience; Win10 EOL (Oct 2025) means 21H2 with ESU is the floor worth supporting | 🟡 |
| 4 | Code signing | Authenticode OV cert / EV cert / none-for-Beta | **OV certificate now (~$200–400/yr), EV before Stable** | OV unblocks SmartScreen gradual reputation building; EV (~$300–700/yr, hardware token) gives instant reputation and is the Stable gate; unsigned Beta erodes the "verifiable" promise | 🔴 |
| 5 | Timestamp service | included with cert / separate | Any RFC-3161 timestamp, **mandatory in the release workflow** | Unsigned-timestamped builds expire with the certificate | 🟢 |
| 6 | Engine pack distribution default | embed Starter + optional downloads / download-on-demand | **Embed Starter (current behavior), user-triggered optional packs** | Already implemented; offline-first promise argues against download-on-demand defaults | 🟢 |
| 7 | PDF default engine | Poppler (current) / PDFium / both | **Keep Poppler for v0.1**; evaluate PDFium only if a certification gap appears | Poppler path is fully evidenced (GW-09, dimensions semantics R-010); switching now invalidates calibrated evidence | 🟢 |
| 8 | Test-corpus licensing model | generated-only / licensed fixtures / both | **Generated fixtures in-repo policy (current) + per-file licensed fixtures recorded in manifests** | Current policy already matches `test-corpus/README.md`; only hosting choice remains: keep in-repo manifests, host licensed binaries externally | 🟡 |
| 9 | AGPL service repo boundary | monorepo path / separate repo | **Separate `formatwright-server` repo created when Phase 6 starts** | Keeps Apache-2.0 core unambiguous; a directory in this repo invites license confusion | 🟢 |
| 10 | Release key ceremony owner + backup policy | single owner + offline backup / dual control | **Two-person integrity: one offline-generated seed, split backup (e.g. 2-of-3 paper/hardware), ceremony recorded in `RELEASE_KEYRING_CEREMONY.md`** | Ed25519 seed is single-point-of-supply-chain-trust; dual control matches the "auditable engines" promise | 🔴 |
| 11 | Transitive review sign-off authority | project maintainer / external counsel | **Maintainer signs file-level inventory; external counsel reviews only FFmpeg/Poppler legal questions** | Full counsel review of every file is cost-prohibitive; targeted review covers the actual risk (codec patents, LGPL/GPL boundaries) | 🔴 |
| 12 | Non-redistributable component policy (if FFmpeg build fails review) | switch FFmpeg build / demote Media pack to optional | **Demote to optional pack first (fast), switch build second (correct)** | Keeps a shippable Starter in every scenario; per SPEC_PLAN §13.2 no unclear-license engine enters the certified set | 🟡 |

## Owner decision — 2026-08-16

- **Drag-and-drop simplicity (HowToConvert-style) and Explorer one-click convert (FileConverter-style) are both in scope.**
- Not in scope: matching HowToConvert's thousands of format pairs.
- Right-click **Convert to X** counts as Plan approval; Open-in remains preview-only.

## Already-decided (recorded for completeness)

- Apache-2.0 (core/CLI/desktop/SDK), AGPL-3.0 (future hosted service), CC BY 4.0 (docs) — SPEC_PLAN §13.1.
- DCO, Conventional Commits, SemVer, ADR-required architecture changes — SPEC_PLAN §13.3.
- No telemetry, no cloud accounts, no in-v0.1 format-count race — SPEC_PLAN §2.5.

## Post-decision actions (owner or agent executes once approved)

1. Record name/org/domain decisions in a new ADR (`0012-product-identity.md` or higher).
2. Purchase certificate; wire `generate_checksums.py` + signtool into the RC workflow; update `release-candidate.yml`.
3. Run the release key ceremony per `docs/security/RELEASE_KEYRING_CEREMONY.md`; commit the resulting public keyring; sign the Starter manifests.
4. Update `SPEC_PLAN.md` §20.2 and `MASTER_EXECUTION_PLAN.md` §4.1 “冻结产品决策” to checked.
