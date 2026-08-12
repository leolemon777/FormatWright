use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::domain::ArtifactIdentity;
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};

const SAMPLE_BYTES: usize = 1024 * 1024;
const FULL_FAST_HASH_THRESHOLD: u64 = (SAMPLE_BYTES as u64) * 3;

/// Builds a bounded-cost identity for a regular file.
///
/// # Errors
///
/// Returns an input error when the path cannot be resolved, is not a regular
/// file, or cannot be sampled.
pub async fn identify_artifact(path: impl AsRef<Path>) -> Result<ArtifactIdentity> {
    let path = path.as_ref().to_path_buf();
    tokio::task::spawn_blocking(move || identify_artifact_blocking(&path))
        .await
        .map_err(|error| {
            FormatWrightError::new(
                ErrorCode::Internal,
                Stage::Inspect,
                "Artifact identity worker failed",
                "Retry the operation.",
            )
            .with_diagnostic(error.to_string())
        })?
}

fn identify_artifact_blocking(path: &Path) -> Result<ArtifactIdentity> {
    ensure_local_filesystem_path(path, Stage::Inspect)?;
    let canonical_path = path.canonicalize().map_err(|error| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            format!("Cannot resolve input path: {}", path.display()),
            "Check that the file exists and is accessible.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let metadata = canonical_path.metadata().map_err(|error| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            format!("Cannot read input metadata: {}", path.display()),
            "Check file permissions and retry.",
        )
        .with_diagnostic(error.to_string())
    })?;
    if !metadata.is_file() {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            format!("Input is not a regular file: {}", path.display()),
            "Select a regular file. Folder workflows use the batch command.",
        ));
    }

    let modified_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        });
    let fast_fingerprint = sampled_blake3(&canonical_path, metadata.len())?;

    Ok(ArtifactIdentity {
        display_path: path.to_string_lossy().into_owned(),
        canonical_path,
        size_bytes: metadata.len(),
        modified_unix_ms,
        fast_fingerprint,
        full_blake3: None,
    })
}

pub(crate) fn ensure_local_filesystem_path(path: &Path, stage: Stage) -> Result<()> {
    #[cfg(not(windows))]
    if path.to_string_lossy().starts_with("//") {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            stage,
            format!("Network filesystem paths are disabled: {}", path.display()),
            "Choose a local filesystem path or use a future explicit network policy.",
        ));
    }
    #[cfg(windows)]
    if matches!(
        path.components().next(),
        Some(std::path::Component::Prefix(prefix))
            if matches!(
                prefix.kind(),
                std::path::Prefix::UNC(_, _) | std::path::Prefix::VerbatimUNC(_, _)
            )
    ) {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            stage,
            format!("Network filesystem paths are disabled: {}", path.display()),
            "Choose a local filesystem path or use a future explicit network policy.",
        ));
    }
    Ok(())
}

fn sampled_blake3(path: &Path, size: u64) -> Result<String> {
    let mut file = File::open(path).map_err(|error| io_error(path, &error))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"formatwright-fast-fingerprint-v1");
    hasher.update(&size.to_le_bytes());

    if size <= FULL_FAST_HASH_THRESHOLD {
        hash_reader(&mut file, &mut hasher, u64::MAX, path)?;
    } else {
        hash_region(&mut file, &mut hasher, 0, SAMPLE_BYTES, path)?;
        let middle = size.saturating_sub(SAMPLE_BYTES as u64) / 2;
        hash_region(&mut file, &mut hasher, middle, SAMPLE_BYTES, path)?;
        let end = size.saturating_sub(SAMPLE_BYTES as u64);
        hash_region(&mut file, &mut hasher, end, SAMPLE_BYTES, path)?;
    }

    Ok(format!("fwfp-v1:{}", hasher.finalize().to_hex()))
}

/// Computes a complete BLAKE3 digest for provenance-sensitive workflows.
///
/// # Errors
///
/// Returns an input error when the file cannot be opened or read.
pub async fn full_blake3(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref().to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = File::open(&path).map_err(|error| io_error(&path, &error))?;
        let mut hasher = blake3::Hasher::new();
        hash_reader(&mut file, &mut hasher, u64::MAX, &path)?;
        Ok(format!("blake3:{}", hasher.finalize().to_hex()))
    })
    .await
    .map_err(|error| {
        FormatWrightError::new(
            ErrorCode::Internal,
            Stage::Inspect,
            "Full hash worker failed",
            "Retry the operation.",
        )
        .with_diagnostic(error.to_string())
    })?
}

fn hash_region(
    file: &mut File,
    hasher: &mut blake3::Hasher,
    offset: u64,
    length: usize,
    path: &Path,
) -> Result<()> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| io_error(path, &error))?;
    hash_reader(file, hasher, length as u64, path)
}

fn hash_reader(
    file: &mut File,
    hasher: &mut blake3::Hasher,
    max_bytes: u64,
    path: &Path,
) -> Result<()> {
    let mut remaining = max_bytes;
    let mut buffer = vec![0_u8; 64 * 1024];
    while remaining > 0 {
        let limit = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
        let read = file
            .read(&mut buffer[..limit])
            .map_err(|error| io_error(path, &error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        remaining = remaining.saturating_sub(read as u64);
    }
    Ok(())
}

fn io_error(path: &Path, error: &std::io::Error) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Inspect,
        format!("Cannot read input: {}", path.display()),
        "Check file permissions and storage health.",
    )
    .with_diagnostic(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::{Seek, Write};
    use std::path::Path;

    use tempfile::NamedTempFile;

    use super::{SAMPLE_BYTES, ensure_local_filesystem_path, full_blake3, identify_artifact};
    use crate::{ErrorCode, Stage};

    #[test]
    fn rejects_network_paths_before_filesystem_access() {
        let error =
            ensure_local_filesystem_path(Path::new("//server/share/input.bin"), Stage::Inspect)
                .expect_err("network path must be policy blocked before I/O");
        assert_eq!(error.code, ErrorCode::PolicyBlocked);
    }

    #[cfg(windows)]
    #[test]
    fn allows_a_verbatim_local_disk_path() {
        ensure_local_filesystem_path(Path::new(r"\\?\E:\local\input.bin"), Stage::Inspect)
            .expect("verbatim disk path is local, not UNC");
    }

    #[tokio::test]
    async fn fast_fingerprint_changes_when_sampled_content_changes() {
        let mut file = NamedTempFile::new().expect("temporary file");
        file.write_all(b"formatwright").expect("write fixture");
        file.flush().expect("flush fixture");
        let first = identify_artifact(file.path())
            .await
            .expect("first identity");

        file.as_file_mut()
            .rewind()
            .expect("rewind temporary fixture");
        file.write_all(b"FormatWright").expect("rewrite fixture");
        file.flush().expect("flush rewrite");
        let second = identify_artifact(file.path())
            .await
            .expect("second identity");

        assert_ne!(first.fast_fingerprint, second.fast_fingerprint);
    }

    #[tokio::test]
    async fn large_file_fingerprint_is_sampled_but_full_hash_is_available() {
        let mut file = NamedTempFile::new().expect("temporary file");
        let data = vec![42_u8; SAMPLE_BYTES * 4];
        file.write_all(&data).expect("write fixture");
        file.flush().expect("flush fixture");

        let identity = identify_artifact(file.path()).await.expect("identity");
        let complete = full_blake3(file.path()).await.expect("full hash");

        assert!(identity.fast_fingerprint.starts_with("fwfp-v1:"));
        assert!(complete.starts_with("blake3:"));
        assert_ne!(
            identity.fast_fingerprint.trim_start_matches("fwfp-v1:"),
            complete.trim_start_matches("blake3:")
        );
    }
}
