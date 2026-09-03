# Privacy

Anole is designed for local file conversion. The v0.1 application does not upload input files, converted outputs, Plans, validation reports, filenames, usage events, or crash data. Conversion Plans use `network_policy: deny`, and the product has no analytics or advertising SDK.

## Data stored locally

The desktop application stores the following under the operating system application-data directory for the identifier `local.formatwright.desktop`:

- `jobs.sqlite3`: input/output paths, immutable Plan JSON, state, and ordered recovery events.
- `reports/<job-id>.json`: validation result, engine identity, checks, and output path.
- `engine-registry/*.json`: canonical paths to engine manifests imported by reference.

Language, basic/expert mode, and future non-secret display preferences use the local WebView storage. Conversion outputs are written only to the destination selected by the user. CLI state is written to the explicit `--state-db` path or its documented local default.

Local reports redact metadata values classified as private or secret, but currently retain local input/output paths (`paths_redacted: false`). Do not share a raw report if its paths are sensitive. Export-time path-redaction controls are not yet implemented and remain a Public Beta gate.

## Network behavior

Anole does not automatically download engines or updates in the current development build. Doctor inspects local or explicitly imported binaries. Engine-pack import reads local files and does not trust a signature merely because one is present.

Third-party conversion engines are separate programs with their own behavior. Anole supplies local paths and typed arguments, but the complete OS-level zero-network sandbox and release audit are not yet certified. Use the development build only with engines you trust, and review `docs/security/THREAT_MODEL.md` before processing hostile files.

## Deletion and retention

Deleting an output does not delete its local job/report history. The current desktop UI does not yet expose bulk history deletion. A user may remove the application-data directory after closing Anole to erase local history and imported-engine references; this does not delete original inputs, conversion outputs, or the referenced engine packs. Back up anything needed before doing so.

## Telemetry and future changes

There is no opt-out switch because telemetry is absent. Any future crash-report or diagnostic upload must be opt-in, show a redaction preview, identify the destination, and update this document before release.

Security issues involving privacy should follow `SECURITY.md`.
