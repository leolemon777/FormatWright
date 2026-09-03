use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use formatwright_engine_sdk::{
    Certification, EngineArchitecture, EngineManifest, EnginePlatform, ManifestLicense,
    ManifestSupplyChain, ReleaseKeyring, SignatureTrust, SupplyChainReviewStatus,
    derive_engine_certification, engine_provenance_message, verify_manifest_signature,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::doctor::sha256_file;
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};

pub const ENGINE_PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerifiedEnginePack {
    pub manifest: EngineManifest,
    pub manifest_path: PathBuf,
    pub manifest_sha256: String,
    pub executables: BTreeMap<String, PathBuf>,
    pub runtime_files: Vec<PathBuf>,
    pub supply_chain_files: Vec<PathBuf>,
    pub signature_present: bool,
    /// Signature verdict against a release keyring (ADR-0011). `None` means
    /// no keyring was supplied, so trust was not evaluated; a present
    /// signature alone must never be read as trust.
    pub signature_trust: Option<SignatureTrust>,
    /// Recorded `sources.json` review, or `Missing` when the sidecar is absent.
    pub review_status: SupplyChainReviewStatus,
}

impl VerifiedEnginePack {
    #[must_use]
    pub fn certification(&self) -> Certification {
        derive_engine_certification(self.signature_trust.as_ref(), self.review_status)
    }

    #[must_use]
    pub fn provenance_message(&self) -> String {
        engine_provenance_message(
            self.signature_trust.as_ref(),
            self.review_status,
            self.signature_present,
        )
    }
}

/// Compiled-in release keyring (ADR-0011). Empty until the owner ceremony
/// publishes a public key; an empty ring still evaluates `Unsigned` /
/// `UnknownKey` instead of pretending trust was not considered.
#[must_use]
pub fn embedded_release_keyring() -> ReleaseKeyring {
    ReleaseKeyring {
        schema_version: formatwright_engine_sdk::RELEASE_KEYRING_SCHEMA_VERSION,
        keys: Vec::new(),
        revocations: Vec::new(),
    }
}

fn apply_embedded_signature_trust(verified: &mut VerifiedEnginePack) {
    let Ok(now_unix_ms) = unix_now_ms() else {
        return;
    };
    let keyring = embedded_release_keyring();
    if keyring.validate().is_err() {
        return;
    }
    verified.signature_trust = Some(verify_manifest_signature(
        &verified.manifest,
        &keyring,
        now_unix_ms,
    ));
}

fn unix_now_ms() -> Result<u64> {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| {
                engine_error(
                    format!("system clock is before the Unix epoch: {error}"),
                    "Fix the system clock and retry.".to_owned(),
                )
            })?
            .as_millis(),
    )
    .map_err(|error| {
        engine_error(
            format!("system clock overflowed keyring timestamps: {error}"),
            "Fix the system clock and retry.".to_owned(),
        )
    })
}

/// Loads a pack manifest, validates static invariants, checks the host target,
/// hashes every executable, and verifies required license files.
///
/// This function does not claim cryptographic certification. A present
/// signature is reported but remains untrusted until the release keyring
/// verifier accepts it.
///
/// # Errors
///
/// Returns an engine error for malformed manifests, unsafe paths, target or
/// protocol mismatches, missing files, symlink escapes, or hash mismatches.
pub fn verify_engine_pack(manifest_path: impl AsRef<Path>) -> Result<VerifiedEnginePack> {
    let manifest_path = manifest_path.as_ref().canonicalize().map_err(|error| {
        engine_error(
            format!(
                "Engine manifest is unavailable: {}",
                manifest_path.as_ref().display()
            ),
            error.to_string(),
        )
    })?;
    let root = manifest_path.parent().ok_or_else(|| {
        engine_error(
            "Engine manifest has no pack directory".to_owned(),
            manifest_path.display().to_string(),
        )
    })?;
    let bytes = std::fs::read(&manifest_path).map_err(|error| {
        engine_error(
            format!("Cannot read engine manifest: {}", manifest_path.display()),
            error.to_string(),
        )
    })?;
    let manifest = serde_json::from_slice::<EngineManifest>(&bytes).map_err(|error| {
        engine_error(
            "Engine manifest is not valid JSON".to_owned(),
            error.to_string(),
        )
    })?;
    manifest
        .validate(ENGINE_PROTOCOL_VERSION)
        .map_err(|error| {
            engine_error("Engine manifest is invalid".to_owned(), error.to_string())
        })?;
    ensure_application_compatible(&manifest)?;
    verify_host_target(&manifest)?;

    let mut executables = BTreeMap::new();
    for executable in &manifest.executables {
        let path = resolve_pack_file(root, &executable.relative_path, "engine executable")?;
        verify_native_executable(&path)?;
        let observed = sha256_file(&path)?;
        if !observed.eq_ignore_ascii_case(&executable.sha256) {
            return Err(engine_error(
                format!("Engine executable hash mismatch: {}", executable.name),
                format!("expected {}, observed {observed}", executable.sha256),
            ));
        }
        executables.insert(executable.name.clone(), path);
    }
    let mut runtime_files = Vec::with_capacity(manifest.runtime_files.len());
    for runtime_file in &manifest.runtime_files {
        let path = resolve_pack_file(root, &runtime_file.relative_path, "engine runtime file")?;
        let observed = sha256_file(&path)?;
        if !observed.eq_ignore_ascii_case(&runtime_file.sha256) {
            return Err(engine_error(
                format!(
                    "Engine runtime file hash mismatch: {}",
                    runtime_file.relative_path.display()
                ),
                format!("expected {}, observed {observed}", runtime_file.sha256),
            ));
        }
        runtime_files.push(path);
    }
    for license in &manifest.licenses {
        verify_license_files(root, license)?;
    }
    let (supply_chain_files, review_status) = match &manifest.supply_chain {
        Some(supply_chain) => {
            let files = verify_supply_chain_files(root, &manifest, supply_chain)?;
            let sources = read_json_object(&files[1], "Engine source inventory")?;
            (
                files,
                SupplyChainReviewStatus::parse_recorded(
                    sources
                        .get("review_status")
                        .and_then(serde_json::Value::as_str),
                ),
            )
        }
        None => (Vec::new(), SupplyChainReviewStatus::Missing),
    };

    let manifest_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let signature_present = manifest.signature.is_some();
    Ok(VerifiedEnginePack {
        manifest,
        manifest_path,
        manifest_sha256,
        executables,
        runtime_files,
        supply_chain_files,
        signature_present,
        signature_trust: None,
        review_status,
    })
}

/// Verifies an engine pack and evaluates its signature against a release
/// keyring (ADR-0011). The keyring must itself be valid; an invalid keyring
/// fails closed instead of degrading to unsigned verification.
///
/// # Errors
///
/// Returns the same errors as [`verify_engine_pack`], plus a keyring error
/// when the supplied keyring is malformed.
pub fn verify_engine_pack_with_keyring(
    manifest_path: impl AsRef<Path>,
    keyring: &ReleaseKeyring,
    now_unix_ms: u64,
) -> Result<VerifiedEnginePack> {
    keyring.validate().map_err(|error| {
        engine_error("Release keyring is invalid".to_owned(), error.to_string())
    })?;
    let mut verified = verify_engine_pack(manifest_path)?;
    verified.signature_trust = Some(formatwright_engine_sdk::verify_manifest_signature(
        &verified.manifest,
        keyring,
        now_unix_ms,
    ));
    Ok(verified)
}

/// Loads and validates a release keyring document (ADR-0011).
///
/// # Errors
///
/// Returns an engine/storage error when the file is unreadable, malformed,
/// or violates keyring invariants.
pub fn load_release_keyring(path: impl AsRef<Path>) -> Result<ReleaseKeyring> {
    let path = path.as_ref();
    let bytes = fs::read(path).map_err(|error| {
        engine_error(
            format!("Cannot read release keyring: {}", path.display()),
            error.to_string(),
        )
    })?;
    let keyring = serde_json::from_slice::<ReleaseKeyring>(&bytes).map_err(|error| {
        engine_error(
            format!("Release keyring is not valid JSON: {}", path.display()),
            error.to_string(),
        )
    })?;
    keyring.validate().map_err(|error| {
        engine_error(
            format!("Release keyring is invalid: {}", path.display()),
            error.to_string(),
        )
    })?;
    Ok(keyring)
}

/// Verifies an engine pack and registers its exact executable paths for this
/// process. Certification stays `Unverified` unless the embedded release
/// keyring trusts the signature **and** `sources.json` records a completed
/// human review.
///
/// # Errors
///
/// Returns the same verification errors as [`verify_engine_pack`], or an
/// internal registry error if the verified paths cannot be activated.
pub fn activate_engine_pack(manifest_path: impl AsRef<Path>) -> Result<VerifiedEnginePack> {
    let mut verified = verify_engine_pack(manifest_path)?;
    apply_embedded_signature_trust(&mut verified);
    crate::doctor::register_engine_pack_paths_with_provenance(
        &verified.executables,
        &verified.manifest_sha256,
        &verified.manifest.engine_id,
        verified.certification(),
        verified.signature_trust.as_ref(),
        verified.review_status,
    )?;
    Ok(verified)
}

/// Copies only manifest-declared, hash-verified pack files into a versioned
/// local store and activates the installed copy.
///
/// Installation is staged beside the final directory and published by rename,
/// so a partial copy never becomes the active pack. Existing identical packs
/// are re-verified and reused.
///
/// # Errors
///
/// Returns an engine/storage error when source verification, copying, staged
/// verification, atomic publication, or activation fails.
pub fn install_engine_pack(
    manifest_path: impl AsRef<Path>,
    store_root: impl AsRef<Path>,
) -> Result<VerifiedEnginePack> {
    let source = verify_engine_pack(manifest_path)?;
    let store_root = store_root.as_ref();
    fs::create_dir_all(store_root).map_err(|error| {
        engine_error(
            format!("Cannot create engine store: {}", store_root.display()),
            error.to_string(),
        )
    })?;
    let version_root = store_root
        .join(&source.manifest.engine_id)
        .join(&source.manifest.version);
    fs::create_dir_all(&version_root).map_err(|error| {
        engine_error(
            format!(
                "Cannot create engine version directory: {}",
                version_root.display()
            ),
            error.to_string(),
        )
    })?;
    let destination = version_root.join(&source.manifest_sha256);
    let destination_manifest = destination.join("manifest.json");
    if destination_manifest.is_file() {
        return activate_engine_pack(destination_manifest);
    }

    let staging = version_root.join(format!(
        ".{}.{}.partial",
        source.manifest_sha256,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir(&staging).map_err(|error| {
        engine_error(
            format!(
                "Cannot create engine staging directory: {}",
                staging.display()
            ),
            error.to_string(),
        )
    })?;
    let result = stage_verified_pack(&source, &staging)
        .and_then(|()| verify_engine_pack(staging.join("manifest.json")))
        .and_then(|_| {
            if let Err(error) = fs::rename(&staging, &destination) {
                if destination_manifest.is_file() {
                    let _ = fs::remove_dir_all(&staging);
                    return activate_engine_pack(&destination_manifest);
                }
                return Err(engine_error(
                    format!("Cannot publish engine pack: {}", destination.display()),
                    error.to_string(),
                ));
            }
            activate_engine_pack(&destination_manifest)
        });
    if result.is_err() && staging.is_dir() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn stage_verified_pack(source: &VerifiedEnginePack, staging: &Path) -> Result<()> {
    let source_root = source.manifest_path.parent().ok_or_else(|| {
        engine_error(
            "Engine manifest has no pack directory".to_owned(),
            source.manifest_path.display().to_string(),
        )
    })?;
    copy_pack_file(
        &source.manifest_path,
        &staging.join("manifest.json"),
        "engine manifest",
    )?;
    for executable in &source.manifest.executables {
        copy_pack_relative(
            source_root,
            staging,
            &executable.relative_path,
            "engine executable",
        )?;
    }
    for runtime_file in &source.manifest.runtime_files {
        copy_pack_relative(
            source_root,
            staging,
            &runtime_file.relative_path,
            "engine runtime file",
        )?;
    }
    for license in &source.manifest.licenses {
        copy_pack_relative(source_root, staging, &license.notice_path, "license notice")?;
        if let Some(path) = &license.source_offer_path {
            copy_pack_relative(source_root, staging, path, "source offer")?;
        }
    }
    if let Some(supply_chain) = &source.manifest.supply_chain {
        copy_pack_relative(source_root, staging, &supply_chain.sbom_path, "engine SBOM")?;
        copy_pack_relative(
            source_root,
            staging,
            &supply_chain.sources_path,
            "engine source inventory",
        )?;
    }
    Ok(())
}

fn copy_pack_relative(
    source_root: &Path,
    staging: &Path,
    relative: &Path,
    purpose: &str,
) -> Result<()> {
    let source = resolve_pack_file(source_root, relative, purpose)?;
    copy_pack_file(&source, &staging.join(relative), purpose)
}

fn copy_pack_file(source: &Path, destination: &Path, purpose: &str) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            engine_error(
                format!("Cannot create {purpose} directory: {}", parent.display()),
                error.to_string(),
            )
        })?;
    }
    fs::copy(source, destination).map_err(|error| {
        engine_error(
            format!("Cannot copy {purpose}: {}", source.display()),
            error.to_string(),
        )
    })?;
    Ok(())
}

fn verify_host_target(manifest: &EngineManifest) -> Result<()> {
    let platform = EnginePlatform::current().ok_or_else(|| {
        engine_error(
            "This operating system has no engine-pack target".to_owned(),
            std::env::consts::OS.to_owned(),
        )
    })?;
    let architecture = EngineArchitecture::current().ok_or_else(|| {
        engine_error(
            "This architecture has no engine-pack target".to_owned(),
            std::env::consts::ARCH.to_owned(),
        )
    })?;
    if manifest.platform != platform || manifest.architecture != architecture {
        return Err(engine_error(
            "Engine pack does not match this host".to_owned(),
            format!(
                "pack={:?}/{:?}, host={platform:?}/{architecture:?}",
                manifest.platform, manifest.architecture
            ),
        ));
    }
    Ok(())
}

fn verify_license_files(root: &Path, license: &ManifestLicense) -> Result<()> {
    resolve_pack_file(root, &license.notice_path, "license notice")?;
    if let Some(path) = &license.source_offer_path {
        resolve_pack_file(root, path, "source offer")?;
    }
    Ok(())
}

fn verify_supply_chain_files(
    root: &Path,
    manifest: &EngineManifest,
    supply_chain: &ManifestSupplyChain,
) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::with_capacity(2);
    for (purpose, relative, expected) in [
        (
            "engine SBOM",
            &supply_chain.sbom_path,
            &supply_chain.sbom_sha256,
        ),
        (
            "engine source inventory",
            &supply_chain.sources_path,
            &supply_chain.sources_sha256,
        ),
    ] {
        let path = resolve_pack_file(root, relative, purpose)?;
        let observed = sha256_file(&path)?;
        if !observed.eq_ignore_ascii_case(expected) {
            return Err(engine_error(
                format!("{purpose} hash mismatch"),
                format!("expected {expected}, observed {observed}"),
            ));
        }
        paths.push(path);
    }

    let sbom = read_json_object(&paths[0], "Engine SBOM")?;
    if sbom.get("spdxVersion").and_then(serde_json::Value::as_str) != Some("SPDX-2.3")
        || sbom.get("dataLicense").and_then(serde_json::Value::as_str) != Some("CC0-1.0")
        || !sbom
            .get("packages")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|packages| {
                packages.iter().any(|package| {
                    package.get("name").and_then(serde_json::Value::as_str)
                        == Some(manifest.engine_id.as_str())
                        && package
                            .get("versionInfo")
                            .and_then(serde_json::Value::as_str)
                            == Some(manifest.version.as_str())
                })
            })
    {
        return Err(engine_error(
            "Engine SBOM identity is invalid".to_owned(),
            paths[0].display().to_string(),
        ));
    }
    verify_sbom_inventory(root, manifest, supply_chain, &sbom)?;

    let sources = read_json_object(&paths[1], "Engine source inventory")?;
    if sources
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
        || sources.get("engine_id").and_then(serde_json::Value::as_str)
            != Some(manifest.engine_id.as_str())
        || sources.get("version").and_then(serde_json::Value::as_str)
            != Some(manifest.version.as_str())
        || sources
            .get("artifacts")
            .and_then(serde_json::Value::as_array)
            .is_none_or(Vec::is_empty)
    {
        return Err(engine_error(
            "Engine source inventory identity is invalid".to_owned(),
            paths[1].display().to_string(),
        ));
    }
    Ok(paths)
}

fn verify_sbom_inventory(
    root: &Path,
    manifest: &EngineManifest,
    supply_chain: &ManifestSupplyChain,
    sbom: &serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    let mut expected = BTreeMap::new();
    for executable in &manifest.executables {
        expected.insert(
            portable_pack_path(&executable.relative_path),
            executable.sha256.to_ascii_lowercase(),
        );
    }
    for runtime_file in &manifest.runtime_files {
        expected.insert(
            portable_pack_path(&runtime_file.relative_path),
            runtime_file.sha256.to_ascii_lowercase(),
        );
    }
    for license in &manifest.licenses {
        for (purpose, relative) in [
            ("license notice", Some(&license.notice_path)),
            ("source offer", license.source_offer_path.as_ref()),
        ] {
            let Some(relative) = relative else {
                continue;
            };
            let path = resolve_pack_file(root, relative, purpose)?;
            expected.insert(portable_pack_path(relative), sha256_file(&path)?);
        }
    }
    expected.insert(
        portable_pack_path(&supply_chain.sources_path),
        supply_chain.sources_sha256.to_ascii_lowercase(),
    );

    let files = sbom
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            engine_error(
                "Engine SBOM has no file inventory".to_owned(),
                supply_chain.sbom_path.display().to_string(),
            )
        })?;
    let mut observed = BTreeMap::new();
    for entry in files {
        let entry = entry.as_object().ok_or_else(|| {
            engine_error(
                "Engine SBOM file entry is invalid".to_owned(),
                supply_chain.sbom_path.display().to_string(),
            )
        })?;
        let relative = entry
            .get("fileName")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                engine_error(
                    "Engine SBOM file entry has no name".to_owned(),
                    supply_chain.sbom_path.display().to_string(),
                )
            })?;
        let sha256 = entry
            .get("checksums")
            .and_then(serde_json::Value::as_array)
            .and_then(|checksums| {
                checksums.iter().find_map(|checksum| {
                    let checksum = checksum.as_object()?;
                    (checksum.get("algorithm")?.as_str()? == "SHA256")
                        .then(|| checksum.get("checksumValue")?.as_str())
                        .flatten()
                })
            })
            .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| {
                engine_error(
                    format!("Engine SBOM file has no valid SHA-256: {relative}"),
                    supply_chain.sbom_path.display().to_string(),
                )
            })?;
        if observed
            .insert(relative.to_owned(), sha256.to_ascii_lowercase())
            .is_some()
        {
            return Err(engine_error(
                format!("Engine SBOM repeats a file: {relative}"),
                supply_chain.sbom_path.display().to_string(),
            ));
        }
    }
    if observed != expected {
        let missing = expected
            .keys()
            .filter(|path| !observed.contains_key(*path))
            .cloned()
            .collect::<Vec<_>>();
        let unexpected = observed
            .keys()
            .filter(|path| !expected.contains_key(*path))
            .cloned()
            .collect::<Vec<_>>();
        return Err(engine_error(
            "Engine SBOM file inventory differs from the manifest-declared payload".to_owned(),
            format!("missing={missing:?}, unexpected={unexpected:?}"),
        ));
    }
    Ok(())
}

fn portable_pack_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn read_json_object(
    path: &Path,
    purpose: &str,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let bytes = fs::read(path).map_err(|error| {
        engine_error(
            format!("Cannot read {purpose}: {}", path.display()),
            error.to_string(),
        )
    })?;
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|error| engine_error(format!("{purpose} is not valid JSON"), error.to_string()))?
        .as_object()
        .cloned()
        .ok_or_else(|| {
            engine_error(
                format!("{purpose} must be a JSON object"),
                path.display().to_string(),
            )
        })
}

fn ensure_application_compatible(manifest: &EngineManifest) -> Result<()> {
    if manifest
        .formatwright_compatibility
        .contains(env!("CARGO_PKG_VERSION"))
    {
        return Ok(());
    }
    Err(engine_error(
        format!(
            "Engine pack requires Anole {}..{}, this application is {}",
            manifest.formatwright_compatibility.minimum,
            manifest.formatwright_compatibility.maximum_exclusive,
            env!("CARGO_PKG_VERSION")
        ),
        "Import a pack built for this Anole version.".to_owned(),
    ))
}

// On non-Windows targets the guard body compiles away, leaving the path
// parameter and the tail Ok(()) unused - both are Windows-only by design.
#[cfg_attr(not(windows), allow(unused_variables, clippy::unnecessary_wraps))]
fn verify_native_executable(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        let extension = path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(extension.as_str(), "exe" | "com") {
            return Err(engine_error(
                "Windows engine packs may contain only native .exe/.com executables".to_owned(),
                path.display().to_string(),
            ));
        }
    }
    Ok(())
}

fn resolve_pack_file(root: &Path, relative: &Path, purpose: &str) -> Result<PathBuf> {
    let candidate = root.join(relative);
    let canonical = candidate.canonicalize().map_err(|error| {
        engine_error(
            format!("Missing {purpose}: {}", candidate.display()),
            error.to_string(),
        )
    })?;
    if !canonical.starts_with(root) {
        return Err(engine_error(
            format!("{purpose} escapes the engine pack"),
            canonical.display().to_string(),
        ));
    }
    if !canonical.is_file() {
        return Err(engine_error(
            format!("{purpose} is not a regular file"),
            canonical.display().to_string(),
        ));
    }
    Ok(canonical)
}

fn engine_error(message: String, diagnostic: String) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::EngineIncompatible,
        Stage::Doctor,
        message,
        "Import a compatible, intact engine pack from a trusted source.",
    )
    .with_diagnostic(diagnostic)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    use formatwright_engine_sdk::{
        Capability, EngineArchitecture, EngineManifest, EnginePlatform, FormatWrightCompatibility,
        LossClass, ManifestExecutable, ManifestLicense, ManifestRuntimeFile, ManifestSource,
        ManifestSupplyChain, Operation,
    };
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::{
        ENGINE_PROTOCOL_VERSION, activate_engine_pack, embedded_release_keyring,
        install_engine_pack, load_release_keyring, read_json_object, verify_engine_pack,
        verify_engine_pack_with_keyring,
    };

    fn manifest(binary_hash: String) -> EngineManifest {
        EngineManifest {
            schema_version: 1,
            engine_id: "fixture-engine".to_owned(),
            version: "1.0.0".to_owned(),
            platform: EnginePlatform::current().expect("supported test platform"),
            architecture: EngineArchitecture::current().expect("supported test architecture"),
            protocol_version: ENGINE_PROTOCOL_VERSION,
            formatwright_compatibility: FormatWrightCompatibility {
                minimum: "0.1.0".to_owned(),
                maximum_exclusive: "0.2.0".to_owned(),
            },
            executables: vec![ManifestExecutable {
                name: "fixture".to_owned(),
                relative_path: PathBuf::from(if cfg!(windows) {
                    "bin/fixture.exe"
                } else {
                    "bin/fixture.bin"
                }),
                sha256: binary_hash,
            }],
            runtime_files: vec![ManifestRuntimeFile {
                relative_path: PathBuf::from("runtime/fixture.dat"),
                sha256: format!("{:x}", Sha256::digest(b"fixture runtime")),
            }],
            source: ManifestSource {
                project_url: "https://example.invalid/fixture".to_owned(),
                source_url: "https://example.invalid/fixture/source".to_owned(),
                source_revision: "v1.0.0".to_owned(),
                build_configuration: "test-only".to_owned(),
            },
            licenses: vec![ManifestLicense {
                spdx: "Apache-2.0".to_owned(),
                notice_path: PathBuf::from("licenses/NOTICE.txt"),
                source_offer_path: None,
            }],
            supply_chain: None,
            capabilities: vec![Capability {
                capability_id: "fixture.copy".to_owned(),
                inputs: vec!["bin".to_owned()],
                outputs: vec!["bin".to_owned()],
                operation: Operation::Transform,
                loss_class: LossClass::None,
                constraints: BTreeMap::new(),
            }],
            signature: None,
        }
    }

    fn create_pack() -> (tempfile::TempDir, PathBuf) {
        let directory = tempdir().expect("temporary pack");
        fs::create_dir_all(directory.path().join("bin")).expect("binary directory");
        fs::create_dir_all(directory.path().join("licenses")).expect("license directory");
        fs::create_dir_all(directory.path().join("runtime")).expect("runtime directory");
        let binary = directory.path().join(if cfg!(windows) {
            "bin/fixture.exe"
        } else {
            "bin/fixture.bin"
        });
        fs::write(&binary, b"fixture engine").expect("binary fixture");
        fs::write(
            directory.path().join("licenses/NOTICE.txt"),
            b"fixture notice",
        )
        .expect("notice fixture");
        fs::write(
            directory.path().join("runtime/fixture.dat"),
            b"fixture runtime",
        )
        .expect("runtime fixture");
        let hash = format!("{:x}", Sha256::digest(b"fixture engine"));
        let manifest_path = directory.path().join("manifest.json");
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest(hash)).expect("serialize manifest"),
        )
        .expect("manifest fixture");
        (directory, manifest_path)
    }

    fn sbom_file(root: &std::path::Path, relative: &str) -> serde_json::Value {
        let bytes = fs::read(root.join(relative)).expect("read SBOM fixture file");
        serde_json::json!({
            "fileName": relative,
            "checksums": [{
                "algorithm": "SHA256",
                "checksumValue": format!("{:x}", Sha256::digest(bytes))
            }]
        })
    }

    #[test]
    fn verifies_target_hashes_and_license_files() {
        let (_directory, manifest_path) = create_pack();
        let verified = verify_engine_pack(&manifest_path).expect("valid pack");
        assert_eq!(verified.manifest.engine_id, "fixture-engine");
        assert_eq!(verified.executables.len(), 1);
        assert_eq!(verified.manifest_sha256.len(), 64);
        assert!(!verified.signature_present);
    }

    #[test]
    fn activates_verified_paths_for_runtime_discovery() {
        let (_directory, manifest_path) = create_pack();
        let verified = activate_engine_pack(&manifest_path).expect("activate valid pack");
        let (path, manifest_sha256) =
            crate::doctor::registered_engine_metadata("fixture").expect("registered fixture path");
        assert_eq!(path, verified.executables["fixture"]);
        assert_eq!(manifest_sha256, verified.manifest_sha256);
    }

    #[test]
    fn rejects_a_tampered_binary() {
        let (_directory, manifest_path) = create_pack();
        let bytes = fs::read(&manifest_path).expect("read manifest");
        let value = serde_json::from_slice::<EngineManifest>(&bytes).expect("parse manifest");
        fs::write(
            manifest_path
                .parent()
                .expect("pack root")
                .join(&value.executables[0].relative_path),
            b"tampered",
        )
        .expect("tamper binary");
        let error = verify_engine_pack(&manifest_path).expect_err("tamper must fail");
        assert!(error.message.contains("hash mismatch"));
    }

    #[test]
    fn rejects_a_tampered_runtime_file() {
        let (_directory, manifest_path) = create_pack();
        let bytes = fs::read(&manifest_path).expect("read manifest");
        let value = serde_json::from_slice::<EngineManifest>(&bytes).expect("parse manifest");
        fs::write(
            manifest_path
                .parent()
                .expect("pack root")
                .join(&value.runtime_files[0].relative_path),
            b"tampered runtime",
        )
        .expect("tamper runtime file");
        let error = verify_engine_pack(&manifest_path).expect_err("runtime tamper must fail");
        assert!(error.message.contains("runtime file hash mismatch"));
    }

    #[test]
    fn verifies_and_installs_supply_chain_sidecars() {
        let (source_directory, manifest_path) = create_pack();
        let root = source_directory.path();
        let sbom_path = root.join("sbom.spdx.json");
        let sources_path = root.join("sources.json");
        fs::write(
            &sources_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 1,
                "engine_id": "fixture-engine",
                "version": "1.0.0",
                "review_status": "incomplete",
                "artifacts": [{"name": "fixture", "source_url": "https://example.invalid/source"}]
            }))
            .expect("serialize sources"),
        )
        .expect("write sources");
        fs::write(
            &sbom_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "spdxVersion": "SPDX-2.3",
                "dataLicense": "CC0-1.0",
                "SPDXID": "SPDXRef-DOCUMENT",
                "name": "fixture-engine-SBOM",
                "packages": [{"name": "fixture-engine", "versionInfo": "1.0.0"}],
                "files": [
                    sbom_file(root, if cfg!(windows) { "bin/fixture.exe" } else { "bin/fixture.bin" }),
                    sbom_file(root, "licenses/NOTICE.txt"),
                    sbom_file(root, "runtime/fixture.dat"),
                    sbom_file(root, "sources.json")
                ]
            }))
            .expect("serialize SBOM"),
        )
        .expect("write SBOM");
        let bytes = fs::read(&manifest_path).expect("read manifest");
        let mut value = serde_json::from_slice::<EngineManifest>(&bytes).expect("parse manifest");
        value.supply_chain = Some(ManifestSupplyChain {
            sbom_path: PathBuf::from("sbom.spdx.json"),
            sbom_sha256: format!("{:x}", Sha256::digest(fs::read(&sbom_path).expect("SBOM"))),
            sources_path: PathBuf::from("sources.json"),
            sources_sha256: format!(
                "{:x}",
                Sha256::digest(fs::read(&sources_path).expect("sources"))
            ),
        });
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&value).expect("serialize manifest"),
        )
        .expect("rewrite manifest");

        let verified = verify_engine_pack(&manifest_path).expect("supply chain pack");
        assert_eq!(verified.supply_chain_files.len(), 2);
        assert_eq!(
            verified.review_status,
            formatwright_engine_sdk::SupplyChainReviewStatus::Incomplete
        );
        assert_eq!(
            verified.certification(),
            formatwright_engine_sdk::Certification::Unverified
        );
        let store = tempdir().expect("temporary engine store");
        let installed = install_engine_pack(&manifest_path, store.path()).expect("install pack");
        assert_eq!(installed.supply_chain_files.len(), 2);
        assert!(
            installed
                .supply_chain_files
                .iter()
                .all(|path| path.is_file())
        );

        let mut incomplete = read_json_object(&sbom_path, "test SBOM").expect("read SBOM");
        incomplete
            .get_mut("files")
            .and_then(serde_json::Value::as_array_mut)
            .expect("SBOM files")
            .pop();
        fs::write(
            &sbom_path,
            serde_json::to_vec_pretty(&incomplete).expect("serialize incomplete SBOM"),
        )
        .expect("write incomplete SBOM");
        let mut value = serde_json::from_slice::<EngineManifest>(
            &fs::read(&manifest_path).expect("read manifest"),
        )
        .expect("parse manifest");
        value
            .supply_chain
            .as_mut()
            .expect("supply chain")
            .sbom_sha256 = format!(
            "{:x}",
            Sha256::digest(fs::read(&sbom_path).expect("incomplete SBOM"))
        );
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&value).expect("serialize manifest"),
        )
        .expect("rewrite manifest");
        let error = verify_engine_pack(&manifest_path).expect_err("incomplete SBOM inventory");
        assert!(error.message.contains("inventory differs"));
    }

    #[test]
    fn rejects_a_tampered_or_wrong_identity_supply_chain_sidecar() {
        let (directory, manifest_path) = create_pack();
        let sbom_path = directory.path().join("sbom.spdx.json");
        let sources_path = directory.path().join("sources.json");
        fs::write(
            &sbom_path,
            br#"{"spdxVersion":"SPDX-2.3","dataLicense":"CC0-1.0","packages":[{"name":"wrong-engine","versionInfo":"1.0.0"}]}"#,
        )
        .expect("write SBOM");
        fs::write(
            &sources_path,
            br#"{"schema_version":1,"engine_id":"fixture-engine","version":"1.0.0","artifacts":[{}]}"#,
        )
        .expect("write sources");
        let bytes = fs::read(&manifest_path).expect("read manifest");
        let mut value = serde_json::from_slice::<EngineManifest>(&bytes).expect("parse manifest");
        value.supply_chain = Some(ManifestSupplyChain {
            sbom_path: PathBuf::from("sbom.spdx.json"),
            sbom_sha256: format!("{:x}", Sha256::digest(fs::read(&sbom_path).expect("SBOM"))),
            sources_path: PathBuf::from("sources.json"),
            sources_sha256: format!(
                "{:x}",
                Sha256::digest(fs::read(&sources_path).expect("sources"))
            ),
        });
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&value).expect("serialize manifest"),
        )
        .expect("rewrite manifest");

        let error = verify_engine_pack(&manifest_path).expect_err("wrong SBOM identity");
        assert!(error.message.contains("SBOM identity"));

        let value = serde_json::from_slice::<EngineManifest>(
            &fs::read(&manifest_path).expect("read manifest"),
        )
        .expect("parse manifest");
        fs::write(&sbom_path, b"tampered").expect("tamper SBOM");
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&value).expect("serialize manifest"),
        )
        .expect("rewrite manifest");
        let error = verify_engine_pack(&manifest_path).expect_err("tampered SBOM hash");
        assert!(error.message.contains("SBOM hash mismatch"));
    }

    #[test]
    fn evaluates_signature_trust_against_a_release_keyring() {
        use formatwright_engine_sdk::{
            KeyRevocation, ReleaseKey, ReleaseKeyring, ed25519_public_key_hex, sign_manifest,
        };
        const SEED: [u8; 32] = [7; 32];
        const NOW: u64 = 1_800_000_000_000;
        let keyring = ReleaseKeyring {
            schema_version: 1,
            keys: vec![ReleaseKey {
                key_id: "release-2026h2".to_owned(),
                algorithm: "ed25519".to_owned(),
                purpose: "engine-manifest".to_owned(),
                public_key: ed25519_public_key_hex(&SEED),
                valid_from_unix_ms: NOW - 1_000,
                valid_until_unix_ms: NOW + 1_000,
            }],
            revocations: Vec::new(),
        };

        let (directory, manifest_path) = create_pack();
        let unsigned = verify_engine_pack(&manifest_path).expect("unsigned pack verifies");
        assert!(!unsigned.signature_present);
        assert!(unsigned.signature_trust.is_none());

        let verified = verify_engine_pack_with_keyring(&manifest_path, &keyring, NOW)
            .expect("keyring evaluation runs");
        assert_eq!(
            verified.signature_trust,
            Some(formatwright_engine_sdk::SignatureTrust::Unsigned)
        );

        let bytes = fs::read(&manifest_path).expect("read manifest");
        let mut value = serde_json::from_slice::<EngineManifest>(&bytes).expect("parse manifest");
        value.signature = Some(sign_manifest(&value, "release-2026h2", &SEED));
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&value).expect("serialize signed manifest"),
        )
        .expect("write signed manifest");

        let trusted = verify_engine_pack_with_keyring(&manifest_path, &keyring, NOW)
            .expect("signed pack verifies");
        assert_eq!(
            trusted.signature_trust,
            Some(formatwright_engine_sdk::SignatureTrust::Trusted {
                key_id: "release-2026h2".to_owned()
            })
        );
        assert_ne!(trusted.manifest_sha256, verified.manifest_sha256);

        let mut tampered = value.clone();
        tampered.version = "9.9.9".to_owned();
        tampered.signature = value.signature.clone();
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&tampered).expect("serialize tampered manifest"),
        )
        .expect("write tampered manifest");
        let tampered = verify_engine_pack_with_keyring(&manifest_path, &keyring, NOW)
            .expect("file hashes still verify");
        assert_eq!(
            tampered.signature_trust,
            Some(formatwright_engine_sdk::SignatureTrust::InvalidSignature)
        );

        let mut revoked = keyring.clone();
        revoked.revocations.push(KeyRevocation {
            key_id: "release-2026h2".to_owned(),
            revoked_unix_ms: 5,
            reason: "compromised in test".to_owned(),
        });
        let (_fresh_directory, fresh_manifest) = create_pack();
        let bytes = fs::read(&fresh_manifest).expect("read fresh manifest");
        let mut fresh =
            serde_json::from_slice::<EngineManifest>(&bytes).expect("parse fresh manifest");
        fresh.signature = Some(sign_manifest(&fresh, "release-2026h2", &SEED));
        fs::write(
            &fresh_manifest,
            serde_json::to_vec_pretty(&fresh).expect("serialize fresh manifest"),
        )
        .expect("write fresh manifest");
        let revoked = verify_engine_pack_with_keyring(&fresh_manifest, &revoked, NOW)
            .expect("revoked evaluation runs");
        assert_eq!(
            revoked.signature_trust,
            Some(formatwright_engine_sdk::SignatureTrust::Revoked {
                key_id: "release-2026h2".to_owned()
            })
        );

        let keyring_path = directory.path().join("keyring.json");
        fs::write(
            &keyring_path,
            serde_json::to_vec_pretty(&keyring).expect("serialize keyring"),
        )
        .expect("write keyring");
        let loaded = load_release_keyring(&keyring_path).expect("load keyring");
        assert_eq!(loaded, keyring);
        fs::write(&keyring_path, b"{\"schema_version\": 9}").expect("write bad keyring");
        assert!(load_release_keyring(&keyring_path).is_err());
    }

    #[test]
    fn activate_applies_embedded_keyring_without_promoting_unsigned_packs() {
        let (_directory, manifest_path) = create_pack();
        let activated = activate_engine_pack(&manifest_path).expect("activate unsigned pack");
        assert_eq!(
            activated.signature_trust,
            Some(formatwright_engine_sdk::SignatureTrust::Unsigned)
        );
        assert_eq!(
            activated.review_status,
            formatwright_engine_sdk::SupplyChainReviewStatus::Missing
        );
        assert_eq!(
            activated.certification(),
            formatwright_engine_sdk::Certification::Unverified
        );
        let (_, trust, review) =
            crate::doctor::registered_engine_provenance("fixture").expect("registered provenance");
        assert_eq!(
            trust,
            Some(formatwright_engine_sdk::SignatureTrust::Unsigned)
        );
        assert_eq!(
            review,
            formatwright_engine_sdk::SupplyChainReviewStatus::Missing
        );
        assert!(embedded_release_keyring().keys.is_empty());
    }

    fn write_signed_supply_chain_pack(review_status: &str) -> (tempfile::TempDir, PathBuf) {
        use formatwright_engine_sdk::sign_manifest;
        const SEED: [u8; 32] = [7; 32];
        let (directory, manifest_path) = create_pack();
        let root = directory.path();
        let sbom_path = root.join("sbom.spdx.json");
        let sources_path = root.join("sources.json");
        fs::write(
            &sources_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 1,
                "engine_id": "fixture-engine",
                "version": "1.0.0",
                "review_status": review_status,
                "artifacts": [{"name": "fixture", "source_url": "https://example.invalid/source"}]
            }))
            .expect("serialize sources"),
        )
        .expect("write sources");
        fs::write(
            &sbom_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "spdxVersion": "SPDX-2.3",
                "dataLicense": "CC0-1.0",
                "SPDXID": "SPDXRef-DOCUMENT",
                "name": "fixture-engine-SBOM",
                "packages": [{"name": "fixture-engine", "versionInfo": "1.0.0"}],
                "files": [
                    sbom_file(root, if cfg!(windows) { "bin/fixture.exe" } else { "bin/fixture.bin" }),
                    sbom_file(root, "licenses/NOTICE.txt"),
                    sbom_file(root, "runtime/fixture.dat"),
                    sbom_file(root, "sources.json")
                ]
            }))
            .expect("serialize SBOM"),
        )
        .expect("write SBOM");
        let bytes = fs::read(&manifest_path).expect("read manifest");
        let mut value = serde_json::from_slice::<EngineManifest>(&bytes).expect("parse manifest");
        value.supply_chain = Some(ManifestSupplyChain {
            sbom_path: PathBuf::from("sbom.spdx.json"),
            sbom_sha256: format!("{:x}", Sha256::digest(fs::read(&sbom_path).expect("SBOM"))),
            sources_path: PathBuf::from("sources.json"),
            sources_sha256: format!(
                "{:x}",
                Sha256::digest(fs::read(&sources_path).expect("sources"))
            ),
        });
        value.signature = Some(sign_manifest(&value, "release-2026h2", &SEED));
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&value).expect("serialize signed manifest"),
        )
        .expect("write signed manifest");
        (directory, manifest_path)
    }

    fn test_release_keyring() -> formatwright_engine_sdk::ReleaseKeyring {
        use formatwright_engine_sdk::{ReleaseKey, ReleaseKeyring, ed25519_public_key_hex};
        const SEED: [u8; 32] = [7; 32];
        const NOW: u64 = 1_800_000_000_000;
        ReleaseKeyring {
            schema_version: 1,
            keys: vec![ReleaseKey {
                key_id: "release-2026h2".to_owned(),
                algorithm: "ed25519".to_owned(),
                purpose: "engine-manifest".to_owned(),
                public_key: ed25519_public_key_hex(&SEED),
                valid_from_unix_ms: NOW - 1_000,
                valid_until_unix_ms: NOW + 1_000,
            }],
            revocations: Vec::new(),
        }
    }

    #[test]
    fn trusted_signature_promotes_only_after_complete_review() {
        use formatwright_engine_sdk::SupplyChainReviewStatus;
        const NOW: u64 = 1_800_000_000_000;
        let keyring = test_release_keyring();
        let (_incomplete_dir, incomplete_manifest) = write_signed_supply_chain_pack("incomplete");
        let trusted_incomplete =
            verify_engine_pack_with_keyring(&incomplete_manifest, &keyring, NOW)
                .expect("trusted pack");
        assert_eq!(
            trusted_incomplete.review_status,
            SupplyChainReviewStatus::Incomplete
        );
        assert_eq!(
            trusted_incomplete.certification(),
            formatwright_engine_sdk::Certification::Unverified
        );
        assert!(
            trusted_incomplete
                .provenance_message()
                .contains("review is incomplete")
        );

        let (_complete_dir, complete_manifest) = write_signed_supply_chain_pack("complete");
        let trusted_complete = verify_engine_pack_with_keyring(&complete_manifest, &keyring, NOW)
            .expect("certified pack");
        assert_eq!(
            trusted_complete.review_status,
            SupplyChainReviewStatus::Complete
        );
        assert_eq!(
            trusted_complete.certification(),
            formatwright_engine_sdk::Certification::Certified
        );
    }

    #[test]
    fn rejects_an_incompatible_application_version() {
        let (_directory, manifest_path) = create_pack();
        let bytes = fs::read(&manifest_path).expect("read manifest");
        let mut value = serde_json::from_slice::<EngineManifest>(&bytes).expect("parse manifest");
        value.formatwright_compatibility.minimum = "9.0.0".to_owned();
        value.formatwright_compatibility.maximum_exclusive = "10.0.0".to_owned();
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&value).expect("serialize manifest"),
        )
        .expect("rewrite manifest");
        let error = verify_engine_pack(&manifest_path).expect_err("future pack must fail");
        assert!(
            error
                .message
                .contains("requires Anole 9.0.0..10.0.0")
        );
        assert!(error.message.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn rejects_a_protocol_mismatch_before_activation() {
        let (_directory, manifest_path) = create_pack();
        let bytes = fs::read(&manifest_path).expect("read manifest");
        let mut value = serde_json::from_slice::<EngineManifest>(&bytes).expect("parse manifest");
        value.protocol_version = ENGINE_PROTOCOL_VERSION + 1;
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&value).expect("serialize manifest"),
        )
        .expect("rewrite manifest");
        let error = verify_engine_pack(&manifest_path).expect_err("protocol mismatch must fail");
        assert!(error.message.contains("Engine manifest is invalid"));
        assert!(
            error
                .diagnostic
                .is_some_and(|text| text.contains("protocol"))
        );
    }

    #[test]
    fn leftover_partial_staging_is_not_published() {
        let (_source, manifest_path) = create_pack();
        let store = tempdir().expect("store");
        let installed = install_engine_pack(&manifest_path, store.path()).expect("install");
        let version_root = store
            .path()
            .join("fixture-engine")
            .join(&installed.manifest.version);
        let leftover = version_root.join(format!(".{}.stale.partial", installed.manifest_sha256));
        fs::create_dir_all(leftover.join("bin")).expect("leftover bin");
        fs::write(leftover.join("manifest.json"), b"{\"schema_version\":1}").expect("leftover");
        let entries = fs::read_dir(&version_root)
            .expect("version root")
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .count();
        assert_eq!(entries, 2, "the leftover staging directory remains on disk");
        let again =
            install_engine_pack(&manifest_path, store.path()).expect("reuse installed pack");
        assert_eq!(again.manifest_sha256, installed.manifest_sha256);
        assert_eq!(
            again
                .manifest_path
                .parent()
                .and_then(std::path::Path::file_name)
                .map(|name| name.to_string_lossy().into_owned())
                .as_deref(),
            Some(installed.manifest_sha256.as_str())
        );
        assert!(
            leftover.is_dir(),
            "reusing an installed pack must not delete leftover staging"
        );
    }

    #[test]
    fn rejects_a_pack_for_another_architecture() {
        let (directory, manifest_path) = create_pack();
        let bytes = fs::read(&manifest_path).expect("read manifest");
        let mut value = serde_json::from_slice::<EngineManifest>(&bytes).expect("parse manifest");
        value.architecture = match value.architecture {
            EngineArchitecture::X86_64 => EngineArchitecture::Aarch64,
            EngineArchitecture::Aarch64 => EngineArchitecture::X86_64,
        };
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&value).expect("serialize manifest"),
        )
        .expect("rewrite manifest");
        let error = verify_engine_pack(directory.path().join("manifest.json"))
            .expect_err("wrong architecture must fail");
        assert!(error.message.contains("does not match this host"));
    }

    #[test]
    #[cfg(windows)]
    fn rejects_windows_script_wrappers_as_pack_executables() {
        let (directory, manifest_path) = create_pack();
        let bytes = fs::read(&manifest_path).expect("read manifest");
        let mut value = serde_json::from_slice::<EngineManifest>(&bytes).expect("parse manifest");
        let old_path = value.executables[0].relative_path.clone();
        let script_path = PathBuf::from("bin/fixture.cmd");
        fs::rename(
            directory.path().join(&old_path),
            directory.path().join(&script_path),
        )
        .expect("rename executable fixture");
        value.executables[0].relative_path = script_path;
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&value).expect("serialize manifest"),
        )
        .expect("rewrite manifest");

        let error = verify_engine_pack(&manifest_path).expect_err("script wrapper must fail");
        assert!(error.message.contains("native .exe/.com"));
    }

    #[test]
    fn installs_and_activates_a_self_contained_copy() {
        let (source_directory, manifest_path) = create_pack();
        let bytes = fs::read(&manifest_path).expect("read manifest");
        let mut value = serde_json::from_slice::<EngineManifest>(&bytes).expect("parse manifest");
        value.engine_id = "install-fixture-engine".to_owned();
        value.executables[0].name = "install-fixture".to_owned();
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&value).expect("serialize manifest"),
        )
        .expect("rewrite manifest");
        let store = tempdir().expect("temporary engine store");

        let installed = install_engine_pack(&manifest_path, store.path()).expect("install pack");
        let canonical_store = store.path().canonicalize().expect("canonical engine store");
        assert!(installed.manifest_path.starts_with(&canonical_store));
        assert!(installed.manifest_path.is_file());
        assert!(installed.executables["install-fixture"].starts_with(&canonical_store));
        assert_eq!(installed.runtime_files.len(), 1);
        assert!(installed.runtime_files[0].starts_with(&canonical_store));
        drop(source_directory);
        assert!(installed.executables["install-fixture"].is_file());
        assert!(installed.runtime_files[0].is_file());
        let (registered, manifest_hash) =
            crate::doctor::registered_engine_metadata("install-fixture")
                .expect("installed engine is active");
        assert_eq!(registered, installed.executables["install-fixture"]);
        assert_eq!(manifest_hash, installed.manifest_sha256);
    }
}
