use std::collections::BTreeMap;
use std::env;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use formatwright_engine_sdk::{
    Certification, DoctorReport, EngineHealth, EngineIdentity, SignatureTrust,
    SupplyChainReviewStatus,
};
use sha2::{Digest, Sha256};
use tokio::process::Command;

use crate::error::{ErrorCode, FormatWrightError, Result, Stage};

/// Controls which engine locations may participate in runtime discovery.
///
/// Production releases must be deterministic: only exact paths activated from
/// a hash-verified engine pack are eligible. Development builds may also use an
/// explicit environment override or PATH to make adapter development practical.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineDiscoveryPolicy {
    VerifiedPacksOnly,
    Development,
}

impl EngineDiscoveryPolicy {
    #[must_use]
    pub const fn for_current_build() -> Self {
        if cfg!(debug_assertions) {
            Self::Development
        } else {
            Self::VerifiedPacksOnly
        }
    }
}

#[derive(Clone, Debug)]
struct RegisteredEnginePath {
    path: PathBuf,
    manifest_sha256: String,
    pack_id: String,
    certification: Certification,
    signature_trust: Option<SignatureTrust>,
    review_status: SupplyChainReviewStatus,
}

static REGISTERED_ENGINE_PATHS: OnceLock<RwLock<BTreeMap<String, RegisteredEnginePath>>> =
    OnceLock::new();

pub async fn doctor() -> DoctorReport {
    doctor_with_policy(EngineDiscoveryPolicy::for_current_build()).await
}

/// Runs Doctor using an explicit engine-discovery policy.
#[must_use]
pub async fn doctor_with_policy(policy: EngineDiscoveryPolicy) -> DoctorReport {
    let mut engines = BTreeMap::new();
    for executable in [
        "ffmpeg",
        "ffprobe",
        "vips",
        "heif-dec",
        "pandoc",
        "soffice",
        "pdftoppm",
        "pdfinfo",
        "pdftotext",
        "pdffonts",
        "msedge",
        "qpdf",
        "tesseract",
    ] {
        let health = match inspect_engine_with_policy(executable, policy).await {
            Ok(identity) => {
                let registered = registered_engine_path(executable);
                EngineHealth {
                    executable: executable.to_owned(),
                    available: true,
                    identity: Some(identity),
                    error_code: None,
                    message: "Available".to_owned(),
                    signature_trust: registered
                        .as_ref()
                        .and_then(|engine| engine.signature_trust.clone()),
                    review_status: registered.as_ref().map(|engine| engine.review_status),
                }
            }
            Err(error) => EngineHealth {
                executable: executable.to_owned(),
                available: false,
                identity: None,
                error_code: Some(format!("{:?}", error.code)),
                message: error.message,
                signature_trust: None,
                review_status: None,
            },
        };
        engines.insert(executable.to_owned(), health);
    }
    DoctorReport { engines }
}

/// Inspects an engine selected by the current build policy and returns its
/// immutable identity.
///
/// # Errors
///
/// Returns an engine error when the executable is missing, cannot start, does
/// not expose a version, or cannot be hashed.
pub async fn inspect_engine(executable: &str) -> Result<EngineIdentity> {
    inspect_engine_with_policy(executable, EngineDiscoveryPolicy::for_current_build()).await
}

/// Inspects an engine using an explicit discovery policy.
///
/// # Errors
///
/// Returns an engine error when the selected policy cannot resolve a verified
/// executable or when version/hash inspection fails.
pub async fn inspect_engine_with_policy(
    executable: &str,
    policy: EngineDiscoveryPolicy,
) -> Result<EngineIdentity> {
    let (path, manifest_sha256) = resolve_engine_path(executable, policy).ok_or_else(|| {
        let message = if policy == EngineDiscoveryPolicy::VerifiedPacksOnly {
            format!("No activated verified engine pack provides: {executable}")
        } else {
            format!("Engine executable was not found: {executable}")
        };
        FormatWrightError::new(
            ErrorCode::EngineMissing,
            Stage::Doctor,
            message,
            "Install or import a certified engine pack, then run doctor again.",
        )
    })?;

    // Windows GUI-subsystem builds of Edge print nothing for `--version`, so the
    // browser is never probed through a subprocess; its identity is derived from
    // the installation layout instead.
    let version = if executable == "msedge" {
        installed_browser_version(&path).await?
    } else {
        let version_argument = match executable {
            "ffmpeg" | "ffprobe" => "-version",
            "pdftoppm" | "pdfinfo" | "pdftotext" | "pdffonts" => "-v",
            _ => "--version",
        };
        let output = Command::new(&path)
            .arg(version_argument)
            .output()
            .await
            .map_err(|error| {
                FormatWrightError::new(
                    ErrorCode::EngineIncompatible,
                    Stage::Doctor,
                    format!("Unable to start engine: {}", path.display()),
                    "Check that the executable matches this operating system and architecture.",
                )
                .with_diagnostic(error.to_string())
            })?;
        if !output.status.success() {
            return Err(FormatWrightError::new(
                ErrorCode::EngineIncompatible,
                Stage::Doctor,
                format!("Engine version check failed: {}", path.display()),
                "Install a supported engine build.",
            )
            .with_diagnostic(bounded_text(&output.stderr)));
        }
        let version_bytes = if output.stdout.is_empty() {
            &output.stderr
        } else {
            &output.stdout
        };
        String::from_utf8_lossy(version_bytes)
            .lines()
            .next()
            .unwrap_or("unknown")
            .trim()
            .to_owned()
    };
    let path_for_hash = path.clone();
    let binary_sha256 = tokio::task::spawn_blocking(move || sha256_file(&path_for_hash))
        .await
        .map_err(|error| {
            FormatWrightError::new(
                ErrorCode::Internal,
                Stage::Doctor,
                "Engine hash worker failed",
                "Retry doctor.",
            )
            .with_diagnostic(error.to_string())
        })??;

    let build_configuration = if executable.eq_ignore_ascii_case("ffmpeg") {
        read_build_configuration(&path).await.ok()
    } else {
        None
    };
    let certification = registered_engine_path(executable)
        .map_or(Certification::Unverified, |engine| engine.certification);

    Ok(EngineIdentity {
        engine_id: executable.to_ascii_lowercase(),
        version,
        binary_path: path,
        binary_sha256,
        manifest_sha256,
        build_configuration,
        certification,
    })
}

fn resolve_engine_path(
    executable: &str,
    policy: EngineDiscoveryPolicy,
) -> Option<(PathBuf, Option<String>)> {
    choose_engine_path(
        registered_engine_path(executable),
        policy,
        configured_engine_path(executable)
            .or_else(|| find_executable(executable))
            .or_else(|| known_install_location(executable)),
    )
}

/// Returns the canonical install location of the system browser engine.
///
/// Only the engine names with a well-known vendor layout participate; every
/// other executable keeps relying on packs, environment overrides, or PATH.
fn known_install_location(executable: &str) -> Option<PathBuf> {
    if executable != "msedge" {
        return None;
    }

    #[cfg(windows)]
    {
        let mut candidates = Vec::new();
        for variable in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Some(directory) = env::var_os(variable) {
                candidates
                    .push(PathBuf::from(directory).join(r"Microsoft\Edge\Application\msedge.exe"));
            }
        }
        candidates.into_iter().find(|path| is_executable_file(path))
    }

    #[cfg(target_os = "macos")]
    {
        macos_msedge_install_locations()
            .into_iter()
            .find(|path| is_executable_file(path))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Edge first; any Chromium-family build backs the same print lane
        // (ADR-0012: system discovery only, never bundled).
        [
            "/usr/bin/microsoft-edge",
            "/usr/bin/microsoft-edge-stable",
            "/opt/microsoft/edge/microsoft-edge",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/opt/google/chrome/chrome",
            "/snap/bin/chromium",
        ]
        .iter()
        .map(PathBuf::from)
        .find(|path| is_executable_file(path))
    }
}

/// Joins every macOS `Applications` root with the Edge bundle's executable
/// path.
///
/// Edge keeps the Chromium application-bundle layout, so the binary always
/// lives at `<root>/Microsoft Edge.app/Contents/MacOS/Microsoft Edge`. Kept as
/// a pure path computation so the layout stays unit-testable on every host.
#[cfg(any(target_os = "macos", test))]
fn macos_browser_bundle_paths(applications_roots: &[PathBuf]) -> Vec<PathBuf> {
    // Edge first; Chromium-family apps back the same print lane. Every
    // bundle keeps the identical Chromium executable layout.
    const APPS: [(&str, &str); 3] = [
        ("Microsoft Edge.app", "Microsoft Edge"),
        ("Google Chrome.app", "Google Chrome"),
        ("Chromium.app", "Chromium"),
    ];
    let mut paths = Vec::new();
    for root in applications_roots {
        for (app, executable) in APPS {
            paths.push(
                root.join(app)
                    .join("Contents")
                    .join("MacOS")
                    .join(executable),
            );
        }
    }
    paths
}

/// Standard macOS roots that may hold an Edge installation: the system-wide
/// `/Applications` and the per-user `~/Applications`.
#[cfg(target_os = "macos")]
fn macos_msedge_install_locations() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(home) = env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }
    macos_browser_bundle_paths(&roots)
}

/// Derives the Edge version from the versioned directory the installer keeps
/// beside the executable, without launching the browser.
///
/// # Errors
///
/// Returns an internal error when the version-directory worker fails.
async fn installed_browser_version(path: &Path) -> Result<String> {
    let owned = path.to_owned();
    let version = tokio::task::spawn_blocking(move || {
        let Some(parent) = owned.parent() else {
            return Ok::<Option<String>, FormatWrightError>(None);
        };
        let best = std::fs::read_dir(parent)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter_map(|name| version_directory_key(&name).map(|key| (key, name)))
            .max_by(|(left, _), (right, _)| left.cmp(right))
            .map(|(_, name)| name);
        Ok::<Option<String>, FormatWrightError>(best)
    })
    .await
    .map_err(|error| {
        FormatWrightError::new(
            ErrorCode::Internal,
            Stage::Doctor,
            "Browser version worker failed",
            "Retry doctor.",
        )
        .with_diagnostic(error.to_string())
    })??;
    Ok(version.unwrap_or_else(|| "unknown".to_owned()))
}

fn version_directory_key(name: &str) -> Option<(u64, u64, u64, u64)> {
    let mut parts = name.split('.');
    let quad = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(quad)
}

fn choose_engine_path(
    registered: Option<RegisteredEnginePath>,
    policy: EngineDiscoveryPolicy,
    development_candidate: Option<PathBuf>,
) -> Option<(PathBuf, Option<String>)> {
    if let Some(engine) = registered {
        return Some((engine.path, Some(engine.manifest_sha256)));
    }
    if policy == EngineDiscoveryPolicy::VerifiedPacksOnly {
        return None;
    }
    development_candidate.map(|path| (path, None))
}

pub(crate) fn register_engine_pack_paths_with_provenance(
    executables: &BTreeMap<String, PathBuf>,
    manifest_sha256: &str,
    pack_id: &str,
    certification: Certification,
    signature_trust: Option<&SignatureTrust>,
    review_status: SupplyChainReviewStatus,
) -> Result<()> {
    let mut paths = REGISTERED_ENGINE_PATHS
        .get_or_init(|| RwLock::new(BTreeMap::new()))
        .write()
        .map_err(|_| {
            FormatWrightError::new(
                ErrorCode::Internal,
                Stage::Doctor,
                "Engine registry lock was poisoned",
                "Restart FormatWright and import the engine pack again.",
            )
        })?;
    for (name, path) in executables {
        if !is_executable_file(path) {
            return Err(FormatWrightError::new(
                ErrorCode::EngineIncompatible,
                Stage::Doctor,
                format!(
                    "Imported engine executable is unavailable: {}",
                    path.display()
                ),
                "Restore the engine pack or import it again.",
            ));
        }
        if let Some(existing) = paths.get(&name.to_ascii_lowercase())
            && existing.pack_id != pack_id
        {
            return Err(FormatWrightError::new(
                ErrorCode::EngineIncompatible,
                Stage::Doctor,
                format!("Engine executable name is already claimed: {name}"),
                "Keep one verified pack for each executable name, then restart FormatWright.",
            ));
        }
    }
    paths.retain(|_, existing| existing.pack_id != pack_id);
    for (name, path) in executables {
        paths.insert(
            name.to_ascii_lowercase(),
            RegisteredEnginePath {
                path: path.clone(),
                manifest_sha256: manifest_sha256.to_owned(),
                pack_id: pack_id.to_owned(),
                certification,
                signature_trust: signature_trust.cloned(),
                review_status,
            },
        );
    }
    Ok(())
}

fn registered_engine_path(executable: &str) -> Option<RegisteredEnginePath> {
    REGISTERED_ENGINE_PATHS
        .get()?
        .read()
        .ok()?
        .get(&executable.to_ascii_lowercase())
        .cloned()
}

#[cfg(test)]
pub(crate) fn registered_engine_metadata(executable: &str) -> Option<(PathBuf, String)> {
    registered_engine_path(executable).map(|engine| (engine.path, engine.manifest_sha256))
}

#[cfg(test)]
pub(crate) fn registered_engine_provenance(
    executable: &str,
) -> Option<(
    Certification,
    Option<SignatureTrust>,
    SupplyChainReviewStatus,
)> {
    registered_engine_path(executable).map(|engine| {
        (
            engine.certification,
            engine.signature_trust,
            engine.review_status,
        )
    })
}

fn configured_engine_path(executable: &str) -> Option<PathBuf> {
    let key = format!(
        "FORMATWRIGHT_ENGINE_{}",
        executable
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect::<String>()
    );
    env::var_os(key)
        .map(PathBuf::from)
        .filter(|path| is_executable_file(path))
        .and_then(|path| path.canonicalize().ok().or(Some(path)))
}

/// Returns the immutable identity of a Rust-native in-process adapter.
///
/// # Errors
///
/// Returns an engine error when the current executable cannot be located or
/// hashed.
pub async fn inspect_builtin_engine(engine_id: &str) -> Result<EngineIdentity> {
    let path = std::env::current_exe().map_err(|error| {
        FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Doctor,
            "Unable to locate the FormatWright executable",
            "Restart FormatWright and run doctor again.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let path_for_hash = path.clone();
    let binary_sha256 = tokio::task::spawn_blocking(move || sha256_file(&path_for_hash))
        .await
        .map_err(|error| {
            FormatWrightError::new(
                ErrorCode::Internal,
                Stage::Doctor,
                "Built-in engine hash worker failed",
                "Retry the operation.",
            )
            .with_diagnostic(error.to_string())
        })??;
    Ok(EngineIdentity {
        engine_id: engine_id.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        binary_path: path,
        binary_sha256,
        manifest_sha256: None,
        build_configuration: Some("rust-native; structured-contract=v1".to_owned()),
        certification: Certification::Experimental,
    })
}

pub fn find_executable(executable: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(executable);
    if (candidate.is_absolute() || candidate.components().count() > 1)
        && is_executable_file(&candidate)
    {
        return candidate.canonicalize().ok().or(Some(candidate));
    }

    let path = env::var_os("PATH")?;
    let extensions = executable_extensions(&candidate);
    for directory in env::split_paths(&path) {
        for extension in &extensions {
            let mut path = directory.join(&candidate);
            if !extension.is_empty() {
                path.set_extension(extension.trim_start_matches('.'));
            }
            if is_executable_file(&path) {
                return path.canonicalize().ok().or(Some(path));
            }
        }
    }
    None
}

fn executable_extensions(candidate: &Path) -> Vec<String> {
    if candidate.extension().is_some() {
        return vec![String::new()];
    }

    #[cfg(windows)]
    {
        let mut extensions = vec![String::new()];
        let path_ext = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
        extensions.extend(
            path_ext
                .split(';')
                .filter(|value| !value.is_empty())
                .map(str::to_ascii_lowercase),
        );
        extensions
    }

    #[cfg(not(windows))]
    {
        vec![String::new()]
    }
}

fn is_executable_file(path: &Path) -> bool {
    path.metadata().is_ok_and(|metadata| metadata.is_file())
}

async fn read_build_configuration(path: &Path) -> Result<String> {
    let output = Command::new(path)
        .arg("-buildconf")
        .output()
        .await
        .map_err(|error| {
            FormatWrightError::new(
                ErrorCode::EngineIncompatible,
                Stage::Doctor,
                "Unable to read engine build configuration",
                "Use another engine build.",
            )
            .with_diagnostic(error.to_string())
        })?;
    if !output.status.success() {
        return Err(FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Doctor,
            "Engine did not expose its build configuration",
            "Use a build that supports configuration inspection.",
        ));
    }
    Ok(bounded_text(&output.stdout))
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::EngineIncompatible,
            Stage::Doctor,
            format!("Cannot open engine binary: {}", path.display()),
            "Check engine file permissions.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            FormatWrightError::new(
                ErrorCode::EngineIncompatible,
                Stage::Doctor,
                format!("Cannot hash engine binary: {}", path.display()),
                "Check storage health and retry.",
            )
            .with_diagnostic(error.to_string())
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn bounded_text(bytes: &[u8]) -> String {
    const MAX_BYTES: usize = 64 * 1024;
    let start = bytes.len().saturating_sub(MAX_BYTES);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use std::path::PathBuf;

    use super::{
        EngineDiscoveryPolicy, RegisteredEnginePath, choose_engine_path, find_executable,
        inspect_engine_with_policy, known_install_location, macos_browser_bundle_paths,
        resolve_engine_path, version_directory_key,
    };
    use formatwright_engine_sdk::SupplyChainReviewStatus;

    #[test]
    fn known_install_locations_cover_only_the_browser_engine() {
        assert!(known_install_location("ffmpeg").is_none());
        assert!(known_install_location("soffice").is_none());
    }

    #[test]
    fn macos_browser_bundle_paths_follow_the_chromium_bundle_layout() {
        let paths = macos_browser_bundle_paths(&[PathBuf::from("/Applications")]);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
                PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
                PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
            ]
        );
    }

    #[test]
    fn macos_browser_bundle_paths_cover_every_applications_root() {
        let paths = macos_browser_bundle_paths(&[
            PathBuf::from("/Applications"),
            PathBuf::from("/Users/demo/Applications"),
        ]);
        assert_eq!(paths.len(), 6);
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("Microsoft Edge.app/Contents/MacOS/Microsoft Edge"))
        );
        assert!(paths.iter().any(|path| {
            path.ends_with("Users/demo/Applications/Chromium.app/Contents/MacOS/Chromium")
        }));
    }

    #[test]
    fn version_directory_keys_accept_only_four_numeric_segments() {
        assert_eq!(
            version_directory_key("138.0.3351.95"),
            Some((138, 0, 3351, 95))
        );
        assert_eq!(version_directory_key("138.0.3351.95.beta"), None);
        assert_eq!(version_directory_key("Application"), None);
    }

    #[test]
    fn finds_current_test_executable_by_explicit_path() {
        let current = std::env::current_exe().expect("current test executable");
        let found = find_executable(current.to_string_lossy().as_ref());
        assert!(found.is_some());
    }

    #[test]
    fn rejects_ambiguous_pack_claims_for_one_executable_name() {
        let current = std::env::current_exe().expect("current test executable");
        let mut first = BTreeMap::new();
        first.insert("collision-fixture".to_owned(), current.clone());
        super::register_engine_pack_paths_with_provenance(
            &first,
            "manifest-a",
            "first-pack",
            crate::Certification::Unverified,
            None,
            formatwright_engine_sdk::SupplyChainReviewStatus::Missing,
        )
        .expect("first registration");

        let mut second = BTreeMap::new();
        second.insert("collision-fixture".to_owned(), current);
        let error = super::register_engine_pack_paths_with_provenance(
            &second,
            "manifest-b",
            "second-pack",
            crate::Certification::Unverified,
            None,
            formatwright_engine_sdk::SupplyChainReviewStatus::Missing,
        )
        .expect_err("a second manifest must not replace the first");
        assert!(error.message.contains("already claimed"));
    }

    #[tokio::test]
    async fn production_policy_ignores_explicit_non_pack_paths() {
        let current = std::env::current_exe().expect("current test executable");
        let executable = current.to_string_lossy();
        assert!(find_executable(&executable).is_some());

        let error =
            inspect_engine_with_policy(&executable, EngineDiscoveryPolicy::VerifiedPacksOnly)
                .await
                .expect_err("production policy must ignore non-pack paths");
        assert_eq!(error.code, crate::ErrorCode::EngineMissing);
        assert!(error.message.contains("activated verified engine pack"));
    }

    #[test]
    fn production_policy_selects_an_exact_registered_pack_path() {
        let executable = "strict-pack-fixture";
        let current = std::env::current_exe().expect("current test executable");
        let mut paths = BTreeMap::new();
        paths.insert(executable.to_owned(), current.clone());
        super::register_engine_pack_paths_with_provenance(
            &paths,
            "strict-pack-manifest",
            "strict-pack",
            crate::Certification::Unverified,
            None,
            formatwright_engine_sdk::SupplyChainReviewStatus::Missing,
        )
        .expect("register exact pack path");

        let (path, manifest_sha256) =
            resolve_engine_path(executable, EngineDiscoveryPolicy::VerifiedPacksOnly)
                .expect("production policy should select the registered exact path");
        assert_eq!(path, current);
        assert_eq!(manifest_sha256.as_deref(), Some("strict-pack-manifest"));
    }

    #[test]
    fn registered_provenance_is_visible_to_doctor_without_promoting_hashes() {
        use formatwright_engine_sdk::{Certification, SignatureTrust, SupplyChainReviewStatus};

        let current = std::env::current_exe().expect("current test executable");
        let mut paths = BTreeMap::new();
        paths.insert("provenance-fixture".to_owned(), current);
        let trust = SignatureTrust::Trusted {
            key_id: "release-2026h2".to_owned(),
        };
        super::register_engine_pack_paths_with_provenance(
            &paths,
            "provenance-manifest",
            "provenance-pack",
            Certification::Unverified,
            Some(&trust),
            SupplyChainReviewStatus::Incomplete,
        )
        .expect("register provenance");

        let (certification, trust, review) =
            super::registered_engine_provenance("provenance-fixture")
                .expect("registered provenance");
        assert_eq!(certification, Certification::Unverified);
        assert_eq!(
            trust,
            Some(SignatureTrust::Trusted {
                key_id: "release-2026h2".to_owned()
            })
        );
        assert_eq!(review, SupplyChainReviewStatus::Incomplete);
    }

    #[test]
    fn production_policy_ignores_development_overrides_and_polluted_paths() {
        let pack = PathBuf::from("C:/verified/pack/ffmpeg.exe");
        let hostile = PathBuf::from("C:/hostile/ffmpeg.exe");
        let registered = RegisteredEnginePath {
            path: pack.clone(),
            manifest_sha256: "aa".repeat(32),
            pack_id: "verified-pack".to_owned(),
            certification: crate::Certification::Unverified,
            signature_trust: None,
            review_status: SupplyChainReviewStatus::Missing,
        };

        let (release_path, manifest) = choose_engine_path(
            Some(registered.clone()),
            EngineDiscoveryPolicy::VerifiedPacksOnly,
            Some(hostile.clone()),
        )
        .expect("registered pack is eligible in Release");
        assert_eq!(release_path, pack);
        assert_eq!(
            manifest.as_deref(),
            Some(registered.manifest_sha256.as_str())
        );

        let (development_path, _) = choose_engine_path(
            Some(registered),
            EngineDiscoveryPolicy::Development,
            Some(hostile.clone()),
        )
        .expect("registered pack still wins in Development");
        assert_eq!(development_path, pack);

        assert!(
            choose_engine_path(
                None,
                EngineDiscoveryPolicy::VerifiedPacksOnly,
                Some(hostile)
            )
            .is_none(),
            "Release must ignore PATH and FORMATWRIGHT_ENGINE_* candidates"
        );
    }
}
