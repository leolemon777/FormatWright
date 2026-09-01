//! Versioned, bounded, and recoverable application-state bundles.

use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::{TempDir, TempPath};
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::domain::ValidationReport;
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::maintenance::{MaintenanceService, RestorePreflightReport};
use crate::preset::PresetLibrary;

pub const APPLICATION_STATE_BUNDLE_SCHEMA_VERSION: u16 = 1;
pub const APPLICATION_SETTINGS_SCHEMA_VERSION: u16 = 1;

const MANIFEST_ENTRY: &str = "manifest.json";
const DATABASE_ENTRY: &str = "database/jobs.sqlite3";
const PRESETS_ENTRY: &str = "presets/presets.json";
const SETTINGS_ENTRY: &str = "settings/settings.json";
const ENGINE_REGISTRY_PREFIX: &str = "engine-registry/";
const REPORTS_PREFIX: &str = "reports/";
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_DATABASE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_PRESETS_BYTES: u64 = 1024 * 1024;
const MAX_SETTINGS_BYTES: u64 = 64 * 1024;
const MAX_REGISTRY_ENTRY_BYTES: u64 = 1024 * 1024;
const MAX_REPORT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MAX_TOTAL_UNCOMPRESSED_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MAX_REGISTRY_ENTRIES: usize = 4096;
const MAX_REPORT_ENTRIES: usize = 100_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationStateLayout {
    pub root_directory: PathBuf,
    pub database_path: PathBuf,
    pub presets_path: PathBuf,
    pub settings_path: PathBuf,
    pub engine_registry_directory: PathBuf,
    pub reports_directory: PathBuf,
}

impl ApplicationStateLayout {
    /// Resolves the complete state layout beside the selected database.
    ///
    /// # Errors
    ///
    /// Returns `InputInvalid` when the database has no usable parent or its
    /// filename overlaps a reserved state component.
    pub fn from_database(database_path: impl Into<PathBuf>) -> Result<Self> {
        let database_path = database_path.into();
        let root_directory = database_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let database_name = database_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| state_error("State database path has no valid filename"))?;
        if matches!(
            database_name.to_ascii_lowercase().as_str(),
            "presets.json" | "settings.json" | "engine-registry" | "reports" | "backups"
        ) {
            return Err(state_error(
                "State database filename overlaps a reserved application-state component",
            ));
        }
        Ok(Self {
            database_path,
            presets_path: root_directory.join("presets.json"),
            settings_path: root_directory.join("settings.json"),
            engine_registry_directory: root_directory.join("engine-registry"),
            reports_directory: root_directory.join("reports"),
            root_directory,
        })
    }

    fn journal_path(&self) -> PathBuf {
        self.root_directory.join(".application-state-restore.json")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationSettings {
    pub schema_version: u16,
    pub language: String,
    pub expert_mode: bool,
}

impl ApplicationSettings {
    /// Validates the portable settings v1 contract.
    ///
    /// # Errors
    ///
    /// Returns `InputInvalid` for unsupported versions or languages.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != APPLICATION_SETTINGS_SCHEMA_VERSION {
            return Err(state_error(
                "Unsupported application-settings schema version",
            ));
        }
        if !matches!(self.language.as_str(), "en" | "zh-CN") {
            return Err(state_error("Application language must be en or zh-CN"));
        }
        Ok(())
    }
}

impl Default for ApplicationSettings {
    fn default() -> Self {
        Self {
            schema_version: APPLICATION_SETTINGS_SCHEMA_VERSION,
            language: "en".to_owned(),
            expert_mode: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationSettingsService {
    path: PathBuf,
}

impl ApplicationSettingsService {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Reads a bounded settings file and recovers a replacement backup.
    ///
    /// # Errors
    ///
    /// Returns a storage or validation error for malformed state.
    pub fn read(&self) -> Result<Option<ApplicationSettings>> {
        recover_file_backup(&self.path)?;
        if !self.path.is_file() {
            return Ok(None);
        }
        let bytes = read_bounded(&self.path, MAX_SETTINGS_BYTES)?;
        let settings = serde_json::from_slice::<ApplicationSettings>(&bytes).map_err(|error| {
            state_error("Stored application settings are invalid")
                .with_diagnostic(error.to_string())
        })?;
        settings.validate()?;
        Ok(Some(settings))
    }

    /// Atomically saves validated settings through a recoverable backup.
    ///
    /// # Errors
    ///
    /// Returns a storage or validation error when the update cannot commit.
    pub fn save(&self, settings: &ApplicationSettings) -> Result<()> {
        settings.validate()?;
        let bytes = serde_json::to_vec_pretty(settings).map_err(|error| {
            state_error("Unable to serialize application settings")
                .with_diagnostic(error.to_string())
        })?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_SETTINGS_BYTES {
            return Err(state_error("Application settings exceed the 64 KiB limit"));
        }
        atomic_replace_file(&self.path, &bytes)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EngineRegistryIdentity {
    #[serde(default)]
    pub engine_id: Option<String>,
    pub manifest_path: PathBuf,
}

impl EngineRegistryIdentity {
    fn validate(&self) -> Result<()> {
        if self.manifest_path.as_os_str().is_empty() {
            return Err(state_error(
                "Engine registry identity has an empty manifest path",
            ));
        }
        if let Some(engine_id) = self.engine_id.as_deref()
            && (engine_id.is_empty()
                || engine_id.len() > 128
                || !engine_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')))
        {
            return Err(state_error(
                "Engine registry identity has an invalid engine ID",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateBundleComponent {
    Database,
    Presets,
    Settings,
    EngineRegistry,
    Reports,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StateBundleEntry {
    pub path: String,
    pub component: StateBundleComponent,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct StateBundleComponents {
    pub database: bool,
    pub presets: bool,
    pub settings: bool,
    pub engine_registry: bool,
    pub reports: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StateBundleManifest {
    pub schema_version: u16,
    pub bundle_id: Uuid,
    pub created_unix_seconds: u64,
    pub application_version: String,
    pub components: StateBundleComponents,
    pub entries: Vec<StateBundleEntry>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StateBundleOptions {
    pub include_reports: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateBundleBackupReport {
    pub bundle_path: PathBuf,
    pub bundle_id: Uuid,
    pub size_bytes: u64,
    pub sha256: String,
    pub entry_count: usize,
    pub reports_included: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateBundlePreflightReport {
    pub bundle_path: PathBuf,
    pub bundle_id: Uuid,
    pub created_unix_seconds: u64,
    pub application_version: String,
    pub entry_count: usize,
    pub total_uncompressed_bytes: u64,
    pub reports_included: bool,
    pub database: RestorePreflightReport,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateBundleRestoreReport {
    pub bundle_path: PathBuf,
    pub bundle_id: Uuid,
    pub database_path: PathBuf,
    pub safety_bundle_path: Option<PathBuf>,
    pub reports_restored: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationStateService {
    layout: ApplicationStateLayout,
}

impl ApplicationStateService {
    #[must_use]
    pub fn new(layout: ApplicationStateLayout) -> Self {
        Self { layout }
    }

    /// Builds a service from the state database and its sibling components.
    ///
    /// # Errors
    ///
    /// Returns `InputInvalid` for an unusable layout.
    pub fn from_database(database_path: impl Into<PathBuf>) -> Result<Self> {
        ApplicationStateLayout::from_database(database_path).map(Self::new)
    }

    #[must_use]
    pub fn layout(&self) -> &ApplicationStateLayout {
        &self.layout
    }

    /// Creates a checked bundle without overwriting an existing destination.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid live state, unsafe files, size limits, or
    /// failed durable publication.
    pub fn backup(
        &self,
        destination: impl AsRef<Path>,
        options: StateBundleOptions,
    ) -> Result<StateBundleBackupReport> {
        self.backup_inner(destination.as_ref(), options)
    }

    /// Validates every archive path/hash/component and performs `SQLite` restore
    /// preflight without changing live state.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, tampered, oversized, or incompatible bundles.
    pub fn restore_preflight(
        &self,
        bundle_path: impl AsRef<Path>,
    ) -> Result<StateBundlePreflightReport> {
        Ok(self.prepare_bundle(bundle_path.as_ref())?.report)
    }

    /// Restores a fully prevalidated bundle with a recovery journal and keeps
    /// a full pre-restore safety bundle.
    ///
    /// # Errors
    ///
    /// Returns an error after attempting rollback when staging or switching fails.
    pub fn restore(&self, bundle_path: impl AsRef<Path>) -> Result<StateBundleRestoreReport> {
        self.recover_interrupted_restore()?;
        let bundle_path = bundle_path.as_ref();
        let prepared = self.prepare_bundle(bundle_path)?;
        self.install_prepared_bundle(bundle_path, prepared)
    }

    /// Rolls an interrupted multi-component restore back to its recorded safety state.
    ///
    /// # Errors
    ///
    /// Returns an error without deleting the journal when safe recovery cannot finish.
    pub fn recover_interrupted_restore(&self) -> Result<bool> {
        let journal_path = self.layout.journal_path();
        recover_file_backup(&journal_path)?;
        if !journal_path.is_file() {
            return Ok(false);
        }
        let bytes = read_bounded(&journal_path, MAX_SETTINGS_BYTES)?;
        let journal = serde_json::from_slice::<RestoreJournal>(&bytes).map_err(|error| {
            state_error("Application-state restore journal is invalid")
                .with_diagnostic(error.to_string())
        })?;
        journal.validate(&self.layout)?;
        if journal.committed {
            self.cleanup_committed_restore(&journal)?;
        } else {
            self.rollback_from_journal(&journal)?;
        }
        fs::remove_file(&journal_path).map_err(state_io_error)?;
        Ok(true)
    }
}

#[derive(Clone, Debug)]
struct StagedEntry {
    archive_path: String,
    component: StateBundleComponent,
    local_path: PathBuf,
    limit: u64,
}

#[derive(Debug)]
struct PreparedBundle {
    _stage: TempDir,
    stage_root: PathBuf,
    manifest: StateBundleManifest,
    report: StateBundlePreflightReport,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum SwapComponent {
    Presets,
    Settings,
    EngineRegistry,
    Reports,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RestoreSwap {
    component: SwapComponent,
    had_live: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RestoreJournal {
    schema_version: u16,
    restore_id: Uuid,
    safety_database_name: String,
    database_had_live: bool,
    database_switch_started: bool,
    committed: bool,
    swaps: Vec<RestoreSwap>,
}

impl ApplicationStateService {
    #[allow(clippy::too_many_lines)]
    fn backup_inner(
        &self,
        destination: &Path,
        options: StateBundleOptions,
    ) -> Result<StateBundleBackupReport> {
        if destination.exists() {
            return Err(state_error(
                "Application-state bundle destination already exists",
            ));
        }
        let stage = tempfile::Builder::new()
            .prefix(".formatwright-state-backup-")
            .tempdir_in(&self.layout.root_directory)
            .map_err(state_io_error)?;
        let portable_database = stage.path().join("jobs.sqlite3");
        MaintenanceService::new(&self.layout.database_path).backup(&portable_database)?;

        let mut staged = vec![StagedEntry {
            archive_path: DATABASE_ENTRY.to_owned(),
            component: StateBundleComponent::Database,
            local_path: portable_database,
            limit: MAX_DATABASE_BYTES,
        }];
        let presets = collect_optional_file(
            &self.layout.presets_path,
            PRESETS_ENTRY,
            StateBundleComponent::Presets,
            MAX_PRESETS_BYTES,
            validate_preset_file,
        )?;
        let settings = collect_optional_file(
            &self.layout.settings_path,
            SETTINGS_ENTRY,
            StateBundleComponent::Settings,
            MAX_SETTINGS_BYTES,
            validate_settings_file,
        )?;
        if let Some(entry) = presets {
            staged.push(entry);
        }
        if let Some(entry) = settings {
            staged.push(entry);
        }
        staged.extend(collect_registry_entries(
            &self.layout.engine_registry_directory,
        )?);
        if options.include_reports {
            staged.extend(collect_report_entries(&self.layout.reports_directory)?);
        }
        staged.sort_by(|left, right| left.archive_path.cmp(&right.archive_path));

        let mut manifest_entries = Vec::with_capacity(staged.len());
        for entry in &staged {
            let size_bytes = fs::metadata(&entry.local_path)
                .map_err(state_io_error)?
                .len();
            if size_bytes > entry.limit {
                return Err(state_error(format!(
                    "Application-state component exceeds its size limit: {}",
                    entry.archive_path
                )));
            }
            manifest_entries.push(StateBundleEntry {
                path: entry.archive_path.clone(),
                component: entry.component,
                size_bytes,
                sha256: sha256_file(&entry.local_path)?,
            });
        }
        let manifest = StateBundleManifest {
            schema_version: APPLICATION_STATE_BUNDLE_SCHEMA_VERSION,
            bundle_id: Uuid::new_v4(),
            created_unix_seconds: current_unix_seconds()?,
            application_version: env!("CARGO_PKG_VERSION").to_owned(),
            components: StateBundleComponents {
                database: true,
                presets: staged
                    .iter()
                    .any(|entry| entry.component == StateBundleComponent::Presets),
                settings: staged
                    .iter()
                    .any(|entry| entry.component == StateBundleComponent::Settings),
                engine_registry: true,
                reports: options.include_reports,
            },
            entries: manifest_entries,
        };
        validate_manifest(&manifest)?;

        if let Some(parent) = destination.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(state_io_error)?;
        }
        let partial = sibling_partial_path(destination, "bundle-partial")?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)
            .map_err(state_io_error)?;
        let write_result = write_bundle(file, &manifest, &staged);
        if let Err(error) = write_result {
            let _ = fs::remove_file(&partial);
            return Err(error);
        }
        if let Err(error) = self.prepare_bundle(&partial) {
            let _ = fs::remove_file(&partial);
            return Err(error);
        }
        let temporary = TempPath::try_from_path(partial).map_err(state_io_error)?;
        temporary
            .persist_noclobber(destination)
            .map_err(|error| state_io_error(error.error))?;
        let size_bytes = fs::metadata(destination).map_err(state_io_error)?.len();
        if size_bytes > MAX_ARCHIVE_BYTES {
            return Err(state_error(
                "Application-state bundle exceeds the 16 GiB limit",
            ));
        }
        Ok(StateBundleBackupReport {
            bundle_path: destination.to_path_buf(),
            bundle_id: manifest.bundle_id,
            size_bytes,
            sha256: sha256_file(destination)?,
            entry_count: manifest.entries.len(),
            reports_included: options.include_reports,
        })
    }

    #[allow(clippy::too_many_lines)]
    fn prepare_bundle(&self, bundle_path: &Path) -> Result<PreparedBundle> {
        let archive_size = fs::metadata(bundle_path).map_err(state_io_error)?.len();
        if archive_size > MAX_ARCHIVE_BYTES {
            return Err(state_error(
                "Application-state bundle exceeds the 16 GiB limit",
            ));
        }
        fs::create_dir_all(&self.layout.root_directory).map_err(state_io_error)?;
        let stage = tempfile::Builder::new()
            .prefix(".formatwright-state-restore-")
            .tempdir_in(&self.layout.root_directory)
            .map_err(state_io_error)?;
        let stage_root = stage.path().to_path_buf();
        let file = File::open(bundle_path).map_err(state_io_error)?;
        let mut archive = ZipArchive::new(file).map_err(state_zip_error)?;
        if archive.len() > MAX_REPORT_ENTRIES + MAX_REGISTRY_ENTRIES + 4 {
            return Err(state_error(
                "Application-state bundle contains too many entries",
            ));
        }

        let mut names = BTreeSet::new();
        let mut manifest_index = None;
        for index in 0..archive.len() {
            let entry = archive.by_index(index).map_err(state_zip_error)?;
            let name = entry.name().to_owned();
            ensure_safe_archive_path(&name)?;
            if !names.insert(name.clone()) {
                return Err(state_error(
                    "Application-state bundle has a duplicate entry",
                ));
            }
            if name == MANIFEST_ENTRY {
                manifest_index = Some(index);
            }
        }
        let manifest_index = manifest_index
            .ok_or_else(|| state_error("Application-state bundle has no manifest.json"))?;
        let manifest_bytes = {
            let mut entry = archive.by_index(manifest_index).map_err(state_zip_error)?;
            read_zip_entry_bounded(&mut entry, MAX_MANIFEST_BYTES)?
        };
        let manifest =
            serde_json::from_slice::<StateBundleManifest>(&manifest_bytes).map_err(|error| {
                state_error("Application-state manifest is invalid")
                    .with_diagnostic(error.to_string())
            })?;
        validate_manifest(&manifest)?;
        let expected = manifest
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .chain(std::iter::once(MANIFEST_ENTRY.to_owned()))
            .collect::<BTreeSet<_>>();
        if names != expected {
            return Err(state_error(
                "Application-state archive entries do not exactly match its manifest",
            ));
        }

        let mut total_uncompressed_bytes = 0_u64;
        for expected_entry in &manifest.entries {
            let mut archive_entry = archive
                .by_name(&expected_entry.path)
                .map_err(state_zip_error)?;
            ensure_regular_zip_entry(&archive_entry)?;
            let limit = component_limit(expected_entry.component);
            if archive_entry.size() != expected_entry.size_bytes || archive_entry.size() > limit {
                return Err(state_error(format!(
                    "Application-state entry size is invalid: {}",
                    expected_entry.path
                )));
            }
            total_uncompressed_bytes = total_uncompressed_bytes
                .checked_add(archive_entry.size())
                .filter(|total| *total <= MAX_TOTAL_UNCOMPRESSED_BYTES)
                .ok_or_else(|| state_error("Application-state bundle expands beyond 16 GiB"))?;
            let output = staged_path(&stage_root, expected_entry)?;
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent).map_err(state_io_error)?;
            }
            let mut writer = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)
                .map_err(state_io_error)?;
            let mut hasher = Sha256::new();
            let mut copied = 0_u64;
            let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
            loop {
                let read = archive_entry.read(&mut buffer).map_err(state_io_error)?;
                if read == 0 {
                    break;
                }
                copied = copied
                    .checked_add(u64::try_from(read).unwrap_or(u64::MAX))
                    .filter(|size| *size <= limit)
                    .ok_or_else(|| state_error("Application-state entry exceeded its limit"))?;
                hasher.update(&buffer[..read]);
                writer.write_all(&buffer[..read]).map_err(state_io_error)?;
            }
            writer.sync_all().map_err(state_io_error)?;
            if copied != expected_entry.size_bytes
                || format!("{:x}", hasher.finalize()) != expected_entry.sha256
            {
                return Err(state_error(format!(
                    "Application-state entry failed its SHA-256 check: {}",
                    expected_entry.path
                )));
            }
        }

        validate_staged_components(&stage_root, &manifest)?;
        let database_path = stage_root.join(DATABASE_ENTRY);
        let database = MaintenanceService::new(&self.layout.database_path)
            .restore_preflight(&database_path)?;
        validate_report_database_links(&stage_root, &manifest)?;
        let mut warnings = Vec::new();
        if manifest.components.engine_registry {
            warnings.push(
                "Engine registry identities are restored, but third-party engine binaries are not bundled; missing paths must be re-imported."
                    .to_owned(),
            );
        }
        if !manifest.components.reports {
            warnings.push(
                "Reports were not included; live reports will be cleared during restore and remain recoverable from the safety bundle."
                    .to_owned(),
            );
        }
        let report = StateBundlePreflightReport {
            bundle_path: bundle_path.to_path_buf(),
            bundle_id: manifest.bundle_id,
            created_unix_seconds: manifest.created_unix_seconds,
            application_version: manifest.application_version.clone(),
            entry_count: manifest.entries.len(),
            total_uncompressed_bytes,
            reports_included: manifest.components.reports,
            database,
            warnings,
        };
        Ok(PreparedBundle {
            _stage: stage,
            stage_root,
            manifest,
            report,
        })
    }

    fn install_prepared_bundle(
        &self,
        bundle_path: &Path,
        prepared: PreparedBundle,
    ) -> Result<StateBundleRestoreReport> {
        let database_had_live = self.layout.database_path.is_file();
        let backup_directory = self.layout.root_directory.join("backups");
        fs::create_dir_all(&backup_directory).map_err(state_io_error)?;
        let restore_id = Uuid::new_v4();
        let safety_bundle_path = if database_had_live {
            let path = backup_directory.join(format!(
                "application-state.pre-restore.{restore_id}.fwstate"
            ));
            self.backup_inner(
                &path,
                StateBundleOptions {
                    include_reports: true,
                },
            )?;
            Some(path)
        } else {
            None
        };
        let safety_database_path = database_had_live.then(|| {
            backup_directory.join(format!(
                "jobs.sqlite3.pre-state-restore.{restore_id}.sqlite3"
            ))
        });
        if let Some(path) = &safety_database_path {
            MaintenanceService::new(&self.layout.database_path).backup(path)?;
        }
        let safety_database_name = safety_database_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        let mut journal = RestoreJournal {
            schema_version: APPLICATION_STATE_BUNDLE_SCHEMA_VERSION,
            restore_id,
            safety_database_name,
            database_had_live,
            database_switch_started: false,
            committed: false,
            swaps: Vec::new(),
        };
        self.save_journal(&journal)?;

        let operation = (|| -> Result<()> {
            self.swap_component(
                &prepared.stage_root,
                SwapComponent::Presets,
                prepared.manifest.components.presets,
                &mut journal,
            )?;
            self.swap_component(
                &prepared.stage_root,
                SwapComponent::Settings,
                prepared.manifest.components.settings,
                &mut journal,
            )?;
            self.swap_component(
                &prepared.stage_root,
                SwapComponent::EngineRegistry,
                prepared.manifest.components.engine_registry,
                &mut journal,
            )?;
            self.swap_component(
                &prepared.stage_root,
                SwapComponent::Reports,
                prepared.manifest.components.reports,
                &mut journal,
            )?;
            journal.database_switch_started = true;
            self.save_journal(&journal)?;
            MaintenanceService::new(&self.layout.database_path)
                .restore(prepared.stage_root.join(DATABASE_ENTRY))?;
            Ok(())
        })();
        if let Err(mut error) = operation {
            if let Err(rollback) = self.rollback_from_journal(&journal) {
                let original = error.diagnostic.take().unwrap_or_default();
                error.diagnostic = Some(format!(
                    "{original}; application-state rollback also failed: {rollback}"
                ));
                return Err(error);
            }
            let _ = fs::remove_file(self.layout.journal_path());
            return Err(error);
        }

        journal.committed = true;
        self.save_journal(&journal)?;
        self.cleanup_committed_restore(&journal)?;
        fs::remove_file(self.layout.journal_path()).map_err(state_io_error)?;
        let journal_backup = recoverable_backup_path(&self.layout.journal_path())?;
        let _ = fs::remove_file(journal_backup);
        Ok(StateBundleRestoreReport {
            bundle_path: bundle_path.to_path_buf(),
            bundle_id: prepared.manifest.bundle_id,
            database_path: self.layout.database_path.clone(),
            safety_bundle_path,
            reports_restored: prepared.manifest.components.reports,
            warnings: prepared.report.warnings,
        })
    }

    fn swap_component(
        &self,
        stage_root: &Path,
        component: SwapComponent,
        included: bool,
        journal: &mut RestoreJournal,
    ) -> Result<()> {
        let live = self.live_component_path(component);
        let staged = Self::staged_component_path(stage_root, component);
        let old = self.old_component_path(component, journal.restore_id);
        let had_live = live.exists();
        journal.swaps.push(RestoreSwap {
            component,
            had_live,
        });
        self.save_journal(journal)?;
        if had_live {
            fs::rename(&live, &old).map_err(state_io_error)?;
        }
        if included && staged.exists() {
            fs::rename(&staged, &live).map_err(state_io_error)?;
        }
        Ok(())
    }

    fn cleanup_committed_restore(&self, journal: &RestoreJournal) -> Result<()> {
        for swap in &journal.swaps {
            remove_path_if_exists(&self.old_component_path(swap.component, journal.restore_id))?;
        }
        if !journal.safety_database_name.is_empty() {
            let safety = self
                .layout
                .root_directory
                .join("backups")
                .join(&journal.safety_database_name);
            if safety.is_file() {
                fs::remove_file(safety).map_err(state_io_error)?;
            }
        }
        Ok(())
    }

    fn save_journal(&self, journal: &RestoreJournal) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(journal).map_err(|error| {
            state_error("Unable to serialize restore journal").with_diagnostic(error.to_string())
        })?;
        atomic_replace_file(&self.layout.journal_path(), &bytes)
    }

    fn rollback_from_journal(&self, journal: &RestoreJournal) -> Result<()> {
        if journal.database_switch_started && journal.database_had_live {
            let safety = self
                .layout
                .root_directory
                .join("backups")
                .join(&journal.safety_database_name);
            MaintenanceService::new(&self.layout.database_path).restore(&safety)?;
        }
        for swap in journal.swaps.iter().rev() {
            let live = self.live_component_path(swap.component);
            let old = self.old_component_path(swap.component, journal.restore_id);
            if old.exists() {
                remove_path_if_exists(&live)?;
                fs::rename(&old, &live).map_err(state_io_error)?;
            } else if !swap.had_live {
                remove_path_if_exists(&live)?;
            }
        }
        let safety = self
            .layout
            .root_directory
            .join("backups")
            .join(&journal.safety_database_name);
        if safety.is_file() {
            fs::remove_file(safety).map_err(state_io_error)?;
        }
        if !journal.database_had_live {
            remove_database_family(&self.layout.database_path)?;
        }
        Ok(())
    }

    fn live_component_path(&self, component: SwapComponent) -> PathBuf {
        match component {
            SwapComponent::Presets => self.layout.presets_path.clone(),
            SwapComponent::Settings => self.layout.settings_path.clone(),
            SwapComponent::EngineRegistry => self.layout.engine_registry_directory.clone(),
            SwapComponent::Reports => self.layout.reports_directory.clone(),
        }
    }

    fn staged_component_path(root: &Path, component: SwapComponent) -> PathBuf {
        match component {
            SwapComponent::Presets => root.join(PRESETS_ENTRY),
            SwapComponent::Settings => root.join(SETTINGS_ENTRY),
            SwapComponent::EngineRegistry => root.join("engine-registry"),
            SwapComponent::Reports => root.join("reports"),
        }
    }

    fn old_component_path(&self, component: SwapComponent, restore_id: Uuid) -> PathBuf {
        let name = match component {
            SwapComponent::Presets => "presets",
            SwapComponent::Settings => "settings",
            SwapComponent::EngineRegistry => "engine-registry",
            SwapComponent::Reports => "reports",
        };
        self.layout
            .root_directory
            .join(format!(".state-restore-{restore_id}-{name}.old"))
    }
}

impl RestoreJournal {
    fn validate(&self, layout: &ApplicationStateLayout) -> Result<()> {
        if self.schema_version != APPLICATION_STATE_BUNDLE_SCHEMA_VERSION
            || (self.database_had_live && self.safety_database_name.is_empty())
            || (!self.safety_database_name.is_empty()
                && Path::new(&self.safety_database_name)
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_))))
        {
            return Err(state_error("Application-state restore journal is unsafe"));
        }
        let mut components = HashSet::new();
        if self
            .swaps
            .iter()
            .any(|swap| !components.insert(swap.component))
        {
            return Err(state_error(
                "Application-state restore journal has duplicate swaps",
            ));
        }
        if !layout.root_directory.exists() {
            return Err(state_error(
                "Application-state restore root no longer exists",
            ));
        }
        Ok(())
    }
}

fn collect_optional_file(
    path: &Path,
    archive_path: &str,
    component: StateBundleComponent,
    limit: u64,
    validate: fn(&Path) -> Result<()>,
) -> Result<Option<StagedEntry>> {
    recover_file_backup(path)?;
    if !path.exists() {
        return Ok(None);
    }
    if !path.is_file() {
        return Err(state_error(format!(
            "Application-state component is not a regular file: {}",
            path.display()
        )));
    }
    validate(path)?;
    Ok(Some(StagedEntry {
        archive_path: archive_path.to_owned(),
        component,
        local_path: path.to_path_buf(),
        limit,
    }))
}

fn collect_registry_entries(directory: &Path) -> Result<Vec<StagedEntry>> {
    recover_json_directory_backups(directory)?;
    collect_json_directory(
        directory,
        ENGINE_REGISTRY_PREFIX,
        StateBundleComponent::EngineRegistry,
        MAX_REGISTRY_ENTRY_BYTES,
        MAX_REGISTRY_ENTRIES,
        |path, file_name| {
            if file_name.starts_with('.') {
                return Err(state_error("Engine registry contains a hidden JSON entry"));
            }
            let bytes = read_bounded(path, MAX_REGISTRY_ENTRY_BYTES)?;
            let identity =
                serde_json::from_slice::<EngineRegistryIdentity>(&bytes).map_err(|error| {
                    state_error("Engine registry identity is invalid")
                        .with_diagnostic(error.to_string())
                })?;
            identity.validate()
        },
    )
}

fn collect_report_entries(directory: &Path) -> Result<Vec<StagedEntry>> {
    recover_json_directory_backups(directory)?;
    collect_json_directory(
        directory,
        REPORTS_PREFIX,
        StateBundleComponent::Reports,
        MAX_REPORT_BYTES,
        MAX_REPORT_ENTRIES,
        |path, file_name| {
            let expected_id = file_name
                .strip_suffix(".json")
                .and_then(|stem| Uuid::parse_str(stem).ok())
                .ok_or_else(|| state_error("Report filename is not a Job UUID"))?;
            let bytes = read_bounded(path, MAX_REPORT_BYTES)?;
            let report = serde_json::from_slice::<ValidationReport>(&bytes).map_err(|error| {
                state_error("Stored ValidationReport is invalid").with_diagnostic(error.to_string())
            })?;
            if report.job_id != expected_id {
                return Err(state_error("Report filename and Job ID do not match"));
            }
            Ok(())
        },
    )
}

fn recover_json_directory_backups(directory: &Path) -> Result<()> {
    if !directory.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(directory).map_err(state_io_error)? {
        let entry = entry.map_err(state_io_error)?;
        let file_name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| state_error("Application-state filename is not valid UTF-8"))?
            .to_owned();
        let Some(stem) = file_name
            .strip_prefix('.')
            .and_then(|name| name.strip_suffix(".backup"))
        else {
            continue;
        };
        if stem.is_empty()
            || stem.len() > 128
            || !stem
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            || is_windows_reserved_stem(stem)
        {
            return Err(state_error(
                "Application-state directory contains an unsafe replacement backup",
            ));
        }
        let destination = directory.join(format!("{stem}.json"));
        if destination.is_file() {
            fs::remove_file(entry.path()).map_err(state_io_error)?;
        } else if !destination.exists() {
            fs::rename(entry.path(), destination).map_err(state_io_error)?;
        } else {
            return Err(state_error(
                "Application-state replacement backup conflicts with a non-file destination",
            ));
        }
    }
    Ok(())
}

fn collect_json_directory<F>(
    directory: &Path,
    archive_prefix: &str,
    component: StateBundleComponent,
    limit: u64,
    maximum_entries: usize,
    mut validate: F,
) -> Result<Vec<StagedEntry>>
where
    F: FnMut(&Path, &str) -> Result<()>,
{
    if !directory.exists() {
        return Ok(Vec::new());
    }
    if !directory.is_dir() {
        return Err(state_error(format!(
            "Application-state component is not a directory: {}",
            directory.display()
        )));
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory).map_err(state_io_error)? {
        let entry = entry.map_err(state_io_error)?;
        let file_type = entry.file_type().map_err(state_io_error)?;
        if !file_type.is_file() {
            return Err(state_error(format!(
                "Application-state directory contains a non-file entry: {}",
                entry.path().display()
            )));
        }
        let file_name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| state_error("Application-state filename is not valid UTF-8"))?
            .to_owned();
        if file_name.starts_with('.') {
            continue;
        }
        if Path::new(&file_name)
            .extension()
            .and_then(|value| value.to_str())
            != Some("json")
        {
            return Err(state_error(format!(
                "Application-state directory contains an unsupported file: {file_name}"
            )));
        }
        validate(&entry.path(), &file_name)?;
        if fs::metadata(entry.path()).map_err(state_io_error)?.len() > limit {
            return Err(state_error(format!(
                "Application-state file exceeds its size limit: {file_name}"
            )));
        }
        entries.push(StagedEntry {
            archive_path: format!("{archive_prefix}{file_name}"),
            component,
            local_path: entry.path(),
            limit,
        });
        if entries.len() > maximum_entries {
            return Err(state_error(
                "Application-state component has too many entries",
            ));
        }
    }
    entries.sort_by(|left, right| left.archive_path.cmp(&right.archive_path));
    Ok(entries)
}

fn validate_preset_file(path: &Path) -> Result<()> {
    let bytes = read_bounded(path, MAX_PRESETS_BYTES)?;
    let library = serde_json::from_slice::<PresetLibrary>(&bytes).map_err(|error| {
        state_error("Preset library is invalid").with_diagnostic(error.to_string())
    })?;
    library.validate()
}

fn validate_settings_file(path: &Path) -> Result<()> {
    let bytes = read_bounded(path, MAX_SETTINGS_BYTES)?;
    let settings = serde_json::from_slice::<ApplicationSettings>(&bytes).map_err(|error| {
        state_error("Application settings are invalid").with_diagnostic(error.to_string())
    })?;
    settings.validate()
}

fn validate_staged_components(root: &Path, manifest: &StateBundleManifest) -> Result<()> {
    if manifest.components.presets {
        validate_preset_file(&root.join(PRESETS_ENTRY))?;
    }
    if manifest.components.settings {
        validate_settings_file(&root.join(SETTINGS_ENTRY))?;
    }
    for entry in &manifest.entries {
        match entry.component {
            StateBundleComponent::EngineRegistry => {
                let bytes = read_bounded(&root.join(&entry.path), MAX_REGISTRY_ENTRY_BYTES)?;
                let identity =
                    serde_json::from_slice::<EngineRegistryIdentity>(&bytes).map_err(|error| {
                        state_error("Bundled engine registry identity is invalid")
                            .with_diagnostic(error.to_string())
                    })?;
                identity.validate()?;
            }
            StateBundleComponent::Reports => {
                let file_name = Path::new(&entry.path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| state_error("Bundled report filename is invalid"))?;
                let expected_id = file_name
                    .strip_suffix(".json")
                    .and_then(|stem| Uuid::parse_str(stem).ok())
                    .ok_or_else(|| state_error("Bundled report filename is not a Job UUID"))?;
                let bytes = read_bounded(&root.join(&entry.path), MAX_REPORT_BYTES)?;
                let report =
                    serde_json::from_slice::<ValidationReport>(&bytes).map_err(|error| {
                        state_error("Bundled ValidationReport is invalid")
                            .with_diagnostic(error.to_string())
                    })?;
                if report.job_id != expected_id {
                    return Err(state_error(
                        "Bundled report filename and Job ID do not match",
                    ));
                }
            }
            StateBundleComponent::Database
            | StateBundleComponent::Presets
            | StateBundleComponent::Settings => {}
        }
    }
    Ok(())
}

fn validate_report_database_links(root: &Path, manifest: &StateBundleManifest) -> Result<()> {
    if !manifest.components.reports {
        return Ok(());
    }
    let database = Connection::open_with_flags(
        root.join(DATABASE_ENTRY),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        state_error("Unable to inspect bundled report references")
            .with_diagnostic(error.to_string())
    })?;
    for entry in manifest
        .entries
        .iter()
        .filter(|entry| entry.component == StateBundleComponent::Reports)
    {
        let bytes = read_bounded(&root.join(&entry.path), MAX_REPORT_BYTES)?;
        let report = serde_json::from_slice::<ValidationReport>(&bytes).map_err(|error| {
            state_error("Bundled ValidationReport is invalid").with_diagnostic(error.to_string())
        })?;
        let stored = database
            .query_row(
                "SELECT plan_hash, state FROM jobs WHERE id = ?1",
                [report.job_id.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|error| {
                state_error("Unable to inspect bundled report references")
                    .with_diagnostic(error.to_string())
            })?;
        let expected_state = match report.status {
            crate::domain::ValidationStatus::Pass => "completed",
            crate::domain::ValidationStatus::Warning | crate::domain::ValidationStatus::Unknown => {
                "warning"
            }
            crate::domain::ValidationStatus::Fail => "failed",
        };
        if stored
            .as_ref()
            .map(|(plan_hash, state)| (plan_hash.as_str(), state.as_str()))
            != Some((report.plan_hash.as_str(), expected_state))
        {
            return Err(state_error(
                "Bundled ValidationReport has no matching terminal Job and Plan in SQLite",
            ));
        }
    }
    Ok(())
}

fn validate_manifest(manifest: &StateBundleManifest) -> Result<()> {
    if manifest.schema_version != APPLICATION_STATE_BUNDLE_SCHEMA_VERSION {
        return Err(state_error(
            "Unsupported application-state bundle schema version",
        ));
    }
    if manifest.application_version.is_empty() || manifest.application_version.len() > 64 {
        return Err(state_error(
            "Application-state manifest has an invalid version",
        ));
    }
    if !manifest.components.database {
        return Err(state_error(
            "Application-state bundle must contain its database",
        ));
    }
    if manifest.entries.len() > MAX_REPORT_ENTRIES + MAX_REGISTRY_ENTRIES + 3 {
        return Err(state_error(
            "Application-state manifest contains too many entries",
        ));
    }
    let mut paths = BTreeSet::new();
    let mut database_count = 0_usize;
    let mut presets_count = 0_usize;
    let mut settings_count = 0_usize;
    let mut registry_count = 0_usize;
    let mut reports_count = 0_usize;
    for entry in &manifest.entries {
        ensure_safe_archive_path(&entry.path)?;
        if entry.path == MANIFEST_ENTRY || !paths.insert(entry.path.clone()) {
            return Err(state_error(
                "Application-state manifest has duplicate entries",
            ));
        }
        if entry.size_bytes > component_limit(entry.component)
            || entry.sha256.len() != 64
            || !entry.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(state_error(
                "Application-state manifest has invalid entry metadata",
            ));
        }
        match entry.component {
            StateBundleComponent::Database => {
                database_count += 1;
                if entry.path != DATABASE_ENTRY {
                    return Err(state_error("Database entry has an unexpected archive path"));
                }
            }
            StateBundleComponent::Presets => {
                presets_count += 1;
                if entry.path != PRESETS_ENTRY {
                    return Err(state_error("Preset entry has an unexpected archive path"));
                }
            }
            StateBundleComponent::Settings => {
                settings_count += 1;
                if entry.path != SETTINGS_ENTRY {
                    return Err(state_error("Settings entry has an unexpected archive path"));
                }
            }
            StateBundleComponent::EngineRegistry => {
                registry_count += 1;
                ensure_prefixed_json_path(&entry.path, ENGINE_REGISTRY_PREFIX)?;
            }
            StateBundleComponent::Reports => {
                reports_count += 1;
                ensure_prefixed_json_path(&entry.path, REPORTS_PREFIX)?;
            }
        }
    }
    if database_count != 1
        || presets_count != usize::from(manifest.components.presets)
        || settings_count != usize::from(manifest.components.settings)
        || registry_count > MAX_REGISTRY_ENTRIES
        || reports_count > MAX_REPORT_ENTRIES
        || (!manifest.components.engine_registry && registry_count != 0)
        || (!manifest.components.reports && reports_count != 0)
    {
        return Err(state_error(
            "Application-state component flags do not match entries",
        ));
    }
    Ok(())
}

fn ensure_prefixed_json_path(path: &str, prefix: &str) -> Result<()> {
    let suffix = path
        .strip_prefix(prefix)
        .filter(|suffix| {
            Path::new(suffix)
                .extension()
                .and_then(|value| value.to_str())
                == Some("json")
                && !suffix.starts_with('.')
        })
        .ok_or_else(|| state_error("Application-state component path is invalid"))?;
    if suffix.contains('/') || suffix.contains('\\') {
        return Err(state_error(
            "Application-state component path is nested unexpectedly",
        ));
    }
    let stem = suffix
        .strip_suffix(".json")
        .ok_or_else(|| state_error("Application-state component filename is invalid"))?;
    if stem.is_empty()
        || stem.len() > 128
        || !stem
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        || stem.ends_with('.')
        || stem.ends_with(' ')
        || is_windows_reserved_stem(stem)
    {
        return Err(state_error(
            "Application-state component filename is not portable",
        ));
    }
    Ok(())
}

fn is_windows_reserved_stem(stem: &str) -> bool {
    let base = stem
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || base
            .strip_prefix("COM")
            .or_else(|| base.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                suffix.len() == 1 && matches!(suffix.as_bytes().first(), Some(b'1'..=b'9'))
            })
}

fn staged_path(root: &Path, entry: &StateBundleEntry) -> Result<PathBuf> {
    ensure_safe_archive_path(&entry.path)?;
    Ok(root.join(entry.path.replace('/', std::path::MAIN_SEPARATOR_STR)))
}

fn ensure_safe_archive_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.contains('\\')
        || path.starts_with('/')
        || path.ends_with('/')
        || Path::new(path)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(state_error(
            "Application-state archive contains an unsafe path",
        ));
    }
    Ok(())
}

fn ensure_regular_zip_entry<R: Read>(entry: &zip::read::ZipFile<'_, R>) -> Result<()> {
    if entry.is_dir()
        || entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170_000 != 0o100_000)
    {
        return Err(state_error(
            "Application-state archive contains a non-regular entry",
        ));
    }
    Ok(())
}

fn component_limit(component: StateBundleComponent) -> u64 {
    match component {
        StateBundleComponent::Database => MAX_DATABASE_BYTES,
        StateBundleComponent::Presets => MAX_PRESETS_BYTES,
        StateBundleComponent::Settings => MAX_SETTINGS_BYTES,
        StateBundleComponent::EngineRegistry => MAX_REGISTRY_ENTRY_BYTES,
        StateBundleComponent::Reports => MAX_REPORT_BYTES,
    }
}

fn write_bundle(
    writer: File,
    manifest: &StateBundleManifest,
    entries: &[StagedEntry],
) -> Result<()> {
    let mut archive = ZipWriter::new(writer);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o600);
    let manifest_bytes = serde_json::to_vec_pretty(manifest).map_err(|error| {
        state_error("Unable to serialize application-state manifest")
            .with_diagnostic(error.to_string())
    })?;
    if u64::try_from(manifest_bytes.len()).unwrap_or(u64::MAX) > MAX_MANIFEST_BYTES {
        return Err(state_error("Application-state manifest exceeds 4 MiB"));
    }
    archive
        .start_file(MANIFEST_ENTRY, options)
        .map_err(state_zip_error)?;
    archive.write_all(&manifest_bytes).map_err(state_io_error)?;
    for entry in entries {
        archive
            .start_file(&entry.archive_path, options)
            .map_err(state_zip_error)?;
        let mut source = File::open(&entry.local_path).map_err(state_io_error)?;
        let mut copied = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
        loop {
            let read = source.read(&mut buffer).map_err(state_io_error)?;
            if read == 0 {
                break;
            }
            copied = copied
                .checked_add(u64::try_from(read).unwrap_or(u64::MAX))
                .filter(|size| *size <= entry.limit)
                .ok_or_else(|| state_error("Application-state source grew beyond its limit"))?;
            archive.write_all(&buffer[..read]).map_err(state_io_error)?;
        }
    }
    let mut writer = archive.finish().map_err(state_zip_error)?;
    writer.flush().map_err(state_io_error)?;
    writer.sync_all().map_err(state_io_error)
}

fn read_zip_entry_bounded<R: Read>(reader: &mut R, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(state_io_error)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(state_error(
            "Application-state archive entry exceeds its limit",
        ));
    }
    Ok(bytes)
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let metadata = fs::metadata(path).map_err(state_io_error)?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(state_error(format!(
            "Application-state file is not regular or exceeds its size limit: {}",
            path.display()
        )));
    }
    fs::read(path).map_err(state_io_error)
}

fn atomic_replace_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(state_io_error)?;
    }
    recover_file_backup(path)?;
    let partial = sibling_partial_path(path, "partial")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)
        .map_err(state_io_error)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&partial);
        return Err(state_io_error(error));
    }
    let backup = recoverable_backup_path(path)?;
    if path.is_file() {
        if backup.exists() {
            fs::remove_file(&backup).map_err(state_io_error)?;
        }
        fs::rename(path, &backup).map_err(state_io_error)?;
    }
    let temporary = TempPath::try_from_path(partial).map_err(state_io_error)?;
    if let Err(error) = temporary.persist_noclobber(path) {
        if backup.is_file() && !path.exists() {
            let _ = fs::rename(&backup, path);
        }
        return Err(state_io_error(error.error));
    }
    if backup.is_file() {
        fs::remove_file(backup).map_err(state_io_error)?;
    }
    Ok(())
}

fn recover_file_backup(path: &Path) -> Result<()> {
    let backup = recoverable_backup_path(path)?;
    if !path.exists() && backup.is_file() {
        fs::rename(backup, path).map_err(state_io_error)?;
    } else if path.is_file() && backup.is_file() {
        fs::remove_file(backup).map_err(state_io_error)?;
    }
    Ok(())
}

fn recoverable_backup_path(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| state_error("Application-state path has no valid filename"))?;
    Ok(path.with_file_name(format!(".{name}.backup")))
}

fn sibling_partial_path(path: &Path, label: &str) -> Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| state_error("Application-state destination has no valid filename"))?;
    Ok(path.with_file_name(format!(".{name}.{label}.{}.partial", Uuid::new_v4())))
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(state_io_error)?;
    } else if path.exists() {
        fs::remove_file(path).map_err(state_io_error)?;
    }
    Ok(())
}

fn remove_database_family(path: &Path) -> Result<()> {
    for candidate in [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ] {
        if candidate.exists() {
            fs::remove_file(candidate).map_err(state_io_error)?;
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(state_io_error)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = file.read(&mut buffer).map_err(state_io_error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn current_unix_seconds() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| {
            state_error("System clock is before the Unix epoch").with_diagnostic(error.to_string())
        })
}

fn state_error(message: impl Into<String>) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::StorageFailed,
        Stage::Store,
        message,
        "Keep live state unchanged, verify the bundle and free disk space, then retry.",
    )
}

#[allow(clippy::needless_pass_by_value)]
fn state_io_error(error: std::io::Error) -> FormatWrightError {
    state_error("Unable to read or persist application state").with_diagnostic(error.to_string())
}

#[allow(clippy::needless_pass_by_value)]
fn state_zip_error(error: zip::result::ZipError) -> FormatWrightError {
    state_error("Application-state bundle is not a valid supported ZIP archive")
        .with_diagnostic(error.to_string())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::domain::{
        ArtifactSummary, ChangeSet, NetworkPolicy, Plan, ReportRedaction, ValidationStatus,
    };
    use crate::job_store::SqliteJobStore;
    use crate::preset::{ConversionPreset, PRESET_SCHEMA_VERSION};

    fn seed_layout(root: &Path) -> ApplicationStateLayout {
        let layout =
            ApplicationStateLayout::from_database(root.join("jobs.sqlite3")).expect("state layout");
        let output = root.join("seed-output.yaml");
        let mut plan = Plan {
            schema_version: 1,
            plan_id: Uuid::new_v4(),
            plan_hash: String::new(),
            input_fingerprint: "fwfp-v1:state-bundle-test".to_owned(),
            target_format: "yaml".to_owned(),
            constraints: std::collections::BTreeMap::new(),
            steps: Vec::new(),
            changes: ChangeSet::default(),
            validators: Vec::new(),
            network_policy: NetworkPolicy::Deny,
            output_path: Some(output.clone()),
            estimated_output_bytes: None,
        };
        plan.plan_hash = crate::planner::deterministic_plan_hash(&plan).expect("durable hash");
        let mut store = SqliteJobStore::open(&layout.database_path).expect("database");
        let seeded = store
            .create_job(root.join("seed-input.json"), output, &plan)
            .expect("seed durable job");
        store
            .transition(seeded.id, crate::domain::JobState::Running, "TEST_RUNNING")
            .expect("seed running");
        store
            .transition(
                seeded.id,
                crate::domain::JobState::Validating,
                "TEST_VALIDATING",
            )
            .expect("seed validating");
        store
            .transition(
                seeded.id,
                crate::domain::JobState::Completed,
                "TEST_COMPLETED",
            )
            .expect("seed completed");
        fs::create_dir_all(&layout.engine_registry_directory).expect("registry");
        fs::create_dir_all(&layout.reports_directory).expect("reports");
        let preset = ConversionPreset {
            schema_version: PRESET_SCHEMA_VERSION,
            preset_id: Uuid::new_v4(),
            name: "Portable".to_owned(),
            target_format: "yaml".to_owned(),
            quality: None,
            width: None,
            dpi: None,
            color_mode: None,
            preserve_all_streams: true,
            video_crf: None,
            video_preset: None,
            audio_bitrate_kbps: None,
        };
        let library = PresetLibrary {
            schema_version: PRESET_SCHEMA_VERSION,
            presets: vec![preset],
        };
        fs::write(
            &layout.presets_path,
            serde_json::to_vec_pretty(&library).expect("preset bytes"),
        )
        .expect("presets");
        ApplicationSettingsService::new(&layout.settings_path)
            .save(&ApplicationSettings {
                schema_version: APPLICATION_SETTINGS_SCHEMA_VERSION,
                language: "zh-CN".to_owned(),
                expert_mode: true,
            })
            .expect("settings");
        fs::write(
            layout.engine_registry_directory.join("media.json"),
            serde_json::to_vec_pretty(&EngineRegistryIdentity {
                engine_id: Some("media".to_owned()),
                manifest_path: root.join("missing-engine/manifest.json"),
            })
            .expect("registry bytes"),
        )
        .expect("registry identity");
        layout
    }

    fn report(job_id: Uuid, plan_hash: &str) -> ValidationReport {
        let artifact = ArtifactSummary {
            display_path: None,
            format_id: "yaml".to_owned(),
            size_bytes: 1,
            fast_fingerprint: "fwfp-v1:state-bundle-test".to_owned(),
            full_blake3: None,
        };
        ValidationReport {
            schema_version: 1,
            report_id: Uuid::new_v4(),
            job_id,
            plan_hash: plan_hash.to_owned(),
            status: ValidationStatus::Pass,
            input: artifact.clone(),
            output: artifact,
            engines: Vec::new(),
            checks: Vec::new(),
            intentional_changes: Vec::new(),
            redaction: ReportRedaction {
                paths_redacted: true,
                metadata_values_redacted: true,
            },
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn full_bundle_round_trip_restores_every_selected_component() {
        let directory = tempdir().expect("state suite");
        let layout = seed_layout(directory.path());
        let existing_job = SqliteJobStore::open(&layout.database_path)
            .expect("report store")
            .list_jobs(1)
            .expect("report jobs")
            .remove(0);
        fs::write(
            layout
                .reports_directory
                .join(format!("{}.json", existing_job.id)),
            serde_json::to_vec_pretty(&report(existing_job.id, &existing_job.plan_hash))
                .expect("report bytes"),
        )
        .expect("report");
        let service = ApplicationStateService::new(layout.clone());
        let bundle = directory.path().join("portable.fwstate");
        let active_report = layout
            .reports_directory
            .join(format!("{}.json", existing_job.id));
        let interrupted_report_backup = layout
            .reports_directory
            .join(format!(".{}.backup", existing_job.id));
        fs::rename(&active_report, &interrupted_report_backup)
            .expect("simulate report replacement interruption");
        let backup = service
            .backup(
                &bundle,
                StateBundleOptions {
                    include_reports: true,
                },
            )
            .expect("bundle backup");
        assert!(active_report.is_file());
        assert!(!interrupted_report_backup.exists());
        assert!(backup.entry_count >= 5);
        let preflight = service.restore_preflight(&bundle).expect("preflight");
        assert!(preflight.database.integrity.ok);

        let mut live = SqliteJobStore::open(&layout.database_path).expect("live store");
        live.create_job(
            directory.path().join("later-input.json"),
            directory.path().join("later-output.yaml"),
            &{
                let mut plan = live
                    .list_jobs(1)
                    .expect("seed jobs")
                    .first()
                    .and_then(|job| live.get_job_details(job.id).ok().flatten())
                    .expect("seed details")
                    .plan;
                plan.output_path = Some(directory.path().join("later-output.yaml"));
                plan.plan_hash =
                    crate::planner::deterministic_plan_hash(&plan).expect("later durable hash");
                plan
            },
        )
        .expect("later job");
        drop(live);
        assert_eq!(
            MaintenanceService::new(&layout.database_path)
                .status()
                .expect("mutated status")
                .job_count,
            2
        );

        fs::write(
            &layout.presets_path,
            serde_json::to_vec_pretty(&PresetLibrary::empty()).expect("empty preset bytes"),
        )
        .expect("mutate presets");
        ApplicationSettingsService::new(&layout.settings_path)
            .save(&ApplicationSettings::default())
            .expect("mutate settings");
        fs::remove_dir_all(&layout.engine_registry_directory).expect("remove registry");
        fs::remove_dir_all(&layout.reports_directory).expect("remove reports");
        let restored = service.restore(&bundle).expect("restore bundle");

        assert!(
            restored
                .safety_bundle_path
                .as_deref()
                .is_some_and(Path::is_file)
        );
        assert_eq!(
            MaintenanceService::new(&layout.database_path)
                .status()
                .expect("restored status")
                .job_count,
            1
        );
        assert_eq!(
            ApplicationSettingsService::new(&layout.settings_path)
                .read()
                .expect("settings")
                .expect("stored settings")
                .language,
            "zh-CN"
        );
        validate_preset_file(&layout.presets_path).expect("restored presets");
        assert!(
            layout
                .engine_registry_directory
                .join("media.json")
                .is_file()
        );
        assert!(
            layout
                .reports_directory
                .join(format!("{}.json", existing_job.id))
                .is_file()
        );
    }

    #[test]
    fn restore_preflight_rejects_a_manifest_hash_mismatch() {
        let directory = tempdir().expect("state suite");
        let layout = seed_layout(directory.path());
        let service = ApplicationStateService::new(layout);
        let valid = directory.path().join("valid.fwstate");
        service
            .backup(&valid, StateBundleOptions::default())
            .expect("valid backup");
        let tampered = directory.path().join("tampered.fwstate");
        tamper_first_component(&valid, &tampered);

        service
            .restore_preflight(&tampered)
            .expect_err("tampered component must fail");
    }

    #[test]
    fn backup_rejects_a_report_without_a_matching_database_job() {
        let directory = tempdir().expect("state suite");
        let layout = seed_layout(directory.path());
        let orphan_id = Uuid::new_v4();
        fs::write(
            layout.reports_directory.join(format!("{orphan_id}.json")),
            serde_json::to_vec_pretty(&report(orphan_id, "blake3:orphan"))
                .expect("orphan report bytes"),
        )
        .expect("orphan report");
        let destination = directory.path().join("must-not-exist.fwstate");

        ApplicationStateService::new(layout)
            .backup(
                &destination,
                StateBundleOptions {
                    include_reports: true,
                },
            )
            .expect_err("orphan report must fail bundle self-check");

        assert!(!destination.exists());
    }

    #[test]
    fn restore_preflight_rejects_unsafe_archive_paths() {
        let directory = tempdir().expect("state suite");
        let layout = seed_layout(directory.path());
        let database_before = MaintenanceService::new(&layout.database_path)
            .status()
            .expect("status before");
        let unsafe_bundle = directory.path().join("unsafe.fwstate");
        let writer = File::create(&unsafe_bundle).expect("unsafe archive");
        let mut archive = ZipWriter::new(writer);
        archive
            .start_file("../escape.json", SimpleFileOptions::default())
            .expect("unsafe member");
        archive.write_all(b"{}").expect("unsafe bytes");
        archive.finish().expect("finish unsafe archive");

        ApplicationStateService::new(layout.clone())
            .restore_preflight(&unsafe_bundle)
            .expect_err("traversal path must be rejected");

        assert_eq!(
            MaintenanceService::new(&layout.database_path)
                .status()
                .expect("status after")
                .job_count,
            database_before.job_count
        );
        assert!(!directory.path().join("escape.json").exists());
    }

    #[test]
    fn interrupted_restore_journal_recovers_old_files() {
        let directory = tempdir().expect("state suite");
        let layout = seed_layout(directory.path());
        let service = ApplicationStateService::new(layout.clone());
        let restore_id = Uuid::new_v4();
        let old = service.old_component_path(SwapComponent::Settings, restore_id);
        fs::rename(&layout.settings_path, &old).expect("stage old settings");
        ApplicationSettingsService::new(&layout.settings_path)
            .save(&ApplicationSettings::default())
            .expect("new settings");
        let safety_database_name = "unused-safety.sqlite3".to_owned();
        service
            .save_journal(&RestoreJournal {
                schema_version: APPLICATION_STATE_BUNDLE_SCHEMA_VERSION,
                restore_id,
                safety_database_name,
                database_had_live: true,
                database_switch_started: false,
                committed: false,
                swaps: vec![RestoreSwap {
                    component: SwapComponent::Settings,
                    had_live: true,
                }],
            })
            .expect("journal");

        assert!(service.recover_interrupted_restore().expect("recover"));
        assert_eq!(
            ApplicationSettingsService::new(&layout.settings_path)
                .read()
                .expect("read")
                .expect("settings")
                .language,
            "zh-CN"
        );
        assert!(!layout.journal_path().exists());
    }

    #[test]
    fn committed_restore_journal_finishes_cleanup_without_rollback() {
        let directory = tempdir().expect("state suite");
        let layout = seed_layout(directory.path());
        let service = ApplicationStateService::new(layout.clone());
        let restore_id = Uuid::new_v4();
        let old = service.old_component_path(SwapComponent::Settings, restore_id);
        fs::copy(&layout.settings_path, &old).expect("orphan old settings");
        let safety_name = "committed-safety.sqlite3";
        let safety_path = layout.root_directory.join("backups").join(safety_name);
        fs::create_dir_all(safety_path.parent().expect("backup parent")).expect("backups");
        fs::write(&safety_path, b"obsolete-safety").expect("safety fixture");
        ApplicationSettingsService::new(&layout.settings_path)
            .save(&ApplicationSettings::default())
            .expect("committed settings");
        service
            .save_journal(&RestoreJournal {
                schema_version: APPLICATION_STATE_BUNDLE_SCHEMA_VERSION,
                restore_id,
                safety_database_name: safety_name.to_owned(),
                database_had_live: true,
                database_switch_started: true,
                committed: true,
                swaps: vec![RestoreSwap {
                    component: SwapComponent::Settings,
                    had_live: true,
                }],
            })
            .expect("committed journal");

        assert!(
            service
                .recover_interrupted_restore()
                .expect("finish cleanup")
        );
        assert!(!old.exists());
        assert!(!safety_path.exists());
        assert_eq!(
            ApplicationSettingsService::new(&layout.settings_path)
                .read()
                .expect("settings")
                .expect("current settings"),
            ApplicationSettings::default()
        );
    }

    #[test]
    fn bundle_restores_onto_a_new_machine_without_existing_state() {
        let directory = tempdir().expect("state suite");
        let source_root = directory.path().join("source");
        fs::create_dir_all(&source_root).expect("source root");
        let source = seed_layout(&source_root);
        let bundle = directory.path().join("portable.fwstate");
        ApplicationStateService::new(source)
            .backup(&bundle, StateBundleOptions::default())
            .expect("source backup");

        let destination_root = directory.path().join("destination");
        fs::create_dir_all(&destination_root).expect("destination root");
        let destination =
            ApplicationStateLayout::from_database(destination_root.join("jobs.sqlite3"))
                .expect("destination layout");
        let restored = ApplicationStateService::new(destination.clone())
            .restore(&bundle)
            .expect("restore new machine");

        assert!(restored.safety_bundle_path.is_none());
        assert_eq!(
            MaintenanceService::new(&destination.database_path)
                .status()
                .expect("destination status")
                .job_count,
            1
        );
        validate_preset_file(&destination.presets_path).expect("destination presets");
    }

    fn tamper_first_component(source: &Path, destination: &Path) {
        let reader = File::open(source).expect("open source");
        let mut archive = ZipArchive::new(reader).expect("source archive");
        let writer = File::create(destination).expect("create tampered");
        let mut output = ZipWriter::new(writer);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let mut tampered = false;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).expect("source entry");
            let name = entry.name().to_owned();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).expect("read entry");
            if name != MANIFEST_ENTRY && !tampered {
                bytes[0] ^= 1;
                tampered = true;
            }
            output.start_file(name, options).expect("copy entry");
            output.write_all(&bytes).expect("write entry");
        }
        output.finish().expect("finish tampered");
    }
}
