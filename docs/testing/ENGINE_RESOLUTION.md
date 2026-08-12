# Production Engine Resolution Evidence

- Status: R-009 fixed; clean-machine release closeout pending R-008
- Updated: 2026-08-12
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
