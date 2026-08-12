use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use formatwright_engine_sdk::{EngineIdentity, LossClass, Operation};
use quick_xml::Reader;
use quick_xml::events::Event;
use serde_json::{Value, json};
use uuid::Uuid;
use zip::ZipArchive;

use crate::domain::{
    ArtifactSummary, ChangeSet, DiagnosticMessage, FormatDescriptor, FormatKind, NetworkPolicy,
    Plan, PlanStep, Probe, ProbeEvidence, ReportRedaction, SCHEMA_VERSION, StreamKind, StreamProbe,
    ValidationCheck, ValidationReport, ValidationStatus,
};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::fingerprint::identify_artifact;
use crate::planner::deterministic_plan_hash;

const MAX_OFFICE_ENTRIES: usize = 10_000;
const MAX_OFFICE_EXPANDED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_RELATIONSHIP_BYTES: u64 = 8 * 1024 * 1024;

/// Detects an OOXML family from ZIP package parts instead of its extension.
///
/// # Errors
///
/// Returns a typed input/resource error when an office-looking package is
/// malformed or exceeds bounded package limits.
pub fn office_format_hint(path: impl AsRef<Path>) -> Result<Option<&'static str>> {
    let path = path.as_ref();
    let mut prefix = [0_u8; 4];
    let read = File::open(path)
        .and_then(|mut file| file.read(&mut prefix))
        .map_err(|error| input_error(path, &error))?;
    if read < prefix.len() || prefix != *b"PK\x03\x04" {
        return Ok(None);
    }
    let extension_is_office = path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "docx" | "pptx" | "xlsx"
            )
        });
    let file = File::open(path).map_err(|error| input_error(path, &error))?;
    let mut archive = match ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(_) if !extension_is_office => return Ok(None),
        Err(error) => return Err(invalid_zip(&error)),
    };
    check_package_limits(&mut archive)?;
    Ok(detect_package_family(&mut archive))
}

/// Inspects a DOCX/PPTX/XLSX package without executing macros or external links.
///
/// # Errors
///
/// Returns a typed error for unknown, malformed, macro-bearing, externally
/// linked, or resource-exhausting packages.
pub async fn inspect_office(path: impl AsRef<Path>) -> Result<Probe> {
    let path = path.as_ref();
    let artifact = identify_artifact(path).await?;
    let owned = artifact.canonical_path.clone();
    let inspection = tokio::task::spawn_blocking(move || inspect_package(&owned))
        .await
        .map_err(|error| {
            FormatWrightError::new(
                ErrorCode::Internal,
                Stage::Inspect,
                "Office inspection worker failed",
                "Retry or report the input.",
            )
            .with_diagnostic(error.to_string())
        })??;
    if inspection.has_macros {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Inspect,
            "Office package contains a VBA project",
            "Remove macros in an isolated trusted editor, then retry with macro-free OOXML.",
        ));
    }
    if inspection.has_external_relationships {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Inspect,
            "Office package contains external relationships under deny-all policy",
            "Embed or remove external resources, then retry.",
        ));
    }
    let extension = artifact
        .canonical_path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    let extension_matches = extension.as_deref() == Some(inspection.format);
    let properties = BTreeMap::from([
        ("package_entries".to_owned(), json!(inspection.entry_count)),
        (
            "expanded_bytes".to_owned(),
            json!(inspection.expanded_bytes),
        ),
        ("required_part_present".to_owned(), json!(true)),
        ("has_macros".to_owned(), json!(false)),
        ("has_external_relationships".to_owned(), json!(false)),
    ]);
    Ok(Probe {
        schema_version: SCHEMA_VERSION,
        artifact,
        format: FormatDescriptor {
            id: inspection.format.to_owned(),
            kind: FormatKind::Document,
            mime_type: Some(mime_type(inspection.format).to_owned()),
            container: Some("zip/opc".to_owned()),
            extension_matches: Some(extension_matches),
            confidence: 1.0,
        },
        streams: vec![StreamProbe {
            index: 0,
            kind: if inspection.format == "xlsx" {
                StreamKind::RecordSet
            } else {
                StreamKind::Page
            },
            codec: Some("ooxml".to_owned()),
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
            vec![DiagnosticMessage {
                code: "EXTENSION_MISMATCH".to_owned(),
                severity: "warning".to_owned(),
                message: format!(
                    "File content is {} but its extension does not match",
                    inspection.format
                ),
            }]
        },
        evidence: ProbeEvidence {
            engine_id: "formatwright.office-inspector".to_owned(),
            engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            engine_binary_sha256: None,
        },
        duration_seconds: None,
        bit_rate: None,
    })
}

/// Plans macro-disabled `LibreOffice` conversion and all-page PDF render validation.
///
/// # Errors
///
/// Returns a planning error for unsupported inputs or incorrect engines.
#[allow(clippy::too_many_lines)]
pub fn plan_office_to_pdf(
    probe: &Probe,
    output_path: std::path::PathBuf,
    soffice: &EngineIdentity,
    pdfinfo: &EngineIdentity,
    pdftoppm: &EngineIdentity,
) -> Result<Plan> {
    if !matches!(probe.format.id.as_str(), "docx" | "pptx" | "xlsx") {
        return Err(unsupported(
            "Office-to-PDF requires DOCX, PPTX, or XLSX input",
        ));
    }
    if soffice.engine_id != "soffice"
        || pdfinfo.engine_id != "pdfinfo"
        || pdftoppm.engine_id != "pdftoppm"
    {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            "Office-to-PDF Plan was given an incorrect engine",
            "Run doctor and use soffice, pdfinfo, and pdftoppm.",
        ));
    }
    let conversion = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: format!("libreoffice.{}-to-pdf.headless", probe.format.id),
        engine: soffice.clone(),
        operation: Operation::Render,
        loss_class: LossClass::Unknown,
        arguments: BTreeMap::from([
            ("source_format".to_owned(), probe.format.id.clone()),
            ("target_format".to_owned(), "pdf".to_owned()),
            ("headless".to_owned(), "true".to_owned()),
            ("isolated_profile".to_owned(), "true".to_owned()),
            ("macros".to_owned(), "disabled".to_owned()),
            ("external_resources".to_owned(), "deny".to_owned()),
        ]),
        estimated_temporary_bytes: Some(probe.artifact.size_bytes.saturating_mul(8)),
    };
    let structural_validation = PlanStep {
        step_id: "step-2".to_owned(),
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
        step_id: "step-3".to_owned(),
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
            ("macros".to_owned(), json!("disabled")),
            ("external_resources".to_owned(), json!("deny")),
            ("isolated_user_profile".to_owned(), json!(true)),
            ("all_pdf_pages_must_render".to_owned(), json!(true)),
        ]),
        steps: vec![conversion, structural_validation, render_validation],
        changes: ChangeSet {
            preserved: vec![
                "visible document content supported by LibreOffice".to_owned(),
                "page order produced by the isolated office renderer".to_owned(),
            ],
            changed: vec![
                "editable Office structure rendered into fixed PDF pages".to_owned(),
                "active content disabled".to_owned(),
            ],
            dropped: vec![
                "macros, editable formulas/objects, transitions, and interactive behavior"
                    .to_owned(),
            ],
            unknown: vec![
                "font substitution and pixel-level layout fidelity require fixture comparison"
                    .to_owned(),
            ],
        },
        validators: vec![
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

#[allow(clippy::too_many_lines)]
pub(crate) fn validate_office_pdf_output(
    input: &Probe,
    output: &Probe,
    plan: &Plan,
    job_id: Uuid,
    rendered_page_count: usize,
    engine_diagnostic: &str,
) -> ValidationReport {
    let page_count = output
        .streams
        .iter()
        .filter(|stream| stream.kind == StreamKind::Page)
        .count();
    let page_sizes_valid = output.streams.iter().all(|stream| {
        stream
            .properties
            .get("width_points")
            .and_then(Value::as_f64)
            .is_some_and(|value| value > 0.0)
            && stream
                .properties
                .get("height_points")
                .and_then(Value::as_f64)
                .is_some_and(|value| value > 0.0)
    });
    let lower_diagnostic = engine_diagnostic.to_ascii_lowercase();
    let font_warning = ["font", "substitut", "glyph"]
        .iter()
        .any(|needle| lower_diagnostic.contains(needle));
    let checks = vec![
        check(
            "OFFICE_PDF_OPENS",
            status(output.format.id == "pdf"),
            true,
            json!("pdf"),
            json!(output.format.id),
            "pdfinfo independently opened the staged output.",
        ),
        check(
            "OFFICE_PDF_PAGE_COUNT",
            status(page_count > 0),
            true,
            json!(">=1"),
            json!(page_count),
            "Ordered page streams reported by pdfinfo.",
        ),
        check(
            "OFFICE_PDF_PAGE_SIZES",
            status(page_sizes_valid),
            true,
            json!("positive point dimensions for every page"),
            json!(page_sizes_valid),
            "Per-page PDF size metadata.",
        ),
        check(
            "OFFICE_PDF_ALL_PAGES_RENDER",
            status(rendered_page_count == page_count && page_count > 0),
            true,
            json!(page_count),
            json!(rendered_page_count),
            "pdftoppm rendered every page and native PNG decoding opened each render.",
        ),
        check(
            "OFFICE_FONT_DIAGNOSTICS",
            if font_warning {
                ValidationStatus::Warning
            } else {
                ValidationStatus::Pass
            },
            false,
            json!("no engine-reported font warning"),
            json!(if font_warning {
                "warning-detected"
            } else {
                "none-detected"
            }),
            "Bounded LibreOffice diagnostic; absence is not proof of font identity.",
        ),
        check(
            "OFFICE_VISUAL_DRIFT",
            ValidationStatus::Unknown,
            false,
            json!("fixture-calibrated visual comparison"),
            json!("not-run"),
            "Alpha validation renders all pages without a source-reference baseline.",
        ),
    ];
    let hard_status = checks
        .iter()
        .filter(|check| check.required)
        .fold(ValidationStatus::Pass, |state, check| {
            state.worst(check.status)
        });
    let report_status = if hard_status == ValidationStatus::Pass {
        ValidationStatus::Warning
    } else {
        hard_status
    };
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

#[derive(Debug)]
struct PackageInspection {
    format: &'static str,
    entry_count: usize,
    expanded_bytes: u64,
    has_macros: bool,
    has_external_relationships: bool,
}

fn inspect_package(path: &Path) -> Result<PackageInspection> {
    let file = File::open(path).map_err(|error| input_error(path, &error))?;
    let mut archive = ZipArchive::new(file).map_err(|error| invalid_zip(&error))?;
    let expanded_bytes = check_package_limits(&mut archive)?;
    let format = detect_package_family(&mut archive).ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Inspect,
            "ZIP package is not recognized as DOCX, PPTX, or XLSX",
            "Choose a supported macro-free OOXML file.",
        )
    })?;
    let mut has_macros = false;
    let mut has_external_relationships = false;
    let mut relationship_bytes = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| package_read_error(&error))?;
        let name = entry.name().replace('\\', "/").to_ascii_lowercase();
        has_macros |= name.ends_with("vbaproject.bin");
        if is_relationship_part(&name) {
            relationship_bytes = relationship_bytes.saturating_add(entry.size());
            if relationship_bytes > MAX_RELATIONSHIP_BYTES {
                return Err(resource_error(
                    "Office relationship XML exceeds the 8 MiB alpha limit",
                ));
            }
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|error| package_io_error(&error))?;
            has_external_relationships |= relationships_are_external(&bytes)?;
        }
    }
    Ok(PackageInspection {
        format,
        entry_count: archive.len(),
        expanded_bytes,
        has_macros,
        has_external_relationships,
    })
}

fn check_package_limits<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<u64> {
    if archive.len() > MAX_OFFICE_ENTRIES {
        return Err(resource_error(
            "Office package contains too many ZIP entries",
        ));
    }
    let mut expanded = 0_u64;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| package_read_error(&error))?;
        expanded = expanded.saturating_add(entry.size());
        if expanded > MAX_OFFICE_EXPANDED_BYTES {
            return Err(resource_error(
                "Office package expanded size exceeds the 1 GiB alpha limit",
            ));
        }
    }
    Ok(expanded)
}

fn detect_package_family<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Option<&'static str> {
    if archive.by_name("word/document.xml").is_ok() {
        Some("docx")
    } else if archive.by_name("ppt/presentation.xml").is_ok() {
        Some("pptx")
    } else if archive.by_name("xl/workbook.xml").is_ok() {
        Some("xlsx")
    } else {
        None
    }
}

fn relationships_are_external(bytes: &[u8]) -> Result<bool> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    loop {
        match reader.read_event() {
            Ok(Event::Start(event) | Event::Empty(event)) => {
                for attribute in event.attributes().with_checks(true) {
                    let attribute = attribute.map_err(|error| relationship_xml_error(&error))?;
                    if attribute.key.local_name().as_ref() == b"TargetMode"
                        && attribute
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .map_err(|error| relationship_xml_error(&error))?
                            .eq_ignore_ascii_case("external")
                    {
                        return Ok(true);
                    }
                }
            }
            Ok(Event::DocType(_)) => {
                return Err(FormatWrightError::new(
                    ErrorCode::PolicyBlocked,
                    Stage::Inspect,
                    "Office relationship XML contains a DTD",
                    "Remove active or external package content and retry.",
                ));
            }
            Ok(Event::Eof) => return Ok(false),
            Ok(_) => {}
            Err(error) => return Err(relationship_xml_error(&error)),
        }
    }
}

fn mime_type(format: &str) -> &'static str {
    match format {
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
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

fn check(
    code: &str,
    status: ValidationStatus,
    required: bool,
    expected: Value,
    observed: Value,
    evidence: &str,
) -> ValidationCheck {
    ValidationCheck {
        code: code.to_owned(),
        status,
        required,
        expected,
        observed,
        evidence: evidence.to_owned(),
        message: if status == ValidationStatus::Pass {
            "Office-to-PDF validation check passed.".to_owned()
        } else {
            "Office-to-PDF validation needs attention.".to_owned()
        },
    }
}

const fn status(condition: bool) -> ValidationStatus {
    if condition {
        ValidationStatus::Pass
    } else {
        ValidationStatus::Fail
    }
}

fn is_relationship_part(name: &str) -> bool {
    name.rsplit('/').next().is_some_and(|file_name| {
        file_name.eq_ignore_ascii_case(".rels")
            || Path::new(file_name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("rels"))
    })
}

fn input_error(path: &Path, error: &std::io::Error) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Inspect,
        format!("Unable to read Office input: {}", path.display()),
        "Check file permissions and storage health.",
    )
    .with_diagnostic(error.to_string())
}

fn invalid_zip(error: &zip::result::ZipError) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Inspect,
        "Office package ZIP structure is invalid",
        "Choose a complete DOCX, PPTX, or XLSX file.",
    )
    .with_diagnostic(error.to_string())
}

fn resource_error(message: &str) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::ResourceExhausted,
        Stage::Inspect,
        message,
        "Reduce or split the Office document, then retry.",
    )
}

fn package_read_error(error: &zip::result::ZipError) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Inspect,
        "Unable to read an Office package entry",
        "Choose a complete OOXML document.",
    )
    .with_diagnostic(error.to_string())
}

fn package_io_error(error: &std::io::Error) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Inspect,
        "Unable to read Office relationship XML",
        "Choose a complete OOXML document.",
    )
    .with_diagnostic(error.to_string())
}

fn relationship_xml_error(error: &impl std::fmt::Display) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Inspect,
        "Office relationship XML is malformed",
        "Repair or recreate the Office document.",
    )
    .with_diagnostic(error.to_string())
}

fn unsupported(message: &str) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::Unsupported,
        Stage::Plan,
        message,
        "Choose DOCX, PPTX, or XLSX input and PDF output.",
    )
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;

    use formatwright_engine_sdk::{Certification, EngineIdentity};
    use tempfile::NamedTempFile;
    use zip::write::SimpleFileOptions;

    use super::{inspect_office, office_format_hint, plan_office_to_pdf};

    fn engine(id: &str) -> EngineIdentity {
        EngineIdentity {
            engine_id: id.to_owned(),
            version: "test".to_owned(),
            binary_path: PathBuf::from(id),
            binary_sha256: "sha".to_owned(),
            manifest_sha256: None,
            build_configuration: None,
            certification: Certification::Experimental,
        }
    }

    fn package(required_part: &str, relationships: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("temporary package");
        {
            let mut writer = zip::ZipWriter::new(file.as_file_mut());
            let options = SimpleFileOptions::default();
            writer
                .start_file("[Content_Types].xml", options)
                .expect("content types");
            writer.write_all(b"<Types/>").expect("content types XML");
            writer
                .start_file(required_part, options)
                .expect("required part");
            writer.write_all(b"<root/>").expect("required XML");
            writer
                .start_file("_rels/.rels", options)
                .expect("relationships");
            writer
                .write_all(relationships.as_bytes())
                .expect("relationship XML");
            writer.finish().expect("finish package");
        }
        file
    }

    #[tokio::test]
    async fn detects_docx_from_package_parts_and_plans_isolation() {
        let file = package(
            "word/document.xml",
            r#"<Relationships><Relationship TargetMode="Internal"/></Relationships>"#,
        );
        assert_eq!(office_format_hint(file.path()).expect("hint"), Some("docx"));
        let probe = inspect_office(file.path()).await.expect("office probe");
        let plan = plan_office_to_pdf(
            &probe,
            PathBuf::from("output.pdf"),
            &engine("soffice"),
            &engine("pdfinfo"),
            &engine("pdftoppm"),
        )
        .expect("office Plan");
        assert_eq!(plan.target_format, "pdf");
        assert_eq!(plan.constraints["isolated_user_profile"], true);
        assert_eq!(plan.steps.len(), 3);
    }

    #[tokio::test]
    async fn blocks_external_relationships() {
        let file = package(
            "ppt/presentation.xml",
            r#"<Relationships><Relationship TargetMode="External" Target="https://example.invalid/"/></Relationships>"#,
        );
        let error = inspect_office(file.path())
            .await
            .expect_err("external link blocked");
        assert_eq!(error.code, crate::ErrorCode::PolicyBlocked);
    }
}
