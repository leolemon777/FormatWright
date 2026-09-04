use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use formatwright_engine_sdk::EngineIdentity;
use serde_json::{Map, Value};
use tokio::process::Command;

use crate::domain::{
    DiagnosticMessage, FormatDescriptor, FormatKind, MetadataClassification, MetadataEntry, Probe,
    ProbeEvidence, SCHEMA_VERSION, StreamKind, StreamProbe,
};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::fingerprint::identify_artifact;

/// Inspects a local media file with a pinned ffprobe identity.
///
/// # Errors
///
/// Returns an input or engine error when the file cannot be identified,
/// ffprobe fails, times out, or produces malformed output.
pub async fn inspect_media(path: impl AsRef<Path>, ffprobe: &EngineIdentity) -> Result<Probe> {
    let artifact = identify_artifact(path).await?;
    let header_hint = sniff_header(&artifact.canonical_path)?;
    let mut command = Command::new(&ffprobe.binary_path);
    command.args([
        "-v",
        "error",
        "-protocol_whitelist",
        "file,pipe",
        "-probesize",
        "10485760",
        "-analyzeduration",
        "10000000",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
    ]);
    if let Some(hint) = header_hint {
        command.arg("-f").arg(hint.demuxer);
    }
    let future = command.arg("-i").arg(&artifact.canonical_path).output();
    let output = tokio::time::timeout(Duration::from_secs(60), future)
        .await
        .map_err(|_| {
            FormatWrightError::new(
                ErrorCode::ExecutionFailed,
                Stage::Inspect,
                "Media inspection timed out",
                "Check whether the file or storage device is responsive.",
            )
            .retryable(true)
        })?
        .map_err(|error| {
            FormatWrightError::new(
                ErrorCode::EngineIncompatible,
                Stage::Inspect,
                "Unable to start ffprobe",
                "Run doctor and verify the ffprobe installation.",
            )
            .with_diagnostic(error.to_string())
        })?;
    if !output.status.success() {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            "ffprobe could not recognize or open the input",
            "Verify that the file is complete and supported.",
        )
        .with_diagnostic(bounded_text(&output.stderr)));
    }

    let raw: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Inspect,
            "ffprobe returned invalid JSON",
            "Use a supported ffprobe build.",
        )
        .with_diagnostic(error.to_string())
    })?;
    Ok(parse_probe(
        &raw,
        artifact,
        ffprobe,
        header_hint.and_then(|hint| hint.format_id),
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HeaderHint {
    demuxer: &'static str,
    format_id: Option<&'static str>,
}

fn sniff_header(path: &Path) -> Result<Option<HeaderHint>> {
    let mut file = std::fs::File::open(path).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            format!("Unable to read input header: {}", path.display()),
            "Check file permissions and storage health.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let mut prefix = [0_u8; 512];
    let read = file.read(&mut prefix).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            format!("Unable to read input header: {}", path.display()),
            "Check file permissions and storage health.",
        )
        .with_diagnostic(error.to_string())
    })?;
    Ok(sniff_header_prefix(&prefix[..read]))
}

#[cfg(test)]
fn sniff_demuxer_prefix(prefix: &[u8]) -> Option<&'static str> {
    sniff_header_prefix(prefix).map(|hint| hint.demuxer)
}

#[allow(clippy::too_many_lines)]
fn sniff_header_prefix(prefix: &[u8]) -> Option<HeaderHint> {
    if prefix.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some(HeaderHint {
            demuxer: "png_pipe",
            format_id: Some("png"),
        });
    }
    if prefix.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(HeaderHint {
            demuxer: "image2",
            format_id: Some("jpeg"),
        });
    }
    if prefix.starts_with(b"GIF87a") || prefix.starts_with(b"GIF89a") {
        return Some(HeaderHint {
            demuxer: "gif",
            format_id: Some("gif"),
        });
    }
    if prefix.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        return Some(HeaderHint {
            demuxer: "matroska",
            format_id: None,
        });
    }
    // TIFF byte-order marks: "II*\0" (little-endian) and "MM\0*" (big-endian).
    if prefix.starts_with(&[0x49, 0x49, 0x2a, 0x00])
        || prefix.starts_with(&[0x4d, 0x4d, 0x00, 0x2a])
    {
        return Some(HeaderHint {
            demuxer: "tiff_pipe",
            format_id: Some("tiff"),
        });
    }
    // "BM" is weak on its own, but the reserved field plus the planar DIB size
    // word keep false positives rare for real files.
    if prefix.starts_with(b"BM") && prefix.get(14..18).is_some_and(|rsv| rsv == [0; 4]) {
        return Some(HeaderHint {
            demuxer: "bmp_pipe",
            format_id: Some("bmp"),
        });
    }
    if prefix.get(4..8) == Some(b"ftyp") {
        let brand = prefix.get(8..12);
        let format_id = match brand {
            Some(b"avif" | b"avis") => Some("avif"),
            Some(b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"mif1") => {
                Some("heic")
            }
            _ => None,
        };
        return Some(HeaderHint {
            demuxer: "mov",
            format_id,
        });
    }
    if prefix.starts_with(b"OggS") {
        return Some(HeaderHint {
            demuxer: "ogg",
            format_id: None,
        });
    }
    if prefix.starts_with(b"fLaC") {
        return Some(HeaderHint {
            demuxer: "flac",
            format_id: None,
        });
    }
    if prefix.starts_with(b"FLV") {
        return Some(HeaderHint {
            demuxer: "flv",
            format_id: None,
        });
    }
    if prefix.starts_with(b".RMF") {
        return Some(HeaderHint {
            demuxer: "rm",
            format_id: None,
        });
    }
    if prefix.starts_with(b"RIFF") {
        return match prefix.get(8..12) {
            Some(b"WEBP") => Some(HeaderHint {
                demuxer: "webp_pipe",
                format_id: Some("webp"),
            }),
            Some(b"WAVE") => Some(HeaderHint {
                demuxer: "wav",
                format_id: None,
            }),
            Some(b"AVI ") => Some(HeaderHint {
                demuxer: "avi",
                format_id: None,
            }),
            _ => None,
        };
    }
    if prefix.starts_with(b"ID3") || is_mp3_frame(prefix) {
        return Some(HeaderHint {
            demuxer: "mp3",
            format_id: None,
        });
    }
    if prefix.starts_with(&[0x00, 0x00, 0x01, 0xba]) {
        return Some(HeaderHint {
            demuxer: "mpeg",
            format_id: None,
        });
    }
    if is_mpeg_transport_stream(prefix) {
        return Some(HeaderHint {
            demuxer: "mpegts",
            format_id: None,
        });
    }
    None
}

fn is_mp3_frame(prefix: &[u8]) -> bool {
    prefix.len() >= 2 && prefix[0] == 0xff && prefix[1] & 0xe0 == 0xe0
}

fn is_mpeg_transport_stream(prefix: &[u8]) -> bool {
    prefix.first() == Some(&0x47)
        && (prefix.get(188) == Some(&0x47) || prefix.get(376) == Some(&0x47))
}

fn parse_probe(
    raw: &Value,
    artifact: crate::domain::ArtifactIdentity,
    ffprobe: &EngineIdentity,
    header_format_id: Option<&str>,
) -> Probe {
    let stream_values = raw
        .get("streams")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let streams: Vec<StreamProbe> = stream_values
        .iter()
        .enumerate()
        .map(|(fallback_index, stream)| parse_stream(stream, fallback_index))
        .collect();
    let format_object = raw
        .get("format")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let format_name = string_value(&format_object, "format_name").unwrap_or("unknown");
    let extension = artifact
        .canonical_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let format_id = header_format_id.map_or_else(
        || normalized_format_id(format_name, &extension),
        str::to_owned,
    );
    let extension_matches = if extension.is_empty() {
        None
    } else {
        Some(extension_matches_detected(
            &extension,
            format_name,
            &format_id,
        ))
    };
    let kind = classify_kind(&streams, &format_id);
    let mut warnings = Vec::new();
    if extension_matches == Some(false) {
        warnings.push(DiagnosticMessage {
            code: "EXTENSION_MISMATCH".to_owned(),
            severity: "warning".to_owned(),
            message: format!(
                "File extension .{extension} does not match detected format {format_name}"
            ),
        });
    }
    let metadata = parse_metadata(format_object.get("tags"));
    let duration_seconds =
        string_value(&format_object, "duration").and_then(|value| value.parse::<f64>().ok());
    let bit_rate =
        string_value(&format_object, "bit_rate").and_then(|value| value.parse::<u64>().ok());

    Probe {
        schema_version: SCHEMA_VERSION,
        artifact,
        format: FormatDescriptor {
            id: format_id,
            kind,
            mime_type: None,
            container: Some(format_name.to_owned()),
            extension_matches,
            confidence: 1.0,
        },
        streams,
        metadata,
        warnings,
        evidence: ProbeEvidence {
            engine_id: ffprobe.engine_id.clone(),
            engine_version: ffprobe.version.clone(),
            engine_binary_sha256: Some(ffprobe.binary_sha256.clone()),
        },
        duration_seconds,
        bit_rate,
    }
}

fn parse_stream(value: &Value, fallback_index: usize) -> StreamProbe {
    let object = value.as_object().cloned().unwrap_or_default();
    let index = object
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
        .unwrap_or_else(|| u32::try_from(fallback_index).unwrap_or(u32::MAX));
    let codec_type = string_value(&object, "codec_type").unwrap_or("unknown");
    let kind = match codec_type {
        "video" => StreamKind::Video,
        "audio" => StreamKind::Audio,
        "subtitle" => StreamKind::Subtitle,
        "attachment" => StreamKind::Attachment,
        "data" => StreamKind::Data,
        _ => StreamKind::Unknown,
    };
    let tags = object.get("tags").and_then(Value::as_object);
    let language = tags
        .and_then(|tags| tags.get("language"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let duration_seconds =
        string_value(&object, "duration").and_then(|duration| duration.parse().ok());
    let width = u32_value(&object, "width");
    let height = u32_value(&object, "height");
    let frame_rate = string_value(&object, "avg_frame_rate")
        .or_else(|| string_value(&object, "r_frame_rate"))
        .map(str::to_owned);
    let sample_rate =
        string_value(&object, "sample_rate").and_then(|rate| rate.parse::<u32>().ok());
    let channels = u32_value(&object, "channels");

    let mut properties = BTreeMap::new();
    for key in [
        "pix_fmt",
        "color_space",
        "color_transfer",
        "color_primaries",
        "color_range",
        "channel_layout",
        "profile",
        "level",
    ] {
        if let Some(value) = object.get(key) {
            properties.insert(key.to_owned(), value.clone());
        }
    }

    StreamProbe {
        index,
        kind,
        codec: string_value(&object, "codec_name").map(str::to_owned),
        language,
        duration_seconds,
        width,
        height,
        frame_rate,
        sample_rate,
        channels,
        properties,
    }
}

fn parse_metadata(value: Option<&Value>) -> BTreeMap<String, MetadataEntry> {
    value
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| {
                    let lowered = key.to_ascii_lowercase();
                    let classification = if ["password", "token", "secret"]
                        .iter()
                        .any(|needle| lowered.contains(needle))
                    {
                        MetadataClassification::Secret
                    } else if matches!(
                        lowered.as_str(),
                        "major_brand"
                            | "minor_version"
                            | "compatible_brands"
                            | "encoder"
                            | "handler_name"
                            | "vendor_id"
                    ) {
                        MetadataClassification::Public
                    } else if [
                        "title",
                        "artist",
                        "album",
                        "author",
                        "comment",
                        "description",
                        "creation_time",
                        "location",
                        "copyright",
                        "genre",
                        "date",
                        "track",
                        "disc",
                    ]
                    .iter()
                    .any(|name| lowered == *name || lowered.starts_with(&format!("{name}-")))
                    {
                        MetadataClassification::Private
                    } else {
                        MetadataClassification::Unknown
                    };
                    (
                        key.clone(),
                        MetadataEntry {
                            value: value.clone(),
                            classification,
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string_value<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn u32_value(object: &Map<String, Value>, key: &str) -> Option<u32> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
}

fn normalized_format_id(format_name: &str, extension: &str) -> String {
    if format_name.contains("png_pipe") {
        return "png".to_owned();
    }
    if format_name.contains("jpeg_pipe")
        || format_name.contains("image2") && matches!(extension, "jpg" | "jpeg")
    {
        return "jpeg".to_owned();
    }
    // ffprobe demuxes BMP files (and sometimes TIFF) with the generic
    // image2 demuxer instead of the *_pipe probe demuxers; the extension
    // disambiguates the raster family the same way it does for JPEG.
    if format_name.contains("image2") && extension == "bmp" {
        return "bmp".to_owned();
    }
    if format_name.contains("image2") && matches!(extension, "tiff" | "tif") {
        return "tiff".to_owned();
    }
    if format_name.contains("webp_pipe") {
        return "webp".to_owned();
    }
    if format_name.contains("tiff_pipe") {
        return "tiff".to_owned();
    }
    if format_name.contains("bmp_pipe") {
        return "bmp".to_owned();
    }
    if format_name.contains("matroska") || format_name.contains("webm") {
        return if extension == "webm" { "webm" } else { "mkv" }.to_owned();
    }
    if format_name.contains("mov") || format_name.contains("mp4") {
        return match extension {
            "mov" => "mov",
            "m4a" => "m4a",
            "3gp" => "3gp",
            _ => "mp4",
        }
        .to_owned();
    }
    if format_name.split(',').any(|name| name == "ogg") && extension == "opus" {
        return "opus".to_owned();
    }
    format_name
        .split(',')
        .next()
        .unwrap_or("unknown")
        .to_ascii_lowercase()
}

fn extension_matches_detected(extension: &str, format_name: &str, format_id: &str) -> bool {
    match format_id {
        "png" => extension == "png",
        "jpeg" => matches!(extension, "jpg" | "jpeg"),
        "webp" => extension == "webp",
        "avif" => extension == "avif",
        "heic" => matches!(extension, "heic" | "heif"),
        "tiff" => matches!(extension, "tiff" | "tif"),
        "bmp" => extension == "bmp",
        "gif" => extension == "gif",
        _ => extension_matches_format(extension, format_name),
    }
}

fn extension_matches_format(extension: &str, format_name: &str) -> bool {
    if format_name.contains("matroska") || format_name.contains("webm") {
        return matches!(extension, "mkv" | "mka" | "webm");
    }
    if format_name.contains("mov") || format_name.contains("mp4") {
        return matches!(extension, "mov" | "mp4" | "m4a" | "m4v" | "3gp" | "3g2");
    }
    if format_name.split(',').any(|name| name == "ogg") {
        return matches!(extension, "ogg" | "oga" | "opus");
    }
    format_name
        .split(',')
        .any(|name| name.eq_ignore_ascii_case(extension))
}

fn classify_kind(streams: &[StreamProbe], format_id: &str) -> FormatKind {
    if matches!(
        format_id,
        "png" | "jpeg" | "webp" | "avif" | "heic" | "gif" | "tiff" | "bmp"
    ) {
        FormatKind::Image
    } else if streams
        .iter()
        .any(|stream| stream.kind == StreamKind::Video)
    {
        FormatKind::Video
    } else if streams
        .iter()
        .any(|stream| stream.kind == StreamKind::Audio)
    {
        FormatKind::Audio
    } else if format_id == "pdf" {
        FormatKind::Pdf
    } else {
        FormatKind::Unknown
    }
}

fn bounded_text(bytes: &[u8]) -> String {
    const MAX_BYTES: usize = 64 * 1024;
    let start = bytes.len().saturating_sub(MAX_BYTES);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{
        extension_matches_format, normalized_format_id, sniff_demuxer_prefix, sniff_header_prefix,
    };

    #[test]
    fn normalizes_container_families_using_extension() {
        assert_eq!(
            normalized_format_id("matroska,webm", "mkv"),
            "mkv".to_owned()
        );
        assert_eq!(
            normalized_format_id("matroska,webm", "webm"),
            "webm".to_owned()
        );
        assert_eq!(
            normalized_format_id("mov,mp4,m4a,3gp,3g2,mj2", "mov"),
            "mov".to_owned()
        );
        assert_eq!(
            normalized_format_id("mov,mp4,m4a,3gp,3g2,mj2", "mp4"),
            "mp4".to_owned()
        );
        assert_eq!(normalized_format_id("tiff_pipe", "tiff"), "tiff".to_owned());
        assert_eq!(normalized_format_id("tiff_pipe", "tif"), "tiff".to_owned());
        assert_eq!(normalized_format_id("bmp_pipe", "bmp"), "bmp".to_owned());
    }

    #[test]
    fn sniffs_tiff_and_bmp_headers() {
        assert_eq!(
            sniff_demuxer_prefix(&[0x49, 0x49, 0x2a, 0x00, 0x00, 0x00]),
            Some("tiff_pipe")
        );
        assert_eq!(
            sniff_demuxer_prefix(&[0x4d, 0x4d, 0x00, 0x2a, 0x00, 0x00]),
            Some("tiff_pipe")
        );
        assert_eq!(
            sniff_demuxer_prefix(&[
                0x42, 0x4d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00
            ]),
            Some("bmp_pipe")
        );
    }

    #[test]
    fn recognizes_family_extensions() {
        assert!(extension_matches_format("mkv", "matroska,webm"));
        assert!(extension_matches_format("webm", "matroska,webm"));
        assert!(extension_matches_format("m4a", "mov,mp4,m4a,3gp,3g2,mj2"));
        assert!(!extension_matches_format("txt", "mov,mp4,m4a,3gp,3g2,mj2"));
    }

    #[test]
    fn header_sniffing_ignores_misleading_extensions() {
        assert_eq!(
            sniff_demuxer_prefix(&[0x1a, 0x45, 0xdf, 0xa3, 0x93, 0x42]),
            Some("matroska")
        );
        assert_eq!(sniff_demuxer_prefix(b"....ftypisom"), Some("mov"));
        assert_eq!(sniff_demuxer_prefix(b"RIFF....WAVEfmt "), Some("wav"));
        assert_eq!(
            sniff_demuxer_prefix(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
            Some("png_pipe")
        );
        assert_eq!(
            sniff_demuxer_prefix(&[0xff, 0xd8, 0xff, 0xe0]),
            Some("image2")
        );
        assert_eq!(sniff_demuxer_prefix(b"RIFF....WEBPVP8 "), Some("webp_pipe"));
        assert_eq!(
            sniff_header_prefix(b"....ftypavif").and_then(|hint| hint.format_id),
            Some("avif")
        );
        assert_eq!(
            sniff_header_prefix(b"....ftypheic").and_then(|hint| hint.format_id),
            Some("heic")
        );
        assert_eq!(sniff_demuxer_prefix(b"not a known header"), None);
    }
}
