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
            || (format == "plain" && matches!(extension, "txt" | "text"))
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
                    "plain" => "text/plain",
                    "svg" => "image/svg+xml",
                    "epub" => "application/epub+zip",
                    "docx" => {
                        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                    }
                    _ => unreachable!("document formats are exhaustive"),
                }
                .to_owned(),
            ),
            container: match format {
                "docx" => Some("zip/opc".to_owned()),
                "epub" => Some("zip/epub".to_owned()),
                _ => None,
            },
            extension_matches: Some(extension_matches),
            confidence: if matches!(format, "docx" | "epub") {
                1.0
            } else {
                0.85
            },
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
    if !matches!(probe.format.id.as_str(), "markdown" | "html" | "plain") {
        return Err(unsupported(
            "Pandoc DOCX input must be Markdown, HTML, or plain text",
        ));
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

/// Plans an offline Pandoc conversion from Markdown/HTML to EPUB.
///
/// # Errors
///
/// Returns `Unsupported` for other inputs, targets, or engines, and
/// `PolicyBlocked` when the markup references external resources.
pub fn plan_markup_to_epub(
    probe: &Probe,
    output_path: std::path::PathBuf,
    pandoc: &EngineIdentity,
) -> Result<Plan> {
    if !matches!(probe.format.id.as_str(), "markdown" | "html" | "plain") {
        return Err(unsupported(
            "Pandoc EPUB input must be Markdown, HTML, or plain text",
        ));
    }
    if pandoc.engine_id != "pandoc" {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            "The EPUB Plan was given the wrong engine",
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
        ("target_format".to_owned(), "epub".to_owned()),
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
        capability_id: format!("pandoc.{}-to-epub.offline", probe.format.id),
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
        target_format: "epub".to_owned(),
        constraints,
        steps: vec![step],
        changes: ChangeSet {
            preserved: vec![
                "normalized textual content".to_owned(),
                "document structure supported by Pandoc".to_owned(),
            ],
            changed: vec!["layout is rendered using Pandoc's default EPUB styling".to_owned()],
            dropped: vec!["external resources under the default deny policy".to_owned()],
            unknown: vec![
                "visual fidelity is not certified by the alpha EPUB validator".to_owned(),
            ],
        },
        validators: vec![
            "epub.container-valid".to_owned(),
            "epub.content-documents".to_owned(),
            "epub.text-coverage".to_owned(),
        ],
        network_policy: NetworkPolicy::Deny,
        output_path: Some(output_path),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

/// Plans an offline Pandoc export from a DOCX package to text markup or EPUB.
///
/// # Errors
///
/// Returns `Unsupported` for other inputs or targets and `EngineIncompatible`
/// for a non-pandoc engine.
pub fn plan_docx_markup_export(
    probe: &Probe,
    output_path: std::path::PathBuf,
    pandoc: &EngineIdentity,
    target: &str,
) -> Result<Plan> {
    let target = target.trim().trim_start_matches('.').to_ascii_lowercase();
    if probe.format.id != "docx" {
        return Err(unsupported("Pandoc export input must be a DOCX package"));
    }
    if !matches!(target.as_str(), "txt" | "md" | "html" | "epub") {
        return Err(unsupported(
            "DOCX export target must be txt, md, html, or epub",
        ));
    }
    if pandoc.engine_id != "pandoc" {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            "The DOCX export Plan was given the wrong engine",
            "Run doctor and use Pandoc.",
        ));
    }
    let arguments = BTreeMap::from([
        ("source_format".to_owned(), "docx".to_owned()),
        ("target_format".to_owned(), target.clone()),
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
        capability_id: format!("pandoc.docx-to-{target}.offline"),
        engine: pandoc.clone(),
        operation: Operation::Transform,
        loss_class: LossClass::Unknown,
        arguments,
        estimated_temporary_bytes: Some(probe.artifact.size_bytes.saturating_mul(4)),
    };
    let is_epub = target == "epub";
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: Uuid::new_v4(),
        plan_hash: String::new(),
        input_fingerprint: probe.artifact.fast_fingerprint.clone(),
        target_format: target.clone(),
        constraints,
        steps: vec![step],
        changes: ChangeSet {
            preserved: vec![
                "normalized textual content".to_owned(),
                "document structure supported by Pandoc".to_owned(),
            ],
            changed: vec![format!(
                "DOCX structure is re-serialized through Pandoc's default {target} writer"
            )],
            dropped: vec!["Word-specific layout and active content".to_owned()],
            unknown: vec![
                "semantic token order may be re-arranged by the Pandoc reader".to_owned(),
            ],
        },
        validators: if is_epub {
            vec![
                "epub.container-valid".to_owned(),
                "epub.content-documents".to_owned(),
                "epub.text-coverage".to_owned(),
            ]
        } else {
            vec!["document.text-extractable".to_owned()]
        },
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
    if !matches!(probe.format.id.as_str(), "markdown" | "html" | "plain") {
        return Err(unsupported(
            "Pandoc PDF input must be Markdown, HTML, or plain text",
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

pub(crate) fn validate_epub_output(
    input: &Probe,
    output: &Probe,
    plan: &Plan,
    job_id: Uuid,
) -> ValidationReport {
    let expected_digest = property(input, "semantic_token_digest");
    let observed_digest = property(output, "semantic_token_digest");
    let expected_chars = property(input, "text_characters").as_u64().unwrap_or(0);
    let observed_chars = property(output, "text_characters").as_u64().unwrap_or(0);
    let content_documents = property(output, "content_documents");
    let checks = vec![
        validation_check(
            "EPUB_PACKAGE_OPENS",
            ValidationStatus::Pass,
            json!(true),
            json!(true),
            "Native ZIP reader opened the package.",
        ),
        validation_check(
            "EPUB_TARGET_FORMAT",
            status(output.format.id == "epub"),
            json!("epub"),
            json!(output.format.id),
            "Detected output format.",
        ),
        validation_check(
            "EPUB_CONTENT_DOCUMENTS",
            status(content_documents.as_u64().unwrap_or(0) >= 1),
            json!(1),
            content_documents,
            "Publication contains at least one XHTML content document.",
        ),
        // EPUB publications embed navigation/toc documents that repeat chapter
        // titles and add UI strings, so the token sequence never equals the
        // input's; coverage is required while the exact digest is advisory.
        validation_check(
            "EPUB_TEXT_COVERAGE",
            status(observed_chars * 10 >= expected_chars * 8),
            json!(expected_chars),
            json!(observed_chars),
            "Output text volume covers at least 80% of the input.",
        ),
        validation_check(
            "EPUB_TEXT_FIDELITY",
            if expected_digest == observed_digest {
                ValidationStatus::Pass
            } else {
                ValidationStatus::Warning
            },
            expected_digest,
            observed_digest,
            "Digest matches, or differs only by EPUB navigation text.",
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

pub(crate) fn validate_text_export_output(
    input: &Probe,
    output: &Probe,
    plan: &Plan,
    job_id: Uuid,
) -> ValidationReport {
    let expected_digest = property(input, "semantic_token_digest");
    let observed_digest = property(output, "semantic_token_digest");
    let observed_chars = property(output, "text_characters").as_u64().unwrap_or(0);
    // Pandoc 重排段落/标题标记会改变 token 序列，因此 digest 只作 Warning，
    // 输出必须保证的是"有文字"。
    let checks = vec![
        validation_check(
            "EXPORT_TEXT_NONEMPTY",
            status(observed_chars > 0),
            json!(">0"),
            json!(observed_chars),
            "Output carries at least one normalized text character.",
        ),
        validation_check(
            "EXPORT_TEXT_FIDELITY",
            if expected_digest == observed_digest {
                ValidationStatus::Pass
            } else {
                ValidationStatus::Warning
            },
            expected_digest,
            observed_digest,
            "Digest matches, or differs only by Pandoc re-serialization.",
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
        // The OCF spec requires an uncompressed `mimetype` entry stored first,
        // so an EPUB's media type string is visible right after the local
        // file header — DOCX packages never contain it.
        if prefix[..read]
            .windows(b"application/epub+zip".len())
            .any(|window| window == b"application/epub+zip")
        {
            return Ok("epub");
        }
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
    if trimmed.starts_with("<svg") || (trimmed.starts_with("<?xml") && trimmed.contains("<svg")) {
        return Ok("svg");
    }
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("md" | "markdown") => Ok("markdown"),
        Some("txt" | "text") => Ok("plain"),
        Some("html" | "htm") => Ok("html"),
        Some("svg") => Ok("svg"),
        Some("docx") => Ok("docx"),
        Some("epub") => Ok("epub"),
        _ => Err(unsupported("Document format is not recognized")),
    }
}

fn inspect_document_properties(path: &Path, format: &str) -> Result<BTreeMap<String, Value>> {
    if format == "docx" {
        return inspect_docx_properties(path);
    }
    if format == "epub" {
        return inspect_epub_properties(path);
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
    let visible = if matches!(format, "html" | "svg") {
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

/// `LibreOffice` 等工具会在本地头置 data-descriptor 标志（flag bit 3），使
/// `ZipArchive::decompressed_size()` 返回 `None`；此时回退为按中央目录逐项
/// 求和，而不是把"未知"当成超限。
fn bounded_expanded_bytes(archive: &mut ZipArchive<File>) -> u64 {
    if let Some(total) = archive.decompressed_size()
        && let Ok(total) = u64::try_from(total)
    {
        return total;
    }
    (0..archive.len())
        .map(|index| archive.by_index(index).map_or(0, |entry| entry.size()))
        .fold(0_u64, u64::saturating_add)
}

fn inspect_docx_properties(path: &Path) -> Result<BTreeMap<String, Value>> {
    let file = File::open(path).map_err(|error| input_error("Unable to open DOCX", error))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| input_error("Invalid DOCX ZIP package", error))?;
    if bounded_expanded_bytes(&mut archive) > MAX_DOCUMENT_XML_BYTES * 4 {
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

fn inspect_epub_properties(path: &Path) -> Result<BTreeMap<String, Value>> {
    let file = File::open(path).map_err(|error| input_error("Unable to open EPUB", error))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| input_error("Invalid EPUB ZIP package", error))?;
    if bounded_expanded_bytes(&mut archive) > MAX_DOCUMENT_XML_BYTES * 4 {
        return Err(FormatWrightError::new(
            ErrorCode::ResourceExhausted,
            Stage::Inspect,
            "EPUB expanded size exceeds the alpha safety limit",
            "Use a smaller trusted document.",
        ));
    }
    // OCF requires `mimetype` to be the first entry, stored uncompressed, with
    // exactly the EPUB media type; `META-INF/container.xml` points at the OPF.
    let mut mimetype = String::new();
    let first_entry_valid = archive
        .by_index_raw(0)
        .ok()
        .and_then(|entry| (entry.name() == "mimetype").then_some(entry))
        .and_then(|mut entry| entry.read_to_string(&mut mimetype).ok())
        .is_some_and(|_| {
            mimetype == "application/epub+zip"
                && archive.index_for_name("META-INF/container.xml").is_some()
        });
    if !first_entry_valid {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            "EPUB is missing the OCF mimetype/container parts",
            "Repair the publication and retry.",
        ));
    }
    let mut content_names: Vec<String> = archive
        .file_names()
        .filter(|name| {
            Path::new(name)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    matches!(
                        extension.to_ascii_lowercase().as_str(),
                        "xhtml" | "html" | "htm"
                    )
                })
        })
        .map(str::to_owned)
        .collect();
    content_names.sort();
    let mut text = String::new();
    for name in &content_names {
        if text.len() > usize::try_from(MAX_MARKUP_BYTES).unwrap_or(usize::MAX) {
            break;
        }
        let mut content = String::new();
        if let Ok(mut entry) = archive.by_name(name)
            && entry.size() <= MAX_MARKUP_BYTES
            && entry.read_to_string(&mut content).is_ok()
        {
            text.push_str(&content);
            text.push('\n');
        }
    }
    let visible = html_text(&text)?;
    let mut properties = properties_for_text(&visible, false);
    properties.insert("content_documents".to_owned(), json!(content_names.len()));
    properties.insert("package_entries".to_owned(), json!(archive.len()));
    Ok(properties)
}

fn html_text(source: &str) -> Result<String> {
    let mut reader = Reader::from_str(source);
    reader.config_mut().trim_text(false);
    // HTML void elements (`<meta>`, `<br>`, `<img>`) never close, so XML-style
    // end-name matching would reject every real-world HTML head as malformed.
    reader.config_mut().check_end_names = false;
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

pub(crate) fn normalized_tokens(text: &str) -> String {
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
    if format == "svg" {
        // Any raster `<image>` placement breaks the vector-output promise, so it
        // is denied together with external links under the deny-all policy.
        return lowered.contains("<image");
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

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{detects_external_resource, inspect_document};

    #[tokio::test]
    async fn svg_documents_are_detected_by_prefix_and_extension() {
        let directory = tempdir().expect("temporary directory");
        let inline = directory.path().join("drawing.svg");
        fs::write(&inline, "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>")
            .expect("write inline SVG");
        let probe = inspect_document(&inline)
            .await
            .expect("inline SVG inspection");
        assert_eq!(probe.format.id, "svg");
        assert_eq!(probe.format.mime_type.as_deref(), Some("image/svg+xml"));

        let declared = directory.path().join("declared.svg");
        fs::write(
            &declared,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\">\
             <text>Hello 123</text></svg>",
        )
        .expect("write XML-declared SVG");
        let probe = inspect_document(&declared)
            .await
            .expect("XML-declared SVG inspection");
        assert_eq!(probe.format.id, "svg");
        assert!(
            probe.streams[0].properties.contains_key("text_characters"),
            "SVG text content is inspected"
        );
    }

    #[test]
    fn svg_raster_images_are_denied_but_plain_svg_text_is_not() {
        assert!(detects_external_resource(
            "<svg xmlns=\"a\"><image href=\"photo.png\"/></svg>",
            "svg"
        ));
        assert!(!detects_external_resource(
            "<svg xmlns=\"a\"><rect width=\"4\" height=\"4\"/><text>ELECTRIC</text></svg>",
            "svg"
        ));
    }

    #[tokio::test]
    async fn html_with_void_elements_is_still_inspectable() {
        // Real-world HTML heads carry void elements (`<meta>`) that never
        // close; the text extractor must tolerate them.
        let directory = tempdir().expect("temporary directory");
        let input = directory.path().join("page.html");
        fs::write(
            &input,
            "<!DOCTYPE html>\n<html lang=\"zh-CN\">\n<head>\n<meta charset=\"UTF-8\">\n\
             <title>Carton 440</title>\n</head>\n<body><p>ELECTRIC 440010147700</p></body>\n</html>\n",
        )
        .expect("write HTML fixture");
        let probe = inspect_document(&input)
            .await
            .expect("HTML with a void <meta> must inspect");
        assert_eq!(probe.format.id, "html");
        assert!(probe.streams[0].properties.contains_key("text_characters"));
    }

    #[tokio::test]
    async fn plain_text_files_are_inspected_as_plain_format() {
        let directory = tempdir().expect("temporary directory");
        let input = directory.path().join("notes.txt");
        fs::write(&input, "ELECTRIC 440 notes\nsecond line 12345").expect("write txt fixture");
        let probe = inspect_document(&input)
            .await
            .expect("plain text inspection");
        assert_eq!(probe.format.id, "plain");
        assert_eq!(probe.format.mime_type.as_deref(), Some("text/plain"));
        assert_eq!(probe.format.extension_matches, Some(true));
        assert!(probe.streams[0].properties.contains_key("text_characters"));

        let uppercase = directory.path().join("NOTES.TEXT");
        fs::write(&uppercase, "windows uppercase extension").expect("write .TEXT fixture");
        let probe = inspect_document(&uppercase)
            .await
            .expect("uppercase extension inspection");
        assert_eq!(probe.format.id, "plain");
    }

    fn write_minimal_epub(path: &std::path::Path) {
        use std::io::Write;
        use zip::CompressionMethod;
        use zip::write::SimpleFileOptions;

        let file = fs::File::create(path).expect("create EPUB fixture");
        let mut archive = zip::ZipWriter::new(file);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        archive
            .start_file("mimetype", stored)
            .expect("mimetype entry");
        archive
            .write_all(b"application/epub+zip")
            .expect("write mimetype");
        archive
            .start_file("META-INF/container.xml", SimpleFileOptions::default())
            .expect("container entry");
        archive
            .write_all(b"<?xml version=\"1.0\"?><container/>")
            .expect("write container");
        archive
            .start_file("OEBPS/chapter1.xhtml", SimpleFileOptions::default())
            .expect("content entry");
        archive
            .write_all(b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>ELECTRIC 440</p></body></html>")
            .expect("write content");
        archive.finish().expect("finish EPUB");
    }

    #[tokio::test]
    async fn epub_packages_are_detected_and_inspected() {
        let directory = tempdir().expect("temporary directory");
        let input = directory.path().join("book.epub");
        write_minimal_epub(&input);
        let probe = inspect_document(&input)
            .await
            .expect("minimal EPUB inspection");
        assert_eq!(probe.format.id, "epub");
        assert_eq!(
            probe.format.mime_type.as_deref(),
            Some("application/epub+zip")
        );
        assert_eq!(probe.format.container.as_deref(), Some("zip/epub"));
        let properties = &probe.streams[0].properties;
        assert_eq!(
            properties
                .get("content_documents")
                .and_then(serde_json::Value::as_u64),
            Some(1),
            "one XHTML content document is counted"
        );
        assert!(
            properties.contains_key("semantic_token_digest"),
            "EPUB text content is inspected"
        );
    }

    #[test]
    fn epub_magic_distinguishes_epub_from_docx_prefixes() {
        use super::document_format_hint;

        let directory = tempdir().expect("temporary directory");

        let epub = directory.path().join("book.epub");
        write_minimal_epub(&epub);
        assert_eq!(document_format_hint(&epub).expect("epub hint"), "epub");

        // A ZIP without the OCF mimetype payload must keep routing to DOCX
        // inspection (which then rejects it for missing OPC parts).
        let docx_like = directory.path().join("plain.zip");
        let file = fs::File::create(&docx_like).expect("create plain zip");
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file("hello.txt", zip::write::SimpleFileOptions::default())
            .expect("plain entry");
        std::io::Write::write_all(&mut archive, b"hello").expect("write plain");
        archive.finish().expect("finish plain zip");
        assert_eq!(document_format_hint(&docx_like).expect("docx hint"), "docx");
    }

    fn write_minimal_docx(path: &std::path::Path) {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let file = fs::File::create(path).expect("create DOCX fixture");
        let mut archive = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        archive
            .start_file("[Content_Types].xml", options)
            .expect("content types entry");
        archive.write_all(b"<Types/>").expect("content types");
        archive
            .start_file("_rels/.rels", options)
            .expect("rels entry");
        archive.write_all(b"<Relationships/>").expect("rels");
        archive
            .start_file("word/document.xml", options)
            .expect("document entry");
        archive
            .write_all(
                b"<?xml version=\"1.0\"?><w:document xmlns:w=\"ns\"><w:body>\
                 <w:p><w:r><w:t>ELECTRIC 440</w:t></w:r></w:p></w:body></w:document>",
            )
            .expect("document XML");
        archive.finish().expect("finish DOCX");
    }

    fn pandoc_engine() -> formatwright_engine_sdk::EngineIdentity {
        formatwright_engine_sdk::EngineIdentity {
            engine_id: "pandoc".to_owned(),
            version: "3.8".to_owned(),
            binary_path: std::path::PathBuf::from("pandoc.exe"),
            binary_sha256: "0".repeat(64),
            manifest_sha256: None,
            build_configuration: None,
            certification: formatwright_engine_sdk::Certification::Unverified,
        }
    }

    #[tokio::test]
    async fn plan_docx_markup_export_builds_pandoc_plans_for_all_text_targets() {
        let directory = tempdir().expect("temporary directory");
        let input = directory.path().join("report.docx");
        write_minimal_docx(&input);
        let probe = inspect_document(&input).await.expect("DOCX inspection");
        assert_eq!(probe.format.id, "docx");
        let pandoc = pandoc_engine();
        for (target, validator) in [
            ("txt", "document.text-extractable"),
            ("md", "document.text-extractable"),
            ("html", "document.text-extractable"),
            ("epub", "epub.container-valid"),
        ] {
            let plan = super::plan_docx_markup_export(
                &probe,
                directory.path().join(format!("out.{target}")),
                &pandoc,
                target,
            )
            .unwrap_or_else(|error| panic!("{target} plan: {error}"));
            assert_eq!(plan.target_format, target);
            assert_eq!(
                plan.steps[0]
                    .arguments
                    .get("source_format")
                    .map(String::as_str),
                Some("docx")
            );
            assert!(plan.validators.iter().any(|value| value == validator));
            assert!(!plan.plan_hash.is_empty());
        }
        // 非 docx 输入 / 非法目标 / 错误引擎都要被拒绝。
        let markdown = directory.path().join("chapter.md");
        fs::write(&markdown, "# Title").expect("write markdown");
        let markdown_probe = inspect_document(&markdown)
            .await
            .expect("markdown inspection");
        assert!(
            super::plan_docx_markup_export(
                &markdown_probe,
                directory.path().join("o.txt"),
                &pandoc,
                "txt"
            )
            .is_err(),
            "markdown input is rejected"
        );
        assert!(
            super::plan_docx_markup_export(&probe, directory.path().join("o.pdf"), &pandoc, "pdf")
                .is_err(),
            "pdf target is rejected"
        );
        let wrong = formatwright_engine_sdk::EngineIdentity {
            engine_id: "soffice".to_owned(),
            ..pandoc
        };
        assert!(
            super::plan_docx_markup_export(&probe, directory.path().join("o.txt"), &wrong, "txt")
                .is_err(),
            "non-pandoc engine is rejected"
        );
    }

    #[tokio::test]
    async fn validate_text_export_output_requires_text_and_warns_on_digest_drift() {
        use uuid::Uuid;

        let directory = tempdir().expect("temporary directory");
        let input = directory.path().join("report.docx");
        write_minimal_docx(&input);
        let input_probe = inspect_document(&input).await.expect("DOCX inspection");
        let pandoc = pandoc_engine();
        let output = directory.path().join("out.txt");
        let plan = super::plan_docx_markup_export(&input_probe, output.clone(), &pandoc, "txt")
            .expect("txt plan");

        // 空输出 → EXPORT_TEXT_NONEMPTY 必须 Fail。
        fs::write(&output, "").expect("empty txt");
        let empty_probe = inspect_document(&output).await.expect("empty inspection");
        let empty_report =
            super::validate_text_export_output(&input_probe, &empty_probe, &plan, Uuid::new_v4());
        assert!(
            empty_report
                .checks
                .iter()
                .any(|check| check.code == "EXPORT_TEXT_NONEMPTY"
                    && check.status == crate::domain::ValidationStatus::Fail)
        );

        // 有文字但顺序不同 → NONEMPTY Pass、FIDELITY 最多 Warning。
        fs::write(&output, "440 ELECTIC reordered words").expect("reordered txt");
        let text_probe = inspect_document(&output).await.expect("txt inspection");
        let report =
            super::validate_text_export_output(&input_probe, &text_probe, &plan, Uuid::new_v4());
        assert_ne!(report.status, crate::domain::ValidationStatus::Fail);
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.code == "EXPORT_TEXT_FIDELITY")
        );
    }

    #[tokio::test]
    async fn plan_markup_to_epub_builds_a_validated_pandoc_plan() {
        use formatwright_engine_sdk::Certification;

        let directory = tempdir().expect("temporary directory");
        let input = directory.path().join("chapter.md");
        fs::write(&input, "# Title\n\nELECTRIC 440 text").expect("write markdown");
        let probe = inspect_document(&input).await.expect("markdown inspection");
        let pandoc = formatwright_engine_sdk::EngineIdentity {
            engine_id: "pandoc".to_owned(),
            version: "3.8".to_owned(),
            binary_path: std::path::PathBuf::from("pandoc.exe"),
            binary_sha256: "0".repeat(64),
            manifest_sha256: None,
            build_configuration: None,
            certification: Certification::Unverified,
        };
        let plan = super::plan_markup_to_epub(&probe, directory.path().join("book.epub"), &pandoc)
            .expect("EPUB plan");
        assert_eq!(plan.target_format, "epub");
        assert_eq!(
            plan.steps[0]
                .arguments
                .get("target_format")
                .map(String::as_str),
            Some("epub")
        );
        assert!(
            plan.validators
                .iter()
                .any(|validator| validator == "epub.text-coverage")
        );
        assert!(!plan.plan_hash.is_empty(), "plan hash is computed");

        let wrong_engine = formatwright_engine_sdk::EngineIdentity {
            engine_id: "soffice".to_owned(),
            ..pandoc
        };
        assert!(
            super::plan_markup_to_epub(&probe, directory.path().join("out.epub"), &wrong_engine)
                .is_err(),
            "the EPUB plan rejects a non-pandoc engine"
        );
    }
}
