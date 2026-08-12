use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;

use formatwright_engine_sdk::{EngineIdentity, LossClass, Operation};
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Number, Value, json};
use uuid::Uuid;

use crate::domain::{
    ArtifactSummary, ChangeSet, DiagnosticMessage, FormatDescriptor, FormatKind, NetworkPolicy,
    Plan, PlanRequest, PlanStep, Probe, ProbeEvidence, ReportRedaction, SCHEMA_VERSION, StreamKind,
    StreamProbe, ValidationCheck, ValidationReport, ValidationStatus,
};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::fingerprint::identify_artifact;
use crate::planner::deterministic_plan_hash;

const STRUCTURED_ENGINE_ID: &str = "formatwright.structured";
const SUPPORTED_FORMATS: [&str; 4] = ["csv", "json", "yaml", "xml"];
const MAX_STRUCTURED_INPUT_BYTES: u64 = 64 * 1024 * 1024;

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq)]
struct Dataset {
    records: Vec<BTreeMap<String, Value>>,
    fields: Vec<String>,
    has_nested: bool,
    has_non_string_scalars: bool,
    has_nulls: bool,
    has_missing: bool,
    semantic_digest: String,
}

/// Returns a header-first structured format hint. CSV and ambiguous YAML
/// require a matching extension; JSON and XML can be recognized after rename.
#[must_use]
pub fn structured_format_hint(path: &Path) -> Option<&'static str> {
    let mut prefix = [0_u8; 4096];
    let read = File::open(path).ok()?.read(&mut prefix).ok()?;
    let text = String::from_utf8_lossy(&prefix[..read]);
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return Some("json");
    }
    if trimmed.starts_with('<') {
        return Some("xml");
    }
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "csv" => Some("csv"),
        "json" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        "xml" => Some("xml"),
        _ if trimmed.starts_with("---") => Some("yaml"),
        _ => None,
    }
}

/// Inspects and parses a structured-data artifact with the native adapter.
///
/// # Errors
///
/// Returns an input error for unsupported, malformed, duplicate-key, nested
/// XML, or non-record-shaped data.
pub async fn inspect_structured(path: impl AsRef<Path>) -> Result<Probe> {
    let path = path.as_ref();
    let format = structured_format_hint(path).ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Inspect,
            "The file is not a recognized structured-data format",
            "Use CSV, JSON, YAML, or XML with a recognizable header or extension.",
        )
    })?;
    let artifact = identify_artifact(path).await?;
    if artifact.size_bytes > MAX_STRUCTURED_INPUT_BYTES {
        return Err(FormatWrightError::new(
            ErrorCode::ResourceExhausted,
            Stage::Inspect,
            format!(
                "Structured input exceeds the {} MiB alpha safety limit",
                MAX_STRUCTURED_INPUT_BYTES / 1024 / 1024
            ),
            "Split the input or use a future streaming structured adapter.",
        ));
    }
    let owned_path = artifact.canonical_path.clone();
    let format_owned = format.to_owned();
    let dataset = tokio::task::spawn_blocking(move || load_dataset(&owned_path, &format_owned))
        .await
        .map_err(|error| worker_error(Stage::Inspect, error))??;
    Ok(probe_from_dataset(artifact, format, &dataset))
}

/// Builds a deterministic plan for the Rust-native structured adapter.
///
/// # Errors
///
/// Returns a policy error when conversion would flatten nested data or lose
/// scalar/null/missing distinctions without explicit authorization.
#[allow(clippy::too_many_lines)]
pub fn plan_structured_conversion(
    probe: &Probe,
    request: &PlanRequest,
    engine: &EngineIdentity,
) -> Result<Plan> {
    let target = normalize_format(&request.target_format);
    if !SUPPORTED_FORMATS.contains(&target.as_str()) {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            format!("Structured target is unsupported: {target}"),
            "Choose CSV, JSON, YAML, or XML.",
        ));
    }
    if probe.format.kind != FormatKind::Data
        || !SUPPORTED_FORMATS.contains(&probe.format.id.as_str())
    {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            "Structured conversion requires CSV, JSON, YAML, or XML input",
            "Inspect a supported structured-data file.",
        ));
    }
    if engine.engine_id != STRUCTURED_ENGINE_ID {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            "The Plan was given the wrong structured-data engine",
            "Run doctor and rebuild the Plan.",
        ));
    }
    let properties = record_properties(probe)?;
    let has_nested = property_bool(properties, "has_nested");
    let has_non_string_scalars = property_bool(properties, "has_non_string_scalars");
    let has_nulls = property_bool(properties, "has_nulls");
    let has_missing = property_bool(properties, "has_missing");
    if matches!(target.as_str(), "csv" | "xml") && has_nested {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Plan,
            "Nested records cannot be flattened implicitly",
            "Choose JSON/YAML or provide a future explicit flatten mapping.",
        ));
    }
    let scalar_loss = matches!(target.as_str(), "csv" | "xml")
        && (has_non_string_scalars || has_nulls || has_missing);
    if scalar_loss && !request.allow_lossy_data {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Plan,
            "The target cannot preserve scalar, null, and missing-field distinctions",
            "Pass --allow-lossy-data only after reviewing the Plan, or choose JSON/YAML.",
        ));
    }
    let mut changes = ChangeSet {
        preserved: vec!["record order".to_owned(), "field names".to_owned()],
        changed: vec![format!(
            "serialization changes from {} to {target}",
            probe.format.id
        )],
        ..ChangeSet::default()
    };
    if scalar_loss {
        changes.changed.push(
            "non-string scalars become text and null/missing values become empty text".to_owned(),
        );
    } else {
        changes
            .preserved
            .push("scalar types, nulls, and missing fields".to_owned());
    }
    let arguments = BTreeMap::from([
        ("source_format".to_owned(), probe.format.id.clone()),
        ("target_format".to_owned(), target.clone()),
        ("mapping".to_owned(), "flat-records-v1".to_owned()),
        ("type_inference".to_owned(), "none".to_owned()),
        ("source_encoding".to_owned(), "utf-8".to_owned()),
        ("target_encoding".to_owned(), "utf-8".to_owned()),
        (
            "source_delimiter".to_owned(),
            if probe.format.id == "csv" { "," } else { "n/a" }.to_owned(),
        ),
        (
            "target_delimiter".to_owned(),
            if target == "csv" { "," } else { "n/a" }.to_owned(),
        ),
        ("record_order".to_owned(), "preserve".to_owned()),
        ("field_order".to_owned(), "lexicographic".to_owned()),
        ("lossy_scalar_mapping".to_owned(), scalar_loss.to_string()),
        (
            "null_and_missing_policy".to_owned(),
            if scalar_loss {
                "empty-text"
            } else {
                "preserve-distinction"
            }
            .to_owned(),
        ),
    ]);
    let constraints = BTreeMap::from([
        (
            "allow_lossy_data".to_owned(),
            json!(request.allow_lossy_data),
        ),
        ("flatten_nested".to_owned(), json!(false)),
        ("duplicate_keys".to_owned(), json!("reject")),
        ("field_order".to_owned(), json!("lexicographic")),
    ]);
    let step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: format!("formatwright.structured.{}-to-{target}", probe.format.id),
        engine: engine.clone(),
        operation: Operation::Serialize,
        loss_class: if scalar_loss {
            LossClass::Lossy
        } else {
            LossClass::None
        },
        arguments,
        estimated_temporary_bytes: Some(probe.artifact.size_bytes.saturating_mul(2)),
    };
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: Uuid::new_v4(),
        plan_hash: String::new(),
        input_fingerprint: probe.artifact.fast_fingerprint.clone(),
        target_format: target,
        constraints,
        steps: vec![step],
        changes,
        validators: vec![
            "structured.output-parses".to_owned(),
            "structured.record-count".to_owned(),
            "structured.field-set".to_owned(),
            "structured.semantic-digest".to_owned(),
        ],
        network_policy: NetworkPolicy::Deny,
        output_path: request.output_path.clone(),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

pub(crate) fn convert_structured_file(input: &Path, output: &Path, plan: &Plan) -> Result<()> {
    let step = plan
        .steps
        .first()
        .ok_or_else(|| invalid_plan("missing structured step"))?;
    if step.engine.engine_id != STRUCTURED_ENGINE_ID {
        return Err(invalid_plan("unexpected structured engine ID"));
    }
    let source_format = checked_format_argument(step, "source_format")?;
    let target_format = checked_format_argument(step, "target_format")?;
    if target_format != plan.target_format {
        return Err(invalid_plan("target format differs from Plan"));
    }
    let detected = structured_format_hint(input)
        .ok_or_else(|| invalid_plan("input no longer has a structured format"))?;
    if detected != source_format {
        return Err(FormatWrightError::new(
            ErrorCode::InputChanged,
            Stage::Execute,
            "Structured input format changed after planning",
            "Inspect and plan the input again.",
        ));
    }
    let dataset = load_dataset(input, source_format)?;
    write_dataset(output, target_format, &dataset)
}

pub(crate) fn validate_structured_output(
    input: &Probe,
    output: &Probe,
    plan: &Plan,
    job_id: Uuid,
) -> ValidationReport {
    let expected_records = record_property(input, "record_count");
    let observed_records = record_property(output, "record_count");
    let expected_fields = record_property(input, "fields");
    let observed_fields = record_property(output, "fields");
    let expected_digest = record_property(input, "semantic_digest");
    let observed_digest = record_property(output, "semantic_digest");
    let format_matches = output.format.id == plan.target_format;
    let lossy = plan
        .steps
        .first()
        .is_some_and(|step| step.loss_class == LossClass::Lossy);
    let digest_matches = expected_digest == observed_digest;
    let checks = vec![
        validation_check(
            "STRUCTURED_OUTPUT_PARSES",
            ValidationStatus::Pass,
            json!(true),
            json!(true),
            "Native parser reopened the complete output.",
        ),
        validation_check(
            "STRUCTURED_TARGET_FORMAT",
            status(format_matches),
            json!(plan.target_format),
            json!(output.format.id),
            "Header-first structured format detection.",
        ),
        validation_check(
            "STRUCTURED_RECORD_COUNT",
            status(expected_records == observed_records),
            expected_records,
            observed_records,
            "Parsed record inventory.",
        ),
        validation_check(
            "STRUCTURED_FIELD_SET",
            status(expected_fields == observed_fields),
            expected_fields,
            observed_fields,
            "Lexicographically normalized field inventory.",
        ),
        validation_check(
            "STRUCTURED_SEMANTIC_DIGEST",
            if digest_matches {
                ValidationStatus::Pass
            } else if lossy {
                ValidationStatus::Warning
            } else {
                ValidationStatus::Fail
            },
            expected_digest,
            observed_digest,
            if digest_matches {
                "Canonical record values are unchanged."
            } else if lossy {
                "Canonical values changed only under the authorized lossy mapping."
            } else {
                "Canonical record values changed unexpectedly."
            },
        ),
    ];
    let report_status = checks
        .iter()
        .fold(ValidationStatus::Pass, |current, check| {
            current.worst(check.status)
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

fn load_dataset(path: &Path, format: &str) -> Result<Dataset> {
    let records = match format {
        "csv" => read_csv(path)?,
        "json" => read_json(path)?,
        "yaml" => read_yaml(path)?,
        "xml" => read_xml(path)?,
        _ => return Err(invalid_plan("unsupported structured source format")),
    };
    dataset(records)
}

fn dataset(records: Vec<BTreeMap<String, Value>>) -> Result<Dataset> {
    let fields = records
        .iter()
        .flat_map(|record| record.keys().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let has_nested = records
        .iter()
        .flat_map(BTreeMap::values)
        .any(|value| matches!(value, Value::Array(_) | Value::Object(_)));
    let has_non_string_scalars = records
        .iter()
        .flat_map(BTreeMap::values)
        .any(|value| !matches!(value, Value::String(_) | Value::Null));
    let has_nulls = records
        .iter()
        .flat_map(BTreeMap::values)
        .any(Value::is_null);
    let has_missing = records
        .iter()
        .any(|record| fields.iter().any(|field| !record.contains_key(field)));
    let canonical = serde_json::to_vec(&records).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::Internal,
            Stage::Inspect,
            "Unable to compute the structured semantic digest",
            "Report this internal error.",
        )
        .with_diagnostic(error.to_string())
    })?;
    Ok(Dataset {
        records,
        fields,
        has_nested,
        has_non_string_scalars,
        has_nulls,
        has_missing,
        semantic_digest: format!("blake3:{}", blake3::hash(&canonical).to_hex()),
    })
}

fn read_csv(path: &Path) -> Result<Vec<BTreeMap<String, Value>>> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(false)
        .from_reader(open_utf8_reader(path, "CSV")?);
    let headers = reader
        .headers()
        .map_err(|error| parse_error("CSV header", error))?
        .iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if headers.is_empty() || headers.iter().any(String::is_empty) {
        return Err(input_error("CSV headers must be non-empty"));
    }
    if headers.iter().collect::<BTreeSet<_>>().len() != headers.len() {
        return Err(input_error("Duplicate CSV headers are not allowed"));
    }
    reader
        .records()
        .map(|row| {
            let row = row.map_err(|error| parse_error("CSV record", error))?;
            Ok(headers
                .iter()
                .zip(row.iter())
                .map(|(field, value)| (field.clone(), Value::String(value.to_owned())))
                .collect())
        })
        .collect()
}

fn read_json(path: &Path) -> Result<Vec<BTreeMap<String, Value>>> {
    let mut deserializer = serde_json::Deserializer::from_reader(open_utf8_reader(path, "JSON")?);
    let value = StrictJsonValue::deserialize(&mut deserializer)
        .map_err(|error| parse_error("JSON", error))?
        .0;
    deserializer
        .end()
        .map_err(|error| parse_error("JSON trailing data", error))?;
    records_from_value(value, "JSON")
}

fn read_yaml(path: &Path) -> Result<Vec<BTreeMap<String, Value>>> {
    let value: Value = serde_yaml_ng::from_reader(open_utf8_reader(path, "YAML")?)
        .map_err(|error| parse_error("YAML", error))?;
    records_from_value(value, "YAML")
}

#[allow(clippy::too_many_lines)]
fn read_xml(path: &Path) -> Result<Vec<BTreeMap<String, Value>>> {
    let mut reader = Reader::from_reader(open_utf8_reader(path, "XML")?);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0_u8;
    let mut records = Vec::new();
    let mut current_record: Option<BTreeMap<String, Value>> = None;
    let mut current_field: Option<(String, String)> = None;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                reject_xml_attributes(&event)?;
                depth = depth.saturating_add(1);
                let name = xml_event_name(event.name().as_ref())?;
                match depth {
                    1 if name == "records" => {}
                    2 if name == "record" => current_record = Some(BTreeMap::new()),
                    3 if current_record.is_some() && valid_xml_name(&name) => {
                        current_field = Some((name, String::new()));
                    }
                    _ => {
                        return Err(input_error(
                            "XML must use records/record/field without nesting",
                        ));
                    }
                }
            }
            Ok(Event::Empty(event)) => {
                reject_xml_attributes(&event)?;
                let name = xml_event_name(event.name().as_ref())?;
                if depth == 2 && current_record.is_some() && valid_xml_name(&name) {
                    insert_xml_field(current_record.as_mut(), &name, String::new())?;
                } else {
                    return Err(input_error("Unexpected empty XML element"));
                }
            }
            Ok(Event::Text(text)) => {
                let decoded = text
                    .decode()
                    .map_err(|error| parse_error("XML text", error))?;
                let unescaped = quick_xml::escape::unescape(&decoded)
                    .map_err(|error| parse_error("XML entity", error))?;
                if let Some((_, value)) = current_field.as_mut() {
                    value.push_str(&unescaped);
                } else if !unescaped.trim().is_empty() {
                    return Err(input_error("XML text is only allowed inside record fields"));
                }
            }
            Ok(Event::CData(text)) => {
                let decoded = text
                    .decode()
                    .map_err(|error| parse_error("XML CDATA", error))?;
                if let Some((_, value)) = current_field.as_mut() {
                    value.push_str(&decoded);
                } else if !decoded.trim().is_empty() {
                    return Err(input_error(
                        "XML CDATA is only allowed inside record fields",
                    ));
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                let name = reference
                    .decode()
                    .map_err(|error| parse_error("XML entity reference", error))?;
                let value = match name.as_ref() {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "apos" => "'",
                    "quot" => "\"",
                    _ => {
                        return Err(FormatWrightError::new(
                            ErrorCode::PolicyBlocked,
                            Stage::Inspect,
                            format!("XML entity reference is disabled: &{name};"),
                            "Use literal text or one of the five predefined XML entities.",
                        ));
                    }
                };
                current_field
                    .as_mut()
                    .ok_or_else(|| input_error("XML entities are only allowed inside fields"))?
                    .1
                    .push_str(value);
            }
            Ok(Event::End(event)) => {
                let name = xml_event_name(event.name().as_ref())?;
                match depth {
                    3 => {
                        let (field, value) = current_field
                            .take()
                            .ok_or_else(|| input_error("XML field close is unbalanced"))?;
                        if field != name {
                            return Err(input_error("XML field close name does not match"));
                        }
                        insert_xml_field(current_record.as_mut(), &field, value)?;
                    }
                    2 if name == "record" => records.push(
                        current_record
                            .take()
                            .ok_or_else(|| input_error("XML record close is unbalanced"))?,
                    ),
                    1 if name == "records" => {}
                    _ => return Err(input_error("Unexpected XML closing element")),
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::DocType(_)) => {
                return Err(FormatWrightError::new(
                    ErrorCode::PolicyBlocked,
                    Stage::Inspect,
                    "XML document types and external entities are disabled",
                    "Remove the DTD and retry with self-contained records.",
                ));
            }
            Ok(Event::Decl(_) | Event::Comment(_) | Event::PI(_)) => {}
            Ok(Event::Eof) => break,
            Err(error) => return Err(parse_error("XML", error)),
        }
        buffer.clear();
    }
    if depth != 0 || current_record.is_some() || current_field.is_some() {
        return Err(input_error("XML document ended with unclosed elements"));
    }
    Ok(records)
}

fn records_from_value(value: Value, label: &str) -> Result<Vec<BTreeMap<String, Value>>> {
    let Value::Array(items) = value else {
        return Err(input_error(&format!(
            "{label} input must be a top-level array of record objects"
        )));
    };
    items
        .into_iter()
        .enumerate()
        .map(|(index, item)| match item {
            Value::Object(map) => Ok(map.into_iter().collect()),
            _ => Err(input_error(&format!(
                "{label} record {index} is not an object"
            ))),
        })
        .collect()
}

fn open_utf8_reader(path: &Path, label: &str) -> Result<BufReader<File>> {
    let file = File::open(path)
        .map_err(|error| io_error(Stage::Inspect, &format!("open {label}"), error))?;
    let mut reader = BufReader::new(file);
    let has_bom = reader
        .fill_buf()
        .map_err(|error| io_error(Stage::Inspect, &format!("read {label}"), error))?
        .starts_with(&[0xEF, 0xBB, 0xBF]);
    if has_bom {
        reader.consume(3);
    }
    Ok(reader)
}

fn write_dataset(path: &Path, format: &str, dataset: &Dataset) -> Result<()> {
    match format {
        "csv" => write_csv(path, dataset),
        "json" => write_json(path, dataset),
        "yaml" => write_yaml(path, dataset),
        "xml" => write_xml(path, dataset),
        _ => Err(invalid_plan("unsupported structured target format")),
    }
}

fn write_json(path: &Path, dataset: &Dataset) -> Result<()> {
    let file =
        File::create(path).map_err(|error| io_error(Stage::Execute, "create JSON", error))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &dataset.records)
        .map_err(|error| parse_error("serialize JSON", error))?;
    writer
        .write_all(b"\n")
        .map_err(|error| io_error(Stage::Execute, "finish JSON", error))
}

fn write_yaml(path: &Path, dataset: &Dataset) -> Result<()> {
    let file =
        File::create(path).map_err(|error| io_error(Stage::Execute, "create YAML", error))?;
    serde_yaml_ng::to_writer(BufWriter::new(file), &dataset.records)
        .map_err(|error| parse_error("serialize YAML", error))
}

fn write_csv(path: &Path, dataset: &Dataset) -> Result<()> {
    let mut writer = csv::WriterBuilder::new()
        .from_path(path)
        .map_err(|error| parse_error("create CSV", error))?;
    writer
        .write_record(&dataset.fields)
        .map_err(|error| parse_error("write CSV header", error))?;
    for record in &dataset.records {
        let row = dataset
            .fields
            .iter()
            .map(|field| scalar_text(record.get(field)))
            .collect::<Result<Vec<_>>>()?;
        writer
            .write_record(row)
            .map_err(|error| parse_error("write CSV record", error))?;
    }
    writer
        .flush()
        .map_err(|error| io_error(Stage::Execute, "flush CSV", error))
}

fn write_xml(path: &Path, dataset: &Dataset) -> Result<()> {
    for field in &dataset.fields {
        if !valid_xml_name(field) {
            return Err(FormatWrightError::new(
                ErrorCode::PolicyBlocked,
                Stage::Execute,
                format!("Field is not a valid XML element name: {field}"),
                "Rename the field or choose JSON/YAML/CSV.",
            ));
        }
    }
    let file = File::create(path).map_err(|error| io_error(Stage::Execute, "create XML", error))?;
    let mut writer = Writer::new_with_indent(BufWriter::new(file), b' ', 2);
    writer
        .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
        .map_err(|error| parse_error("write XML declaration", error))?;
    writer
        .write_event(Event::Start(BytesStart::new("records")))
        .map_err(|error| parse_error("write XML root", error))?;
    for record in &dataset.records {
        writer
            .write_event(Event::Start(BytesStart::new("record")))
            .map_err(|error| parse_error("write XML record", error))?;
        for field in &dataset.fields {
            writer
                .write_event(Event::Start(BytesStart::new(field)))
                .map_err(|error| parse_error("write XML field", error))?;
            let value = scalar_text(record.get(field))?;
            writer
                .write_event(Event::Text(BytesText::new(&value)))
                .map_err(|error| parse_error("write XML text", error))?;
            writer
                .write_event(Event::End(BytesEnd::new(field)))
                .map_err(|error| parse_error("close XML field", error))?;
        }
        writer
            .write_event(Event::End(BytesEnd::new("record")))
            .map_err(|error| parse_error("close XML record", error))?;
    }
    writer
        .write_event(Event::End(BytesEnd::new("records")))
        .map_err(|error| parse_error("close XML root", error))?;
    writer
        .get_mut()
        .flush()
        .map_err(|error| io_error(Stage::Execute, "flush XML", error))
}

fn scalar_text(value: Option<&Value>) -> Result<String> {
    match value {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(value)) => Ok(value.clone()),
        Some(Value::Bool(value)) => Ok(value.to_string()),
        Some(Value::Number(value)) => Ok(value.to_string()),
        Some(Value::Array(_) | Value::Object(_)) => Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Execute,
            "Nested data reached a flat serializer",
            "Choose JSON/YAML or create an explicit flatten mapping.",
        )),
    }
}

fn probe_from_dataset(
    artifact: crate::domain::ArtifactIdentity,
    format: &str,
    dataset: &Dataset,
) -> Probe {
    let extension = artifact
        .canonical_path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    let extension_matches = extension
        .as_deref()
        .is_some_and(|extension| extension == format || (format == "yaml" && extension == "yml"));
    let mut warnings = Vec::new();
    if !extension_matches {
        warnings.push(DiagnosticMessage {
            code: "EXTENSION_MISMATCH".to_owned(),
            severity: "warning".to_owned(),
            message: format!("File extension does not match detected format {format}"),
        });
    }
    let properties = BTreeMap::from([
        ("record_count".to_owned(), json!(dataset.records.len())),
        ("fields".to_owned(), json!(dataset.fields)),
        ("has_nested".to_owned(), json!(dataset.has_nested)),
        (
            "has_non_string_scalars".to_owned(),
            json!(dataset.has_non_string_scalars),
        ),
        ("has_nulls".to_owned(), json!(dataset.has_nulls)),
        ("has_missing".to_owned(), json!(dataset.has_missing)),
        ("semantic_digest".to_owned(), json!(dataset.semantic_digest)),
        ("encoding".to_owned(), json!("utf-8")),
        (
            "delimiter".to_owned(),
            if format == "csv" {
                json!(",")
            } else {
                Value::Null
            },
        ),
    ]);
    Probe {
        schema_version: SCHEMA_VERSION,
        artifact,
        format: FormatDescriptor {
            id: format.to_owned(),
            kind: FormatKind::Data,
            mime_type: structured_mime(format).map(str::to_owned),
            container: None,
            extension_matches: Some(extension_matches),
            confidence: if matches!(format, "json" | "xml") {
                1.0
            } else {
                0.8
            },
        },
        streams: vec![StreamProbe {
            index: 0,
            kind: StreamKind::RecordSet,
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
        warnings,
        evidence: ProbeEvidence {
            engine_id: STRUCTURED_ENGINE_ID.to_owned(),
            engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            engine_binary_sha256: None,
        },
        duration_seconds: None,
        bit_rate: None,
    }
}

fn record_properties(probe: &Probe) -> Result<&BTreeMap<String, Value>> {
    probe
        .streams
        .iter()
        .find(|stream| stream.kind == StreamKind::RecordSet)
        .map(|stream| &stream.properties)
        .ok_or_else(|| input_error("Structured Probe has no record-set stream"))
}

fn record_property(probe: &Probe, name: &str) -> Value {
    record_properties(probe)
        .ok()
        .and_then(|properties| properties.get(name))
        .cloned()
        .unwrap_or(Value::Null)
}

fn property_bool(properties: &BTreeMap<String, Value>, name: &str) -> bool {
    properties
        .get(name)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn checked_format_argument<'a>(step: &'a PlanStep, name: &str) -> Result<&'a str> {
    let value = step
        .arguments
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| invalid_plan("missing format argument"))?;
    if SUPPORTED_FORMATS.contains(&value) {
        Ok(value)
    } else {
        Err(invalid_plan("unsupported format argument"))
    }
}

fn normalize_format(value: &str) -> String {
    match value
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .as_str()
    {
        "yml" => "yaml".to_owned(),
        value => value.to_owned(),
    }
}

fn structured_mime(format: &str) -> Option<&'static str> {
    match format {
        "csv" => Some("text/csv"),
        "json" => Some("application/json"),
        "yaml" => Some("application/yaml"),
        "xml" => Some("application/xml"),
        _ => None,
    }
}

fn insert_xml_field(
    record: Option<&mut BTreeMap<String, Value>>,
    field: &str,
    value: String,
) -> Result<()> {
    let record = record.ok_or_else(|| input_error("XML field is outside a record"))?;
    if record
        .insert(field.to_owned(), Value::String(value))
        .is_some()
    {
        return Err(input_error(&format!("Duplicate XML field: {field}")));
    }
    Ok(())
}

fn xml_event_name(bytes: &[u8]) -> Result<String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|error| parse_error("XML element name", error))
}

fn reject_xml_attributes(event: &BytesStart<'_>) -> Result<()> {
    if event.attributes().next().is_some() {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Inspect,
            "XML attributes are outside the records-v1 mapping",
            "Represent values as record field elements or choose JSON/YAML.",
        ));
    }
    Ok(())
}

fn valid_xml_name(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    (first == '_' || first.is_alphabetic())
        && characters.all(|character| {
            character == '_' || character == '-' || character == '.' || character.is_alphanumeric()
        })
}

fn validation_check(
    code: &str,
    check_status: ValidationStatus,
    expected: Value,
    observed: Value,
    message: &str,
) -> ValidationCheck {
    ValidationCheck {
        code: code.to_owned(),
        status: check_status,
        required: true,
        expected,
        observed,
        evidence: "FormatWright native structured parser".to_owned(),
        message: message.to_owned(),
    }
}

const fn status(matches: bool) -> ValidationStatus {
    if matches {
        ValidationStatus::Pass
    } else {
        ValidationStatus::Fail
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

fn input_error(message: &str) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Inspect,
        message,
        "Correct the structured input or choose an explicit mapping.",
    )
}

fn invalid_plan(message: &str) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::PolicyBlocked,
        Stage::Execute,
        format!("Invalid structured Plan: {message}"),
        "Create a new Plan with the current FormatWright version.",
    )
}

fn parse_error(label: &str, error: impl fmt::Display) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Inspect,
        format!("Unable to parse {label}"),
        "Correct the input and retry.",
    )
    .with_diagnostic(error.to_string())
}

#[allow(clippy::needless_pass_by_value)]
fn io_error(stage: Stage, action: &str, error: std::io::Error) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::StorageFailed,
        stage,
        format!("Unable to {action}"),
        "Check file permissions and storage health.",
    )
    .with_diagnostic(error.to_string())
}

#[allow(clippy::needless_pass_by_value)]
fn worker_error(stage: Stage, error: tokio::task::JoinError) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::Internal,
        stage,
        "Structured parser worker failed",
        "Retry the operation.",
    )
    .with_diagnostic(error.to_string())
}

#[derive(Debug)]
struct StrictJsonValue(Value);

impl<'de> Deserialize<'de> for StrictJsonValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

struct StrictJsonVisitor;

impl<'de> Visitor<'de> for StrictJsonVisitor {
    type Value = StrictJsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Number(Number::from(value))))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Number(Number::from(value))))
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .map(StrictJsonValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Null))
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictJsonValue>()? {
            values.push(value.0);
        }
        Ok(StrictJsonValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = serde_json::Map::new();
        while let Some((key, value)) = map.next_entry::<String, StrictJsonValue>()? {
            if values.insert(key.clone(), value.0).is_some() {
                return Err(de::Error::custom(format!("duplicate JSON key: {key}")));
            }
        }
        Ok(StrictJsonValue(Value::Object(values)))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use formatwright_engine_sdk::{Certification, EngineIdentity};
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::{
        convert_structured_file, inspect_structured, plan_structured_conversion,
        validate_structured_output,
    };
    use crate::{ErrorCode, PlanRequest, ValidationStatus};

    fn engine() -> EngineIdentity {
        EngineIdentity {
            engine_id: "formatwright.structured".to_owned(),
            version: "test".to_owned(),
            binary_path: PathBuf::from("formatwright"),
            binary_sha256: "00".repeat(32),
            manifest_sha256: None,
            build_configuration: Some("test".to_owned()),
            certification: Certification::Experimental,
        }
    }

    fn request(target: &str, output: PathBuf) -> PlanRequest {
        PlanRequest {
            target_format: target.to_owned(),
            output_path: Some(output),
            ..PlanRequest::default()
        }
    }

    #[tokio::test]
    async fn json_to_yaml_preserves_semantic_digest() {
        let directory = tempdir().expect("temporary directory");
        let input = directory.path().join("records.json");
        let output = directory.path().join("records.yaml");
        fs::write(
            &input,
            r#"[{"id":9007199254740993,"name":"Ada","active":true,"note":null}]"#,
        )
        .expect("write JSON");
        let input_probe = inspect_structured(&input).await.expect("inspect JSON");
        let plan =
            plan_structured_conversion(&input_probe, &request("yaml", output.clone()), &engine())
                .expect("plan JSON to YAML");
        convert_structured_file(&input, &output, &plan).expect("convert JSON to YAML");
        let output_probe = inspect_structured(&output).await.expect("inspect YAML");
        let report = validate_structured_output(&input_probe, &output_probe, &plan, Uuid::new_v4());
        assert_eq!(report.status, ValidationStatus::Pass);
    }

    #[tokio::test]
    async fn nested_json_to_csv_is_blocked_without_flatten_mapping() {
        let directory = tempdir().expect("temporary directory");
        let input = directory.path().join("nested.json");
        fs::write(&input, r#"[{"id":1,"nested":{"value":2}}]"#).expect("write JSON");
        let probe = inspect_structured(&input).await.expect("inspect JSON");
        let error = plan_structured_conversion(
            &probe,
            &request("csv", directory.path().join("nested.csv")),
            &engine(),
        )
        .expect_err("nested flatten must be blocked");
        assert_eq!(error.code, ErrorCode::PolicyBlocked);
    }

    #[tokio::test]
    async fn duplicate_json_keys_are_rejected() {
        let directory = tempdir().expect("temporary directory");
        let input = directory.path().join("duplicate.json");
        fs::write(&input, r#"[{"id":1,"id":2}]"#).expect("write JSON");
        let error = inspect_structured(&input)
            .await
            .expect_err("duplicate keys must fail");
        assert_eq!(error.code, ErrorCode::InputInvalid);
        assert!(
            error
                .diagnostic
                .is_some_and(|value| value.contains("duplicate JSON key"))
        );
    }

    #[tokio::test]
    async fn oversized_structured_input_is_rejected_before_parsing() {
        let directory = tempdir().expect("temporary directory");
        let input = directory.path().join("oversized.json");
        let file = fs::File::create(&input).expect("create sparse fixture");
        file.set_len(super::MAX_STRUCTURED_INPUT_BYTES + 1)
            .expect("size sparse fixture");
        let error = inspect_structured(&input)
            .await
            .expect_err("oversized structured input must fail safely");
        assert_eq!(error.code, ErrorCode::ResourceExhausted);
    }
}
