# Production Engine Resolution Evidence

- Status: R-009 fixed; clean-machine release closeout pending R-008
- Updated: 2026-08-16
- Platform: Windows x86-64

## Contract

- Debug/development discovery may use an exact environment override or PATH after registered packs.
- Release discovery uses only exact paths activated from a hash-verified pack.
- An absolute path that is not registered is rejected by `VerifiedPacksOnly`.
- Windows pack executables must be native `.exe` or `.com`; `.cmd` and `.bat` wrappers are rejected during verification.

## Regression evidence

The ordinary workspace suite passes with 82 non-ignored Rust tests after adding:

- `doctor::tests::production_policy_ignores_explicit_non_pack_paths`
- `doctor::tests::production_policy_selects_an_exact_registered_pack_path`
- `engine_pack::tests::rejects_windows_script_wrappers_as_pack_executables`

The first test was also compiled and executed under the Release profile. `cargo fmt --all --check` and workspace Clippy with warnings denied pass.

R-009 remains `Fixed`, not `Closed`, until the R-008 Starter pack is installed and used for an offline conversion on a clean machine with no system engines.

## Certification threading (ADR-0011)

Doctor, activated registry paths, Plan `engine.certification`, and ValidationReport `engines[]` share one promotion rule:

- `Certified` only when signature trust is `Trusted` **and** `sources.json.review_status` is `complete`.
- A trusted signature with incomplete review stays `Unverified` and is labeled “signature trusted, review incomplete”.
- The compiled-in release keyring is currently empty, so shipped Starter packs evaluate as `Unsigned` / `Unverified` after activation. That is intentional until the owner key ceremony.

Regression coverage:

- `formatwright_engine_sdk::derive_certification_requires_trusted_signature_and_complete_review`
- `engine_pack::activate_applies_embedded_keyring_without_promoting_unsigned_packs`
- `engine_pack::trusted_signature_promotes_only_after_complete_review`
- frontend `engine certification display`

## Negative discovery and install matrix (Gate U)

Automated regressions now cover the Gate U failure list on the development host (not a clean VM):

| Case | Evidence |
|---|---|
| Missing pack | `capabilities::backend_reports_missing_pack_for_a_supported_pdf_route`; `doctor::production_policy_ignores_explicit_non_pack_paths` |
| Hash tamper | `engine_pack::rejects_a_tampered_binary` / runtime / SBOM sidecars |
| Version incompatible | `engine_pack::rejects_an_incompatible_application_version`; `engine_pack::rejects_a_protocol_mismatch_before_activation`; `FormatWrightCompatibility::contains` |
| Revoked / invalid signature | `engine_pack::evaluates_signature_trust_against_a_release_keyring` |
| Half-install leftover | `engine_pack::leftover_partial_staging_is_not_published`; `engine_registry::leftover_partial_directories_are_not_installed_versions` |
| Failed upgrade | `engine_registry::failed_upgrade_does_not_move_the_active_pointer` |
| Malicious PATH / env override | `doctor::production_policy_ignores_development_overrides_and_polluted_paths`; Windows `.cmd`/`.bat` rejection |

`verify_engine_pack` now rejects packs whose `formatwright_compatibility` range does not contain `CARGO_PKG_VERSION`. Leftover `.partial` staging directories are not treated as installed versions and do not become the active pointer.
