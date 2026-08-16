#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use ed25519_dalek::Signer;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const ENGINE_MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Certification {
    Certified,
    Experimental,
    Unverified,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LossClass {
    None,
    ContainerOnly,
    Lossless,
    Lossy,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Inspect,
    Remux,
    Transcode,
    Transform,
    Render,
    Serialize,
    MetadataClean,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EngineIdentity {
    pub engine_id: String,
    pub version: String,
    pub binary_path: PathBuf,
    pub binary_sha256: String,
    pub manifest_sha256: Option<String>,
    pub build_configuration: Option<String>,
    pub certification: Certification,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Capability {
    pub capability_id: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub operation: Operation,
    pub loss_class: LossClass,
    #[serde(default)]
    pub constraints: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EnginePlatform {
    Windows,
    Macos,
    Linux,
}

impl EnginePlatform {
    #[must_use]
    pub const fn current() -> Option<Self> {
        if cfg!(target_os = "windows") {
            Some(Self::Windows)
        } else if cfg!(target_os = "macos") {
            Some(Self::Macos)
        } else if cfg!(target_os = "linux") {
            Some(Self::Linux)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineArchitecture {
    X86_64,
    Aarch64,
}

impl EngineArchitecture {
    #[must_use]
    pub const fn current() -> Option<Self> {
        if cfg!(target_arch = "x86_64") {
            Some(Self::X86_64)
        } else if cfg!(target_arch = "aarch64") {
            Some(Self::Aarch64)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FormatWrightCompatibility {
    pub minimum: String,
    pub maximum_exclusive: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestExecutable {
    pub name: String,
    pub relative_path: PathBuf,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestRuntimeFile {
    pub relative_path: PathBuf,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestSource {
    pub project_url: String,
    pub source_url: String,
    pub source_revision: String,
    pub build_configuration: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestLicense {
    pub spdx: String,
    pub notice_path: PathBuf,
    pub source_offer_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestSupplyChain {
    pub sbom_path: PathBuf,
    pub sbom_sha256: String,
    pub sources_path: PathBuf,
    pub sources_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestSignature {
    pub algorithm: String,
    pub key_id: String,
    pub value: String,
}

/// Signature algorithm supported by release keyrings (ADR-0011).
pub const SIGNATURE_ALGORITHM_ED25519: &str = "ed25519";

/// Keyring document schema (ADR-0011).
pub const RELEASE_KEYRING_SCHEMA_VERSION: u32 = 1;

/// The only key purpose accepted for engine-manifest signatures (ADR-0011).
pub const KEYRING_PURPOSE_ENGINE_MANIFEST: &str = "engine-manifest";

/// Trust verdict for a manifest signature, evaluated against a release
/// keyring (ADR-0011). The evaluation order is deterministic: `Unsigned`,
/// `UnknownKey`, `Revoked`, `Expired`, `InvalidSignature`, `Trusted`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SignatureTrust {
    Trusted { key_id: String },
    Unsigned,
    UnknownKey { key_id: String },
    Revoked { key_id: String },
    Expired { key_id: String },
    InvalidSignature,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("invalid release keyring: {message}")]
pub struct KeyringValidationError {
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReleaseKey {
    pub key_id: String,
    pub algorithm: String,
    pub purpose: String,
    /// Lowercase hex of the 32-byte Ed25519 verifying key.
    pub public_key: String,
    pub valid_from_unix_ms: u64,
    pub valid_until_unix_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct KeyRevocation {
    pub key_id: String,
    pub revoked_unix_ms: u64,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReleaseKeyring {
    pub schema_version: u32,
    #[serde(default)]
    pub keys: Vec<ReleaseKey>,
    #[serde(default)]
    pub revocations: Vec<KeyRevocation>,
}

impl ReleaseKeyring {
    /// Validates keyring invariants: schema version, key identity/algorithm/
    /// purpose/validity bounds, hex keys, and unique IDs.
    ///
    /// # Errors
    ///
    /// Returns the first violated keyring invariant.
    pub fn validate(&self) -> Result<(), KeyringValidationError> {
        if self.schema_version != RELEASE_KEYRING_SCHEMA_VERSION {
            return keyring_error(format!(
                "unsupported schema version {}",
                self.schema_version
            ));
        }
        let mut key_ids = BTreeSet::new();
        for key in &self.keys {
            if !valid_engine_id(&key.key_id) {
                return keyring_error(format!(
                    "key_id must match [a-z0-9][a-z0-9._-]+: {}",
                    key.key_id
                ));
            }
            if !key_ids.insert(&key.key_id) {
                return keyring_error(format!("duplicate key_id {}", key.key_id));
            }
            if key.algorithm != SIGNATURE_ALGORITHM_ED25519 {
                return keyring_error(format!(
                    "key {} algorithm must be {SIGNATURE_ALGORITHM_ED25519}",
                    key.key_id
                ));
            }
            if key.purpose != KEYRING_PURPOSE_ENGINE_MANIFEST {
                return keyring_error(format!(
                    "key {} purpose must be {KEYRING_PURPOSE_ENGINE_MANIFEST}",
                    key.key_id
                ));
            }
            if !is_lower_hex(&key.public_key, 32) {
                return keyring_error(format!(
                    "key {} public_key must be 64 lowercase hex characters",
                    key.key_id
                ));
            }
            if key.valid_from_unix_ms == 0 || key.valid_until_unix_ms <= key.valid_from_unix_ms {
                return keyring_error(format!(
                    "key {} validity window is empty or inverted",
                    key.key_id
                ));
            }
        }
        let mut revoked_ids = BTreeSet::new();
        for revocation in &self.revocations {
            if !valid_engine_id(&revocation.key_id) {
                return keyring_error(format!(
                    "revocation key_id must match [a-z0-9][a-z0-9._-]+: {}",
                    revocation.key_id
                ));
            }
            if !revoked_ids.insert(&revocation.key_id) {
                return keyring_error(format!("duplicate revocation for {}", revocation.key_id));
            }
            if revocation.revoked_unix_ms == 0 {
                return keyring_error(format!(
                    "revocation for {} must carry a timestamp",
                    revocation.key_id
                ));
            }
        }
        Ok(())
    }
}

/// Deterministic signing payload for a manifest (ADR-0011): the compact JSON
/// serialization of the manifest with the `signature` field removed. Struct
/// field order is the schema order; future map-typed fields must serialize
/// with sorted keys. The payload never contains an install absolute path.
///
/// # Panics
///
/// Panics only if the manifest fails JSON serialization, which the type
/// system prevents for this schema.
pub fn canonical_manifest_bytes(manifest: &EngineManifest) -> Vec<u8> {
    let mut unsigned = manifest.clone();
    unsigned.signature = None;
    serde_json::to_vec(&unsigned).expect("engine manifest serializes to JSON")
}

/// Signs a manifest with an Ed25519 seed. Release tooling and tests use this
/// to produce `signature.value` over [`canonical_manifest_bytes`].
pub fn sign_manifest(
    manifest: &EngineManifest,
    key_id: &str,
    seed: &[u8; 32],
) -> ManifestSignature {
    let signing = ed25519_dalek::SigningKey::from_bytes(seed);
    let signature = signing.sign(&canonical_manifest_bytes(manifest));
    ManifestSignature {
        algorithm: SIGNATURE_ALGORITHM_ED25519.to_owned(),
        key_id: key_id.to_owned(),
        value: to_lower_hex(&signature.to_bytes()),
    }
}

/// Derives the keyring `public_key` hex for an Ed25519 seed.
pub fn ed25519_public_key_hex(seed: &[u8; 32]) -> String {
    to_lower_hex(
        ed25519_dalek::SigningKey::from_bytes(seed)
            .verifying_key()
            .as_bytes(),
    )
}

/// Evaluates a manifest signature against a release keyring (ADR-0011).
/// The caller supplies `now_unix_ms` so the verdict stays deterministic and
/// testable. The keyring itself is trusted input and should be validated with
/// [`ReleaseKeyring::validate`] before use.
#[must_use]
pub fn verify_manifest_signature(
    manifest: &EngineManifest,
    keyring: &ReleaseKeyring,
    now_unix_ms: u64,
) -> SignatureTrust {
    let Some(signature) = &manifest.signature else {
        return SignatureTrust::Unsigned;
    };
    let Some(key) = keyring
        .keys
        .iter()
        .find(|key| key.key_id == signature.key_id)
    else {
        return SignatureTrust::UnknownKey {
            key_id: signature.key_id.clone(),
        };
    };
    if keyring
        .revocations
        .iter()
        .any(|revocation| revocation.key_id == signature.key_id)
    {
        return SignatureTrust::Revoked {
            key_id: signature.key_id.clone(),
        };
    }
    if now_unix_ms < key.valid_from_unix_ms || now_unix_ms > key.valid_until_unix_ms {
        return SignatureTrust::Expired {
            key_id: key.key_id.clone(),
        };
    }
    let Some(verifying_bytes) = decode_lower_hex(&key.public_key, 32) else {
        return SignatureTrust::InvalidSignature;
    };
    let Some(signature_bytes) = decode_lower_hex(&signature.value, 64) else {
        return SignatureTrust::InvalidSignature;
    };
    let Ok(verifying) = ed25519_dalek::VerifyingKey::from_bytes(&{
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(&verifying_bytes);
        bytes
    }) else {
        return SignatureTrust::InvalidSignature;
    };
    let mut fixed_signature = [0_u8; 64];
    fixed_signature.copy_from_slice(&signature_bytes);
    let signature = ed25519_dalek::Signature::from_bytes(&fixed_signature);
    if verifying
        .verify_strict(&canonical_manifest_bytes(manifest), &signature)
        .is_ok()
    {
        SignatureTrust::Trusted {
            key_id: key.key_id.clone(),
        }
    } else {
        SignatureTrust::InvalidSignature
    }
}

fn is_lower_hex(value: &str, expected_bytes: usize) -> bool {
    value.len() == expected_bytes * 2
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn decode_lower_hex(value: &str, expected_bytes: usize) -> Option<Vec<u8>> {
    if !is_lower_hex(value, expected_bytes) {
        return None;
    }
    (0..expected_bytes)
        .map(|index| u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok())
        .collect()
}

fn to_lower_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn keyring_error<T>(message: String) -> Result<T, KeyringValidationError> {
    Err(KeyringValidationError { message })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EngineManifest {
    pub schema_version: u32,
    pub engine_id: String,
    pub version: String,
    pub platform: EnginePlatform,
    pub architecture: EngineArchitecture,
    pub protocol_version: u32,
    pub formatwright_compatibility: FormatWrightCompatibility,
    pub executables: Vec<ManifestExecutable>,
    #[serde(default)]
    pub runtime_files: Vec<ManifestRuntimeFile>,
    pub source: ManifestSource,
    pub licenses: Vec<ManifestLicense>,
    #[serde(default)]
    pub supply_chain: Option<ManifestSupplyChain>,
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    pub signature: Option<ManifestSignature>,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("invalid engine manifest: {message}")]
pub struct ManifestValidationError {
    pub message: String,
}

impl EngineManifest {
    /// Validates security-sensitive manifest invariants without accessing the
    /// engine pack filesystem.
    ///
    /// # Errors
    ///
    /// Returns the first invalid schema, protocol, identity, hash, path,
    /// license, capability, provenance, or signature invariant.
    pub fn validate(&self, supported_protocol: u32) -> Result<(), ManifestValidationError> {
        if self.schema_version != ENGINE_MANIFEST_SCHEMA_VERSION {
            return manifest_error(format!(
                "unsupported schema version {}",
                self.schema_version
            ));
        }
        if self.protocol_version != supported_protocol {
            return manifest_error(format!(
                "protocol {} does not match supported protocol {supported_protocol}",
                self.protocol_version
            ));
        }
        if !valid_engine_id(&self.engine_id) {
            return manifest_error("engine_id must match [a-z0-9][a-z0-9._-]+".to_owned());
        }
        if !valid_store_segment(&self.version) {
            return manifest_error(
                "version must be a safe portable path segment containing only letters, digits, '.', '_', '+', or '-'"
                    .to_owned(),
            );
        }
        if self.formatwright_compatibility.minimum.trim().is_empty()
            || self
                .formatwright_compatibility
                .maximum_exclusive
                .trim()
                .is_empty()
        {
            return manifest_error("FormatWright compatibility bounds are empty".to_owned());
        }
        if self.executables.is_empty() {
            return manifest_error("at least one executable is required".to_owned());
        }
        self.validate_pack_files()?;
        self.validate_provenance_and_licenses()?;
        self.validate_capabilities()?;
        if let Some(signature) = &self.signature {
            if signature.algorithm != SIGNATURE_ALGORITHM_ED25519 {
                return manifest_error(format!(
                    "signature algorithm must be {SIGNATURE_ALGORITHM_ED25519}"
                ));
            }
            if signature.key_id.trim().is_empty() || !valid_engine_id(signature.key_id.trim()) {
                return manifest_error(
                    "signature key_id must match [a-z0-9][a-z0-9._-]+".to_owned(),
                );
            }
            if !is_lower_hex(&signature.value, 64) {
                return manifest_error(
                    "signature value must be 128 lowercase hex characters".to_owned(),
                );
            }
        }
        Ok(())
    }

    fn validate_pack_files(&self) -> Result<(), ManifestValidationError> {
        let mut executable_names = BTreeSet::new();
        let mut pack_paths = BTreeSet::new();
        for executable in &self.executables {
            if executable.name.trim().is_empty() {
                return manifest_error("executable name is empty".to_owned());
            }
            if !executable_names.insert(&executable.name) {
                return manifest_error(format!("duplicate executable name {}", executable.name));
            }
            validate_relative_path(&executable.relative_path, "executable")?;
            validate_sha256(&executable.sha256, "executable")?;
            if !pack_paths.insert(&executable.relative_path) {
                return manifest_error(format!(
                    "duplicate pack path {}",
                    executable.relative_path.display()
                ));
            }
        }
        for runtime_file in &self.runtime_files {
            validate_relative_path(&runtime_file.relative_path, "runtime file")?;
            validate_sha256(&runtime_file.sha256, "runtime file")?;
            if !pack_paths.insert(&runtime_file.relative_path) {
                return manifest_error(format!(
                    "duplicate pack path {}",
                    runtime_file.relative_path.display()
                ));
            }
        }
        for license in &self.licenses {
            for (purpose, path) in [
                ("license notice", Some(&license.notice_path)),
                ("source offer", license.source_offer_path.as_ref()),
            ] {
                let Some(path) = path else {
                    continue;
                };
                validate_relative_path(path, purpose)?;
                if !pack_paths.insert(path) {
                    return manifest_error(format!("duplicate pack path {}", path.display()));
                }
            }
        }
        if let Some(supply_chain) = &self.supply_chain {
            for (purpose, path, hash) in [
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
                validate_relative_path(path, purpose)?;
                validate_sha256(hash, purpose)?;
                if !pack_paths.insert(path) {
                    return manifest_error(format!("duplicate pack path {}", path.display()));
                }
            }
            if supply_chain.sbom_path == supply_chain.sources_path {
                return manifest_error(
                    "engine SBOM and source inventory must be separate files".to_owned(),
                );
            }
        }
        Ok(())
    }

    fn validate_provenance_and_licenses(&self) -> Result<(), ManifestValidationError> {
        if !is_https_url(&self.source.project_url) || !is_https_url(&self.source.source_url) {
            return manifest_error("source URLs must use HTTPS".to_owned());
        }
        if self.source.source_revision.trim().is_empty()
            || self.source.build_configuration.trim().is_empty()
        {
            return manifest_error(
                "source revision and build configuration are required".to_owned(),
            );
        }
        if self.licenses.is_empty() {
            return manifest_error("at least one license is required".to_owned());
        }
        for license in &self.licenses {
            if license.spdx.trim().is_empty() {
                return manifest_error("license SPDX expression is empty".to_owned());
            }
        }
        Ok(())
    }

    fn validate_capabilities(&self) -> Result<(), ManifestValidationError> {
        let mut capability_ids = BTreeSet::new();
        for capability in &self.capabilities {
            if capability.capability_id.trim().is_empty()
                || capability.inputs.is_empty()
                || capability.outputs.is_empty()
            {
                return manifest_error("capability requires an ID, inputs, and outputs".to_owned());
            }
            if !capability_ids.insert(&capability.capability_id) {
                return manifest_error(format!(
                    "duplicate capability ID {}",
                    capability.capability_id
                ));
            }
            if capability
                .inputs
                .iter()
                .chain(&capability.outputs)
                .any(|format| format.trim().is_empty())
            {
                return manifest_error("capability contains an empty format ID".to_owned());
            }
        }
        Ok(())
    }
}

fn valid_engine_id(value: &str) -> bool {
    value.len() >= 2
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn valid_store_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn validate_relative_path(path: &Path, purpose: &str) -> Result<(), ManifestValidationError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return manifest_error(format!("{purpose} path must be a safe relative path"));
    }
    Ok(())
}

fn validate_sha256(value: &str, purpose: &str) -> Result<(), ManifestValidationError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return manifest_error(format!(
            "{purpose} SHA-256 must contain 64 hexadecimal digits"
        ));
    }
    Ok(())
}

fn is_https_url(value: &str) -> bool {
    value.starts_with("https://") && value.len() > "https://".len()
}

fn manifest_error<T>(message: String) -> Result<T, ManifestValidationError> {
    Err(ManifestValidationError { message })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EngineHealth {
    pub executable: String,
    pub available: bool,
    pub identity: Option<EngineIdentity>,
    pub error_code: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DoctorReport {
    pub engines: BTreeMap<String, EngineHealth>,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::{
        Capability, EngineArchitecture, EngineManifest, EnginePlatform, FormatWrightCompatibility,
        LossClass, ManifestExecutable, ManifestLicense, ManifestSource, Operation,
    };

    fn manifest() -> EngineManifest {
        EngineManifest {
            schema_version: 1,
            engine_id: "fixture-engine".to_owned(),
            version: "1.0.0".to_owned(),
            platform: EnginePlatform::current().unwrap_or(EnginePlatform::Linux),
            architecture: EngineArchitecture::current().unwrap_or(EngineArchitecture::X86_64),
            protocol_version: 1,
            formatwright_compatibility: FormatWrightCompatibility {
                minimum: "0.1.0".to_owned(),
                maximum_exclusive: "0.2.0".to_owned(),
            },
            executables: vec![ManifestExecutable {
                name: "fixture".to_owned(),
                relative_path: PathBuf::from("bin/fixture"),
                sha256: "ab".repeat(32),
            }],
            runtime_files: Vec::new(),
            source: ManifestSource {
                project_url: "https://example.invalid/project".to_owned(),
                source_url: "https://example.invalid/source".to_owned(),
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

    #[test]
    fn validates_a_minimal_manifest() {
        manifest().validate(1).expect("valid manifest");
    }

    #[test]
    fn rejects_path_traversal() {
        let mut value = manifest();
        value.executables[0].relative_path = PathBuf::from("../outside");
        let error = value.validate(1).expect_err("traversal must fail");
        assert!(error.message.contains("safe relative path"));
    }

    #[test]
    fn rejects_protocol_mismatch() {
        let error = manifest().validate(2).expect_err("protocol must match");
        assert!(error.message.contains("does not match"));
    }

    #[test]
    fn rejects_duplicate_capability_ids() {
        let mut value = manifest();
        value.capabilities.push(value.capabilities[0].clone());
        let error = value
            .validate(1)
            .expect_err("duplicate capability must fail");
        assert!(error.message.contains("duplicate capability"));
    }

    fn keyring(seed: &[u8; 32], from: u64, until: u64, revoked: bool) -> super::ReleaseKeyring {
        super::ReleaseKeyring {
            schema_version: super::RELEASE_KEYRING_SCHEMA_VERSION,
            keys: vec![super::ReleaseKey {
                key_id: "release-2026h2".to_owned(),
                algorithm: super::SIGNATURE_ALGORITHM_ED25519.to_owned(),
                purpose: super::KEYRING_PURPOSE_ENGINE_MANIFEST.to_owned(),
                public_key: super::ed25519_public_key_hex(seed),
                valid_from_unix_ms: from,
                valid_until_unix_ms: until,
            }],
            revocations: if revoked {
                vec![super::KeyRevocation {
                    key_id: "release-2026h2".to_owned(),
                    revoked_unix_ms: 5,
                    reason: "test revocation".to_owned(),
                }]
            } else {
                Vec::new()
            },
        }
    }

    const NOW: u64 = 1_800_000_000_000;
    const WINDOW: (u64, u64) = (NOW - 1_000, NOW + 1_000);
    const SEED: [u8; 32] = [7; 32];

    #[test]
    fn signs_and_verifies_a_manifest_as_trusted() {
        let mut value = manifest();
        value.signature = Some(super::sign_manifest(&value, "release-2026h2", &SEED));
        value.validate(1).expect("signed manifest validates");
        let trust = super::verify_manifest_signature(
            &value,
            &keyring(&SEED, WINDOW.0, WINDOW.1, false),
            NOW,
        );
        assert_eq!(
            trust,
            super::SignatureTrust::Trusted {
                key_id: "release-2026h2".to_owned()
            }
        );
    }

    #[test]
    fn canonical_bytes_exclude_the_signature_field() {
        let unsigned = manifest();
        let mut signed = unsigned.clone();
        signed.signature = Some(super::sign_manifest(&signed, "release-2026h2", &SEED));
        assert_eq!(
            super::canonical_manifest_bytes(&signed),
            super::canonical_manifest_bytes(&unsigned)
        );
        let text = String::from_utf8(super::canonical_manifest_bytes(&signed)).expect("utf-8");
        assert!(text.contains("\"signature\":null"));
        assert!(!text.contains(&signed.signature.as_ref().expect("signature").value));
    }

    #[test]
    fn rejects_a_tampered_manifest_or_replayed_signature() {
        let mut value = manifest();
        value.signature = Some(super::sign_manifest(&value, "release-2026h2", &SEED));
        value.executables[0].sha256 = "cd".repeat(32);
        assert_eq!(
            super::verify_manifest_signature(
                &value,
                &keyring(&SEED, WINDOW.0, WINDOW.1, false),
                NOW
            ),
            super::SignatureTrust::InvalidSignature
        );
        let other = manifest();
        let mut replayed = other.clone();
        replayed.engine_id = "other-engine".to_owned();
        replayed.signature = value.signature.clone();
        assert_eq!(
            super::verify_manifest_signature(
                &replayed,
                &keyring(&SEED, WINDOW.0, WINDOW.1, false),
                NOW
            ),
            super::SignatureTrust::InvalidSignature
        );
    }

    #[test]
    fn distinguishes_unsigned_unknown_revoked_and_expired_keys() {
        let trusted_keyring = keyring(&SEED, WINDOW.0, WINDOW.1, false);
        assert_eq!(
            super::verify_manifest_signature(&manifest(), &trusted_keyring, NOW),
            super::SignatureTrust::Unsigned
        );
        let mut unknown = manifest();
        unknown.signature = Some(super::sign_manifest(&unknown, "release-2026h1", &SEED));
        assert_eq!(
            super::verify_manifest_signature(&unknown, &trusted_keyring, NOW),
            super::SignatureTrust::UnknownKey {
                key_id: "release-2026h1".to_owned()
            }
        );
        let mut wrong_key = manifest();
        wrong_key.signature = Some(super::sign_manifest(&wrong_key, "release-2026h2", &[9; 32]));
        assert_eq!(
            super::verify_manifest_signature(&wrong_key, &trusted_keyring, NOW),
            super::SignatureTrust::InvalidSignature
        );
        let mut known = manifest();
        known.signature = Some(super::sign_manifest(&known, "release-2026h2", &SEED));
        assert_eq!(
            super::verify_manifest_signature(
                &known,
                &keyring(&SEED, WINDOW.0, WINDOW.1, true),
                NOW
            ),
            super::SignatureTrust::Revoked {
                key_id: "release-2026h2".to_owned()
            }
        );
        assert_eq!(
            super::verify_manifest_signature(
                &known,
                &keyring(&SEED, NOW + 10, NOW + 20, false),
                NOW
            ),
            super::SignatureTrust::Expired {
                key_id: "release-2026h2".to_owned()
            }
        );
    }

    #[test]
    fn validates_keyring_shape_and_rejects_bad_signatures_in_manifests() {
        keyring(&SEED, WINDOW.0, WINDOW.1, false)
            .validate()
            .expect("keyring validates");
        let mut bad_keyring = keyring(&SEED, WINDOW.0, WINDOW.1, false);
        bad_keyring.keys[0].public_key = "XYZ".to_owned();
        assert!(bad_keyring.validate().is_err());
        let mut empty_window = keyring(&SEED, 100, 100, false);
        empty_window.keys[0].valid_until_unix_ms = 100;
        assert!(empty_window.validate().is_err());

        let mut bad_signature = manifest();
        bad_signature.signature = Some(super::ManifestSignature {
            algorithm: "rsa-sha256".to_owned(),
            key_id: "release-2026h2".to_owned(),
            value: "ab".repeat(64),
        });
        let error = bad_signature
            .validate(1)
            .expect_err("algorithm must be pinned");
        assert!(error.message.contains("algorithm"));

        let mut uppercase = manifest();
        uppercase.signature = Some(super::ManifestSignature {
            algorithm: super::SIGNATURE_ALGORITHM_ED25519.to_owned(),
            key_id: "release-2026h2".to_owned(),
            value: "AB".repeat(64),
        });
        let error = uppercase.validate(1).expect_err("hex must be lowercase");
        assert!(error.message.contains("lowercase hex"));
    }
}
