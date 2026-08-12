use formatwright_engine_sdk::Operation;
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

use crate::domain::{
    ArtifactSummary, Plan, Probe, ReportRedaction, SCHEMA_VERSION, StreamKind, ValidationCheck,
    ValidationReport, ValidationStatus,
};

#[allow(clippy::too_many_lines)]
pub fn validate_media_output(
    input: &Probe,
    output: &Probe,
    plan: &Plan,
    job_id: Uuid,
) -> ValidationReport {
    let mut checks = Vec::new();

    checks.push(check(
        "MEDIA_OUTPUT_OPENS",
        ValidationStatus::Pass,
        true,
        json!(true),
        json!(true),
        "ffprobe parsed the complete output metadata",
        "Output opens with ffprobe.",
    ));

    let format_matches = output.format.id == plan.target_format;
    checks.push(check(
        "MEDIA_TARGET_FORMAT",
        if format_matches {
            ValidationStatus::Pass
        } else {
            ValidationStatus::Fail
        },
        true,
        json!(plan.target_format),
        json!(output.format.id),
        "ffprobe format detection",
        if format_matches {
            "Detected output format matches the Plan."
        } else {
            "Detected output format does not match the Plan."
        },
    ));

    let operation = plan
        .steps
        .first()
        .map_or(Operation::Transcode, |step| step.operation);
    if !is_image_target(&plan.target_format) {
        checks.push(duration_check(
            expected_duration(input.duration_seconds, plan),
            output.duration_seconds,
            operation,
        ));
    }

    for kind in [StreamKind::Video, StreamKind::Audio, StreamKind::Subtitle] {
        let expected = stream_count(input, kind, plan);
        let observed = output
            .streams
            .iter()
            .filter(|stream| stream.kind == kind)
            .count();
        let matches = expected == observed;
        checks.push(check(
            match kind {
                StreamKind::Video => "MEDIA_VIDEO_STREAM_COUNT",
                StreamKind::Audio => "MEDIA_AUDIO_STREAM_COUNT",
                StreamKind::Subtitle => "MEDIA_SUBTITLE_STREAM_COUNT",
                _ => unreachable!("only selected stream kinds are validated"),
            },
            if matches {
                ValidationStatus::Pass
            } else {
                ValidationStatus::Fail
            },
            true,
            json!(expected),
            json!(observed),
            "ffprobe stream inventory",
            if matches {
                "Stream count matches the Plan."
            } else {
                "Stream count differs from the Plan."
            },
        ));
    }

    if matches!(
        plan.target_format.as_str(),
        "mp4" | "gif" | "jpeg" | "png" | "webp" | "avif"
    ) {
        let input_video = input
            .streams
            .iter()
            .find(|stream| stream.kind == StreamKind::Video);
        let output_video = output
            .streams
            .iter()
            .find(|stream| stream.kind == StreamKind::Video);
        let expected_dimensions =
            if plan.target_format == "gif" || is_image_target(&plan.target_format) {
                expected_scaled_width_dimensions(input_video, plan)
            } else {
                input_video.map(|stream| (stream.width, stream.height))
            };
        let observed_dimensions = output_video.map(|stream| (stream.width, stream.height));
        let dimensions_match = expected_dimensions == observed_dimensions;
        checks.push(check(
            "MEDIA_DIMENSIONS",
            if dimensions_match {
                ValidationStatus::Pass
            } else {
                ValidationStatus::Fail
            },
            true,
            json!(expected_dimensions),
            json!(observed_dimensions),
            "ffprobe video stream dimensions",
            if dimensions_match {
                "Video dimensions are preserved."
            } else {
                "Video dimensions changed without a scaling constraint."
            },
        ));
    }

    if let Some(expected_codec) = expected_image_codec(&plan.target_format) {
        let output_image = output
            .streams
            .iter()
            .find(|stream| stream.kind == StreamKind::Video);
        let observed_codec = output_image.and_then(|stream| stream.codec.as_deref());
        let codec_matches = observed_codec == Some(expected_codec);
        checks.push(check(
            "IMAGE_CODEC",
            if codec_matches {
                ValidationStatus::Pass
            } else {
                ValidationStatus::Fail
            },
            true,
            json!(expected_codec),
            json!(observed_codec),
            "ffprobe image codec",
            if codec_matches {
                "Image codec matches the Plan."
            } else {
                "Image codec does not match the Plan."
            },
        ));
        let alpha_required = plan
            .constraints
            .get("preserve_alpha")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let alpha_observed = output_image
            .and_then(|stream| stream.properties.get("pix_fmt"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(pixel_format_has_alpha);
        checks.push(check(
            "IMAGE_ALPHA",
            if !alpha_required || alpha_observed {
                ValidationStatus::Pass
            } else {
                ValidationStatus::Fail
            },
            true,
            json!(alpha_required),
            json!(alpha_observed),
            "ffprobe pixel format",
            if !alpha_required || alpha_observed {
                "Alpha-channel policy is satisfied."
            } else {
                "The output lost a required alpha channel."
            },
        ));
    }

    if let Some(expected_codec) = expected_audio_codec(&plan.target_format) {
        let observed_codec = output
            .streams
            .iter()
            .find(|stream| stream.kind == StreamKind::Audio)
            .and_then(|stream| stream.codec.as_deref());
        let codec_matches = observed_codec == Some(expected_codec);
        checks.push(check(
            "MEDIA_AUDIO_CODEC",
            if codec_matches {
                ValidationStatus::Pass
            } else {
                ValidationStatus::Fail
            },
            true,
            json!(expected_codec),
            json!(observed_codec),
            "ffprobe audio codec",
            if codec_matches {
                "Audio codec matches the Plan."
            } else {
                "Audio codec does not match the Plan."
            },
        ));
    }

    if operation == Operation::MetadataClean {
        let removed = constraint_strings(plan, "removed_metadata_keys");
        let retained = constraint_strings(plan, "retained_metadata_keys");
        let still_present = removed
            .iter()
            .filter(|key| output.metadata.contains_key(key.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        checks.push(check(
            "METADATA_REMOVED_KEYS",
            if still_present.is_empty() {
                ValidationStatus::Pass
            } else {
                ValidationStatus::Fail
            },
            true,
            json!(removed),
            json!(still_present),
            "ffprobe format metadata inventory",
            if still_present.is_empty() {
                "Every metadata key named by the Plan was removed."
            } else {
                "One or more metadata keys named by the Plan remain."
            },
        ));
        let missing_retained = retained
            .iter()
            .filter(|key| !output.metadata.contains_key(key.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        checks.push(check(
            "METADATA_RETAINED_KEYS",
            if missing_retained.is_empty() {
                ValidationStatus::Pass
            } else {
                ValidationStatus::Fail
            },
            true,
            json!(retained),
            json!(missing_retained),
            "ffprobe format metadata inventory",
            if missing_retained.is_empty() {
                "Structural and unknown metadata keys selected for retention remain."
            } else {
                "One or more metadata keys selected for retention are missing."
            },
        ));
        let expected_payload = payload_signature(input);
        let observed_payload = payload_signature(output);
        let payload_matches = expected_payload == observed_payload;
        checks.push(check(
            "METADATA_PAYLOAD_SIGNATURE",
            if payload_matches {
                ValidationStatus::Pass
            } else {
                ValidationStatus::Fail
            },
            true,
            expected_payload,
            observed_payload,
            "ffprobe stream codecs and dimensions",
            if payload_matches {
                "Encoded payload stream identities are unchanged."
            } else {
                "Metadata cleaning changed an encoded payload stream."
            },
        ));
    }

    let status = aggregate_status(&checks);
    ValidationReport {
        schema_version: SCHEMA_VERSION,
        report_id: Uuid::new_v4(),
        job_id,
        plan_hash: plan.plan_hash.clone(),
        status,
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

fn duration_check(
    expected: Option<f64>,
    observed: Option<f64>,
    operation: Operation,
) -> ValidationCheck {
    let tolerance = match operation {
        Operation::Remux => 0.050,
        _ => 0.250,
    };
    match (expected, observed) {
        (Some(expected), Some(observed)) => {
            let difference = (expected - observed).abs();
            let matches = difference <= tolerance;
            check(
                "MEDIA_DURATION",
                if matches {
                    ValidationStatus::Pass
                } else {
                    ValidationStatus::Fail
                },
                true,
                json!({"seconds": expected, "tolerance_seconds": tolerance}),
                json!({"seconds": observed, "difference_seconds": difference}),
                "ffprobe container duration",
                if matches {
                    "Duration is within the Plan tolerance."
                } else {
                    "Duration differs beyond the Plan tolerance."
                },
            )
        }
        _ => check(
            "MEDIA_DURATION",
            ValidationStatus::Unknown,
            true,
            json!(expected),
            json!(observed),
            "ffprobe container duration",
            "Duration was unavailable and cannot be marked Pass.",
        ),
    }
}

fn stream_count(input: &Probe, kind: StreamKind, plan: &Plan) -> usize {
    let input_count = input
        .streams
        .iter()
        .filter(|stream| stream.kind == kind)
        .count();
    if expected_audio_codec(&plan.target_format).is_some() {
        return match kind {
            StreamKind::Audio => 1,
            StreamKind::Video | StreamKind::Subtitle => 0,
            _ => input_count,
        };
    }
    if is_image_target(&plan.target_format) {
        return match kind {
            StreamKind::Video => 1,
            StreamKind::Audio | StreamKind::Subtitle => 0,
            _ => input_count,
        };
    }
    if plan.target_format == "gif" {
        return match kind {
            StreamKind::Video => 1,
            StreamKind::Audio | StreamKind::Subtitle => 0,
            _ => input_count,
        };
    }
    if kind != StreamKind::Subtitle {
        return input_count;
    }
    let subtitle_mode = plan
        .steps
        .first()
        .and_then(|step| step.arguments.get("subtitle_mode"))
        .map_or("none", String::as_str);
    if subtitle_mode == "copy" {
        input_count
    } else {
        0
    }
}

fn expected_duration(input_seconds: Option<f64>, plan: &Plan) -> Option<f64> {
    if plan.target_format != "gif" {
        return input_seconds;
    }
    let duration_millis = plan
        .constraints
        .get("duration_millis")
        .and_then(serde_json::Value::as_u64);
    if let Some(duration_millis) = duration_millis {
        return Some(Duration::from_millis(duration_millis).as_secs_f64());
    }
    let start_millis = plan
        .constraints
        .get("start_millis")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    input_seconds
        .map(|seconds| (seconds - Duration::from_millis(start_millis).as_secs_f64()).max(0.0))
}

fn expected_scaled_width_dimensions(
    input_video: Option<&crate::domain::StreamProbe>,
    plan: &Plan,
) -> Option<(Option<u32>, Option<u32>)> {
    let input_video = input_video?;
    let requested_width = plan
        .constraints
        .get("width")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let Some(width) = requested_width else {
        return Some((input_video.width, input_video.height));
    };
    let height =
        input_video
            .width
            .zip(input_video.height)
            .and_then(|(input_width, input_height)| {
                if input_width == 0 {
                    return None;
                }
                let numerator = u64::from(input_height) * u64::from(width);
                let divisor = u64::from(input_width);
                let rounded = (numerator + divisor / 2) / divisor;
                let even = if rounded % 2 == 0 {
                    rounded
                } else {
                    rounded + 1
                };
                u32::try_from(even.max(2)).ok()
            });
    Some((Some(width), height))
}

fn is_image_target(target: &str) -> bool {
    matches!(target, "jpeg" | "png" | "webp" | "avif")
}

fn expected_image_codec(target: &str) -> Option<&'static str> {
    match target {
        "jpeg" => Some("mjpeg"),
        "png" => Some("png"),
        "webp" => Some("webp"),
        "avif" => Some("av1"),
        _ => None,
    }
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

fn constraint_strings(plan: &Plan, name: &str) -> Vec<String> {
    plan.constraints
        .get(name)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn payload_signature(probe: &Probe) -> serde_json::Value {
    json!(
        probe
            .streams
            .iter()
            .map(|stream| (
                stream.kind,
                stream.codec.as_deref(),
                stream.width,
                stream.height,
                stream.sample_rate,
                stream.channels,
            ))
            .collect::<Vec<_>>()
    )
}

fn expected_audio_codec(target: &str) -> Option<&'static str> {
    match target {
        "mp3" => Some("mp3"),
        "m4a" | "aac" => Some("aac"),
        "wav" => Some("pcm_s16le"),
        "flac" => Some("flac"),
        "ogg" => Some("vorbis"),
        "opus" => Some("opus"),
        _ => None,
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

fn aggregate_status(checks: &[ValidationCheck]) -> ValidationStatus {
    checks.iter().fold(ValidationStatus::Pass, |status, item| {
        if item.required {
            status.worst(item.status)
        } else {
            status
        }
    })
}

fn check(
    code: &str,
    status: ValidationStatus,
    required: bool,
    expected: serde_json::Value,
    observed: serde_json::Value,
    evidence: &str,
    message: &str,
) -> ValidationCheck {
    ValidationCheck {
        code: code.to_owned(),
        status,
        required,
        expected,
        observed,
        evidence: evidence.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use formatwright_engine_sdk::{Certification, EngineIdentity, LossClass, Operation};
    use uuid::Uuid;

    use super::validate_media_output;
    use crate::domain::{
        ArtifactIdentity, ChangeSet, FormatDescriptor, FormatKind, NetworkPolicy, Plan, PlanStep,
        Probe, ProbeEvidence, SCHEMA_VERSION, StreamKind, StreamProbe, ValidationStatus,
    };

    fn probe(path: &str, format: &str, duration: f64) -> Probe {
        Probe {
            schema_version: SCHEMA_VERSION,
            artifact: ArtifactIdentity {
                display_path: path.to_owned(),
                canonical_path: PathBuf::from(path),
                size_bytes: 100,
                modified_unix_ms: 1,
                fast_fingerprint: format!("fwfp-v1:{path}"),
                full_blake3: None,
            },
            format: FormatDescriptor {
                id: format.to_owned(),
                kind: FormatKind::Video,
                mime_type: None,
                container: None,
                extension_matches: Some(true),
                confidence: 1.0,
            },
            streams: vec![StreamProbe {
                index: 0,
                kind: StreamKind::Video,
                codec: Some("h264".to_owned()),
                language: None,
                duration_seconds: Some(duration),
                width: Some(1920),
                height: Some(1080),
                frame_rate: Some("30/1".to_owned()),
                sample_rate: None,
                channels: None,
                properties: BTreeMap::new(),
            }],
            metadata: BTreeMap::new(),
            warnings: Vec::new(),
            evidence: ProbeEvidence {
                engine_id: "ffprobe".to_owned(),
                engine_version: "test".to_owned(),
                engine_binary_sha256: None,
            },
            duration_seconds: Some(duration),
            bit_rate: None,
        }
    }

    fn plan() -> Plan {
        Plan {
            schema_version: SCHEMA_VERSION,
            plan_id: Uuid::new_v4(),
            plan_hash: "blake3:test".to_owned(),
            input_fingerprint: "fwfp-v1:input".to_owned(),
            target_format: "mp4".to_owned(),
            constraints: BTreeMap::new(),
            steps: vec![PlanStep {
                step_id: "step-1".to_owned(),
                capability_id: "test".to_owned(),
                engine: EngineIdentity {
                    engine_id: "ffmpeg".to_owned(),
                    version: "test".to_owned(),
                    binary_path: PathBuf::from("ffmpeg"),
                    binary_sha256: "00".repeat(32),
                    manifest_sha256: None,
                    build_configuration: None,
                    certification: Certification::Unverified,
                },
                operation: Operation::Remux,
                loss_class: LossClass::ContainerOnly,
                arguments: BTreeMap::from([("subtitle_mode".to_owned(), "none".to_owned())]),
                estimated_temporary_bytes: None,
            }],
            changes: ChangeSet::default(),
            validators: Vec::new(),
            network_policy: NetworkPolicy::Deny,
            output_path: Some(PathBuf::from("output.mp4")),
            estimated_output_bytes: None,
        }
    }

    #[test]
    fn matching_media_passes() {
        let report = validate_media_output(
            &probe("input.mkv", "mkv", 10.0),
            &probe("output.mp4", "mp4", 10.01),
            &plan(),
            Uuid::new_v4(),
        );
        assert_eq!(report.status, ValidationStatus::Pass);
    }

    #[test]
    fn duration_drift_fails() {
        let report = validate_media_output(
            &probe("input.mkv", "mkv", 10.0),
            &probe("output.mp4", "mp4", 11.0),
            &plan(),
            Uuid::new_v4(),
        );
        assert_eq!(report.status, ValidationStatus::Fail);
    }
}
