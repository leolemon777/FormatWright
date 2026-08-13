//! Validation-only execution for an existing durable job output.

use std::path::Path;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::doctor::{inspect_builtin_engine, inspect_engine};
use crate::document::{inspect_document, validate_docx_output};
use crate::domain::{Plan, Probe, ValidationCheck, ValidationReport, ValidationStatus};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::inspect::inspect_media;
use crate::office::{inspect_office, validate_office_pdf_output};
use crate::pdf::{inspect_pdf, validate_pdf_render};
use crate::runner::render_office_pdf_for_validation;
use crate::structured::{inspect_structured, validate_structured_output};
use crate::validation::validate_media_output;

/// Re-runs independent validators against existing input and output artifacts.
#[derive(Debug, Default)]
pub struct RevalidationService;

impl RevalidationService {
    /// Revalidates without executing conversion steps or modifying the output.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the original input changed, an artifact is
    /// unavailable, required validator engines are missing, or validation
    /// evidence cannot be collected safely.
    pub async fn revalidate(
        input_path: &Path,
        output_path: &Path,
        plan: &Plan,
        job_id: Uuid,
        cancellation: CancellationToken,
    ) -> Result<ValidationReport> {
        ensure_plan_integrity(plan)?;
        let first_engine = plan
            .steps
            .first()
            .map(|step| step.engine.engine_id.as_str())
            .ok_or_else(|| invalid_revalidation_plan("executable step"))?;
        let mut report = match (first_engine, plan.target_format.as_str()) {
            ("formatwright.structured", _) => {
                let input = inspect_structured(input_path).await?;
                ensure_input_matches(&input, plan)?;
                let output = inspect_structured(output_path).await?;
                let mut report = validate_structured_output(&input, &output, plan, job_id);
                append_engine(
                    &mut report,
                    inspect_builtin_engine("formatwright.structured").await?,
                );
                report
            }
            ("pandoc", "docx") => {
                let input = inspect_document(input_path).await?;
                ensure_input_matches(&input, plan)?;
                let output = inspect_document(output_path).await?;
                let mut report = validate_docx_output(&input, &output, plan, job_id);
                append_engine(
                    &mut report,
                    inspect_builtin_engine("formatwright.document-validator").await?,
                );
                report
            }
            ("soffice" | "pandoc", "pdf") => {
                Self::revalidate_office_pdf(
                    input_path,
                    output_path,
                    plan,
                    job_id,
                    cancellation,
                    first_engine,
                )
                .await?
            }
            ("pdftoppm", "png" | "jpeg") => {
                let pdfinfo = inspect_engine("pdfinfo").await?;
                let ffprobe = inspect_engine("ffprobe").await?;
                let input = inspect_pdf(input_path, &pdfinfo).await?;
                ensure_input_matches(&input, plan)?;
                let mut report =
                    validate_pdf_render(&input, output_path, plan, &ffprobe, job_id).await?;
                append_engine(&mut report, pdfinfo);
                append_engine(&mut report, ffprobe);
                report
            }
            _ => {
                let ffprobe = inspect_engine("ffprobe").await?;
                let input = inspect_media(input_path, &ffprobe).await?;
                ensure_input_matches(&input, plan)?;
                let output = inspect_media(output_path, &ffprobe).await?;
                let mut report = validate_media_output(&input, &output, plan, job_id);
                append_engine(&mut report, ffprobe);
                report
            }
        };
        report.output.display_path = Some(output_path.to_string_lossy().into_owned());
        Ok(report)
    }

    async fn revalidate_office_pdf(
        input_path: &Path,
        output_path: &Path,
        plan: &Plan,
        job_id: Uuid,
        cancellation: CancellationToken,
        first_engine: &str,
    ) -> Result<ValidationReport> {
        let input = if first_engine == "soffice" {
            inspect_office(input_path).await?
        } else {
            inspect_document(input_path).await?
        };
        ensure_input_matches(&input, plan)?;
        let pdfinfo = inspect_engine("pdfinfo").await?;
        let pdftoppm = inspect_engine("pdftoppm").await?;
        let output = inspect_pdf(output_path, &pdfinfo).await?;
        let mut validation_step = plan
            .steps
            .iter()
            .find(|step| step.engine.engine_id == "pdftoppm")
            .cloned()
            .ok_or_else(|| invalid_revalidation_plan("PDF render-validation step"))?;
        validation_step.engine = pdftoppm.clone();
        let workspace = tempfile::tempdir().map_err(|error| revalidation_io_error(&error))?;
        let render_directory = workspace.path().join("render-validation");
        std::fs::create_dir(&render_directory).map_err(|error| revalidation_io_error(&error))?;
        let rendered_page_count = render_office_pdf_for_validation(
            output_path,
            &output,
            &validation_step,
            &render_directory,
            cancellation,
        )
        .await?;
        let mut report = validate_office_pdf_output(
            &input,
            &output,
            plan,
            job_id,
            rendered_page_count,
            "validation-only run; converter diagnostics are not available",
        );
        if first_engine == "pandoc" {
            report.checks.push(ValidationCheck {
                code: "REVALIDATE_INTERMEDIATE_DOCX".to_owned(),
                status: ValidationStatus::Unknown,
                required: false,
                expected: serde_json::json!("original intermediate semantic evidence"),
                observed: serde_json::json!("not recreated during validation-only"),
                evidence: "Validation-only never reruns Pandoc or LibreOffice conversion steps."
                    .to_owned(),
                message: "The committed PDF was rechecked, but the discarded intermediate DOCX cannot be independently recreated without conversion."
                    .to_owned(),
            });
        }
        append_engine(&mut report, pdfinfo);
        append_engine(&mut report, pdftoppm);
        Ok(report)
    }
}

fn ensure_plan_integrity(plan: &Plan) -> Result<()> {
    let computed = crate::planner::deterministic_plan_hash(plan)?;
    if computed == plan.plan_hash {
        return Ok(());
    }
    Err(FormatWrightError::new(
        ErrorCode::InputChanged,
        Stage::Validate,
        "Stored Plan failed its deterministic integrity check",
        "Run an integrity check and restore a consistent application-state backup.",
    )
    .with_diagnostic(format!("stored={}; computed={computed}", plan.plan_hash)))
}

fn ensure_input_matches(input: &Probe, plan: &Plan) -> Result<()> {
    if input.artifact.fast_fingerprint == plan.input_fingerprint {
        return Ok(());
    }
    Err(FormatWrightError::new(
        ErrorCode::InputChanged,
        Stage::Validate,
        "The original input changed after this job was planned",
        "Restore the original input or create and approve a new conversion Plan.",
    )
    .with_diagnostic(format!(
        "planned={}; observed={}",
        plan.input_fingerprint, input.artifact.fast_fingerprint
    )))
}

fn append_engine(report: &mut ValidationReport, engine: formatwright_engine_sdk::EngineIdentity) {
    if !report.engines.iter().any(|existing| {
        existing.engine_id == engine.engine_id && existing.binary_sha256 == engine.binary_sha256
    }) {
        report.engines.push(engine);
    }
}

fn invalid_revalidation_plan(field: &str) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::StorageFailed,
        Stage::Validate,
        format!("Stored Plan is missing its {field}"),
        "Restore a valid job database or create a new conversion.",
    )
}

fn revalidation_io_error(error: &std::io::Error) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::StorageFailed,
        Stage::Validate,
        "Validation-only workspace could not be created",
        "Check temporary storage permissions and available space, then retry.",
    )
    .with_diagnostic(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use super::RevalidationService;
    use crate::{PlanRequest, ValidationStatus, prepare_conversion};

    #[tokio::test]
    async fn structured_revalidation_never_modifies_the_existing_output() {
        let suite = tempdir().expect("suite");
        let input = suite.path().join("input.json");
        let output = suite.path().join("output.yaml");
        fs::write(&input, br#"[{"id":1,"name":"alpha"}]"#).expect("input");
        fs::write(&output, b"- id: 1\n  name: alpha\n").expect("output");
        let request = PlanRequest {
            target_format: "yaml".to_owned(),
            output_path: Some(output.clone()),
            ..PlanRequest::default()
        };
        let (_, plan, _) = prepare_conversion(&input, &request).await.expect("plan");
        let before = fs::read(&output).expect("before");

        let report = RevalidationService::revalidate(
            &input,
            &output,
            &plan,
            Uuid::new_v4(),
            CancellationToken::new(),
        )
        .await
        .expect("revalidate");

        assert_eq!(report.status, ValidationStatus::Pass);
        assert_eq!(fs::read(&output).expect("after"), before);
    }

    #[tokio::test]
    async fn structured_revalidation_rejects_a_changed_original_input() {
        let suite = tempdir().expect("suite");
        let input = suite.path().join("input.json");
        let output = suite.path().join("output.yaml");
        fs::write(&input, br#"[{"id":1}]"#).expect("input");
        fs::write(&output, b"- id: 1\n").expect("output");
        let request = PlanRequest {
            target_format: "yaml".to_owned(),
            output_path: Some(output.clone()),
            ..PlanRequest::default()
        };
        let (_, plan, _) = prepare_conversion(&input, &request).await.expect("plan");
        fs::write(&input, br#"[{"id":2}]"#).expect("change input");

        let error = RevalidationService::revalidate(
            &input,
            &output,
            &plan,
            Uuid::new_v4(),
            CancellationToken::new(),
        )
        .await
        .expect_err("changed input must fail");

        assert_eq!(error.code, crate::ErrorCode::InputChanged);
    }

    #[tokio::test]
    async fn structured_revalidation_rejects_a_tampered_plan() {
        let suite = tempdir().expect("suite");
        let input = suite.path().join("input.json");
        let output = suite.path().join("output.yaml");
        fs::write(&input, br#"[{"id":1}]"#).expect("input");
        fs::write(&output, b"- id: 1\n").expect("output");
        let request = PlanRequest {
            target_format: "yaml".to_owned(),
            output_path: Some(output.clone()),
            ..PlanRequest::default()
        };
        let (_, mut plan, _) = prepare_conversion(&input, &request).await.expect("plan");
        plan.target_format = "json".to_owned();

        let error = RevalidationService::revalidate(
            &input,
            &output,
            &plan,
            Uuid::new_v4(),
            CancellationToken::new(),
        )
        .await
        .expect_err("tampered plan must fail");

        assert_eq!(error.code, crate::ErrorCode::InputChanged);
    }

    #[tokio::test]
    async fn structured_revalidation_reports_a_changed_output_without_modifying_it() {
        let suite = tempdir().expect("suite");
        let input = suite.path().join("input.json");
        let output = suite.path().join("output.yaml");
        fs::write(&input, br#"[{"id":1}]"#).expect("input");
        fs::write(&output, b"- id: 2\n").expect("changed output");
        let request = PlanRequest {
            target_format: "yaml".to_owned(),
            output_path: Some(output.clone()),
            ..PlanRequest::default()
        };
        let (_, plan, _) = prepare_conversion(&input, &request).await.expect("plan");
        let before = fs::read(&output).expect("before");

        let report = RevalidationService::revalidate(
            &input,
            &output,
            &plan,
            Uuid::new_v4(),
            CancellationToken::new(),
        )
        .await
        .expect("report validation failure");

        assert_eq!(report.status, ValidationStatus::Fail);
        assert_eq!(fs::read(&output).expect("after"), before);
    }
}
