use std::collections::BTreeMap;
use std::time::Duration;

use formatwright_engine_sdk::{Certification, EngineIdentity, LossClass, Operation};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use crate::domain::{
    ChangeSet, FormatKind, MetadataClassification, NetworkPolicy, Plan, PlanRequest, PlanStep,
    Probe, SCHEMA_VERSION, StreamKind,
};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};

/// Builds a deterministic conversion Plan for a supported media target.
///
/// # Errors
///
/// Returns a planning error when the target is unsupported, a required stream
/// is missing, or the requested preservation policy cannot be satisfied.
pub fn plan_conversion(
    probe: &Probe,
    request: &PlanRequest,
    ffmpeg: &EngineIdentity,
) -> Result<Plan> {
    let target = request
        .target_format
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    match target.as_str() {
        "mp4" => plan_mp4_conversion(probe, request, ffmpeg),
        "mp3" | "m4a" | "wav" | "flac" | "ogg" | "opus" | "aac" => {
            plan_audio_conversion(probe, request, ffmpeg, &target)
        }
        "gif" => plan_gif_conversion(probe, request, ffmpeg),
        "jpg" | "jpeg" | "png" | "webp" | "avif" => {
            plan_image_conversion(probe, request, ffmpeg, &target)
        }
        _ => Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            format!("No certified media planner is available for {target}"),
            "Choose MP4, GIF, an audio target, or JPG, PNG, WebP, or AVIF.",
        )),
    }
}

/// Builds the development fallback Plan for HEIC/HEIF to JPEG/PNG.
///
/// # Errors
///
/// Returns a policy or planning error for unsupported inputs, targets, resize
/// requests, or an incorrect libheif engine.
#[allow(clippy::too_many_lines)]
pub fn plan_heic_conversion(
    probe: &Probe,
    request: &PlanRequest,
    heif_convert: &EngineIdentity,
) -> Result<Plan> {
    if probe.format.id != "heic" || probe.format.kind != FormatKind::Image {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            "The libheif fallback requires HEIC or HEIF image input",
            "Choose a content-detected HEIC/HEIF still image.",
        ));
    }
    if heif_convert.engine_id != "heif-convert" {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            "The HEIC Plan was given the wrong engine",
            "Run doctor and use heif-convert.",
        ));
    }
    if request.width.is_some() {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            "The libheif fallback does not yet combine HEIC decode with resize",
            "Remove --width or use a future certified libvips adapter.",
        ));
    }
    let requested = request
        .target_format
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    let target = match requested.as_str() {
        "jpg" | "jpeg" => "jpeg",
        "png" => "png",
        _ => {
            return Err(FormatWrightError::new(
                ErrorCode::Unsupported,
                Stage::Plan,
                "HEIC/HEIF fallback output must be JPEG or PNG",
                "Choose JPG or PNG.",
            ));
        }
    };
    let quality = if target == "jpeg" {
        let quality = request.quality.unwrap_or(85);
        if !(1..=100).contains(&quality) {
            return Err(FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Plan,
                "JPEG quality must be between 1 and 100",
                "Choose a supported --quality value.",
            ));
        }
        Some(quality)
    } else {
        if request.quality.is_some() {
            return Err(FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Plan,
                "PNG output is lossless and does not accept --quality",
                "Remove --quality.",
            ));
        }
        None
    };
    let step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: format!("libheif.heic-to-{target}.development-fallback"),
        engine: heif_convert.clone(),
        operation: Operation::Transcode,
        loss_class: if target == "jpeg" {
            LossClass::Lossy
        } else {
            LossClass::Lossless
        },
        arguments: BTreeMap::from([
            ("source_format".to_owned(), "heic".to_owned()),
            ("target_format".to_owned(), target.to_owned()),
            (
                "quality".to_owned(),
                quality.map_or_else(|| "lossless".to_owned(), |value| value.to_string()),
            ),
            (
                "orientation".to_owned(),
                "apply-heif-transformations".to_owned(),
            ),
            ("metadata".to_owned(), "drop".to_owned()),
            ("image_selection".to_owned(), "single-primary".to_owned()),
        ]),
        estimated_temporary_bytes: Some(probe.artifact.size_bytes.saturating_mul(8)),
    };
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: Uuid::new_v4(),
        plan_hash: String::new(),
        input_fingerprint: probe.artifact.fast_fingerprint.clone(),
        target_format: target.to_owned(),
        constraints: BTreeMap::from([
            ("quality".to_owned(), json!(quality)),
            ("metadata_policy".to_owned(), json!("drop")),
            ("single_primary_image".to_owned(), json!(true)),
            ("resize".to_owned(), json!(false)),
        ]),
        steps: vec![step],
        changes: ChangeSet {
            preserved: vec![
                "primary still-image dimensions".to_owned(),
                "HEIF orientation transformations applied to pixels".to_owned(),
            ],
            changed: vec![format!(
                "HEVC-compressed pixels are decoded and encoded as {target}"
            )],
            dropped: vec![
                "EXIF, XMP, auxiliary images, thumbnails, depth data, and additional image items"
                    .to_owned(),
            ],
            unknown: vec!["embedded color-profile equivalence is not yet certified".to_owned()],
        },
        validators: vec![
            "media.output-opens".to_owned(),
            "media.target-format".to_owned(),
            "image.single-frame".to_owned(),
            "image.dimensions".to_owned(),
            "image.codec".to_owned(),
        ],
        network_policy: NetworkPolicy::Deny,
        output_path: request.output_path.clone(),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

/// Builds a metadata-clean Plan that preserves encoded payloads and retains
/// structural or unknown metadata by default.
///
/// # Errors
///
/// Returns a policy error for unsupported containers or in-place output.
#[allow(clippy::too_many_lines)]
pub fn plan_metadata_clean(
    probe: &Probe,
    output_path: std::path::PathBuf,
    ffmpeg: &EngineIdentity,
) -> Result<Plan> {
    let muxer = metadata_clean_muxer(&probe.format.id).ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            format!("Metadata cleaning is not available for {}", probe.format.id),
            "Choose a supported media/image file or a future type-specific adapter.",
        )
    })?;
    if output_path
        .canonicalize()
        .is_ok_and(|path| path == probe.artifact.canonical_path)
    {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Plan,
            "In-place metadata cleaning is disabled",
            "Choose a new output path so the source remains unchanged.",
        ));
    }
    let removed_keys = probe
        .metadata
        .iter()
        .filter(|(_, entry)| {
            matches!(
                entry.classification,
                MetadataClassification::Private | MetadataClassification::Secret
            )
        })
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    let retained_keys = probe
        .metadata
        .iter()
        .filter(|(_, entry)| {
            matches!(
                entry.classification,
                MetadataClassification::Public | MetadataClassification::Unknown
            )
        })
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    if let Some(key) = removed_keys.iter().find(|key| !valid_metadata_key(key)) {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Plan,
            format!("Metadata key cannot be represented safely: {key:?}"),
            "Retain the field or use a future type-specific metadata adapter.",
        ));
    }
    let changes = ChangeSet {
        preserved: vec![
            "encoded audio/video/image payloads".to_owned(),
            "stream inventory and dimensions".to_owned(),
            format!("structural/unknown metadata keys: {retained_keys:?}"),
        ],
        changed: vec!["container metadata table is rebuilt".to_owned()],
        dropped: removed_keys
            .iter()
            .map(|key| format!("metadata key: {key}"))
            .collect(),
        unknown: Vec::new(),
    };
    let arguments = BTreeMap::from([
        ("workflow".to_owned(), "metadata-clean".to_owned()),
        ("muxer".to_owned(), muxer.to_owned()),
        (
            "metadata_policy".to_owned(),
            "remove-private-secret-retain-structural-unknown".to_owned(),
        ),
        ("payload_mode".to_owned(), "copy".to_owned()),
        ("chapter_policy".to_owned(), "remove".to_owned()),
    ]);
    let constraints = BTreeMap::from([
        ("removed_metadata_keys".to_owned(), json!(removed_keys)),
        ("retained_metadata_keys".to_owned(), json!(retained_keys)),
        ("in_place".to_owned(), json!(false)),
        ("unknown_metadata_policy".to_owned(), json!("retain")),
    ]);
    let step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: format!("ffmpeg.{}.metadata-clean.experimental", probe.format.id),
        engine: ffmpeg.clone(),
        operation: Operation::MetadataClean,
        loss_class: LossClass::ContainerOnly,
        arguments,
        estimated_temporary_bytes: Some(probe.artifact.size_bytes),
    };
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: Uuid::new_v4(),
        plan_hash: String::new(),
        input_fingerprint: probe.artifact.fast_fingerprint.clone(),
        target_format: probe.format.id.clone(),
        constraints,
        steps: vec![step],
        changes,
        validators: vec![
            "media.output-opens".to_owned(),
            "metadata.removed-keys".to_owned(),
            "metadata.payload-codecs".to_owned(),
            "metadata.stream-inventory".to_owned(),
        ],
        network_policy: NetworkPolicy::Deny,
        output_path: Some(output_path),
        estimated_output_bytes: Some(probe.artifact.size_bytes),
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

fn metadata_clean_muxer(format: &str) -> Option<&'static str> {
    match format {
        "mp4" => Some("mp4"),
        "mov" => Some("mov"),
        "mkv" => Some("matroska"),
        "webm" => Some("webm"),
        "mp3" => Some("mp3"),
        "m4a" => Some("ipod"),
        "wav" => Some("wav"),
        "flac" => Some("flac"),
        "ogg" => Some("ogg"),
        "opus" => Some("opus"),
        "aac" => Some("adts"),
        "jpeg" | "png" => Some("image2"),
        "webp" => Some("webp"),
        "avif" => Some("avif"),
        _ => None,
    }
}

fn valid_metadata_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

#[allow(clippy::too_many_lines)]
fn plan_image_conversion(
    probe: &Probe,
    request: &PlanRequest,
    ffmpeg: &EngineIdentity,
    requested_target: &str,
) -> Result<Plan> {
    if probe.format.kind != FormatKind::Image {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            "Image conversion requires a still-image input",
            "Choose a PNG, JPEG, WebP, AVIF, HEIC, or HEIF image.",
        ));
    }
    let image = probe
        .streams
        .iter()
        .find(|stream| stream.kind == StreamKind::Video)
        .ok_or_else(|| {
            FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Plan,
                "No image payload was detected",
                "Inspect the input and choose another file.",
            )
        })?;
    let target = if matches!(requested_target, "jpg" | "jpeg") {
        "jpeg"
    } else {
        requested_target
    };
    if let Some(width) = request.width
        && !(1..=16_384).contains(&width)
    {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "Image width must be between 1 and 16384 pixels",
            "Choose a supported --width value.",
        ));
    }
    let lossy = target != "png";
    let quality = if lossy {
        let quality = request.quality.unwrap_or(85);
        if !(1..=100).contains(&quality) {
            return Err(FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Plan,
                "Image quality must be between 1 and 100",
                "Choose a supported --quality value.",
            ));
        }
        Some(quality)
    } else {
        if request.quality.is_some() {
            return Err(FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Plan,
                "PNG output is lossless and does not accept --quality",
                "Remove --quality or choose JPEG, WebP, or AVIF.",
            ));
        }
        None
    };
    let source_alpha = image
        .properties
        .get("pix_fmt")
        .and_then(serde_json::Value::as_str)
        .is_some_and(pixel_format_has_alpha);
    if target == "jpeg" && source_alpha {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Plan,
            "JPEG cannot preserve the input alpha channel",
            "Choose PNG/WebP/AVIF, or wait for an explicit background-composite policy.",
        ));
    }
    let (codec, muxer) = match target {
        "jpeg" => ("mjpeg", "image2"),
        "png" => ("png", "image2"),
        "webp" => ("libwebp", "webp"),
        "avif" => ("libaom-av1", "avif"),
        _ => unreachable!("image target dispatch is exhaustive"),
    };
    let mut arguments = BTreeMap::from([
        ("workflow".to_owned(), "still-image-convert".to_owned()),
        ("video_stream_index".to_owned(), image.index.to_string()),
        ("codec".to_owned(), codec.to_owned()),
        ("muxer".to_owned(), muxer.to_owned()),
        ("orientation".to_owned(), "autorotate-pixels".to_owned()),
        ("metadata".to_owned(), "drop".to_owned()),
        (
            "width".to_owned(),
            request
                .width
                .map_or_else(|| "source".to_owned(), |value| value.to_string()),
        ),
        (
            "quality".to_owned(),
            quality.map_or_else(|| "lossless".to_owned(), |value| value.to_string()),
        ),
    ]);
    if let Some(quality) = quality {
        let encoder_quality = match target {
            "jpeg" => ((101_u16 - u16::from(quality)) * 29 / 100 + 2).clamp(2, 31),
            "avif" => ((100_u16 - u16::from(quality)) * 63 / 100).clamp(0, 63),
            "webp" => u16::from(quality),
            _ => unreachable!("lossy image targets are exhaustive"),
        };
        arguments.insert("encoder_quality".to_owned(), encoder_quality.to_string());
    }
    let mut changes = ChangeSet {
        preserved: vec!["single still-image payload".to_owned()],
        changed: vec![
            format!("pixels are encoded as {target}"),
            "orientation is applied to pixels".to_owned(),
            "source metadata is dropped by the alpha image adapter".to_owned(),
        ],
        unknown: vec!["embedded ICC profile equivalence is not yet certified".to_owned()],
        ..ChangeSet::default()
    };
    if let Some(width) = request.width {
        changes.changed.push(format!(
            "output width becomes {width} pixels with aspect ratio preserved"
        ));
    } else {
        changes.preserved.push("source dimensions".to_owned());
    }
    if source_alpha {
        changes.preserved.push("alpha channel".to_owned());
    }
    let constraints = BTreeMap::from([
        ("width".to_owned(), json!(request.width)),
        ("quality".to_owned(), json!(quality)),
        ("preserve_alpha".to_owned(), json!(source_alpha)),
        ("metadata_policy".to_owned(), json!("drop")),
    ]);
    let step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: format!("ffmpeg.image-to-{target}.experimental"),
        engine: ffmpeg.clone(),
        operation: Operation::Transcode,
        loss_class: if lossy {
            LossClass::Lossy
        } else {
            LossClass::Lossless
        },
        arguments,
        estimated_temporary_bytes: Some(probe.artifact.size_bytes.saturating_mul(2)),
    };
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: Uuid::new_v4(),
        plan_hash: String::new(),
        input_fingerprint: probe.artifact.fast_fingerprint.clone(),
        target_format: target.to_owned(),
        constraints,
        steps: vec![step],
        changes,
        validators: vec![
            "media.output-opens".to_owned(),
            "media.target-format".to_owned(),
            "image.single-frame".to_owned(),
            "image.dimensions".to_owned(),
            "image.codec".to_owned(),
            "image.alpha".to_owned(),
        ],
        network_policy: NetworkPolicy::Deny,
        output_path: request.output_path.clone(),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

fn pixel_format_has_alpha(pixel_format: &str) -> bool {
    pixel_format.starts_with("rgba")
        || pixel_format.starts_with("bgra")
        || pixel_format.starts_with("argb")
        || pixel_format.starts_with("abgr")
        || pixel_format.starts_with("yuva")
        || pixel_format.starts_with("gbrap")
        || matches!(pixel_format, "pal8" | "ya8" | "ya16le" | "ya16be")
}

#[allow(clippy::too_many_lines)]
fn plan_gif_conversion(
    probe: &Probe,
    request: &PlanRequest,
    ffmpeg: &EngineIdentity,
) -> Result<Plan> {
    let video = probe
        .streams
        .iter()
        .find(|stream| stream.kind == StreamKind::Video)
        .ok_or_else(|| {
            FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Plan,
                "GIF conversion requires a video stream",
                "Inspect the input and choose a file containing video.",
            )
        })?;
    let start_millis = request.start_millis.unwrap_or(0);
    let duration_millis = request.duration_millis;
    let frames_per_second = request.frames_per_second.unwrap_or(15);
    let loop_count = request.loop_count.unwrap_or(0);
    if duration_millis == Some(0) {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "GIF duration must be greater than zero",
            "Choose a positive --duration-ms value.",
        ));
    }
    if !(1..=60).contains(&frames_per_second) {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "GIF frame rate must be between 1 and 60",
            "Choose --fps in the supported range.",
        ));
    }
    if let Some(width) = request.width
        && !(1..=16_384).contains(&width)
    {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "GIF width must be between 1 and 16384 pixels",
            "Choose a supported --width value.",
        ));
    }
    if let Some(input_seconds) = probe.duration_seconds {
        let start_seconds = Duration::from_millis(start_millis).as_secs_f64();
        let end_seconds = duration_millis
            .map(|duration| start_seconds + Duration::from_millis(duration).as_secs_f64());
        if start_seconds >= input_seconds
            || end_seconds.is_some_and(|end| end > input_seconds + 0.250)
        {
            return Err(FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Plan,
                "GIF time range is outside the input duration",
                "Choose a start and duration within the inspected media duration.",
            ));
        }
    }

    let mut arguments = BTreeMap::from([
        ("workflow".to_owned(), "video-to-gif".to_owned()),
        ("video_stream_index".to_owned(), video.index.to_string()),
        ("start_millis".to_owned(), start_millis.to_string()),
        (
            "duration_millis".to_owned(),
            duration_millis.map_or_else(|| "full".to_owned(), |value| value.to_string()),
        ),
        (
            "frames_per_second".to_owned(),
            frames_per_second.to_string(),
        ),
        ("loop_count".to_owned(), loop_count.to_string()),
        ("palette_max_colors".to_owned(), "256".to_owned()),
        ("palette_dither".to_owned(), "sierra2_4a".to_owned()),
    ]);
    arguments.insert(
        "width".to_owned(),
        request
            .width
            .map_or_else(|| "source".to_owned(), |value| value.to_string()),
    );
    let mut changes = ChangeSet {
        preserved: vec!["selected video content".to_owned()],
        changed: vec![
            format!("frame rate becomes {frames_per_second} fps"),
            "video is quantized to a generated 256-color GIF palette".to_owned(),
            format!("GIF loop count is {loop_count} (zero means infinite)"),
        ],
        ..ChangeSet::default()
    };
    if start_millis > 0 || duration_millis.is_some() {
        changes.changed.push(format!(
            "time range starts at {start_millis} ms and lasts {}",
            duration_millis
                .map_or_else(|| "to input end".to_owned(), |value| format!("{value} ms"))
        ));
    }
    if let Some(width) = request.width {
        changes.changed.push(format!(
            "output width becomes {width} pixels with aspect ratio preserved"
        ));
    } else {
        changes.preserved.push("source dimensions".to_owned());
    }
    changes.dropped.extend(
        probe
            .streams
            .iter()
            .filter(|stream| stream.kind != StreamKind::Video || stream.index != video.index)
            .map(|stream| format!("non-selected stream {}", stream.index)),
    );

    let constraints = BTreeMap::from([
        ("start_millis".to_owned(), json!(start_millis)),
        ("duration_millis".to_owned(), json!(duration_millis)),
        ("width".to_owned(), json!(request.width)),
        ("frames_per_second".to_owned(), json!(frames_per_second)),
        ("loop_count".to_owned(), json!(loop_count)),
        ("palette_max_colors".to_owned(), json!(256)),
        ("palette_dither".to_owned(), json!("sierra2_4a")),
    ]);
    let step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: "ffmpeg.video-to-gif.palette".to_owned(),
        engine: ffmpeg.clone(),
        operation: Operation::Transcode,
        loss_class: LossClass::Lossy,
        arguments,
        estimated_temporary_bytes: Some(probe.artifact.size_bytes / 2),
    };
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: Uuid::new_v4(),
        plan_hash: String::new(),
        input_fingerprint: probe.artifact.fast_fingerprint.clone(),
        target_format: "gif".to_owned(),
        constraints,
        steps: vec![step],
        changes,
        validators: vec![
            "media.output-opens".to_owned(),
            "media.target-format".to_owned(),
            "media.duration".to_owned(),
            "media.video-stream".to_owned(),
            "media.dimensions".to_owned(),
        ],
        network_policy: NetworkPolicy::Deny,
        output_path: request.output_path.clone(),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

#[allow(clippy::too_many_lines)]
fn plan_mp4_conversion(
    probe: &Probe,
    request: &PlanRequest,
    ffmpeg: &EngineIdentity,
) -> Result<Plan> {
    let target = "mp4".to_owned();
    if probe.format.kind != FormatKind::Video {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            "MP4 planning requires an input containing video",
            "Choose a workflow that matches the detected input type.",
        ));
    }

    let videos: Vec<_> = probe
        .streams
        .iter()
        .filter(|stream| stream.kind == StreamKind::Video)
        .collect();
    if videos.is_empty() {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "No video stream was detected",
            "Inspect the input and select another file.",
        ));
    }
    let audios: Vec<_> = probe
        .streams
        .iter()
        .filter(|stream| stream.kind == StreamKind::Audio)
        .collect();
    let subtitles: Vec<_> = probe
        .streams
        .iter()
        .filter(|stream| stream.kind == StreamKind::Subtitle)
        .collect();
    let other_streams: Vec<_> = probe
        .streams
        .iter()
        .filter(|stream| {
            matches!(
                stream.kind,
                StreamKind::Attachment | StreamKind::Data | StreamKind::Unknown
            )
        })
        .collect();

    let unsupported_subtitles: Vec<_> = subtitles
        .iter()
        .filter(|stream| stream.codec.as_deref() != Some("mov_text"))
        .collect();
    if request.preserve_all_streams
        && (!unsupported_subtitles.is_empty() || !other_streams.is_empty())
    {
        let details = format!(
            "{} unsupported subtitle stream(s), {} attachment/data/unknown stream(s)",
            unsupported_subtitles.len(),
            other_streams.len()
        );
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Plan,
            "MP4 cannot preserve every detected stream with the current adapter",
            "Use an expert policy that explicitly converts, externalizes, or drops each stream.",
        )
        .with_diagnostic(details));
    }

    let video_copy = videos
        .iter()
        .all(|stream| matches!(stream.codec.as_deref(), Some("h264" | "hevc")));
    let audio_copy = audios
        .iter()
        .all(|stream| stream.codec.as_deref() == Some("aac"));
    let subtitle_copy = unsupported_subtitles.is_empty();
    let remux = video_copy && audio_copy && subtitle_copy && other_streams.is_empty();

    let operation = if remux {
        Operation::Remux
    } else {
        Operation::Transcode
    };
    let loss_class = if remux {
        LossClass::ContainerOnly
    } else {
        LossClass::Lossy
    };

    let mut arguments = BTreeMap::new();
    arguments.insert(
        "video_mode".to_owned(),
        if video_copy { "copy" } else { "libx264" }.to_owned(),
    );
    arguments.insert(
        "audio_mode".to_owned(),
        if audio_copy { "copy" } else { "aac" }.to_owned(),
    );
    arguments.insert(
        "subtitle_mode".to_owned(),
        if subtitles.is_empty() {
            "none"
        } else if subtitle_copy {
            "copy"
        } else {
            "drop"
        }
        .to_owned(),
    );
    arguments.insert("movflags".to_owned(), "+faststart".to_owned());

    let mut changes = ChangeSet {
        preserved: vec![
            "duration".to_owned(),
            "video dimensions".to_owned(),
            "selected stream count".to_owned(),
            "metadata".to_owned(),
            "chapters".to_owned(),
        ],
        ..ChangeSet::default()
    };
    if remux {
        changes
            .changed
            .push("container changes to MP4 without media re-encode".to_owned());
    } else {
        if !video_copy {
            changes
                .changed
                .push("video is re-encoded as H.264".to_owned());
        }
        if !audio_copy && !audios.is_empty() {
            changes
                .changed
                .push("audio is re-encoded as AAC".to_owned());
        }
        if !request.preserve_all_streams {
            changes.dropped.extend(
                unsupported_subtitles
                    .iter()
                    .map(|stream| format!("subtitle stream {}", stream.index)),
            );
            changes.dropped.extend(
                other_streams
                    .iter()
                    .map(|stream| format!("non-media stream {}", stream.index)),
            );
        }
    }

    let mut constraints = BTreeMap::new();
    constraints.insert(
        "preserve_all_streams".to_owned(),
        json!(request.preserve_all_streams),
    );

    let step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: if remux {
            "ffmpeg.media-to-mp4.remux"
        } else {
            "ffmpeg.media-to-mp4.transcode"
        }
        .to_owned(),
        engine: ffmpeg.clone(),
        operation,
        loss_class,
        arguments,
        estimated_temporary_bytes: Some(
            probe
                .artifact
                .size_bytes
                .saturating_add(probe.artifact.size_bytes / 10),
        ),
    };
    let validators = vec![
        "media.output-opens".to_owned(),
        "media.target-format".to_owned(),
        "media.duration".to_owned(),
        "media.streams".to_owned(),
    ];
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: Uuid::new_v4(),
        plan_hash: String::new(),
        input_fingerprint: probe.artifact.fast_fingerprint.clone(),
        target_format: target,
        constraints,
        steps: vec![step],
        changes,
        validators,
        network_policy: NetworkPolicy::Deny,
        output_path: request.output_path.clone(),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

#[allow(clippy::too_many_lines)]
fn plan_audio_conversion(
    probe: &Probe,
    request: &PlanRequest,
    ffmpeg: &EngineIdentity,
    target: &str,
) -> Result<Plan> {
    if !matches!(probe.format.kind, FormatKind::Video | FormatKind::Audio) {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            "Audio conversion requires an input containing audio",
            "Choose a media file with a detectable audio stream.",
        ));
    }
    let audios = probe
        .streams
        .iter()
        .filter(|stream| stream.kind == StreamKind::Audio)
        .collect::<Vec<_>>();
    if audios.is_empty() {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "No audio stream was detected",
            "Inspect the input and choose a file containing audio.",
        ));
    }
    if request.preserve_all_streams && audios.len() > 1 {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Plan,
            "A single-file audio target cannot preserve every detected audio stream",
            "Select one audio stream and pass --allow-stream-drop, or create separate outputs.",
        )
        .with_diagnostic(format!("{} audio streams detected", audios.len())));
    }
    let selected = match request.audio_stream_index {
        Some(index) => audios
            .iter()
            .copied()
            .find(|stream| stream.index == index)
            .ok_or_else(|| {
                FormatWrightError::new(
                    ErrorCode::InputInvalid,
                    Stage::Plan,
                    format!("Audio stream index {index} does not exist"),
                    "Run inspect and select one of the reported audio stream indexes.",
                )
            })?,
        None => audios[0],
    };

    let (target_codec, encoder, muxer, lossy) = match target {
        "mp3" => ("mp3", "libmp3lame", "mp3", true),
        "m4a" => ("aac", "aac", "ipod", true),
        "wav" => ("pcm_s16le", "pcm_s16le", "wav", false),
        "flac" => ("flac", "flac", "flac", false),
        "ogg" => ("vorbis", "libvorbis", "ogg", true),
        "opus" => ("opus", "libopus", "opus", true),
        "aac" => ("aac", "aac", "adts", true),
        _ => unreachable!("target dispatch is exhaustive"),
    };
    let can_copy = selected.codec.as_deref() == Some(target_codec);
    let operation = if can_copy {
        Operation::Remux
    } else {
        Operation::Transcode
    };
    let loss_class = if can_copy {
        LossClass::ContainerOnly
    } else if lossy {
        LossClass::Lossy
    } else {
        LossClass::Lossless
    };

    let mut arguments = BTreeMap::new();
    arguments.insert("workflow".to_owned(), "audio-convert".to_owned());
    arguments.insert("audio_stream_index".to_owned(), selected.index.to_string());
    arguments.insert(
        "audio_mode".to_owned(),
        if can_copy { "copy" } else { encoder }.to_owned(),
    );
    arguments.insert("muxer".to_owned(), muxer.to_owned());
    arguments.insert("target_codec".to_owned(), target_codec.to_owned());

    let mut changes = ChangeSet {
        preserved: vec![
            "duration".to_owned(),
            format!("audio stream {}", selected.index),
            "source metadata supported by the target".to_owned(),
        ],
        ..ChangeSet::default()
    };
    if can_copy {
        changes.changed.push(format!(
            "audio stream {} is remuxed without re-encoding",
            selected.index
        ));
    } else {
        changes.changed.push(format!(
            "audio stream {} is encoded as {target_codec}",
            selected.index
        ));
        if !lossy
            && matches!(
                selected.codec.as_deref(),
                Some("mp3" | "aac" | "vorbis" | "opus")
            )
        {
            changes.unknown.push(
                "lossless output does not restore quality already lost in the source".to_owned(),
            );
        }
    }
    changes.dropped.extend(
        probe
            .streams
            .iter()
            .filter(|stream| stream.kind == StreamKind::Video)
            .map(|stream| format!("video stream {}", stream.index)),
    );
    changes.dropped.extend(
        audios
            .iter()
            .filter(|stream| stream.index != selected.index)
            .map(|stream| format!("audio stream {}", stream.index)),
    );
    changes.dropped.extend(
        probe
            .streams
            .iter()
            .filter(|stream| !matches!(stream.kind, StreamKind::Video | StreamKind::Audio))
            .map(|stream| format!("non-audio stream {}", stream.index)),
    );

    let constraints = BTreeMap::from([
        (
            "preserve_all_streams".to_owned(),
            json!(request.preserve_all_streams),
        ),
        ("selected_audio_stream".to_owned(), json!(selected.index)),
    ]);
    let step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: format!(
            "ffmpeg.media-to-{target}.{}",
            if can_copy { "remux" } else { "transcode" }
        ),
        engine: ffmpeg.clone(),
        operation,
        loss_class,
        arguments,
        estimated_temporary_bytes: Some(probe.artifact.size_bytes),
    };
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: Uuid::new_v4(),
        plan_hash: String::new(),
        input_fingerprint: probe.artifact.fast_fingerprint.clone(),
        target_format: target.to_owned(),
        constraints,
        steps: vec![step],
        changes,
        validators: vec![
            "media.output-opens".to_owned(),
            "media.target-format".to_owned(),
            "media.duration".to_owned(),
            "media.audio-stream".to_owned(),
            "media.audio-codec".to_owned(),
        ],
        network_policy: NetworkPolicy::Deny,
        output_path: request.output_path.clone(),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

pub(crate) fn deterministic_plan_hash(plan: &Plan) -> Result<String> {
    #[derive(Serialize)]
    struct HashEngine<'a> {
        engine_id: &'a str,
        version: &'a str,
        binary_sha256: &'a str,
        manifest_sha256: &'a Option<String>,
        certification: Certification,
    }

    #[derive(Serialize)]
    struct HashStep<'a> {
        step_id: &'a str,
        capability_id: &'a str,
        engine: HashEngine<'a>,
        operation: Operation,
        loss_class: LossClass,
        arguments: &'a BTreeMap<String, String>,
        estimated_temporary_bytes: Option<u64>,
    }

    #[derive(Serialize)]
    struct HashMaterial<'a> {
        schema_version: u32,
        input_fingerprint: &'a str,
        target_format: &'a str,
        constraints: &'a BTreeMap<String, serde_json::Value>,
        steps: &'a [HashStep<'a>],
        changes: &'a ChangeSet,
        validators: &'a [String],
        network_policy: NetworkPolicy,
    }

    let steps = plan
        .steps
        .iter()
        .map(|step| HashStep {
            step_id: &step.step_id,
            capability_id: &step.capability_id,
            engine: HashEngine {
                engine_id: &step.engine.engine_id,
                version: &step.engine.version,
                binary_sha256: &step.engine.binary_sha256,
                manifest_sha256: &step.engine.manifest_sha256,
                certification: step.engine.certification,
            },
            operation: step.operation,
            loss_class: step.loss_class,
            arguments: &step.arguments,
            estimated_temporary_bytes: step.estimated_temporary_bytes,
        })
        .collect::<Vec<_>>();
    let material = HashMaterial {
        schema_version: plan.schema_version,
        input_fingerprint: &plan.input_fingerprint,
        target_format: &plan.target_format,
        constraints: &plan.constraints,
        steps: &steps,
        changes: &plan.changes,
        validators: &plan.validators,
        network_policy: plan.network_policy,
    };
    let bytes = serde_json::to_vec(&material).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::Internal,
            Stage::Plan,
            "Unable to serialize deterministic Plan material",
            "Report this as an internal error.",
        )
        .with_diagnostic(error.to_string())
    })?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use formatwright_engine_sdk::{Certification, EngineIdentity, Operation};
    use serde_json::json;

    use super::{plan_conversion, plan_metadata_clean};
    use crate::domain::{
        ArtifactIdentity, FormatDescriptor, FormatKind, MetadataClassification, MetadataEntry,
        PlanRequest, Probe, ProbeEvidence, SCHEMA_VERSION, StreamKind, StreamProbe,
    };

    fn engine() -> EngineIdentity {
        EngineIdentity {
            engine_id: "ffmpeg".to_owned(),
            version: "ffmpeg test".to_owned(),
            binary_path: PathBuf::from("ffmpeg"),
            binary_sha256: "00".repeat(32),
            manifest_sha256: None,
            build_configuration: None,
            certification: Certification::Unverified,
        }
    }

    fn probe(video_codec: &str, audio_codec: Option<&str>) -> Probe {
        let mut streams = vec![StreamProbe {
            index: 0,
            kind: StreamKind::Video,
            codec: Some(video_codec.to_owned()),
            language: None,
            duration_seconds: Some(10.0),
            width: Some(1920),
            height: Some(1080),
            frame_rate: Some("30/1".to_owned()),
            sample_rate: None,
            channels: None,
            properties: BTreeMap::new(),
        }];
        if let Some(codec) = audio_codec {
            streams.push(StreamProbe {
                index: 1,
                kind: StreamKind::Audio,
                codec: Some(codec.to_owned()),
                language: Some("eng".to_owned()),
                duration_seconds: Some(10.0),
                width: None,
                height: None,
                frame_rate: None,
                sample_rate: Some(48_000),
                channels: Some(2),
                properties: BTreeMap::new(),
            });
        }
        Probe {
            schema_version: SCHEMA_VERSION,
            artifact: ArtifactIdentity {
                display_path: "input.mkv".to_owned(),
                canonical_path: PathBuf::from("input.mkv"),
                size_bytes: 1000,
                modified_unix_ms: 1,
                fast_fingerprint: "fwfp-v1:test".to_owned(),
                full_blake3: None,
            },
            format: FormatDescriptor {
                id: "mkv".to_owned(),
                kind: FormatKind::Video,
                mime_type: None,
                container: Some("matroska,webm".to_owned()),
                extension_matches: Some(true),
                confidence: 1.0,
            },
            streams,
            metadata: BTreeMap::new(),
            warnings: Vec::new(),
            evidence: ProbeEvidence {
                engine_id: "ffprobe".to_owned(),
                engine_version: "test".to_owned(),
                engine_binary_sha256: None,
            },
            duration_seconds: Some(10.0),
            bit_rate: None,
        }
    }

    fn request() -> PlanRequest {
        PlanRequest {
            target_format: "mp4".to_owned(),
            output_path: Some(PathBuf::from("output.mp4")),
            preserve_all_streams: true,
            audio_stream_index: None,
            start_millis: None,
            duration_millis: None,
            width: None,
            quality: None,
            dpi: None,
            color_mode: None,
            frames_per_second: None,
            loop_count: None,
            allow_lossy_data: false,
        }
    }

    fn image_probe(alpha: bool) -> Probe {
        let mut source = probe("png", None);
        source.artifact.display_path = "input.png".to_owned();
        source.artifact.canonical_path = PathBuf::from("input.png");
        source.format.id = "png".to_owned();
        source.format.kind = FormatKind::Image;
        source.format.container = Some("png_pipe".to_owned());
        source.duration_seconds = None;
        source.streams[0].duration_seconds = None;
        source.streams[0].frame_rate = Some("25/1".to_owned());
        source.streams[0].properties.insert(
            "pix_fmt".to_owned(),
            json!(if alpha { "rgba" } else { "rgb24" }),
        );
        source
    }

    #[test]
    fn h264_and_aac_choose_remux() {
        let plan = plan_conversion(&probe("h264", Some("aac")), &request(), &engine())
            .expect("remux plan");
        assert_eq!(plan.steps[0].operation, Operation::Remux);
        assert_eq!(plan.steps[0].arguments["video_mode"], "copy");
        assert_eq!(plan.steps[0].arguments["audio_mode"], "copy");
    }

    #[test]
    fn vp9_and_opus_choose_transcode() {
        let plan = plan_conversion(&probe("vp9", Some("opus")), &request(), &engine())
            .expect("transcode plan");
        assert_eq!(plan.steps[0].operation, Operation::Transcode);
        assert_eq!(plan.steps[0].arguments["video_mode"], "libx264");
        assert_eq!(plan.steps[0].arguments["audio_mode"], "aac");
    }

    #[test]
    fn deterministic_hash_excludes_random_plan_id() {
        let first = plan_conversion(&probe("h264", Some("aac")), &request(), &engine())
            .expect("first plan");
        let second = plan_conversion(&probe("h264", Some("aac")), &request(), &engine())
            .expect("second plan");
        assert_ne!(first.plan_id, second.plan_id);
        assert_eq!(first.plan_hash, second.plan_hash);
    }

    #[test]
    fn deterministic_hash_excludes_install_and_output_paths() {
        let source = probe("h264", Some("aac"));
        let first = plan_conversion(&source, &request(), &engine()).expect("first plan");
        let mut other_engine = engine();
        other_engine.binary_path = PathBuf::from("another/install/ffmpeg");
        let mut other_request = request();
        other_request.output_path = Some(PathBuf::from("another/output.mp4"));
        let second = plan_conversion(&source, &other_request, &other_engine).expect("second plan");
        assert_eq!(first.plan_hash, second.plan_hash);
    }

    #[test]
    fn deterministic_hash_changes_with_engine_identity() {
        let source = probe("h264", Some("aac"));
        let first = plan_conversion(&source, &request(), &engine()).expect("first plan");
        let mut changed_engine = engine();
        changed_engine.version = "ffmpeg changed".to_owned();
        changed_engine.binary_sha256 = "11".repeat(32);
        let second = plan_conversion(&source, &request(), &changed_engine).expect("second plan");
        assert_ne!(first.plan_hash, second.plan_hash);
    }

    #[test]
    fn preservation_policy_blocks_incompatible_subtitles() {
        let mut source = probe("h264", Some("aac"));
        source.streams.push(StreamProbe {
            index: 2,
            kind: StreamKind::Subtitle,
            codec: Some("subrip".to_owned()),
            language: Some("eng".to_owned()),
            duration_seconds: Some(10.0),
            width: None,
            height: None,
            frame_rate: None,
            sample_rate: None,
            channels: None,
            properties: BTreeMap::new(),
        });
        let error = plan_conversion(&source, &request(), &engine())
            .expect_err("preserve-all cannot silently drop SubRip");
        assert_eq!(error.code, crate::ErrorCode::PolicyBlocked);
        assert_eq!(error.stage, crate::Stage::Plan);
        assert_eq!(
            error.diagnostic.as_deref(),
            Some("1 unsupported subtitle stream(s), 0 attachment/data/unknown stream(s)")
        );
    }

    #[test]
    fn video_audio_can_be_planned_to_mp3() {
        let mut audio_request = request();
        audio_request.target_format = "mp3".to_owned();
        audio_request.output_path = Some(PathBuf::from("output.mp3"));
        let plan = plan_conversion(&probe("h264", Some("aac")), &audio_request, &engine())
            .expect("audio extraction plan");
        assert_eq!(plan.target_format, "mp3");
        assert_eq!(plan.steps[0].operation, Operation::Transcode);
        assert_eq!(plan.steps[0].arguments["audio_mode"], "libmp3lame");
        assert_eq!(plan.steps[0].arguments["audio_stream_index"], "1");
        assert!(plan.changes.dropped.contains(&"video stream 0".to_owned()));
    }

    #[test]
    fn audio_target_rejects_input_without_audio() {
        let mut audio_request = request();
        audio_request.target_format = "wav".to_owned();
        let error = plan_conversion(&probe("h264", None), &audio_request, &engine())
            .expect_err("video without audio must be rejected");
        assert_eq!(error.code, crate::ErrorCode::InputInvalid);
    }

    #[test]
    fn explicit_audio_selection_requires_drop_permission_for_multiple_tracks() {
        let mut source = probe("h264", Some("aac"));
        let mut second = source.streams[1].clone();
        second.index = 2;
        second.language = Some("zho".to_owned());
        source.streams.push(second);
        let mut audio_request = request();
        audio_request.target_format = "m4a".to_owned();
        audio_request.audio_stream_index = Some(2);
        let error = plan_conversion(&source, &audio_request, &engine())
            .expect_err("preserve-all must not silently drop another audio track");
        assert_eq!(error.code, crate::ErrorCode::PolicyBlocked);

        audio_request.preserve_all_streams = false;
        let plan = plan_conversion(&source, &audio_request, &engine())
            .expect("explicit selection with drop permission");
        assert_eq!(plan.steps[0].arguments["audio_stream_index"], "2");
    }

    #[test]
    fn gif_plan_records_bounded_visual_parameters() {
        let mut gif_request = request();
        gif_request.target_format = "gif".to_owned();
        gif_request.start_millis = Some(500);
        gif_request.duration_millis = Some(2_000);
        gif_request.width = Some(640);
        gif_request.frames_per_second = Some(12);
        gif_request.loop_count = Some(3);
        let mut source = probe("h264", Some("aac"));
        source.duration_seconds = Some(4.0);
        let plan = plan_conversion(&source, &gif_request, &engine()).expect("GIF plan");
        assert_eq!(plan.steps[0].operation, Operation::Transcode);
        assert_eq!(plan.steps[0].arguments["frames_per_second"], "12");
        assert_eq!(plan.steps[0].arguments["width"], "640");
        assert_eq!(plan.constraints["loop_count"], json!(3));
    }

    #[test]
    fn gif_plan_rejects_invalid_time_and_frame_rate() {
        let mut gif_request = request();
        gif_request.target_format = "gif".to_owned();
        gif_request.duration_millis = Some(0);
        assert_eq!(
            plan_conversion(&probe("h264", None), &gif_request, &engine())
                .expect_err("zero duration")
                .code,
            crate::ErrorCode::InputInvalid
        );
        gif_request.duration_millis = Some(1_000);
        gif_request.frames_per_second = Some(61);
        assert_eq!(
            plan_conversion(&probe("h264", None), &gif_request, &engine())
                .expect_err("unbounded frame rate")
                .code,
            crate::ErrorCode::InputInvalid
        );
    }

    #[test]
    fn image_plan_records_quality_resize_and_codec() {
        let mut image_request = request();
        image_request.target_format = "webp".to_owned();
        image_request.width = Some(640);
        image_request.quality = Some(88);
        let plan = plan_conversion(&image_probe(false), &image_request, &engine())
            .expect("WebP image plan");
        assert_eq!(plan.target_format, "webp");
        assert_eq!(plan.steps[0].arguments["codec"], "libwebp");
        assert_eq!(plan.steps[0].arguments["quality"], "88");
        assert_eq!(plan.constraints["width"], json!(640));
    }

    #[test]
    fn image_plan_blocks_implicit_alpha_loss() {
        let mut image_request = request();
        image_request.target_format = "jpg".to_owned();
        let error = plan_conversion(&image_probe(true), &image_request, &engine())
            .expect_err("JPEG must not silently drop alpha");
        assert_eq!(error.code, crate::ErrorCode::PolicyBlocked);
    }

    #[test]
    fn metadata_clean_plan_names_keys_without_values() {
        let mut source = probe("h264", Some("aac"));
        source.metadata.insert(
            "title".to_owned(),
            MetadataEntry {
                value: json!("private value"),
                classification: MetadataClassification::Private,
            },
        );
        source.metadata.insert(
            "CUSTOM_TAG".to_owned(),
            MetadataEntry {
                value: json!("retained value"),
                classification: MetadataClassification::Unknown,
            },
        );
        let plan = plan_metadata_clean(&source, PathBuf::from("clean.mkv"), &engine())
            .expect("metadata clean plan");
        let serialized = serde_json::to_string(&plan).expect("serialize Plan");
        assert_eq!(plan.steps[0].operation, Operation::MetadataClean);
        assert_eq!(plan.constraints["removed_metadata_keys"], json!(["title"]));
        assert_eq!(
            plan.constraints["retained_metadata_keys"],
            json!(["CUSTOM_TAG"])
        );
        assert!(!serialized.contains("private value"));
        assert!(!serialized.contains("retained value"));
    }
}
