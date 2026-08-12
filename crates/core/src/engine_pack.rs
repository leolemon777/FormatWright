use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use formatwright_engine_sdk::{
    EngineArchitecture, EngineManifest, EnginePlatform, ManifestLicense,
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
    pub signature_present: bool,
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
    verify_host_target(&manifest)?;

    let mut executables = BTreeMap::new();
    for executable in &manifest.executables {
        let path = resolve_pack_file(root, &executable.relative_path, "engine executable")?;
        let observed = sha256_file(&path)?;
        if !observed.eq_ignore_ascii_case(&executable.sha256) {
            return Err(engine_error(
                format!("Engine executable hash mismatch: {}", executable.name),
                format!("expected {}, observed {observed}", executable.sha256),
            ));
        }
        executables.insert(executable.name.clone(), path);
    }
    for license in &manifest.licenses {
        verify_license_files(root, license)?;
    }

    let manifest_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let signature_present = manifest.signature.is_some();
    Ok(VerifiedEnginePack {
        manifest,
        manifest_path,
        manifest_sha256,
        executables,
        signature_present,
    })
}

/// Verifies an engine pack and registers its exact executable paths for this
/// process. Registered binaries retain `Unverified` certification until a
/// release keyring validates the pack signature.
///
/// # Errors
///
/// Returns the same verification errors as [`verify_engine_pack`], or an
/// internal registry error if the verified paths cannot be activated.
pub fn activate_engine_pack(manifest_path: impl AsRef<Path>) -> Result<VerifiedEnginePack> {
    let verified = verify_engine_pack(manifest_path)?;
    crate::doctor::register_engine_pack_paths(&verified.executables, &verified.manifest_sha256)?;
    Ok(verified)
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
        LossClass, ManifestExecutable, ManifestLicense, ManifestSource, Operation,
    };
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::{ENGINE_PROTOCOL_VERSION, activate_engine_pack, verify_engine_pack};

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
                relative_path: PathBuf::from("bin/fixture.bin"),
                sha256: binary_hash,
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
        let binary = directory.path().join("bin/fixture.bin");
        fs::write(&binary, b"fixture engine").expect("binary fixture");
        fs::write(
            directory.path().join("licenses/NOTICE.txt"),
            b"fixture notice",
        )
        .expect("notice fixture");
        let hash = format!("{:x}", Sha256::digest(b"fixture engine"));
        let manifest_path = directory.path().join("manifest.json");
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest(hash)).expect("serialize manifest"),
        )
        .expect("manifest fixture");
        (directory, manifest_path)
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
        fs::write(
            manifest_path
                .parent()
                .expect("pack root")
                .join("bin/fixture.bin"),
            b"tampered",
        )
        .expect("tamper binary");
        let error = verify_engine_pack(&manifest_path).expect_err("tamper must fail");
        assert!(error.message.contains("hash mismatch"));
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
}
