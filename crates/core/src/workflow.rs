use std::path::Path;

use formatwright_engine_sdk::EngineIdentity;

use crate::doctor::{inspect_builtin_engine, inspect_engine};
use crate::document::{inspect_document, plan_markup_to_docx, plan_markup_to_pdf};
use crate::domain::{Plan, PlanRequest, Probe};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::inspect::inspect_media;
use crate::office::{inspect_office, office_format_hint, plan_office_to_pdf};
use crate::pdf::{inspect_pdf, pdf_format_hint, plan_pdf_render};
use crate::planner::{plan_conversion, plan_heic_conversion};
use crate::structured::{inspect_structured, plan_structured_conversion};

/// Inspects an input, discovers exact engines, and builds the same runnable
/// Plan for every first-party surface.
///
/// # Errors
///
/// Returns typed inspection, engine, policy, or planning errors.
#[allow(clippy::too_many_lines)]
pub async fn prepare_conversion(
    input: &Path,
    request: &PlanRequest,
) -> Result<(Probe, Plan, EngineIdentity)> {
    if is_structured_target(&request.target_format) {
        let probe = inspect_structured(input).await?;
        let engine = inspect_builtin_engine("formatwright.structured").await?;
        let plan = plan_structured_conversion(&probe, request, &engine)?;
        return Ok((probe, plan, engine));
    }
    let target = normalized_target(&request.target_format);
    if target == "docx" {
        let probe = inspect_document(input).await?;
        let pandoc = inspect_engine("pandoc").await?;
        let output = required_output(request, "DOCX conversion")?;
        let plan = plan_markup_to_docx(&probe, output, &pandoc)?;
        return Ok((probe, plan, pandoc));
    }
    if target == "pdf" && office_format_hint(input)?.is_some() {
        let probe = inspect_office(input).await?;
        let soffice = inspect_engine("soffice").await?;
        let pdftoppm = inspect_engine("pdftoppm").await?;
        let pdfinfo = inspect_engine("pdfinfo").await?;
        let output = required_output(request, "Office-to-PDF conversion")?;
        let plan = plan_office_to_pdf(&probe, output, &soffice, &pdfinfo, &pdftoppm)?;
        return Ok((probe, plan, pdfinfo));
    }
    if target == "pdf"
        && let Ok(probe) = inspect_document(input).await
        && matches!(probe.format.id.as_str(), "markdown" | "html")
    {
        let pandoc = inspect_engine("pandoc").await?;
        let soffice = inspect_engine("soffice").await?;
        let pdfinfo = inspect_engine("pdfinfo").await?;
        let pdftoppm = inspect_engine("pdftoppm").await?;
        let output = required_output(request, "Markup-to-PDF conversion")?;
        let plan = plan_markup_to_pdf(&probe, output, &pandoc, &soffice, &pdfinfo, &pdftoppm)?;
        return Ok((probe, plan, pdfinfo));
    }
    if pdf_format_hint(input)? {
        let pdfinfo = inspect_engine("pdfinfo").await?;
        let pdftoppm = inspect_engine("pdftoppm").await?;
        let ffprobe = inspect_engine("ffprobe").await?;
        let probe = inspect_pdf(input, &pdfinfo).await?;
        let plan = plan_pdf_render(&probe, request, &pdftoppm)?;
        return Ok((probe, plan, ffprobe));
    }
    let ffprobe = inspect_engine("ffprobe").await?;
    let probe = inspect_media(input, &ffprobe).await?;
    if probe.format.id == "heic" && matches!(target.as_str(), "jpg" | "jpeg" | "png") {
        let heif_convert = inspect_engine("heif-convert").await?;
        let plan = plan_heic_conversion(&probe, request, &heif_convert)?;
        return Ok((probe, plan, ffprobe));
    }
    let ffmpeg = inspect_engine("ffmpeg").await?;
    let plan = plan_conversion(&probe, request, &ffmpeg)?;
    Ok((probe, plan, ffprobe))
}

fn required_output(request: &PlanRequest, operation: &str) -> Result<std::path::PathBuf> {
    request.output_path.clone().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            format!("{operation} requires an output path"),
            "Choose an output path.",
        )
    })
}

fn normalized_target(target: &str) -> String {
    target.trim().trim_start_matches('.').to_ascii_lowercase()
}

fn is_structured_target(target: &str) -> bool {
    matches!(
        normalized_target(target).as_str(),
        "csv" | "json" | "yaml" | "yml" | "xml"
    )
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::Builder;

    use super::prepare_conversion;
    use crate::PlanRequest;

    #[tokio::test]
    async fn shared_surface_preparation_builds_a_runnable_structured_plan() {
        let mut input = Builder::new()
            .suffix(".json")
            .tempfile()
            .expect("temporary JSON input");
        input
            .write_all(br#"[{"id":1,"name":"alpha"}]"#)
            .expect("write JSON fixture");
        let output = input.path().with_extension("yaml");
        let request = PlanRequest {
            target_format: "yaml".to_owned(),
            output_path: Some(output),
            ..PlanRequest::default()
        };
        let (probe, plan, validation_engine) = prepare_conversion(input.path(), &request)
            .await
            .expect("shared Plan preparation");
        assert_eq!(probe.format.id, "json");
        assert_eq!(plan.target_format, "yaml");
        assert_eq!(plan.steps[0].engine.engine_id, "formatwright.structured");
        assert_eq!(validation_engine.engine_id, "formatwright.structured");
    }
}
