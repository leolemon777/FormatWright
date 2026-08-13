//! Safe recursive folder enumeration and deterministic output mapping.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::job_store::JobCreateRequest;

pub const MAX_FOLDER_BATCH_FILES: usize = 100_000;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FolderMappingEntry {
    pub input_path: PathBuf,
    pub relative_input_path: PathBuf,
    pub output_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FolderMappingPlan {
    pub input_root: PathBuf,
    pub output_root: PathBuf,
    pub target_extension: String,
    pub discovered: usize,
    pub skipped: usize,
    pub mappings: Vec<FolderMappingEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FolderDiskBudget {
    pub estimated_output_bytes: u64,
    pub peak_temporary_bytes: u64,
    pub safety_margin_bytes: u64,
    pub required_bytes: u64,
    pub available_bytes: u64,
    pub sufficient: bool,
}

#[derive(Debug, Default)]
pub struct FolderBatchService;

impl FolderBatchService {
    /// Recursively enumerates regular files without following links and maps
    /// them under a distinct output root while preserving relative folders.
    ///
    /// # Errors
    ///
    /// Returns an input or policy error for missing, overlapping, non-local,
    /// unreadable, or unbounded roots and invalid target extensions.
    pub fn preview_mapping(
        input_root: impl AsRef<Path>,
        output_root: impl AsRef<Path>,
        target_extension: &str,
    ) -> Result<FolderMappingPlan> {
        let target_extension = validate_target_extension(target_extension)?;
        let input_root = canonical_local_directory(input_root.as_ref(), "input")?;
        let output_root = canonical_local_directory(output_root.as_ref(), "output")?;
        if input_root.starts_with(&output_root) || output_root.starts_with(&input_root) {
            return Err(FormatWrightError::new(
                ErrorCode::PolicyBlocked,
                Stage::Plan,
                "Folder batch input and output roots must not overlap",
                "Choose two separate local folders.",
            ));
        }

        let mut directories = vec![input_root.clone()];
        let mut files = Vec::new();
        let mut discovered = 0_usize;
        let mut skipped = 0_usize;
        while let Some(directory) = directories.pop() {
            let mut entries = std::fs::read_dir(&directory)
                .map_err(|error| folder_io_error(&directory, "enumerate", error))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|error| folder_io_error(&directory, "read an entry from", error))?;
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries.into_iter().rev() {
                let path = entry.path();
                let file_type = entry
                    .file_type()
                    .map_err(|error| folder_io_error(&path, "inspect", error))?;
                if file_type.is_symlink() {
                    discovered = discovered.saturating_add(1);
                    skipped = skipped.saturating_add(1);
                } else if file_type.is_dir() {
                    directories.push(path);
                } else if file_type.is_file() {
                    discovered = discovered.saturating_add(1);
                    files.push(path);
                    if files.len() > MAX_FOLDER_BATCH_FILES {
                        return Err(FormatWrightError::new(
                            ErrorCode::ResourceExhausted,
                            Stage::Inspect,
                            "Folder batch exceeds the 100,000-file limit",
                            "Choose a smaller root or split the work into multiple batches.",
                        ));
                    }
                } else {
                    discovered = discovered.saturating_add(1);
                    skipped = skipped.saturating_add(1);
                }
            }
        }
        files.sort();

        let mut reserved = HashSet::with_capacity(files.len());
        let mut mappings = Vec::with_capacity(files.len());
        for input_path in files {
            let relative_input_path = input_path
                .strip_prefix(&input_root)
                .map_err(|error| {
                    FormatWrightError::new(
                        ErrorCode::Internal,
                        Stage::Plan,
                        "Enumerated file escaped its canonical folder root",
                        "Retry the preview and report this error if it repeats.",
                    )
                    .with_diagnostic(error.to_string())
                })?
                .to_path_buf();
            let output_path = unique_output_path(
                &output_root,
                &relative_input_path,
                &target_extension,
                &mut reserved,
            )?;
            mappings.push(FolderMappingEntry {
                input_path,
                relative_input_path,
                output_path,
            });
        }
        Ok(FolderMappingPlan {
            input_root,
            output_root,
            target_extension,
            discovered,
            skipped,
            mappings,
        })
    }

    /// Estimates durable output plus bounded concurrent staging and compares
    /// it with current free space at the canonical output root.
    ///
    /// # Errors
    ///
    /// Returns a storage error when input sizes or free space cannot be read.
    pub fn disk_budget(
        output_root: impl AsRef<Path>,
        requests: &[JobCreateRequest],
        parallelism: usize,
    ) -> Result<FolderDiskBudget> {
        let output_root = canonical_local_directory(output_root.as_ref(), "output")?;
        let parallelism = parallelism.clamp(1, 16);
        let mut estimated_output_bytes = 0_u64;
        let mut temporary = Vec::with_capacity(requests.len());
        for request in requests {
            let input_bytes = std::fs::metadata(&request.input_path)
                .map_err(|error| folder_io_error(&request.input_path, "measure", error))?
                .len();
            let output_bytes = request
                .plan
                .estimated_output_bytes
                .unwrap_or_else(|| input_bytes.saturating_mul(2).saturating_add(64 * 1024));
            estimated_output_bytes = estimated_output_bytes.saturating_add(output_bytes);
            let peak = request
                .plan
                .steps
                .iter()
                .filter_map(|step| step.estimated_temporary_bytes)
                .max()
                .unwrap_or_else(|| input_bytes.saturating_mul(2));
            temporary.push(peak);
        }
        temporary.sort_unstable_by(|left, right| right.cmp(left));
        let peak_temporary_bytes = temporary
            .into_iter()
            .take(parallelism)
            .fold(0_u64, u64::saturating_add);
        let working_bytes = estimated_output_bytes.saturating_add(peak_temporary_bytes);
        let safety_margin_bytes = (working_bytes / 10).max(256 * 1024 * 1024);
        let required_bytes = working_bytes.saturating_add(safety_margin_bytes);
        let available_bytes = fs2::available_space(&output_root).map_err(|error| {
            FormatWrightError::new(
                ErrorCode::StorageFailed,
                Stage::Store,
                format!(
                    "Cannot read available space for folder batch output: {}",
                    output_root.display()
                ),
                "Choose a local output disk with readable capacity information.",
            )
            .with_diagnostic(error.to_string())
        })?;
        Ok(FolderDiskBudget {
            estimated_output_bytes,
            peak_temporary_bytes,
            safety_margin_bytes,
            required_bytes,
            available_bytes,
            sufficient: available_bytes >= required_bytes,
        })
    }
}

fn validate_target_extension(value: &str) -> Result<String> {
    let normalized = value.trim().trim_start_matches('.').to_ascii_lowercase();
    if normalized.is_empty()
        || normalized.len() > 16
        || !normalized.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "Folder batch target extension is invalid",
            "Choose a target format using 1–16 letters or digits.",
        ));
    }
    Ok(match normalized.as_str() {
        "jpeg" => "jpg".to_owned(),
        "yml" => "yaml".to_owned(),
        _ => normalized,
    })
}

fn canonical_local_directory(path: &Path, purpose: &str) -> Result<PathBuf> {
    if path.as_os_str().is_empty() || is_network_or_device_path(path) {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Inspect,
            format!("Folder batch {purpose} root must be a local disk path"),
            "Choose a local folder instead of a network or device path.",
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| folder_io_error(path, &format!("resolve the {purpose} root"), error))?;
    if !canonical.is_dir() || is_network_or_device_path(&canonical) {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            format!("Folder batch {purpose} root is not a local directory"),
            "Choose an existing readable local folder.",
        ));
    }
    Ok(canonical)
}

fn is_network_or_device_path(path: &Path) -> bool {
    let rendered = path.as_os_str().to_string_lossy();
    if let Some(verbatim) = rendered.strip_prefix("\\\\?\\") {
        let bytes = verbatim.as_bytes();
        return verbatim.to_ascii_uppercase().starts_with("UNC\\")
            || bytes.len() < 3
            || !bytes[0].is_ascii_alphabetic()
            || bytes[1] != b':'
            || !matches!(bytes[2], b'\\' | b'/');
    }
    rendered.starts_with("\\\\") || rendered.starts_with("//")
}

fn unique_output_path(
    output_root: &Path,
    relative_input: &Path,
    target_extension: &str,
    reserved: &mut HashSet<String>,
) -> Result<PathBuf> {
    let stem = relative_input
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Plan,
                "Folder batch contains a filename that is not valid Unicode",
                "Rename the file and retry.",
            )
        })?;
    let parent = relative_input.parent().unwrap_or_else(|| Path::new(""));
    let source_extension = relative_input
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("source")
        .to_ascii_lowercase();
    for attempt in 0_u32..=10_000 {
        let suffix = match attempt {
            0 => String::new(),
            1 => format!(".from-{source_extension}"),
            value => format!(".from-{source_extension}-{value}"),
        };
        let output = output_root
            .join(parent)
            .join(format!("{stem}{suffix}.{target_extension}"));
        if reserved.insert(output_key(&output)) {
            return Ok(output);
        }
    }
    Err(FormatWrightError::new(
        ErrorCode::OutputConflict,
        Stage::Plan,
        "Folder batch could not assign a unique output name",
        "Rename colliding source files or choose another output root.",
    ))
}

fn output_key(path: &Path) -> String {
    let rendered = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        rendered.to_ascii_lowercase()
    } else {
        rendered
    }
}

#[allow(clippy::needless_pass_by_value)]
fn folder_io_error(path: &Path, action: &str, error: std::io::Error) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Inspect,
        format!("Cannot {action} folder batch path: {}", path.display()),
        "Check local folder permissions and retry.",
    )
    .with_diagnostic(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::FolderBatchService;
    use crate::job_store::JobCreateRequest;

    #[test]
    fn preserves_relative_folders_and_disambiguates_same_stem_outputs() {
        let suite = tempdir().expect("suite");
        let input = suite.path().join("input");
        let output = suite.path().join("output");
        fs::create_dir_all(input.join("nested")).expect("input folders");
        fs::create_dir_all(&output).expect("output folder");
        fs::write(input.join("nested/photo.jpg"), b"jpg").expect("jpg");
        fs::write(input.join("nested/photo.png"), b"png").expect("png");

        let plan =
            FolderBatchService::preview_mapping(&input, &output, "webp").expect("folder mapping");

        assert_eq!(plan.discovered, 2);
        assert_eq!(plan.skipped, 0);
        assert_eq!(plan.mappings.len(), 2);
        assert_eq!(
            plan.mappings[0].output_path,
            plan.output_root.join("nested/photo.webp")
        );
        assert_eq!(
            plan.mappings[1].output_path,
            plan.output_root.join("nested/photo.from-png.webp")
        );
    }

    #[test]
    fn rejects_overlapping_roots_and_invalid_extensions() {
        let suite = tempdir().expect("suite");
        let input = suite.path().join("input");
        let nested = input.join("output");
        fs::create_dir_all(&nested).expect("folders");

        assert!(FolderBatchService::preview_mapping(&input, &nested, "webp").is_err());
        assert!(FolderBatchService::preview_mapping(&input, suite.path(), "../exe").is_err());
    }

    #[test]
    fn disk_budget_includes_outputs_parallel_staging_and_safety_margin() {
        let suite = tempdir().expect("suite");
        let output = suite.path().join("output");
        fs::create_dir_all(&output).expect("output");
        let mut requests = Vec::new();
        for index in 0..3 {
            let input = suite.path().join(format!("input-{index}.json"));
            fs::write(&input, vec![0_u8; 1024]).expect("input");
            let mut plan = crate::domain::Plan {
                schema_version: crate::domain::SCHEMA_VERSION,
                plan_id: uuid::Uuid::new_v4(),
                plan_hash: "blake3:test".to_owned(),
                input_fingerprint: "fwfp-v1:test".to_owned(),
                target_format: "yaml".to_owned(),
                constraints: std::collections::BTreeMap::new(),
                steps: Vec::new(),
                changes: crate::domain::ChangeSet::default(),
                validators: Vec::new(),
                network_policy: crate::domain::NetworkPolicy::Deny,
                output_path: Some(output.join(format!("output-{index}.yaml"))),
                estimated_output_bytes: Some(2_000),
            };
            plan.steps.push(crate::domain::PlanStep {
                step_id: format!("step-{index}"),
                capability_id: "test".to_owned(),
                engine: formatwright_engine_sdk::EngineIdentity {
                    engine_id: "test".to_owned(),
                    version: "1".to_owned(),
                    binary_path: suite.path().join("test"),
                    binary_sha256: "sha256:test".to_owned(),
                    manifest_sha256: None,
                    build_configuration: None,
                    certification: formatwright_engine_sdk::Certification::Unverified,
                },
                operation: formatwright_engine_sdk::Operation::Serialize,
                loss_class: formatwright_engine_sdk::LossClass::Lossless,
                arguments: std::collections::BTreeMap::new(),
                estimated_temporary_bytes: Some((index + 1) * 1_000),
            });
            requests.push(JobCreateRequest {
                input_path: input,
                output_path: output.join(format!("output-{index}.yaml")),
                plan,
            });
        }

        let budget = FolderBatchService::disk_budget(&output, &requests, 2).expect("disk budget");

        assert_eq!(budget.estimated_output_bytes, 6_000);
        assert_eq!(budget.peak_temporary_bytes, 5_000);
        assert_eq!(budget.safety_margin_bytes, 256 * 1024 * 1024);
        assert_eq!(budget.required_bytes, 6_000 + 5_000 + 256 * 1024 * 1024);
        assert_eq!(
            budget.sufficient,
            budget.available_bytes >= budget.required_bytes
        );
    }
}
