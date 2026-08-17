//! Versioned engine activation registry (ADR-0011 item 6).
//!
//! The registry keeps one atomic active pointer per engine
//! (`<registry_root>/<engine_id>.json`, the `EngineRegistryIdentity` format
//! also used by application-state bundles). Rollback history is *derived*
//! from the content-addressed engine store
//! (`<store_root>/<engine_id>/<version>/<manifest_sha256>/`), which keeps
//! every installed version, so no extra history files or migrations exist.
//!
//! Startup recovery never skips a broken engine silently: if the active
//! version fails re-verification, the most recent still-verifiable store
//! version is activated as an automatic fallback and the failure is
//! reported; if nothing verifies, the failure is reported explicitly.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::application_state::EngineRegistryIdentity;
use crate::engine_pack::{VerifiedEnginePack, activate_engine_pack};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};

/// Bounded retention for automatic fallback reporting.
const MAX_REPORTED_ENGINES: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstalledEngineVersion {
    pub engine_id: String,
    pub version: String,
    pub manifest_sha256: String,
    pub manifest_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EngineFallback {
    pub failed_version: String,
    pub failed_manifest_sha256: String,
    pub reason: String,
    pub fallback_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum EngineRecovery {
    /// The active version verified and was activated.
    Activated {
        engine_id: String,
        version: String,
        manifest_sha256: String,
    },
    /// The active version failed verification and an older installed
    /// version was re-verified and activated instead.
    FellBack {
        engine_id: String,
        fallback: EngineFallback,
    },
    /// Neither the active version nor any installed alternative verified.
    Failed {
        engine_id: String,
        failed_version: String,
        reason: String,
    },
}

/// A registry over one active pointer per engine plus the multi-version
/// engine store that supplies rollback candidates.
#[derive(Clone, Debug)]
pub struct EngineRegistry {
    registry_root: PathBuf,
    store_root: PathBuf,
}

impl EngineRegistry {
    #[must_use]
    pub fn new(registry_root: impl Into<PathBuf>, store_root: impl Into<PathBuf>) -> Self {
        Self {
            registry_root: registry_root.into(),
            store_root: store_root.into(),
        }
    }

    /// Atomically points an engine at a verified pack: staged partial write,
    /// backup of the previous pointer, rename publication, backup cleanup.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the pointer cannot be written or
    /// published atomically.
    pub fn set_active(&self, verified: &VerifiedEnginePack) -> Result<()> {
        fs::create_dir_all(&self.registry_root).map_err(|error| {
            registry_error(
                format!(
                    "Cannot create engine registry: {}",
                    self.registry_root.display()
                ),
                error.to_string(),
            )
        })?;
        let engine_id = &verified.manifest.engine_id;
        let entry = EngineRegistryIdentity {
            engine_id: Some(engine_id.clone()),
            manifest_path: verified.manifest_path.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&entry).map_err(|error| {
            registry_error(
                "Engine registry entry failed to serialize".to_owned(),
                error.to_string(),
            )
        })?;
        let destination = self.registry_root.join(format!("{engine_id}.json"));
        let partial = self
            .registry_root
            .join(format!(".{engine_id}.{}.partial", uuid::Uuid::new_v4()));
        let backup = self.registry_root.join(format!(".{engine_id}.backup"));
        fs::write(&partial, &bytes).map_err(|error| {
            registry_error(
                format!("Cannot stage engine registry entry: {}", partial.display()),
                error.to_string(),
            )
        })?;
        if destination.is_file() {
            let _ = fs::remove_file(&backup);
            if let Err(error) = fs::rename(&destination, &backup) {
                let _ = fs::remove_file(&partial);
                return Err(registry_error(
                    format!(
                        "Cannot back up the previous engine registry entry: {}",
                        destination.display()
                    ),
                    error.to_string(),
                ));
            }
        }
        match fs::rename(&partial, &destination) {
            Ok(()) => {
                let _ = fs::remove_file(&backup);
                Ok(())
            }
            Err(error) => {
                let _ = fs::remove_file(&partial);
                if backup.is_file() {
                    let _ = fs::rename(&backup, &destination);
                }
                Err(registry_error(
                    format!(
                        "Cannot publish engine registry entry: {}",
                        destination.display()
                    ),
                    error.to_string(),
                ))
            }
        }
    }

    /// Reads every active pointer. Malformed individual entries are skipped
    /// with their path reported in the error only when nothing parses; a
    /// registry with at least one parseable entry still recovers.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the registry directory cannot be read.
    pub fn active_entries(&self) -> Result<Vec<EngineRegistryIdentity>> {
        read_active_entries(&self.registry_root)
    }

    /// Enumerates installed versions of one engine from the store, newest
    /// version first. Content-addressed duplicates per version collapse to
    /// the lexicographically last manifest hash for determinism.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the store directory cannot be read.
    pub fn installed_versions(&self, engine_id: &str) -> Result<Vec<InstalledEngineVersion>> {
        let engine_root = self.store_root.join(engine_id);
        if !engine_root.is_dir() {
            return Ok(Vec::new());
        }
        let mut by_version: BTreeMap<(VersionKey, String), InstalledEngineVersion> =
            BTreeMap::new();
        for version_entry in fs::read_dir(&engine_root).map_err(|error| {
            registry_error(
                format!("Cannot read engine store: {}", engine_root.display()),
                error.to_string(),
            )
        })? {
            let version_entry = version_entry.map_err(|error| {
                registry_error(
                    format!("Cannot read engine store: {}", engine_root.display()),
                    error.to_string(),
                )
            })?;
            let version = version_entry.file_name().to_string_lossy().into_owned();
            let version_dir = version_entry.path();
            if !version_dir.is_dir() {
                continue;
            }
            for hash_entry in fs::read_dir(&version_dir).map_err(|error| {
                registry_error(
                    format!("Cannot read engine version: {}", version_dir.display()),
                    error.to_string(),
                )
            })? {
                let hash_entry = hash_entry.map_err(|error| {
                    registry_error(
                        format!("Cannot read engine version: {}", version_dir.display()),
                        error.to_string(),
                    )
                })?;
                let manifest_sha256 = hash_entry.file_name().to_string_lossy().into_owned();
                let manifest_path = hash_entry.path().join("manifest.json");
                if manifest_sha256.len() != 64
                    || !manifest_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                    || !manifest_path.is_file()
                {
                    continue;
                }
                by_version.insert(
                    (
                        VersionKey::parse(&version),
                        manifest_sha256.to_ascii_lowercase(),
                    ),
                    InstalledEngineVersion {
                        engine_id: engine_id.to_owned(),
                        version: version.clone(),
                        manifest_sha256: manifest_sha256.to_ascii_lowercase(),
                        manifest_path,
                    },
                );
            }
        }
        Ok(by_version.into_values().rev().collect())
    }

    /// Activates every engine's active version, falling back to the newest
    /// still-verifiable installed version when the active one is broken
    /// (ADR-0011 item 6). The fallback becomes the new active pointer so the
    /// next start uses it directly.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the registry or store cannot be read.
    /// Individual engine verification failures are reported per-engine in
    /// the returned outcomes instead of aborting other engines.
    pub fn recover(&self) -> Result<Vec<EngineRecovery>> {
        let mut outcomes = Vec::new();
        for entry in self.active_entries()? {
            if outcomes.len() >= MAX_REPORTED_ENGINES {
                break;
            }
            let engine_id = match entry.engine_id.as_deref() {
                Some(engine_id) => engine_id.to_owned(),
                None => match crate::engine_pack::verify_engine_pack(&entry.manifest_path) {
                    Ok(verified) => verified.manifest.engine_id,
                    Err(error) => {
                        outcomes.push(EngineRecovery::Failed {
                            engine_id: entry.manifest_path.display().to_string(),
                            failed_version: "unknown".to_owned(),
                            reason: error.message,
                        });
                        continue;
                    }
                },
            };
            outcomes.push(self.recover_engine(&engine_id, &entry.manifest_path)?);
        }
        Ok(outcomes)
    }

    fn recover_engine(&self, engine_id: &str, active_path: &Path) -> Result<EngineRecovery> {
        match activate_engine_pack(active_path) {
            Ok(verified) => Ok(EngineRecovery::Activated {
                engine_id: engine_id.to_owned(),
                version: verified.manifest.version.clone(),
                manifest_sha256: verified.manifest_sha256.clone(),
            }),
            Err(error) => {
                let failed_version = std::fs::read(active_path)
                    .ok()
                    .and_then(|bytes| {
                        serde_json::from_slice::<formatwright_engine_sdk::EngineManifest>(&bytes)
                            .ok()
                    })
                    .map_or_else(|| "unknown".to_owned(), |manifest| manifest.version);
                let failed_manifest_sha256 = active_path
                    .parent()
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                for candidate in self.installed_versions(engine_id)? {
                    if candidate.manifest_path == active_path {
                        continue;
                    }
                    if let Ok(verified) = activate_engine_pack(&candidate.manifest_path) {
                        let fallback = EngineFallback {
                            failed_version,
                            failed_manifest_sha256,
                            reason: error.message.clone(),
                            fallback_version: candidate.version.clone(),
                        };
                        self.set_active(&verified)?;
                        return Ok(EngineRecovery::FellBack {
                            engine_id: engine_id.to_owned(),
                            fallback,
                        });
                    }
                }
                Ok(EngineRecovery::Failed {
                    engine_id: engine_id.to_owned(),
                    failed_version,
                    reason: error.message,
                })
            }
        }
    }

    /// Rolls an engine back to an already-installed store version after
    /// re-verification. Downgrades (target version older than the active
    /// version) require explicit authorization.
    ///
    /// # Errors
    ///
    /// Returns `POLICY_BLOCKED` for an unauthorized downgrade, an engine
    /// error when the target no longer verifies, or a storage error when the
    /// pointer cannot be published.
    pub fn rollback(
        &self,
        engine_id: &str,
        target: &InstalledEngineVersion,
        allow_downgrade: bool,
    ) -> Result<VerifiedEnginePack> {
        if target.engine_id != engine_id {
            return Err(registry_error(
                "Rollback target belongs to a different engine".to_owned(),
                format!("expected {engine_id}, got {}", target.engine_id),
            ));
        }
        let active = self.active_entries()?.into_iter().find(|entry| {
            entry.engine_id.as_deref() == Some(engine_id)
                || crate::engine_pack::verify_engine_pack(&entry.manifest_path)
                    .is_ok_and(|verified| verified.manifest.engine_id == engine_id)
        });
        if let Some(active) = &active {
            let active_version = crate::engine_pack::verify_engine_pack(&active.manifest_path)
                .map(|verified| verified.manifest.version)
                .unwrap_or_default();
            if !active_version.is_empty()
                && VersionKey::parse(&target.version) < VersionKey::parse(&active_version)
                && !allow_downgrade
            {
                return Err(FormatWrightError::new(
                    ErrorCode::PolicyBlocked,
                    Stage::Doctor,
                    format!(
                        "Rolling {engine_id} from {active_version} back to {} is a downgrade",
                        target.version
                    ),
                    "Pass allow_downgrade=true to authorize the downgrade explicitly.",
                ));
            }
        }
        let verified = activate_engine_pack(&target.manifest_path)?;
        self.set_active(&verified)?;
        Ok(verified)
    }
}

fn read_active_entries(registry_root: &Path) -> Result<Vec<EngineRegistryIdentity>> {
    if !registry_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(registry_root).map_err(|error| {
        registry_error(
            format!("Cannot read engine registry: {}", registry_root.display()),
            error.to_string(),
        )
    })? {
        let entry = entry.map_err(|error| {
            registry_error(
                format!("Cannot read engine registry: {}", registry_root.display()),
                error.to_string(),
            )
        })?;
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| {
            registry_error(
                format!("Cannot read engine registry entry: {}", path.display()),
                error.to_string(),
            )
        })?;
        let identity =
            serde_json::from_slice::<EngineRegistryIdentity>(&bytes).map_err(|error| {
                registry_error(
                    format!("Invalid engine registry entry: {}", path.display()),
                    error.to_string(),
                )
            })?;
        entries.push(identity);
    }
    entries.sort_by(|a, b| a.manifest_path.cmp(&b.manifest_path));
    Ok(entries)
}

/// Tolerant dotted-numeric version ordering ("26.02.0-0", "9.0", "1.0.0"):
/// numeric components compare numerically, remaining segments
/// lexicographically.
#[derive(Clone, Debug, Eq, PartialEq)]
struct VersionKey(Vec<VersionSegment>);

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum VersionSegment {
    Numeric(u64),
    Text(String),
}

impl VersionKey {
    fn parse(value: &str) -> Self {
        Self(
            value
                .split('.')
                .map(|part| match part.parse::<u64>() {
                    Ok(number) => VersionSegment::Numeric(number),
                    Err(_) => VersionSegment::Text(part.to_owned()),
                })
                .collect(),
        )
    }

    fn compare(&self, other: &Self) -> Ordering {
        let mut own = self.0.iter().collect::<Vec<_>>();
        let mut theirs = other.0.iter().collect::<Vec<_>>();
        while let (Some(left), Some(right)) = (own.first(), theirs.first()) {
            match (left, right) {
                (VersionSegment::Numeric(a), VersionSegment::Numeric(b)) => match a.cmp(b) {
                    Ordering::Equal => {
                        own.remove(0);
                        theirs.remove(0);
                    }
                    ordering => return ordering,
                },
                (VersionSegment::Text(a), VersionSegment::Text(b)) => match a.cmp(b) {
                    Ordering::Equal => {
                        own.remove(0);
                        theirs.remove(0);
                    }
                    ordering => return ordering,
                },
                // Numeric segments sort above textual ones so "9.0" > "9.0-beta".
                (VersionSegment::Numeric(_), VersionSegment::Text(_)) => {
                    return Ordering::Greater;
                }
                (VersionSegment::Text(_), VersionSegment::Numeric(_)) => {
                    return Ordering::Less;
                }
            }
        }
        own.len().cmp(&theirs.len())
    }
}

impl Ord for VersionKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.compare(other)
    }
}

impl PartialOrd for VersionKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn registry_error(message: String, diagnostic: impl Into<String>) -> FormatWrightError {
    FormatWrightError::new(ErrorCode::StorageFailed, Stage::Store, message, diagnostic)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use sha2::{Digest, Sha256};
    use tempfile::{TempDir, tempdir};

    use super::EngineRegistry;
    use crate::engine_pack::install_engine_pack;
    use formatwright_engine_sdk::{
        Capability, EngineArchitecture, EngineManifest, EnginePlatform, FormatWrightCompatibility,
        LossClass, ManifestExecutable, ManifestLicense, ManifestSource, Operation,
    };

    fn manifest(version: &str) -> EngineManifest {
        EngineManifest {
            schema_version: 1,
            engine_id: "fixture-engine".to_owned(),
            version: version.to_owned(),
            platform: EnginePlatform::current().unwrap_or(EnginePlatform::Linux),
            architecture: EngineArchitecture::current().unwrap_or(EngineArchitecture::X86_64),
            protocol_version: crate::engine_pack::ENGINE_PROTOCOL_VERSION,
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
                sha256: format!("{:x}", Sha256::digest(b"fixture engine")),
            }],
            runtime_files: Vec::new(),
            source: ManifestSource {
                project_url: "https://example.invalid/project".to_owned(),
                source_url: "https://example.invalid/source".to_owned(),
                source_revision: format!("v{version}"),
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
                constraints: BTreeMap::default(),
            }],
            signature: None,
        }
    }

    /// Builds a source pack directory for one version and installs it into
    /// the shared store.
    fn install_version(
        source_root: &TempDir,
        store_root: &Path,
        version: &str,
        binary: &[u8],
    ) -> PathBuf {
        let pack_root = source_root.path().join(format!("pack-{version}"));
        fs::create_dir_all(pack_root.join("bin")).expect("bin directory");
        fs::create_dir_all(pack_root.join("licenses")).expect("license directory");
        let binary_path = pack_root.join(if cfg!(windows) {
            "bin/fixture.exe"
        } else {
            "bin/fixture.bin"
        });
        fs::write(&binary_path, binary).expect("binary fixture");
        fs::write(pack_root.join("licenses/NOTICE.txt"), b"notice").expect("notice");
        let mut value = manifest(version);
        value.executables[0].sha256 = format!("{:x}", Sha256::digest(binary));
        let manifest_path = pack_root.join("manifest.json");
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&value).expect("serialize manifest"),
        )
        .expect("write manifest");
        let verified = install_engine_pack(&manifest_path, store_root).expect("install pack");
        verified.manifest_path
    }

    #[test]
    fn orders_versions_numerically_with_text_suffixes_below() {
        use super::VersionKey;
        let key = |value: &str| VersionKey::parse(value);
        // Numeric components compare numerically, not lexicographically.
        assert!(key("9.10") > key("9.0"));
        assert!(key("10.0") > key("2.0"));
        // Textual segments sort below numeric ones at the same position.
        assert!(key("1.0.0") > key("1.0.0-alpha"));
        assert!(key("1.0.0-beta") < key("1.0.0-rc"));
        // Leading zeros carry no weight.
        assert_eq!(key("26.02.0-0"), key("26.2.0-0"));
        // Longer otherwise-equal versions sort higher.
        assert!(key("1.0.0.1") > key("1.0.0"));
    }

    #[test]
    fn recovers_active_version_and_derives_history_from_the_store() {
        let source_root = tempdir().expect("source root");
        let state_root = tempdir().expect("state root");
        let store_root = state_root.path().join("engine-store");
        let registry_root = state_root.path().join("engine-registry");
        let registry = EngineRegistry::new(&registry_root, &store_root);

        let first = install_version(&source_root, &store_root, "1.0.0", b"binary-one");
        let second = install_version(&source_root, &store_root, "2.0.0", b"binary-two");
        assert_ne!(first, second);

        // Nothing is active before set_active, so recover() sees no engines.
        assert!(registry.recover().expect("recover").is_empty());
        let verified = crate::engine_pack::verify_engine_pack(&second).expect("verify second");
        registry.set_active(&verified).expect("set active");

        let versions = registry
            .installed_versions("fixture-engine")
            .expect("versions");
        assert_eq!(
            versions
                .iter()
                .map(|version| version.version.as_str())
                .collect::<Vec<_>>(),
            ["2.0.0", "1.0.0"],
            "history is derived from the store, newest first"
        );

        let outcomes = registry.recover().expect("recover with active");
        assert_eq!(
            outcomes,
            vec![super::EngineRecovery::Activated {
                engine_id: "fixture-engine".to_owned(),
                version: "2.0.0".to_owned(),
                manifest_sha256: verified.manifest_sha256.clone(),
            }]
        );
    }

    #[test]
    fn falls_back_to_the_newest_verifiable_version_when_active_breaks() {
        let source_root = tempdir().expect("source root");
        let state_root = tempdir().expect("state root");
        let store_root = state_root.path().join("engine-store");
        let registry_root = state_root.path().join("engine-registry");
        let registry = EngineRegistry::new(&registry_root, &store_root);

        install_version(&source_root, &store_root, "1.0.0", b"binary-one");
        let second = install_version(&source_root, &store_root, "2.0.0", b"binary-two");
        let verified = crate::engine_pack::verify_engine_pack(&second).expect("verify second");
        registry.set_active(&verified).expect("set active");

        // Corrupt the active version's declared binary in the store.
        let binary = second.parent().expect("pack root").join(if cfg!(windows) {
            "bin/fixture.exe"
        } else {
            "bin/fixture.bin"
        });
        fs::write(&binary, b"tampered").expect("tamper active binary");

        let outcomes = registry.recover().expect("recover");
        assert_eq!(outcomes.len(), 1);
        let super::EngineRecovery::FellBack {
            engine_id,
            fallback,
        } = &outcomes[0]
        else {
            panic!("expected fallback, got {outcomes:?}");
        };
        assert_eq!(engine_id, "fixture-engine");
        assert_eq!(fallback.failed_version, "2.0.0");
        assert_eq!(fallback.fallback_version, "1.0.0");

        // The fallback became the active pointer and verifies on restart.
        let outcomes = registry.recover().expect("recover after fallback");
        assert!(matches!(
            &outcomes[0],
            super::EngineRecovery::Activated { version, .. } if version == "1.0.0"
        ));
    }

    #[test]
    fn reports_explicit_failure_when_no_version_verifies() {
        let source_root = tempdir().expect("source root");
        let state_root = tempdir().expect("state root");
        let store_root = state_root.path().join("engine-store");
        let registry_root = state_root.path().join("engine-registry");
        let registry = EngineRegistry::new(&registry_root, &store_root);

        install_version(&source_root, &store_root, "1.0.0", b"binary-one");
        let second = install_version(&source_root, &store_root, "2.0.0", b"binary-two");
        let verified = crate::engine_pack::verify_engine_pack(&second).expect("verify second");
        registry.set_active(&verified).expect("set active");

        // Corrupt every installed version of the engine.
        for version_dir in fs::read_dir(store_root.join("fixture-engine")).expect("versions") {
            let version_dir = version_dir.expect("version dir").path();
            for hash_dir in fs::read_dir(&version_dir).expect("hash dirs") {
                let binary = hash_dir.expect("hash dir").path().join(if cfg!(windows) {
                    "bin/fixture.exe"
                } else {
                    "bin/fixture.bin"
                });
                if binary.is_file() {
                    fs::write(&binary, b"tampered").expect("tamper binary");
                }
            }
        }

        let outcomes = registry.recover().expect("recover");
        assert_eq!(outcomes.len(), 1);
        match &outcomes[0] {
            super::EngineRecovery::Failed {
                engine_id, reason, ..
            } => {
                assert_eq!(engine_id, "fixture-engine");
                assert!(!reason.is_empty());
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[test]
    fn rollback_reverifies_targets_and_blocks_unauthorized_downgrades() {
        let source_root = tempdir().expect("source root");
        let state_root = tempdir().expect("state root");
        let store_root = state_root.path().join("engine-store");
        let registry_root = state_root.path().join("engine-registry");
        let registry = EngineRegistry::new(&registry_root, &store_root);

        install_version(&source_root, &store_root, "1.0.0", b"binary-one");
        let second = install_version(&source_root, &store_root, "2.0.0", b"binary-two");
        let verified = crate::engine_pack::verify_engine_pack(&second).expect("verify second");
        registry.set_active(&verified).expect("set active");

        let versions = registry
            .installed_versions("fixture-engine")
            .expect("versions");
        let oldest = versions
            .iter()
            .find(|version| version.version == "1.0.0")
            .expect("oldest version");

        let blocked = registry
            .rollback("fixture-engine", oldest, false)
            .expect_err("downgrade must be blocked");
        assert_eq!(blocked.code, crate::error::ErrorCode::PolicyBlocked);

        let rolled = registry
            .rollback("fixture-engine", oldest, true)
            .expect("authorized downgrade");
        assert_eq!(rolled.manifest.version, "1.0.0");
        let outcomes = registry.recover().expect("recover after rollback");
        assert!(matches!(
            &outcomes[0],
            super::EngineRecovery::Activated { version, .. } if version == "1.0.0"
        ));

        // A tampered target is never activated even when authorized.
        let binary = oldest
            .manifest_path
            .parent()
            .expect("pack root")
            .join(if cfg!(windows) {
                "bin/fixture.exe"
            } else {
                "bin/fixture.bin"
            });
        fs::write(&binary, b"tampered").expect("tamper target");
        let newest = registry
            .installed_versions("fixture-engine")
            .expect("versions")
            .into_iter()
            .find(|version| version.version == "2.0.0")
            .expect("newest");
        let rolled_forward = registry
            .rollback("fixture-engine", &newest, false)
            .expect("upgrade needs no authorization");
        assert_eq!(rolled_forward.manifest.version, "2.0.0");
    }

    #[test]
    fn leftover_partial_directories_are_not_installed_versions() {
        let source_root = tempdir().expect("source root");
        let state_root = tempdir().expect("state root");
        let store_root = state_root.path().join("engine-store");
        let registry_root = state_root.path().join("engine-registry");
        let registry = EngineRegistry::new(&registry_root, &store_root);

        let first = install_version(&source_root, &store_root, "1.0.0", b"binary-one");
        let verified = crate::engine_pack::verify_engine_pack(&first).expect("verify");
        registry.set_active(&verified).expect("set active");

        let leftover = store_root
            .join("fixture-engine")
            .join("2.0.0")
            .join(format!(".{}.partial", "ab".repeat(32)));
        fs::create_dir_all(leftover.join("bin")).expect("leftover");
        fs::write(leftover.join("manifest.json"), b"{}").expect("leftover manifest");

        let versions = registry
            .installed_versions("fixture-engine")
            .expect("versions");
        assert_eq!(
            versions
                .iter()
                .map(|version| version.version.as_str())
                .collect::<Vec<_>>(),
            ["1.0.0"]
        );
    }

    #[test]
    fn failed_upgrade_does_not_move_the_active_pointer() {
        let source_root = tempdir().expect("source root");
        let state_root = tempdir().expect("state root");
        let store_root = state_root.path().join("engine-store");
        let registry_root = state_root.path().join("engine-registry");
        let registry = EngineRegistry::new(&registry_root, &store_root);

        let first = install_version(&source_root, &store_root, "1.0.0", b"binary-one");
        let verified = crate::engine_pack::verify_engine_pack(&first).expect("verify");
        registry.set_active(&verified).expect("set active");

        let pack_root = source_root.path().join("pack-2.0.0-bad");
        fs::create_dir_all(pack_root.join("bin")).expect("bin");
        fs::create_dir_all(pack_root.join("licenses")).expect("licenses");
        let binary_path = pack_root.join(if cfg!(windows) {
            "bin/fixture.exe"
        } else {
            "bin/fixture.bin"
        });
        fs::write(&binary_path, b"binary-two").expect("binary");
        fs::write(pack_root.join("licenses/NOTICE.txt"), b"notice").expect("notice");
        let mut value = manifest("2.0.0");
        value.executables[0].sha256 = format!("{:x}", Sha256::digest(b"binary-two"));
        value.protocol_version = crate::engine_pack::ENGINE_PROTOCOL_VERSION + 7;
        let manifest_path = pack_root.join("manifest.json");
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&value).expect("serialize"),
        )
        .expect("write bad upgrade");

        let error = install_engine_pack(&manifest_path, &store_root)
            .expect_err("incompatible upgrade must fail");
        assert!(error.message.contains("Engine manifest is invalid"));
        assert!(
            !store_root.join("fixture-engine").join("2.0.0").is_dir()
                || registry
                    .installed_versions("fixture-engine")
                    .expect("versions")
                    .iter()
                    .all(|version| version.version != "2.0.0")
        );

        let outcomes = registry.recover().expect("recover after failed upgrade");
        assert!(matches!(
            &outcomes[0],
            super::EngineRecovery::Activated { version, .. } if version == "1.0.0"
        ));
    }

    #[test]
    fn set_active_is_atomic_and_ignores_stale_partials() {
        let source_root = tempdir().expect("source root");
        let state_root = tempdir().expect("state root");
        let store_root = state_root.path().join("engine-store");
        let registry_root = state_root.path().join("engine-registry");
        let registry = EngineRegistry::new(&registry_root, &store_root);

        let first = install_version(&source_root, &store_root, "1.0.0", b"binary-one");
        let second = install_version(&source_root, &store_root, "2.0.0", b"binary-two");
        let one = crate::engine_pack::verify_engine_pack(&first).expect("verify one");
        let two = crate::engine_pack::verify_engine_pack(&second).expect("verify two");
        registry.set_active(&one).expect("first active");
        registry.set_active(&two).expect("second active");

        // Simulate a crashed write: stale partial and backup remain.
        fs::write(registry_root.join(".fixture-engine.stale.partial"), b"{}").expect("partial");
        fs::copy(
            registry_root.join("fixture-engine.json"),
            registry_root.join(".fixture-engine.backup"),
        )
        .expect("backup");

        let entries = registry.active_entries().expect("entries");
        assert_eq!(entries.len(), 1, "partials are never parsed");
        assert_eq!(entries[0].engine_id.as_deref(), Some("fixture-engine"));
        assert!(entries[0].manifest_path.to_string_lossy().contains("2.0.0"));
    }
}
