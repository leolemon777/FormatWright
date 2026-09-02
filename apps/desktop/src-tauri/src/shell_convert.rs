use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

pub const CONVERT_MERGE_QUIET: Duration = Duration::from_millis(800);
pub const CONVERT_READY_FIFO_LIMIT: usize = 8;
pub const CONVERT_PATHS_LIMIT: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DesktopShellOpenBatch {
    pub target: String,
    pub paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct ShellConvertCoordinator {
    buffer_target: Option<String>,
    buffer_paths: Vec<PathBuf>,
    ready: VecDeque<DesktopShellOpenBatch>,
    pub generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConvertPushOutcome {
    pub flushed_ready: bool,
    pub overflowed: bool,
}

impl ShellConvertCoordinator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, target: String, path: PathBuf) -> ConvertPushOutcome {
        let mut flushed_ready = false;
        if self
            .buffer_target
            .as_ref()
            .is_some_and(|current| current != &target)
        {
            self.flush_quiet();
            flushed_ready = true;
        }
        if self.buffer_target.is_none() {
            self.buffer_target = Some(target);
        }
        let mut overflowed = false;
        if self.buffer_paths.len() >= CONVERT_PATHS_LIMIT {
            self.buffer_paths.remove(0);
            overflowed = true;
        }
        self.buffer_paths.push(path);
        self.generation = self.generation.saturating_add(1);
        ConvertPushOutcome {
            flushed_ready,
            overflowed,
        }
    }

    pub fn flush_quiet(&mut self) -> Option<DesktopShellOpenBatch> {
        let target = self.buffer_target.take()?;
        if self.buffer_paths.is_empty() {
            return None;
        }
        let batch = DesktopShellOpenBatch {
            target,
            paths: std::mem::take(&mut self.buffer_paths),
        };
        if self.ready.len() >= CONVERT_READY_FIFO_LIMIT {
            self.ready.pop_front();
        }
        self.ready.push_back(batch.clone());
        Some(batch)
    }

    pub fn take_ready(&mut self) -> Option<DesktopShellOpenBatch> {
        self.ready.pop_front()
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn ready_len(&self) -> usize {
        self.ready.len()
    }
}

#[must_use]
pub fn should_run_immediately(path_count: usize, queue_window_busy: bool) -> bool {
    path_count == 1 && !queue_window_busy
}

#[must_use]
pub fn is_pdf_page_directory(input: &Path, target: &str) -> bool {
    let extension = input
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    extension == "pdf" && matches!(target, "png" | "jpg" | "jpeg")
}

#[must_use]
pub fn suggested_converted_name(
    input: &Path,
    target: &str,
    reserved: &HashSet<PathBuf>,
) -> PathBuf {
    let target = if target.eq_ignore_ascii_case("jpeg") {
        "jpg"
    } else {
        target
    };
    let parent = input.parent().unwrap_or_else(|| Path::new(""));
    let stem = input
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("output");
    let src_ext = input
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("bin")
        .to_ascii_lowercase();
    let first = if is_pdf_page_directory(input, target) {
        parent.join(format!("{stem}.converted-{target}-pages"))
    } else {
        parent.join(format!("{stem}.converted.{target}"))
    };
    if !contains_path(reserved, &first) {
        return first;
    }
    let second = if is_pdf_page_directory(input, target) {
        parent.join(format!("{stem}.from-{src_ext}.converted-{target}-pages"))
    } else {
        parent.join(format!("{stem}.from-{src_ext}.converted.{target}"))
    };
    if !contains_path(reserved, &second) {
        return second;
    }
    let mut index = 2_u32;
    loop {
        let candidate = if is_pdf_page_directory(input, target) {
            parent.join(format!(
                "{stem}.from-{src_ext}-{index}.converted-{target}-pages"
            ))
        } else {
            parent.join(format!("{stem}.from-{src_ext}-{index}.converted.{target}"))
        };
        if !contains_path(reserved, &candidate) {
            return candidate;
        }
        index = index.saturating_add(1);
    }
}

fn contains_path(reserved: &HashSet<PathBuf>, candidate: &Path) -> bool {
    reserved.iter().any(|existing| existing == candidate)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedConvertItem {
    pub input: PathBuf,
    pub output: PathBuf,
    pub skipped_conflict: bool,
    pub rejected: bool,
}

#[must_use]
pub fn plan_convert_outputs(paths: &[PathBuf], target: &str) -> Vec<PlannedConvertItem> {
    let mut reserved = HashSet::new();
    let mut planned = Vec::with_capacity(paths.len());
    for path in paths {
        if !path.is_file() {
            planned.push(PlannedConvertItem {
                input: path.clone(),
                output: PathBuf::new(),
                skipped_conflict: false,
                rejected: true,
            });
            continue;
        }
        let output = suggested_converted_name(path, target, &reserved);
        reserved.insert(output.clone());
        let skipped_conflict = output.exists();
        planned.push(PlannedConvertItem {
            input: path.clone(),
            output,
            skipped_conflict,
            rejected: false,
        });
    }
    planned
}

#[must_use]
pub fn surviving_convert_items(items: &[PlannedConvertItem]) -> Vec<&PlannedConvertItem> {
    items
        .iter()
        .filter(|item| !item.rejected && !item.skipped_conflict)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        CONVERT_PATHS_LIMIT, CONVERT_READY_FIFO_LIMIT, ShellConvertCoordinator,
        plan_convert_outputs, should_run_immediately, suggested_converted_name,
        surviving_convert_items,
    };
    use std::collections::HashSet;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn same_target_paths_merge_until_quiet_flush() {
        let mut coordinator = ShellConvertCoordinator::new();
        for index in 0..10 {
            let outcome =
                coordinator.push("webp".to_owned(), PathBuf::from(format!("p{index}.jpg")));
            assert!(!outcome.flushed_ready);
        }
        assert_eq!(coordinator.ready_len(), 0);
        let batch = coordinator.flush_quiet().expect("quiet flush");
        assert_eq!(batch.target, "webp");
        assert_eq!(batch.paths.len(), 10);
        assert_eq!(
            coordinator.take_ready().expect("ready fifo").paths.len(),
            10
        );
        assert!(coordinator.take_ready().is_none());
    }

    #[test]
    fn mixed_targets_flush_the_previous_batch_immediately() {
        let mut coordinator = ShellConvertCoordinator::new();
        coordinator.push("png".to_owned(), PathBuf::from(r"C:\in\manual.pdf"));
        let outcome = coordinator.push("webp".to_owned(), PathBuf::from(r"C:\in\photo.jpg"));
        assert!(outcome.flushed_ready);
        let first = coordinator.take_ready().expect("first batch");
        assert_eq!(first.target, "png");
        assert_eq!(first.paths.len(), 1);
        let second = coordinator.flush_quiet().expect("second");
        assert_eq!(second.target, "webp");
    }

    #[test]
    fn ready_fifo_survives_until_take() {
        let mut coordinator = ShellConvertCoordinator::new();
        coordinator.push("yaml".to_owned(), PathBuf::from(r"C:\in\a.json"));
        coordinator.flush_quiet();
        assert_eq!(coordinator.ready_len(), 1);
        assert_eq!(coordinator.take_ready().expect("batch").target, "yaml");
    }

    #[test]
    fn convert_buffer_keeps_the_newest_thirty_two_paths() {
        let mut coordinator = ShellConvertCoordinator::new();
        for index in 0..=CONVERT_PATHS_LIMIT {
            coordinator.push("json".to_owned(), PathBuf::from(format!("{index}.csv")));
        }
        let batch = coordinator.flush_quiet().expect("flush");
        assert_eq!(batch.paths.len(), CONVERT_PATHS_LIMIT);
        assert_eq!(batch.paths[0], PathBuf::from("1.csv"));
        let _ = CONVERT_READY_FIFO_LIMIT;
    }

    #[test]
    fn suggested_converted_name_uses_converted_segment_and_from_disambiguator() {
        let mut reserved = HashSet::new();
        let first = suggested_converted_name(
            PathBuf::from(r"C:\album\photo.jpg").as_path(),
            "webp",
            &reserved,
        );
        assert_eq!(first, PathBuf::from(r"C:\album\photo.converted.webp"));
        reserved.insert(first);
        let second = suggested_converted_name(
            PathBuf::from(r"C:\album\photo.png").as_path(),
            "webp",
            &reserved,
        );
        assert_eq!(
            second,
            PathBuf::from(r"C:\album\photo.from-png.converted.webp")
        );
        reserved.insert(second);
        let third = suggested_converted_name(
            PathBuf::from(r"C:\album\photo.gif").as_path(),
            "webp",
            &reserved,
        );
        assert_eq!(
            third,
            PathBuf::from(r"C:\album\photo.from-gif.converted.webp")
        );
    }

    #[test]
    fn all_conflict_plan_has_zero_survivors() {
        let suite = tempdir().expect("suite");
        let input = suite.path().join("notes.json");
        fs::write(&input, b"[{\"id\":1}]").expect("input");
        let output = suite.path().join("notes.converted.yaml");
        fs::write(&output, b"exists: true\n").expect("existing");
        let planned = plan_convert_outputs(&[input], "yaml");
        assert_eq!(planned.len(), 1);
        assert!(planned[0].skipped_conflict);
        assert!(surviving_convert_items(&planned).is_empty());
    }

    #[test]
    fn immediate_run_only_when_single_path_and_idle_window() {
        assert!(should_run_immediately(1, false));
        assert!(!should_run_immediately(1, true));
        assert!(!should_run_immediately(2, false));
    }
}
