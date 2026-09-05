//! EML（RFC 5322/MIME）邮件输入格式：纯 Rust 解析、安全净化与导出。
//!
//! 邮件正文是不可信输入：HTML body 在导出前必须剥掉 `<script>` 与
//! 远程资源引用（deny-all 网络策略与 SVG raster 同语义），`From/To`
//! 等头经 RFC 2047 解码后进入输出头部块。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use formatwright_engine_sdk::{EngineIdentity, LossClass, Operation};
use mailparse::parse_mail;
use quick_xml::Reader;
use quick_xml::events::Event;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::domain::{
    ArtifactSummary, ChangeSet, NetworkPolicy, Plan, PlanStep, Probe, ReportRedaction,
    SCHEMA_VERSION, ValidationCheck, ValidationReport, ValidationStatus,
};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::planner::deterministic_plan_hash;

/// 内置（进程内）EML 导出引擎标识，与 `formatwright.structured` 同款模式。
pub const EML_ENGINE_ID: &str = "formatwright.eml";

const MAX_EML_BYTES: u64 = 16 * 1024 * 1024;

/// 一封解析后的邮件：解码过的关键头 + 第一个 text/plain 与第一个
/// text/html part（附件一律跳过）。
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedEmail {
    pub from: Option<String>,
    pub to: Option<String>,
    pub subject: Option<String>,
    pub date: Option<String>,
    pub plain_body: Option<String>,
    pub html_body: Option<String>,
}

impl ParsedEmail {
    /// 供文本统计与 txt 导出使用的可见文本：优先纯文本正文，否则剥掉
    /// HTML 标签取文字。
    #[must_use]
    pub fn visible_text(&self) -> String {
        if let Some(plain) = self.plain_body.as_deref() {
            return plain.to_owned();
        }
        self.html_body
            .as_deref()
            .and_then(|html| crate::document::html_text(html).ok())
            .unwrap_or_default()
    }
}

/// 解析一个 EML 文件字节流。
///
/// # Errors
///
/// 输入超限或不符合 RFC 822 结构时返回 `InputInvalid`。
pub fn parse_eml_bytes(bytes: &[u8]) -> Result<ParsedEmail> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_EML_BYTES {
        return Err(FormatWrightError::new(
            ErrorCode::ResourceExhausted,
            Stage::Inspect,
            "EML input exceeds the 16 MiB alpha limit",
            "Use a smaller email export.",
        ));
    }
    let parsed = parse_mail(bytes).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            "The file is not a parseable RFC 822/MIME message",
            "Re-export the email or use a future MBOX/MSG adapter.",
        )
        .with_diagnostic(error.to_string())
    })?;
    Ok(ParsedEmail {
        from: first_header(&parsed, "From"),
        to: first_header(&parsed, "To"),
        subject: first_header(&parsed, "Subject"),
        date: first_header(&parsed, "Date"),
        plain_body: first_body(&parsed, "text/plain"),
        html_body: first_body(&parsed, "text/html"),
    })
}

/// 读取并解析 EML 文件。
///
/// # Errors
///
/// 读取失败或解析失败时返回类型化错误。
pub fn parse_eml_file(path: &Path) -> Result<ParsedEmail> {
    let bytes = fs::read(path).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            "Unable to read the EML file",
            "Confirm the file exists and is readable.",
        )
        .with_diagnostic(error.to_string())
    })?;
    parse_eml_bytes(&bytes)
}

/// `inspect_document` 的 EML 属性入口：关键头 + 文本统计 + 远程资源检测。
///
/// # Errors
///
/// 解析失败时返回类型化错误。
pub(crate) fn inspect_eml_properties(path: &Path) -> Result<BTreeMap<String, Value>> {
    let email = parse_eml_file(path)?;
    let visible = email.visible_text();
    let normalized = crate::document::normalized_tokens(&visible);
    let has_external_resource = email
        .html_body
        .as_deref()
        .is_some_and(contains_remote_reference);
    let mut properties = BTreeMap::new();
    if let Some(value) = &email.from {
        properties.insert("eml_from".to_owned(), json!(value));
    }
    if let Some(value) = &email.to {
        properties.insert("eml_to".to_owned(), json!(value));
    }
    if let Some(value) = &email.subject {
        properties.insert("eml_subject".to_owned(), json!(value));
    }
    if let Some(value) = &email.date {
        properties.insert("eml_date".to_owned(), json!(value));
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
    Ok(properties)
}

fn first_header(part: &mailparse::ParsedMail, key: &str) -> Option<String> {
    part.headers
        .iter()
        .find(|header| header.get_key_ref().eq_ignore_ascii_case(key))
        .map(mailparse::MailHeader::get_value)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// 深度优先取第一个匹配 MIME 类型的正文 part；附件（Content-Disposition
/// 为 attachment）一律跳过。
fn first_body(part: &mailparse::ParsedMail, mimetype: &str) -> Option<String> {
    let disposition = part.get_content_disposition();
    if matches!(
        disposition.disposition,
        mailparse::DispositionType::Attachment
    ) {
        return None;
    }
    if part.ctype.mimetype.eq_ignore_ascii_case(mimetype) {
        return part.get_body().ok();
    }
    part.subparts
        .iter()
        .find_map(|subpart| first_body(subpart, mimetype))
}

/// 检测 HTML body 是否引用远程资源（http/https/协议相对 URL）。
pub(crate) fn contains_remote_reference(html: &str) -> bool {
    let lowered = html.to_ascii_lowercase();
    ["src=\"", "src='", "href=\"", "href='", "src=", "href="]
        .iter()
        .any(|marker| {
            let mut search = 0_usize;
            while let Some(position) = lowered[search..].find(marker) {
                let start = search + position + marker.len();
                let rest = &lowered[start..];
                let rest = rest.trim_start_matches([' ', '\t']);
                if rest.starts_with("http://")
                    || rest.starts_with("https://")
                    || rest.starts_with("//")
                {
                    return true;
                }
                search = start;
            }
            false
        })
}

fn is_remote_url(value: &str) -> bool {
    let trimmed = value
        .trim_start_matches([' ', '\t', '\n', '\r'])
        .to_ascii_lowercase();
    trimmed.starts_with("http://") || trimmed.starts_with("https://") || trimmed.starts_with("//")
}

/// 净化不可信的邮件 HTML：剥除 `<script>...</script>` 整段、移除远程
/// `src`/`href` 引用与 `on*` 事件属性。解析失败时回退为纯转义 `<pre>`。
#[must_use]
pub fn sanitize_html(source: &str) -> String {
    match try_sanitize_html(source) {
        Ok(sanitized) => sanitized,
        Err(_) => format!("<pre>{}</pre>", escape_html(source)),
    }
}

fn try_sanitize_html(source: &str) -> Result<String> {
    let mut reader = Reader::from_str(source);
    reader.config_mut().trim_text(false);
    reader.config_mut().check_end_names = false;
    let mut output = String::new();
    let mut script_depth = 0_usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(tag)) => {
                let name = String::from_utf8_lossy(tag.name().as_ref()).to_ascii_lowercase();
                if name == "script" {
                    if script_depth == 0 {
                        output.push_str("<!-- script removed -->");
                    }
                    script_depth += 1;
                    continue;
                }
                if script_depth > 0 {
                    continue;
                }
                output.push('<');
                output.push_str(&String::from_utf8_lossy(tag.name().as_ref()));
                push_sanitized_attributes(&mut output, tag.attributes());
                output.push('>');
            }
            Ok(Event::Empty(tag)) => {
                if script_depth > 0 {
                    continue;
                }
                let name = String::from_utf8_lossy(tag.name().as_ref()).to_ascii_lowercase();
                if name == "script" {
                    output.push_str("<!-- script removed -->");
                    continue;
                }
                output.push('<');
                output.push_str(&String::from_utf8_lossy(tag.name().as_ref()));
                push_sanitized_attributes(&mut output, tag.attributes());
                output.push_str("/>");
            }
            Ok(Event::End(tag)) => {
                let name = String::from_utf8_lossy(tag.name().as_ref()).to_ascii_lowercase();
                if name == "script" {
                    script_depth = script_depth.saturating_sub(1);
                    continue;
                }
                if script_depth > 0 {
                    continue;
                }
                output.push_str("</");
                output.push_str(&String::from_utf8_lossy(tag.name().as_ref()));
                output.push('>');
            }
            Ok(Event::Text(text)) => {
                if script_depth == 0 {
                    // 源文本本身已转义，原样透传避免二次转义。
                    output.push_str(&String::from_utf8_lossy(&text));
                }
            }
            Ok(Event::CData(content)) if script_depth == 0 => {
                output.push_str("<![CDATA[");
                output.push_str(&String::from_utf8_lossy(&content));
                output.push_str("]]>");
            }
            Ok(Event::Comment(comment)) if script_depth == 0 => {
                output.push_str("<!--");
                output.push_str(&String::from_utf8_lossy(&comment));
                output.push_str("-->");
            }
            Ok(Event::Decl(decl)) if script_depth == 0 => {
                output.push_str("<?");
                output.push_str(&String::from_utf8_lossy(&decl));
                output.push_str("?>");
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => {
                return Err(FormatWrightError::new(
                    ErrorCode::InputInvalid,
                    Stage::Inspect,
                    "The email HTML body is not tokenizable",
                    "The sanitizer falls back to escaped plain text.",
                )
                .with_diagnostic(error.to_string()));
            }
        }
    }
    Ok(output)
}

fn push_sanitized_attributes(
    output: &mut String,
    attributes: quick_xml::events::attributes::Attributes<'_>,
) {
    for attribute in attributes.flatten() {
        let key = String::from_utf8_lossy(attribute.key.as_ref()).to_ascii_lowercase();
        let value = String::from_utf8_lossy(&attribute.value);
        if (key == "src" || key == "href" || key == "xlink:href") && is_remote_url(&value) {
            output.push_str(" data-fw-remote-removed=\"1\"");
            continue;
        }
        if key.starts_with("on") {
            continue;
        }
        output.push(' ');
        output.push_str(&String::from_utf8_lossy(attribute.key.as_ref()));
        output.push('=');
        output.push_str(&value);
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// txt 导出：头块（From/To/Subject/Date）+ 空行 + 正文。
#[must_use]
pub fn render_txt(email: &ParsedEmail) -> String {
    let mut output = String::new();
    for (label, value) in [
        ("From", &email.from),
        ("To", &email.to),
        ("Subject", &email.subject),
        ("Date", &email.date),
    ] {
        if let Some(value) = value {
            output.push_str(label);
            output.push_str(": ");
            output.push_str(value);
            output.push('\n');
        }
    }
    output.push('\n');
    output.push_str(&email.visible_text());
    output
}

/// html 导出：规范化 HTML 文档，头部信息表 + 净化后的 body（无 HTML 时
/// 用 `<pre>` 回退显示纯文本）。
#[must_use]
pub fn render_html(email: &ParsedEmail) -> String {
    let title = email.subject.as_deref().unwrap_or("Email");
    let mut table = String::new();
    for (label, value) in [
        ("From", &email.from),
        ("To", &email.to),
        ("Subject", &email.subject),
        ("Date", &email.date),
    ] {
        if let Some(value) = value {
            table.push_str("<tr><th>");
            table.push_str(label);
            table.push_str("</th><td>");
            table.push_str(&escape_html(value));
            table.push_str("</td></tr>\n");
        }
    }
    let body = if let Some(html) = email.html_body.as_deref() {
        format!("<div>\n{}</div>", sanitize_html(html))
    } else {
        format!("<pre>\n{}</pre>", escape_html(&email.visible_text()))
    };
    format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n\
         </head>\n<body>\n<table border=\"0\">\n{}</table>\n<hr>\n{}\n</body>\n</html>\n",
        escape_html(title),
        table,
        body
    )
}

/// 为 EML 输入构建内置 txt/html 导出 Plan（无外部引擎）。
///
/// # Errors
///
/// 非法输入格式/目标/引擎返回 `Unsupported` 或 `EngineIncompatible`；
/// 远程资源引用触发 deny-all 时返回 `PolicyBlocked`。
pub fn plan_eml_export(
    probe: &Probe,
    output_path: PathBuf,
    engine: &EngineIdentity,
    target: &str,
) -> Result<Plan> {
    let target = target.trim().trim_start_matches('.').to_ascii_lowercase();
    if probe.format.id != "eml" {
        return Err(unsupported("EML export input must be an .eml message"));
    }
    if !matches!(target.as_str(), "txt" | "html") {
        return Err(unsupported("EML export target must be txt or html"));
    }
    if engine.engine_id != EML_ENGINE_ID {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            "The EML export Plan was given the wrong engine",
            "Use the built-in formatwright.eml adapter.",
        ));
    }
    if property(probe, "has_external_resource") == json!(true) {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Plan,
            "The email HTML body references an external resource under deny-all policy",
            "Remove the remote image/link or wait for an explicitly authorized resource-root policy.",
        ));
    }
    let step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: format!("formatwright.eml.eml-to-{target}.builtin"),
        engine: engine.clone(),
        operation: Operation::Transform,
        loss_class: LossClass::Unknown,
        arguments: BTreeMap::from([
            ("source_format".to_owned(), "eml".to_owned()),
            ("target_format".to_owned(), target.clone()),
            ("network".to_owned(), "deny".to_owned()),
            ("sanitize_html".to_owned(), "true".to_owned()),
        ]),
        estimated_temporary_bytes: Some(probe.artifact.size_bytes.saturating_mul(2)),
    };
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: Uuid::new_v4(),
        plan_hash: String::new(),
        input_fingerprint: probe.artifact.fast_fingerprint.clone(),
        target_format: target,
        constraints: BTreeMap::from([
            ("network".to_owned(), json!("deny")),
            ("external_resources".to_owned(), json!("deny")),
            ("scripts".to_owned(), json!("stripped")),
        ]),
        steps: vec![step],
        changes: ChangeSet {
            preserved: vec![
                "decoded From/To/Subject/Date headers".to_owned(),
                "normalized textual content".to_owned(),
            ],
            changed: vec!["the email is re-serialized as a standalone document".to_owned()],
            dropped: vec![
                "attachments and non-text MIME parts".to_owned(),
                "scripts, event handlers, and remote resources".to_owned(),
            ],
            unknown: vec!["visual fidelity of the original HTML".to_owned()],
        },
        validators: vec![
            "document.text-extractable".to_owned(),
            "eml.headers-present".to_owned(),
        ],
        network_policy: NetworkPolicy::Deny,
        output_path: Some(output_path),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

/// 执行内置 EML 导出：纯 Rust 写出 + 重新 inspect + 验收。
///
/// # Errors
///
/// 写出、解析或必检失败时返回类型化错误；必检失败会删除已写输出。
pub async fn execute_eml_export(probe: &Probe, plan: &Plan) -> Result<(PathBuf, ValidationReport)> {
    let output = plan.output_path.clone().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Execute,
            "EML export Plan has no output path",
            "Choose an output path.",
        )
    })?;
    if output.exists() {
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Execute,
            "The EML export destination already exists",
            "Choose another output path and retry.",
        ));
    }
    let email = parse_eml_file(&probe.artifact.canonical_path)?;
    let rendered = match plan.target_format.as_str() {
        "html" => render_html(&email),
        _ => render_txt(&email),
    };
    fs::write(&output, rendered.as_str()).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Execute,
            "Unable to write the EML export output",
            "Check the destination directory and retry.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let output_probe = match crate::document::inspect_document(&output).await {
        Ok(probe) => probe,
        Err(error) => {
            let _ = fs::remove_file(&output);
            return Err(error);
        }
    };
    let report = validate_eml_export_output(probe, &output_probe, plan, Uuid::new_v4(), &rendered);
    if report.status == ValidationStatus::Fail {
        let _ = fs::remove_file(&output);
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "EML export output failed required validation checks",
            "Inspect the validation report and choose another Plan.",
        )
        .with_diagnostic(serde_json::to_string(&report).unwrap_or_default()));
    }
    Ok((output, report))
}

/// EML 导出验收：目标格式、文本非空、From/To 头提取、（html 目标）
/// 无 `<script>`。
#[must_use]
pub fn validate_eml_export_output(
    input: &Probe,
    output: &Probe,
    plan: &Plan,
    job_id: Uuid,
    rendered_output: &str,
) -> ValidationReport {
    let target_format = plan.target_format.clone();
    let expected_output_format = if target_format == "html" {
        "html"
    } else {
        "plain"
    };
    let observed_chars = property(output, "text_characters").as_u64().unwrap_or(0);
    let headers_present =
        property(input, "eml_from").is_string() && property(input, "eml_to").is_string();
    let mut checks = vec![
        validation_check(
            "EML_TARGET_FORMAT",
            status(output.format.id == expected_output_format),
            json!(expected_output_format),
            json!(output.format.id),
            "Detected output format.",
        ),
        validation_check(
            "EXPORT_TEXT_NONEMPTY",
            status(observed_chars > 0),
            json!(">0"),
            json!(observed_chars),
            "Output carries at least one normalized text character.",
        ),
        validation_check(
            "EMLO_FROMTO",
            status(headers_present),
            json!("from+to"),
            json!({
                "from": property(input, "eml_from"),
                "to": property(input, "eml_to"),
            }),
            "From/To headers were decoded from the message.",
        ),
    ];
    if target_format == "html" {
        // 输出 HTML 里不应出现未剥除的 script 标记（安全红线，必检）。
        checks.push(validation_check(
            "EML_HTML_SCRIPT_FREE",
            status(!rendered_output.to_ascii_lowercase().contains("<script")),
            json!("no <script"),
            json!("checked"),
            "The exported HTML contains no <script> element.",
        ));
    }
    let report_status = checks.iter().fold(ValidationStatus::Pass, |state, check| {
        state.worst(check.status)
    });
    ValidationReport {
        schema_version: SCHEMA_VERSION,
        report_id: Uuid::new_v4(),
        job_id,
        plan_hash: plan.plan_hash.clone(),
        status: report_status,
        input: ArtifactSummary {
            display_path: Some(input.artifact.display_path.clone()),
            format_id: input.format.id.clone(),
            size_bytes: input.artifact.size_bytes,
            fast_fingerprint: input.artifact.fast_fingerprint.clone(),
            full_blake3: input.artifact.full_blake3.clone(),
        },
        output: ArtifactSummary {
            display_path: Some(output.artifact.display_path.clone()),
            format_id: output.format.id.clone(),
            size_bytes: output.artifact.size_bytes,
            fast_fingerprint: output.artifact.fast_fingerprint.clone(),
            full_blake3: output.artifact.full_blake3.clone(),
        },
        engines: plan.steps.iter().map(|step| step.engine.clone()).collect(),
        checks,
        intentional_changes: plan.changes.changed.clone(),
        redaction: ReportRedaction {
            paths_redacted: false,
            metadata_values_redacted: true,
        },
    }
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
        evidence: "Anole native EML adapter".to_owned(),
        message: message.to_owned(),
    }
}

fn unsupported(message: &str) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::Unsupported,
        Stage::Plan,
        message,
        "Use an .eml input and a txt or html target.",
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        EML_ENGINE_ID, contains_remote_reference, inspect_eml_properties, parse_eml_bytes,
        plan_eml_export, render_html, render_txt, sanitize_html, validate_eml_export_output,
    };
    use crate::domain::ValidationStatus;
    use uuid::Uuid;

    const SINGLE_PART: &str = "From: Alice <alice@example.com>\r\n\
        To: Bob <bob@example.org>\r\n\
        Subject: Hello\r\n\
        Date: Mon, 01 Sep 2026 10:00:00 +0800\r\n\
        Content-Type: text/plain; charset=utf-8\r\n\
        \r\n\
        ELECTRIC 440 body text\r\nsecond line\r\n";

    const MULTIPART_CHINESE: &str = "From: =?utf-8?B?5rWL6K+V5ZGY?= <zhang@example.cn>\r\n\
        To: li@example.cn\r\n\
        Subject: =?utf-8?B?6L+Z5piv5LiA5bCB5rWL6K+V6YKu5Lu2?=\r\n\
        Date: Tue, 02 Sep 2026 09:30:00 +0800\r\n\
        MIME-Version: 1.0\r\n\
        Content-Type: multipart/mixed; boundary=\"BOUND\"\r\n\
        \r\n\
        --BOUND\r\n\
        Content-Type: text/plain; charset=utf-8\r\n\
        \r\n\
        这是纯文本部分 electric 440\r\n\
        --BOUND\r\n\
        Content-Type: text/html; charset=utf-8\r\n\
        Content-Transfer-Encoding: base64\r\n\
        \r\n\
        PGh0bWw+PGJvZHk+PHA+6YKu5Lu25q2j5paH6L2s5o2iPC9wPjwvYm9keT48L2h0bWw+\r\n\
        --BOUND\r\n\
        Content-Type: application/pdf; name=report.pdf\r\n\
        Content-Disposition: attachment; filename=report.pdf\r\n\
        Content-Transfer-Encoding: base64\r\n\
        \r\n\
        JVBERi0xLjQ=\r\n\
        --BOUND--\r\n";

    #[test]
    fn parses_single_part_english_mail() {
        let email = parse_eml_bytes(SINGLE_PART.as_bytes()).expect("single-part parse");
        assert_eq!(email.from.as_deref(), Some("Alice <alice@example.com>"));
        assert_eq!(email.subject.as_deref(), Some("Hello"));
        assert_eq!(
            email.date.as_deref(),
            Some("Mon, 01 Sep 2026 10:00:00 +0800")
        );
        assert!(
            email
                .plain_body
                .as_deref()
                .is_some_and(|body| body.contains("ELECTRIC 440"))
        );
        assert!(email.html_body.is_none());
    }

    #[test]
    fn parses_multipart_chinese_mail_and_skips_attachments() {
        let email = parse_eml_bytes(MULTIPART_CHINESE.as_bytes()).expect("multipart parse");
        // RFC 2047 解码后的中文头。
        assert_eq!(email.subject.as_deref(), Some("这是一封测试邮件"));
        assert!(
            email
                .from
                .as_deref()
                .is_some_and(|value| value.contains("测试员"))
        );
        assert!(
            email
                .plain_body
                .as_deref()
                .is_some_and(|body| body.contains("这是纯文本部分"))
        );
        // base64 解码后的 HTML body（包含"邮件正文转换"）。
        assert!(
            email
                .html_body
                .as_deref()
                .is_some_and(|body| body.contains("邮件正文转换"))
        );
        // 附件不能进入任何正文。
        assert!(!email.visible_text().contains("JVBERi"));
    }

    #[test]
    fn sanitizer_strips_scripts_and_remote_references() {
        let hostile = "<div onclick=\"evil()\">text<img src=\"https://evil.example/x.png\">\
                       <a href=\"http://tracker.example/c\">link</a> \
                       <a href=\"/local.html\">local</a>\
                       <script>alert(1)</script>tail</div>";
        let clean = sanitize_html(hostile);
        let lowered = clean.to_ascii_lowercase();
        assert!(!lowered.contains("<script"), "script element is removed");
        assert!(!lowered.contains("onclick"), "event handlers are removed");
        assert!(
            !lowered.contains("https://evil.example"),
            "remote src is removed"
        );
        assert!(
            !lowered.contains("http://tracker.example"),
            "remote href is removed"
        );
        assert!(clean.contains("local.html"), "relative links survive");
        assert!(
            clean.contains("text") && clean.contains("tail"),
            "text survives"
        );
    }

    #[test]
    fn remote_reference_detection_matches_http_sources() {
        assert!(contains_remote_reference(
            "<img src=\"https://x.example/a.png\">"
        ));
        assert!(contains_remote_reference(
            "<a href='http://x.example/'>x</a>"
        ));
        assert!(contains_remote_reference(
            "<img src=\"//cdn.example/a.png\">"
        ));
        assert!(!contains_remote_reference("<img src=\"cid:inline.png\">"));
        assert!(!contains_remote_reference("<a href=\"page.html\">x</a>"));
        assert!(!contains_remote_reference("plain text with no markup"));
    }

    #[test]
    fn txt_export_renders_header_block_then_body() {
        let email = parse_eml_bytes(SINGLE_PART.as_bytes()).expect("parse");
        let text = render_txt(&email);
        assert!(text.starts_with("From: Alice <alice@example.com>\n"));
        assert!(text.contains("To: Bob <bob@example.org>\n"));
        assert!(text.contains("Subject: Hello\n"));
        assert!(text.contains("\n\nELECTRIC 440 body text"));
    }

    #[test]
    fn html_export_is_a_normalized_document_with_pre_fallback() {
        let email = parse_eml_bytes(SINGLE_PART.as_bytes()).expect("parse");
        let html = render_html(&email);
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<title>Hello</title>"));
        assert!(html.contains("<th>From</th>"));
        assert!(
            html.contains("<pre>"),
            "plain-only mail falls back to <pre>"
        );
        let lowered = html.to_ascii_lowercase();
        assert!(!lowered.contains("<script"));
    }

    #[test]
    fn html_export_sanitizes_the_html_body() {
        let source = "From: a@b.c\r\nTo: d@e.f\r\nSubject: s\r\n\
             Content-Type: text/html; charset=utf-8\r\n\r\n\
             <p>hi</p><script>steal()</script><img src=\"http://x.example/i.png\">";
        let email = parse_eml_bytes(source.as_bytes()).expect("parse");
        let html = render_html(&email);
        let lowered = html.to_ascii_lowercase();
        assert!(!lowered.contains("<script"));
        assert!(!lowered.contains("x.example"));
        assert!(lowered.contains("<p>hi</p>"));
    }

    #[tokio::test]
    async fn inspect_eml_properties_exposes_headers_and_text_stats() {
        let directory = tempdir().expect("tempdir");
        let input = directory.path().join("mail.eml");
        fs::write(&input, MULTIPART_CHINESE).expect("write eml");
        let properties = inspect_eml_properties(&input).expect("inspect");
        assert_eq!(
            properties
                .get("eml_subject")
                .and_then(|value| value.as_str()),
            Some("这是一封测试邮件")
        );
        assert!(
            properties
                .get("text_characters")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|count| count > 0)
        );
        assert_eq!(
            properties.get("has_external_resource"),
            Some(&serde_json::json!(false))
        );
    }

    fn builtin_engine() -> formatwright_engine_sdk::EngineIdentity {
        formatwright_engine_sdk::EngineIdentity {
            engine_id: EML_ENGINE_ID.to_owned(),
            version: "0.1.0".to_owned(),
            binary_path: std::path::PathBuf::from("formatwright.exe"),
            binary_sha256: "0".repeat(64),
            manifest_sha256: None,
            build_configuration: None,
            certification: formatwright_engine_sdk::Certification::Experimental,
        }
    }

    #[tokio::test]
    async fn plan_eml_export_builds_builtin_plans_and_enforces_policy() {
        let directory = tempdir().expect("tempdir");
        let input = directory.path().join("mail.eml");
        fs::write(&input, SINGLE_PART).expect("write eml");
        let probe = crate::document::inspect_document(&input)
            .await
            .expect("eml inspection");
        assert_eq!(probe.format.id, "eml");
        let engine = builtin_engine();
        for target in ["txt", "html"] {
            let plan = plan_eml_export(
                &probe,
                directory.path().join(format!("out.{target}")),
                &engine,
                target,
            )
            .unwrap_or_else(|error| panic!("{target} plan: {error}"));
            assert_eq!(plan.target_format, target);
            assert_eq!(plan.steps[0].engine.engine_id, EML_ENGINE_ID);
            assert!(!plan.plan_hash.is_empty());
        }
        assert!(
            plan_eml_export(&probe, directory.path().join("o.pdf"), &engine, "pdf").is_err(),
            "pdf direct export is rejected (browser lane handles it)"
        );
        let mut wrong = engine.clone();
        wrong.engine_id = "pandoc".to_owned();
        assert!(plan_eml_export(&probe, directory.path().join("o.txt"), &wrong, "txt").is_err());

        // 远程资源 → deny-all PolicyBlocked（与 SVG raster 同语义）。
        let hostile = "From: a@b.c\r\nTo: d@e.f\r\nSubject: s\r\n\
             Content-Type: text/html\r\n\r\n\
             <p>hello</p><img src=\"https://tracker.example/px.gif\">";
        let hostile_path = directory.path().join("hostile.eml");
        fs::write(&hostile_path, hostile).expect("write hostile eml");
        let hostile_probe = crate::document::inspect_document(&hostile_path)
            .await
            .expect("hostile inspection");
        assert_eq!(
            hostile_probe.streams[0]
                .properties
                .get("has_external_resource"),
            Some(&serde_json::json!(true))
        );
        let blocked = plan_eml_export(
            &hostile_probe,
            directory.path().join("h.txt"),
            &engine,
            "txt",
        )
        .expect_err("remote resources are denied");
        assert_eq!(blocked.code, crate::ErrorCode::PolicyBlocked);
    }

    #[tokio::test]
    async fn validate_eml_export_requires_text_and_fromto_headers() {
        let directory = tempdir().expect("tempdir");
        let input = directory.path().join("mail.eml");
        fs::write(&input, SINGLE_PART).expect("write eml");
        let probe = crate::document::inspect_document(&input)
            .await
            .expect("eml inspection");
        let engine = builtin_engine();
        let output = directory.path().join("out.txt");
        let plan = plan_eml_export(&probe, output.clone(), &engine, "txt").expect("txt plan");

        // 空输出 → EXPORT_TEXT_NONEMPTY Fail。
        fs::write(&output, "").expect("empty output");
        let empty_probe = crate::document::inspect_document(&output)
            .await
            .expect("empty inspection");
        let report = validate_eml_export_output(&probe, &empty_probe, &plan, Uuid::new_v4(), "");
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.code == "EXPORT_TEXT_NONEMPTY"
                    && check.status == ValidationStatus::Fail)
        );

        // 正常输出 → 全部 Pass（含 EMLO_FROMTO）。
        fs::write(&output, "From: x\r\n\r\nELECTRIC 440").expect("text output");
        let text_probe = crate::document::inspect_document(&output)
            .await
            .expect("txt inspection");
        let report =
            validate_eml_export_output(&probe, &text_probe, &plan, Uuid::new_v4(), "sample text");
        assert_eq!(report.status, ValidationStatus::Pass);
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.code == "EMLO_FROMTO" && check.status == ValidationStatus::Pass)
        );

        // html 目标 → EML_HTML_SCRIPT_FREE 必检存在。
        let html_plan = plan_eml_export(&probe, directory.path().join("o.html"), &engine, "html")
            .expect("html plan");
        let html = render_html(&parse_eml_bytes(SINGLE_PART.as_bytes()).expect("parse"));
        fs::write(directory.path().join("o.html"), &html).expect("html output");
        let html_probe = crate::document::inspect_document(directory.path().join("o.html"))
            .await
            .expect("html inspection");
        let html_report =
            validate_eml_export_output(&probe, &html_probe, &html_plan, Uuid::new_v4(), &html);
        assert_eq!(html_report.status, ValidationStatus::Pass);
        assert!(
            html_report
                .checks
                .iter()
                .any(|check| check.code == "EML_HTML_SCRIPT_FREE")
        );
    }

    #[tokio::test]
    async fn execute_eml_export_writes_and_validates_txt_and_html() {
        let directory = tempdir().expect("tempdir");
        let engine = builtin_engine();
        for (name, target) in [("english", "txt"), ("chinese", "html")] {
            let source = if name == "english" {
                SINGLE_PART
            } else {
                MULTIPART_CHINESE
            };
            let input = directory.path().join(format!("{name}.eml"));
            fs::write(&input, source).expect("write eml");
            let probe = crate::document::inspect_document(&input)
                .await
                .expect("eml inspection");
            let output = directory.path().join(format!("{name}.{target}"));
            let plan = plan_eml_export(&probe, output.clone(), &engine, target)
                .unwrap_or_else(|error| panic!("{target} plan: {error}"));
            let (path, report) = super::execute_eml_export(&probe, &plan)
                .await
                .expect("execute");
            assert_eq!(path, output);
            assert_eq!(report.status, ValidationStatus::Pass);
        }
    }
}
