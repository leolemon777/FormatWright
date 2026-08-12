#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

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
pub struct ManifestSignature {
    pub algorithm: String,
    pub key_id: String,
    pub value: String,
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
    pub source: ManifestSource,
    pub licenses: Vec<ManifestLicense>,
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
        if self.version.trim().is_empty() {
            return manifest_error("version is empty".to_owned());
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
        let mut executable_names = BTreeSet::new();
        for executable in &self.executables {
            if executable.name.trim().is_empty() {
                return manifest_error("executable name is empty".to_owned());
            }
            if !executable_names.insert(&executable.name) {
                return manifest_error(format!("duplicate executable name {}", executable.name));
            }
            validate_relative_path(&executable.relative_path, "executable")?;
            validate_sha256(&executable.sha256, "executable")?;
        }
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
            validate_relative_path(&license.notice_path, "license notice")?;
            if let Some(path) = &license.source_offer_path {
                validate_relative_path(path, "source offer")?;
            }
        }
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
        if let Some(signature) = &self.signature
            && (signature.algorithm.trim().is_empty()
                || signature.key_id.trim().is_empty()
                || signature.value.trim().is_empty())
        {
            return manifest_error("signature fields must all be populated".to_owned());
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
}
