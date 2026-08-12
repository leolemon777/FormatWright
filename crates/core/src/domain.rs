use std::collections::BTreeMap;
use std::path::PathBuf;

use formatwright_engine_sdk::{EngineIdentity, LossClass, Operation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactIdentity {
    pub display_path: String,
    pub canonical_path: PathBuf,
    pub size_bytes: u64,
    pub modified_unix_ms: i64,
    pub fast_fingerprint: String,
    pub full_blake3: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FormatKind {
    Image,
    Video,
    Audio,
    Document,
    Pdf,
    Data,
    Archive,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FormatDescriptor {
    pub id: String,
    pub kind: FormatKind,
    pub mime_type: Option<String>,
    pub container: Option<String>,
    pub extension_matches: Option<bool>,
    pub confidence: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StreamKind {
    Video,
    Audio,
    Subtitle,
    Attachment,
    Data,
    Page,
    RecordSet,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StreamProbe {
    pub index: u32,
    pub kind: StreamKind,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub duration_seconds: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    #[serde(default)]
    pub properties: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MetadataClassification {
    Public,
    Private,
    Secret,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MetadataEntry {
    pub value: serde_json::Value,
    pub classification: MetadataClassification,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiagnosticMessage {
    pub code: String,
    pub severity: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProbeEvidence {
    pub engine_id: String,
    pub engine_version: String,
    pub engine_binary_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Probe {
    pub schema_version: u32,
    pub artifact: ArtifactIdentity,
    pub format: FormatDescriptor,
    pub streams: Vec<StreamProbe>,
    pub metadata: BTreeMap<String, MetadataEntry>,
    pub warnings: Vec<DiagnosticMessage>,
    pub evidence: ProbeEvidence,
    pub duration_seconds: Option<f64>,
    pub bit_rate: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanRequest {
    pub target_format: String,
    pub output_path: Option<PathBuf>,
    pub preserve_all_streams: bool,
    #[serde(default)]
    pub audio_stream_index: Option<u32>,
    #[serde(default)]
    pub start_millis: Option<u64>,
    #[serde(default)]
    pub duration_millis: Option<u64>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub quality: Option<u8>,
    #[serde(default)]
    pub dpi: Option<u16>,
    #[serde(default)]
    pub color_mode: Option<String>,
    #[serde(default)]
    pub frames_per_second: Option<u32>,
    #[serde(default)]
    pub loop_count: Option<u16>,
    #[serde(default)]
    pub allow_lossy_data: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkPolicy {
    Deny,
    ExplicitAllow,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChangeSet {
    pub preserved: Vec<String>,
    pub changed: Vec<String>,
    pub dropped: Vec<String>,
    pub unknown: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanStep {
    pub step_id: String,
    pub capability_id: String,
    pub engine: EngineIdentity,
    pub operation: Operation,
    pub loss_class: LossClass,
    pub arguments: BTreeMap<String, String>,
    pub estimated_temporary_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Plan {
    pub schema_version: u32,
    pub plan_id: Uuid,
    pub plan_hash: String,
    pub input_fingerprint: String,
    pub target_format: String,
    pub constraints: BTreeMap<String, serde_json::Value>,
    pub steps: Vec<PlanStep>,
    pub changes: ChangeSet,
    pub validators: Vec<String>,
    pub network_policy: NetworkPolicy,
    pub output_path: Option<PathBuf>,
    pub estimated_output_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValidationStatus {
    Pass,
    Warning,
    Fail,
    Unknown,
}

impl ValidationStatus {
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Pass => 0,
            Self::Warning => 1,
            Self::Unknown => 2,
            Self::Fail => 3,
        }
    }

    #[must_use]
    pub const fn worst(self, other: Self) -> Self {
        if self.rank() >= other.rank() {
            self
        } else {
            other
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ValidationCheck {
    pub code: String,
    pub status: ValidationStatus,
    pub required: bool,
    pub expected: serde_json::Value,
    pub observed: serde_json::Value,
    pub evidence: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactSummary {
    pub display_path: Option<String>,
    pub format_id: String,
    pub size_bytes: u64,
    pub fast_fingerprint: String,
    pub full_blake3: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReportRedaction {
    pub paths_redacted: bool,
    pub metadata_values_redacted: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ValidationReport {
    pub schema_version: u32,
    pub report_id: Uuid,
    pub job_id: Uuid,
    pub plan_hash: String,
    pub status: ValidationStatus,
    pub input: ArtifactSummary,
    pub output: ArtifactSummary,
    pub engines: Vec<EngineIdentity>,
    pub checks: Vec<ValidationCheck>,
    pub intentional_changes: Vec<String>,
    pub redaction: ReportRedaction,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobState {
    Queued,
    Inspecting,
    Planned,
    Blocked,
    Running,
    Validating,
    Completed,
    Warning,
    Failed,
    Cancelled,
    Interrupted,
}

impl JobState {
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        use JobState::{
            Blocked, Cancelled, Completed, Failed, Inspecting, Interrupted, Planned, Queued,
            Running, Validating, Warning,
        };

        matches!(
            (self, next),
            (Inspecting, Planned | Blocked | Failed | Cancelled)
                | (Planned, Queued | Running | Blocked | Cancelled)
                | (Queued, Inspecting | Blocked | Cancelled)
                | (Blocked | Interrupted, Queued | Cancelled)
                | (Running, Validating | Failed | Cancelled | Interrupted)
                | (
                    Validating,
                    Completed | Warning | Failed | Cancelled | Interrupted
                )
                | (Failed | Cancelled, Queued)
        )
    }
}
