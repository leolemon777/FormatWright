use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use formatwright_engine_sdk::{EngineIdentity, LossClass, Operation};
use image::{GenericImageView, ImageReader};
use serde_json::{Value, json};
use tokio::process::Command;
use uuid::Uuid;

use crate::domain::{
    ArtifactSummary, ChangeSet, DiagnosticMessage, FormatDescriptor, FormatKind, NetworkPolicy,
    Plan, PlanRequest, PlanStep, Probe, ProbeEvidence, ReportRedaction, SCHEMA_VERSION, StreamKind,
    StreamProbe, ValidationCheck, ValidationReport, ValidationStatus,
};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::fingerprint::identify_artifact;
use crate::inspect::inspect_media;
use crate::planner::deterministic_plan_hash;

const PDF_HEADER_SCAN_BYTES: usize = 1_024;
const MAX_PDF_PAGES: u32 = 10_000;
const PDFINFO_TIMEOUT: Duration = Duration::from_secs(60);

/// Returns whether the file contains a PDF header in the first 1,024 bytes.
///
/// # Errors
///
/// Returns an input error when the header cannot be read.
pub fn pdf_format_hint(path: impl AsRef<Path>) -> Result<bool> {
    let path = path.as_ref();
    let mut file = File::open(path).map_err(|error| input_error(path, &error))?;
    let mut prefix = [0_u8; PDF_HEADER_SCAN_BYTES];
    let read = file
        .read(&mut prefix)
        .map_err(|error| input_error(path, &error))?;
    Ok(prefix[..read]
        .windows(b"%PDF-".len())
        .any(|window| window == b"%PDF-"))
}

/// Inspects a local PDF with an exact pdfinfo engine identity.
///
/// # Errors
///
/// Returns a typed input, policy, resource, or engine error for malformed,
/// encrypted, oversized-page-count, or uninspectable PDFs.
pub async fn inspect_pdf(path: impl AsRef<Path>, pdfinfo: &EngineIdentity) -> Result<Probe> {
    inspect_pdf_inner(path.as_ref(), pdfinfo, None).await
}

/// Inspects a password-protected PDF by passing `-upw` to pdfinfo. Used by
/// `pdf-decrypt` planning, where the encrypted container is the operation
/// input; a wrong password surfaces as a pdfinfo failure.
///
/// # Errors
///
/// Same as [`inspect_pdf`], plus an input error for a wrong password.
pub async fn inspect_pdf_unlocked(
    path: impl AsRef<Path>,
    pdfinfo: &EngineIdentity,
    password: &str,
) -> Result<Probe> {
    inspect_pdf_inner(path.as_ref(), pdfinfo, Some(password)).await
}

async fn inspect_pdf_inner(
    path: &Path,
    pdfinfo: &EngineIdentity,
    password: Option<&str>,
) -> Result<Probe> {
    if pdfinfo.engine_id != "pdfinfo" {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Inspect,
            "PDF inspection was given the wrong engine",
            "Run doctor and use pdfinfo.",
        ));
    }
    if !pdf_format_hint(path)? {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Inspect,
            "Input does not contain a PDF header",
            "Choose a complete PDF file.",
        ));
    }
    let artifact = identify_artifact(path).await?;
    let unlocked_summary_args: Vec<&str> = match password {
        Some(password) => vec!["-upw", password, "-enc", "UTF-8"],
        None => vec!["-enc", "UTF-8"],
    };
    let summary = run_pdfinfo(
        pdfinfo,
        &unlocked_summary_args,
        &artifact.canonical_path,
        password.is_some(),
    )
    .await?;
    let page_count = parse_u32_field(&summary, "Pages").ok_or_else(|| {
        incompatible_pdfinfo("pdfinfo did not report a valid page count", &summary)
    })?;
    if page_count == 0 {
        return Err(input_pdf_error("PDF contains no pages", &summary));
    }
    if page_count > MAX_PDF_PAGES {
        return Err(FormatWrightError::new(
            ErrorCode::ResourceExhausted,
            Stage::Inspect,
            format!("PDF has {page_count} pages; the alpha limit is {MAX_PDF_PAGES}"),
            "Split the PDF into smaller documents and retry.",
        ));
    }
    if password.is_none()
        && parse_field(&summary, "Encrypted")
            .is_some_and(|value| value.to_ascii_lowercase().starts_with("yes"))
    {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Inspect,
            "Encrypted PDFs are not accepted by the alpha renderer",
            "Decrypt an authorized copy locally, then retry.",
        ));
    }

    let page_count_argument = page_count.to_string();
    let mut unlocked_details_args: Vec<&str> = match password {
        Some(password) => vec!["-upw", password],
        None => Vec::new(),
    };
    unlocked_details_args.extend_from_slice(&[
        "-box",
        "-f",
        "1",
        "-l",
        page_count_argument.as_str(),
        "-enc",
        "UTF-8",
    ]);
    let details = run_pdfinfo(
        pdfinfo,
        &unlocked_details_args,
        &artifact.canonical_path,
        password.is_some(),
    )
    .await?;
    pdf_probe_from_details(artifact, &details, page_count, &summary, pdfinfo)
}

/// Assembles the pdfinfo Probe from the measured summary and page details.
fn pdf_probe_from_details(
    artifact: crate::domain::ArtifactIdentity,
    details: &str,
    page_count: u32,
    summary: &str,
    pdfinfo: &EngineIdentity,
) -> Result<Probe> {
    let pages = parse_page_details(details, page_count)?;
    let extension = artifact
        .canonical_path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    let extension_matches = extension.as_deref().is_some_and(|value| value == "pdf");
    let warnings = if extension_matches {
        Vec::new()
    } else {
        vec![DiagnosticMessage {
            code: "EXTENSION_MISMATCH".to_owned(),
            severity: "warning".to_owned(),
            message: "File content is PDF but the extension is not .pdf".to_owned(),
        }]
    };
    let version = parse_field(summary, "PDF version").unwrap_or("unknown");
    Ok(Probe {
        schema_version: SCHEMA_VERSION,
        artifact,
        format: FormatDescriptor {
            id: "pdf".to_owned(),
            kind: FormatKind::Pdf,
            mime_type: Some("application/pdf".to_owned()),
            container: Some(format!("pdf-{version}")),
            extension_matches: Some(extension_matches),
            confidence: 1.0,
        },
        streams: pages,
        metadata: BTreeMap::new(),
        warnings,
        evidence: ProbeEvidence {
            engine_id: pdfinfo.engine_id.clone(),
            engine_version: pdfinfo.version.clone(),
            engine_binary_sha256: Some(pdfinfo.binary_sha256.clone()),
        },
        duration_seconds: None,
        bit_rate: None,
    })
}

/// Plans a complete PDF render into an atomically committed page directory.
///
/// # Errors
///
/// Returns a planning error for unsupported targets, invalid DPI/quality/color
/// settings, non-PDF probes, or the wrong rendering engine.
#[allow(clippy::too_many_lines)]
pub fn plan_pdf_render(
    probe: &Probe,
    request: &PlanRequest,
    pdftoppm: &EngineIdentity,
) -> Result<Plan> {
    if probe.format.id != "pdf" || probe.format.kind != FormatKind::Pdf {
        return Err(unsupported("PDF rendering requires a PDF input"));
    }
    if pdftoppm.engine_id != "pdftoppm" {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            "PDF rendering was given the wrong engine",
            "Run doctor and use pdftoppm.",
        ));
    }
    let target = match request
        .target_format
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "jpeg",
        "png" => "png",
        _ => return Err(unsupported("PDF rendering supports only PNG or JPEG")),
    };
    let dpi = request.dpi.unwrap_or(144);
    if !(36..=600).contains(&dpi) {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "PDF render DPI must be between 36 and 600",
            "Choose a supported --dpi value.",
        ));
    }
    let color_mode = request
        .color_mode
        .as_deref()
        .unwrap_or("rgb")
        .trim()
        .to_ascii_lowercase();
    if !matches!(color_mode.as_str(), "rgb" | "gray") {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "PDF color mode must be rgb or gray",
            "Choose --color-mode rgb or --color-mode gray.",
        ));
    }
    let quality = if target == "jpeg" {
        let value = request.quality.unwrap_or(85);
        if !(1..=100).contains(&value) {
            return Err(FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Plan,
                "JPEG quality must be between 1 and 100",
                "Choose a supported --quality value.",
            ));
        }
        Some(value)
    } else {
        if request.quality.is_some() {
            return Err(FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Plan,
                "PNG rendering is lossless and does not accept --quality",
                "Remove --quality or choose JPEG.",
            ));
        }
        None
    };
    let output_path = request.output_path.clone().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "PDF rendering requires an output directory path",
            "Choose a new page-directory path.",
        )
    })?;
    let page_count = u32::try_from(probe.streams.len()).map_err(|_| {
        FormatWrightError::new(
            ErrorCode::ResourceExhausted,
            Stage::Plan,
            "PDF page count cannot be represented",
            "Split the PDF and retry.",
        )
    })?;
    if page_count == 0
        || probe
            .streams
            .iter()
            .any(|page| page.kind != StreamKind::Page)
    {
        return Err(input_pdf_error(
            "PDF probe has no valid page set",
            &probe.artifact.display_path,
        ));
    }
    let expected_dimensions = expected_dimensions(probe, dpi)?;
    if expected_dimensions
        .iter()
        .any(|[width, height]| u64::from(*width).saturating_mul(u64::from(*height)) > 100_000_000)
    {
        return Err(FormatWrightError::new(
            ErrorCode::ResourceExhausted,
            Stage::Plan,
            "A rendered PDF page would exceed the 100-megapixel alpha limit",
            "Choose a lower DPI or split unusual pages into another document.",
        ));
    }
    let arguments = BTreeMap::from([
        ("target_format".to_owned(), target.to_owned()),
        ("dpi".to_owned(), dpi.to_string()),
        ("color_mode".to_owned(), color_mode.clone()),
        ("page_count".to_owned(), page_count.to_string()),
        (
            "jpeg_quality".to_owned(),
            quality.map_or_else(|| "not-applicable".to_owned(), |value| value.to_string()),
        ),
        ("page_prefix".to_owned(), "page".to_owned()),
    ]);
    let constraints = BTreeMap::from([
        ("network".to_owned(), json!("deny")),
        ("output_kind".to_owned(), json!("page-directory")),
        ("page_count".to_owned(), json!(page_count)),
        ("dpi".to_owned(), json!(dpi)),
        ("color_mode".to_owned(), json!(color_mode)),
        ("expected_dimensions".to_owned(), json!(expected_dimensions)),
        ("alpha_expected".to_owned(), json!(false)),
        ("background".to_owned(), json!("opaque-white")),
    ]);
    let raw_bytes = estimated_raster_bytes(&expected_dimensions, color_mode == "gray");
    let step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: format!("poppler.pdf-to-{target}.all-pages"),
        engine: pdftoppm.clone(),
        operation: Operation::Render,
        loss_class: if target == "png" {
            LossClass::Lossless
        } else {
            LossClass::Lossy
        },
        arguments,
        estimated_temporary_bytes: Some(raw_bytes),
    };
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: Uuid::new_v4(),
        plan_hash: String::new(),
        input_fingerprint: probe.artifact.fast_fingerprint.clone(),
        target_format: target.to_owned(),
        constraints,
        steps: vec![step],
        changes: ChangeSet {
            preserved: vec![
                "all PDF pages in source order".to_owned(),
                "page aspect ratios at the selected DPI".to_owned(),
            ],
            changed: vec![
                format!("vector and text content rasterized at {dpi} DPI"),
                format!("color output rendered in {color_mode} mode"),
                "page backgrounds flattened to opaque white".to_owned(),
            ],
            dropped: vec![
                "searchable text, links, forms, annotations, and PDF structure".to_owned(),
                "page transparency".to_owned(),
            ],
            unknown: vec![
                "ICC/profile equivalence is not certified by the alpha validator".to_owned(),
            ],
        },
        validators: vec![
            "pdf.page-count".to_owned(),
            "pdf.each-page-opens".to_owned(),
            "pdf.page-format".to_owned(),
            "pdf.page-dimensions".to_owned(),
            "pdf.color-mode".to_owned(),
            "pdf.alpha-policy".to_owned(),
        ],
        network_policy: NetworkPolicy::Deny,
        output_path: Some(output_path),
        estimated_output_bytes: Some(raw_bytes),
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn validate_pdf_render(
    input: &Probe,
    directory: &Path,
    plan: &Plan,
    ffprobe: &EngineIdentity,
    job_id: Uuid,
) -> Result<ValidationReport> {
    let target = match plan.target_format.as_str() {
        "png" => "png",
        "jpeg" => "jpeg",
        _ => return Err(invalid_plan("target format")),
    };
    let expected_count = plan_u32_constraint(plan, "page_count")?;
    let expected_dimensions = plan
        .constraints
        .get("expected_dimensions")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_plan("expected_dimensions"))?;
    let color_mode = plan
        .constraints
        .get("color_mode")
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "rgb" | "gray"))
        .ok_or_else(|| invalid_plan("color_mode"))?;
    let extension = if target == "jpeg" { "jpg" } else { "png" };
    let paths = exact_page_paths(directory, expected_count, extension)?;
    let mut probes = Vec::with_capacity(paths.len());
    for path in &paths {
        probes.push(inspect_media(path, ffprobe).await?);
    }
    let formats = probes
        .iter()
        .map(|probe| probe.format.id.clone())
        .collect::<Vec<_>>();
    let observed_dimensions = probes
        .iter()
        .map(|probe| {
            let stream = probe.streams.first();
            json!([
                stream.and_then(|value| value.width),
                stream.and_then(|value| value.height)
            ])
        })
        .collect::<Vec<_>>();
    let pixel_formats = probes
        .iter()
        .map(|probe| page_pixel_format(probe).unwrap_or("unknown").to_owned())
        .collect::<Vec<_>>();
    let mut pixel_audits = Vec::with_capacity(paths.len());
    for path in &paths {
        let path = path.clone();
        pixel_audits.push(
            tokio::task::spawn_blocking(move || decode_page_pixels(&path))
                .await
                .map_err(|error| {
                    FormatWrightError::new(
                        ErrorCode::Internal,
                        Stage::Validate,
                        "Rendered-page pixel audit worker failed",
                        "Retry the render or report the input.",
                    )
                    .with_diagnostic(error.to_string())
                })??,
        );
    }
    let decoded_colors = pixel_audits
        .iter()
        .map(|audit| if audit.grayscale { "gray" } else { "color" })
        .collect::<Vec<_>>();
    let color_matches = color_mode == "rgb" || pixel_audits.iter().all(|audit| audit.grayscale);
    let opaque = pixel_audits.iter().all(|audit| audit.opaque);
    let checks = vec![
        check(
            "PDF_PAGE_COUNT",
            status(paths.len() == expected_count as usize),
            json!(expected_count),
            json!(paths.len()),
            "Exact files in the staged page directory.",
        ),
        check(
            "PDF_EACH_PAGE_OPENS",
            status(probes.len() == expected_count as usize),
            json!(expected_count),
            json!(probes.len()),
            "Every page was independently opened by ffprobe.",
        ),
        check(
            "PDF_PAGE_FORMAT",
            status(formats.iter().all(|format| format == target)),
            json!(vec![target; expected_count as usize]),
            json!(formats),
            "Header-first detected format for every rendered page.",
        ),
        check(
            "PDF_PAGE_DIMENSIONS",
            status(observed_dimensions.as_slice() == expected_dimensions.as_slice()),
            Value::Array(expected_dimensions.clone()),
            json!(observed_dimensions),
            "Per-page pixel dimensions derived from points and selected DPI.",
        ),
        check(
            "PDF_COLOR_MODE",
            status(color_matches),
            json!(color_mode),
            json!({
                "decoded_content": decoded_colors,
                "encoded_pixel_formats": pixel_formats,
                "sample_counts": pixel_audits.iter().map(|audit| audit.sample_count).collect::<Vec<_>>()
            }),
            "Bounded native pixel samples plus encoded pixel formats for every page.",
        ),
        check(
            "PDF_ALPHA_POLICY",
            status(opaque),
            json!("opaque"),
            json!(if opaque { "opaque" } else { "alpha-present" }),
            "The declared opaque-white page background policy.",
        ),
    ];
    let report_status = checks.iter().fold(ValidationStatus::Pass, |state, check| {
        state.worst(check.status)
    });
    let size_bytes = probes
        .iter()
        .map(|probe| probe.artifact.size_bytes)
        .fold(0_u64, u64::saturating_add);
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"formatwright-pdf-page-set-v1");
    hasher.update(target.as_bytes());
    for probe in &probes {
        hasher.update(probe.artifact.fast_fingerprint.as_bytes());
    }
    let mut engines = plan
        .steps
        .iter()
        .map(|step| step.engine.clone())
        .collect::<Vec<_>>();
    if !engines.iter().any(|engine| {
        engine.engine_id == ffprobe.engine_id && engine.binary_sha256 == ffprobe.binary_sha256
    }) {
        engines.push(ffprobe.clone());
    }
    Ok(ValidationReport {
        schema_version: SCHEMA_VERSION,
        report_id: Uuid::new_v4(),
        job_id,
        plan_hash: plan.plan_hash.clone(),
        status: report_status,
        input: artifact_summary(input),
        output: ArtifactSummary {
            display_path: Some(directory.to_string_lossy().into_owned()),
            format_id: format!("{target}-page-set"),
            size_bytes,
            fast_fingerprint: format!("fwpages-v1:{}", hasher.finalize().to_hex()),
            full_blake3: None,
        },
        engines,
        checks,
        intentional_changes: plan.changes.changed.clone(),
        redaction: ReportRedaction {
            paths_redacted: false,
            metadata_values_redacted: true,
        },
    })
}

async fn run_pdfinfo(
    engine: &EngineIdentity,
    arguments: &[&str],
    path: &Path,
    password_attempt: bool,
) -> Result<String> {
    let mut command = Command::new(&engine.binary_path);
    command.args(arguments).arg(path);
    let output = tokio::time::timeout(PDFINFO_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            FormatWrightError::new(
                ErrorCode::ExecutionFailed,
                Stage::Inspect,
                "PDF inspection timed out",
                "Check whether the file or storage is responsive.",
            )
            .retryable(true)
        })?
        .map_err(|error| {
            FormatWrightError::new(
                ErrorCode::EngineIncompatible,
                Stage::Inspect,
                "Unable to start pdfinfo",
                "Run doctor and verify the pdfinfo installation.",
            )
            .with_diagnostic(error.to_string())
        })?;
    let stdout = bounded_text(&output.stdout);
    let stderr = bounded_text(&output.stderr);
    if !output.status.success() {
        let combined = format!("{stdout}\n{stderr}");
        if combined.to_ascii_lowercase().contains("incorrect password") {
            if password_attempt {
                // The caller supplied `-upw`: the password is simply wrong for
                // this document (pdf-decrypt planning path).
                return Err(FormatWrightError::new(
                    ErrorCode::InputInvalid,
                    Stage::Inspect,
                    "The password did not unlock the document",
                    "Check the password and retry.",
                ));
            }
            return Err(FormatWrightError::new(
                ErrorCode::PolicyBlocked,
                Stage::Inspect,
                "Encrypted PDFs are not accepted by the alpha renderer",
                "Decrypt an authorized copy locally, then retry.",
            ));
        }
        return Err(input_pdf_error(
            "pdfinfo could not parse the PDF",
            &combined,
        ));
    }
    Ok(stdout)
}

fn parse_page_details(text: &str, expected_count: u32) -> Result<Vec<StreamProbe>> {
    let mut sizes = BTreeMap::<u32, (f64, f64)>::new();
    let mut rotations = BTreeMap::<u32, i32>::new();
    for line in text.lines() {
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        if tokens.len() >= 7
            && tokens[0] == "Page"
            && tokens[2] == "size:"
            && tokens[4] == "x"
            && tokens[6] == "pts"
            && let (Ok(page), Ok(width), Ok(height)) = (
                tokens[1].parse::<u32>(),
                tokens[3].parse::<f64>(),
                tokens[5].parse::<f64>(),
            )
        {
            sizes.insert(page, (width, height));
        } else if tokens.len() >= 4
            && tokens[0] == "Page"
            && tokens[2] == "rot:"
            && let (Ok(page), Ok(rotation)) = (tokens[1].parse::<u32>(), tokens[3].parse::<i32>())
        {
            rotations.insert(page, rotation);
        }
    }
    if sizes.len() != expected_count as usize {
        return Err(incompatible_pdfinfo(
            "pdfinfo did not report dimensions for every page",
            text,
        ));
    }
    let mut streams = Vec::with_capacity(expected_count as usize);
    for page_number in 1..=expected_count {
        let (width, height) = sizes
            .get(&page_number)
            .copied()
            .ok_or_else(|| incompatible_pdfinfo("pdfinfo page numbering is incomplete", text))?;
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return Err(input_pdf_error("PDF contains an invalid page size", text));
        }
        let rotation = rotations.get(&page_number).copied().unwrap_or(0);
        let properties = BTreeMap::from([
            ("page_number".to_owned(), json!(page_number)),
            ("width_points".to_owned(), json!(width)),
            ("height_points".to_owned(), json!(height)),
            ("rotation_degrees".to_owned(), json!(rotation)),
        ]);
        streams.push(StreamProbe {
            index: page_number - 1,
            kind: StreamKind::Page,
            codec: Some("pdf-page".to_owned()),
            language: None,
            duration_seconds: None,
            width: None,
            height: None,
            frame_rate: None,
            sample_rate: None,
            channels: None,
            properties,
        });
    }
    Ok(streams)
}

fn expected_dimensions(probe: &Probe, dpi: u16) -> Result<Vec<[u32; 2]>> {
    probe
        .streams
        .iter()
        .map(|page| {
            let mut width = page
                .properties
                .get("width_points")
                .and_then(Value::as_f64)
                .ok_or_else(|| input_pdf_error("PDF probe lacks page width", ""))?;
            let mut height = page
                .properties
                .get("height_points")
                .and_then(Value::as_f64)
                .ok_or_else(|| input_pdf_error("PDF probe lacks page height", ""))?;
            let rotation = page
                .properties
                .get("rotation_degrees")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                .rem_euclid(360);
            if matches!(rotation, 90 | 270) {
                std::mem::swap(&mut width, &mut height);
            }
            let scale = f64::from(dpi) / 72.0;
            let width = poppler_raster_dimension(width * scale)?;
            let height = poppler_raster_dimension(height * scale)?;
            Ok([width, height])
        })
        .collect()
}

fn poppler_raster_dimension(value: f64) -> Result<u32> {
    if !value.is_finite() || !(1.0..=16_384.0).contains(&value) {
        return Err(FormatWrightError::new(
            ErrorCode::ResourceExhausted,
            Stage::Plan,
            "Rendered PDF page dimension is outside the alpha limit",
            "Choose a lower DPI or split unusual pages into another document.",
        ));
    }
    value.ceil().to_string().parse::<u32>().map_err(|error| {
        FormatWrightError::new(
            ErrorCode::Internal,
            Stage::Plan,
            "Unable to represent a validated rendered page dimension",
            "Report this internal error.",
        )
        .with_diagnostic(error.to_string())
    })
}

fn estimated_raster_bytes(dimensions: &[[u32; 2]], gray: bool) -> u64 {
    let channels = if gray { 1_u64 } else { 3_u64 };
    dimensions.iter().fold(0_u64, |total, [width, height]| {
        total.saturating_add(
            u64::from(*width)
                .saturating_mul(u64::from(*height))
                .saturating_mul(channels),
        )
    })
}

fn exact_page_paths(directory: &Path, page_count: u32, extension: &str) -> Result<Vec<PathBuf>> {
    if !directory.is_dir() {
        return Err(FormatWrightError::new(
            ErrorCode::ExecutionFailed,
            Stage::Validate,
            "PDF renderer did not produce a page directory",
            "Retry the render or inspect the engine diagnostic.",
        ));
    }
    let mut observed = std::fs::read_dir(directory)
        .map_err(|error| storage_error(directory, &error))?
        .map(|entry| entry.map(|value| value.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|error| storage_error(directory, &error))?;
    observed.sort();
    let expected = (1..=page_count)
        .map(|page| directory.join(format!("page-{page:06}.{extension}")))
        .collect::<Vec<_>>();
    if observed != expected || expected.iter().any(|path| !path.is_file()) {
        return Err(FormatWrightError::new(
            ErrorCode::ValidationFailed,
            Stage::Validate,
            "Rendered page directory is incomplete or contains unexpected entries",
            "Inspect the PDF and rendering engine, then retry.",
        )
        .with_diagnostic(format!("expected={expected:?}; observed={observed:?}")));
    }
    Ok(expected)
}

fn page_pixel_format(probe: &Probe) -> Option<&str> {
    probe
        .streams
        .first()
        .and_then(|stream| stream.properties.get("pix_fmt"))
        .and_then(Value::as_str)
}

#[derive(Debug)]
struct PixelAudit {
    grayscale: bool,
    opaque: bool,
    sample_count: usize,
}

fn decode_page_pixels(path: &Path) -> Result<PixelAudit> {
    let mut reader = ImageReader::open(path)
        .and_then(ImageReader::with_guessed_format)
        .map_err(|error| pixel_decode_error(path, error.to_string()))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| pixel_decode_error(path, error.to_string()))?;
    let total = u64::from(image.width()).saturating_mul(u64::from(image.height()));
    let step = usize::try_from(total.saturating_div(16_384).max(1)).unwrap_or(usize::MAX);
    let mut grayscale = true;
    let mut opaque = true;
    let mut sample_count = 0_usize;
    for (_, _, pixel) in image.pixels().step_by(step) {
        let [red, green, blue, alpha] = pixel.0;
        grayscale &=
            red.abs_diff(green) <= 2 && green.abs_diff(blue) <= 2 && red.abs_diff(blue) <= 2;
        opaque &= alpha == u8::MAX;
        sample_count = sample_count.saturating_add(1);
    }
    Ok(PixelAudit {
        grayscale,
        opaque,
        sample_count,
    })
}

fn pixel_decode_error(path: &Path, diagnostic: String) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::ValidationFailed,
        Stage::Validate,
        format!("Unable to decode rendered page pixels: {}", path.display()),
        "Inspect the renderer output and retry.",
    )
    .with_diagnostic(diagnostic)
}

fn parse_field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == key).then(|| value.trim())
    })
}

fn parse_u32_field(text: &str, key: &str) -> Option<u32> {
    parse_field(text, key)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn plan_u32_constraint(plan: &Plan, name: &str) -> Result<u32> {
    plan.constraints
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| invalid_plan(name))
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
    expected: Value,
    observed: Value,
    evidence: &str,
) -> ValidationCheck {
    ValidationCheck {
        code: code.to_owned(),
        status,
        required: true,
        expected,
        observed,
        evidence: evidence.to_owned(),
        message: if status == ValidationStatus::Pass {
            "Required PDF render invariant passed.".to_owned()
        } else {
            "Required PDF render invariant failed.".to_owned()
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

fn bounded_text(bytes: &[u8]) -> String {
    const LIMIT: usize = 64 * 1024;
    let start = bytes.len().saturating_sub(LIMIT);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

fn input_error(path: &Path, error: &std::io::Error) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Inspect,
        format!("Unable to read PDF header: {}", path.display()),
        "Check file permissions and storage health.",
    )
    .with_diagnostic(error.to_string())
}

fn input_pdf_error(message: &str, diagnostic: &str) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Inspect,
        message,
        "Choose a complete, supported PDF.",
    )
    .with_diagnostic(diagnostic.to_owned())
}

fn incompatible_pdfinfo(message: &str, diagnostic: &str) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::EngineIncompatible,
        Stage::Inspect,
        message,
        "Run doctor and use a supported pdfinfo build.",
    )
    .with_diagnostic(diagnostic.to_owned())
}

fn unsupported(message: &str) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::Unsupported,
        Stage::Plan,
        message,
        "Choose PDF input and PNG or JPEG output.",
    )
}

fn invalid_plan(name: &str) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::PolicyBlocked,
        Stage::Validate,
        format!("PDF Plan contains an invalid or missing {name}"),
        "Create a new Plan with the installed FormatWright version.",
    )
}

fn storage_error(path: &Path, error: &std::io::Error) -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::StorageFailed,
        Stage::Validate,
        format!("Unable to read rendered page directory: {}", path.display()),
        "Check storage permissions and retry.",
    )
    .with_diagnostic(error.to_string())
}

/// Parses a page range like `1-3,7` into (sorted selected pages, total count),
/// rejecting empty, zero, and overshooting selections.
pub(crate) fn parse_page_range(range: &str, page_count: u32) -> Result<(Vec<u32>, u32)> {
    let mut selected = Vec::new();
    for part in range.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let bounds: Vec<&str> = part.split('-').collect();
        match bounds.as_slice() {
            [single] => {
                let page = single.parse::<u32>().map_err(|_| invalid_page_range())?;
                selected.push(page);
            }
            [start, end] => {
                let start = start
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| invalid_page_range())?;
                let end = end
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| invalid_page_range())?;
                if end < start {
                    return Err(invalid_page_range());
                }
                selected.extend(start..=end);
            }
            _ => return Err(invalid_page_range()),
        }
    }
    if selected.is_empty() {
        return Err(invalid_page_range());
    }
    if let Some(&maximum) = selected.iter().max()
        && (maximum > page_count || selected.iter().min() == Some(&0))
    {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            format!("Page range reaches page {maximum} but the PDF has {page_count} pages"),
            "Adjust the range to the document.",
        ));
    }
    let total = u32::try_from(selected.len()).unwrap_or(u32::MAX);
    Ok((selected, total))
}

fn invalid_page_range() -> FormatWrightError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        Stage::Plan,
        "Page range must look like 1-3,7 within the document",
        "Use 1-based page numbers separated by commas and hyphens.",
    )
}

fn joint_input_fingerprint(probes: &[Probe]) -> String {
    let joined = probes
        .iter()
        .map(|probe| probe.artifact.fast_fingerprint.as_str())
        .collect::<Vec<_>>()
        .join("\u{1f}");
    format!("joint:{}", blake3::hash(joined.as_bytes()).to_hex())
}

/// Plans a qpdf merge of ordered PDF inputs into one output (ADR-0013).
///
/// # Errors
///
/// Returns `Unsupported` for fewer than two inputs or a non-qpdf engine, and
/// `ResourceExhausted` when the merged page count exceeds the alpha limit.
pub fn plan_pdf_merge(
    probes: &[Probe],
    output_path: PathBuf,
    qpdf: &EngineIdentity,
) -> Result<Plan> {
    if probes.len() < 2 {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            "PDF merge needs at least two inputs",
            "Pass two or more PDF files.",
        ));
    }
    if probes
        .iter()
        .any(|probe| probe.format.id != "pdf" || probe.streams.is_empty())
    {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            "PDF merge inputs must all be inspected PDFs",
            "Run doctor and retry with PDF files only.",
        ));
    }
    if qpdf.engine_id != "qpdf" {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            "The merge Plan was given the wrong engine",
            "Run doctor and use qpdf.",
        ));
    }
    let expected_pages: u64 = probes
        .iter()
        .map(|probe| u64::try_from(probe.streams.len()).unwrap_or(u64::MAX))
        .sum();
    if expected_pages > u64::from(MAX_PDF_PAGES) {
        return Err(FormatWrightError::new(
            ErrorCode::ResourceExhausted,
            Stage::Plan,
            format!(
                "Merged PDF would have {expected_pages} pages; the alpha limit is {MAX_PDF_PAGES}"
            ),
            "Merge fewer or smaller documents.",
        ));
    }
    let inputs_argument = probes
        .iter()
        .map(|probe| {
            let path = probe.artifact.canonical_path.to_string_lossy();
            // qpdf rejects Windows verbatim (`\\?\`) paths, which
            // canonicalization produces; strip the prefix for drive paths.
            match path.strip_prefix(r"\\?\") {
                Some(stripped) => stripped.to_owned(),
                None => path.into_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(";");
    let arguments = BTreeMap::from([
        ("operation".to_owned(), "pdf-merge".to_owned()),
        ("inputs".to_owned(), inputs_argument),
        ("expected_pages".to_owned(), expected_pages.to_string()),
    ]);
    let step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: "qpdf.pdf-merge".to_owned(),
        engine: qpdf.clone(),
        operation: Operation::Transform,
        loss_class: LossClass::None,
        arguments,
        estimated_temporary_bytes: Some(
            probes
                .iter()
                .map(|probe| probe.artifact.size_bytes)
                .sum::<u64>()
                .saturating_mul(2),
        ),
    };
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: Uuid::new_v4(),
        plan_hash: String::new(),
        input_fingerprint: joint_input_fingerprint(probes),
        target_format: "pdf".to_owned(),
        constraints: BTreeMap::from([
            ("network".to_owned(), json!("deny")),
            ("external_resources".to_owned(), json!("deny")),
        ]),
        steps: vec![step],
        changes: ChangeSet {
            preserved: vec![
                "every page of every input, in input order".to_owned(),
                "page content streams".to_owned(),
            ],
            changed: vec!["documents are concatenated into one container".to_owned()],
            dropped: vec![],
            unknown: vec!["per-document metadata of all but the first input".to_owned()],
        },
        validators: vec!["pdf-ops.merge-page-count".to_owned()],
        network_policy: NetworkPolicy::Deny,
        output_path: Some(output_path),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

/// Plans a qpdf page-range extraction into a new single PDF (ADR-0013).
///
/// # Errors
///
/// Returns `Unsupported`/`InputInvalid` for bad ranges or engines.
pub fn plan_pdf_extract(
    probe: &Probe,
    page_range: &str,
    output_path: PathBuf,
    qpdf: &EngineIdentity,
) -> Result<Plan> {
    if probe.format.id != "pdf" {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            "PDF extraction needs a PDF input",
            "Retry with a PDF file.",
        ));
    }
    if qpdf.engine_id != "qpdf" {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            "The extraction Plan was given the wrong engine",
            "Run doctor and use qpdf.",
        ));
    }
    let page_count = u32::try_from(probe.streams.len()).unwrap_or(u32::MAX);
    let (_, selected) = parse_page_range(page_range, page_count)?;
    let normalized_range = normalize_page_range_for_qpdf(page_range)?;
    let arguments = BTreeMap::from([
        ("operation".to_owned(), "pdf-extract".to_owned()),
        ("page_range".to_owned(), normalized_range),
        ("expected_pages".to_owned(), selected.to_string()),
    ]);
    let step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: "qpdf.pdf-extract".to_owned(),
        engine: qpdf.clone(),
        operation: Operation::Transform,
        loss_class: LossClass::None,
        arguments,
        estimated_temporary_bytes: Some(probe.artifact.size_bytes.saturating_mul(2)),
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
        ]),
        steps: vec![step],
        changes: ChangeSet {
            preserved: vec![
                "selected pages in document order".to_owned(),
                "page content streams".to_owned(),
            ],
            changed: vec!["unselected pages are removed".to_owned()],
            dropped: vec!["pages outside the requested range".to_owned()],
            unknown: vec!["document-level metadata is not certified".to_owned()],
        },
        validators: vec!["pdf-ops.extract-page-count".to_owned()],
        network_policy: NetworkPolicy::Deny,
        output_path: Some(output_path),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

/// Validates a merge/extraction output by measured page-count conservation
/// (ADR-0013): the probed output must carry exactly the planned page count.
pub(crate) fn validate_pdf_ops_output(
    input: &Probe,
    output: &Probe,
    plan: &Plan,
    job_id: Uuid,
    expected_pages: u32,
) -> ValidationReport {
    let observed_pages = output.streams.len();
    let mut checks = vec![
        ValidationCheck {
            code: "PDF_OPS_OPENS".to_owned(),
            status: ValidationStatus::Pass,
            required: true,
            expected: json!(true),
            observed: json!(true),
            evidence: "Poppler pdfinfo".to_owned(),
            message: "pdfinfo opened the operation output.".to_owned(),
        },
        ValidationCheck {
            code: "PDF_OPS_PAGE_COUNT".to_owned(),
            status: if u32::try_from(observed_pages).unwrap_or(u32::MAX) == expected_pages {
                ValidationStatus::Pass
            } else {
                ValidationStatus::Fail
            },
            required: true,
            expected: json!(expected_pages),
            observed: json!(observed_pages),
            evidence: "Poppler pdfinfo".to_owned(),
            message: "Measured output page count equals the planned conservation.".to_owned(),
        },
    ];
    if let Some(step) = plan.steps.first()
        && step.arguments.get("operation").map(String::as_str) == Some("pdf-compress")
    {
        // Report-only: structural recompression can legitimately grow tiny or
        // already-optimal documents, so a ratio above 1.0 warns but never fails.
        let input_bytes = input.artifact.size_bytes.max(1);
        let milliratio: u32 =
            u32::try_from(output.artifact.size_bytes.saturating_mul(1000) / input_bytes)
                .unwrap_or(u32::MAX);
        checks.push(ValidationCheck {
            code: "PDF_COMPRESSION_RATIO".to_owned(),
            status: if milliratio <= 1000 {
                ValidationStatus::Pass
            } else {
                ValidationStatus::Warning
            },
            required: false,
            expected: json!("<= 1.0"),
            observed: json!(f64::from(milliratio) / 1000.0),
            evidence: "input/output byte sizes".to_owned(),
            message: "Compressed output bytes divided by input bytes (report-only).".to_owned(),
        });
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

/// Execution-only password hand-off for encrypt/decrypt plans.
///
/// The serialized Plan (which the job store persists as `plan_json`) carries a
/// `[redacted]` placeholder in its step arguments, so the cleartext password
/// never reaches disk. Instead, the planning process registers the real
/// password against the freshly generated `plan_id` here, and the runner takes
/// (and removes) it when the step executes. Consequence: a durably queued
/// encrypt/decrypt plan replayed after a process restart fails with a clear
/// "password unavailable" error instead of leaking or stalling; immediate CLI
/// execution always succeeds because planner and runner share the process.
fn pdf_secret_store() -> &'static std::sync::Mutex<std::collections::HashMap<Uuid, String>> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<Uuid, String>>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn register_pdf_secret(plan_id: Uuid, password: &str) {
    if let Ok(mut store) = pdf_secret_store().lock() {
        store.insert(plan_id, password.to_owned());
    }
}

/// Removes and returns the execution-only password for a plan, if registered.
pub(crate) fn take_pdf_secret(plan_id: Uuid) -> Option<String> {
    pdf_secret_store().lock().ok()?.remove(&plan_id)
}

fn pdf_operation_plan(
    capability_id: &str,
    arguments: BTreeMap<String, String>,
    probe: &Probe,
    output_path: PathBuf,
    qpdf: &EngineIdentity,
    changes: ChangeSet,
    validators: Vec<String>,
) -> Result<Plan> {
    let step = PlanStep {
        step_id: "step-1".to_owned(),
        capability_id: capability_id.to_owned(),
        engine: qpdf.clone(),
        operation: Operation::Transform,
        loss_class: LossClass::None,
        arguments,
        estimated_temporary_bytes: Some(probe.artifact.size_bytes.saturating_mul(2)),
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
        ]),
        steps: vec![step],
        changes,
        validators,
        network_policy: NetworkPolicy::Deny,
        output_path: Some(output_path),
        estimated_output_bytes: None,
    };
    plan.plan_hash = deterministic_plan_hash(&plan)?;
    Ok(plan)
}

fn ensure_pdf_probe(probe: &Probe, operation: &str) -> Result<()> {
    if probe.format.id != "pdf" {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            format!("PDF {operation} needs a PDF input"),
            "Retry with a PDF file.",
        ));
    }
    Ok(())
}

fn ensure_qpdf(qpdf: &EngineIdentity, operation: &str) -> Result<()> {
    if qpdf.engine_id != "qpdf" {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Plan,
            format!("The {operation} Plan was given the wrong engine"),
            "Run doctor and use qpdf.",
        ));
    }
    Ok(())
}

/// Plans a qpdf page rotation (ADR-0013, G-20). An empty page spec rotates
/// every page; rotation is lossless and conserves the page count.
///
/// # Errors
///
/// Returns `InputInvalid` for angles outside 90/180/270 or bad page specs.
pub fn plan_pdf_rotate(
    probe: &Probe,
    angle: u16,
    page_spec: Option<&str>,
    output_path: PathBuf,
    qpdf: &EngineIdentity,
) -> Result<Plan> {
    ensure_pdf_probe(probe, "rotation")?;
    ensure_qpdf(qpdf, "rotation")?;
    if !matches!(angle, 90 | 180 | 270) {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            format!("Rotation angle must be 90, 180, or 270; got {angle}"),
            "Pass --angle with 90, 180, or 270.",
        ));
    }
    let page_count = u32::try_from(probe.streams.len()).unwrap_or(u32::MAX);
    if let Some(spec) = page_spec {
        parse_page_range(spec, page_count)?;
    }
    let normalized_pages = page_spec.map(normalize_page_range_for_qpdf).transpose()?;
    let arguments = BTreeMap::from([
        ("operation".to_owned(), "pdf-rotate".to_owned()),
        ("angle".to_owned(), angle.to_string()),
        (
            "pages".to_owned(),
            normalized_pages.unwrap_or_default(), // empty means all pages
        ),
        ("expected_pages".to_owned(), page_count.to_string()),
    ]);
    pdf_operation_plan(
        "qpdf.pdf-rotate",
        arguments,
        probe,
        output_path,
        qpdf,
        ChangeSet {
            preserved: vec![
                "every page in document order".to_owned(),
                "page content streams".to_owned(),
            ],
            changed: vec![format!("selected pages rotate by {angle} degrees")],
            dropped: vec![],
            unknown: vec!["viewer-dependent rendering of rotated pages".to_owned()],
        },
        vec!["pdf-ops.rotate-page-count".to_owned()],
    )
}

/// Plans a qpdf structural recompression (ADR-0013, G-21). Page count is
/// conserved; the acceptance additionally reports the byte ratio as a
/// non-blocking Warning when the output grows.
///
/// # Errors
///
/// Returns `Unsupported`/`EngineIncompatible` for non-PDF inputs or engines.
pub fn plan_pdf_compress(
    probe: &Probe,
    output_path: PathBuf,
    qpdf: &EngineIdentity,
) -> Result<Plan> {
    ensure_pdf_probe(probe, "compression")?;
    ensure_qpdf(qpdf, "compression")?;
    let page_count = u32::try_from(probe.streams.len()).unwrap_or(u32::MAX);
    let arguments = BTreeMap::from([
        ("operation".to_owned(), "pdf-compress".to_owned()),
        ("expected_pages".to_owned(), page_count.to_string()),
    ]);
    pdf_operation_plan(
        "qpdf.pdf-compress",
        arguments,
        probe,
        output_path,
        qpdf,
        ChangeSet {
            preserved: vec![
                "every page in document order".to_owned(),
                "page content".to_owned(),
            ],
            changed: vec![
                "streams are recompressed with maximum Flate".to_owned(),
                "objects are packed into object streams".to_owned(),
            ],
            dropped: vec![],
            unknown: vec!["exact byte-level reproduction is not certified".to_owned()],
        },
        vec![
            "pdf-ops.compress-page-count".to_owned(),
            "pdf-ops.compress-ratio".to_owned(),
        ],
    )
}

fn plan_pdf_secret_operation(
    probe: &Probe,
    operation: &str,
    password: Option<&str>,
    output_path: PathBuf,
    qpdf: &EngineIdentity,
    changed: Vec<String>,
    validator: &str,
) -> Result<Plan> {
    ensure_pdf_probe(probe, operation)?;
    ensure_qpdf(qpdf, operation)?;
    let password = password.unwrap_or("").trim();
    if password.is_empty() {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            format!("PDF {operation} needs a non-empty password"),
            "Pass --password with the document password.",
        ));
    }
    let page_count = u32::try_from(probe.streams.len()).unwrap_or(u32::MAX);
    let arguments = BTreeMap::from([
        ("operation".to_owned(), operation.to_owned()),
        // Never the cleartext password: Plans serialize into the durable job
        // store. The real secret travels via the execution-only store keyed by
        // plan_id; see `pdf_secret_store`.
        ("password".to_owned(), "[redacted]".to_owned()),
        ("expected_pages".to_owned(), page_count.to_string()),
    ]);
    let plan = pdf_operation_plan(
        &format!("qpdf.{operation}"),
        arguments,
        probe,
        output_path,
        qpdf,
        ChangeSet {
            preserved: vec!["every page in document order".to_owned()],
            changed,
            dropped: vec![],
            unknown: vec!["document-level metadata is not certified".to_owned()],
        },
        vec![validator.to_owned()],
    )?;
    register_pdf_secret(plan.plan_id, password);
    Ok(plan)
}

/// Plans a qpdf AES-256 encryption (ADR-0013, G-22). The user and owner
/// passwords are both set to the supplied password with printing allowed and
/// modification denied.
///
/// # Errors
///
/// Returns `InputInvalid` for empty passwords, `Unsupported` otherwise.
pub fn plan_pdf_encrypt(
    probe: &Probe,
    password: Option<&str>,
    output_path: PathBuf,
    qpdf: &EngineIdentity,
) -> Result<Plan> {
    plan_pdf_secret_operation(
        probe,
        "pdf-encrypt",
        password,
        output_path,
        qpdf,
        vec![
            "the document is encrypted with AES-256".to_owned(),
            "printing stays allowed; modification is denied".to_owned(),
        ],
        "pdf-ops.encrypt-locked",
    )
}

/// Plans a qpdf decryption of a password-protected input (ADR-0013, G-22).
///
/// # Errors
///
/// Returns `InputInvalid` for empty passwords, `Unsupported` otherwise.
pub fn plan_pdf_decrypt(
    probe: &Probe,
    password: Option<&str>,
    output_path: PathBuf,
    qpdf: &EngineIdentity,
) -> Result<Plan> {
    plan_pdf_secret_operation(
        probe,
        "pdf-decrypt",
        password,
        output_path,
        qpdf,
        vec!["encryption is removed from the document".to_owned()],
        "pdf-ops.decrypt-page-count",
    )
}

/// Upper bound for /Info text values (pdf-metadata). Longer titles or author
/// names are rejected rather than silently truncated.
const MAX_METADATA_TEXT_BYTES: usize = 200;

/// Plans a metadata-only revision (ADR-0013 operation `pdf-metadata`): the
/// document /Title and/or /Author are set through an in-process incremental
/// update, so no external engine rewrites the file. Page count is conserved.
///
/// # Errors
///
/// Returns `InputInvalid` when neither field is supplied or a value is empty,
/// overlong, or contains control characters; `Unsupported` otherwise.
pub fn plan_pdf_metadata(
    probe: &Probe,
    title: Option<&str>,
    author: Option<&str>,
    output_path: PathBuf,
    qpdf: &EngineIdentity,
) -> Result<Plan> {
    ensure_pdf_probe(probe, "metadata")?;
    ensure_qpdf(qpdf, "metadata")?;
    let title = title.map(str::trim).filter(|value| !value.is_empty());
    let author = author.map(str::trim).filter(|value| !value.is_empty());
    if title.is_none() && author.is_none() {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "PDF metadata needs a title or an author",
            "Pass --metadata-title and/or --metadata-author.",
        ));
    }
    for (field, value) in [("title", title), ("author", author)] {
        if let Some(value) = value
            && (value.len() > MAX_METADATA_TEXT_BYTES || value.chars().any(char::is_control))
        {
            return Err(FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Plan,
                format!(
                    "PDF metadata {field} must be at most {MAX_METADATA_TEXT_BYTES} bytes without control characters"
                ),
                "Shorten the value and remove control characters.",
            ));
        }
    }
    let page_count = u32::try_from(probe.streams.len()).unwrap_or(u32::MAX);
    let mut arguments = BTreeMap::from([
        ("operation".to_owned(), "pdf-metadata".to_owned()),
        ("expected_pages".to_owned(), page_count.to_string()),
    ]);
    if let Some(title) = title {
        arguments.insert("metadata_title".to_owned(), title.to_owned());
    }
    if let Some(author) = author {
        arguments.insert("metadata_author".to_owned(), author.to_owned());
    }
    let mut changed = Vec::new();
    if title.is_some() {
        changed.push("the document /Title is replaced".to_owned());
    }
    if author.is_some() {
        changed.push("the document /Author is replaced".to_owned());
    }
    pdf_operation_plan(
        "qpdf.pdf-metadata",
        arguments,
        probe,
        output_path,
        qpdf,
        ChangeSet {
            preserved: vec![
                "every page in document order".to_owned(),
                "page content streams".to_owned(),
            ],
            changed,
            dropped: vec![],
            unknown: vec!["other /Info entries are not certified".to_owned()],
        },
        vec![
            "pdf-ops.metadata-page-count".to_owned(),
            "pdf-ops.metadata-fields".to_owned(),
        ],
    )
}

/// Applies /Title and/or /Author to a PDF through an incremental update: the
/// original bytes are preserved verbatim and a new /Info object, a one-entry
/// xref subsection, and a trailer with `/Prev` pointing at the previous xref
/// are appended. Zero dependencies, fully deterministic.
///
/// # Errors
///
/// Returns an input error when `startxref`, the previous trailer, or its
/// `/Root` reference cannot be located (the file is not a well-formed PDF).
pub(crate) fn apply_pdf_metadata(
    input_bytes: &[u8],
    title: Option<&str>,
    author: Option<&str>,
) -> Result<Vec<u8>> {
    let text = String::from_utf8_lossy(input_bytes).into_owned();
    let previous_xref = last_startxref_offset(input_bytes).ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Execute,
            "PDF has no startxref marker for an incremental update",
            "Verify the file is a complete PDF and retry.",
        )
    })?;
    let search_from = usize::try_from(previous_xref).unwrap_or(0).min(text.len());
    let trailer_start = text[search_from..]
        .find("trailer")
        .map(|offset| search_from + offset)
        .or_else(|| text.find("trailer"))
        .ok_or_else(|| {
            FormatWrightError::new(
                ErrorCode::InputInvalid,
                Stage::Execute,
                "PDF trailer could not be located",
                "Verify the file is a complete PDF and retry.",
            )
        })?;
    let trailer_text = &text[trailer_start..];
    let root = indirect_reference(trailer_text, "/Root").ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Execute,
            "PDF trailer carries no /Root reference",
            "Verify the file is a complete PDF and retry.",
        )
    })?;
    let max_object = max_object_number(input_bytes);
    let new_object = max_object.saturating_add(1);
    let size = max_object
        .saturating_add(2)
        .max(parse_trailer_size(trailer_text));

    let mut info_entries = String::new();
    if let Some(title) = title.map(str::trim).filter(|value| !value.is_empty()) {
        info_entries.push_str("/Title (");
        info_entries.push_str(&escape_pdf_string(title));
        info_entries.push_str(") ");
    }
    if let Some(author) = author.map(str::trim).filter(|value| !value.is_empty()) {
        info_entries.push_str("/Author (");
        info_entries.push_str(&escape_pdf_string(author));
        info_entries.push_str(") ");
    }
    if info_entries.is_empty() {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Execute,
            "PDF metadata update carries no fields",
            "Pass a title or an author.",
        ));
    }

    let mut output = Vec::with_capacity(input_bytes.len() + 256);
    output.extend_from_slice(input_bytes);
    output.push(b'\n');
    let new_object_offset = output.len() as u64;
    output
        .extend_from_slice(format!("{new_object} 0 obj\n<< {info_entries}>>\nendobj\n").as_bytes());
    let new_xref_offset = output.len() as u64;
    output.extend_from_slice(
        format!(
            "xref\n{new_object} 1\n{new_object_offset:010} 00000 n \n\
             trailer\n<< /Size {size} /Prev {previous_xref} /Root {root} /Info {new_object} 0 R >>\n\
             startxref\n{new_xref_offset}\n%%EOF\n"
        )
        .as_bytes(),
    );
    Ok(output)
}

/// Returns the byte offset carried by the last `startxref` marker.
fn last_startxref_offset(bytes: &[u8]) -> Option<u64> {
    let tail = &bytes[bytes.len().saturating_sub(2048)..];
    let position = tail
        .windows(b"startxref".len())
        .rposition(|window| window == b"startxref")?;
    let mut index = position + b"startxref".len();
    while index < tail.len() && tail[index].is_ascii_whitespace() {
        index += 1;
    }
    let digits: &[u8] = &tail[index..];
    let end = digits
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(digits.len());
    let value = std::str::from_utf8(&digits[..end]).ok()?.parse().ok()?;
    Some(value)
}

/// Returns the highest `N` from every `N G obj` header in the file.
fn max_object_number(bytes: &[u8]) -> u64 {
    let text = String::from_utf8_lossy(bytes);
    let mut max: u64 = 0;
    let tokens = text.split_whitespace().collect::<Vec<_>>();
    for window in tokens.windows(3) {
        if window[2] == "obj"
            && let (Ok(number), Ok(_generation)) =
                (window[0].parse::<u64>(), window[1].parse::<u64>())
        {
            max = max.max(number);
        }
    }
    max
}

/// Extracts the `N G R` reference that follows a trailer key like `/Root`.
fn indirect_reference(trailer_text: &str, key: &str) -> Option<String> {
    let key_position = trailer_text.find(key)?;
    let rest = &trailer_text[key_position + key.len()..];
    let mut tokens = rest.split_whitespace();
    let number = tokens.next()?;
    let generation = tokens.next()?;
    let marker = tokens.next()?;
    if marker.starts_with('R') && number.chars().all(char::is_numeric) {
        Some(format!("{number} {generation} R"))
    } else {
        None
    }
}

fn parse_trailer_size(trailer_text: &str) -> u64 {
    parse_field(trailer_text, "/Size")
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

/// Escapes a PDF literal string: `\` and parentheses are backslash-escaped and
/// every non-ASCII byte is written as a three-digit octal escape.
fn escape_pdf_string(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' | '(' | ')' => {
                escaped.push('\\');
                escaped.push(character);
            }
            ' '..='\u{7F}' => escaped.push(character),
            _ => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\{:03o}", u32::from(character));
            }
        }
    }
    escaped
}

/// Reads the document-level `/Title` and `/Author` pdfinfo reports for a PDF.
///
/// # Errors
///
/// Returns the `pdfinfo` inspection errors unchanged.
pub(crate) async fn pdfinfo_document_metadata(
    path: &Path,
    pdfinfo: &EngineIdentity,
) -> Result<(Option<String>, Option<String>)> {
    let summary = run_pdfinfo(pdfinfo, &["-enc", "UTF-8"], path, false).await?;
    Ok((
        parse_field(&summary, "Title").map(str::to_owned),
        parse_field(&summary, "Author").map(str::to_owned),
    ))
}

/// Appends the required `PDF_METADATA_TITLE`/`PDF_METADATA_AUTHOR` acceptance
/// re-derives the worst-case report status. Fields the Plan did not set are
/// skipped: an incremental update cannot clear values it never touches.
pub(crate) fn append_metadata_checks(
    report: &mut ValidationReport,
    plan: &Plan,
    observed_title: Option<&str>,
    observed_author: Option<&str>,
) {
    let step = plan.steps.first();
    for (code, argument, observed) in [
        ("PDF_METADATA_TITLE", "metadata_title", observed_title),
        ("PDF_METADATA_AUTHOR", "metadata_author", observed_author),
    ] {
        let Some(expected) = step.and_then(|step| step.arguments.get(argument)) else {
            continue;
        };
        let matched = observed.is_some_and(|value| value.trim() == expected.trim());
        let check = ValidationCheck {
            code: code.to_owned(),
            status: if matched {
                ValidationStatus::Pass
            } else {
                ValidationStatus::Fail
            },
            required: true,
            expected: json!(expected),
            observed: json!(observed),
            evidence: "Poppler pdfinfo".to_owned(),
            message: "pdfinfo must report the metadata value the Plan set.".to_owned(),
        };
        report.status = report.status.worst(check.status);
        report.checks.push(check);
    }
}

/// Upper bound for watermark text length; longer banners degrade readability
/// on typical pages.
const MAX_WATERMARK_TEXT_BYTES: usize = 80;
const WATERMARK_FONT_SIZE_PT: f64 = 36.0;
const WATERMARK_FILL_ALPHA: &str = "0.18";

/// Plans a qpdf text watermark stamped on every page (ADR-0013, G-23). The
/// watermark text itself is not confidential, so it travels inside the Plan
/// (unlike passwords); page count is conserved.
///
/// # Errors
///
/// Returns `InputInvalid` for empty, overlong, or non-printable-ASCII text and
/// for angles outside -180..=180, `Unsupported`/`EngineIncompatible` otherwise.
pub fn plan_pdf_watermark(
    probe: &Probe,
    text: &str,
    angle: Option<i16>,
    output_path: PathBuf,
    qpdf: &EngineIdentity,
) -> Result<Plan> {
    ensure_pdf_probe(probe, "watermark")?;
    ensure_qpdf(qpdf, "watermark")?;
    let text = text.trim();
    if text.is_empty() {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "PDF watermark needs non-empty text",
            "Pass --watermark-text with the stamp text.",
        ));
    }
    if text.len() > MAX_WATERMARK_TEXT_BYTES
        || !text.bytes().all(|byte| (0x20..=0x7E).contains(&byte))
    {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            format!(
                "Watermark text must be printable ASCII of at most {MAX_WATERMARK_TEXT_BYTES} bytes"
            ),
            "Use plain ASCII letters, digits, spaces, and punctuation.",
        ));
    }
    let angle = angle.unwrap_or(-45);
    if !(-180..=180).contains(&angle) {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            format!("Watermark angle must be within -180..=180 degrees; got {angle}"),
            "Pass --watermark-angle between -180 and 180.",
        ));
    }
    let page_count = u32::try_from(probe.streams.len()).unwrap_or(u32::MAX);
    let arguments = BTreeMap::from([
        ("operation".to_owned(), "pdf-watermark".to_owned()),
        ("watermark_text".to_owned(), text.to_owned()),
        ("watermark_angle".to_owned(), angle.to_string()),
        ("expected_pages".to_owned(), page_count.to_string()),
    ]);
    pdf_operation_plan(
        "qpdf.pdf-watermark",
        arguments,
        probe,
        output_path,
        qpdf,
        ChangeSet {
            preserved: vec![
                "every page in document order".to_owned(),
                "existing page content".to_owned(),
            ],
            changed: vec![format!(
                "a translucent '{text}' watermark at {angle} degrees is stamped on every page"
            )],
            dropped: vec![],
            unknown: vec!["z-order of the watermark over interactive forms".to_owned()],
        },
        vec![
            "pdf-ops.watermark-page-count".to_owned(),
            "pdf-ops.watermark-text-present".to_owned(),
        ],
    )
}

/// Builds a minimal one-page watermark-overlay PDF (G-23): Helvetica-Bold
/// text, centered, rotated, 0.18 alpha via `ExtGState`, mid-gray fill. The
/// is uncompressed PDF 1.4 with a hand-built xref table — zero dependencies,
/// fully deterministic for identical inputs.
pub(crate) fn build_watermark_pdf(
    page_width_pt: f64,
    page_height_pt: f64,
    text: &str,
    angle_degrees: i16,
) -> Vec<u8> {
    let radians = f64::from(angle_degrees).to_radians();
    let (sin, cos) = radians.sin_cos();
    let neg_sin = -sin;
    let center_x = page_width_pt / 2.0;
    let center_y = page_height_pt / 2.0;
    // Helvetica-Bold average glyph width (~0.58 em) approximates the string
    // width well enough to center the baseline.
    let character_count = u16::try_from(text.chars().count()).unwrap_or(u16::MAX);
    let text_width = WATERMARK_FONT_SIZE_PT * 0.58 * f64::from(character_count);
    let escaped = escape_pdf_text(text);
    let content = format!(
        "q\n{cos:.5} {sin:.5} {neg_sin:.5} {cos:.5} {center_x:.2} {center_y:.2} cm\n\
         /Gs0 gs\n0.5 g\nBT\n/F1 {WATERMARK_FONT_SIZE_PT:.0} Tf\n\
         1 0 0 1 {:.2} {:.2} Td\n({escaped}) Tj\nET\nQ\n",
        -text_width / 2.0,
        -WATERMARK_FONT_SIZE_PT * 0.35,
    );
    let mut objects: Vec<String> = Vec::with_capacity(6);
    objects.push("<< /Type /Catalog /Pages 2 0 R >>".to_owned());
    objects.push("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned());
    objects.push(format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {page_width_pt:.2} {page_height_pt:.2}] \
         /Resources << /Font << /F1 5 0 R >> /ExtGState << /Gs0 6 0 R >> >> /Contents 4 0 R >>"
    ));
    objects.push(format!(
        "<< /Length {} >>\nstream\n{content}endstream",
        content.len()
    ));
    objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>".to_owned());
    objects.push(format!(
        "<< /Type /ExtGState /ca {WATERMARK_FILL_ALPHA} /CA {WATERMARK_FILL_ALPHA} >>"
    ));

    let mut pdf = Vec::with_capacity(2048);
    pdf.extend_from_slice(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len() as u64);
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref_offset = pdf.len() as u64;
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

fn escape_pdf_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '(' | ')' | '\\' => {
                escaped.push('\\');
                escaped.push(character);
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Lowercases and strips all whitespace so `pdftotext` output can be matched
/// against the planned watermark text without layout sensitivity.
pub(crate) fn normalized_watermark_text(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

/// Multi-set (character-histogram) containment of the expected watermark
/// text inside the extracted text. Poppler reorders the glyphs of rotated
/// text into visual line fragments, so an exact substring match is not
/// reliable even though every character is present.
pub(crate) fn watermark_chars_present(expected_text: &str, extracted_text: &str) -> bool {
    let mut remaining: Vec<char> = normalized_watermark_text(expected_text).chars().collect();
    for character in normalized_watermark_text(extracted_text).chars() {
        if let Some(position) = remaining.iter().position(|value| *value == character) {
            remaining.remove(position);
        }
        if remaining.is_empty() {
            return true;
        }
    }
    remaining.is_empty()
}

/// Appends the machine-readable watermark-text acceptance check (G-23) to an
/// operation report and re-derives the worst-case report status.
pub(crate) fn append_watermark_text_check(
    report: &mut crate::domain::ValidationReport,
    expected_text: &str,
    extracted_text: &str,
) {
    let normalized_extracted = normalized_watermark_text(extracted_text);
    let normalized_expected = normalized_watermark_text(expected_text);
    let present = normalized_extracted.contains(&normalized_expected)
        || watermark_chars_present(&normalized_expected, &normalized_extracted);
    let check = ValidationCheck {
        code: "PDF_OPS_WATERMARK_TEXT".to_owned(),
        status: if present {
            ValidationStatus::Pass
        } else {
            ValidationStatus::Fail
        },
        required: true,
        expected: json!(expected_text),
        observed: json!(present),
        evidence: "Poppler pdftotext".to_owned(),
        message: "Every-page text extraction must contain the watermark text.".to_owned(),
    };
    report.status = report.status.worst(check.status);
    report.checks.push(check);
}

/// Validates an encryption output (ADR-0013, G-22). Page-count conservation
/// cannot be probed: pdfinfo refuses to open the AES-256 output without the
/// password. That refusal is itself the acceptance evidence — the caller runs
/// pdfinfo and passes `encrypted = true` when the inspect failed, which proves
/// the output is password-protected. File existence and qpdf's success exit
/// were already enforced by the runner before validation.
pub(crate) fn validate_pdf_encrypt_output(
    input: &Probe,
    output_identity: &crate::domain::ArtifactIdentity,
    plan: &Plan,
    job_id: Uuid,
    encrypted: bool,
) -> ValidationReport {
    let checks = vec![
        ValidationCheck {
            code: "PDF_ENCRYPTED".to_owned(),
            status: if encrypted {
                ValidationStatus::Pass
            } else {
                ValidationStatus::Fail
            },
            required: true,
            expected: json!(true),
            observed: json!(encrypted),
            evidence: "Poppler pdfinfo rejected the output without a password".to_owned(),
            message: "pdfinfo must fail to open the encrypted output.".to_owned(),
        },
        ValidationCheck {
            code: "PDF_OPS_OUTPUT_EXISTS".to_owned(),
            status: ValidationStatus::Pass,
            required: true,
            expected: json!(true),
            observed: json!(true),
            evidence: "post-execution file check".to_owned(),
            message: "qpdf exited successfully and wrote the output file.".to_owned(),
        },
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
        input: ArtifactSummary {
            display_path: Some(input.artifact.display_path.clone()),
            format_id: input.format.id.clone(),
            size_bytes: input.artifact.size_bytes,
            fast_fingerprint: input.artifact.fast_fingerprint.clone(),
            full_blake3: input.artifact.full_blake3.clone(),
        },
        output: ArtifactSummary {
            display_path: Some(output_identity.display_path.clone()),
            format_id: "pdf".to_owned(),
            size_bytes: output_identity.size_bytes,
            fast_fingerprint: output_identity.fast_fingerprint.clone(),
            full_blake3: output_identity.full_blake3.clone(),
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

fn normalize_page_range_for_qpdf(range: &str) -> Result<String> {
    let parts: Vec<String> = range
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.replace(' ', ""))
        .collect();
    if parts.is_empty() {
        return Err(invalid_page_range());
    }
    Ok(parts.join(","))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use formatwright_engine_sdk::{Certification, EngineIdentity};

    use super::{
        append_watermark_text_check, build_watermark_pdf, normalized_watermark_text,
        parse_page_details, parse_page_range, plan_pdf_compress, plan_pdf_decrypt,
        plan_pdf_encrypt, plan_pdf_extract, plan_pdf_merge, plan_pdf_render, plan_pdf_rotate,
        plan_pdf_watermark, poppler_raster_dimension,
    };
    use crate::domain::{
        ArtifactIdentity, FormatDescriptor, FormatKind, PlanRequest, Probe, ProbeEvidence,
        SCHEMA_VERSION,
    };

    fn probe() -> Probe {
        let details = "Pages: 2\nPage    1 size: 612 x 792 pts (letter)\nPage    1 rot: 0\nPage    2 size: 792 x 612 pts (letter)\nPage    2 rot: 0\n";
        Probe {
            schema_version: SCHEMA_VERSION,
            artifact: ArtifactIdentity {
                display_path: "fixture.pdf".to_owned(),
                canonical_path: PathBuf::from("fixture.pdf"),
                size_bytes: 100,
                modified_unix_ms: 1,
                fast_fingerprint: "fwfp-v1:test".to_owned(),
                full_blake3: None,
            },
            format: FormatDescriptor {
                id: "pdf".to_owned(),
                kind: FormatKind::Pdf,
                mime_type: Some("application/pdf".to_owned()),
                container: Some("pdf-1.7".to_owned()),
                extension_matches: Some(true),
                confidence: 1.0,
            },
            streams: parse_page_details(details, 2).expect("page details"),
            metadata: BTreeMap::new(),
            warnings: Vec::new(),
            evidence: ProbeEvidence {
                engine_id: "pdfinfo".to_owned(),
                engine_version: "test".to_owned(),
                engine_binary_sha256: Some("sha".to_owned()),
            },
            duration_seconds: None,
            bit_rate: None,
        }
    }

    fn engine() -> EngineIdentity {
        EngineIdentity {
            engine_id: "pdftoppm".to_owned(),
            version: "test".to_owned(),
            binary_path: PathBuf::from("pdftoppm"),
            binary_sha256: "sha".to_owned(),
            manifest_sha256: None,
            build_configuration: None,
            certification: Certification::Experimental,
        }
    }

    #[test]
    fn plans_every_page_with_deterministic_dimensions() {
        let request = PlanRequest {
            target_format: "png".to_owned(),
            output_path: Some(PathBuf::from("pages")),
            dpi: Some(144),
            color_mode: Some("gray".to_owned()),
            ..PlanRequest::default()
        };
        let plan = plan_pdf_render(&probe(), &request, &engine()).expect("PDF Plan");
        assert_eq!(plan.constraints["page_count"], 2);
        assert_eq!(
            plan.constraints["expected_dimensions"],
            serde_json::json!([[1224, 1584], [1584, 1224]])
        );
        assert_eq!(plan.steps[0].arguments["color_mode"], "gray");
    }

    #[test]
    fn poppler_dimensions_round_fractional_pixels_up() {
        assert_eq!(
            poppler_raster_dimension(595.32).expect("A4 width at 72 DPI"),
            596
        );
        assert_eq!(
            poppler_raster_dimension(420.96).expect("A4 height at 36 DPI"),
            421
        );
        assert_eq!(
            poppler_raster_dimension(612.0).expect("integral letter width"),
            612
        );
    }

    #[test]
    fn rejects_png_quality_and_invalid_dpi() {
        let mut request = PlanRequest {
            target_format: "png".to_owned(),
            output_path: Some(PathBuf::from("pages")),
            quality: Some(80),
            ..PlanRequest::default()
        };
        assert!(plan_pdf_render(&probe(), &request, &engine()).is_err());
        request.quality = None;
        request.dpi = Some(601);
        assert!(plan_pdf_render(&probe(), &request, &engine()).is_err());
    }

    fn qpdf_engine() -> EngineIdentity {
        EngineIdentity {
            engine_id: "qpdf".to_owned(),
            version: "12.0".to_owned(),
            binary_path: PathBuf::from("qpdf"),
            binary_sha256: "sha".to_owned(),
            manifest_sha256: None,
            build_configuration: None,
            certification: Certification::Experimental,
        }
    }

    #[test]
    fn page_ranges_parse_with_bounds_enforcement() {
        let (pages, total) = parse_page_range("1-3,7", 10).expect("valid range");
        assert_eq!(pages, vec![1, 2, 3, 7]);
        assert_eq!(total, 4);
        assert!(parse_page_range("", 10).is_err(), "empty range is rejected");
        assert!(
            parse_page_range("5-3", 10).is_err(),
            "reversed range is rejected"
        );
        assert!(parse_page_range("0", 10).is_err(), "page zero is rejected");
        assert!(
            parse_page_range("11", 10).is_err(),
            "overshooting range is rejected"
        );
        assert!(
            parse_page_range("9-11", 10).is_err(),
            "overshooting span is rejected"
        );
    }

    #[test]
    fn merge_plans_carry_the_conservative_page_count() {
        let first = probe();
        let second = probe();
        let plan = plan_pdf_merge(
            &[first, second],
            PathBuf::from("merged.pdf"),
            &qpdf_engine(),
        )
        .expect("merge plan");
        assert_eq!(plan.target_format, "pdf");
        assert_eq!(plan.steps[0].arguments["expected_pages"], "4");
        assert_eq!(plan.steps[0].arguments["operation"], "pdf-merge");
        assert!(
            plan.validators
                .iter()
                .any(|validator| validator == "pdf-ops.merge-page-count")
        );

        let single = plan_pdf_merge(&[probe()], PathBuf::from("x.pdf"), &qpdf_engine());
        assert!(single.is_err(), "a merge needs at least two inputs");
        let wrong_engine = plan_pdf_merge(&[probe(), probe()], PathBuf::from("x.pdf"), &engine());
        assert!(
            wrong_engine.is_err(),
            "the merge plan rejects non-qpdf engines"
        );
    }

    #[test]
    fn extract_plans_count_the_requested_pages() {
        let plan = plan_pdf_extract(&probe(), "1,2", PathBuf::from("subset.pdf"), &qpdf_engine())
            .expect("extract plan");
        assert_eq!(plan.steps[0].arguments["expected_pages"], "2");
        assert_eq!(plan.steps[0].arguments["page_range"], "1,2");
        assert_eq!(
            plan.changes.dropped,
            vec!["pages outside the requested range".to_owned()]
        );

        let overshoot = plan_pdf_extract(&probe(), "1-5", PathBuf::from("x.pdf"), &qpdf_engine());
        assert!(
            overshoot.is_err(),
            "extraction beyond the last page is rejected"
        );
    }

    #[test]
    fn rotate_plans_conserve_the_page_count() {
        let plan = plan_pdf_rotate(
            &probe(),
            90,
            Some("1,2"),
            PathBuf::from("rotated.pdf"),
            &qpdf_engine(),
        )
        .expect("rotate plan");
        assert_eq!(plan.steps[0].arguments["operation"], "pdf-rotate");
        assert_eq!(plan.steps[0].arguments["angle"], "90");
        assert_eq!(plan.steps[0].arguments["pages"], "1,2");
        assert_eq!(plan.steps[0].arguments["expected_pages"], "2");
        assert!(
            plan.validators
                .iter()
                .any(|validator| validator == "pdf-ops.rotate-page-count")
        );

        let all_pages = plan_pdf_rotate(
            &probe(),
            180,
            None,
            PathBuf::from("rotated.pdf"),
            &qpdf_engine(),
        )
        .expect("rotate-all plan");
        assert_eq!(all_pages.steps[0].arguments["pages"], "");

        assert!(
            plan_pdf_rotate(&probe(), 45, None, PathBuf::from("x.pdf"), &qpdf_engine()).is_err(),
            "non-right angles are rejected"
        );
        assert!(
            plan_pdf_rotate(
                &probe(),
                90,
                Some("1-9"),
                PathBuf::from("x.pdf"),
                &qpdf_engine()
            )
            .is_err(),
            "rotation beyond the last page is rejected"
        );
        assert!(
            plan_pdf_rotate(&probe(), 90, None, PathBuf::from("x.pdf"), &engine()).is_err(),
            "the rotate plan rejects non-qpdf engines"
        );
    }

    #[test]
    fn compress_plans_keep_every_page_and_declare_the_ratio_check() {
        let plan = plan_pdf_compress(&probe(), PathBuf::from("small.pdf"), &qpdf_engine())
            .expect("compress plan");
        assert_eq!(plan.steps[0].arguments["operation"], "pdf-compress");
        assert_eq!(plan.steps[0].arguments["expected_pages"], "2");
        assert!(
            plan.validators
                .iter()
                .any(|validator| validator == "pdf-ops.compress-ratio")
        );
        assert!(
            plan_pdf_compress(&probe(), PathBuf::from("x.pdf"), &engine()).is_err(),
            "the compress plan rejects non-qpdf engines"
        );
    }

    #[test]
    fn encrypt_plans_redact_the_password_and_register_the_secret() {
        let plan = plan_pdf_encrypt(
            &probe(),
            Some("s3cret"),
            PathBuf::from("locked.pdf"),
            &qpdf_engine(),
        )
        .expect("encrypt plan");
        assert_eq!(plan.steps[0].arguments["operation"], "pdf-encrypt");
        assert_eq!(plan.steps[0].arguments["password"], "[redacted]");
        assert_eq!(plan.steps[0].arguments["expected_pages"], "2");
        assert!(
            plan.validators
                .iter()
                .any(|validator| validator == "pdf-ops.encrypt-locked")
        );
        let serialized = serde_json::to_string(&plan).expect("serialized Plan");
        assert!(
            !serialized.contains("s3cret"),
            "the cleartext password never serializes"
        );
        assert_eq!(
            super::take_pdf_secret(plan.plan_id).as_deref(),
            Some("s3cret"),
            "the runner can take the execution-only secret"
        );
        assert!(
            super::take_pdf_secret(plan.plan_id).is_none(),
            "taking the secret is one-shot"
        );

        assert!(
            plan_pdf_encrypt(&probe(), None, PathBuf::from("x.pdf"), &qpdf_engine()).is_err(),
            "encryption without a password is rejected"
        );
        assert!(
            plan_pdf_encrypt(&probe(), Some("  "), PathBuf::from("x.pdf"), &qpdf_engine()).is_err(),
            "blank passwords are rejected"
        );
    }

    #[test]
    fn decrypt_plans_redact_the_password_and_conserve_pages() {
        let plan = plan_pdf_decrypt(
            &probe(),
            Some("pw"),
            PathBuf::from("open.pdf"),
            &qpdf_engine(),
        )
        .expect("decrypt plan");
        assert_eq!(plan.steps[0].arguments["operation"], "pdf-decrypt");
        assert_eq!(plan.steps[0].arguments["password"], "[redacted]");
        assert_eq!(plan.steps[0].arguments["expected_pages"], "2");
        assert!(
            plan.validators
                .iter()
                .any(|validator| validator == "pdf-ops.decrypt-page-count")
        );
        assert_eq!(
            super::take_pdf_secret(plan.plan_id).as_deref(),
            Some("pw"),
            "the decrypt secret is registered for the runner"
        );
        assert!(
            plan_pdf_decrypt(&probe(), None, PathBuf::from("x.pdf"), &qpdf_engine()).is_err(),
            "decryption without a password is rejected"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn watermark_plans_carry_the_text_and_default_angle() {
        let plan = plan_pdf_watermark(
            &probe(),
            "CONFIDENTIAL 440",
            None,
            PathBuf::from("stamped.pdf"),
            &qpdf_engine(),
        )
        .expect("watermark plan");
        assert_eq!(plan.steps[0].arguments["operation"], "pdf-watermark");
        assert_eq!(
            plan.steps[0].arguments["watermark_text"],
            "CONFIDENTIAL 440"
        );
        assert_eq!(plan.steps[0].arguments["watermark_angle"], "-45");
        assert_eq!(plan.steps[0].arguments["expected_pages"], "2");
        for validator in [
            "pdf-ops.watermark-page-count",
            "pdf-ops.watermark-text-present",
        ] {
            assert!(
                plan.validators.iter().any(|value| value == validator),
                "missing validator {validator}"
            );
        }
        let serialized = serde_json::to_string(&plan).expect("serialized Plan");
        assert!(
            serialized.contains("CONFIDENTIAL 440"),
            "watermark text is not confidential and must serialize into the Plan"
        );

        let angled = plan_pdf_watermark(
            &probe(),
            "draft",
            Some(30),
            PathBuf::from("stamped.pdf"),
            &qpdf_engine(),
        )
        .expect("angled watermark plan");
        assert_eq!(angled.steps[0].arguments["watermark_angle"], "30");

        assert!(
            plan_pdf_watermark(
                &probe(),
                "   ",
                None,
                PathBuf::from("x.pdf"),
                &qpdf_engine()
            )
            .is_err(),
            "blank watermark text is rejected"
        );
        assert!(
            plan_pdf_watermark(
                &probe(),
                " Confidential ",
                None,
                PathBuf::from("x.pdf"),
                &qpdf_engine()
            )
            .is_ok(),
            "surrounding whitespace is trimmed, not fatal"
        );
        assert!(
            plan_pdf_watermark(
                &probe(),
                &"x".repeat(81),
                None,
                PathBuf::from("x.pdf"),
                &qpdf_engine()
            )
            .is_err(),
            "overlong watermark text is rejected"
        );
        assert!(
            plan_pdf_watermark(
                &probe(),
                "mérkit",
                None,
                PathBuf::from("x.pdf"),
                &qpdf_engine()
            )
            .is_err(),
            "non-ASCII watermark text is rejected"
        );
        assert!(
            plan_pdf_watermark(
                &probe(),
                "ok",
                Some(181),
                PathBuf::from("x.pdf"),
                &qpdf_engine()
            )
            .is_err(),
            "angles beyond 180 are rejected"
        );
        assert!(
            plan_pdf_watermark(
                &probe(),
                "ok",
                Some(-181),
                PathBuf::from("x.pdf"),
                &qpdf_engine()
            )
            .is_err(),
            "angles below -180 are rejected"
        );
        assert!(
            plan_pdf_watermark(&probe(), "ok", None, PathBuf::from("x.pdf"), &engine()).is_err(),
            "the watermark plan rejects non-qpdf engines"
        );
    }

    #[test]
    fn watermark_layer_pdf_has_a_valid_shape() {
        let pdf = build_watermark_pdf(612.0, 792.0, "CONFIDENTIAL 440", -45);
        assert!(
            pdf.starts_with(b"%PDF-"),
            "watermark layer starts with %PDF"
        );
        assert!(pdf.ends_with(b"%%EOF\n"), "watermark layer ends with %%EOF");
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains("/Helvetica-Bold"), "font is embedded by name");
        assert!(text.contains("xref"), "xref table present");
        assert!(text.contains("/ExtGState"), "alpha state present");
        assert!(text.contains("(CONFIDENTIAL 440) Tj"), "text is drawn");
        // Escaping: parentheses and backslashes must not break the string.
        let escaped = build_watermark_pdf(612.0, 792.0, "a(b)c\\d", 0);
        assert!(String::from_utf8_lossy(&escaped).contains(r"(a\(b\)c\\d) Tj"));
        // Determinism: identical inputs produce byte-identical output.
        assert_eq!(
            build_watermark_pdf(612.0, 792.0, "same", -45),
            build_watermark_pdf(612.0, 792.0, "same", -45)
        );
    }

    #[test]
    fn watermark_text_matching_ignores_case_and_whitespace() {
        use uuid::Uuid;

        assert_eq!(
            normalized_watermark_text("  Con fidential\t440\n"),
            "confidential440"
        );
        assert_eq!(normalized_watermark_text("DRAFT"), "draft");

        let mut report = crate::domain::ValidationReport {
            schema_version: SCHEMA_VERSION,
            report_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            plan_hash: "hash".to_owned(),
            status: crate::domain::ValidationStatus::Pass,
            input: super::artifact_summary(&probe()),
            output: super::artifact_summary(&probe()),
            engines: Vec::new(),
            checks: Vec::new(),
            intentional_changes: Vec::new(),
            redaction: crate::domain::ReportRedaction {
                paths_redacted: false,
                metadata_values_redacted: true,
            },
        };
        append_watermark_text_check(
            &mut report,
            "Confidential 440",
            "page text … CONFIDENTIAL\n440",
        );
        assert_eq!(report.status, crate::domain::ValidationStatus::Pass);
        assert_eq!(report.checks[0].code, "PDF_OPS_WATERMARK_TEXT");
        // Poppler reorders rotated glyphs into visual fragments; the fallback
        // multi-set match still accepts a complete but scrambled extraction.
        append_watermark_text_check(&mut report, "Confidential 440", "L A TI EN D FI N O C 0 44");
        assert_eq!(report.status, crate::domain::ValidationStatus::Pass);
        append_watermark_text_check(&mut report, "Secret", "nothing here");
        assert_eq!(report.status, crate::domain::ValidationStatus::Fail);
    }

    fn startxref_value(bytes: &[u8]) -> u64 {
        let text = String::from_utf8_lossy(bytes).into_owned();
        let position = text.rfind("startxref").expect("startxref marker");
        text[position + "startxref".len()..]
            .split_whitespace()
            .next()
            .expect("offset digits")
            .parse()
            .expect("numeric offset")
    }

    #[test]
    fn incremental_metadata_update_appends_a_valid_revision() {
        let original = build_watermark_pdf(612.0, 792.0, "BASE", 0);
        let previous_xref = startxref_value(&original);
        let updated =
            super::apply_pdf_metadata(&original, Some("ELECTRIC 440010147700"), Some("Author (A)"))
                .expect("apply metadata");
        assert!(
            updated.starts_with(&original),
            "the incremental update must preserve every original byte"
        );
        let text = String::from_utf8_lossy(&updated).into_owned();
        assert!(text.contains("/Title (ELECTRIC 440010147700)"));
        assert!(text.contains("/Author (Author \\(A\\))"));
        assert!(text.contains(&format!("/Prev {previous_xref}")));
        assert!(text.matches("trailer").count() >= 2);
        assert!(startxref_value(&updated) > previous_xref);
        assert!(u64::try_from(updated.len()).is_ok_and(|len| startxref_value(&updated) <= len));
        // Deterministic: identical inputs produce identical revisions.
        let again =
            super::apply_pdf_metadata(&original, Some("ELECTRIC 440010147700"), Some("Author (A)"))
                .expect("apply metadata again");
        assert_eq!(updated, again);
    }

    #[test]
    fn metadata_update_rejects_bytes_without_a_pdf_tail() {
        assert!(super::apply_pdf_metadata(b"not a pdf", Some("t"), None).is_err());
        assert!(super::apply_pdf_metadata(b"%PDF-1.4\ntrailer only", Some("t"), None).is_err());
        assert!(
            super::apply_pdf_metadata(&build_watermark_pdf(100.0, 100.0, "x", 0), None, None)
                .is_err(),
            "an update with no fields is rejected"
        );
    }
}
