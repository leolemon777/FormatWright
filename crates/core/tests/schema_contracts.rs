use std::collections::BTreeMap;
use std::path::PathBuf;

use formatwright_core::{
    APPLICATION_STATE_BUNDLE_SCHEMA_VERSION, ApplicationSettings, ArtifactIdentity,
    ArtifactSummary, Certification, ChangeSet, ConversionPreset, EngineIdentity, FormatDescriptor,
    FormatKind, JobEventRecord, JobProgress, JobState, MetadataEntry, NetworkPolicy, Operation,
    PRESET_SCHEMA_VERSION, Plan, PlanStep, PresetLibrary, Probe, ProbeEvidence, ReportRedaction,
    StateBundleComponent, StateBundleComponents, StateBundleEntry, StateBundleManifest, StreamKind,
    StreamProbe, ValidationCheck, ValidationReport, ValidationStatus,
};
use formatwright_engine_sdk::LossClass;
use formatwright_engine_sdk::{
    Capability, EngineArchitecture, EngineManifest, EnginePlatform, FormatWrightCompatibility,
    ManifestExecutable, ManifestLicense, ManifestSource,
};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

const PROBE_SCHEMA: &str = include_str!("../../../schemas/probe/v1.schema.json");
const PLAN_SCHEMA: &str = include_str!("../../../schemas/plan/v1.schema.json");
const JOB_EVENT_SCHEMA: &str = include_str!("../../../schemas/job-event/v1.schema.json");
const VALIDATION_REPORT_SCHEMA: &str =
    include_str!("../../../schemas/validation-report/v1.schema.json");
const ENGINE_MANIFEST_SCHEMA: &str =
    include_str!("../../../schemas/engine-manifest/v1.schema.json");
const PRESET_LIBRARY_SCHEMA: &str = include_str!("../../../schemas/preset-library/v1.schema.json");
const APPLICATION_STATE_MANIFEST_SCHEMA: &str =
    include_str!("../../../schemas/application-state-manifest/v1.schema.json");
const APPLICATION_SETTINGS_SCHEMA: &str =
    include_str!("../../../schemas/application-settings/v1.schema.json");

fn engine() -> EngineIdentity {
    EngineIdentity {
        engine_id: "ffmpeg".to_owned(),
        version: "8.1.1".to_owned(),
        binary_path: PathBuf::from("engines/ffmpeg/bin/ffmpeg"),
        binary_sha256: "ab".repeat(32),
        manifest_sha256: Some("cd".repeat(32)),
        build_configuration: Some("--disable-network".to_owned()),
        certification: Certification::Experimental,
    }
}

fn probe() -> Probe {
    Probe {
        schema_version: 1,
        artifact: ArtifactIdentity {
            display_path: "clip.mkv".to_owned(),
            canonical_path: PathBuf::from("/fixtures/clip.mkv"),
            size_bytes: 4_096,
            modified_unix_ms: 1_786_349_600_000,
            fast_fingerprint: "fwfp-v1:test".to_owned(),
            full_blake3: None,
        },
        format: FormatDescriptor {
            id: "mkv".to_owned(),
            kind: FormatKind::Video,
            mime_type: Some("video/x-matroska".to_owned()),
            container: Some("matroska,webm".to_owned()),
            extension_matches: Some(true),
            confidence: 1.0,
        },
        streams: vec![StreamProbe {
            index: 0,
            kind: StreamKind::Video,
            codec: Some("h264".to_owned()),
            language: None,
            duration_seconds: Some(2.0),
            width: Some(320),
            height: Some(240),
            frame_rate: Some("24/1".to_owned()),
            sample_rate: None,
            channels: None,
            properties: BTreeMap::from([("pix_fmt".to_owned(), json!("yuv420p"))]),
        }],
        metadata: BTreeMap::from([(
            "title".to_owned(),
            MetadataEntry {
                value: json!("fixture"),
                classification: formatwright_core::domain::MetadataClassification::Private,
            },
        )]),
        warnings: Vec::new(),
        evidence: ProbeEvidence {
            engine_id: "ffprobe".to_owned(),
            engine_version: "8.1.1".to_owned(),
            engine_binary_sha256: Some("ef".repeat(32)),
        },
        duration_seconds: Some(2.0),
        bit_rate: Some(500_000),
    }
}

fn plan() -> Plan {
    Plan {
        schema_version: 1,
        plan_id: Uuid::new_v4(),
        plan_hash: format!("blake3:{}", "01".repeat(32)),
        input_fingerprint: "fwfp-v1:test".to_owned(),
        target_format: "mp4".to_owned(),
        constraints: BTreeMap::from([("preserve_all_streams".to_owned(), json!(true))]),
        steps: vec![PlanStep {
            step_id: "step-1".to_owned(),
            capability_id: "ffmpeg.media-to-mp4.remux".to_owned(),
            engine: engine(),
            operation: Operation::Remux,
            loss_class: LossClass::ContainerOnly,
            arguments: BTreeMap::from([("video_mode".to_owned(), "copy".to_owned())]),
            estimated_temporary_bytes: Some(4_500),
        }],
        changes: ChangeSet {
            preserved: vec!["video stream".to_owned()],
            changed: vec!["container".to_owned()],
            dropped: Vec::new(),
            unknown: Vec::new(),
        },
        validators: vec!["media.output-opens".to_owned()],
        network_policy: NetworkPolicy::Deny,
        output_path: Some(PathBuf::from("/fixtures/output.mp4")),
        estimated_output_bytes: Some(4_096),
    }
}

fn validation_report() -> ValidationReport {
    ValidationReport {
        schema_version: 1,
        report_id: Uuid::new_v4(),
        job_id: Uuid::new_v4(),
        plan_hash: plan().plan_hash,
        status: ValidationStatus::Pass,
        input: ArtifactSummary {
            display_path: Some("clip.mkv".to_owned()),
            format_id: "mkv".to_owned(),
            size_bytes: 4_096,
            fast_fingerprint: "fwfp-v1:input".to_owned(),
            full_blake3: None,
        },
        output: ArtifactSummary {
            display_path: Some("output.mp4".to_owned()),
            format_id: "mp4".to_owned(),
            size_bytes: 4_000,
            fast_fingerprint: "fwfp-v1:output".to_owned(),
            full_blake3: None,
        },
        engines: vec![engine()],
        checks: vec![ValidationCheck {
            code: "MEDIA_OUTPUT_OPENS".to_owned(),
            status: ValidationStatus::Pass,
            required: true,
            expected: json!(true),
            observed: json!(true),
            evidence: "independent probe".to_owned(),
            message: "Output opens.".to_owned(),
        }],
        intentional_changes: vec!["container".to_owned()],
        redaction: ReportRedaction {
            paths_redacted: false,
            metadata_values_redacted: true,
        },
    }
}

fn job_event() -> JobEventRecord {
    let job_id = Uuid::new_v4();
    JobEventRecord {
        schema_version: 1,
        event_id: Uuid::new_v4(),
        job_id,
        sequence: 3,
        previous_state: Some(JobState::Running),
        next_state: JobState::Validating,
        code: "ENGINE_FINISHED".to_owned(),
        timestamp_unix_ms: 1_786_349_600_000,
        progress: Some(JobProgress {
            completed: 1.0,
            total: Some(1.0),
            unit: "engine".to_owned(),
        }),
        data: BTreeMap::new(),
    }
}

fn engine_manifest() -> EngineManifest {
    EngineManifest {
        schema_version: 1,
        engine_id: "fixture-engine".to_owned(),
        version: "1.0.0".to_owned(),
        platform: EnginePlatform::current().expect("supported test platform"),
        architecture: EngineArchitecture::current().expect("supported test architecture"),
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

fn preset_library() -> PresetLibrary {
    PresetLibrary {
        schema_version: PRESET_SCHEMA_VERSION,
        presets: vec![ConversionPreset {
            schema_version: PRESET_SCHEMA_VERSION,
            preset_id: Uuid::new_v4(),
            name: "Smaller image".to_owned(),
            target_format: "webp".to_owned(),
            quality: Some(78),
            width: Some(1920),
            dpi: None,
            color_mode: Some("rgb".to_owned()),
            preserve_all_streams: true,
        }],
    }
}

fn application_state_manifest() -> StateBundleManifest {
    StateBundleManifest {
        schema_version: APPLICATION_STATE_BUNDLE_SCHEMA_VERSION,
        bundle_id: Uuid::new_v4(),
        created_unix_seconds: 1_786_349_600_u64,
        application_version: "0.1.0".to_owned(),
        components: StateBundleComponents {
            database: true,
            presets: true,
            settings: true,
            engine_registry: true,
            reports: false,
        },
        entries: vec![
            StateBundleEntry {
                path: "database/jobs.sqlite3".to_owned(),
                component: StateBundleComponent::Database,
                size_bytes: 4096,
                sha256: "ab".repeat(32),
            },
            StateBundleEntry {
                path: "presets/presets.json".to_owned(),
                component: StateBundleComponent::Presets,
                size_bytes: 128,
                sha256: "cd".repeat(32),
            },
            StateBundleEntry {
                path: "settings/settings.json".to_owned(),
                component: StateBundleComponent::Settings,
                size_bytes: 64,
                sha256: "ef".repeat(32),
            },
            StateBundleEntry {
                path: "engine-registry/media.json".to_owned(),
                component: StateBundleComponent::EngineRegistry,
                size_bytes: 96,
                sha256: "01".repeat(32),
            },
        ],
    }
}

fn assert_contract<T: Serialize>(schema_source: &str, instance: &T) {
    let schema: Value = serde_json::from_str(schema_source).expect("schema is JSON");
    jsonschema::draft202012::meta::validate(&schema).expect("schema satisfies meta-schema");
    let validator = jsonschema::draft202012::new(&schema).expect("schema compiles");
    let value = serde_json::to_value(instance).expect("instance serializes");
    if let Err(error) = validator.validate(&value) {
        panic!("instance does not satisfy schema: {error}\n{value:#}");
    }
}

#[test]
fn rust_probe_matches_public_schema() {
    assert_contract(PROBE_SCHEMA, &probe());
}

#[test]
fn rust_plan_matches_public_schema() {
    assert_contract(PLAN_SCHEMA, &plan());
}

#[test]
fn rust_job_event_matches_public_schema() {
    assert_contract(JOB_EVENT_SCHEMA, &job_event());
}

#[test]
fn rust_validation_report_matches_public_schema() {
    assert_contract(VALIDATION_REPORT_SCHEMA, &validation_report());
}

#[test]
fn rust_engine_manifest_matches_public_schema() {
    assert_contract(ENGINE_MANIFEST_SCHEMA, &engine_manifest());
}

#[test]
fn rust_preset_library_matches_public_schema() {
    assert_contract(PRESET_LIBRARY_SCHEMA, &preset_library());
}

#[test]
fn rust_application_state_manifest_matches_public_schema() {
    assert_contract(
        APPLICATION_STATE_MANIFEST_SCHEMA,
        &application_state_manifest(),
    );
}

#[test]
fn rust_application_settings_match_public_schema() {
    assert_contract(
        APPLICATION_SETTINGS_SCHEMA,
        &ApplicationSettings {
            schema_version: 1,
            language: "zh-CN".to_owned(),
            expert_mode: true,
        },
    );
}

#[test]
fn schemas_reject_undeclared_top_level_fields() {
    let schema: Value = serde_json::from_str(PLAN_SCHEMA).expect("schema is JSON");
    let validator = jsonschema::draft202012::new(&schema).expect("schema compiles");
    let mut value = serde_json::to_value(plan()).expect("plan serializes");
    value
        .as_object_mut()
        .expect("plan is object")
        .insert("silent_drop".to_owned(), json!(true));
    assert!(!validator.is_valid(&value));
}
