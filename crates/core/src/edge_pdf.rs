use std::collections::BTreeMap;
use std::path::Path;

use formatwright_engine_sdk::{EngineIdentity, LossClass, Operation};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::document::normalized_tokens;
use crate::domain::{
    ArtifactSummary, ChangeSet, NetworkPolicy, Plan, PlanStep, Probe, ReportRedaction,
    SCHEMA_VERSION, ValidationCheck, ValidationReport, ValidationStatus,
};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::planner::deterministic_plan_hash;

const POPPLER_UTILITY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Evidence gathered from the staged PDF by independent Poppler utilities.
#[derive(Debug)]
pub(crate) struct EdgePrintEvidence {
    pub extracted_text: String,
    pub font_table: String,
}

/// Plans an offline browser print of HTML/SVG into a vector PDF plus
/// structural, render, text-layer, and font-embedding validation.
///
/// # Errors
///
/// Returns a planning error for unsupported input, external resources, or an
/// incorrectly selected engine.
#[allow(clippy::too_many_lines)]
pub fn plan_edge_print_to_pdf(
    probe: &Probe,
    output_path: std::path::PathBuf,
    msedge: &EngineIdentity,
    pdfinfo: &EngineIdentity,
    pdftoppm: &EngineIdentity,
    pdftotext: &EngineIdentity,
    pdffonts: &EngineIdentity,
) -> Result<Plan> {
    if !matches!(probe.format.id.as_str(), "html" | "svg") {
        return Err(unsupported("Browser print input must be HTML or SVG"));
    }
    if property(probe, "has_external_resource") == json!(true) {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Plan,
            "Markup contains an external image/resource under deny-all policy",
            "Remove the resource or wait for an explicitly authorized resource-root policy.",
        ));
    }
    if msedge.engine_id != "msedge"
        || pdfinfo.engine_id != "pdfinfo"
        || pdftoppm.engine_id != "pdftoppm"
        || pdftotext.engine_id != "pdftotext"
        || pdffonts.engine_id != "pdffonts"
    {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            "The browser-print Plan was given an incorrect engine",
            "Run doctor and use msedge, pdfinfo, pdftoppm, pdftotext, and pdffonts.",
        ));
    }
    let print_step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: format!("edge.{}-to-pdf.vector-print", probe.format.id),
        engine: msedge.clone(),
        operation: Operation::Render,
        loss_class: LossClass::None,
        arguments: BTreeMap::from([
            ("source_format".to_owned(), probe.format.id.clone()),
            ("target_format".to_owned(), "pdf".to_owned()),
            ("headless".to_owned(), "true".to_owned()),
            ("isolated_profile".to_owned(), "true".to_owned()),
            ("network".to_owned(), "deny".to_owned()),
            ("external_resources".to_owned(), "deny".to_owned()),
        ]),
        estimated_temporary_bytes: Some(probe.artifact.size_bytes.saturating_mul(6)),
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
    let text_validation = PlanStep {
        step_id: "step-4".to_owned(),
        capability_id: "poppler.pdf-text-layer-validation.all-pages".to_owned(),
        engine: pdftotext.clone(),
        operation: Operation::Inspect,
        loss_class: LossClass::None,
        arguments: BTreeMap::from([
            ("layout".to_owned(), "reading-order".to_owned()),
            ("target_format".to_owned(), "text".to_owned()),
            ("purpose".to_owned(), "validation-only".to_owned()),
        ]),
        estimated_temporary_bytes: None,
    };
    let font_validation = PlanStep {
        step_id: "step-5".to_owned(),
        capability_id: "poppler.pdf-font-embedding-validation.all-pages".to_owned(),
        engine: pdffonts.clone(),
        operation: Operation::Inspect,
        loss_class: LossClass::None,
        arguments: BTreeMap::from([
            ("embedded".to_owned(), "required".to_owned()),
            ("target_format".to_owned(), "pdf".to_owned()),
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
            ("isolated_user_profile".to_owned(), json!(true)),
            ("all_pdf_pages_must_render".to_owned(), json!(true)),
            ("text_must_remain_extractable".to_owned(), json!(true)),
        ]),
        steps: vec![
            print_step,
            structural_validation,
            render_validation,
            text_validation,
            font_validation,
        ],
        changes: ChangeSet {
            preserved: vec![
                "text is printed as selectable, searchable vector text".to_owned(),
                "vector graphics are emitted as PDF drawing operations".to_owned(),
            ],
            changed: vec![
                "markup is paginated by the installed browser print engine into fixed PDF pages"
                    .to_owned(),
            ],
            dropped: vec!["external resources, scripts, and interactive behavior".to_owned()],
            unknown: vec![
                "browser font substitution fidelity requires fixture comparison".to_owned(),
            ],
        },
        validators: vec![
            "edge.pdf-opens".to_owned(),
            "edge.pdf-page-count".to_owned(),
            "edge.pdf-page-sizes".to_owned(),
            "edge.pdf-all-pages-render".to_owned(),
            "edge.pdf-text-layer".to_owned(),
            "edge.pdf-fonts-embedded".to_owned(),
            "edge.pdf-text-fidelity".to_owned(),
        ],
        network_policy: NetworkPolicy::Deny,
        output_path: Some(output_path),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

/// Validates a browser-printed PDF: structure, page rendering, an extractable
/// text layer, embedded fonts, and non-required text-fidelity evidence.
#[allow(clippy::too_many_lines)]
pub(crate) fn validate_edge_pdf_output(
    input: &Probe,
    output: &Probe,
    plan: &Plan,
    job_id: Uuid,
    rendered_page_count: usize,
    evidence: &EdgePrintEvidence,
) -> ValidationReport {
    let page_count = output
        .streams
        .iter()
        .filter(|stream| stream.kind == crate::domain::StreamKind::Page)
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
    let input_characters = property(input, "text_characters")
        .as_u64()
        .unwrap_or_default();
    let extracted_characters =
        u64::try_from(normalized_tokens(&evidence.extracted_text).chars().count())
            .unwrap_or(u64::MAX);
    let text_expected = input_characters > 0;
    let fonts = parse_pdffonts_table(&evidence.font_table);
    let fidelity = text_fidelity(input_characters, extracted_characters);
    let checks = vec![
        check(
            "EDGE_PDF_OPENS",
            status(output.format.id == "pdf"),
            true,
            json!("pdf"),
            json!(output.format.id),
            "pdfinfo independently opened the staged output.",
        ),
        check(
            "EDGE_PDF_PAGE_COUNT",
            status(page_count > 0),
            true,
            json!(">=1"),
            json!(page_count),
            "Ordered page streams reported by pdfinfo.",
        ),
        check(
            "EDGE_PDF_PAGE_SIZES",
            status(page_sizes_valid),
            true,
            json!("positive point dimensions for every page"),
            json!(page_sizes_valid),
            "Per-page PDF size metadata.",
        ),
        check(
            "EDGE_PDF_ALL_PAGES_RENDER",
            status(rendered_page_count == page_count && page_count > 0),
            true,
            json!(page_count),
            json!(rendered_page_count),
            "pdftoppm rendered every page and native PNG decoding opened each render.",
        ),
        check(
            "EDGE_PDF_TEXT_EXTRACTABLE",
            status(!text_expected || extracted_characters > 0),
            true,
            json!(if text_expected {
                ">0 extractable characters"
            } else {
                "input declares no text; nothing to extract"
            }),
            json!(extracted_characters),
            "pdftotext independently extracted a text layer from the printed PDF.",
        ),
        check(
            "EDGE_PDF_FONTS_EMBEDDED",
            status(fonts.unembedded.is_empty()),
            true,
            json!(if fonts.font_count == 0 {
                "no fonts declared (pure vector graphics) or all fonts embedded"
            } else {
                "every declared font embedded"
            }),
            json!(format!(
                "{}/{} embedded{}",
                fonts.embedded_count,
                fonts.font_count,
                if fonts.unembedded.is_empty() {
                    String::new()
                } else {
                    format!("; missing: {}", fonts.unembedded.join(", "))
                }
            )),
            "pdffonts reported the embedding state of every declared font.",
        ),
        check(
            "EDGE_PDF_TEXT_FIDELITY",
            if !text_expected || fidelity >= 0.5 {
                ValidationStatus::Pass
            } else {
                ValidationStatus::Warning
            },
            false,
            json!("extracted characters preserve at least half of the input text"),
            json!(format!("{fidelity:.2}")),
            "Non-required heuristic ratio; extraction loses hyphenation, ligatures, and layout.",
        ),
    ];
    let hard_status = checks
        .iter()
        .filter(|check| check.required)
        .fold(ValidationStatus::Pass, |state, check| {
            state.worst(check.status)
        });
    let report_status = if hard_status == ValidationStatus::Pass {
        checks.iter().fold(ValidationStatus::Pass, |state, check| {
            state.worst(check.status)
        })
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

/// Extracts the PDF text layer with `pdftotext` into bounded memory.
///
/// # Errors
///
/// Returns an engine error when the utility cannot start, times out, or exits
/// non-zero.
pub(crate) async fn extract_pdf_text(engine: &EngineIdentity, pdf: &Path) -> Result<String> {
    let mut command = tokio::process::Command::new(&engine.binary_path);
    command
        .arg("-q")
        .arg("--")
        .arg(pdf)
        .arg("-")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(POPPLER_UTILITY_TIMEOUT, command.output())
        .await
        .map_err(|_| poppler_timeout("pdftotext"))?
        .map_err(|error| poppler_start_failure("pdftotext", &error))?;
    if !output.status.success() {
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "The PDF text-layer validator exited with an error",
            "Run doctor and verify pdftotext.",
        )
        .with_diagnostic(bounded_utf8(&output.stderr)));
    }
    Ok(bounded_utf8(&output.stdout))
}

/// Reads the font table of a PDF with `pdffonts` into bounded memory.
///
/// # Errors
///
/// Returns an engine error when the utility cannot start, times out, or exits
/// non-zero.
pub(crate) async fn inspect_pdf_font_table(engine: &EngineIdentity, pdf: &Path) -> Result<String> {
    let mut command = tokio::process::Command::new(&engine.binary_path);
    command
        .arg("--")
        .arg(pdf)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(POPPLER_UTILITY_TIMEOUT, command.output())
        .await
        .map_err(|_| poppler_timeout("pdffonts"))?
        .map_err(|error| poppler_start_failure("pdffonts", &error))?;
    if !output.status.success() {
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "The PDF font-embedding validator exited with an error",
            "Run doctor and verify pdffonts.",
        )
        .with_diagnostic(bounded_utf8(&output.stderr)));
    }
    Ok(bounded_utf8(&output.stdout))
}

/// Summarizes `pdffonts` output for the embedding check.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct FontEmbeddingSummary {
    pub font_count: usize,
    pub embedded_count: usize,
    pub unembedded: Vec<String>,
}

/// Parses `pdffonts` stdout. Columns from the right are fixed
/// (`emb sub uni object ID` preceded by `encoding`), so embedding state is the
/// fifth token from the end; the font name is taken from the first token
/// because variable-width `type` values make an exact left-boundary split
/// unreliable.
pub(crate) fn parse_pdffonts_table(stdout: &str) -> FontEmbeddingSummary {
    let mut summary = FontEmbeddingSummary::default();
    for line in stdout.lines().skip(2) {
        if line.starts_with('-') {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 8 {
            continue;
        }
        let Some(embedded) = tokens.iter().rev().nth(4) else {
            continue;
        };
        summary.font_count += 1;
        if embedded.eq_ignore_ascii_case("yes") {
            summary.embedded_count += 1;
        } else {
            summary.unembedded.push(tokens[0].to_owned());
        }
    }
    summary
}

/// Character counts beyond 2^53 cannot occur for inspected markup inputs, so
/// the ratio cast is exact for every real document.
#[allow(clippy::cast_precision_loss)]
fn text_fidelity(input_characters: u64, extracted_characters: u64) -> f64 {
    if input_characters == 0 {
        return 1.0;
    }
    (extracted_characters as f64 / input_characters as f64).clamp(0.0, 1.0)
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

fn check(
    code: &str,
    status: ValidationStatus,
    required: bool,
    expected: Value,
    observed: Value,
    message: &str,
) -> ValidationCheck {
    ValidationCheck {
        code: code.to_owned(),
        status,
        required,
        expected,
        observed,
        evidence: "Independent Poppler validators over the staged browser-printed PDF".to_owned(),
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
        "Choose HTML/SVG to PDF or another supported document route.",
    )
}

fn poppler_timeout(name: &str) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::ExecutionFailed,
        Stage::Validate,
        format!("The {name} validator timed out"),
        "Inspect the generated PDF and retry.",
    )
}

fn poppler_start_failure(name: &str, error: &std::io::Error) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::EngineIncompatible,
        Stage::Validate,
        format!("Unable to start the {name} validator"),
        "Run doctor and verify the Poppler utilities.",
    )
    .with_diagnostic(error.to_string())
}

fn bounded_utf8(bytes: &[u8]) -> String {
    const MAX_BYTES: usize = 64 * 1024;
    let start = bytes.len().saturating_sub(MAX_BYTES);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use formatwright_engine_sdk::Certification;
    use serde_json::json;

    use super::{
        EdgePrintEvidence, FontEmbeddingSummary, parse_pdffonts_table, plan_edge_print_to_pdf,
        text_fidelity, validate_edge_pdf_output,
    };
    use crate::domain::{
        ArtifactIdentity, FormatDescriptor, FormatKind, Probe, ProbeEvidence, StreamKind,
        StreamProbe,
    };

    fn engine(engine_id: &str) -> formatwright_engine_sdk::EngineIdentity {
        formatwright_engine_sdk::EngineIdentity {
            engine_id: engine_id.to_owned(),
            version: "test".to_owned(),
            binary_path: PathBuf::from("engine.bin"),
            binary_sha256: "0".repeat(64),
            manifest_sha256: None,
            build_configuration: None,
            certification: Certification::Unverified,
        }
    }

    fn probe(format_id: &str, properties: BTreeMap<String, serde_json::Value>) -> Probe {
        Probe {
            schema_version: 1,
            artifact: ArtifactIdentity {
                display_path: format!("fixture.{format_id}"),
                canonical_path: PathBuf::from(format!("fixture.{format_id}")),
                size_bytes: 128,
                modified_unix_ms: 0,
                fast_fingerprint: "fp".to_owned(),
                full_blake3: None,
            },
            format: FormatDescriptor {
                id: format_id.to_owned(),
                kind: FormatKind::Document,
                mime_type: None,
                container: None,
                extension_matches: Some(true),
                confidence: 0.85,
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
            warnings: Vec::new(),
            evidence: ProbeEvidence {
                engine_id: "formatwright.document-inspector".to_owned(),
                engine_version: "test".to_owned(),
                engine_binary_sha256: None,
            },
            duration_seconds: None,
            bit_rate: None,
        }
    }

    fn pdf_page(properties: BTreeMap<String, serde_json::Value>) -> StreamProbe {
        StreamProbe {
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
        }
    }

    #[test]
    fn plan_uses_five_steps_and_vector_loss_class() {
        let input = probe(
            "html",
            BTreeMap::from([("has_external_resource".to_owned(), json!(false))]),
        );
        let plan = plan_edge_print_to_pdf(
            &input,
            PathBuf::from("out.pdf"),
            &engine("msedge"),
            &engine("pdfinfo"),
            &engine("pdftoppm"),
            &engine("pdftotext"),
            &engine("pdffonts"),
        )
        .expect("browser-print plan");
        assert_eq!(plan.steps.len(), 5);
        assert_eq!(plan.steps[0].capability_id, "edge.html-to-pdf.vector-print");
        assert_eq!(plan.steps[0].engine.engine_id, "msedge");
        assert_eq!(
            plan.steps[0].loss_class,
            formatwright_engine_sdk::LossClass::None
        );
        assert_eq!(plan.steps[4].engine.engine_id, "pdffonts");
        assert!(plan.validators.contains(&"edge.pdf-text-layer".to_owned()));
        assert!(!plan.plan_hash.is_empty());
    }

    #[test]
    fn plan_rejects_the_wrong_engine_and_external_resources() {
        let input = probe(
            "html",
            BTreeMap::from([("has_external_resource".to_owned(), json!(false))]),
        );
        let error = plan_edge_print_to_pdf(
            &input,
            PathBuf::from("out.pdf"),
            &engine("soffice"),
            &engine("pdfinfo"),
            &engine("pdftoppm"),
            &engine("pdftotext"),
            &engine("pdffonts"),
        )
        .expect_err("engine mismatch must be rejected");
        assert_eq!(error.code, crate::ErrorCode::EngineIncompatible);

        let linked = probe(
            "html",
            BTreeMap::from([("has_external_resource".to_owned(), json!(true))]),
        );
        let error = plan_edge_print_to_pdf(
            &linked,
            PathBuf::from("out.pdf"),
            &engine("msedge"),
            &engine("pdfinfo"),
            &engine("pdftoppm"),
            &engine("pdftotext"),
            &engine("pdffonts"),
        )
        .expect_err("external resources are denied");
        assert_eq!(error.code, crate::ErrorCode::PolicyBlocked);
    }

    #[test]
    fn validation_passes_when_text_fonts_and_rendering_hold() {
        let input = probe(
            "html",
            BTreeMap::from([
                ("has_external_resource".to_owned(), json!(false)),
                ("text_characters".to_owned(), json!(40)),
            ]),
        );
        let plan = plan_edge_print_to_pdf(
            &input,
            PathBuf::from("out.pdf"),
            &engine("msedge"),
            &engine("pdfinfo"),
            &engine("pdftoppm"),
            &engine("pdftotext"),
            &engine("pdffonts"),
        )
        .expect("plan");
        let mut output = probe(
            "pdf",
            BTreeMap::from([
                ("width_points".to_owned(), json!(420.0)),
                ("height_points".to_owned(), json!(293.0)),
            ]),
        );
        output.streams.push(pdf_page(BTreeMap::from([
            ("width_points".to_owned(), json!(420.0)),
            ("height_points".to_owned(), json!(293.0)),
        ])));
        let evidence = EdgePrintEvidence {
            extracted_text: "ELECTRIC COMPONENTS 440010147700".to_owned(),
            font_table: "name                       type            emb sub uni object ID\n\
                         AAAAAA+ArialMT             TrueType        yes yes yes      12  0"
                .to_owned(),
        };
        let report =
            validate_edge_pdf_output(&input, &output, &plan, uuid_for_test(), 2, &evidence);
        assert_eq!(report.status, crate::ValidationStatus::Pass);
        assert!(
            report
                .checks
                .iter()
                .all(|check| check.status != crate::ValidationStatus::Fail)
        );
    }

    #[test]
    fn validation_fails_when_the_text_layer_is_missing() {
        let input = probe(
            "html",
            BTreeMap::from([
                ("has_external_resource".to_owned(), json!(false)),
                ("text_characters".to_owned(), json!(120)),
            ]),
        );
        let plan = plan_edge_print_to_pdf(
            &input,
            PathBuf::from("out.pdf"),
            &engine("msedge"),
            &engine("pdfinfo"),
            &engine("pdftoppm"),
            &engine("pdftotext"),
            &engine("pdffonts"),
        )
        .expect("plan");
        let mut output = probe(
            "pdf",
            BTreeMap::from([
                ("width_points".to_owned(), json!(420.0)),
                ("height_points".to_owned(), json!(293.0)),
            ]),
        );
        output.streams.push(pdf_page(BTreeMap::from([
            ("width_points".to_owned(), json!(420.0)),
            ("height_points".to_owned(), json!(293.0)),
        ])));
        let evidence = EdgePrintEvidence {
            extracted_text: String::new(),
            font_table: String::new(),
        };
        let report =
            validate_edge_pdf_output(&input, &output, &plan, uuid_for_test(), 2, &evidence);
        assert_eq!(report.status, crate::ValidationStatus::Fail);
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.code == "EDGE_PDF_TEXT_EXTRACTABLE"
                    && check.status == crate::ValidationStatus::Fail)
        );
    }

    #[test]
    fn pdffonts_table_parses_embedding_columns_from_the_right() {
        let table = "name                       type            encoding         emb sub uni object ID\n\
                     ------------------------------------ ----------------- ---------------- --- --- --- ---------\n\
                     BAAAAA+SimSun              CID TrueType    WinAnsi           yes yes yes      7  0\n\
                     Helvetica                  Type 1          WinAnsi           no  no  no       9  0\n";
        let summary = parse_pdffonts_table(table);
        assert_eq!(
            summary,
            FontEmbeddingSummary {
                font_count: 2,
                embedded_count: 1,
                unembedded: vec!["Helvetica".to_owned()],
            }
        );
        assert_eq!(
            parse_pdffonts_table(
                "name type encoding emb sub uni object ID\n-----------------------------------------\n"
            ),
            FontEmbeddingSummary::default(),
            "a header-only table declares no fonts"
        );
    }

    #[test]
    fn fidelity_ratio_is_bounded() {
        assert!((text_fidelity(0, 0) - 1.0).abs() < f64::EPSILON);
        assert!((text_fidelity(100, 40) - 0.4).abs() < f64::EPSILON);
        assert!((text_fidelity(10, 999) - 1.0).abs() < f64::EPSILON);
    }

    fn uuid_for_test() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }
}
