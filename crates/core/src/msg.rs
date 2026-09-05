//! MSG (Outlook) 输入：CFB 复合文档解析 + MSG→EML 合成，随后复用 EML
//! 导出管线（净化、渲染、验收）。内置 `formatwright.msg` 引擎，无外部
//! 进程；无法安全解析的输入一律 fail-closed。

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use formatwright_engine_sdk::{EngineIdentity, LossClass, Operation};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::domain::{
    ChangeSet, FormatDescriptor, FormatKind, NetworkPolicy, Plan, PlanStep, Probe, ProbeEvidence,
    SCHEMA_VERSION, StreamKind, StreamProbe,
};
use crate::eml::{self, ParsedEmail};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::fingerprint::identify_artifact;
use crate::planner::deterministic_plan_hash;

pub const MSG_ENGINE_ID: &str = "formatwright.msg";

const MAX_MSG_BYTES: u64 = 64 * 1024 * 1024;

/// Reads an Outlook `.msg` file and synthesizes an RFC822 EML from the root
/// message properties (transport headers, subject, sender, submit time, and
/// the plain/HTML body). Attachments and recipient tables are intentionally
/// not carried over.
///
/// # Errors
///
/// Returns `InputInvalid` when the file is not a readable CFB container with
/// a recognizable message payload (fail-closed), and storage errors when the
/// file cannot be opened.
pub fn msg_to_eml_bytes(path: &Path) -> Result<Vec<u8>> {
    if let Ok(metadata) = std::fs::metadata(path)
        && metadata.len() > MAX_MSG_BYTES
    {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            "MSG input exceeds the 64 MiB built-in adapter limit",
            "Export a smaller message or strip its attachments first.",
        ));
    }
    let mut compound = cfb::open(path).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            "The file is not a readable Outlook MSG (compound document)",
            "Choose a message exported by Outlook as .msg.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let subject = read_string_property(&mut compound, 0x0037);
    let sender_name = read_string_property(&mut compound, 0x0C1A);
    let transport_headers = read_string_property(&mut compound, 0x007D);
    let plain_body = read_string_property(&mut compound, 0x1000);
    let html_body = read_binary_property(&mut compound, 0x1013)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let submit_time = read_binary_property(&mut compound, 0x0039)
        .filter(|bytes| bytes.len() == 8)
        .and_then(|bytes| {
            let mut little_endian = [0_u8; 8];
            little_endian.copy_from_slice(&bytes);
            filetime_to_rfc2822(u64::from_le_bytes(little_endian))
        });
    if transport_headers.is_none() && plain_body.is_none() && html_body.is_none() {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            "The MSG container carries no message headers or body",
            "Choose a complete Outlook message export.",
        ));
    }

    // The synthesized EML is single-part; drop any body-describing headers
    // from the transport block so the attached body is the only truth.
    let mut header_lines: Vec<String> = transport_headers
        .as_deref()
        .unwrap_or_default()
        .lines()
        .filter(|line| {
            let lowered = line.trim().to_ascii_lowercase();
            !(lowered.starts_with("content-type:")
                || lowered.starts_with("content-transfer-encoding:")
                || lowered.starts_with("mime-version:")
                || lowered.starts_with("content-disposition:"))
        })
        .map(str::to_owned)
        .collect();
    if !header_lines
        .iter()
        .any(|line| line.to_ascii_lowercase().starts_with("subject:"))
    {
        header_lines.push(format!("Subject: {}", subject.unwrap_or_default()));
    }
    if !header_lines
        .iter()
        .any(|line| line.to_ascii_lowercase().starts_with("from:"))
    {
        header_lines.push(format!(
            "From: {}",
            sender_name
                .filter(|value| !value.is_empty())
                .unwrap_or_default()
        ));
    }
    if !header_lines
        .iter()
        .any(|line| line.to_ascii_lowercase().starts_with("date:"))
        && let Some(date) = &submit_time
    {
        header_lines.push(format!("Date: {date}"));
    }
    let (body, content_type) = if let Some(html) = html_body.as_deref() {
        (html.to_owned(), "text/html; charset=utf-8")
    } else {
        (
            plain_body.clone().unwrap_or_default(),
            "text/plain; charset=utf-8",
        )
    };
    header_lines.push("MIME-Version: 1.0".to_owned());
    header_lines.push(format!("Content-Type: {content_type}"));
    header_lines.push("Content-Transfer-Encoding: 8bit".to_owned());
    let mut eml = header_lines.join("\r\n");
    eml.push_str("\r\n\r\n");
    eml.push_str(&body);
    eml.push_str("\r\n");
    Ok(eml.into_bytes())
}

/// Parses a MSG file into the shared [`ParsedEmail`] shape by round-tripping
/// the synthesized EML through the EML parser (RFC 2047 decoding included).
///
/// # Errors
///
/// Returns the MSG or EML parse error.
pub fn parse_msg_file(path: &Path) -> Result<ParsedEmail> {
    let bytes = msg_to_eml_bytes(path)?;
    eml::parse_eml_bytes(&bytes)
}

/// Builds the MSG Probe carrying the same property keys as the EML lane (on
/// the first stream, where the EML planner reads them).
///
/// # Errors
///
/// Returns the artifact/parse errors.
pub async fn inspect_msg(path: &Path) -> Result<Probe> {
    let artifact = identify_artifact(path).await?;
    let email = parse_msg_file(path)?;
    Ok(Probe {
        schema_version: SCHEMA_VERSION,
        artifact,
        format: FormatDescriptor {
            id: "msg".to_owned(),
            kind: FormatKind::Document,
            mime_type: Some("application/vnd.ms-outlook".to_owned()),
            container: Some("cfb".to_owned()),
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
            properties: msg_properties(&email),
        }],
        metadata: BTreeMap::new(),
        warnings: Vec::new(),
        evidence: ProbeEvidence {
            engine_id: MSG_ENGINE_ID.to_owned(),
            engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            engine_binary_sha256: None,
        },
        duration_seconds: None,
        bit_rate: None,
    })
}

fn msg_properties(email: &ParsedEmail) -> BTreeMap<String, Value> {
    let visible = email.visible_text();
    let normalized = crate::document::normalized_tokens(&visible);
    let has_external_resource = email
        .html_body
        .as_deref()
        .is_some_and(crate::eml::contains_remote_reference);
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

/// Builds the export Plan for `msg -> txt|html` through the built-in
/// `formatwright.msg` adapter (mirror of the EML export plan).
///
/// # Errors
///
/// Returns `Unsupported`/`EngineIncompatible` for wrong input, target, or
/// engine, and `PolicyBlocked` for remote-resource references under deny-all.
pub fn plan_msg_export(
    probe: &Probe,
    output_path: PathBuf,
    engine: &EngineIdentity,
    target: &str,
) -> Result<Plan> {
    let target = target.trim().trim_start_matches('.').to_ascii_lowercase();
    if probe.format.id != "msg" {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            "MSG export input must be an Outlook .msg message",
            "Choose a message exported by Outlook as .msg.",
        ));
    }
    if !matches!(target.as_str(), "txt" | "html") {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            "MSG export target must be txt or html",
            "Choose txt or html; other targets compose through chains.",
        ));
    }
    if engine.engine_id != MSG_ENGINE_ID {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            "The MSG export Plan was given the wrong engine",
            "Use the built-in formatwright.msg adapter.",
        ));
    }
    if stream_property(probe, "has_external_resource") == json!(true) {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Plan,
            "The MSG HTML body references an external resource under deny-all policy",
            "Remove the remote image/link or wait for an explicitly authorized resource-root policy.",
        ));
    }
    let step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: format!("formatwright.msg.msg-to-{target}.builtin"),
        engine: engine.clone(),
        operation: Operation::Transform,
        loss_class: LossClass::Unknown,
        arguments: BTreeMap::from([
            ("source_format".to_owned(), "msg".to_owned()),
            ("target_format".to_owned(), target.clone()),
            ("intermediate".to_owned(), "rfc822-eml".to_owned()),
            ("network".to_owned(), "deny".to_owned()),
            ("sanitize_html".to_owned(), "true".to_owned()),
            ("attachments".to_owned(), "drop".to_owned()),
        ]),
        estimated_temporary_bytes: Some(probe.artifact.size_bytes.saturating_mul(2)),
    };
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
            ("attachments".to_owned(), json!("drop")),
        ]),
        steps: vec![step],
        changes: ChangeSet {
            preserved: vec![
                "decoded From/Subject/Date headers".to_owned(),
                "normalized textual content".to_owned(),
            ],
            changed: vec!["the Outlook message renders as a standalone document".to_owned()],
            dropped: vec![
                "Outlook-specific properties, recipient tables, and attachments".to_owned(),
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

/// Executes the MSG export: CFB→EML→shared EML renderers, then the EML
/// output validator. HTML is sanitized exactly like untrusted EML HTML.
///
/// # Errors
///
/// Returns parse/write/validation errors; the output file is removed when
/// validation fails.
pub async fn execute_msg_export(
    probe: &Probe,
    plan: &Plan,
) -> Result<(PathBuf, crate::domain::ValidationReport)> {
    let output = plan.output_path.clone().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Execute,
            "MSG export Plan has no output path",
            "Choose an output path.",
        )
    })?;
    if output.exists() {
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Execute,
            "The MSG export destination already exists",
            "Choose another output path and retry.",
        ));
    }
    let email = parse_msg_file(&probe.artifact.canonical_path)?;
    let rendered = match plan.target_format.as_str() {
        "html" => eml::render_html(&email),
        _ => eml::render_txt(&email),
    };
    let write_output = output.clone();
    let rendered_for_write = rendered.clone();
    tokio::task::spawn_blocking(move || std::fs::write(&write_output, rendered_for_write))
        .await
        .map_err(|error| {
            FormatWrightError::new(
                ErrorCode::Internal,
                Stage::Execute,
                "MSG export writer task failed",
                "Retry the conversion.",
            )
            .with_diagnostic(error.to_string())
        })?
        .map_err(|error| {
            FormatWrightError::new(
                ErrorCode::ExecutionFailed,
                Stage::Execute,
                "Unable to write the MSG export output",
                "Check the destination directory and retry.",
            )
            .with_diagnostic(error.to_string())
        })?;
    let output_probe = match crate::document::inspect_document(&output).await {
        Ok(probe) => probe,
        Err(error) => {
            let _ = std::fs::remove_file(&output);
            return Err(error);
        }
    };
    let report =
        eml::validate_eml_export_output(probe, &output_probe, plan, Uuid::new_v4(), &rendered);
    if report.status == crate::domain::ValidationStatus::Fail {
        let _ = std::fs::remove_file(&output);
    }
    Ok((output, report))
}

fn read_stream(compound: &mut cfb::CompoundFile<std::fs::File>, name: &str) -> Option<Vec<u8>> {
    let mut stream = compound.open_stream(format!("/{name}")).ok()?;
    let mut buffer = Vec::new();
    stream.read_to_end(&mut buffer).ok()?;
    Some(buffer)
}

fn decode_property_string(bytes: &[u8], wide: bool) -> Option<String> {
    if wide {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .take_while(|unit| *unit != 0)
            .collect();
        String::from_utf16(&units).ok()
    } else {
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
}

/// Reads a `PtypString` root property (`__substg1.0_<tag>001F`), falling
/// back to the legacy `001E` encoding.
fn read_string_property(
    compound: &mut cfb::CompoundFile<std::fs::File>,
    tag: u16,
) -> Option<String> {
    read_stream(compound, &format!("__substg1.0_{tag:04X}001F"))
        .and_then(|bytes| decode_property_string(&bytes, true))
        .or_else(|| {
            read_stream(compound, &format!("__substg1.0_{tag:04X}001E"))
                .and_then(|bytes| decode_property_string(&bytes, false))
        })
}

/// Reads a `PtypBinary` root property (`__substg1.0_<tag>0102`).
fn read_binary_property(
    compound: &mut cfb::CompoundFile<std::fs::File>,
    tag: u16,
) -> Option<Vec<u8>> {
    read_stream(compound, &format!("__substg1.0_{tag:04X}0102"))
}

/// Converts a Windows FILETIME (100 ns units since 1601-01-01 UTC) into an
/// RFC 2822 date string; `None` when the value overflows.
fn filetime_to_rfc2822(filetime: u64) -> Option<String> {
    const EPOCH_DELTA: u64 = 11_644_473_600;
    let seconds = filetime.checked_div(10_000_000)?.checked_sub(EPOCH_DELTA)?;
    let days = i64::try_from(seconds / 86_400).ok()?;
    let time = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let weekdays = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let weekday = weekdays[usize::try_from(days.rem_euclid(7)).ok()?];
    Some(format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} +0000",
        weekday,
        day,
        months[(month - 1) as usize],
        year,
        time / 3_600,
        (time % 3_600) / 60,
        time % 60
    ))
}

/// Howard Hinnant's `civil_from_days` (days since 1970-01-01 → y/m/d).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::Path;

    use tempfile::TempDir;

    use super::{MSG_ENGINE_ID, msg_to_eml_bytes, parse_msg_file};

    fn write_utf16le(value: &str) -> Vec<u8> {
        value
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    fn builtin_engine() -> formatwright_engine_sdk::EngineIdentity {
        formatwright_engine_sdk::EngineIdentity {
            engine_id: MSG_ENGINE_ID.to_owned(),
            version: "0.1.0".to_owned(),
            binary_path: std::path::PathBuf::from("formatwright.exe"),
            binary_sha256: "0".repeat(64),
            manifest_sha256: None,
            build_configuration: None,
            certification: formatwright_engine_sdk::Certification::Experimental,
        }
    }

    /// Synthesizes a minimal but structurally faithful .msg via the cfb
    /// writer: the root property streams Outlook defines.
    fn synthetic_msg(path: &Path, html_body: Option<&[u8]>) {
        let mut compound = cfb::create(path).expect("create cfb");
        let put = |compound: &mut cfb::CompoundFile<std::fs::File>, name: &str, bytes: &[u8]| {
            let mut stream = compound.create_stream(format!("/{name}")).expect("stream");
            stream.write_all(bytes).expect("write stream");
            stream.flush().expect("flush stream");
        };
        put(
            &mut compound,
            "__substg1.0_0037001F",
            &write_utf16le("MSG Subject 440010147700"),
        );
        put(
            &mut compound,
            "__substg1.0_0C1A001F",
            &write_utf16le("Alice Sender"),
        );
        put(
            &mut compound,
            "__substg1.0_007D001F",
            &write_utf16le(
                "From: Alice <alice@example.org>\r\nTo: bob@example.org\r\nSubject: MSG Subject 440010147700\r\nContent-Type: multipart/mixed; boundary=ignored\r\n",
            ),
        );
        put(
            &mut compound,
            "__substg1.0_1000001F",
            &write_utf16le("MSG plain body ELECTRIC 998877."),
        );
        if let Some(html) = html_body {
            put(&mut compound, "__substg1.0_10130102", html);
        }
        drop(compound);
    }

    #[test]
    fn msg_synthesizes_a_single_part_eml() {
        let directory = TempDir::new().expect("tempdir");
        let path = directory.path().join("sample.msg");
        synthetic_msg(
            &path,
            Some(b"<html><body><p>MSG html body 440010147700</p></body></html>"),
        );
        let eml = msg_to_eml_bytes(&path).expect("eml bytes");
        let text = String::from_utf8(eml).expect("utf8 eml");
        assert!(text.contains("Subject: MSG Subject 440010147700"));
        // The transport header's multipart Content-Type must be dropped for
        // the single-part synthesis.
        assert!(!text.contains("multipart/mixed"));
        assert!(text.contains("Content-Type: text/html"));
        assert!(text.contains("MSG html body 440010147700"));

        let email = parse_msg_file(&path).expect("parsed email");
        assert_eq!(email.subject.as_deref(), Some("MSG Subject 440010147700"));
        assert_eq!(email.from.as_deref(), Some("Alice <alice@example.org>"));
        assert!(
            email
                .html_body
                .as_deref()
                .is_some_and(|body| body.contains("MSG html body 440010147700"))
        );
    }

    #[test]
    fn plain_only_msg_selects_text_plain() {
        let directory = TempDir::new().expect("tempdir");
        let path = directory.path().join("plain.msg");
        synthetic_msg(&path, None);
        let text = String::from_utf8(msg_to_eml_bytes(&path).expect("eml bytes")).expect("utf8");
        assert!(text.contains("Content-Type: text/plain"));
        assert!(text.contains("MSG plain body ELECTRIC 998877."));
    }

    #[test]
    fn non_cfb_input_fails_closed() {
        let directory = TempDir::new().expect("tempdir");
        let path = directory.path().join("bogus.msg");
        std::fs::write(&path, b"definitely not a compound file").expect("write");
        let error = msg_to_eml_bytes(&path).expect_err("must fail closed");
        assert_eq!(error.code, crate::ErrorCode::InputInvalid);
    }

    #[test]
    fn filetime_formats_as_rfc2822() {
        // 2026-09-04 12:00:00 UTC = FILETIME 134329968000000000.
        assert_eq!(
            super::filetime_to_rfc2822(134_329_968_000_000_000).as_deref(),
            Some("Fri, 04 Sep 2026 12:00:00 +0000")
        );
    }

    #[tokio::test]
    async fn msg_export_executes_and_validates_txt_and_html() {
        let directory = TempDir::new().expect("tempdir");
        let source = directory.path().join("sample.msg");
        synthetic_msg(
            &source,
            Some(b"<html><body><p>MSG html body 440010147700</p><script>alert(1)</script></body></html>"),
        );
        let probe = super::inspect_msg(&source).await.expect("msg probe");
        assert_eq!(probe.format.id, "msg");
        assert_eq!(
            probe.streams[0].properties.get("has_external_resource"),
            Some(&serde_json::json!(false))
        );
        let engine = builtin_engine();
        for target in ["txt", "html"] {
            let output = directory.path().join(format!("out.{target}"));
            let plan = super::plan_msg_export(&probe, output.clone(), &engine, target)
                .expect("msg export plan");
            assert_eq!(plan.steps[0].engine.engine_id, MSG_ENGINE_ID);
            let (path, report) = super::execute_msg_export(&probe, &plan)
                .await
                .expect("msg export executes");
            assert!(path.is_file());
            assert_ne!(
                report.status,
                crate::domain::ValidationStatus::Fail,
                "msg -> {target} validates"
            );
            let text = std::fs::read_to_string(&path).expect("output text");
            assert!(
                text.contains("440010147700"),
                "target {target} keeps tokens"
            );
            if target == "html" {
                assert!(!text.contains("<script>"), "scripts are stripped");
            }
        }
    }

    #[tokio::test]
    async fn remote_html_resource_is_policy_blocked() {
        let directory = TempDir::new().expect("tempdir");
        let path = directory.path().join("remote.msg");
        synthetic_msg(
            &path,
            Some(b"<html><body><img src=\"https://tracker.example.org/p.gif\">body 123</body></html>"),
        );
        let probe = super::inspect_msg(&path).await.expect("probe");
        assert_eq!(
            probe.streams[0].properties.get("has_external_resource"),
            Some(&serde_json::json!(true))
        );
        let engine = builtin_engine();
        let plan = super::plan_msg_export(
            &probe,
            directory.path().join("blocked.html"),
            &engine,
            "html",
        );
        assert_eq!(
            plan.expect_err("remote resource must block").code,
            crate::ErrorCode::PolicyBlocked
        );
    }
}
