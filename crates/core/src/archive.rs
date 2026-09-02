use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
use formatwright_engine_sdk::{EngineIdentity, LossClass, Operation};
use serde_json::{Value, json};
use uuid::Uuid;
use zip::ZipArchive;

use crate::domain::{
    ArtifactSummary, ChangeSet, FormatDescriptor, FormatKind, NetworkPolicy, Plan, PlanRequest,
    PlanStep, Probe, ProbeEvidence, ReportRedaction, SCHEMA_VERSION, StreamKind, StreamProbe,
    ValidationCheck, ValidationReport, ValidationStatus,
};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::fingerprint::identify_artifact;
use crate::planner::deterministic_plan_hash;

const MAX_ARCHIVE_ENTRIES: usize = 10_000;
const MAX_ARCHIVE_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Returns the archive family for a path whose extension names an archive.
///
/// The extension leads because DOCX/EPUB packages share the ZIP prefix; the
/// magic bytes are then re-checked so a renamed non-archive fails loudly.
///
/// # Errors
///
/// Returns an input error when the bytes do not match the extension's family.
pub fn archive_format_hint(path: &Path) -> Result<Option<&'static str>> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    // file_name is lowercased above, so the comparisons are case-insensitive.
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    let is_targz = matches!(extension.as_deref(), Some("tgz"))
        || file_name.ends_with(".tar.gz")
        || file_name.ends_with(".taz");
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    let is_zip = extension.as_deref() == Some("zip");
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    let is_7z = extension.as_deref() == Some("7z");
    if !is_targz && !is_zip && !is_7z {
        return Ok(None);
    }
    let mut prefix = [0_u8; 6];
    let read = File::open(path)
        .and_then(|mut file| file.read(&mut prefix))
        .map_err(|error| input_error(path, error))?;
    if read < prefix.len() {
        return Err(input_error(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "archive header is truncated",
            ),
        ));
    }
    if is_zip {
        if prefix[..4] == *b"PK\x03\x04" {
            Ok(Some("zip"))
        } else {
            Err(input_error(
                path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "zip extension does not carry a ZIP prefix",
                ),
            ))
        }
    } else if is_7z {
        if prefix == [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C] {
            Ok(Some("7z"))
        } else {
            Err(input_error(
                path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "7z extension does not carry a 7z signature header",
                ),
            ))
        }
    } else if prefix[..2] == *b"\x1f\x8b" {
        Ok(Some("tar.gz"))
    } else {
        Err(input_error(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "tar.gz extension does not carry a gzip prefix",
            ),
        ))
    }
}

/// Entry inventory shared by inspection, execution, and validation.
#[derive(Debug)]
pub(crate) struct ArchiveEntry {
    pub name: String,
    pub size: u64,
}

impl ArchiveEntry {
    fn safe_name(&self) -> Result<&str> {
        let candidate = Path::new(&self.name);
        if self.name.starts_with('/')
            || self.name.starts_with('\\')
            || self.name.contains("..")
            || candidate.is_absolute()
            || self.name.contains(':')
        {
            return Err(FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Inspect,
                format!("Archive entry has an unsafe path: {}", self.name),
                "Rebuild the archive without absolute or traversal paths.",
            ));
        }
        Ok(&self.name)
    }
}

pub(crate) fn read_zip_entries(path: &Path) -> Result<Vec<ArchiveEntry>> {
    let file = File::open(path).map_err(|error| input_error(path, error))?;
    let mut archive = ZipArchive::new(file).map_err(|error| input_error(path, error))?;
    enforce_archive_limits(
        archive.len(),
        archive.decompressed_size().unwrap_or(u128::MAX),
    )?;
    let mut entries = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive
            .by_index_raw(index)
            .map_err(|error| input_error(path, error))?;
        let record = ArchiveEntry {
            name: entry.name().to_owned(),
            size: entry.size(),
        };
        record.safe_name()?;
        entries.push(record);
    }
    Ok(entries)
}

pub(crate) fn read_targz_entries(path: &Path) -> Result<Vec<ArchiveEntry>> {
    let file = File::open(path).map_err(|error| input_error(path, error))?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let mut entries = Vec::new();
    let mut total: u64 = 0;
    for entry in archive
        .entries()
        .map_err(|error| input_error(path, error))?
    {
        let entry = entry.map_err(|error| input_error(path, error))?;
        let size = entry
            .header()
            .entry_size()
            .map_err(|error| input_error(path, error))?;
        let name = entry
            .path()
            .map_err(|error| input_error(path, error))?
            .to_string_lossy()
            .replace('\\', "/");
        let record = ArchiveEntry { name, size };
        record.safe_name()?;
        total = total.saturating_add(size);
        entries.push(record);
    }
    enforce_archive_limits(entries.len(), u128::from(total))?;
    Ok(entries)
}

/// Counts copied bytes while discarding them, so 7z entry inventory can be
/// taken without buffering payloads.
struct CountingDiscard {
    bytes: u64,
}

impl std::io::Write for CountingDiscard {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buffer.len() as u64);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn read_7z_entries(path: &Path) -> Result<Vec<ArchiveEntry>> {
    let mut reader = sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty())
        .map_err(|error| input_error(path, error))?;
    let mut entries = Vec::new();
    let mut total: u64 = 0;
    let mut rejected: Option<FormatWrightError> = None;
    reader
        .for_each_entries(|entry, data| {
            let mut name = entry.name().replace('\\', "/");
            if entry.is_directory() && !name.ends_with('/') {
                // ZIP inventory names directories with a trailing slash; keep
                // the manifest comparable across containers.
                name.push('/');
            }
            let size = if entry.has_stream() {
                let mut sink = CountingDiscard { bytes: 0 };
                std::io::copy(data, &mut sink).map_err(sevenz_rust::Error::io)?;
                sink.bytes
            } else {
                0
            };
            let record = ArchiveEntry { name, size };
            if let Err(error) = record.safe_name() {
                rejected.get_or_insert(error);
            }
            total = total.saturating_add(size);
            entries.push(record);
            // Returning false continues the iteration; every entry must be
            // drained so the reader stays positioned.
            Ok(rejected.is_none())
        })
        .map_err(|error| input_error(path, error))?;
    if let Some(error) = rejected {
        return Err(error);
    }
    enforce_archive_limits(entries.len(), u128::from(total))?;
    Ok(entries)
}

fn enforce_archive_limits(entry_count: usize, total_bytes: u128) -> Result<()> {
    if entry_count > MAX_ARCHIVE_ENTRIES {
        return Err(FormatWrightError::new(
            ErrorCode::ResourceExhausted,
            Stage::Inspect,
            "Archive exceeds the 10,000-entry alpha limit",
            "Split the archive and retry.",
        ));
    }
    if total_bytes > u128::from(MAX_ARCHIVE_TOTAL_BYTES) {
        return Err(FormatWrightError::new(
            ErrorCode::ResourceExhausted,
            Stage::Inspect,
            "Archive expanded size exceeds the 2 GiB alpha limit",
            "Convert a smaller archive.",
        ));
    }
    Ok(())
}

pub(crate) fn entry_manifest_digest(entries: &[ArchiveEntry]) -> String {
    let mut lines: Vec<String> = entries
        .iter()
        .map(|entry| format!("{}:{}", entry.name, entry.size))
        .collect();
    lines.sort();
    let joined = lines.join("\n");
    format!("blake3:{}", blake3::hash(joined.as_bytes()).to_hex())
}

fn input_error(path: &Path, error: impl std::fmt::Display) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Inspect,
        format!("Unable to read archive {}", path.display()),
        "Verify the archive is not corrupted.",
    )
    .with_diagnostic(error.to_string())
}

/// Inspects a ZIP or tar.gz archive without extracting to disk.
///
/// # Errors
///
/// Returns typed input, policy, or resource errors for malformed archives.
pub async fn inspect_archive(path: impl AsRef<Path>) -> Result<Probe> {
    let path = path.as_ref();
    let format = archive_format_hint(path)?
        .ok_or_else(|| unsupported("Archive format is not recognized"))?;
    let artifact = identify_artifact(path).await?;
    let owned = artifact.canonical_path.clone();
    let format_owned = format.to_owned();
    let (entries, total_bytes) = tokio::task::spawn_blocking(move || {
        let entries = match format_owned.as_str() {
            "zip" => read_zip_entries(&owned)?,
            "7z" => read_7z_entries(&owned)?,
            _ => read_targz_entries(&owned)?,
        };
        let total: u64 = entries.iter().map(|entry| entry.size).sum();
        Ok::<_, FormatWrightError>((entries, total))
    })
    .await
    .map_err(|error| worker_error(&error))??;
    let properties = BTreeMap::from([
        (
            "entry_count".to_owned(),
            json!(u64::try_from(entries.len()).unwrap_or(u64::MAX)),
        ),
        ("total_entry_bytes".to_owned(), json!(total_bytes)),
        (
            "entry_manifest_digest".to_owned(),
            json!(entry_manifest_digest(&entries)),
        ),
    ]);
    Ok(Probe {
        schema_version: SCHEMA_VERSION,
        artifact,
        format: FormatDescriptor {
            id: format.to_owned(),
            kind: FormatKind::Archive,
            mime_type: Some(
                match format {
                    "zip" => "application/zip",
                    "7z" => "application/x-7z-compressed",
                    _ => "application/gzip",
                }
                .to_owned(),
            ),
            container: Some("archive".to_owned()),
            extension_matches: Some(true),
            confidence: 1.0,
        },
        streams: vec![StreamProbe {
            index: 0,
            kind: StreamKind::Data,
            codec: None,
            language: None,
            duration_seconds: None,
            width: None,
            height: None,
            frame_rate: None,
            sample_rate: None,
            channels: None,
            properties,
        }],
        metadata: BTreeMap::new(),
        warnings: Vec::new(),
        evidence: ProbeEvidence {
            engine_id: "formatwright.archive-inspector".to_owned(),
            engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            engine_binary_sha256: None,
        },
        duration_seconds: None,
        bit_rate: None,
    })
}

fn worker_error(error: &tokio::task::JoinError) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::Internal,
        Stage::Inspect,
        "Archive inspection worker failed",
        "Retry or report the input.",
    )
    .with_diagnostic(error.to_string())
}

fn unsupported(message: &str) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::Unsupported,
        Stage::Plan,
        message.to_owned(),
        "Choose a supported archive route.",
    )
}

/// Plans a native ZIP <-> tar.gz repack.
///
/// # Errors
///
/// Returns `Unsupported` for any pair other than zip -> tar.gz and
/// tar.gz -> zip.
pub fn plan_archive_conversion(
    probe: &Probe,
    request: &PlanRequest,
    engine: &EngineIdentity,
) -> Result<Plan> {
    let requested = request
        .target_format
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    let normalized_target = match requested.as_str() {
        "tar.gz" | "tgz" | "taz" => "tar.gz",
        "zip" => "zip",
        "7z" => "7z",
        _ => {
            return Err(unsupported(
                "Archive conversion must be zip -> tar.gz, tar.gz -> zip, zip -> 7z, or 7z -> zip",
            ));
        }
    };
    if !matches!(
        (probe.format.id.as_str(), normalized_target),
        ("zip", "tar.gz" | "7z") | ("tar.gz" | "7z", "zip")
    ) {
        return Err(unsupported(
            "Archive conversion must be zip -> tar.gz, tar.gz -> zip, zip -> 7z, or 7z -> zip",
        ));
    }
    if engine.engine_id != "formatwright.archive" {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            "The archive Plan was given the wrong engine",
            "Use the built-in archive engine.",
        ));
    }
    let arguments = BTreeMap::from([
        ("source_format".to_owned(), probe.format.id.clone()),
        ("target_format".to_owned(), normalized_target.to_owned()),
        ("resource_policy".to_owned(), "deny-all".to_owned()),
    ]);
    let constraints = BTreeMap::from([
        ("network".to_owned(), json!("deny")),
        ("extraction_to_disk".to_owned(), json!("deny")),
    ]);
    let step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: format!(
            "formatwright.archive.{}-to-{}.native",
            probe.format.id, normalized_target
        ),
        engine: engine.clone(),
        operation: Operation::Transform,
        loss_class: LossClass::ContainerOnly,
        arguments,
        estimated_temporary_bytes: Some(
            property_u64(probe, "total_entry_bytes").unwrap_or(probe.artifact.size_bytes) * 2,
        ),
    };
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: Uuid::new_v4(),
        plan_hash: String::new(),
        input_fingerprint: probe.artifact.fast_fingerprint.clone(),
        target_format: normalized_target.to_owned(),
        constraints,
        steps: vec![step],
        changes: ChangeSet {
            preserved: vec![
                "every archive entry's name, order-independent set, and size".to_owned(),
                "entry payload bytes".to_owned(),
            ],
            changed: vec![format!(
                "container changes from {} to {}",
                probe.format.id, normalized_target
            )],
            dropped: vec![],
            unknown: vec![
                "stored file metadata (timestamps, permissions) is not certified".to_owned(),
            ],
        },
        validators: vec![
            "archive.entry-count".to_owned(),
            "archive.entry-manifest".to_owned(),
        ],
        network_policy: NetworkPolicy::Deny,
        output_path: request.output_path.clone(),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

fn property_u64(probe: &Probe, name: &str) -> Option<u64> {
    probe
        .streams
        .first()
        .and_then(|stream| stream.properties.get(name))
        .and_then(Value::as_u64)
}

/// Repacks a ZIP into a tar.gz without extracting to disk. Timestamps are
/// pinned to zero so identical inputs produce identical outputs.
pub(crate) fn repack_zip_to_targz(input: &Path, output: &Path) -> Result<()> {
    let file = File::open(input).map_err(|error| input_error(input, error))?;
    let mut archive = ZipArchive::new(file).map_err(|error| input_error(input, error))?;
    enforce_archive_limits(
        archive.len(),
        archive.decompressed_size().unwrap_or(u128::MAX),
    )?;
    let out = File::create(output).map_err(|error| output_error(output, error))?;
    let encoder = flate2::write::GzEncoder::new(out, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| input_error(input, error))?;
        let name = entry.name().to_owned();
        let mut header = tar::Header::new_gnu();
        if entry.is_dir() {
            let mut directory = name.clone();
            if !directory.ends_with('/') {
                directory.push('/');
            }
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            append_tar_entry(&mut builder, &mut header, &directory, &mut std::io::empty())?;
        } else {
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(entry.size());
            append_tar_entry(&mut builder, &mut header, &name, &mut entry)?;
        }
    }
    let encoder = builder
        .into_inner()
        .map_err(|error| output_error(output, error))?;
    encoder
        .finish()
        .map_err(|error| output_error(output, error))?;
    Ok(())
}

fn append_tar_entry<W, R>(
    builder: &mut tar::Builder<W>,
    header: &mut tar::Header,
    name: &str,
    data: &mut R,
) -> Result<()>
where
    W: std::io::Write,
    R: std::io::Read,
{
    header.set_mode(0o644);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_cksum();
    builder.append_data(header, name, data).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            "Unable to append an entry to the tar.gz output",
            "Retry the conversion.",
        )
        .with_diagnostic(error.to_string())
    })
}

/// Repacks a tar.gz into a ZIP without extracting to disk. Symlink, hardlink,
/// and device entries are rejected to keep the validated manifest honest.
pub(crate) fn repack_targz_to_zip(input: &Path, output: &Path) -> Result<()> {
    let file = File::open(input).map_err(|error| input_error(input, error))?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let out = File::create(output).map_err(|error| output_error(output, error))?;
    let mut writer = zip::ZipWriter::new(out);
    let options = zip::write::SimpleFileOptions::default();
    let mut total: u64 = 0;
    for (count, entry) in archive
        .entries()
        .map_err(|error| input_error(input, error))?
        .enumerate()
    {
        let mut entry = entry.map_err(|error| input_error(input, error))?;
        let name = entry
            .path()
            .map_err(|error| input_error(input, error))?
            .to_string_lossy()
            .replace('\\', "/");
        let header = entry.header();
        let entry_type = header.entry_type();
        if entry_type.is_dir() {
            let mut directory = name.clone();
            if !directory.ends_with('/') {
                directory.push('/');
            }
            writer
                .add_directory(directory, options)
                .map_err(|error| output_error(output, error))?;
        } else if entry_type.is_file() {
            let size = entry
                .header()
                .entry_size()
                .map_err(|error| input_error(input, error))?;
            total = total.saturating_add(size);
            enforce_archive_limits(count + 1, u128::from(total))?;
            writer
                .start_file(name, options)
                .map_err(|error| output_error(output, error))?;
            std::io::copy(&mut entry, &mut writer).map_err(|error| {
                FormatWrightError::new(
                    ErrorCode::ExecutionFailed,
                    Stage::Execute,
                    "Unable to copy an entry into the ZIP output",
                    "Retry the conversion.",
                )
                .with_diagnostic(error.to_string())
            })?;
        } else {
            return Err(FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Execute,
                format!("Archive entry {name} is a link or device, not a regular file"),
                "Rebuild the archive without links or devices and retry.",
            ));
        }
    }
    writer
        .finish()
        .map_err(|error| output_error(output, error))?;
    Ok(())
}

/// Repacks a ZIP into a 7z without extracting to disk. Timestamps stay unset
/// so identical inputs produce identical outputs.
pub(crate) fn repack_zip_to_7z(input: &Path, output: &Path) -> Result<()> {
    let file = File::open(input).map_err(|error| input_error(input, error))?;
    let mut archive = ZipArchive::new(file).map_err(|error| input_error(input, error))?;
    enforce_archive_limits(
        archive.len(),
        archive.decompressed_size().unwrap_or(u128::MAX),
    )?;
    let mut writer =
        sevenz_rust::SevenZWriter::create(output).map_err(|error| output_error(output, error))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| input_error(input, error))?;
        let mut record = sevenz_rust::SevenZArchiveEntry::new();
        record.name = entry.name().to_owned();
        if entry.is_dir() {
            writer
                .push_archive_entry::<std::io::Empty>(record, None)
                .map_err(|error| output_error(output, error))?;
        } else {
            record.size = entry.size();
            writer
                .push_archive_entry(record, Some(&mut entry))
                .map_err(|error| output_error(output, error))?;
        }
    }
    writer
        .finish()
        .map_err(|error| output_error(output, error))?;
    Ok(())
}

/// Repacks a 7z into a ZIP without extracting to disk.
pub(crate) fn repack_7z_to_zip(input: &Path, output: &Path) -> Result<()> {
    let mut reader = sevenz_rust::SevenZReader::open(input, sevenz_rust::Password::empty())
        .map_err(|error| input_error(input, error))?;
    let out = File::create(output).map_err(|error| output_error(output, error))?;
    let mut writer = zip::ZipWriter::new(out);
    let options = zip::write::SimpleFileOptions::default();
    let mut total: u64 = 0;
    let mut count: usize = 0;
    // The 7z callback returns its own error type, so the first typed
    // FormatWrightError is captured here and surfaced after the iteration.
    let mut failure: Option<FormatWrightError> = None;
    reader
        .for_each_entries(|entry, data| {
            if failure.is_some() {
                return Ok(true);
            }
            let outcome: std::io::Result<(String, u64, bool)> = (|| {
                let name = entry.name().replace('\\', "/");
                if entry.is_directory() {
                    let mut directory = name.clone();
                    if !directory.ends_with('/') {
                        directory.push('/');
                    }
                    writer.add_directory(directory, options)?;
                } else {
                    writer.start_file(name.clone(), options)?;
                    std::io::copy(data, &mut writer)?;
                }
                Ok((name, entry.size, entry.is_directory()))
            })();
            let (name, size, is_directory) = match outcome {
                Ok(value) => value,
                Err(error) => {
                    failure.get_or_insert(output_error(output, error));
                    return Ok(true);
                }
            };
            if !is_directory {
                let record = ArchiveEntry { name, size };
                if let Err(error) = record.safe_name() {
                    failure.get_or_insert(error);
                    return Ok(true);
                }
                total = total.saturating_add(size);
                count += 1;
                if let Err(error) = enforce_archive_limits(count, u128::from(total)) {
                    failure.get_or_insert(error);
                    return Ok(true);
                }
            }
            Ok(false)
        })
        .map_err(|error| input_error(input, error))?;
    if let Some(error) = failure {
        return Err(error);
    }
    writer
        .finish()
        .map_err(|error| output_error(output, error))?;
    Ok(())
}

fn output_error(path: &Path, error: impl std::fmt::Display) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::StorageFailed,
        Stage::Execute,
        format!("Unable to write archive {}", path.display()),
        "Check destination permissions and storage health.",
    )
    .with_diagnostic(error.to_string())
}

/// Validates that a repacked archive carries the same entry set and sizes.
pub(crate) fn validate_archive_output(
    input: &Probe,
    output: &Probe,
    plan: &Plan,
    job_id: Uuid,
) -> ValidationReport {
    let expected_count = property_u64(input, "entry_count");
    let observed_count = property_u64(output, "entry_count");
    let expected_manifest = property_string(input, "entry_manifest_digest");
    let observed_manifest = property_string(output, "entry_manifest_digest");
    let checks = vec![
        validation_check(
            "ARCHIVE_OPENS",
            ValidationStatus::Pass,
            json!(true),
            json!(true),
            "Native reader enumerated the repacked container.",
        ),
        validation_check(
            "ARCHIVE_TARGET_FORMAT",
            status(output.format.id != input.format.id),
            json!(plan.target_format),
            json!(output.format.id),
            "Output container differs from the input container.",
        ),
        validation_check(
            "ARCHIVE_ENTRY_COUNT",
            status(expected_count.is_some() && expected_count == observed_count),
            json!(expected_count),
            json!(observed_count),
            "Entry count is preserved.",
        ),
        validation_check(
            "ARCHIVE_ENTRY_MANIFEST",
            status(
                expected_manifest.is_string()
                    && observed_manifest.is_string()
                    && expected_manifest == observed_manifest,
            ),
            expected_manifest,
            observed_manifest,
            "Name-and-size manifest digest is preserved.",
        ),
    ];
    let report_status = checks.iter().fold(ValidationStatus::Pass, |state, check| {
        state.worst(check.status)
    });
    ValidationReport {
        schema_version: SCHEMA_VERSION,
        report_id: Uuid::new_v4(),
        job_id,
        plan_hash: plan.plan_hash.clone(),
        status: report_status,
        input: artifact_summary(input),
        output: artifact_summary(output),
        engines: plan.steps.iter().map(|step| step.engine.clone()).collect(),
        checks,
        intentional_changes: plan.changes.changed.clone(),
        redaction: ReportRedaction {
            paths_redacted: false,
            metadata_values_redacted: true,
        },
    }
}

fn property_string(probe: &Probe, name: &str) -> Value {
    probe
        .streams
        .first()
        .and_then(|stream| stream.properties.get(name))
        .cloned()
        .unwrap_or(Value::Null)
}

fn status(value: bool) -> ValidationStatus {
    if value {
        ValidationStatus::Pass
    } else {
        ValidationStatus::Fail
    }
}

fn validation_check(
    code: &str,
    status: ValidationStatus,
    expected: Value,
    observed: Value,
    message: &str,
) -> ValidationCheck {
    ValidationCheck {
        code: code.to_owned(),
        status,
        required: true,
        expected,
        observed,
        evidence: "FormatWright native archive inspector".to_owned(),
        message: message.to_owned(),
    }
}

fn artifact_summary(probe: &Probe) -> ArtifactSummary {
    ArtifactSummary {
        display_path: Some(probe.artifact.display_path.clone()),
        format_id: probe.format.id.clone(),
        size_bytes: probe.artifact.size_bytes,
        fast_fingerprint: probe.artifact.fast_fingerprint.clone(),
        full_blake3: probe.artifact.full_blake3.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        archive_format_hint, entry_manifest_digest, inspect_archive, plan_archive_conversion,
    };

    fn write_zip(path: &std::path::Path, files: &[(&str, &str)]) {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let file = fs::File::create(path).expect("create zip");
        let mut archive = zip::ZipWriter::new(file);
        for (name, content) in files {
            archive
                .start_file((*name).to_owned(), SimpleFileOptions::default())
                .expect("zip entry");
            archive.write_all(content.as_bytes()).expect("zip content");
        }
        archive.finish().expect("zip finish");
    }

    #[tokio::test]
    async fn zip_archives_inspect_with_entry_inventory() {
        let directory = tempdir().expect("tempdir");
        let input = directory.path().join("bundle.zip");
        write_zip(
            &input,
            &[
                ("a.txt", "alpha"),
                ("b/c.txt", "charlie ELECTRIC 440010147700"),
            ],
        );
        let probe = inspect_archive(&input).await.expect("zip inspection");
        assert_eq!(probe.format.id, "zip");
        assert_eq!(probe.format.kind, crate::domain::FormatKind::Archive);
        assert_eq!(
            probe.streams[0]
                .properties
                .get("entry_count")
                .and_then(serde_json::Value::as_u64),
            Some(2)
        );
        assert!(
            probe.streams[0]
                .properties
                .contains_key("entry_manifest_digest")
        );
    }

    #[test]
    fn unsafe_entry_paths_are_rejected() {
        use super::ArchiveEntry;

        let suspicious = ArchiveEntry {
            name: "../escape.txt".to_owned(),
            size: 1,
        };
        assert!(suspicious.safe_name().is_err());
        let absolute = ArchiveEntry {
            name: "/etc/passwd".to_owned(),
            size: 1,
        };
        assert!(absolute.safe_name().is_err());
        let safe = ArchiveEntry {
            name: "docs/report.txt".to_owned(),
            size: 1,
        };
        assert!(safe.safe_name().is_ok());
    }

    #[test]
    fn manifest_digest_is_order_independent() {
        use super::ArchiveEntry;

        let first = vec![
            ArchiveEntry {
                name: "a.txt".to_owned(),
                size: 10,
            },
            ArchiveEntry {
                name: "b.txt".to_owned(),
                size: 20,
            },
        ];
        let second = vec![
            ArchiveEntry {
                name: "b.txt".to_owned(),
                size: 20,
            },
            ArchiveEntry {
                name: "a.txt".to_owned(),
                size: 10,
            },
        ];
        assert_eq!(
            entry_manifest_digest(&first),
            entry_manifest_digest(&second)
        );
    }

    #[test]
    fn extension_must_match_the_magic_family() {
        let directory = tempdir().expect("tempdir");
        let fake = directory.path().join("not-really.zip");
        fs::write(&fake, "plain text, not a zip").expect("write fake");
        assert!(archive_format_hint(&fake).is_err());
        let plain = directory.path().join("readme.txt");
        fs::write(&plain, "hello").expect("write plain");
        assert_eq!(archive_format_hint(&plain).expect("hint"), None);
    }

    #[tokio::test]
    async fn archive_plan_targets_the_opposite_container() {
        let directory = tempdir().expect("tempdir");
        let input = directory.path().join("bundle.zip");
        write_zip(&input, &[("a.txt", "alpha")]);
        let probe = inspect_archive(&input).await.expect("zip inspection");
        let engine = formatwright_engine_sdk::EngineIdentity {
            engine_id: "formatwright.archive".to_owned(),
            version: "test".to_owned(),
            binary_path: std::path::PathBuf::from("self.exe"),
            binary_sha256: "0".repeat(64),
            manifest_sha256: None,
            build_configuration: None,
            certification: formatwright_engine_sdk::Certification::Experimental,
        };
        let request = crate::domain::PlanRequest {
            target_format: "tar.gz".to_owned(),
            output_path: Some(directory.path().join("bundle.tar.gz")),
            ..crate::domain::PlanRequest::default()
        };
        let plan = plan_archive_conversion(&probe, &request, &engine).expect("archive plan");
        assert_eq!(plan.target_format, "tar.gz");
        assert_eq!(
            plan.steps[0].loss_class,
            formatwright_engine_sdk::LossClass::ContainerOnly
        );
        assert!(
            plan.validators
                .iter()
                .any(|validator| validator == "archive.entry-count")
        );
    }

    #[tokio::test]
    async fn seven_z_round_trip_preserves_the_entry_manifest() {
        let directory = tempdir().expect("tempdir");
        let original = directory.path().join("original.zip");
        write_zip(
            &original,
            &[
                (
                    "docs/readme.txt",
                    "FormatWright 7z round trip ELECTRIC 440010147700",
                ),
                ("data/manifest.json", r#"{"entries": 2}"#),
                ("nested/", ""),
            ],
        );
        let source_probe = inspect_archive(&original)
            .await
            .expect("source zip inspection");
        let manifest_before = source_probe.streams[0].properties["entry_manifest_digest"].clone();
        let count_before = source_probe.streams[0].properties["entry_count"].clone();

        let sevenz = directory.path().join("roundtrip.7z");
        super::repack_zip_to_7z(&original, &sevenz).expect("zip -> 7z repack");
        let sevenz_probe = inspect_archive(&sevenz).await.expect("7z inspection");
        assert_eq!(sevenz_probe.format.id, "7z");
        assert_eq!(
            sevenz_probe.streams[0].properties["entry_manifest_digest"], manifest_before,
            "the 7z repack preserves the name-and-size manifest"
        );

        let back = directory.path().join("back.zip");
        super::repack_7z_to_zip(&sevenz, &back).expect("7z -> zip repack");
        let back_probe = inspect_archive(&back)
            .await
            .expect("round-trip zip inspection");
        assert_eq!(back_probe.format.id, "zip");
        assert_eq!(
            back_probe.streams[0].properties["entry_manifest_digest"], manifest_before,
            "the zip -> 7z -> zip round trip preserves the manifest"
        );
        assert_eq!(
            back_probe.streams[0].properties["entry_count"],
            count_before,
        );
    }

    #[tokio::test]
    async fn repack_round_trip_preserves_the_entry_manifest() {
        let directory = tempdir().expect("tempdir");
        let original = directory.path().join("original.zip");
        write_zip(
            &original,
            &[
                ("docs/readme.txt", "FormatWright archive round trip"),
                ("data/manifest.json", r#"{"entries": 2}"#),
            ],
        );
        let source_probe = inspect_archive(&original)
            .await
            .expect("source zip inspection");
        let manifest_before = source_probe.streams[0].properties["entry_manifest_digest"].clone();

        let targz = directory.path().join("roundtrip.tar.gz");
        super::repack_zip_to_targz(&original, &targz).expect("zip -> tar.gz repack");
        let targz_probe = inspect_archive(&targz).await.expect("tar.gz inspection");
        assert_eq!(targz_probe.format.id, "tar.gz");
        assert_eq!(
            targz_probe.streams[0].properties["entry_manifest_digest"], manifest_before,
            "the repack preserves the name-and-size manifest"
        );

        let back = directory.path().join("back.zip");
        super::repack_targz_to_zip(&targz, &back).expect("tar.gz -> zip repack");
        let back_probe = inspect_archive(&back)
            .await
            .expect("round-trip zip inspection");
        assert_eq!(back_probe.format.id, "zip");
        assert_eq!(
            back_probe.streams[0].properties["entry_manifest_digest"], manifest_before,
            "the round trip preserves the name-and-size manifest"
        );
        assert_eq!(
            back_probe.streams[0].properties["entry_count"],
            source_probe.streams[0].properties["entry_count"],
        );
    }
}
