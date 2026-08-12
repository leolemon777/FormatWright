use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use formatwright_engine_sdk::{EngineIdentity, LossClass, Operation};
use quick_xml::Reader;
use quick_xml::events::Event;
use serde_json::{Value, json};
use uuid::Uuid;
use zip::ZipArchive;

use crate::domain::{
    ArtifactSummary, ChangeSet, FormatDescriptor, FormatKind, NetworkPolicy, Plan, PlanStep, Probe,
    ProbeEvidence, ReportRedaction, SCHEMA_VERSION, StreamKind, StreamProbe, ValidationCheck,
    ValidationReport, ValidationStatus,
};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::fingerprint::identify_artifact;
use crate::planner::deterministic_plan_hash;

const MAX_MARKUP_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DOCUMENT_XML_BYTES: u64 = 32 * 1024 * 1024;

/// Inspects Markdown, HTML, or DOCX without executing document macros or loading resources.
///
/// # Errors
///
/// Returns a typed input or resource error for malformed or oversized input.
pub async fn inspect_document(path: impl AsRef<Path>) -> Result<Probe> {
    let path = path.as_ref();
    let artifact = identify_artifact(path).await?;
    let format = document_format_hint(path)?;
    let owned = artifact.canonical_path.clone();
    let format_owned = format.to_owned();
    let properties =
        tokio::task::spawn_blocking(move || inspect_document_properties(&owned, &format_owned))
            .await
            .map_err(worker_error)??;
    let extension = artifact
        .canonical_path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    let extension_matches = extension.as_deref().is_some_and(|extension| {
        extension == format
            || (format == "markdown" && matches!(extension, "md" | "markdown"))
            || (format == "html" && matches!(extension, "html" | "htm"))
    });
    Ok(Probe {
        schema_version: SCHEMA_VERSION,
        artifact,
        format: FormatDescriptor {
            id: format.to_owned(),
            kind: FormatKind::Document,
            mime_type: Some(
                match format {
                    "markdown" => "text/markdown",
                    "html" => "text/html",
                    "docx" => {
                        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                    }
                    _ => unreachable!("document formats are exhaustive"),
                }
                .to_owned(),
            ),
            container: (format == "docx").then(|| "zip/opc".to_owned()),
            extension_matches: Some(extension_matches),
            confidence: if format == "docx" { 1.0 } else { 0.85 },
        },
        streams: vec![StreamProbe {
            index: 0,
            kind: StreamKind::Page,
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
        warnings: if extension_matches {
            Vec::new()
        } else {
            vec![crate::domain::DiagnosticMessage {
                code: "EXTENSION_MISMATCH".to_owned(),
                severity: "warning".to_owned(),
                message: format!("Extension does not match detected {format}"),
            }]
        },
        evidence: ProbeEvidence {
            engine_id: "formatwright.document-inspector".to_owned(),
            engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            engine_binary_sha256: None,
        },
        duration_seconds: None,
        bit_rate: None,
    })
}

/// Plans an offline Pandoc conversion from Markdown/HTML to DOCX.
///
/// # Errors
///
/// Returns Unsupported for other inputs or targets.
pub fn plan_markup_to_docx(
    probe: &Probe,
    output_path: std::path::PathBuf,
    pandoc: &EngineIdentity,
) -> Result<Plan> {
    if !matches!(probe.format.id.as_str(), "markdown" | "html") {
        return Err(unsupported("Pandoc DOCX input must be Markdown or HTML"));
    }
    if pandoc.engine_id != "pandoc" {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            "The document Plan was given the wrong engine",
            "Run doctor and use Pandoc.",
        ));
    }
    if property(probe, "has_external_resource") == json!(true) {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Plan,
            "Markup contains an external image/resource under deny-all policy",
            "Remove the resource or wait for an explicitly authorized resource-root policy.",
        ));
    }
    let arguments = BTreeMap::from([
        ("source_format".to_owned(), probe.format.id.clone()),
        ("target_format".to_owned(), "docx".to_owned()),
        ("sandbox".to_owned(), "true".to_owned()),
        ("standalone".to_owned(), "true".to_owned()),
        ("resource_policy".to_owned(), "deny-all".to_owned()),
    ]);
    let constraints = BTreeMap::from([
        ("network".to_owned(), json!("deny")),
        ("external_resources".to_owned(), json!("deny")),
        ("macros".to_owned(), json!("disabled")),
    ]);
    let step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: format!("pandoc.{}-to-docx.offline", probe.format.id),
        engine: pandoc.clone(),
        operation: Operation::Transform,
        loss_class: LossClass::Unknown,
        arguments,
        estimated_temporary_bytes: Some(probe.artifact.size_bytes.saturating_mul(4)),
    };
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: Uuid::new_v4(),
        plan_hash: String::new(),
        input_fingerprint: probe.artifact.fast_fingerprint.clone(),
        target_format: "docx".to_owned(),
        constraints,
        steps: vec![step],
        changes: ChangeSet {
            preserved: vec![
                "normalized textual content".to_owned(),
                "document structure supported by Pandoc".to_owned(),
            ],
            changed: vec![
                "layout is rendered using Pandoc's default DOCX reference document".to_owned(),
            ],
            dropped: vec!["external resources under the default deny policy".to_owned()],
            unknown: vec![
                "visual fidelity is not certified by the alpha DOCX validator".to_owned(),
            ],
        },
        validators: vec![
            "docx.package-opens".to_owned(),
            "docx.required-parts".to_owned(),
            "docx.semantic-token-digest".to_owned(),
        ],
        network_policy: NetworkPolicy::Deny,
        output_path: Some(output_path),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

/// Plans an offline Markdown/HTML to PDF pipeline through an intermediate DOCX.
///
/// # Errors
///
/// Returns a planning error for unsupported input, external resources, or an
/// incorrectly selected engine.
#[allow(clippy::too_many_lines)]
pub fn plan_markup_to_pdf(
    probe: &Probe,
    output_path: std::path::PathBuf,
    pandoc: &EngineIdentity,
    soffice: &EngineIdentity,
    pdfinfo: &EngineIdentity,
    pdftoppm: &EngineIdentity,
) -> Result<Plan> {
    if !matches!(probe.format.id.as_str(), "markdown" | "html") {
        return Err(unsupported("Pandoc PDF input must be Markdown or HTML"));
    }
    if property(probe, "has_external_resource") == json!(true) {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Plan,
            "Markup contains an external image/resource under deny-all policy",
            "Remove the resource or wait for an explicitly authorized resource-root policy.",
        ));
    }
    if pandoc.engine_id != "pandoc"
        || soffice.engine_id != "soffice"
        || pdfinfo.engine_id != "pdfinfo"
        || pdftoppm.engine_id != "pdftoppm"
    {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            "The markup-to-PDF Plan was given an incorrect engine",
            "Run doctor and use Pandoc, soffice, pdfinfo, and pdftoppm.",
        ));
    }
    let pandoc_step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: format!("pandoc.{}-to-docx.intermediate.offline", probe.format.id),
        engine: pandoc.clone(),
        operation: Operation::Transform,
        loss_class: LossClass::Unknown,
        arguments: BTreeMap::from([
            ("source_format".to_owned(), probe.format.id.clone()),
            ("target_format".to_owned(), "docx".to_owned()),
            ("sandbox".to_owned(), "true".to_owned()),
            ("standalone".to_owned(), "true".to_owned()),
            ("resource_policy".to_owned(), "deny-all".to_owned()),
            ("purpose".to_owned(), "intermediate".to_owned()),
        ]),
        estimated_temporary_bytes: Some(probe.artifact.size_bytes.saturating_mul(4)),
    };
    let office_step = PlanStep {
        step_id: "step-2".to_owned(),
        capability_id: "libreoffice.docx-to-pdf.headless".to_owned(),
        engine: soffice.clone(),
        operation: Operation::Render,
        loss_class: LossClass::Unknown,
        arguments: BTreeMap::from([
            ("source_format".to_owned(), "docx".to_owned()),
            ("target_format".to_owned(), "pdf".to_owned()),
            ("headless".to_owned(), "true".to_owned()),
            ("isolated_profile".to_owned(), "true".to_owned()),
            ("macros".to_owned(), "disabled".to_owned()),
            ("external_resources".to_owned(), "deny".to_owned()),
        ]),
        estimated_temporary_bytes: Some(probe.artifact.size_bytes.saturating_mul(8)),
    };
    let structural_validation = PlanStep {
        step_id: "step-3".to_owned(),
        capability_id: "poppler.pdf-structural-validation.all-pages".to_owned(),
        engine: pdfinfo.clone(),
        operation: Operation::Inspect,
        loss_class: LossClass::None,
        arguments: BTreeMap::from([
            ("page_sizes".to_owned(), "required".to_owned()),
            ("target_format".to_owned(), "pdf".to_owned()),
            ("purpose".to_owned(), "validation-only".to_owned()),
        ]),
        estimated_temporary_bytes: None,
    };
    let render_validation = PlanStep {
        step_id: "step-4".to_owned(),
        capability_id: "poppler.pdf-render-validation.all-pages".to_owned(),
        engine: pdftoppm.clone(),
        operation: Operation::Inspect,
        loss_class: LossClass::None,
        arguments: BTreeMap::from([
            ("dpi".to_owned(), "72".to_owned()),
            ("target_format".to_owned(), "png".to_owned()),
            ("purpose".to_owned(), "validation-only".to_owned()),
        ]),
        estimated_temporary_bytes: None,
    };
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: Uuid::new_v4(),
        plan_hash: String::new(),
        input_fingerprint: probe.artifact.fast_fingerprint.clone(),
        target_format: "pdf".to_owned(),
        constraints: BTreeMap::from([
            ("network".to_owned(), json!("deny")),
            ("external_resources".to_owned(), json!("deny")),
            ("macros".to_owned(), json!("disabled")),
            ("isolated_user_profile".to_owned(), json!(true)),
            ("all_pdf_pages_must_render".to_owned(), json!(true)),
        ]),
        steps: vec![
            pandoc_step,
            office_step,
            structural_validation,
            render_validation,
        ],
        changes: ChangeSet {
            preserved: vec![
                "normalized textual content through the intermediate DOCX".to_owned(),
                "document structure supported by Pandoc and LibreOffice".to_owned(),
            ],
            changed: vec![
                "markup is normalized through Pandoc's default DOCX reference document".to_owned(),
                "editable structure is rendered into fixed PDF pages".to_owned(),
            ],
            dropped: vec![
                "external resources, active content, and interactive behavior".to_owned(),
            ],
            unknown: vec![
                "font substitution and pixel-level layout fidelity require fixture comparison"
                    .to_owned(),
            ],
        },
        validators: vec![
            "docx.semantic-token-digest".to_owned(),
            "office.pdf-opens".to_owned(),
            "office.pdf-page-count".to_owned(),
            "office.pdf-page-sizes".to_owned(),
            "office.pdf-all-pages-render".to_owned(),
            "office.font-diagnostics".to_owned(),
            "office.visual-drift".to_owned(),
        ],
        network_policy: NetworkPolicy::Deny,
        output_path: Some(output_path),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

pub(crate) fn validate_docx_output(
    input: &Probe,
    output: &Probe,
    plan: &Plan,
    job_id: Uuid,
) -> ValidationReport {
    let expected_digest = property(input, "semantic_token_digest");
    let observed_digest = property(output, "semantic_token_digest");
    let checks = vec![
        validation_check(
            "DOCX_PACKAGE_OPENS",
            ValidationStatus::Pass,
            json!(true),
            json!(true),
            "Native ZIP reader opened the package.",
        ),
        validation_check(
            "DOCX_TARGET_FORMAT",
            status(output.format.id == "docx"),
            json!("docx"),
            json!(output.format.id),
            "Detected output format.",
        ),
        validation_check(
            "DOCX_REQUIRED_PARTS",
            status(property(output, "required_parts_present") == json!(true)),
            json!(true),
            property(output, "required_parts_present"),
            "Required OPC parts exist.",
        ),
        validation_check(
            "DOCX_SEMANTIC_TOKEN_DIGEST",
            status(expected_digest == observed_digest),
            expected_digest,
            observed_digest,
            "Normalized Unicode token digest.",
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

fn document_format_hint(path: &Path) -> Result<&'static str> {
    let mut prefix = [0_u8; 4096];
    let read = File::open(path)
        .and_then(|mut file| file.read(&mut prefix))
        .map_err(|error| input_error("Unable to read document header", error))?;
    if prefix[..read].starts_with(b"PK\x03\x04") {
        return Ok("docx");
    }
    let text = String::from_utf8_lossy(&prefix[..read]);
    let trimmed = text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_lowercase();
    if trimmed.starts_with("<!doctype html") || trimmed.starts_with("<html") {
        return Ok("html");
    }
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("md" | "markdown") => Ok("markdown"),
        Some("html" | "htm") => Ok("html"),
        Some("docx") => Ok("docx"),
        _ => Err(unsupported("Document format is not recognized")),
    }
}

fn inspect_document_properties(path: &Path, format: &str) -> Result<BTreeMap<String, Value>> {
    if format == "docx" {
        return inspect_docx_properties(path);
    }
    let metadata = path
        .metadata()
        .map_err(|error| input_error("Unable to inspect document size", error))?;
    if metadata.len() > MAX_MARKUP_BYTES {
        return Err(FormatWrightError::new(
            ErrorCode::ResourceExhausted,
            Stage::Inspect,
            "Markup input exceeds the 16 MiB alpha limit",
            "Split the document or use a future streaming adapter.",
        ));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|error| input_error("Markup must be valid UTF-8", error))?;
    let has_external_resource = detects_external_resource(&text, format);
    let visible = if format == "html" {
        html_text(&text)?
    } else {
        text
    };
    let mut properties = properties_for_text(&visible, false);
    properties.insert(
        "has_external_resource".to_owned(),
        json!(has_external_resource),
    );
    Ok(properties)
}

fn inspect_docx_properties(path: &Path) -> Result<BTreeMap<String, Value>> {
    let file = File::open(path).map_err(|error| input_error("Unable to open DOCX", error))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| input_error("Invalid DOCX ZIP package", error))?;
    if archive.decompressed_size().unwrap_or(u128::MAX) > u128::from(MAX_DOCUMENT_XML_BYTES) * 4 {
        return Err(FormatWrightError::new(
            ErrorCode::ResourceExhausted,
            Stage::Inspect,
            "DOCX expanded size exceeds the alpha safety limit",
            "Use a smaller trusted document.",
        ));
    }
    let required = ["[Content_Types].xml", "_rels/.rels", "word/document.xml"];
    let required_parts_present = required
        .iter()
        .all(|name| archive.index_for_name(name).is_some());
    if !required_parts_present {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            "DOCX is missing required OPC parts",
            "Repair the document and retry.",
        ));
    }
    let package_entries = archive.len();
    let mut document = archive
        .by_name("word/document.xml")
        .map_err(|error| input_error("Cannot open DOCX document part", error))?;
    if document.size() > MAX_DOCUMENT_XML_BYTES {
        return Err(FormatWrightError::new(
            ErrorCode::ResourceExhausted,
            Stage::Inspect,
            "DOCX document XML exceeds the alpha safety limit",
            "Use a smaller document.",
        ));
    }
    let mut xml = String::new();
    document
        .read_to_string(&mut xml)
        .map_err(|error| input_error("DOCX XML must be UTF-8", error))?;
    let text = docx_text(&xml)?;
    let mut properties = properties_for_text(&text, true);
    properties.insert("required_parts_present".to_owned(), json!(true));
    properties.insert("package_entries".to_owned(), json!(package_entries));
    Ok(properties)
}

fn html_text(source: &str) -> Result<String> {
    let mut reader = Reader::from_str(source);
    reader.config_mut().trim_text(false);
    let mut output = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Text(text)) => {
                output.push_str(
                    &text
                        .decode()
                        .map_err(|error| input_error("Invalid HTML text", error))?,
                );
                output.push(' ');
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(input_error("Malformed HTML", error)),
            _ => {}
        }
    }
    Ok(output)
}

fn docx_text(source: &str) -> Result<String> {
    let mut reader = Reader::from_str(source);
    let mut output = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Text(text)) => output.push_str(
                &text
                    .decode()
                    .map_err(|error| input_error("Invalid DOCX text", error))?,
            ),
            Ok(Event::End(event))
                if event.name().as_ref().ends_with(b":p") || event.name().as_ref() == b"p" =>
            {
                output.push(' ');
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(input_error("Malformed DOCX XML", error)),
            _ => {}
        }
    }
    Ok(output)
}

fn properties_for_text(text: &str, docx: bool) -> BTreeMap<String, Value> {
    let normalized = normalized_tokens(text);
    BTreeMap::from([
        (
            "semantic_token_digest".to_owned(),
            json!(format!(
                "blake3:{}",
                blake3::hash(normalized.as_bytes()).to_hex()
            )),
        ),
        (
            "text_characters".to_owned(),
            json!(normalized.chars().count()),
        ),
        ("required_parts_present".to_owned(), json!(docx)),
    ])
}

fn normalized_tokens(text: &str) -> String {
    let mut normalized = String::new();
    let mut pending_space = false;
    for character in text.chars() {
        if character.is_alphanumeric() {
            if pending_space && !normalized.is_empty() {
                normalized.push(' ');
            }
            normalized.extend(character.to_lowercase());
            pending_space = false;
        } else {
            pending_space = true;
        }
    }
    normalized
}

fn detects_external_resource(source: &str, format: &str) -> bool {
    let lowered = source.to_ascii_lowercase();
    if format == "markdown" {
        return lowered.contains("![") && lowered.contains("](");
    }
    lowered.contains("<img") && lowered.contains("src=")
}

fn property(probe: &Probe, name: &str) -> Value {
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
        evidence: "FormatWright native document inspector".to_owned(),
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

fn unsupported(message: &str) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::Unsupported,
        Stage::Plan,
        message,
        "Choose Markdown/HTML → DOCX or install a future document adapter.",
    )
}

fn input_error(message: &str, error: impl std::fmt::Display) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Inspect,
        message,
        "Correct or replace the document and retry.",
    )
    .with_diagnostic(error.to_string())
}

#[allow(clippy::needless_pass_by_value)]
fn worker_error(error: tokio::task::JoinError) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::Internal,
        Stage::Inspect,
        "Document inspector worker failed",
        "Retry the operation.",
    )
    .with_diagnostic(error.to_string())
}
