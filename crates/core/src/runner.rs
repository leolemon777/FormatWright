use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::sys::signal::{Signal, killpg};
#[cfg(unix)]
use nix::unistd::Pid;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
#[cfg(unix)]
use tokio::time::{Instant, sleep, timeout};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::EngineIdentity;
use crate::document::{inspect_document, validate_docx_output, validate_epub_output};
use crate::domain::{Plan, Probe, ValidationReport, ValidationStatus};
use crate::edge_pdf::{
    EdgePrintEvidence, extract_pdf_text, inspect_pdf_font_table, validate_edge_pdf_output,
};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::fingerprint::{ensure_local_filesystem_path, identify_artifact};
use crate::inspect::inspect_media;
use crate::job_store::resolve_output_identity;
use crate::office::validate_office_pdf_output;
use crate::pdf::inspect_pdf;
use crate::pdf::validate_pdf_render;
use crate::structured::{convert_structured_file, inspect_structured, validate_structured_output};
use crate::validation::validate_media_output;
use formatwright_engine_sdk::Operation;

const MAX_STDERR_BYTES: usize = 64 * 1024;
const EDGE_PRINT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
#[cfg(unix)]
const GRACEFUL_TERMINATION_TIMEOUT: Duration = Duration::from_millis(750);
#[cfg(unix)]
const FORCED_TERMINATION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub struct ExecutionResult {
    pub output_path: PathBuf,
    pub report: ValidationReport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionMilestone {
    EngineFinished,
}

/// Executes one validated Plan through an isolated subprocess and commits only
/// an output that passes required validation.
///
/// # Errors
///
/// Returns a typed error for input changes, engine failures, cancellation,
/// validation failure, path conflicts, or commit failure.
#[allow(clippy::too_many_lines)]
pub async fn execute_plan(
    input: &Probe,
    plan: &Plan,
    ffprobe: &EngineIdentity,
    job_id: Uuid,
    cancellation: CancellationToken,
) -> Result<ExecutionResult> {
    execute_plan_observed(input, plan, ffprobe, job_id, cancellation, |_| Ok(())).await
}

/// Executes a Plan and reports durable phase boundaries before validation.
///
/// The observer is called only after the engine has exited successfully and a
/// staged output exists. A caller can therefore persist `validating` before
/// independent probing begins.
///
/// # Errors
///
/// Returns the same typed failures as [`execute_plan`], plus any observer
/// persistence error.
#[allow(clippy::too_many_lines)]
pub async fn execute_plan_observed<F>(
    input: &Probe,
    plan: &Plan,
    ffprobe: &EngineIdentity,
    job_id: Uuid,
    cancellation: CancellationToken,
    mut observer: F,
) -> Result<ExecutionResult>
where
    F: FnMut(ExecutionMilestone) -> Result<()>,
{
    enforce_network_policy(plan)?;
    let step = plan.steps.first().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::Internal,
            Stage::Execute,
            "Plan has no executable step",
            "Create a new Plan and retry.",
        )
    })?;
    let output_path = resolve_output_path(plan)?;
    if output_path.exists() {
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Commit,
            format!("Output already exists: {}", output_path.display()),
            "Choose another output path or an explicit conflict policy.",
        ));
    }
    ensure_input_unchanged(input, Stage::Execute).await?;

    let partial_path = if step.engine.engine_id == "soffice"
        || step.engine.engine_id == "msedge"
        || (step.engine.engine_id == "pandoc" && plan.target_format == "pdf")
    {
        office_staged_work_path(&output_path, job_id)?
    } else {
        staged_output_path(&output_path, job_id)?
    };
    if partial_path.exists() {
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Execute,
            format!(
                "Unexpected partial path already exists: {}",
                partial_path.display()
            ),
            "Inspect and remove or quarantine the unexpected partial file.",
        ));
    }

    let output_parent = output_path.parent().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "Resolved output path has no parent directory",
            "Choose a complete output path.",
        )
    })?;
    if plan
        .steps
        .first()
        .and_then(|step| step.arguments.get("operation"))
        .is_some()
    {
        return execute_pdf_ops_plan(
            input,
            plan,
            job_id,
            cancellation,
            &output_path,
            &partial_path,
            &mut observer,
        )
        .await;
    }
    if step.engine.engine_id == "formatwright.archive" {
        return execute_archive_plan(
            input,
            plan,
            job_id,
            cancellation,
            &output_path,
            &partial_path,
            &mut observer,
        )
        .await;
    }
    if step.engine.engine_id == "formatwright.structured" {
        return execute_structured_plan(
            input,
            plan,
            job_id,
            cancellation,
            &output_path,
            &partial_path,
            &mut observer,
        )
        .await;
    }
    if step.engine.engine_id == "heif-convert" {
        return execute_heif_plan(
            input,
            plan,
            ffprobe,
            job_id,
            cancellation,
            &output_path,
            &partial_path,
            &mut observer,
        )
        .await;
    }
    if step.engine.engine_id == "pandoc" {
        if plan.target_format == "pdf" {
            return execute_markup_pdf_plan(
                input,
                plan,
                ffprobe,
                job_id,
                cancellation,
                &output_path,
                &partial_path,
                &mut observer,
            )
            .await;
        }
        return execute_pandoc_plan(
            input,
            plan,
            job_id,
            cancellation,
            &output_path,
            &partial_path,
            &mut observer,
        )
        .await;
    }
    if step.engine.engine_id == "soffice" {
        return execute_office_pdf_plan(
            input,
            input,
            plan,
            ffprobe,
            job_id,
            cancellation,
            &output_path,
            &partial_path,
            &mut observer,
        )
        .await;
    }
    if step.engine.engine_id == "msedge" {
        return execute_edge_print_plan(
            input,
            plan,
            ffprobe,
            job_id,
            cancellation,
            &output_path,
            &partial_path,
            &mut observer,
        )
        .await;
    }
    if step.engine.engine_id == "pdftoppm" {
        return execute_pdf_render_plan(
            input,
            plan,
            ffprobe,
            job_id,
            cancellation,
            &output_path,
            &partial_path,
            &mut observer,
        )
        .await;
    }
    let mut command = Command::new(&step.engine.binary_path);
    command
        .current_dir(output_parent)
        .arg("-nostdin")
        .arg("-hide_banner")
        .arg("-v")
        .arg("error")
        .arg("-progress")
        .arg("pipe:1")
        .arg("-nostats")
        .arg("-protocol_whitelist")
        .arg("file,pipe")
        .arg("-i")
        .arg(&input.artifact.canonical_path);
    configure_ffmpeg_output(&mut command, plan, &partial_path)?;
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // A dedicated Unix process group lets cancellation reach descendants as
    // well as the adapter process. PID 0 means the child becomes group leader.
    #[cfg(unix)]
    command.process_group(0);

    tracing::info!(
        job_id = %job_id,
        input = %input.artifact.canonical_path.display(),
        partial = %partial_path.display(),
        "starting conversion engine"
    );
    let mut child = command.spawn().map_err(|error| {
        FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Execute,
            "Unable to start FFmpeg",
            "Run doctor and verify the selected engine.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::Internal,
            Stage::Execute,
            "FFmpeg stdout pipe was not created",
            "Report this internal error.",
        )
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::Internal,
            Stage::Execute,
            "FFmpeg stderr pipe was not created",
            "Report this internal error.",
        )
    })?;
    let progress_task = tokio::spawn(drain_stream(stdout));
    let stderr_task = tokio::spawn(read_bounded_tail(stderr, MAX_STDERR_BYTES));

    let status = tokio::select! {
        status = child.wait() => status.map_err(|error| {
            FormatWrightError::new(
                ErrorCode::ExecutionFailed,
                Stage::Execute,
                "Unable to wait for FFmpeg",
                "Retry the conversion.",
            )
            .with_diagnostic(error.to_string())
        })?,
        () = cancellation.cancelled() => {
            terminate_process_tree(&mut child).await;
            cleanup_partial(&partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::Cancelled,
                Stage::Execute,
                "Conversion was cancelled",
                "Retry when ready.",
            ));
        }
    };
    let _ = progress_task.await;
    let stderr = stderr_task
        .await
        .unwrap_or_else(|error| format!("stderr reader failed: {error}"));

    if !status.success() {
        cleanup_partial(&partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            format!("FFmpeg exited with status {status}"),
            "Inspect the diagnostic, adjust the Plan, or try another engine.",
        )
        .with_diagnostic(stderr));
    }
    if !partial_path.is_file() {
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            "FFmpeg reported success but produced no output",
            "Retry with a different engine or report the input as a bug.",
        ));
    }

    if cancellation.is_cancelled() {
        cleanup_partial(&partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::Cancelled,
            Stage::Execute,
            "Conversion was cancelled before validation",
            "Retry when ready.",
        ));
    }

    if let Err(error) = observer(ExecutionMilestone::EngineFinished) {
        cleanup_partial(&partial_path);
        return Err(error);
    }

    if let Err(error) = ensure_input_unchanged(input, Stage::Commit).await {
        cleanup_partial(&partial_path);
        return Err(error);
    }
    let output_probe = match inspect_media(&partial_path, ffprobe).await {
        Ok(probe) => probe,
        Err(error) => {
            cleanup_partial(&partial_path);
            return Err(error);
        }
    };
    let mut report = validate_media_output(input, &output_probe, plan, job_id);
    if report.status == ValidationStatus::Fail {
        cleanup_partial(&partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "Output failed required validation checks",
            "Inspect the validation report and choose another Plan.",
        )
        .with_diagnostic(
            serde_json::to_string(&report).unwrap_or_else(|_| "report serialization failed".into()),
        ));
    }

    if output_path.exists() {
        cleanup_partial(&partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Commit,
            "The destination appeared while the job was running",
            "Choose another output path or retry with an explicit conflict policy.",
        ));
    }
    if let Err(error) = commit_path_no_replace(&partial_path, &output_path) {
        cleanup_partial(&partial_path);
        return Err(error);
    }
    report.output.display_path = Some(output_path.to_string_lossy().into_owned());

    Ok(ExecutionResult {
        output_path,
        report,
    })
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn execute_office_pdf_plan<F>(
    input: &Probe,
    identity_input: &Probe,
    plan: &Plan,
    pdfinfo: &EngineIdentity,
    job_id: Uuid,
    cancellation: CancellationToken,
    output_path: &Path,
    partial_path: &Path,
    observer: &mut F,
) -> Result<ExecutionResult>
where
    F: FnMut(ExecutionMilestone) -> Result<()>,
{
    let planned_pdfinfo = plan
        .steps
        .iter()
        .find(|value| value.engine.engine_id == "pdfinfo")
        .ok_or_else(|| invalid_plan_argument("Office PDF structural-validation step"))?;
    if pdfinfo.engine_id != "pdfinfo"
        || pdfinfo.version != planned_pdfinfo.engine.version
        || pdfinfo.binary_sha256 != planned_pdfinfo.engine.binary_sha256
    {
        return Err(invalid_plan_argument("pdfinfo validation engine"));
    }
    let step = plan
        .steps
        .first()
        .ok_or_else(|| invalid_plan_argument("Office conversion step"))?;
    let validation_step = plan
        .steps
        .iter()
        .find(|value| value.engine.engine_id == "pdftoppm")
        .ok_or_else(|| invalid_plan_argument("Office PDF render-validation step"))?;
    let source = checked_argument(step, "source_format", &["docx", "pptx", "xlsx"])?;
    checked_argument(step, "target_format", &["pdf"])?;
    checked_argument(step, "headless", &["true"])?;
    checked_argument(step, "isolated_profile", &["true"])?;
    checked_argument(step, "macros", &["disabled"])?;
    checked_argument(step, "external_resources", &["deny"])?;
    checked_argument(validation_step, "dpi", &["72"])?;
    checked_argument(validation_step, "target_format", &["png"])?;
    checked_argument(validation_step, "purpose", &["validation-only"])?;
    std::fs::create_dir(partial_path).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Execute,
            "Unable to create the staged Office workspace",
            "Check destination permissions and storage health.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let conversion_directory = partial_path.join("converted");
    let profile_directory = partial_path.join("profile");
    let render_directory = partial_path.join("render-validation");
    for directory in [&conversion_directory, &profile_directory, &render_directory] {
        if let Err(error) = std::fs::create_dir(directory) {
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::StorageFailed,
                Stage::Execute,
                "Unable to create an isolated Office work directory",
                "Check destination permissions and storage health.",
            )
            .with_diagnostic(error.to_string()));
        }
    }
    let profile_url = match local_file_url(&profile_directory) {
        Ok(url) => url,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(error);
        }
    };
    let filter = match source {
        "docx" => "pdf:writer_pdf_Export",
        "pptx" => "pdf:impress_pdf_Export",
        "xlsx" => "pdf:calc_pdf_Export",
        _ => unreachable!("checked source format"),
    };
    let output_parent = output_path
        .parent()
        .ok_or_else(|| invalid_plan_argument("output parent"))?;
    let office_input_path = external_process_path(&input.artifact.canonical_path);
    let office_output_directory = external_process_path(&conversion_directory);
    let office_current_directory = external_process_path(output_parent);
    let office_engine_path = external_process_path(&step.engine.binary_path);
    let mut command = Command::new(office_engine_path);
    command
        .current_dir(office_current_directory)
        .arg(format!("-env:UserInstallation={profile_url}"))
        .arg("--headless")
        .arg("--nologo")
        .arg("--nodefault")
        .arg("--nolockcheck")
        .arg("--norestore")
        .arg("--convert-to")
        .arg(filter)
        .arg("--outdir")
        .arg(&office_output_directory)
        .arg(&office_input_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    tracing::info!(
        job_id = %job_id,
        input = %input.artifact.canonical_path.display(),
        partial = %partial_path.display(),
        "starting isolated Office PDF renderer"
    );
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::EngineIncompatible,
                Stage::Execute,
                "Unable to start LibreOffice",
                "Run doctor and verify the soffice installation.",
            )
            .with_diagnostic(error.to_string()));
        }
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid_plan_argument("LibreOffice stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| invalid_plan_argument("LibreOffice stderr"))?;
    let stdout_task = tokio::spawn(read_bounded_tail(stdout, MAX_STDERR_BYTES));
    let stderr_task = tokio::spawn(read_bounded_tail(stderr, MAX_STDERR_BYTES));
    let status = tokio::select! {
        status = child.wait() => status.map_err(|error| {
            FormatWrightError::new(
                ErrorCode::ExecutionFailed,
                Stage::Execute,
                "Unable to wait for LibreOffice",
                "Retry the conversion.",
            )
            .with_diagnostic(error.to_string())
        })?,
        () = cancellation.cancelled() => {
            terminate_process_tree(&mut child).await;
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::Cancelled,
                Stage::Execute,
                "Office conversion was cancelled",
                "Retry when ready.",
            ));
        }
    };
    let stdout = stdout_task
        .await
        .unwrap_or_else(|error| format!("stdout reader failed: {error}"));
    let stderr = stderr_task
        .await
        .unwrap_or_else(|error| format!("stderr reader failed: {error}"));
    let diagnostic = format!("{stdout}\n{stderr}");
    if !status.success() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            format!("LibreOffice exited with status {status}"),
            "Inspect the diagnostic and verify the Office document.",
        )
        .with_diagnostic(diagnostic));
    }
    let source_stem = input
        .artifact
        .canonical_path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| invalid_plan_argument("Office input filename"))?;
    let produced_pdf = conversion_directory.join(format!("{source_stem}.pdf"));
    let output_appeared = wait_for_regular_file(
        &produced_pdf,
        &cancellation,
        std::time::Duration::from_secs(30),
    )
    .await;
    if cancellation.is_cancelled() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::Cancelled,
            Stage::Execute,
            "Office conversion was cancelled while waiting for renderer output",
            "Retry when ready.",
        ));
    }
    if !output_appeared {
        let observed_entries = std::fs::read_dir(&conversion_directory)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let detailed_diagnostic = format!(
            "{diagnostic}\ninput={}\noutdir={}\nprofile={profile_url}\nentries={observed_entries:?}",
            office_input_path.display(),
            office_output_directory.display()
        );
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            "LibreOffice reported success but produced no expected PDF",
            "Inspect the diagnostic and retry.",
        )
        .with_diagnostic(detailed_diagnostic));
    }
    if let Err(error) = observer(ExecutionMilestone::EngineFinished) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    if cancellation.is_cancelled() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::Cancelled,
            Stage::Validate,
            "Office conversion was cancelled before validation",
            "Retry when ready.",
        ));
    }
    if let Err(error) = ensure_input_unchanged(identity_input, Stage::Commit).await {
        cleanup_partial(partial_path);
        return Err(error);
    }
    let output_probe = match inspect_pdf(&produced_pdf, pdfinfo).await {
        Ok(probe) => probe,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(error);
        }
    };
    let rendered_page_count = match render_office_pdf_for_validation(
        &produced_pdf,
        &output_probe,
        validation_step,
        &render_directory,
        cancellation,
    )
    .await
    {
        Ok(count) => count,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(error);
        }
    };
    let mut report = validate_office_pdf_output(
        input,
        &output_probe,
        plan,
        job_id,
        rendered_page_count,
        &diagnostic,
    );
    if report.status == ValidationStatus::Fail {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "Office PDF failed required validation",
            "Inspect the validation report and adjust the source.",
        )
        .with_diagnostic(serde_json::to_string(&report).unwrap_or_default()));
    }
    if output_path.exists() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Commit,
            "The PDF destination appeared while LibreOffice was running",
            "Choose another output path.",
        ));
    }
    if let Err(error) = commit_path_no_replace(&produced_pdf, output_path) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    if let Err(error) = std::fs::remove_dir_all(partial_path) {
        tracing::warn!(partial = %partial_path.display(), %error, "committed Office PDF but could not clean workspace");
    }
    report.output.display_path = Some(output_path.to_string_lossy().into_owned());
    Ok(ExecutionResult {
        output_path: output_path.to_owned(),
        report,
    })
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn execute_edge_print_plan<F>(
    input: &Probe,
    plan: &Plan,
    pdfinfo: &EngineIdentity,
    job_id: Uuid,
    cancellation: CancellationToken,
    output_path: &Path,
    partial_path: &Path,
    observer: &mut F,
) -> Result<ExecutionResult>
where
    F: FnMut(ExecutionMilestone) -> Result<()>,
{
    let planned_pdfinfo = plan
        .steps
        .iter()
        .find(|value| value.engine.engine_id == "pdfinfo")
        .ok_or_else(|| invalid_plan_argument("Browser-print structural-validation step"))?;
    if pdfinfo.engine_id != "pdfinfo"
        || pdfinfo.version != planned_pdfinfo.engine.version
        || pdfinfo.binary_sha256 != planned_pdfinfo.engine.binary_sha256
    {
        return Err(invalid_plan_argument("pdfinfo validation engine"));
    }
    let step = plan
        .steps
        .first()
        .ok_or_else(|| invalid_plan_argument("Browser-print step"))?;
    let validation_step = plan
        .steps
        .iter()
        .find(|value| value.engine.engine_id == "pdftoppm")
        .ok_or_else(|| invalid_plan_argument("Browser-print render-validation step"))?;
    let text_step = plan
        .steps
        .iter()
        .find(|value| value.engine.engine_id == "pdftotext")
        .ok_or_else(|| invalid_plan_argument("Browser-print text-validation step"))?;
    let font_step = plan
        .steps
        .iter()
        .find(|value| value.engine.engine_id == "pdffonts")
        .ok_or_else(|| invalid_plan_argument("Browser-print font-validation step"))?;
    checked_argument(step, "source_format", &["html", "svg"])?;
    checked_argument(step, "target_format", &["pdf"])?;
    checked_argument(step, "headless", &["true"])?;
    checked_argument(step, "isolated_profile", &["true"])?;
    checked_argument(step, "network", &["deny"])?;
    checked_argument(step, "external_resources", &["deny"])?;
    checked_argument(validation_step, "dpi", &["72"])?;
    checked_argument(validation_step, "target_format", &["png"])?;
    checked_argument(validation_step, "purpose", &["validation-only"])?;
    checked_argument(text_step, "layout", &["reading-order"])?;
    checked_argument(text_step, "purpose", &["validation-only"])?;
    checked_argument(font_step, "embedded", &["required"])?;
    std::fs::create_dir(partial_path).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Execute,
            "Unable to create the staged browser-print workspace",
            "Check destination permissions and storage health.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let profile_directory = partial_path.join("profile");
    let render_directory = partial_path.join("render-validation");
    for directory in [&profile_directory, &render_directory] {
        if let Err(error) = std::fs::create_dir(directory) {
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::StorageFailed,
                Stage::Execute,
                "Unable to create an isolated browser work directory",
                "Check destination permissions and storage health.",
            )
            .with_diagnostic(error.to_string()));
        }
    }
    let staged_pdf = partial_path.join("print.pdf");
    let input_url = match local_file_url(&input.artifact.canonical_path) {
        Ok(url) => url,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(error);
        }
    };
    let output_parent = output_path
        .parent()
        .ok_or_else(|| invalid_plan_argument("output parent"))?;
    let engine_path = external_process_path(&step.engine.binary_path);
    let print_target = external_process_path(&staged_pdf);
    let profile_target = external_process_path(&profile_directory);
    let mut command = Command::new(engine_path);
    command
        .current_dir(external_process_path(output_parent))
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-extensions")
        .arg("--disable-background-networking")
        .arg("--disable-sync")
        .arg(format!("--user-data-dir={}", profile_target.display()))
        .arg("--host-resolver-rules=MAP * ~NOTFOUND")
        .arg("--print-to-pdf-no-header")
        .arg(format!("--print-to-pdf={}", print_target.display()))
        .arg(&input_url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    tracing::info!(
        job_id = %job_id,
        input = %input.artifact.canonical_path.display(),
        partial = %partial_path.display(),
        "starting isolated browser PDF print"
    );
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::EngineIncompatible,
                Stage::Execute,
                "Unable to start the browser print engine",
                "Run doctor and verify the Edge installation.",
            )
            .with_diagnostic(error.to_string()));
        }
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid_plan_argument("Browser stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| invalid_plan_argument("Browser stderr"))?;
    let stdout_task = tokio::spawn(read_bounded_tail(stdout, MAX_STDERR_BYTES));
    let stderr_task = tokio::spawn(read_bounded_tail(stderr, MAX_STDERR_BYTES));
    let status = tokio::select! {
        status = child.wait() => status.map_err(|error| {
            FormatWrightError::new(
                ErrorCode::ExecutionFailed,
                Stage::Execute,
                "Unable to wait for the browser print engine",
                "Retry the conversion.",
            )
            .with_diagnostic(error.to_string())
        })?,
        () = cancellation.cancelled() => {
            terminate_process_tree(&mut child).await;
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::Cancelled,
                Stage::Execute,
                "Browser print was cancelled",
                "Retry when ready.",
            ));
        }
        () = tokio::time::sleep(EDGE_PRINT_TIMEOUT) => {
            terminate_process_tree(&mut child).await;
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::ExecutionFailed,
                Stage::Execute,
                "The browser print engine timed out",
                "Simplify the document or check the browser installation.",
            ));
        }
    };
    let stdout = stdout_task
        .await
        .unwrap_or_else(|error| format!("stdout reader failed: {error}"));
    let stderr = stderr_task
        .await
        .unwrap_or_else(|error| format!("stderr reader failed: {error}"));
    let diagnostic = format!("{stdout}\n{stderr}");
    if !status.success() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            format!("The browser print engine exited with status {status}"),
            "Inspect the diagnostic and verify the HTML/SVG document.",
        )
        .with_diagnostic(diagnostic));
    }
    let output_appeared = wait_for_regular_file(
        &staged_pdf,
        &cancellation,
        std::time::Duration::from_secs(30),
    )
    .await;
    if cancellation.is_cancelled() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::Cancelled,
            Stage::Execute,
            "Browser print was cancelled while waiting for the printed PDF",
            "Retry when ready.",
        ));
    }
    if !output_appeared {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            "The browser print engine reported success but produced no PDF",
            "Inspect the diagnostic and retry.",
        )
        .with_diagnostic(diagnostic));
    }
    if let Err(error) = observer(ExecutionMilestone::EngineFinished) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    if cancellation.is_cancelled() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::Cancelled,
            Stage::Validate,
            "Browser print was cancelled before validation",
            "Retry when ready.",
        ));
    }
    if let Err(error) = ensure_input_unchanged(input, Stage::Commit).await {
        cleanup_partial(partial_path);
        return Err(error);
    }
    let output_probe = match inspect_pdf(&staged_pdf, pdfinfo).await {
        Ok(probe) => probe,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(error);
        }
    };
    let rendered_page_count = match render_office_pdf_for_validation(
        &staged_pdf,
        &output_probe,
        validation_step,
        &render_directory,
        cancellation.clone(),
    )
    .await
    {
        Ok(count) => count,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(error);
        }
    };
    let evidence = match gather_edge_print_evidence(text_step, font_step, &staged_pdf).await {
        Ok(evidence) => evidence,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(error);
        }
    };
    let mut report = validate_edge_pdf_output(
        input,
        &output_probe,
        plan,
        job_id,
        rendered_page_count,
        &evidence,
    );
    if report.status == ValidationStatus::Fail {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "Browser-printed PDF failed required validation",
            "Inspect the validation report and adjust the source.",
        )
        .with_diagnostic(serde_json::to_string(&report).unwrap_or_default()));
    }
    if output_path.exists() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Commit,
            "The PDF destination appeared while the browser was printing",
            "Choose another output path.",
        ));
    }
    if let Err(error) = commit_path_no_replace(&staged_pdf, output_path) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    if let Err(error) = std::fs::remove_dir_all(partial_path) {
        tracing::warn!(partial = %partial_path.display(), %error, "committed browser-printed PDF but could not clean workspace");
    }
    report.output.display_path = Some(output_path.to_string_lossy().into_owned());
    Ok(ExecutionResult {
        output_path: output_path.to_owned(),
        report,
    })
}

async fn gather_edge_print_evidence(
    text_step: &crate::domain::PlanStep,
    font_step: &crate::domain::PlanStep,
    staged_pdf: &Path,
) -> Result<EdgePrintEvidence> {
    let extracted_text = extract_pdf_text(&text_step.engine, staged_pdf).await?;
    let font_table = inspect_pdf_font_table(&font_step.engine, staged_pdf).await?;
    Ok(EdgePrintEvidence {
        extracted_text,
        font_table,
    })
}

async fn wait_for_regular_file(
    path: &Path,
    cancellation: &CancellationToken,
    maximum: std::time::Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + maximum;
    loop {
        if path.is_file() {
            return true;
        }
        if cancellation.is_cancelled() || tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

pub(crate) async fn render_office_pdf_for_validation(
    pdf: &Path,
    probe: &Probe,
    step: &crate::domain::PlanStep,
    render_directory: &Path,
    cancellation: CancellationToken,
) -> Result<usize> {
    let prefix = render_directory.join("page");
    let mut command = Command::new(&step.engine.binary_path);
    command
        .arg("-r")
        .arg("72")
        .arg("-png")
        .arg(pdf)
        .arg(prefix)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| {
        FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Validate,
            "Unable to start the PDF page-render validator",
            "Run doctor and verify pdftoppm.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid_plan_argument("pdftoppm validation stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| invalid_plan_argument("pdftoppm validation stderr"))?;
    let stdout_task = tokio::spawn(drain_stream(stdout));
    let stderr_task = tokio::spawn(read_bounded_tail(stderr, MAX_STDERR_BYTES));
    let status = tokio::select! {
        status = child.wait() => status.map_err(|error| {
            FormatWrightError::new(
                ErrorCode::ExecutionFailed,
                Stage::Validate,
                "Unable to wait for the PDF page-render validator",
                "Retry validation.",
            )
            .with_diagnostic(error.to_string())
        })?,
        () = cancellation.cancelled() => {
            terminate_process_tree(&mut child).await;
            return Err(FormatWrightError::new(
                ErrorCode::Cancelled,
                Stage::Validate,
                "Office PDF validation was cancelled",
                "Retry when ready.",
            ));
        }
    };
    let _ = stdout_task.await;
    let stderr = stderr_task.await.unwrap_or_default();
    if !status.success() {
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            format!("PDF page-render validator exited with status {status}"),
            "Inspect the generated PDF and validator diagnostic.",
        )
        .with_diagnostic(stderr));
    }
    let directory = render_directory.to_owned();
    let expected = probe.clone();
    tokio::task::spawn_blocking(move || validate_office_render_files(&directory, &expected))
        .await
        .map_err(|error| {
            FormatWrightError::new(
                ErrorCode::Internal,
                Stage::Validate,
                "Office PDF pixel-validation worker failed",
                "Retry or report the generated PDF.",
            )
            .with_diagnostic(error.to_string())
        })?
}

fn validate_office_render_files(directory: &Path, probe: &Probe) -> Result<usize> {
    let mut pages = BTreeMap::new();
    for entry in
        std::fs::read_dir(directory).map_err(|error| pdf_staging_error(directory, &error))?
    {
        let path = entry
            .map_err(|error| pdf_staging_error(directory, &error))?
            .path();
        let page_number = path
            .file_stem()
            .and_then(|value| value.to_str())
            .and_then(|value| value.strip_prefix("page-"))
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or_else(|| invalid_plan_argument("rendered validation-page filename"))?;
        if !path.is_file()
            || !path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("png"))
            || pages.insert(page_number, path).is_some()
        {
            return Err(FormatWrightError::new(
                ErrorCode::ValidationFailed,
                Stage::Validate,
                "PDF render validation produced unexpected page entries",
                "Inspect the generated PDF and retry.",
            ));
        }
    }
    if pages.len() != probe.streams.len() {
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "Not every Office PDF page rendered",
            "Inspect the generated PDF and retry.",
        ));
    }
    for (index, stream) in probe.streams.iter().enumerate() {
        let page_number = u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1);
        let path = pages
            .get(&page_number)
            .ok_or_else(|| invalid_plan_argument("rendered validation-page sequence"))?;
        let mut reader = image::ImageReader::open(path)
            .and_then(image::ImageReader::with_guessed_format)
            .map_err(|error| office_pixel_error(path, &error))?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(16_384);
        limits.max_image_height = Some(16_384);
        limits.max_alloc = Some(512 * 1024 * 1024);
        reader.limits(limits);
        let image = reader
            .decode()
            .map_err(|error| office_pixel_error(path, &error))?;
        let mut width = stream
            .properties
            .get("width_points")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| invalid_plan_argument("PDF page width"))?;
        let mut height = stream
            .properties
            .get("height_points")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| invalid_plan_argument("PDF page height"))?;
        let rotation = stream
            .properties
            .get("rotation_degrees")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
            .rem_euclid(360);
        if matches!(rotation, 90 | 270) {
            std::mem::swap(&mut width, &mut height);
        }
        if (f64::from(image.width()) - width.round()).abs() > 1.0
            || (f64::from(image.height()) - height.round()).abs() > 1.0
        {
            return Err(FormatWrightError::new(
                ErrorCode::ValidationFailed,
                Stage::Validate,
                "Rendered Office PDF page dimensions do not match the PDF page box",
                "Inspect the generated PDF and renderer configuration.",
            ));
        }
    }
    Ok(pages.len())
}

fn office_pixel_error(path: &Path, error: &impl std::fmt::Display) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::ValidationFailed,
        Stage::Validate,
        format!(
            "Unable to decode rendered Office PDF page: {}",
            path.display()
        ),
        "Inspect the generated PDF and retry.",
    )
    .with_diagnostic(error.to_string())
}

fn local_file_url(path: &Path) -> Result<String> {
    let canonical = path.canonicalize().map_err(|error| {
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Execute,
            "Unable to resolve the isolated LibreOffice profile directory",
            "Check destination storage and retry.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let rendered = canonical.to_string_lossy();
    let without_verbatim = rendered.strip_prefix(r"\\?\").unwrap_or(&rendered);
    let normalized = without_verbatim.replace('\\', "/");
    let mut encoded = String::with_capacity(normalized.len());
    for byte in normalized.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            write!(&mut encoded, "%{byte:02X}").map_err(|error| {
                FormatWrightError::new(
                    ErrorCode::Internal,
                    Stage::Execute,
                    "Unable to encode the LibreOffice profile URL",
                    "Report this internal error.",
                )
                .with_diagnostic(error.to_string())
            })?;
        }
    }
    Ok(format!("file:///{encoded}"))
}

fn external_process_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let rendered = path.to_string_lossy();
        if let Some(rest) = rendered.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = rendered.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_owned()
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn execute_pdf_render_plan<F>(
    input: &Probe,
    plan: &Plan,
    ffprobe: &EngineIdentity,
    job_id: Uuid,
    cancellation: CancellationToken,
    output_path: &Path,
    partial_path: &Path,
    observer: &mut F,
) -> Result<ExecutionResult>
where
    F: FnMut(ExecutionMilestone) -> Result<()>,
{
    let step = plan
        .steps
        .first()
        .ok_or_else(|| invalid_plan_argument("PDF render step"))?;
    let target = checked_argument(step, "target_format", &["png", "jpeg"])?;
    let dpi = checked_u32_argument(step, "dpi", 36, 600)?;
    let color_mode = checked_argument(step, "color_mode", &["rgb", "gray"])?;
    let page_count = checked_u32_argument(step, "page_count", 1, 10_000)?;
    checked_argument(step, "page_prefix", &["page"])?;
    let extension = if target == "jpeg" { "jpg" } else { "png" };
    std::fs::create_dir(partial_path).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Execute,
            "Unable to create the staged PDF page directory",
            "Check destination permissions and storage health.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let output_parent = output_path
        .parent()
        .ok_or_else(|| invalid_plan_argument("output parent"))?;
    let page_prefix = partial_path.join("page");
    let mut command = Command::new(&step.engine.binary_path);
    command
        .current_dir(output_parent)
        .arg("-r")
        .arg(dpi.to_string());
    if color_mode == "gray" {
        command.arg("-gray");
    }
    if target == "png" {
        checked_argument(step, "jpeg_quality", &["not-applicable"])?;
        command.arg("-png");
    } else {
        let quality = checked_u32_argument(step, "jpeg_quality", 1, 100)?;
        command
            .arg("-jpeg")
            .arg("-jpegopt")
            .arg(format!("quality={quality},progressive=n,optimize=y"));
    }
    command
        .arg(&input.artifact.canonical_path)
        .arg(&page_prefix)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    tracing::info!(
        job_id = %job_id,
        input = %input.artifact.canonical_path.display(),
        partial = %partial_path.display(),
        page_count,
        dpi,
        "starting PDF page renderer"
    );
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::EngineIncompatible,
                Stage::Execute,
                "Unable to start pdftoppm",
                "Run doctor and verify the pdftoppm installation.",
            )
            .with_diagnostic(error.to_string()));
        }
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid_plan_argument("pdftoppm stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| invalid_plan_argument("pdftoppm stderr"))?;
    let stdout_task = tokio::spawn(drain_stream(stdout));
    let stderr_task = tokio::spawn(read_bounded_tail(stderr, MAX_STDERR_BYTES));
    let status = tokio::select! {
        status = child.wait() => status.map_err(|error| {
            FormatWrightError::new(
                ErrorCode::ExecutionFailed,
                Stage::Execute,
                "Unable to wait for pdftoppm",
                "Retry the render.",
            )
            .with_diagnostic(error.to_string())
        })?,
        () = cancellation.cancelled() => {
            terminate_process_tree(&mut child).await;
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::Cancelled,
                Stage::Execute,
                "PDF rendering was cancelled",
                "Retry when ready.",
            ));
        }
    };
    let _ = stdout_task.await;
    let stderr = stderr_task
        .await
        .unwrap_or_else(|error| format!("stderr reader failed: {error}"));
    if !status.success() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            format!("pdftoppm exited with status {status}"),
            "Inspect the diagnostic and verify the PDF.",
        )
        .with_diagnostic(stderr));
    }
    if let Err(error) = normalize_poppler_pages(partial_path, page_count, extension) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    if cancellation.is_cancelled() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::Cancelled,
            Stage::Execute,
            "PDF rendering was cancelled before validation",
            "Retry when ready.",
        ));
    }
    if let Err(error) = observer(ExecutionMilestone::EngineFinished) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    if let Err(error) = ensure_input_unchanged(input, Stage::Commit).await {
        cleanup_partial(partial_path);
        return Err(error);
    }
    let mut report = match validate_pdf_render(input, partial_path, plan, ffprobe, job_id).await {
        Ok(report) => report,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(error);
        }
    };
    if report.status == ValidationStatus::Fail {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "Rendered PDF pages failed required validation",
            "Inspect the validation report and adjust the Plan.",
        )
        .with_diagnostic(serde_json::to_string(&report).unwrap_or_default()));
    }
    if output_path.exists() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Commit,
            "The page-directory destination appeared while rendering",
            "Choose another output directory.",
        ));
    }
    if let Err(error) = commit_path_no_replace(partial_path, output_path) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    report.output.display_path = Some(output_path.to_string_lossy().into_owned());
    Ok(ExecutionResult {
        output_path: output_path.to_owned(),
        report,
    })
}

fn normalize_poppler_pages(directory: &Path, page_count: u32, extension: &str) -> Result<()> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| pdf_staging_error(directory, &error))?
        .map(|entry| entry.map(|value| value.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|error| pdf_staging_error(directory, &error))?;
    let mut pages = BTreeMap::new();
    for path in entries {
        if !path.is_file()
            || !path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        {
            return Err(FormatWrightError::new(
                ErrorCode::ValidationFailed,
                Stage::Validate,
                "pdftoppm produced an unexpected page-directory entry",
                "Inspect the engine output and retry.",
            ));
        }
        let page = path
            .file_stem()
            .and_then(|value| value.to_str())
            .and_then(|value| value.strip_prefix("page-"))
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| (1..=page_count).contains(value))
            .ok_or_else(|| {
                FormatWrightError::new(
                    ErrorCode::ValidationFailed,
                    Stage::Validate,
                    "pdftoppm produced an unexpected page filename",
                    "Inspect the engine output and retry.",
                )
            })?;
        if pages.insert(page, path).is_some() {
            return Err(FormatWrightError::new(
                ErrorCode::ValidationFailed,
                Stage::Validate,
                "pdftoppm produced duplicate page numbers",
                "Inspect the engine output and retry.",
            ));
        }
    }
    if pages.len() != page_count as usize {
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "pdftoppm did not render every PDF page",
            "Inspect the PDF and engine diagnostic, then retry.",
        ));
    }
    for (page, source) in &pages {
        let temporary = directory.join(format!(".formatwright-rename-{page:06}.{extension}"));
        std::fs::rename(source, &temporary)
            .map_err(|error| pdf_staging_error(directory, &error))?;
    }
    for page in 1..=page_count {
        let temporary = directory.join(format!(".formatwright-rename-{page:06}.{extension}"));
        let final_path = directory.join(format!("page-{page:06}.{extension}"));
        std::fs::rename(temporary, final_path)
            .map_err(|error| pdf_staging_error(directory, &error))?;
    }
    Ok(())
}

fn pdf_staging_error(path: &Path, error: &std::io::Error) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::StorageFailed,
        Stage::Execute,
        format!("Unable to normalize rendered pages in {}", path.display()),
        "Check destination storage and retry.",
    )
    .with_diagnostic(error.to_string())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn execute_heif_plan<F>(
    input: &Probe,
    plan: &Plan,
    ffprobe: &EngineIdentity,
    job_id: Uuid,
    cancellation: CancellationToken,
    output_path: &Path,
    partial_path: &Path,
    observer: &mut F,
) -> Result<ExecutionResult>
where
    F: FnMut(ExecutionMilestone) -> Result<()>,
{
    if ffprobe.engine_id != "ffprobe" {
        return Err(invalid_plan_argument("HEIC validation ffprobe"));
    }
    let step = plan
        .steps
        .first()
        .filter(|step| step.engine.engine_id == "heif-convert")
        .ok_or_else(|| invalid_plan_argument("HEIC libheif step"))?;
    checked_argument(step, "source_format", &["heic"])?;
    let target = checked_argument(step, "target_format", &["jpeg", "png"])?;
    checked_argument(step, "orientation", &["apply-heif-transformations"])?;
    checked_argument(step, "metadata", &["drop"])?;
    checked_argument(step, "image_selection", &["single-primary"])?;
    std::fs::create_dir(partial_path).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Execute,
            "Unable to create the staged HEIC workspace",
            "Check destination permissions and storage health.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let staged_output = partial_path.join(if target == "jpeg" {
        "output.jpg"
    } else {
        "output.png"
    });
    let mut command = Command::new(&step.engine.binary_path);
    command.current_dir(partial_path).arg("--quiet");
    if target == "jpeg" {
        let quality = checked_u32_argument(step, "quality", 1, 100)?;
        command.arg("--quality").arg(quality.to_string());
    } else {
        checked_argument(step, "quality", &["lossless"])?;
        command.arg("--png-compression-level").arg("6");
    }
    command
        .arg(&input.artifact.canonical_path)
        .arg(&staged_output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::EngineIncompatible,
                Stage::Execute,
                "Unable to start heif-convert",
                "Run doctor and verify the libheif engine.",
            )
            .with_diagnostic(error.to_string()));
        }
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid_plan_argument("heif-convert stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| invalid_plan_argument("heif-convert stderr"))?;
    let stdout_task = tokio::spawn(read_bounded_tail(stdout, MAX_STDERR_BYTES));
    let stderr_task = tokio::spawn(read_bounded_tail(stderr, MAX_STDERR_BYTES));
    let status = tokio::select! {
        status = child.wait() => status.map_err(|error| {
            FormatWrightError::new(
                ErrorCode::ExecutionFailed,
                Stage::Execute,
                "Unable to wait for heif-convert",
                "Retry the conversion.",
            )
            .with_diagnostic(error.to_string())
        })?,
        () = cancellation.cancelled() => {
            terminate_process_tree(&mut child).await;
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::Cancelled,
                Stage::Execute,
                "HEIC conversion was cancelled",
                "Retry when ready.",
            ));
        }
    };
    let stdout = stdout_task
        .await
        .unwrap_or_else(|error| format!("stdout reader failed: {error}"));
    let stderr = stderr_task
        .await
        .unwrap_or_else(|error| format!("stderr reader failed: {error}"));
    if !status.success() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            format!("heif-convert exited with status {status}"),
            "Inspect the HEIC input and decoder availability.",
        )
        .with_diagnostic(format!("{stdout}\n{stderr}")));
    }
    let staged_files = std::fs::read_dir(partial_path)
        .map_err(|error| pdf_staging_error(partial_path, &error))?
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().is_file())
        .collect::<Vec<_>>();
    if staged_files.len() != 1 || !staged_output.is_file() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Execute,
            "HEIC input did not produce exactly one primary image",
            "Choose a single-image HEIC/HEIF file.",
        ));
    }
    if let Err(error) = observer(ExecutionMilestone::EngineFinished) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    if let Err(error) = ensure_input_unchanged(input, Stage::Commit).await {
        cleanup_partial(partial_path);
        return Err(error);
    }
    let output_probe = match inspect_media(&staged_output, ffprobe).await {
        Ok(probe) => probe,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(error);
        }
    };
    let mut report = validate_media_output(input, &output_probe, plan, job_id);
    if report.status == ValidationStatus::Fail {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "HEIC output failed required validation",
            "Inspect the validation report and choose another Plan.",
        )
        .with_diagnostic(serde_json::to_string(&report).unwrap_or_default()));
    }
    if output_path.exists() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Commit,
            "The HEIC destination appeared while conversion was running",
            "Choose another output path.",
        ));
    }
    if let Err(error) = commit_path_no_replace(&staged_output, output_path) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    if let Err(error) = std::fs::remove_dir(partial_path) {
        tracing::warn!(partial = %partial_path.display(), %error, "committed HEIC output but could not remove empty workspace");
    }
    report.output.display_path = Some(output_path.to_string_lossy().into_owned());
    Ok(ExecutionResult {
        output_path: output_path.to_owned(),
        report,
    })
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn execute_markup_pdf_plan<F>(
    input: &Probe,
    plan: &Plan,
    pdfinfo: &EngineIdentity,
    job_id: Uuid,
    cancellation: CancellationToken,
    output_path: &Path,
    partial_path: &Path,
    observer: &mut F,
) -> Result<ExecutionResult>
where
    F: FnMut(ExecutionMilestone) -> Result<()>,
{
    let pandoc_step = plan
        .steps
        .first()
        .filter(|step| step.engine.engine_id == "pandoc")
        .ok_or_else(|| invalid_plan_argument("markup PDF Pandoc step"))?;
    let source = checked_argument(pandoc_step, "source_format", &["markdown", "html", "plain"])?;
    checked_argument(pandoc_step, "target_format", &["docx"])?;
    checked_argument(pandoc_step, "sandbox", &["true"])?;
    checked_argument(pandoc_step, "resource_policy", &["deny-all"])?;
    checked_argument(pandoc_step, "purpose", &["intermediate"])?;
    if plan
        .steps
        .get(1)
        .is_none_or(|step| step.engine.engine_id != "soffice")
    {
        return Err(invalid_plan_argument("markup PDF LibreOffice step"));
    }
    std::fs::create_dir(partial_path).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Execute,
            "Unable to create the staged markup PDF workspace",
            "Check destination permissions and storage health.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let intermediate_path = partial_path.join("intermediate.docx");
    // Plain text has no dedicated Pandoc reader; every plain document is
    // valid (subset) Markdown, so it rides the GFM reader unchanged.
    let reader = match source {
        "html" => "html",
        _ => "gfm",
    };
    let mut command = Command::new(&pandoc_step.engine.binary_path);
    command
        .current_dir(partial_path)
        .arg("--sandbox=true")
        .arg(format!("--from={reader}"))
        .arg("--to=docx")
        .arg("--standalone")
        .arg("--output")
        .arg(&intermediate_path)
        .arg("--")
        .arg(&input.artifact.canonical_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::EngineIncompatible,
                Stage::Execute,
                "Unable to start Pandoc for markup PDF conversion",
                "Run doctor and verify the Pandoc engine.",
            )
            .with_diagnostic(error.to_string()));
        }
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid_plan_argument("Pandoc stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| invalid_plan_argument("Pandoc stderr"))?;
    let stdout_task = tokio::spawn(drain_stream(stdout));
    let stderr_task = tokio::spawn(read_bounded_tail(stderr, MAX_STDERR_BYTES));
    let status = tokio::select! {
        status = child.wait() => status.map_err(|error| {
            FormatWrightError::new(
                ErrorCode::ExecutionFailed,
                Stage::Execute,
                "Unable to wait for Pandoc",
                "Retry the conversion.",
            )
            .with_diagnostic(error.to_string())
        })?,
        () = cancellation.cancelled() => {
            terminate_process_tree(&mut child).await;
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::Cancelled,
                Stage::Execute,
                "Markup PDF conversion was cancelled during Pandoc execution",
                "Retry when ready.",
            ));
        }
    };
    let _ = stdout_task.await;
    let stderr = stderr_task
        .await
        .unwrap_or_else(|error| format!("stderr reader failed: {error}"));
    if !status.success() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            format!("Pandoc exited with status {status}"),
            "Correct the markup or inspect the diagnostic.",
        )
        .with_diagnostic(stderr));
    }
    if !intermediate_path.is_file() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            "Pandoc produced no intermediate DOCX",
            "Retry or report the input.",
        ));
    }
    if cancellation.is_cancelled() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::Cancelled,
            Stage::Execute,
            "Markup PDF conversion was cancelled before DOCX validation",
            "Retry when ready.",
        ));
    }
    let intermediate_probe = match inspect_document(&intermediate_path).await {
        Ok(probe) => probe,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(error);
        }
    };
    let intermediate_report = validate_docx_output(input, &intermediate_probe, plan, job_id);
    if intermediate_report.status == ValidationStatus::Fail {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "Intermediate DOCX failed semantic validation",
            "Inspect the markup and conversion report.",
        )
        .with_diagnostic(serde_json::to_string(&intermediate_report).unwrap_or_default()));
    }
    let mut office_plan = plan.clone();
    office_plan.steps = plan.steps.iter().skip(1).cloned().collect();
    office_plan.input_fingerprint = intermediate_probe.artifact.fast_fingerprint.clone();
    let office_workspace = partial_path.join("o");
    let mut result = match execute_office_pdf_plan(
        &intermediate_probe,
        input,
        &office_plan,
        pdfinfo,
        job_id,
        cancellation,
        output_path,
        &office_workspace,
        observer,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(error);
        }
    };
    result.report.input = intermediate_report.input;
    result.report.engines = plan.steps.iter().map(|step| step.engine.clone()).collect();
    result.report.intentional_changes = plan.changes.changed.clone();
    result
        .report
        .checks
        .splice(0..0, intermediate_report.checks);
    if let Err(error) = std::fs::remove_dir_all(partial_path) {
        tracing::warn!(partial = %partial_path.display(), %error, "committed markup PDF but could not clean workspace");
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
/// Builds the qpdf command line for one ADR-0013 PDF operation step. The
/// input path goes through `external_process_path` because qpdf rejects
/// Windows verbatim (`\\?\`) paths.
fn pdf_ops_command(
    step: &crate::domain::PlanStep,
    input: &Probe,
    operation: &str,
    plan_id: Uuid,
) -> Result<Command> {
    let mut command = Command::new(&step.engine.binary_path);
    match operation {
        "pdf-merge" => {
            command.arg("--empty").arg("--pages");
            let inputs = step
                .arguments
                .get("inputs")
                .ok_or_else(|| invalid_plan_argument("inputs"))?
                .split(';')
                .map(PathBuf::from)
                .collect::<Vec<_>>();
            if inputs.len() < 2 {
                return Err(invalid_plan_argument("inputs"));
            }
            for input_path in &inputs {
                command.arg(external_process_path(input_path)).arg("1-z");
            }
        }
        "pdf-extract" => {
            let range = step
                .arguments
                .get("page_range")
                .ok_or_else(|| invalid_plan_argument("page_range"))?;
            command
                .arg("--empty")
                .arg("--pages")
                .arg(external_process_path(&input.artifact.canonical_path))
                .arg(range);
        }
        "pdf-rotate" => {
            let angle = checked_argument(step, "angle", &["90", "180", "270"])?;
            let pages = step
                .arguments
                .get("pages")
                .map(String::as_str)
                .unwrap_or_default();
            let rotation = if pages.is_empty() {
                format!("--rotate=+{angle}")
            } else {
                format!("--rotate=+{angle}:{pages}")
            };
            command
                .arg(external_process_path(&input.artifact.canonical_path))
                .arg(rotation);
        }
        "pdf-compress" => {
            command
                .arg(external_process_path(&input.artifact.canonical_path))
                .arg("--compress-streams=y")
                .arg("--object-streams=generate")
                .arg("--recompress-flate")
                .arg("--compression-level=9");
        }
        "pdf-encrypt" | "pdf-decrypt" => {
            // The Plan stores a `[redacted]` placeholder; the cleartext
            // password travels through the execution-only in-process store
            // keyed by plan_id (see pdf::take_pdf_secret), so a serialized
            // Plan never carries the secret.
            let password = crate::pdf::take_pdf_secret(plan_id)
                .ok_or_else(|| invalid_plan_argument("password (unavailable after a restart)"))?;
            if operation == "pdf-encrypt" {
                command
                    .arg(external_process_path(&input.artifact.canonical_path))
                    .arg("--encrypt")
                    .arg(&password)
                    .arg(&password)
                    .arg("256")
                    .arg("--print=full")
                    .arg("--modify=none");
            } else {
                command
                    .arg(format!("--password={password}"))
                    .arg("--decrypt")
                    .arg(external_process_path(&input.artifact.canonical_path));
            }
        }
        _ => return Err(invalid_plan_argument("operation")),
    }
    Ok(command)
}

/// Builds the acceptance report for a PDF operation output. Encrypted outputs
/// cannot be page-probed, so pdfinfo's refusal to open them is the evidence.
async fn validate_pdf_operation_output(
    input: &Probe,
    partial_path: &Path,
    plan: &Plan,
    job_id: Uuid,
    operation: &str,
    expected_pages: u32,
    pdfinfo: &formatwright_engine_sdk::EngineIdentity,
) -> Result<crate::domain::ValidationReport> {
    if operation == "pdf-encrypt" {
        // Page-count probing is impossible on an encrypted output: pdfinfo
        // cannot open it without the password. That very failure is the
        // acceptance evidence for encryption (see validate_pdf_encrypt_output).
        let encrypted = crate::pdf::inspect_pdf(partial_path, pdfinfo)
            .await
            .is_err();
        let output_identity = identify_artifact(partial_path).await?;
        return Ok(crate::pdf::validate_pdf_encrypt_output(
            input,
            &output_identity,
            plan,
            job_id,
            encrypted,
        ));
    }
    let output_probe = crate::pdf::inspect_pdf(partial_path, pdfinfo).await?;
    Ok(crate::pdf::validate_pdf_ops_output(
        input,
        &output_probe,
        plan,
        job_id,
        expected_pages,
    ))
}

/// Waits for the qpdf child, enforcing cancellation, stderr capture, and a
/// successful exit status.
async fn await_pdf_ops_child(
    child: &mut tokio::process::Child,
    cancellation: &CancellationToken,
    partial_path: &Path,
) -> Result<()> {
    let stderr_task = tokio::spawn(read_bounded_tail(
        child
            .stderr
            .take()
            .ok_or_else(|| invalid_plan_argument("qpdf stderr"))?,
        MAX_STDERR_BYTES,
    ));
    let stdout_task = tokio::spawn(drain_stream(
        child
            .stdout
            .take()
            .ok_or_else(|| invalid_plan_argument("qpdf stdout"))?,
    ));
    let status = tokio::select! {
        status = child.wait() => status.map_err(|error| {
            FormatWrightError::new(
                ErrorCode::ExecutionFailed,
                Stage::Execute,
                "Unable to wait for qpdf",
                "Retry the operation.",
            )
            .with_diagnostic(error.to_string())
        })?,
        () = cancellation.cancelled() => {
            terminate_process_tree(child).await;
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::Cancelled,
                Stage::Execute,
                "PDF operation was cancelled",
                "Retry when ready.",
            ));
        }
    };
    let _ = stdout_task.await;
    let stderr = stderr_task
        .await
        .unwrap_or_else(|error| format!("stderr reader failed: {error}"));
    if !status.success() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            format!("qpdf exited with status {status}"),
            "Inspect the diagnostic and adjust the inputs.",
        )
        .with_diagnostic(stderr));
    }
    Ok(())
}

async fn execute_pdf_ops_plan<F>(
    input: &Probe,
    plan: &Plan,
    job_id: Uuid,
    cancellation: CancellationToken,
    output_path: &Path,
    partial_path: &Path,
    observer: &mut F,
) -> Result<ExecutionResult>
where
    F: FnMut(ExecutionMilestone) -> Result<()>,
{
    let step = plan
        .steps
        .first()
        .ok_or_else(|| invalid_plan_argument("PDF operation step"))?;
    let operation = checked_argument(
        step,
        "operation",
        &[
            "pdf-merge",
            "pdf-extract",
            "pdf-rotate",
            "pdf-compress",
            "pdf-encrypt",
            "pdf-decrypt",
        ],
    )?;
    let expected_pages: u32 = step
        .arguments
        .get("expected_pages")
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| invalid_plan_argument("expected_pages"))?;
    let mut command = pdf_ops_command(step, input, operation, plan.plan_id)?;
    command.arg("--").arg(partial_path);
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| {
        FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Execute,
            "Unable to start qpdf",
            "Run doctor and verify the qpdf engine.",
        )
        .with_diagnostic(error.to_string())
    })?;
    await_pdf_ops_child(&mut child, &cancellation, partial_path).await?;
    if !partial_path.is_file() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            "qpdf produced no PDF output",
            "Retry or report the inputs.",
        ));
    }
    if let Err(error) = observer(ExecutionMilestone::EngineFinished) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    if let Err(error) = ensure_input_unchanged(input, Stage::Commit).await {
        cleanup_partial(partial_path);
        return Err(error);
    }
    let pdfinfo = crate::doctor::inspect_engine("pdfinfo").await?;
    let report = match validate_pdf_operation_output(
        input,
        partial_path,
        plan,
        job_id,
        operation,
        expected_pages,
        &pdfinfo,
    )
    .await
    {
        Ok(report) => report,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(error);
        }
    };
    if report.status == ValidationStatus::Fail {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "PDF operation output failed validation",
            "Inspect the validation report and adjust the inputs.",
        )
        .with_diagnostic(serde_json::to_string(&report).unwrap_or_default()));
    }
    commit_pdf_ops_result(partial_path, output_path, report)
}

/// Commits a validated PDF operation output without replacing an existing
/// destination (ADR-0013 commit gate).
fn commit_pdf_ops_result(
    partial_path: &Path,
    output_path: &Path,
    mut report: crate::domain::ValidationReport,
) -> Result<ExecutionResult> {
    if output_path.exists() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Commit,
            "The PDF destination appeared while running",
            "Choose another output path.",
        ));
    }
    if let Err(error) = commit_path_no_replace(partial_path, output_path) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    report.output.display_path = Some(output_path.to_string_lossy().into_owned());
    Ok(ExecutionResult {
        output_path: output_path.to_owned(),
        report,
    })
}

#[allow(clippy::too_many_arguments)]
async fn execute_archive_plan<F>(
    input: &Probe,
    plan: &Plan,
    job_id: Uuid,
    cancellation: CancellationToken,
    output_path: &Path,
    partial_path: &Path,
    observer: &mut F,
) -> Result<ExecutionResult>
where
    F: FnMut(ExecutionMilestone) -> Result<()>,
{
    let step = plan
        .steps
        .first()
        .ok_or_else(|| invalid_plan_argument("Archive step"))?;
    let source = checked_argument(step, "source_format", &["zip", "tar.gz"])?;
    let target = checked_argument(step, "target_format", &["zip", "tar.gz"])?;
    if source == target {
        return Err(invalid_plan_argument("target_format"));
    }
    let input_path = input.artifact.canonical_path.clone();
    let partial = partial_path.to_path_buf();
    let repack_source = source.to_owned();
    let repack_target = target.to_owned();
    tokio::task::spawn_blocking(move || {
        if (repack_source.as_str(), repack_target.as_str()) == ("zip", "tar.gz") {
            crate::archive::repack_zip_to_targz(&input_path, &partial)
        } else {
            crate::archive::repack_targz_to_zip(&input_path, &partial)
        }
    })
    .await
    .map_err(|error| {
        FormatWrightError::new(
            ErrorCode::Internal,
            Stage::Execute,
            "Archive repack worker failed",
            "Retry the conversion.",
        )
        .with_diagnostic(error.to_string())
    })??;
    if !partial_path.is_file() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            "Archive repack produced no output",
            "Retry or report the input.",
        ));
    }
    if cancellation.is_cancelled() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::Cancelled,
            Stage::Execute,
            "Archive conversion was cancelled",
            "Retry when ready.",
        ));
    }
    if let Err(error) = observer(ExecutionMilestone::EngineFinished) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    if let Err(error) = ensure_input_unchanged(input, Stage::Commit).await {
        cleanup_partial(partial_path);
        return Err(error);
    }
    let output_probe = match crate::archive::inspect_archive(partial_path).await {
        Ok(probe) => probe,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(error);
        }
    };
    let mut report = crate::archive::validate_archive_output(input, &output_probe, plan, job_id);
    if report.status == ValidationStatus::Fail {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "Archive output failed validation",
            "Inspect the validation report and adjust the source.",
        )
        .with_diagnostic(serde_json::to_string(&report).unwrap_or_default()));
    }
    if output_path.exists() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Commit,
            "The archive destination appeared while running",
            "Choose another output path.",
        ));
    }
    if let Err(error) = commit_path_no_replace(partial_path, output_path) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    report.output.display_path = Some(output_path.to_string_lossy().into_owned());
    Ok(ExecutionResult {
        output_path: output_path.to_owned(),
        report,
    })
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn execute_pandoc_plan<F>(
    input: &Probe,
    plan: &Plan,
    job_id: Uuid,
    cancellation: CancellationToken,
    output_path: &Path,
    partial_path: &Path,
    observer: &mut F,
) -> Result<ExecutionResult>
where
    F: FnMut(ExecutionMilestone) -> Result<()>,
{
    let step = plan
        .steps
        .first()
        .ok_or_else(|| invalid_plan_argument("Pandoc step"))?;
    let source = checked_argument(step, "source_format", &["markdown", "html", "plain"])?;
    let target = checked_argument(step, "target_format", &["docx", "epub"])?;
    checked_argument(step, "sandbox", &["true"])?;
    checked_argument(step, "resource_policy", &["deny-all"])?;
    // Plain text has no dedicated Pandoc reader; it rides the GFM reader.
    let reader = if source == "html" { "html" } else { "gfm" };
    let output_parent = output_path
        .parent()
        .ok_or_else(|| invalid_plan_argument("output parent"))?;
    let mut command = Command::new(&step.engine.binary_path);
    command
        .current_dir(output_parent)
        .arg("--sandbox=true")
        .arg(format!("--from={reader}"))
        .arg(format!("--to={target}"))
        .arg("--standalone")
        .arg("--output")
        .arg(partial_path)
        .arg("--")
        .arg(&input.artifact.canonical_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| {
        FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Execute,
            "Unable to start Pandoc",
            "Run doctor and verify the Pandoc engine.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid_plan_argument("Pandoc stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| invalid_plan_argument("Pandoc stderr"))?;
    let stdout_task = tokio::spawn(drain_stream(stdout));
    let stderr_task = tokio::spawn(read_bounded_tail(stderr, MAX_STDERR_BYTES));
    let status = tokio::select! {
        status = child.wait() => status.map_err(|error| {
            FormatWrightError::new(ErrorCode::ExecutionFailed, Stage::Execute, "Unable to wait for Pandoc", "Retry the conversion.").with_diagnostic(error.to_string())
        })?,
        () = cancellation.cancelled() => {
            terminate_process_tree(&mut child).await;
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(ErrorCode::Cancelled, Stage::Execute, "Document conversion was cancelled", "Retry when ready."));
        }
    };
    let _ = stdout_task.await;
    let stderr = stderr_task
        .await
        .unwrap_or_else(|error| format!("stderr reader failed: {error}"));
    if !status.success() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            format!("Pandoc exited with status {status}"),
            "Correct the document or inspect the diagnostic.",
        )
        .with_diagnostic(stderr));
    }
    if !partial_path.is_file() {
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            format!("Pandoc produced no {} output", target.to_uppercase()),
            "Retry or report the input.",
        ));
    }
    if cancellation.is_cancelled() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::Cancelled,
            Stage::Execute,
            "Document conversion was cancelled before validation",
            "Retry when ready.",
        ));
    }
    if let Err(error) = observer(ExecutionMilestone::EngineFinished) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    if let Err(error) = ensure_input_unchanged(input, Stage::Commit).await {
        cleanup_partial(partial_path);
        return Err(error);
    }
    let output_probe = match inspect_document(partial_path).await {
        Ok(probe) => probe,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(error);
        }
    };
    let mut report = if target == "epub" {
        validate_epub_output(input, &output_probe, plan, job_id)
    } else {
        validate_docx_output(input, &output_probe, plan, job_id)
    };
    if report.status == ValidationStatus::Fail {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            format!("{} output failed validation", target.to_uppercase()),
            "Inspect the validation report and adjust the source.",
        )
        .with_diagnostic(serde_json::to_string(&report).unwrap_or_default()));
    }
    if output_path.exists() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Commit,
            format!(
                "The {} destination appeared while running",
                target.to_uppercase()
            ),
            "Choose another output path.",
        ));
    }
    if let Err(error) = commit_path_no_replace(partial_path, output_path) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    report.output.display_path = Some(output_path.to_string_lossy().into_owned());
    Ok(ExecutionResult {
        output_path: output_path.to_owned(),
        report,
    })
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn execute_structured_plan<F>(
    input: &Probe,
    plan: &Plan,
    job_id: Uuid,
    cancellation: CancellationToken,
    output_path: &Path,
    partial_path: &Path,
    observer: &mut F,
) -> Result<ExecutionResult>
where
    F: FnMut(ExecutionMilestone) -> Result<()>,
{
    let worker_input = input.artifact.canonical_path.clone();
    let worker_partial = partial_path.to_owned();
    let worker_plan = plan.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        convert_structured_file(&worker_input, &worker_partial, &worker_plan)
    });
    let worker_result = tokio::select! {
        result = &mut worker => result.map_err(|error| {
            FormatWrightError::new(
                ErrorCode::Internal,
                Stage::Execute,
                "Structured conversion worker failed",
                "Retry the conversion or report the input as a bug.",
            )
            .with_diagnostic(error.to_string())
        })?,
        () = cancellation.cancelled() => {
            // Blocking filesystem work cannot be aborted safely. Wait until it
            // releases the staged file, then remove that file before returning.
            let _ = worker.await;
            cleanup_partial(partial_path);
            return Err(FormatWrightError::new(
                ErrorCode::Cancelled,
                Stage::Execute,
                "Conversion was cancelled",
                "Retry when ready.",
            ));
        }
    };
    if let Err(error) = worker_result {
        cleanup_partial(partial_path);
        return Err(error);
    }
    if !partial_path.is_file() {
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            "Structured adapter reported success but produced no output",
            "Retry the conversion or report the input as a bug.",
        ));
    }
    if cancellation.is_cancelled() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::Cancelled,
            Stage::Execute,
            "Conversion was cancelled before validation",
            "Retry when ready.",
        ));
    }
    if let Err(error) = observer(ExecutionMilestone::EngineFinished) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    if let Err(error) = ensure_input_unchanged(input, Stage::Commit).await {
        cleanup_partial(partial_path);
        return Err(error);
    }
    let output_probe = match inspect_structured(partial_path).await {
        Ok(probe) => probe,
        Err(error) => {
            cleanup_partial(partial_path);
            return Err(error);
        }
    };
    let mut report = validate_structured_output(input, &output_probe, plan, job_id);
    if report.status == ValidationStatus::Fail {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "Structured output failed required validation checks",
            "Inspect the validation report and choose another Plan.",
        )
        .with_diagnostic(
            serde_json::to_string(&report).unwrap_or_else(|_| "report serialization failed".into()),
        ));
    }
    if output_path.exists() {
        cleanup_partial(partial_path);
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Commit,
            "The destination appeared while the job was running",
            "Choose another output path and retry.",
        ));
    }
    if let Err(error) = commit_path_no_replace(partial_path, output_path) {
        cleanup_partial(partial_path);
        return Err(error);
    }
    report.output.display_path = Some(output_path.to_string_lossy().into_owned());
    Ok(ExecutionResult {
        output_path: output_path.to_owned(),
        report,
    })
}

#[allow(clippy::too_many_lines)]
fn configure_ffmpeg_output(command: &mut Command, plan: &Plan, partial_path: &Path) -> Result<()> {
    let step = plan.steps.first().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::Internal,
            Stage::Execute,
            "Plan has no FFmpeg step",
            "Create a new Plan and retry.",
        )
    })?;
    if step.operation == Operation::MetadataClean {
        configure_metadata_clean(command, plan, step)?;
        command.arg("-n").arg(partial_path);
        return Ok(());
    }
    match plan.target_format.as_str() {
        "mp4" => {
            command.arg("-map").arg("0:v?").arg("-map").arg("0:a?");
            if step.arguments.get("subtitle_mode").map(String::as_str) == Some("copy") {
                command.arg("-map").arg("0:s?");
            }
            let video_mode = checked_argument(step, "video_mode", &["copy", "libx264"])?;
            let audio_mode = checked_argument(step, "audio_mode", &["copy", "aac"])?;
            command.arg("-c:v").arg(video_mode);
            if video_mode != "copy" {
                let preset = video_preset_argument(step)?;
                let crf = video_crf_argument(step)?;
                command
                    .arg("-preset")
                    .arg(preset)
                    .arg("-crf")
                    .arg(crf.to_string());
            }
            command.arg("-c:a").arg(audio_mode);
            if audio_mode != "copy" {
                let bitrate = audio_bitrate_argument(step)?;
                command.arg("-b:a").arg(format!("{bitrate}k"));
            }
            if step.arguments.get("subtitle_mode").map(String::as_str) == Some("copy") {
                command.arg("-c:s").arg("copy");
            }
            command
                .arg("-map_metadata")
                .arg("0")
                .arg("-map_chapters")
                .arg("0")
                .arg("-movflags")
                .arg("+faststart")
                .arg("-f")
                .arg("mp4");
        }
        "mp3" | "m4a" | "wav" | "flac" | "ogg" | "opus" | "aac" => {
            let stream_index = step
                .arguments
                .get("audio_stream_index")
                .and_then(|value| value.parse::<u32>().ok())
                .ok_or_else(|| invalid_plan_argument("audio_stream_index"))?;
            let (allowed_codecs, expected_muxer): (&[&str], &str) =
                match plan.target_format.as_str() {
                    "mp3" => (&["copy", "libmp3lame"], "mp3"),
                    "m4a" => (&["copy", "aac"], "ipod"),
                    "wav" => (&["copy", "pcm_s16le"], "wav"),
                    "flac" => (&["copy", "flac"], "flac"),
                    "ogg" => (&["copy", "libvorbis"], "ogg"),
                    "opus" => (&["copy", "libopus"], "opus"),
                    "aac" => (&["copy", "aac"], "adts"),
                    _ => unreachable!("outer match is exhaustive"),
                };
            let audio_mode = checked_argument(step, "audio_mode", allowed_codecs)?;
            let muxer = checked_argument(step, "muxer", &[expected_muxer])?;
            command
                .arg("-map")
                .arg(format!("0:{stream_index}"))
                .arg("-vn")
                .arg("-c:a")
                .arg(audio_mode);
            match audio_mode {
                "libvorbis" => {
                    command.arg("-q:a").arg("5");
                }
                "libopus" => {
                    let bitrate = audio_bitrate_argument(step)?.max(8);
                    command.arg("-b:a").arg(format!("{bitrate}k"));
                }
                "libmp3lame" | "aac" => {
                    let bitrate = audio_bitrate_argument(step)?;
                    command.arg("-b:a").arg(format!("{bitrate}k"));
                }
                _ => {}
            }
            command.arg("-map_metadata").arg("0");
            if plan.target_format == "m4a" {
                command.arg("-movflags").arg("+faststart");
            }
            command.arg("-f").arg(muxer);
        }
        "gif" => {
            let stream_index = checked_u32_argument(step, "video_stream_index", 0, u32::MAX)?;
            let start_millis = checked_u64_argument(step, "start_millis")?;
            let duration = step
                .arguments
                .get("duration_millis")
                .ok_or_else(|| invalid_plan_argument("duration_millis"))?;
            let frames_per_second = checked_u32_argument(step, "frames_per_second", 1, 60)?;
            let loop_count = checked_u32_argument(step, "loop_count", 0, u16::MAX.into())?;
            let width = step
                .arguments
                .get("width")
                .ok_or_else(|| invalid_plan_argument("width"))?;
            let scale = if width == "source" {
                String::new()
            } else {
                let parsed_width = width
                    .parse::<u32>()
                    .ok()
                    .filter(|value| (1..=16_384).contains(value))
                    .ok_or_else(|| invalid_plan_argument("width"))?;
                format!("scale={parsed_width}:-2:flags=lanczos,")
            };
            let filter = format!(
                "[0:{stream_index}]fps={frames_per_second},{scale}split[s0][s1];[s0]palettegen=max_colors=256[p];[s1][p]paletteuse=dither=sierra2_4a[v]"
            );
            if start_millis > 0 {
                command
                    .arg("-ss")
                    .arg(format_seconds_from_millis(start_millis));
            }
            if duration != "full" {
                let duration_millis = duration
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or_else(|| invalid_plan_argument("duration_millis"))?;
                command
                    .arg("-t")
                    .arg(format_seconds_from_millis(duration_millis));
            }
            command
                .arg("-filter_complex")
                .arg(filter)
                .arg("-map")
                .arg("[v]")
                .arg("-an")
                .arg("-loop")
                .arg(loop_count.to_string())
                .arg("-f")
                .arg("gif");
        }
        "jpeg" | "png" | "webp" | "avif" => {
            let stream_index = checked_u32_argument(step, "video_stream_index", 0, u32::MAX)?;
            let (allowed_codec, expected_muxer) = match plan.target_format.as_str() {
                "jpeg" => ("mjpeg", "image2"),
                "png" => ("png", "image2"),
                "webp" => ("libwebp", "webp"),
                "avif" => ("libaom-av1", "avif"),
                _ => unreachable!("outer match is exhaustive"),
            };
            let codec = checked_argument(step, "codec", &[allowed_codec])?;
            let muxer = checked_argument(step, "muxer", &[expected_muxer])?;
            command
                .arg("-map")
                .arg(format!("0:{stream_index}"))
                .arg("-frames:v")
                .arg("1")
                .arg("-an")
                .arg("-sn")
                .arg("-dn")
                .arg("-map_metadata")
                .arg("-1");
            if let Some(width) = step.arguments.get("width")
                && width != "source"
            {
                let width = width
                    .parse::<u32>()
                    .ok()
                    .filter(|value| (1..=16_384).contains(value))
                    .ok_or_else(|| invalid_plan_argument("width"))?;
                command
                    .arg("-vf")
                    .arg(format!("scale={width}:-2:flags=lanczos"));
            }
            command.arg("-c:v").arg(codec);
            match plan.target_format.as_str() {
                "jpeg" => {
                    let quality = checked_u32_argument(step, "encoder_quality", 2, 31)?;
                    command
                        .arg("-q:v")
                        .arg(quality.to_string())
                        .arg("-pix_fmt")
                        .arg("yuvj444p");
                }
                "png" => {
                    command.arg("-compression_level").arg("6");
                }
                "webp" => {
                    let quality = checked_u32_argument(step, "encoder_quality", 1, 100)?;
                    command.arg("-quality").arg(quality.to_string());
                }
                "avif" => {
                    let quality = checked_u32_argument(step, "encoder_quality", 0, 63)?;
                    command
                        .arg("-crf")
                        .arg(quality.to_string())
                        .arg("-cpu-used")
                        .arg("4")
                        .arg("-still-picture")
                        .arg("1");
                }
                _ => unreachable!("outer match is exhaustive"),
            }
            command.arg("-f").arg(muxer);
        }
        target => {
            return Err(FormatWrightError::new(
                ErrorCode::Unsupported,
                Stage::Execute,
                format!("No FFmpeg runner is available for {target}"),
                "Create a Plan for a supported media target.",
            ));
        }
    }
    command.arg("-n").arg(partial_path);
    Ok(())
}

fn configure_metadata_clean(
    command: &mut Command,
    plan: &Plan,
    step: &crate::domain::PlanStep,
) -> Result<()> {
    let expected_muxer = match plan.target_format.as_str() {
        "mp4" => "mp4",
        "mov" => "mov",
        "mkv" => "matroska",
        "webm" => "webm",
        "mp3" => "mp3",
        "m4a" => "ipod",
        "wav" => "wav",
        "flac" => "flac",
        "ogg" => "ogg",
        "opus" => "opus",
        "aac" => "adts",
        "jpeg" | "png" => "image2",
        "webp" => "webp",
        "avif" => "avif",
        _ => return Err(invalid_plan_argument("metadata-clean target format")),
    };
    let muxer = checked_argument(step, "muxer", &[expected_muxer])?;
    command
        .arg("-map")
        .arg("0")
        .arg("-c")
        .arg("copy")
        .arg("-map_metadata")
        .arg("0")
        .arg("-map_chapters")
        .arg("-1");
    let removed_keys = plan
        .constraints
        .get("removed_metadata_keys")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid_plan_argument("removed_metadata_keys"))?;
    for key in removed_keys {
        let key = key
            .as_str()
            .filter(|key| valid_metadata_key(key))
            .ok_or_else(|| invalid_plan_argument("removed_metadata_keys"))?;
        command.arg("-metadata").arg(format!("{key}="));
    }
    command.arg("-f").arg(muxer);
    Ok(())
}

fn valid_metadata_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn checked_u32_argument(
    step: &crate::domain::PlanStep,
    name: &str,
    minimum: u32,
    maximum: u32,
) -> Result<u32> {
    step.arguments
        .get(name)
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| (minimum..=maximum).contains(value))
        .ok_or_else(|| invalid_plan_argument(name))
}

fn checked_u64_argument(step: &crate::domain::PlanStep, name: &str) -> Result<u64> {
    step.arguments
        .get(name)
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| invalid_plan_argument(name))
}

fn format_seconds_from_millis(milliseconds: u64) -> String {
    format!("{}.{:03}", milliseconds / 1_000, milliseconds % 1_000)
}

fn checked_argument<'a>(
    step: &'a crate::domain::PlanStep,
    name: &str,
    allowed: &[&str],
) -> Result<&'a str> {
    let value = step
        .arguments
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| invalid_plan_argument(name))?;
    if allowed.contains(&value) {
        Ok(value)
    } else {
        Err(invalid_plan_argument(name))
    }
}

/// Reads the requested x264 preset with an allowlist; defaults to `medium`,
/// which is the value the hardcoded pipeline used before the knob existed.
fn video_preset_argument(step: &crate::domain::PlanStep) -> Result<&str> {
    let preset = step
        .arguments
        .get("video_preset")
        .map_or("medium", String::as_str);
    if matches!(
        preset,
        "ultrafast"
            | "superfast"
            | "veryfast"
            | "faster"
            | "fast"
            | "medium"
            | "slow"
            | "slower"
            | "veryslow"
    ) {
        Ok(preset)
    } else {
        Err(invalid_plan_argument("video_preset"))
    }
}

/// Reads the requested CRF quality (0-51); defaults to 20.
fn video_crf_argument(step: &crate::domain::PlanStep) -> Result<u8> {
    let crf = step
        .arguments
        .get("video_crf")
        .map_or("20", String::as_str)
        .parse::<u8>()
        .map_err(|_| invalid_plan_argument("video_crf"))?;
    if crf <= 51 {
        Ok(crf)
    } else {
        Err(invalid_plan_argument("video_crf"))
    }
}

/// Reads the requested audio bitrate in kbps (8-320); defaults to 192.
fn audio_bitrate_argument(step: &crate::domain::PlanStep) -> Result<u32> {
    let bitrate = step
        .arguments
        .get("audio_bitrate_kbps")
        .map_or("192", String::as_str)
        .parse::<u32>()
        .map_err(|_| invalid_plan_argument("audio_bitrate_kbps"))?;
    if (8..=320).contains(&bitrate) {
        Ok(bitrate)
    } else {
        Err(invalid_plan_argument("audio_bitrate_kbps"))
    }
}

fn invalid_plan_argument(name: &str) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::PolicyBlocked,
        Stage::Execute,
        format!("Plan contains an invalid or missing {name} argument"),
        "Create a new Plan with the installed FormatWright version.",
    )
}

fn enforce_network_policy(plan: &Plan) -> Result<()> {
    if plan.network_policy != crate::domain::NetworkPolicy::Deny {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Execute,
            "This build executes only network-denied Plans",
            "Create a local-only Plan. Explicit network access is out of scope for v0.1.",
        ));
    }
    Ok(())
}

/// Resolves a Plan output into a canonical parent directory without touching
/// the destination file.
///
/// # Errors
///
/// Returns an input error for missing filenames, network paths, or unavailable
/// directories.
pub fn resolve_output_path(plan: &Plan) -> Result<PathBuf> {
    let requested = plan.output_path.as_ref().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "Plan has no output path",
            "Provide an explicit output path.",
        )
    })?;
    ensure_local_filesystem_path(requested, Stage::Plan)?;
    let resolved = resolve_output_identity(requested, Stage::Plan)?;
    let parent = resolved.parent().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "Resolved output path has no parent directory",
            "Choose a complete output path.",
        )
    })?;
    let metadata = parent.metadata().map_err(|error| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            format!("Output directory is unavailable: {}", parent.display()),
            "Create the output directory and check permissions.",
        )
        .with_diagnostic(error.to_string())
    })?;
    if !metadata.is_dir() {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            format!("Output parent is not a directory: {}", parent.display()),
            "Choose an existing output directory.",
        ));
    }
    Ok(resolved)
}

/// Returns the deterministic same-directory staging path for one durable job.
///
/// # Errors
///
/// Returns an input error when the output filename is not Unicode.
pub fn staged_output_path(output: &Path, job_id: Uuid) -> Result<PathBuf> {
    let file_name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Plan,
                "Output filename is not valid Unicode",
                "Choose a Unicode output filename.",
            )
        })?;
    Ok(output.with_file_name(format!(".formatwright-partial-{job_id}-{file_name}")))
}

/// Removes a deterministic staged output left after a process crash.
///
/// # Errors
///
/// Returns a storage error when an existing staged file cannot be removed.
pub fn cleanup_staged_output(output: &Path, job_id: Uuid) -> Result<bool> {
    ensure_local_filesystem_path(output, Stage::Commit)?;
    let resolved_output = resolve_output_identity(output, Stage::Commit)?;
    let candidates = staged_output_candidates(&resolved_output, job_id)?;
    let mut removed = false;
    for partial in candidates {
        removed |= remove_staged_path(&partial)?;
    }
    Ok(removed)
}

/// Returns every deterministic staging path that recovery may own for a job.
///
/// # Errors
///
/// Returns an input error when the output path is incomplete or non-Unicode.
pub fn staged_output_candidates(output: &Path, job_id: Uuid) -> Result<Vec<PathBuf>> {
    Ok(vec![
        staged_output_path(output, job_id)?,
        office_staged_work_path(output, job_id)?,
    ])
}

fn office_staged_work_path(output: &Path, job_id: Uuid) -> Result<PathBuf> {
    let parent = output.parent().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "Office output path has no parent directory",
            "Choose a complete output path.",
        )
    })?;
    let identity = job_id.simple().to_string();
    Ok(parent.join(format!(".fw-{}", &identity[..12])))
}

fn remove_staged_path(partial: &Path) -> Result<bool> {
    let metadata = match std::fs::symlink_metadata(partial) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(staged_cleanup_error(partial, &error)),
    };
    if is_reparse_or_symlink(&metadata) {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Commit,
            format!(
                "Refusing to remove a linked or reparse staging path: {}",
                partial.display()
            ),
            "Inspect or quarantine the unexpected staging link manually.",
        ));
    }
    let result = if metadata.is_dir() {
        std::fs::remove_dir_all(partial)
    } else {
        std::fs::remove_file(partial)
    };
    match result {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(staged_cleanup_error(partial, &error)),
    }
}

#[cfg(windows)]
fn is_reparse_or_symlink(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_or_symlink(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn staged_cleanup_error(partial: &Path, error: &std::io::Error) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::StorageFailed,
        Stage::Commit,
        format!("Unable to remove staged output: {}", partial.display()),
        "Close processes using the file and retry recovery.",
    )
    .with_diagnostic(error.to_string())
}

async fn ensure_input_unchanged(input: &Probe, stage: Stage) -> Result<()> {
    let observed = identify_artifact(&input.artifact.canonical_path).await?;
    if observed.size_bytes != input.artifact.size_bytes
        || observed.modified_unix_ms != input.artifact.modified_unix_ms
        || observed.fast_fingerprint != input.artifact.fast_fingerprint
    {
        return Err(FormatWrightError::new(
            ErrorCode::InputChanged,
            stage,
            "Input changed after inspection",
            "Inspect and plan the input again.",
        ));
    }
    Ok(())
}

async fn drain_stream<R>(mut reader: R)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

async fn read_bounded_tail<R>(mut reader: R, limit: usize) -> String
where
    R: AsyncRead + Unpin,
{
    let mut tail = VecDeque::with_capacity(limit);
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        for byte in &buffer[..read] {
            if tail.len() == limit {
                tail.pop_front();
            }
            tail.push_back(*byte);
        }
    }
    String::from_utf8_lossy(&tail.into_iter().collect::<Vec<_>>()).into_owned()
}

async fn terminate_process_tree(child: &mut Child) {
    #[cfg(unix)]
    if terminate_unix_process_group(child).await {
        return;
    }

    #[cfg(windows)]
    if let Some(process_id) = child.id() {
        let status = Command::new("taskkill")
            .args(["/PID", &process_id.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        if status.is_ok_and(|status| status.success()) {
            let _ = child.wait().await;
            return;
        }
    }

    let _ = child.start_kill();
    let _ = child.wait().await;
}

#[cfg(unix)]
async fn terminate_unix_process_group(child: &mut Child) -> bool {
    let Some(process_id) = child.id() else {
        return false;
    };
    let Ok(raw_process_id) = i32::try_from(process_id) else {
        return false;
    };
    let process_group = Pid::from_raw(raw_process_id);

    match killpg(process_group, Signal::SIGTERM) {
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(error) => {
            tracing::warn!(%process_id, %error, "failed to terminate Unix process group gracefully");
            return false;
        }
    }

    let deadline = Instant::now() + GRACEFUL_TERMINATION_TIMEOUT;
    while Instant::now() < deadline {
        if matches!(killpg(process_group, None), Err(Errno::ESRCH)) {
            break;
        }
        sleep(Duration::from_millis(25)).await;
    }

    if !matches!(killpg(process_group, None), Err(Errno::ESRCH)) {
        match killpg(process_group, Signal::SIGKILL) {
            Ok(()) | Err(Errno::ESRCH) => {}
            Err(error) => {
                tracing::warn!(%process_id, %error, "failed to force-kill Unix process group");
            }
        }
    }

    if timeout(FORCED_TERMINATION_TIMEOUT, child.wait())
        .await
        .is_err()
    {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    true
}

fn cleanup_partial(path: &Path) {
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    if let Err(error) = result
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(partial = %path.display(), %error, "failed to clean partial output");
    }
}

fn commit_path_no_replace(source: &Path, destination: &Path) -> Result<()> {
    rename_path_no_replace(source, destination).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists || destination.exists() {
            return FormatWrightError::new(
                ErrorCode::OutputConflict,
                Stage::Commit,
                "The destination appeared before the validated output could be committed",
                "Keep the existing destination and choose another output path.",
            )
            .with_diagnostic(error.to_string());
        }
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Commit,
            "Validated output could not be committed without overwriting",
            "Check destination permissions and local filesystem support for atomic no-replace moves.",
        )
        .with_diagnostic(error.to_string())
    })
}

fn rename_path_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    let path = tempfile::TempPath::try_from_path(source.to_path_buf())?;
    match path.persist_noclobber(destination) {
        Ok(()) => Ok(()),
        Err(mut error) => {
            error.path.disable_cleanup(true);
            Err(error.error)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    #[cfg(windows)]
    use std::path::PathBuf;

    #[cfg(any(unix, windows))]
    use std::process::Stdio;
    #[cfg(unix)]
    use std::time::Duration;

    use tempfile::tempdir;
    #[cfg(unix)]
    use tokio::process::Command;
    #[cfg(windows)]
    use tokio::process::Command;
    #[cfg(unix)]
    use tokio::time::sleep;
    #[cfg(windows)]
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use super::commit_path_no_replace;
    #[cfg(unix)]
    use super::terminate_process_tree;
    #[cfg(windows)]
    use super::{cleanup_partial, terminate_process_tree, wait_for_regular_file};
    use super::{
        cleanup_staged_output, enforce_network_policy, resolve_output_path,
        staged_output_candidates, staged_output_path,
    };
    use crate::ErrorCode;
    use crate::domain::{ChangeSet, NetworkPolicy, Plan, PlanStep, SCHEMA_VERSION};

    fn knob_step(arguments: &[(&str, &str)]) -> PlanStep {
        PlanStep {
            step_id: "step-1".to_owned(),
            capability_id: "ffmpeg.test".to_owned(),
            engine: formatwright_engine_sdk::EngineIdentity {
                engine_id: "ffmpeg".to_owned(),
                version: "test".to_owned(),
                binary_path: std::path::PathBuf::from("ffmpeg.exe"),
                binary_sha256: "0".repeat(64),
                manifest_sha256: None,
                build_configuration: None,
                certification: formatwright_engine_sdk::Certification::Unverified,
            },
            operation: formatwright_engine_sdk::Operation::Transcode,
            loss_class: formatwright_engine_sdk::LossClass::Lossy,
            arguments: arguments
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
            estimated_temporary_bytes: None,
        }
    }

    #[test]
    fn quality_knob_helpers_apply_defaults_and_validate_ranges() {
        use super::{audio_bitrate_argument, video_crf_argument, video_preset_argument};

        let bare = knob_step(&[]);
        assert_eq!(
            video_preset_argument(&bare).expect("default preset"),
            "medium"
        );
        assert_eq!(video_crf_argument(&bare).expect("default crf"), 20);
        assert_eq!(audio_bitrate_argument(&bare).expect("default bitrate"), 192);

        let tuned = knob_step(&[
            ("video_preset", "slow"),
            ("video_crf", "28"),
            ("audio_bitrate_kbps", "96"),
        ]);
        assert_eq!(video_preset_argument(&tuned).expect("tuned preset"), "slow");
        assert_eq!(video_crf_argument(&tuned).expect("tuned crf"), 28);
        assert_eq!(audio_bitrate_argument(&tuned).expect("tuned bitrate"), 96);

        assert!(
            video_preset_argument(&knob_step(&[("video_preset", "turbo")])).is_err(),
            "unknown presets are rejected"
        );
        assert!(
            video_crf_argument(&knob_step(&[("video_crf", "52")])).is_err(),
            "CRF above 51 is rejected"
        );
        assert!(
            audio_bitrate_argument(&knob_step(&[("audio_bitrate_kbps", "4")])).is_err(),
            "bitrates below 8 kbps are rejected"
        );
    }

    #[test]
    fn refuses_a_plan_that_requests_network_access() {
        let plan = Plan {
            schema_version: SCHEMA_VERSION,
            plan_id: Uuid::new_v4(),
            plan_hash: "blake3:test".to_owned(),
            input_fingerprint: "fwfp-v1:test".to_owned(),
            target_format: "bin".to_owned(),
            constraints: BTreeMap::new(),
            steps: Vec::new(),
            changes: ChangeSet::default(),
            validators: Vec::new(),
            network_policy: NetworkPolicy::ExplicitAllow,
            output_path: None,
            estimated_output_bytes: None,
        };
        let error = enforce_network_policy(&plan).expect_err("network plan must be blocked");
        assert_eq!(error.code, ErrorCode::PolicyBlocked);
    }

    #[test]
    fn atomic_file_commit_never_replaces_an_existing_destination() {
        let directory = tempdir().expect("temporary directory");
        let staged = directory.path().join("staged.bin");
        let destination = directory.path().join("destination.bin");
        fs::write(&staged, b"validated output").expect("staged output");
        fs::write(&destination, b"existing data").expect("existing destination");

        let error = commit_path_no_replace(&staged, &destination)
            .expect_err("no-replace commit must reject an existing file");

        assert_eq!(error.code, ErrorCode::OutputConflict);
        assert_eq!(
            fs::read(&destination).expect("destination data"),
            b"existing data"
        );
        assert_eq!(fs::read(&staged).expect("staged data"), b"validated output");
    }

    #[test]
    fn atomic_directory_commit_never_replaces_an_existing_destination() {
        let directory = tempdir().expect("temporary directory");
        let staged = directory.path().join("staged-pages");
        let destination = directory.path().join("destination-pages");
        fs::create_dir(&staged).expect("staged directory");
        fs::write(staged.join("page-1.png"), b"validated page").expect("staged page");
        fs::create_dir(&destination).expect("destination directory");
        fs::write(destination.join("keep.txt"), b"existing data").expect("existing marker");

        let error = commit_path_no_replace(&staged, &destination)
            .expect_err("no-replace commit must reject an existing directory");

        assert_eq!(error.code, ErrorCode::OutputConflict);
        assert_eq!(
            fs::read(destination.join("keep.txt")).expect("destination marker"),
            b"existing data"
        );
        assert!(staged.join("page-1.png").is_file());
    }

    #[test]
    fn atomic_file_commit_moves_to_an_absent_destination() {
        let directory = tempdir().expect("temporary directory");
        let staged = directory.path().join("staged.bin");
        let destination = directory.path().join("destination.bin");
        fs::write(&staged, b"validated output").expect("staged output");

        commit_path_no_replace(&staged, &destination).expect("no-replace commit");

        assert!(!staged.exists());
        assert_eq!(
            fs::read(destination).expect("committed data"),
            b"validated output"
        );
    }

    #[cfg(windows)]
    #[test]
    fn output_resolution_uses_the_same_windows_identity_policy_as_reservations() {
        let directory = tempdir().expect("temporary directory");
        let ordinary = directory.path().join("result.mp4");
        let mut plan = Plan {
            schema_version: SCHEMA_VERSION,
            plan_id: Uuid::new_v4(),
            plan_hash: "blake3:test".to_owned(),
            input_fingerprint: "fwfp-v1:test".to_owned(),
            target_format: "bin".to_owned(),
            constraints: BTreeMap::new(),
            steps: Vec::new(),
            changes: ChangeSet::default(),
            validators: Vec::new(),
            network_policy: NetworkPolicy::Deny,
            output_path: Some(PathBuf::from(format!(r"\\?\{}", ordinary.display()))),
            estimated_output_bytes: None,
        };
        let canonical_directory = directory
            .path()
            .canonicalize()
            .expect("canonical temporary directory");
        let canonical_rendered = canonical_directory.to_string_lossy();
        let expected = PathBuf::from(
            canonical_rendered
                .strip_prefix(r"\\?\")
                .unwrap_or(&canonical_rendered),
        )
        .join("result.mp4");
        assert_eq!(
            resolve_output_path(&plan).expect("resolve verbatim disk path"),
            expected
        );

        plan.output_path = Some(directory.path().join("result.mp4."));
        let error = resolve_output_path(&plan).expect_err("trimmed alias must be rejected");
        assert_eq!(error.code, ErrorCode::InputInvalid);
        assert_eq!(error.stage, crate::Stage::Plan);
    }

    #[test]
    fn staged_output_is_deterministic_and_recoverable() {
        let directory = tempdir().expect("temporary directory");
        let output = directory.path().join("result.mp4");
        let job_id =
            Uuid::parse_str("019fea79-90c7-7e31-8165-f5c468ac119e").expect("static UUID is valid");
        let staged = staged_output_path(&output, job_id).expect("staged path");
        assert_eq!(
            staged.file_name().and_then(|name| name.to_str()),
            Some(".formatwright-partial-019fea79-90c7-7e31-8165-f5c468ac119e-result.mp4")
        );

        fs::write(&staged, b"partial").expect("write partial");
        assert!(cleanup_staged_output(&output, job_id).expect("clean partial"));
        assert!(!staged.exists());
        assert!(!cleanup_staged_output(&output, job_id).expect("idempotent cleanup"));

        fs::create_dir(&staged).expect("create staged directory");
        fs::write(staged.join("page-000001.png"), b"partial page").expect("write staged page");
        assert!(cleanup_staged_output(&output, job_id).expect("clean staged directory"));
        assert!(!staged.exists());

        let office_stage = staged_output_candidates(&output, job_id)
            .expect("staging candidates")
            .into_iter()
            .nth(1)
            .expect("Office staging candidate");
        fs::create_dir(&office_stage).expect("create Office staging directory");
        fs::write(office_stage.join("profile-state"), b"partial")
            .expect("write Office staged state");
        assert!(cleanup_staged_output(&output, job_id).expect("clean Office stage"));
        assert!(!office_stage.exists());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_termination_reaches_descendants_and_cleans_partial_output() {
        let directory = tempdir().expect("temporary directory");
        let ready_marker = directory.path().join("descendant-ready");
        let survivor_marker = directory.path().join("descendant-survived");
        let partial = directory.path().join(".formatwright-partial-fixture");
        fs::create_dir(&partial).expect("create staged directory");
        fs::write(partial.join("page-000001.tmp"), b"partial").expect("write partial fixture");

        let parent_script = r"
$payload = '$ready=$env:FORMATWRIGHT_CHILD_READY; $survivor=$env:FORMATWRIGHT_SURVIVOR_MARKER; [IO.File]::WriteAllText($ready,''ready''); Start-Sleep -Milliseconds 1200; [IO.File]::WriteAllText($survivor,''survived'')'
$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($payload))
Start-Process -FilePath $env:FORMATWRIGHT_POWERSHELL -ArgumentList @('-NoProfile','-NonInteractive','-EncodedCommand',$encoded) -WindowStyle Hidden
Start-Sleep -Seconds 30
";
        let powershell =
            PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
                .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut command = Command::new(&powershell);
        command
            .args(["-NoProfile", "-NonInteractive", "-Command", parent_script])
            .env("FORMATWRIGHT_POWERSHELL", &powershell)
            .env("FORMATWRIGHT_CHILD_READY", &ready_marker)
            .env("FORMATWRIGHT_SURVIVOR_MARKER", &survivor_marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().expect("spawn Windows process-tree fixture");
        assert!(
            wait_for_regular_file(
                &ready_marker,
                &CancellationToken::new(),
                std::time::Duration::from_secs(3),
            )
            .await,
            "descendant did not start"
        );

        terminate_process_tree(&mut child).await;
        cleanup_partial(&partial);
        tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;

        assert!(child.try_wait().expect("query parent").is_some());
        assert!(
            !survivor_marker.exists(),
            "a descendant escaped taskkill /T"
        );
        assert!(!partial.exists(), "cancelled partial output remained");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_termination_reaches_descendant_processes() {
        let directory = tempdir().expect("temporary directory");
        let survivor_marker = directory.path().join("descendant-survived");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("(sleep 1; printf survived > \"$FORMATWRIGHT_SURVIVOR_MARKER\") & wait")
            .env("FORMATWRIGHT_SURVIVOR_MARKER", &survivor_marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = command.spawn().expect("spawn process-group fixture");

        sleep(Duration::from_millis(100)).await;
        terminate_process_tree(&mut child).await;
        sleep(Duration::from_millis(1_100)).await;

        assert!(
            !survivor_marker.exists(),
            "a descendant escaped process-group termination"
        );
    }
}
