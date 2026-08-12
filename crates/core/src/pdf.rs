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
    let path = path.as_ref();
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
    let summary = run_pdfinfo(pdfinfo, &["-enc", "UTF-8"], &artifact.canonical_path).await?;
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
    if parse_field(&summary, "Encrypted")
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("yes"))
    {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            Stage::Inspect,
            "Encrypted PDFs are not accepted by the alpha renderer",
            "Decrypt an authorized copy locally, then retry.",
        ));
    }

    let details = run_pdfinfo(
        pdfinfo,
        &[
            "-box",
            "-f",
            "1",
            "-l",
            &page_count.to_string(),
            "-enc",
            "UTF-8",
        ],
        &artifact.canonical_path,
    )
    .await?;
    let pages = parse_page_details(&details, page_count)?;
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
    let version = parse_field(&summary, "PDF version").unwrap_or("unknown");
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

async fn run_pdfinfo(engine: &EngineIdentity, arguments: &[&str], path: &Path) -> Result<String> {
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
            let width = rounded_dimension(width * scale)?;
            let height = rounded_dimension(height * scale)?;
            Ok([width, height])
        })
        .collect()
}

fn rounded_dimension(value: f64) -> Result<u32> {
    if !value.is_finite() || !(1.0..=16_384.0).contains(&value) {
        return Err(FormatWrightError::new(
            ErrorCode::ResourceExhausted,
            Stage::Plan,
            "Rendered PDF page dimension is outside the alpha limit",
            "Choose a lower DPI or split unusual pages into another document.",
        ));
    }
    value.round().to_string().parse::<u32>().map_err(|error| {
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use formatwright_engine_sdk::{Certification, EngineIdentity};

    use super::{parse_page_details, plan_pdf_render};
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
}
