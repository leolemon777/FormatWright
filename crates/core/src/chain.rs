//! 转换链（conversion chaining）：当直接路由不存在时，自动经一个
//! 白名单中间格式两段完成（例如 xlsx → pdf → jpg）。
//!
//! 诚实语义：每段各自走完整的 probe → plan → execute → 验收流程；
//! 面向用户的报告以末段验收为主，链描述（"via intermediate PDF"）由
//! [`ConversionChain::description`] 提供。第二段的 Plan 必须在第一段
//! 落盘后才能生成（probe 需要真实的中间文件），因此链的各段 Plan
//! 是执行期惰性构建的，而不是在准备期一次性拼装。

use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::capabilities::{normalize_target, required_engines, supported_targets};
use crate::domain::PlanRequest;
use crate::error::{ErrorCode, FormatWrightError, Result, Stage};
use crate::runner::{ExecutionResult, execute_plan};
use crate::workflow::prepare_conversion;

/// 链式搜索的深度上限：一段直接路由 + 至多一段中间格式。
const MAX_CHAIN_DEPTH: usize = 2;

/// 无损中转白名单：只有这些格式允许充当中间节点。
///
/// 理由：
/// - 文本/结构化格式（txt/json/csv/yaml/xml/html）与 docx/pdf 的内容
///   表示是可往返的，不引入媒体级有损压缩；
/// - png 是无损位图；jpg 虽是有损图像格式，但作为“渲染终点型”中间
///   格式（office→jpg→pdf）业界通行且损耗可控，故按规格纳入；
/// - wav 是无损音频；
/// - tiff 与 png 同为无损位图，且是 PSD/RAW 磁带出口（C1 后入列）；
/// - mp3/mp4 等有损媒体、zip/7z 等归档容器不作中转（归档展开语义
///   与单文件转换不符）。
const INTERMEDIATE_WHITELIST: [&str; 12] = [
    "html", "png", "jpg", "tiff", "pdf", "docx", "txt", "json", "csv", "yaml", "xml", "wav",
];

/// 一段转换边：from → to，附该边首选 lane 的引擎需求。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChainHop {
    pub from: String,
    pub to: String,
    pub required_engines: Vec<String>,
}

/// 一次链式转换的完整路径（1..=2 段）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversionChain {
    hops: Vec<ChainHop>,
}

impl ConversionChain {
    #[must_use]
    pub fn hops(&self) -> &[ChainHop] {
        &self.hops
    }

    /// 面向用户的链描述，例如 `xlsx -> jpg via intermediate PDF`。
    #[must_use]
    pub fn description(&self) -> String {
        match self.hops.as_slice() {
            [first, second] => format!(
                "{} -> {} via intermediate {}",
                first.from,
                second.to,
                first.to.to_uppercase()
            ),
            [only] => format!("{} -> {}", only.from, only.to),
            _ => "empty conversion chain".to_owned(),
        }
    }
}

/// 在路由图中做 BFS 最短路径搜索（深度上限 2）。
///
/// 直接路由存在时返回 `None`：链只在直接路由缺失时启用，保证现有
/// 单段路径的行为完全不变。
#[must_use]
pub fn find_conversion_chain(input: &Path, target: &str) -> Option<ConversionChain> {
    let target = normalize_target(target);
    let input_ext = input_extension(input)?;
    let start = normalize_target(&input_ext);
    if start == target {
        return None;
    }
    // 直接路由仍然优先于链。
    if neighbors(&start).contains(&target) {
        return None;
    }
    // BFS：队列元素为已走节点路径（含起点）。最终路径至多
    // MAX_CHAIN_DEPTH + 1 个节点（两段 hop）。
    let mut queue = std::collections::VecDeque::from(vec![vec![start.clone()]]);
    while let Some(path) = queue.pop_front() {
        let node = path.last().cloned().unwrap_or_default();
        for next in neighbors(&node) {
            if next == node || next.is_empty() || path.contains(&next) {
                continue;
            }
            let mut extended = path.clone();
            extended.push(next.clone());
            if next == target {
                return Some(ConversionChain {
                    hops: to_hops(&extended),
                });
            }
            // 中间节点必须是白名单里的无损中转格式，且还有剩余深度。
            if INTERMEDIATE_WHITELIST.contains(&next.as_str()) && extended.len() <= MAX_CHAIN_DEPTH
            {
                queue.push_back(extended);
            }
        }
    }
    None
}

fn to_hops(path: &[String]) -> Vec<ChainHop> {
    path.windows(2)
        .map(|pair| ChainHop {
            from: pair[0].clone(),
            to: pair[1].clone(),
            required_engines: required_engines(Some(&pair[0]), &pair[1]),
        })
        .collect()
}

/// 返回一个格式（已归一化）的所有直接可达目标（同样归一化）。
fn neighbors(format: &str) -> Vec<String> {
    supported_targets(Some(format))
        .iter()
        .map(|value| normalize_target(value))
        .collect()
}

// The name is lowercased first, so the comparisons are case-insensitive.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn input_extension(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") || name.ends_with(".taz") {
        return Some("tar.gz".to_owned());
    }
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase)
}

/// 顺序执行一条转换链：每段各自 prepare + execute + 验收，中间文件
/// 放在与最终输出同目录的隐藏 staging 目录（`.fw-chain-<id>`），任一
/// 段失败即整链失败并清理 staging；全部成功后清理 staging、保留末段
/// 输出（末段直接 commit 到最终路径，天然保持 no-clobber 语义——
/// 开始前先检查最终输出不存在，末段的 `execute_plan` 会再次检查）。
///
/// # Errors
///
/// 返回任一段的 probe/plan/execute/验收错误，或最终输出已存在时的
/// `OutputConflict`。
pub async fn execute_conversion_chain(
    input: &Path,
    request: &PlanRequest,
    chain: &ConversionChain,
    job_id: Uuid,
    cancellation: CancellationToken,
) -> Result<ExecutionResult> {
    let final_output = request.output_path.clone().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "Chained conversion requires an output path",
            "Choose an output path.",
        )
    })?;
    if final_output.exists() {
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Commit,
            format!("Output already exists: {}", final_output.display()),
            "Choose another output path or an explicit conflict policy.",
        ));
    }
    let staging = chain_staging_dir(&final_output, job_id)?;
    if staging.exists() {
        return Err(FormatWrightError::new(
            ErrorCode::OutputConflict,
            Stage::Execute,
            format!(
                "Chain staging directory already exists: {}",
                staging.display()
            ),
            "Inspect and remove the leftover chain staging directory.",
        ));
    }
    std::fs::create_dir(&staging).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Execute,
            format!(
                "Unable to create chain staging directory: {}",
                staging.display()
            ),
            "Choose an output directory you can write to.",
        )
        .with_diagnostic(error.to_string())
    })?;

    let outcome =
        run_chain_segments(input, request, chain, &final_output, &staging, cancellation).await;
    // 无论成败都清理 staging：成功时中间文件已无用，失败时不能留垃圾。
    if let Err(cleanup_error) = std::fs::remove_dir_all(&staging)
        && outcome.is_ok()
    {
        return Err(FormatWrightError::new(
            ErrorCode::StorageFailed,
            Stage::Commit,
            format!(
                "Unable to remove chain staging directory: {}",
                staging.display()
            ),
            "Remove the leftover staging directory manually.",
        )
        .with_diagnostic(cleanup_error.to_string()));
    }
    outcome
}

async fn run_chain_segments(
    input: &Path,
    request: &PlanRequest,
    chain: &ConversionChain,
    final_output: &Path,
    staging: &Path,
    cancellation: CancellationToken,
) -> Result<ExecutionResult> {
    let mut segment_input = input.to_path_buf();
    let hop_count = chain.hops.len();
    let mut last_result = None;
    for (index, hop) in chain.hops().iter().enumerate() {
        let is_final = index + 1 == hop_count;
        let segment_output = if is_final {
            final_output.to_path_buf()
        } else {
            // 中间文件名必须带 hop 目标扩展名：后续段的 probe/plan 靠
            // 扩展名识别格式。
            staging.join(format!("hop-{index}.{}", hop.to))
        };
        let mut segment_request = request.clone();
        segment_request.target_format = hop.to.clone();
        segment_request.output_path = Some(segment_output);
        // 链只承载普通转换：操作型请求（pdf-merge 等）不走链。
        segment_request.operation = None;
        let (probe, plan, validation_engine) =
            prepare_conversion(&segment_input, &segment_request).await?;
        let result = execute_plan(
            &probe,
            &plan,
            &validation_engine,
            Uuid::new_v4(),
            cancellation.clone(),
        )
        .await?;
        segment_input.clone_from(&result.output_path);
        last_result = Some(result);
    }
    last_result.ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::Internal,
            Stage::Plan,
            "Conversion chain has no segments",
            "Create a new Plan and retry.",
        )
    })
}

fn chain_staging_dir(final_output: &Path, job_id: Uuid) -> Result<PathBuf> {
    let parent = final_output.parent().ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            Stage::Plan,
            "Resolved output path has no parent directory",
            "Choose a complete output path.",
        )
    })?;
    // Deterministic in the job id so crash recovery can find and clean it
    // through staged_output_candidates like every other staging path.
    Ok(parent.join(format!(".fw-chain-{job_id}")))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::tempdir;

    use super::{INTERMEDIATE_WHITELIST, execute_conversion_chain, find_conversion_chain};
    use crate::PlanRequest;

    fn chain_of(input: &str, target: &str) -> Option<Vec<(String, String)>> {
        find_conversion_chain(Path::new(&format!("fixture.{input}")), target).map(|chain| {
            chain
                .hops()
                .iter()
                .map(|hop| (hop.from.clone(), hop.to.clone()))
                .collect()
        })
    }

    #[test]
    fn direct_routes_prefer_the_existing_single_segment_path() {
        assert_eq!(chain_of("xlsx", "pdf"), None, "xlsx -> pdf is direct");
        assert_eq!(chain_of("json", "yaml"), None);
    }

    #[test]
    fn chains_through_whitelisted_intermediates_are_found() {
        // xlsx -> pdf -> jpg：pdf 在白名单内。
        assert_eq!(
            chain_of("xlsx", "jpg"),
            Some(vec![
                ("xlsx".to_owned(), "pdf".to_owned()),
                ("pdf".to_owned(), "jpg".to_owned())
            ])
        );
        // heic -> jpg -> pdf：jpg 在白名单内。
        assert_eq!(
            chain_of("heic", "pdf"),
            Some(vec![
                ("heic".to_owned(), "jpg".to_owned()),
                ("jpg".to_owned(), "pdf".to_owned())
            ])
        );
    }

    #[test]
    fn targets_without_any_two_hop_path_return_none() {
        assert_eq!(chain_of("xlsx", "mp3"), None, "xlsx 无法两跳到 mp3");
        assert_eq!(chain_of("json", "mp4"), None);
    }

    #[test]
    fn non_whitelisted_intermediates_are_rejected() {
        // 有损媒体与归档容器绝不作为中间节点。
        for forbidden in ["mp3", "mp4", "gif", "webp", "zip", "7z", "tar.gz", "epub"] {
            assert!(
                !INTERMEDIATE_WHITELIST.contains(&forbidden),
                "{forbidden} must not be an intermediate"
            );
        }
        // docx 的白名单中转（pdf/txt/html）都到不了 csv。
        assert_eq!(chain_of("docx", "csv"), None);
    }

    #[test]
    fn depth_is_capped_at_one_intermediate() {
        // png 的白名单中转是 pdf/txt；pdf 只到 jpg/png，txt 只到
        // pdf/docx/epub，第二跳均到不了 mp3。
        assert_eq!(chain_of("png", "mp3"), None, "png -> ... -> mp3 超出深度 2");
    }

    #[test]
    fn aliases_are_normalized_on_both_ends() {
        assert_eq!(
            chain_of("xlsx", "jpeg"),
            Some(vec![
                ("xlsx".to_owned(), "pdf".to_owned()),
                ("pdf".to_owned(), "jpg".to_owned())
            ]),
            "jpeg 归一化为 jpg"
        );
    }

    #[test]
    fn description_names_the_intermediate_format() {
        let chain = find_conversion_chain(Path::new("fixture.xlsx"), "jpg").expect("chain exists");
        assert_eq!(chain.description(), "xlsx -> jpg via intermediate PDF");
    }

    #[test]
    fn hop_records_the_engine_requirements_of_each_edge() {
        let chain = find_conversion_chain(Path::new("fixture.heic"), "pdf").expect("chain exists");
        let hops = chain.hops();
        assert_eq!(hops[0].required_engines, vec!["ffprobe", "heif-dec"]);
        assert!(!hops[1].required_engines.is_empty());
    }

    #[tokio::test]
    async fn chain_execution_requires_an_output_path() {
        let chain = find_conversion_chain(Path::new("fixture.xlsx"), "jpg").expect("chain exists");
        let request = PlanRequest {
            target_format: "jpg".to_owned(),
            ..PlanRequest::default()
        };
        let error = execute_conversion_chain(
            Path::new("fixture.xlsx"),
            &request,
            &chain,
            uuid::Uuid::new_v4(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect_err("no output path");
        assert_eq!(error.code, crate::ErrorCode::InputInvalid);
    }

    #[tokio::test]
    async fn chain_reserves_the_final_output_before_running_any_segment() {
        let directory = tempdir().expect("temporary chain workspace");
        let final_output = directory.path().join("out.jpg");
        std::fs::write(&final_output, b"existing").expect("existing output");
        let chain = find_conversion_chain(Path::new("fixture.xlsx"), "jpg").expect("chain exists");
        let request = PlanRequest {
            target_format: "jpg".to_owned(),
            output_path: Some(final_output.clone()),
            ..PlanRequest::default()
        };
        let error = execute_conversion_chain(
            Path::new("fixture.xlsx"),
            &request,
            &chain,
            uuid::Uuid::new_v4(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect_err("final output exists");
        assert_eq!(error.code, crate::ErrorCode::OutputConflict);
        // 整链未启动，也没有留下 staging 目录。
        assert_eq!(
            std::fs::read_dir(directory.path())
                .expect("list workspace")
                .count(),
            1,
            "only the pre-existing output remains"
        );
    }
}
