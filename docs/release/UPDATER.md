# Updater Signing and Release Channel

- Status: active (dev keypair generated 2026-09-01)
- Scope: desktop `tauri-plugin-updater` configuration, key management, release publishing

## Current state

- `apps/desktop/src-tauri/tauri.conf.json` enables `bundle.createUpdaterArtifacts`,
  pins the updater `pubkey`, and points `endpoints` at
  `https://github.com/leolemon777/FormatWright/releases/latest/download/latest.json`.
- The **release keypair** (rotated 2026-09-01 from the dev pair, strong random
  password) lives at `target/updater-keys/formatwright-release.key` (private)
  and `.key.pub` (public, embedded in the config). The password is stored next
  to it in `RELEASE_KEY_PASSWORD.txt`. `target/` is git-ignored: neither the
  private key nor the password ever enters the repository. The superseded
  dev pair (`formatwright-updater.key`, empty password) is retained only for
  testing and must not sign any published channel.
- The Settings page exposes a "Check for updates" action that reports available
  versions without auto-installing (alpha posture; the app stays zero-network
  except for this explicit user-initiated check, executed by the Rust side and
  therefore unaffected by the WebView CSP).

## Publishing an update

1. Generate a **release** keypair with a strong password and store the private
   key in a password manager plus the CI secret `TAURI_SIGNING_PRIVATE_KEY`
   (and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`):
   `pnpm --dir apps/desktop tauri signer generate -w <path>`.
2. Replace the `pubkey` in `tauri.conf.json` with the release public key
   **before** the first signed release ships (switching keys later orphans
   every installed copy).
3. Build with signing; the bundle emits `.sig` signature files next to the
   updater artifacts.
4. Publish `latest.json` (version, notes, platform URLs pointing at the
   signed artifacts) as a GitHub Release asset named exactly `latest.json`.

## Security notes

- Update packages are minisign-verified against the pinned public key; a
  compromised endpoint cannot push code without the private key.
- The dev keypair above must be treated as **test-only**: regenerate before
  any public release (see step 1) and never publish artifacts signed with it
  to the stable channel.
