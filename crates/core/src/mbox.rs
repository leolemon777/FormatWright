//! MBOX 邮箱聚合输入：解析拆分为逐封 EML，txt/html 直接聚合渲染，
//! pdf 则逐封渲染→逐封 PDF→qpdf 合并，实现「整个邮箱导出一个 PDF」。
//! 内置 `formatwright.mbox` 引擎；解析失败 fail-closed。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use formatwright_engine_sdk::{EngineIdentity, LossClass, Operation};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::domain::{
    ArtifactSummary, ChangeSet, FormatDescriptor, FormatKind, NetworkPolicy, Plan, PlanRequest,
    PlanStep, Probe, ProbeEvidence, ReportRedaction, SCHEMA_VERSION, StreamKind, StreamProbe,
    ValidationCheck, ValidationReport, ValidationStatus,
};
use crate::eml::{self, ParsedEmail};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::fingerprint::identify_artifact;
use crate::planner::deterministic_plan_hash;

pub const MBOX_ENGINE_ID: &str = "formatwright.mbox";

const MAX_MBOX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_MBOX_MAILS: usize = 1000;

/// 一封从 MBOX 拆出的邮件：原始 EML 字节 + 解析结果。
pub struct MboxMail {
    pub eml_bytes: Vec<u8>,
    pub email: ParsedEmail,
}

/// 按 mboxrd 语义拆分 MBOX：以行首 `From `（前面是文件头或空行）分界，
/// 正文中的 `>From ` 系列回退一层转义。
///
/// # Errors
///
/// `InputInvalid`：空邮箱、超限（字节或封数）或任何一封解析失败。
pub fn split_mbox_bytes(bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            "MBOX input is not valid UTF-8",
            "Choose an mbox file exported in UTF-8.",
        )
        .with_diagnostic(error.to_string())
    })?;
    // Every mbox message starts with a "From " envelope line; a file that
    // never has one is not an mbox (fail-closed instead of one fake mail).
    if !text
        .lines()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| line.starts_with("From "))
    {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            "The file does not start with an mbox \"From \" envelope line",
            "Choose a file in mbox/mboxrd format.",
        ));
    }
    let mut messages: Vec<Vec<u8>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut last_line_blank = true;
    for line in text.lines() {
        if last_line_blank && line.starts_with("From ") {
            if !current.is_empty() {
                messages.push(current.join("\r\n").into_bytes());
            }
            current = Vec::new();
            last_line_blank = false;
            continue;
        }
        // mboxrd：正文中的 ">>*From " 被发件方转义，还原一层。
        let rendered = if let Some(rest) = line.strip_prefix('>')
            && rest.starts_with("From ")
        {
            rest.to_owned()
        } else {
            line.to_owned()
        };
        last_line_blank = rendered.trim().is_empty();
        current.push(rendered);
    }
    if !current.is_empty() {
        messages.push(current.join("\r\n").into_bytes());
    }
    if messages.is_empty() {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            "The MBOX file contains no messages",
            "Choose an mbox file with at least one mail.",
        ));
    }
    if messages.len() > MAX_MBOX_MAILS {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            format!(
                "The MBOX holds {} mails; the adapter limit is {MAX_MBOX_MAILS}",
                messages.len()
            ),
            "Split the mailbox before converting.",
        ));
    }
    Ok(messages)
}

/// 拆分并把每封解析为 [`ParsedEmail`]（任何一封失败即整体 fail-closed）。
///
/// # Errors
///
/// 同 [`split_mbox_bytes`]，外加逐封 EML 解析错误。
pub fn parse_mbox_file(path: &Path) -> Result<Vec<MboxMail>> {
    if let Ok(metadata) = std::fs::metadata(path)
        && metadata.len() > MAX_MBOX_BYTES
    {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            "MBOX input exceeds the 256 MiB built-in adapter limit",
            "Split the mailbox before converting.",
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            "Unable to read the MBOX file",
            "Choose an existing mbox file.",
        )
        .with_diagnostic(error.to_string())
    })?;
    split_mbox_bytes(&bytes)?
        .into_iter()
        .map(|eml_bytes| {
            let email = eml::parse_eml_bytes(&eml_bytes)?;
            Ok(MboxMail { eml_bytes, email })
        })
        .collect()
}

/// 构建 MBOX Probe：逐封聚合的属性挂在首条 stream 上（与 EML/MSG 同形）。
///
/// # Errors
///
/// 返回读取/解析错误。
pub async fn inspect_mbox(path: &Path) -> Result<Probe> {
    let artifact = identify_artifact(path).await?;
    let mails = parse_mbox_file(path)?;
    Ok(Probe {
        schema_version: SCHEMA_VERSION,
        artifact,
        format: FormatDescriptor {
            id: "mbox".to_owned(),
            kind: FormatKind::Document,
            mime_type: Some("application/mbox".to_owned()),
            container: Some("mboxrd".to_owned()),
            extension_matches: Some(true),
            confidence: 1.0,
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
            properties: mbox_properties(&mails),
        }],
        metadata: BTreeMap::new(),
        warnings: Vec::new(),
        evidence: ProbeEvidence {
            engine_id: MBOX_ENGINE_ID.to_owned(),
            engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            engine_binary_sha256: None,
        },
        duration_seconds: None,
        bit_rate: None,
    })
}

fn mbox_properties(mails: &[MboxMail]) -> BTreeMap<String, Value> {
    let concatenated = mails
        .iter()
        .map(|mail| mail.email.visible_text())
        .collect::<Vec<_>>()
        .join("\n");
    let normalized = crate::document::normalized_tokens(&concatenated);
    let has_external_resource = mails.iter().any(|mail| {
        mail.email
            .html_body
            .as_deref()
            .is_some_and(crate::eml::contains_remote_reference)
    });
    let mut properties = BTreeMap::new();
    properties.insert("mail_count".to_owned(), json!(mails.len()));
    if let Some(first) = mails.first()
        && let Some(value) = &first.email.subject
    {
        properties.insert("eml_subject".to_owned(), json!(value));
    }
    properties.insert(
        "semantic_token_digest".to_owned(),
        json!(format!(
            "blake3:{}",
            blake3::hash(normalized.as_bytes()).to_hex()
        )),
    );
    properties.insert(
        "text_characters".to_owned(),
        json!(normalized.chars().count()),
    );
    properties.insert(
        "has_external_resource".to_owned(),
        json!(has_external_resource),
    );
    properties
}

fn stream_property(probe: &Probe, name: &str) -> Value {
    probe
        .streams
        .first()
        .and_then(|stream| stream.properties.get(name))
        .cloned()
        .unwrap_or(Value::Null)
}

/// 逐封渲染的分隔标记；PDF 文本层回读时逐一核对，证明每封都进了合并。
fn mail_separator(index: usize, total: usize) -> String {
    format!("==== Anole Mail {}/{} ====", index + 1, total)
}

/// Builds the MBOX export Plan: txt/html are single builtin renders;
/// pdf is the composite (builtin render per mail → html→pdf lane per mail
/// → qpdf merge) whose per-mail sub-conversions resolve their own plans at
/// execution, exactly like chain segments.
///
/// # Errors
///
/// `Unsupported` for wrong input/target/engine, `PolicyBlocked` when any
/// mail's HTML references remote resources under deny-all.
pub fn plan_mbox_export(
    probe: &Probe,
    output_path: PathBuf,
    engine: &EngineIdentity,
    qpdf: &EngineIdentity,
    target: &str,
) -> Result<Plan> {
    let target = target.trim().trim_start_matches('.').to_ascii_lowercase();
    if probe.format.id != "mbox" {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            "MBOX export input must be an mbox mailbox",
            "Choose a file in mbox/mboxrd format.",
        ));
    }
    if !matches!(target.as_str(), "txt" | "html" | "pdf") {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            "MBOX export target must be txt, html, or pdf",
            "Choose txt, html, or the whole-mailbox pdf.",
        ));
    }
    if engine.engine_id != MBOX_ENGINE_ID {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            "The MBOX export Plan was given the wrong engine",
            "Use the built-in formatwright.mbox adapter.",
        ));
    }
    if stream_property(probe, "has_external_resource") == json!(true) {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Plan,
            "A mail in the MBOX references an external resource under deny-all policy",
            "Remove the remote image/link or wait for an explicitly authorized resource-root policy.",
        ));
    }
    let mail_count = stream_property(probe, "mail_count")
        .as_u64()
        .unwrap_or_default();
    let mut steps = vec![PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: format!("formatwright.mbox.mbox-render-{target}.builtin"),
        engine: engine.clone(),
        operation: Operation::Transform,
        loss_class: LossClass::Unknown,
        arguments: BTreeMap::from([
            ("source_format".to_owned(), "mbox".to_owned()),
            ("target_format".to_owned(), target.clone()),
            ("mail_count".to_owned(), mail_count.to_string()),
            ("network".to_owned(), "deny".to_owned()),
            ("sanitize_html".to_owned(), "true".to_owned()),
        ]),
        estimated_temporary_bytes: Some(probe.artifact.size_bytes.saturating_mul(4)),
    }];
    let mut validators = vec![
        "document.text-extractable".to_owned(),
        "mbox.mail-separators".to_owned(),
    ];
    if target == "pdf" {
        if qpdf.engine_id != "qpdf" {
            return Err(FormatWrightError::new(
                ErrorCode::EngineIncompatible,
                Stage::Plan,
                "The MBOX merge step was given the wrong engine",
                "Run doctor and use qpdf.",
            ));
        }
        steps.push(PlanStep {
            step_id: "step-2".to_owned(),
            capability_id: "qpdf.mbox-merge.all-mails".to_owned(),
            engine: qpdf.clone(),
            operation: Operation::Transform,
            loss_class: LossClass::None,
            arguments: BTreeMap::from([
                ("merge_mode".to_owned(), "concatenate-1-z".to_owned()),
                ("mail_count".to_owned(), mail_count.to_string()),
            ]),
            estimated_temporary_bytes: Some(probe.artifact.size_bytes.saturating_mul(6)),
        });
        validators.push("mbox.page-conservation".to_owned());
    }
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: Uuid::new_v4(),
        plan_hash: String::new(),
        input_fingerprint: probe.artifact.fast_fingerprint.clone(),
        target_format: target.clone(),
        constraints: BTreeMap::from([
            ("network".to_owned(), json!("deny")),
            ("external_resources".to_owned(), json!("deny")),
            ("scripts".to_owned(), json!("stripped")),
            ("mail_count".to_owned(), json!(mail_count)),
        ]),
        steps,
        changes: ChangeSet {
            preserved: vec![
                "every mail, in original order".to_owned(),
                "normalized textual content".to_owned(),
            ],
            changed: vec![format!(
                "the mailbox renders as one {} document",
                if target == "pdf" {
                    "PDF"
                } else {
                    target.as_str()
                }
            )],
            dropped: vec![
                "attachments and non-text MIME parts".to_owned(),
                "scripts, event handlers, and remote resources".to_owned(),
            ],
            unknown: vec!["visual fidelity of the original HTML".to_owned()],
        },
        validators,
        network_policy: NetworkPolicy::Deny,
        output_path: Some(output_path),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

/// Executes the MBOX export. The pdf target stages per-mail HTML, runs each
/// through the html→pdf lane (their own plans + receipts), merges with qpdf,
/// and proves page conservation (sum of per-mail pages == merged pages).
///
/// # Errors
///
/// Parse/write/engine/validation errors; staging is cleaned on every path.
pub async fn execute_mbox_export(
    probe: &Probe,
    plan: &Plan,
    job_id: Uuid,
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<(PathBuf, ValidationReport)> {
    let output = plan.output_path.clone().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Execute,
            "MBOX export Plan has no output path",
            "Choose an output path.",
        )
    })?;
    if output.exists() {
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Execute,
            "The MBOX export destination already exists",
            "Choose another output path and retry.",
        ));
    }
    let mails = parse_mbox_file(&probe.artifact.canonical_path)?;
    let mail_count = mails.len();
    let parent = output.parent().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "Resolved output path has no parent directory",
            "Choose a complete output path.",
        )
    })?;
    let staging = parent.join(format!(".fw-mbox-{job_id}"));
    std::fs::create_dir(&staging).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Execute,
            "Unable to create the MBOX staging directory",
            "Choose an output directory you can write to.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let outcome = run_mbox_export(&mails, plan, job_id, cancellation, &output, &staging).await;
    if let Err(error) = std::fs::remove_dir_all(&staging)
        && outcome.is_ok()
    {
        return Err(FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Commit,
            "Unable to remove the MBOX staging directory",
            "Remove the leftover staging directory manually.",
        )
        .with_diagnostic(error.to_string()));
    }
    let (path, mut report) = outcome?;
    report.job_id = job_id;
    Ok((path, report))
}

async fn run_mbox_export(
    mails: &[MboxMail],
    plan: &Plan,
    job_id: Uuid,
    cancellation: tokio_util::sync::CancellationToken,
    output: &Path,
    staging: &Path,
) -> Result<(PathBuf, ValidationReport)> {
    let target = plan.target_format.as_str();
    if target == "pdf" {
        // Boxing breaks the async recursion execute_plan -> mbox -> execute_plan.
        Box::pin(execute_mbox_pdf(
            mails,
            plan,
            job_id,
            cancellation,
            output,
            staging,
        ))
        .await
    } else {
        let rendered = render_mbox(mails, target == "html");
        tokio::task::spawn_blocking({
            let output = output.to_path_buf();
            let rendered = rendered.clone();
            move || std::fs::write(&output, rendered)
        })
        .await
        .map_err(worker_error)?
        .map_err(write_error)?;
        let output_probe = match crate::document::inspect_document(output).await {
            Ok(probe) => probe,
            Err(error) => {
                let _ = std::fs::remove_file(output);
                return Err(error);
            }
        };
        let rendered_text = crate::document::html_text(&rendered)
            .ok()
            .unwrap_or_else(|| rendered.clone());
        let report = build_mbox_report(
            mails,
            plan,
            job_id,
            if target == "html" { "html" } else { "plain" },
            output_probe.format.id.clone(),
            output,
            None,
            None,
            rendered_text.as_str(),
        );
        if report.status == ValidationStatus::Fail {
            let _ = std::fs::remove_file(output);
        }
        Ok((output.to_path_buf(), report))
    }
}

/// 拼接逐封渲染结果：每封前置分隔标记行，HTML 走同一净化管线。
fn render_mbox(mails: &[MboxMail], html: bool) -> String {
    let total = mails.len();
    mails
        .iter()
        .enumerate()
        .map(|(index, mail)| {
            let separator = mail_separator(index, total);
            if html {
                let body = eml::render_html(&mail.email);
                if let Some(start) = body.find("<body")
                    && let Some(open_end) = body[start..].find('>')
                {
                    let insert_at = start + open_end + 1;
                    return format!(
                        "{}<h3>{separator} {}</h3>{}",
                        &body[..insert_at],
                        mail.email.subject.as_deref().unwrap_or(""),
                        &body[insert_at..]
                    );
                }
                format!("<html><body><h3>{separator}</h3>{body}</body></html>")
            } else {
                format!("{}\n{}", separator, eml::render_txt(&mail.email))
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[allow(clippy::too_many_lines)]
async fn execute_mbox_pdf(
    mails: &[MboxMail],
    plan: &Plan,
    job_id: Uuid,
    cancellation: tokio_util::sync::CancellationToken,
    output: &Path,
    staging: &Path,
) -> Result<(PathBuf, ValidationReport)> {
    use crate::runner::execute_plan;
    use crate::workflow::prepare_conversion;

    let total = mails.len();
    let mut staged_pdfs = Vec::new();
    for (index, _mail) in mails.iter().enumerate() {
        if cancellation.is_cancelled() {
            return Err(FormatWrightError::new(
                ErrorCode::Cancelled,
                Stage::Execute,
                "MBOX export was cancelled",
                "Retry when ready.",
            ));
        }
        let html_path = staging.join(format!("mail-{index}.html"));
        let packet = render_single_html_packet(&mails[index], index, total);
        tokio::task::spawn_blocking({
            let path = html_path.clone();
            let packet = packet.clone();
            move || std::fs::write(&path, packet)
        })
        .await
        .map_err(worker_error)?
        .map_err(write_error)?;
        let pdf_path = staging.join(format!("mail-{index}.pdf"));
        let request = PlanRequest {
            target_format: "pdf".to_owned(),
            output_path: Some(pdf_path.clone()),
            ..PlanRequest::default()
        };
        let (probe, segment_plan, validation_engine) =
            prepare_conversion(&html_path, &request).await?;
        let result = execute_plan(
            &probe,
            &segment_plan,
            &validation_engine,
            Uuid::new_v4(),
            cancellation.clone(),
        )
        .await?;
        staged_pdfs.push(result.output_path);
    }
    // 逐封页数求和（每页一条 stream）。
    let pdfinfo = crate::doctor::inspect_engine("pdfinfo").await?;
    let mut expected_pages = 0_usize;
    for pdf in &staged_pdfs {
        let probe = crate::pdf::inspect_pdf(pdf, &pdfinfo).await?;
        expected_pages += probe.streams.len();
    }
    let merged = staging.join("merged.pdf");
    let qpdf = crate::doctor::inspect_engine("qpdf").await?;
    run_qpdf_merge(&qpdf, &staged_pdfs, &merged).await?;
    let merged_probe = crate::pdf::inspect_pdf(&merged, &pdfinfo).await?;
    let observed_pages = merged_probe.streams.len();
    let pdftotext = crate::doctor::inspect_engine("pdftotext").await?;
    let extracted = extract_pdf_text(&merged, &pdftotext).await?;
    let report = build_mbox_report(
        mails,
        plan,
        job_id,
        "pdf",
        merged_probe.format.id.clone(),
        &merged,
        Some(expected_pages),
        Some(observed_pages),
        &extracted,
    );
    if report.status == ValidationStatus::Fail {
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "MBOX PDF failed required validation",
            "Inspect the validation report and choose another Plan.",
        )
        .with_diagnostic(serde_json::to_string(&report).unwrap_or_default()));
    }
    if output.exists() {
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Commit,
            "The MBOX destination appeared while conversion was running",
            "Choose another output path.",
        ));
    }
    crate::runner::commit_path_no_replace(&merged, output)?;
    Ok((output.to_path_buf(), report))
}

/// 单封 HTML 包：分隔标记 + 净化后的完整邮件渲染。
fn render_single_html_packet(mail: &MboxMail, index: usize, total: usize) -> String {
    let separator = mail_separator(index, total);
    let subject = mail.email.subject.as_deref().unwrap_or("");
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head><body><h3>{separator} {subject}</h3>{}</body></html>",
        eml::render_html(&mail.email)
    )
}

async fn run_qpdf_merge(qpdf: &EngineIdentity, inputs: &[PathBuf], output: &Path) -> Result<()> {
    use tokio::io::AsyncReadExt;
    let mut command = tokio::process::Command::new(&qpdf.binary_path);
    command.arg("--empty").arg("--pages");
    for input in inputs {
        command.arg(input).arg("1-z");
    }
    command.arg("--").arg(output);
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            FormatWrightError::new(
                ErrorCode::EngineIncompatible,
                Stage::Execute,
                "Unable to start qpdf",
                "Run doctor and verify the qpdf engine.",
            )
            .with_diagnostic(error.to_string())
        })?;
    let mut stderr = Vec::new();
    if let Some(mut stream) = child.stderr.take() {
        stream.read_to_end(&mut stderr).await.ok();
    }
    let status = child.wait().await.map_err(|error| {
        FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            "Unable to wait for qpdf",
            "Retry the conversion.",
        )
        .with_diagnostic(error.to_string())
    })?;
    if !status.success() {
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            "qpdf could not merge the per-mail PDFs",
            "Inspect the per-mail PDFs and qpdf availability.",
        )
        .with_diagnostic(String::from_utf8_lossy(&stderr).into_owned()));
    }
    Ok(())
}

async fn extract_pdf_text(path: &Path, pdftotext: &EngineIdentity) -> Result<String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        tokio::process::Command::new(&pdftotext.binary_path)
            .arg(path)
            .arg("-")
            .output(),
    )
    .await
    .map_err(|_| {
        FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Validate,
            "PDF text extraction timed out",
            "Retry the conversion.",
        )
        .retryable(true)
    })?
    .map_err(|error| {
        FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Validate,
            "Unable to start pdftotext",
            "Run doctor and verify the Poppler utilities.",
        )
        .with_diagnostic(error.to_string())
    })?;
    if !output.status.success() {
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "pdftotext could not read the merged PDF",
            "Inspect the merged PDF.",
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[allow(clippy::too_many_arguments)]
fn build_mbox_report(
    mails: &[MboxMail],
    plan: &Plan,
    job_id: Uuid,
    expected_format: &str,
    observed_format: String,
    output: &Path,
    expected_pages: Option<usize>,
    observed_pages: Option<usize>,
    extracted_text: &str,
) -> ValidationReport {
    let total = mails.len();
    let separators_present = (0..total)
        .filter(|index| extracted_text.contains(&mail_separator(*index, total)))
        .count();
    let mut checks = vec![
        check(
            "MBOX_TARGET_FORMAT",
            observed_format == expected_format,
            json!(expected_format),
            json!(observed_format),
            "Detected output format.",
        ),
        check(
            "MBOX_MAIL_SEPARATORS",
            separators_present == total,
            json!(total),
            json!(separators_present),
            "Every mail separator survives into the output text layer.",
        ),
    ];
    if plan.target_format == "pdf" {
        let expected = expected_pages.unwrap_or_default();
        let observed = observed_pages.unwrap_or_default();
        checks.push(check(
            "MBOX_PAGE_CONSERVATION",
            expected == observed && observed > 0,
            json!(expected),
            json!(observed),
            "Merged page count equals the sum of per-mail pages.",
        ));
    }
    let report_status = checks.iter().fold(ValidationStatus::Pass, |state, item| {
        state.worst(item.status)
    });
    let output_artifact = identify_artifact_sync_summary(output);
    ValidationReport {
        schema_version: SCHEMA_VERSION,
        report_id: Uuid::new_v4(),
        job_id,
        plan_hash: plan.plan_hash.clone(),
        status: report_status,
        input: ArtifactSummary {
            display_path: None,
            format_id: "mbox".to_owned(),
            size_bytes: 0,
            fast_fingerprint: String::new(),
            full_blake3: None,
        },
        output: output_artifact,
        engines: plan.steps.iter().map(|step| step.engine.clone()).collect(),
        checks,
        intentional_changes: plan.changes.changed.clone(),
        redaction: ReportRedaction {
            paths_redacted: false,
            metadata_values_redacted: true,
        },
    }
}

fn identify_artifact_sync_summary(path: &Path) -> ArtifactSummary {
    let metadata = std::fs::metadata(path).ok();
    ArtifactSummary {
        display_path: Some(path.to_string_lossy().into_owned()),
        format_id: "pdf".to_owned(),
        size_bytes: metadata.map(|value| value.len()).unwrap_or_default(),
        fast_fingerprint: String::new(),
        full_blake3: None,
    }
}

fn check(
    code: &str,
    passed: bool,
    expected: Value,
    observed: Value,
    message: &str,
) -> ValidationCheck {
    ValidationCheck {
        code: code.to_owned(),
        status: if passed {
            ValidationStatus::Pass
        } else {
            ValidationStatus::Fail
        },
        required: true,
        expected,
        observed,
        evidence: "Anole native MBOX adapter".to_owned(),
        message: message.to_owned(),
    }
}

fn worker_error(error: tokio::task::JoinError) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::Internal,
        Stage::Execute,
        "MBOX export worker task failed",
        "Retry the conversion.",
    )
    .with_diagnostic(error.to_string())
}

fn write_error(error: std::io::Error) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::ExecutionFailed,
        Stage::Execute,
        "Unable to write the MBOX export output",
        "Check the destination directory and retry.",
    )
    .with_diagnostic(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::{MBOX_ENGINE_ID, split_mbox_bytes};

    const THREE_MAILS: &str = "From alice@example.org Fri Sep  4 10:00:00 2026\r
From: Alice <alice@example.org>\r
To: bob@example.org\r
Subject: First mail 440010147700\r
\r
ELECTRIC body one 998877.\r
>From the escaped line stays.\r
\r
From carol@example.org Fri Sep  4 11:00:00 2026\r
From: Carol <carol@example.org>\r
To: bob@example.org\r
Subject: =?UTF-8?B?5LiW55WM6YKu5Lu=?= MAILTOKEN2\r
\r
Body two with 中文内容 MAIL2TOKEN.\r
\r
From dave@example.org Fri Sep  4 12:00:00 2026\r
From: Dave <dave@example.org>\r
Subject: Third plain mail\r
Content-Type: text/html\r
\r
<html><body><p>MAIL3TOKEN</p><script>alert(1)</script></body></html>\r
";

    fn builtin_engine() -> formatwright_engine_sdk::EngineIdentity {
        formatwright_engine_sdk::EngineIdentity {
            engine_id: MBOX_ENGINE_ID.to_owned(),
            version: "0.1.0".to_owned(),
            binary_path: std::path::PathBuf::from("formatwright.exe"),
            binary_sha256: "0".repeat(64),
            manifest_sha256: None,
            build_configuration: None,
            certification: formatwright_engine_sdk::Certification::Experimental,
        }
    }

    fn qpdf_engine() -> formatwright_engine_sdk::EngineIdentity {
        formatwright_engine_sdk::EngineIdentity {
            engine_id: "qpdf".to_owned(),
            ..builtin_engine()
        }
    }

    #[test]
    fn splits_three_mails_and_unescapes_mboxrd() {
        let messages = split_mbox_bytes(THREE_MAILS.as_bytes()).expect("split");
        assert_eq!(messages.len(), 3);
        let first = String::from_utf8(messages[0].clone()).expect("utf8");
        assert!(first.contains("From the escaped line stays."));
        assert!(!first.contains(">From the escaped line stays."));
    }

    #[test]
    fn empty_or_garbage_mbox_fails_closed() {
        assert!(split_mbox_bytes(b"").is_err());
        assert!(split_mbox_bytes(b"no from lines at all\njust text\n").is_err());
    }

    #[test]
    fn single_mail_mbox_round_trips() {
        let single = "From a@example.org Thu Sep  3 09:00:00 2026\r
From: a@example.org\r
Subject: Solo 440\r
\r
solo body\r
";
        let messages = split_mbox_bytes(single.as_bytes()).expect("split");
        assert_eq!(messages.len(), 1);
        let parsed = crate::eml::parse_eml_bytes(&messages[0]).expect("parse");
        assert_eq!(parsed.subject.as_deref(), Some("Solo 440"));
    }

    #[tokio::test]
    async fn mbox_txt_and_html_export_validate() {
        let directory = TempDir::new().expect("tempdir");
        let source = directory.path().join("sample.mbox");
        std::fs::write(&source, THREE_MAILS).expect("write mbox");
        let probe = super::inspect_mbox(&source).await.expect("probe");
        assert_eq!(
            probe.streams[0].properties.get("mail_count"),
            Some(&serde_json::json!(3))
        );
        let engine = builtin_engine();
        for target in ["txt", "html"] {
            let output = directory.path().join(format!("out.{target}"));
            let plan =
                super::plan_mbox_export(&probe, output.clone(), &engine, &qpdf_engine(), target)
                    .expect("plan");
            let (path, report) = super::execute_mbox_export(
                &probe,
                &plan,
                uuid::Uuid::new_v4(),
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .expect("execute");
            assert!(path.is_file());
            assert_ne!(report.status, crate::domain::ValidationStatus::Fail);
            let text = std::fs::read_to_string(&path).expect("read");
            assert!(text.contains("==== Anole Mail 1/3 ===="));
            assert!(text.contains("==== Anole Mail 3/3 ===="));
            if target == "html" {
                assert!(!text.contains("<script>"), "scripts stripped");
            }
        }
    }

    #[tokio::test]
    async fn remote_resource_mbox_is_policy_blocked() {
        let directory = TempDir::new().expect("tempdir");
        let source = directory.path().join("remote.mbox");
        std::fs::write(
            &source,
            "From a@example.org Thu Sep  3 09:00:00 2026\r
From: a@example.org\r
Subject: tracker\r
Content-Type: text/html\r
\r
<html><body><img src=\"https://tracker.example.org/p.gif\">x</body></html>\r
",
        )
        .expect("write");
        let probe = super::inspect_mbox(&source).await.expect("probe");
        assert_eq!(
            probe.streams[0].properties.get("has_external_resource"),
            Some(&serde_json::json!(true))
        );
        let plan = super::plan_mbox_export(
            &probe,
            directory.path().join("blocked.html"),
            &builtin_engine(),
            &qpdf_engine(),
            "html",
        );
        assert_eq!(
            plan.expect_err("remote resource must block").code,
            crate::ErrorCode::PolicyBlocked
        );
    }

    #[tokio::test]
    async fn mbox_pdf_export_validates_when_engines_exist() {
        let directory = TempDir::new().expect("tempdir");
        let source = directory.path().join("sample.mbox");
        std::fs::write(&source, THREE_MAILS).expect("write mbox");
        let probe = super::inspect_mbox(&source).await.expect("probe");
        // The composite needs the html→pdf lane plus qpdf/poppler; skip when
        // this environment has none (e.g. plain CI runners).
        for engine in ["qpdf", "pdfinfo", "pdftotext"] {
            if crate::doctor::inspect_engine(engine).await.is_err() {
                eprintln!("skipping mbox pdf e2e: engine {engine} missing");
                return;
            }
        }
        let qpdf = crate::doctor::inspect_engine("qpdf").await.expect("qpdf");
        let output = directory.path().join("out.pdf");
        let plan = super::plan_mbox_export(&probe, output.clone(), &builtin_engine(), &qpdf, "pdf")
            .expect("plan");
        let (path, report) = super::execute_mbox_export(
            &probe,
            &plan,
            uuid::Uuid::new_v4(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("execute");
        assert!(path.is_file());
        assert_eq!(report.status, crate::domain::ValidationStatus::Pass);
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.code == "MBOX_PAGE_CONSERVATION"
                    && check.status == crate::domain::ValidationStatus::Pass)
        );
    }
}
