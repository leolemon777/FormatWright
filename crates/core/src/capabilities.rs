use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::doctor::{EngineDiscoveryPolicy, inspect_engine_with_policy};
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};

const KNOWN_TARGETS: [&str; 22] = [
    "jpg", "png", "webp", "avif", "mp4", "mp3", "m4a", "wav", "gif", "pdf", "docx", "epub", "json",
    "csv", "yaml", "xml", "zip", "tar.gz", "md", "txt", "odt", "7z",
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RouteAvailability {
    pub target_format: String,
    pub available: bool,
    pub required_engines: Vec<String>,
    pub missing_engines: Vec<String>,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapabilitySnapshot {
    pub input_extension: Option<String>,
    pub routes: BTreeMap<String, RouteAvailability>,
}

impl CapabilitySnapshot {
    #[must_use]
    pub fn route(&self, target: &str) -> Option<&RouteAvailability> {
        self.routes.get(&normalize_target(target))
    }

    #[must_use]
    pub fn available_targets(&self) -> Vec<String> {
        self.routes
            .values()
            .filter(|route| route.available)
            .map(|route| route.target_format.clone())
            .collect()
    }
}

pub async fn capability_snapshot_for_input(
    input: &Path,
    policy: EngineDiscoveryPolicy,
) -> CapabilitySnapshot {
    let extension = input_extension(input);
    let mut lanes_by_target = BTreeMap::new();
    let supported_targets = supported_targets(extension.as_deref());
    for target in KNOWN_TARGETS {
        lanes_by_target.insert(
            target.to_owned(),
            if supported_targets.contains(target) {
                Some(route_engine_lanes(extension.as_deref(), target))
            } else {
                None
            },
        );
    }

    let unique_engines = lanes_by_target
        .values()
        .filter_map(Option::as_ref)
        .flatten()
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut available_engines = BTreeMap::new();
    for engine in unique_engines {
        available_engines.insert(
            engine.clone(),
            inspect_engine_with_policy(&engine, policy).await.is_ok(),
        );
    }

    let mut routes = BTreeMap::new();
    for (target, lanes) in lanes_by_target {
        let Some(lanes) = lanes else {
            routes.insert(
                target.clone(),
                RouteAvailability {
                    target_format: target,
                    available: false,
                    required_engines: Vec::new(),
                    missing_engines: Vec::new(),
                    message: "This input/target route is not supported.".to_owned(),
                },
            );
            continue;
        };
        let lane_missing = |lane: &[String]| {
            lane.iter()
                .filter(|engine| !available_engines.get(*engine).copied().unwrap_or(false))
                .cloned()
                .collect::<Vec<_>>()
        };
        let preferred = lanes.first().cloned().unwrap_or_default();
        let selected_lane = lanes.iter().find(|lane| lane_missing(lane).is_empty());
        let (required_engines, missing_engines, available) = match selected_lane {
            Some(lane) => (lane.clone(), Vec::new(), true),
            None => (preferred.clone(), lane_missing(&preferred), false),
        };
        let message = if available {
            if required_engines.is_empty() {
                "Available through the built-in structured engine.".to_owned()
            } else {
                format!("Available through {}.", required_engines.join(", "))
            }
        } else {
            format!(
                "Install or import an engine pack that provides: {}.",
                missing_engines.join(", ")
            )
        };
        routes.insert(
            target.clone(),
            RouteAvailability {
                target_format: target,
                available,
                required_engines,
                missing_engines,
                message,
            },
        );
    }
    CapabilitySnapshot {
        input_extension: extension,
        routes,
    }
}

/// Rejects a route unless it is supported and every required engine is
/// available under the selected discovery policy.
///
/// # Errors
///
/// Returns `UnsupportedConversion` for an unsupported pair and
/// `EngineMissing` when a supported pair lacks an activated engine.
pub async fn ensure_route_available(
    input: &Path,
    target: &str,
    policy: EngineDiscoveryPolicy,
) -> Result<RouteAvailability> {
    let snapshot = capability_snapshot_for_input(input, policy).await;
    let normalized = normalize_target(target);
    let route = snapshot.route(&normalized).cloned().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::Unsupported,
            Stage::Plan,
            format!("Unknown target format: {normalized}"),
            "Choose one of the target formats shown as available.",
        )
    })?;
    if route.available {
        return Ok(route);
    }
    let (code, action) = if route.missing_engines.is_empty() {
        (
            ErrorCode::Unsupported,
            "Choose a supported target for this input.",
        )
    } else {
        (
            ErrorCode::EngineMissing,
            "Open Engines and install or import the required verified pack.",
        )
    };
    Err(FormatWrightError::new(
        code,
        Stage::Plan,
        route.message,
        action,
    ))
}

fn supported_targets(input: Option<&str>) -> BTreeSet<&'static str> {
    let values: &[&str] = match input.unwrap_or_default() {
        "csv" | "json" | "yaml" | "yml" | "xml" => &["csv", "json", "yaml", "xml"],
        "pdf" | "heic" | "heif" => &["jpg", "png"],
        "docx" => &["pdf", "txt", "md", "html", "epub", "odt"],
        "odt" => &["pdf", "docx"],
        "pptx" | "xlsx" | "ods" | "odp" | "rtf" | "svg" => &["pdf"],
        "md" | "markdown" | "html" | "htm" | "txt" | "text" => &["pdf", "docx", "epub"],
        "zip" => &["tar.gz", "7z"],
        "tar.gz" | "7z" => &["zip"],
        "png" | "jpg" | "jpeg" => &["webp", "avif", "pdf", "txt"],
        "mov" | "mkv" | "avi" | "webm" | "mp4" => &["mp4", "gif", "mp3"],
        "wav" | "flac" | "aac" | "m4a" | "ogg" | "opus" | "mp3" => &["m4a", "mp3", "wav"],
        _ => &[],
    };
    values.iter().copied().collect()
}

/// Returns the ordered engine lanes that can serve one route. The first fully
/// available lane wins; later lanes are fallbacks.
fn route_engine_lanes(input: Option<&str>, target: &str) -> Vec<Vec<String>> {
    let target = normalize_target(target);
    let input = input.unwrap_or_default();
    if input == "svg" && target == "pdf" {
        return vec![browser_print_lane()];
    }
    if matches!(input, "html" | "htm") && target == "pdf" {
        return vec![
            browser_print_lane(),
            engine_names(&["pandoc", "soffice", "pdfinfo", "pdftoppm"]),
        ];
    }
    vec![required_engines(Some(input), &target)]
}

fn browser_print_lane() -> Vec<String> {
    engine_names(&["msedge", "pdfinfo", "pdftoppm", "pdftotext", "pdffonts"])
}

fn required_engines(input: Option<&str>, target: &str) -> Vec<String> {
    let target = normalize_target(target);
    let input = input.unwrap_or_default();
    if matches!(input, "csv" | "json" | "yaml" | "yml" | "xml")
        && matches!(target.as_str(), "csv" | "json" | "yaml" | "xml")
    {
        return Vec::new();
    }
    if matches!(input, "zip" | "tar.gz" | "7z")
        && matches!(target.as_str(), "zip" | "tar.gz" | "7z")
    {
        return Vec::new();
    }
    if input == "pdf" && matches!(target.as_str(), "jpg" | "png") {
        return engine_names(&["pdfinfo", "pdftoppm", "ffprobe"]);
    }
    if input == "docx" && matches!(target.as_str(), "txt" | "md" | "html" | "epub") {
        return engine_names(&["pandoc"]);
    }
    // 文档互换（docx <-> odt）只需 soffice；结构验收不依赖 Poppler。
    if (input == "docx" && target == "odt") || (input == "odt" && target == "docx") {
        return engine_names(&["soffice"]);
    }
    if matches!(
        input,
        "docx" | "pptx" | "xlsx" | "odt" | "ods" | "odp" | "rtf"
    ) && target == "pdf"
    {
        return engine_names(&["soffice", "pdfinfo", "pdftoppm"]);
    }
    if matches!(input, "md" | "markdown" | "html" | "htm" | "txt" | "text")
        && matches!(target.as_str(), "docx" | "epub")
    {
        return engine_names(&["pandoc"]);
    }
    if matches!(input, "html" | "htm" | "svg") && target == "pdf" {
        return browser_print_lane();
    }
    if matches!(input, "md" | "markdown" | "txt" | "text") && target == "pdf" {
        return engine_names(&["pandoc", "soffice", "pdfinfo", "pdftoppm"]);
    }
    if matches!(input, "png" | "jpg" | "jpeg") && target == "pdf" {
        return engine_names(&["soffice", "pdfinfo", "pdftoppm"]);
    }
    if matches!(input, "heic" | "heif") && matches!(target.as_str(), "jpg" | "png") {
        return engine_names(&["ffprobe", "heif-dec"]);
    }
    if matches!(input, "png" | "jpg" | "jpeg") && target == "txt" {
        return engine_names(&["ffprobe", "tesseract"]);
    }
    engine_names(&["ffprobe", "ffmpeg"])
}

#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn input_extension(path: &Path) -> Option<String> {
    // The name is lowercased first, so the comparisons are case-insensitive.
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") || name.ends_with(".taz") {
        return Some("tar.gz".to_owned());
    }
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase)
}

fn normalize_target(target: &str) -> String {
    match target
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .as_str()
    {
        "jpeg" => "jpg".to_owned(),
        "yml" => "yaml".to_owned(),
        "tgz" | "taz" => "tar.gz".to_owned(),
        value => value.to_owned(),
    }
}

fn engine_names(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use super::{
        CapabilitySnapshot, RouteAvailability, ensure_route_available, required_engines,
        route_engine_lanes, supported_targets,
    };
    use crate::{EngineDiscoveryPolicy, ErrorCode};

    #[test]
    fn pdf_routes_require_the_complete_starter_runtime() {
        assert_eq!(
            required_engines(Some("pdf"), "png"),
            ["pdfinfo", "pdftoppm", "ffprobe"]
        );
        assert_eq!(
            required_engines(Some("pdf"), "jpg"),
            ["pdfinfo", "pdftoppm", "ffprobe"]
        );
    }

    #[test]
    fn structured_routes_are_builtin() {
        assert!(required_engines(Some("json"), "yaml").is_empty());
        assert!(required_engines(Some("csv"), "xml").is_empty());
    }

    #[test]
    fn html_pdf_prefers_the_browser_print_lane_with_a_pandoc_fallback() {
        let lanes = route_engine_lanes(Some("html"), "pdf");
        assert_eq!(
            lanes[0],
            ["msedge", "pdfinfo", "pdftoppm", "pdftotext", "pdffonts"]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            lanes[1],
            ["pandoc", "soffice", "pdfinfo", "pdftoppm"]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            "the Pandoc lane remains a fallback for HTML"
        );
        assert_eq!(
            required_engines(Some("htm"), "pdf"),
            lanes[0],
            "required_engines reports the preferred lane"
        );
    }

    #[test]
    fn svg_pdf_requires_the_browser_print_lane() {
        let lanes = route_engine_lanes(Some("svg"), "pdf");
        assert_eq!(lanes.len(), 1, "SVG has no fallback lane");
        assert_eq!(
            lanes[0],
            ["msedge", "pdfinfo", "pdftoppm", "pdftotext", "pdffonts"]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
        let targets = supported_targets(Some("svg"));
        assert!(targets.contains("pdf"));
        assert!(!targets.contains("docx"));
    }

    #[test]
    fn markdown_pdf_keeps_the_pandoc_lane() {
        let lanes = route_engine_lanes(Some("md"), "pdf");
        assert_eq!(lanes.len(), 1);
        assert_eq!(
            lanes[0],
            ["pandoc", "soffice", "pdfinfo", "pdftoppm"]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn unsupported_pdf_targets_are_not_advertised() {
        let targets = supported_targets(Some("pdf"));
        assert!(targets.contains("png"));
        assert!(targets.contains("jpg"));
        assert!(!targets.contains("webp"));
        assert!(!targets.contains("mp4"));
    }

    #[test]
    fn docx_exports_route_through_pandoc_and_document_exchange_through_soffice() {
        let targets = supported_targets(Some("docx"));
        for target in ["pdf", "txt", "md", "html", "epub", "odt"] {
            assert!(targets.contains(target), "docx -> {target}");
        }
        assert_eq!(
            required_engines(Some("docx"), "txt"),
            ["pandoc"],
            "docx -> txt/md/html/epub only needs pandoc"
        );
        assert_eq!(required_engines(Some("docx"), "epub"), ["pandoc"]);
        assert_eq!(
            required_engines(Some("docx"), "odt"),
            ["soffice"],
            "document exchange avoids the poppler-only PDF validators"
        );
        assert_eq!(required_engines(Some("odt"), "docx"), ["soffice"]);
    }

    #[test]
    fn odt_targets_stay_within_pdf_and_docx() {
        let targets = supported_targets(Some("odt"));
        assert!(targets.contains("pdf"));
        assert!(targets.contains("docx"));
        assert!(!targets.contains("epub"));
    }

    #[tokio::test]
    async fn backend_rejects_an_unsupported_route_before_execution() {
        let error = ensure_route_available(
            Path::new("fixture.pdf"),
            "webp",
            EngineDiscoveryPolicy::VerifiedPacksOnly,
        )
        .await
        .expect_err("PDF to WebP is not a declared route");
        assert_eq!(error.code, ErrorCode::Unsupported);
    }

    #[tokio::test]
    async fn backend_reports_missing_pack_for_a_supported_pdf_route() {
        let error = ensure_route_available(
            Path::new("fixture.pdf"),
            "png",
            EngineDiscoveryPolicy::VerifiedPacksOnly,
        )
        .await
        .expect_err("PDF Starter engines are not registered in this test");
        assert_eq!(error.code, ErrorCode::EngineMissing);
        assert!(error.message.contains("pdfinfo"));
        assert!(error.message.contains("pdftoppm"));
        assert!(error.message.contains("ffprobe"));
    }

    #[test]
    fn snapshot_normalizes_target_lookup_and_lists_only_available_routes() {
        let mut routes = BTreeMap::new();
        routes.insert(
            "jpg".to_owned(),
            RouteAvailability {
                target_format: "jpg".to_owned(),
                available: true,
                required_engines: Vec::new(),
                missing_engines: Vec::new(),
                message: "available".to_owned(),
            },
        );
        routes.insert(
            "png".to_owned(),
            RouteAvailability {
                target_format: "png".to_owned(),
                available: false,
                required_engines: vec!["pdftoppm".to_owned()],
                missing_engines: vec!["pdftoppm".to_owned()],
                message: "missing".to_owned(),
            },
        );
        let snapshot = CapabilitySnapshot {
            input_extension: Path::new("fixture.pdf")
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .map(str::to_owned),
            routes,
        };
        assert!(snapshot.route("JPEG").is_some_and(|route| route.available));
        assert_eq!(snapshot.available_targets(), ["jpg"]);
    }
}
