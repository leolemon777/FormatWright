use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use formatwright_core::{
    JobCreateRequest, JobExecutionService, JobState, Plan, PlanRequest, QueueWindowControl,
    ReportService, SqliteJobStore, activate_engine_pack, inspect_builtin_engine, inspect_engine,
    inspect_media, inspect_structured, plan_conversion, plan_structured_conversion,
};
use serde::Serialize;

const JOB_COUNT: usize = 10_000;
const STRUCTURED_JOBS: usize = 9_600;
const IMAGE_JOBS: usize = 200;
const MEDIA_JOBS: usize = 200;
const INJECTED_BLOCKED_JOBS: usize = 20;
const SCHEDULING_WINDOW: usize = 256;
const PARALLELISM: usize = 4;

#[derive(Debug, Serialize)]
struct MixedTenThousandResult {
    schema_version: u32,
    jobs: usize,
    structured_jobs: usize,
    image_jobs: usize,
    media_jobs: usize,
    scheduling_window: usize,
    parallelism: usize,
    planning_ms: u128,
    execution_ms: u128,
    throughput_jobs_per_second: f64,
    queue_latency_p50_ms: i64,
    queue_latency_p95_ms: i64,
    queue_latency_max_ms: i64,
    batch_first_start_spread_ms: i64,
    early_window_structured: usize,
    early_window_image: usize,
    early_window_media: usize,
    maximum_hydrated: usize,
    maximum_peak_active: usize,
    injected_blocked: usize,
    resumed_after_repair: usize,
    completed: usize,
    outputs: usize,
    reports: usize,
    database_bytes: u64,
    wal_bytes: u64,
    staged_outputs_remaining: usize,
}

/// Expensive release gate: 9,600 JSON→YAML, 200 PNG→WebP, and 200
/// MKV→MP4 Jobs execute through the shared durable queue service.
#[tokio::test]
#[ignore = "release gate: run explicitly for the 10,000-file mixed certification"]
#[allow(clippy::too_many_lines)]
async fn converts_ten_thousand_mixed_files_with_fair_bounded_scheduling() {
    let suite = required_directory("FORMATWRIGHT_MIXED_SUITE_ROOT");
    let input_root = suite.join("input");
    let output_root = suite.join("output");
    let report_root = suite.join("reports");
    fs::create_dir_all(&input_root).expect("input root");
    fs::create_dir_all(&output_root).expect("output root");
    let database = suite.join("jobs.sqlite3");
    activate_engine_pack(required_fixture("FORMATWRIGHT_MIXED_MEDIA_PACK_MANIFEST"))
        .expect("activate verified media pack");

    let planning_started = Instant::now();
    let structured_input = input_root.join("records.json");
    fs::write(
        &structured_input,
        r#"[{"id":1,"name":"mixed-release-gate"}]"#,
    )
    .expect("structured fixture");
    let structured_probe = inspect_structured(&structured_input)
        .await
        .expect("inspect structured fixture");
    let structured_engine = inspect_builtin_engine("formatwright.structured")
        .await
        .expect("structured engine");

    let ffprobe = inspect_engine("ffprobe").await.expect("ffprobe engine");
    let ffmpeg = inspect_engine("ffmpeg").await.expect("ffmpeg engine");
    let image_input = required_fixture("FORMATWRIGHT_MIXED_IMAGE_FIXTURE");
    let media_input = required_fixture("FORMATWRIGHT_MIXED_MEDIA_FIXTURE");
    let changed_image_input = required_fixture("FORMATWRIGHT_MIXED_CHANGED_IMAGE_FIXTURE");
    let changed_media_input = required_fixture("FORMATWRIGHT_MIXED_CHANGED_MEDIA_FIXTURE");
    let image_probe = inspect_media(&image_input, &ffprobe)
        .await
        .expect("inspect image fixture");
    let media_probe = inspect_media(&media_input, &ffprobe)
        .await
        .expect("inspect media fixture");

    let structured_template = plan_structured_conversion(
        &structured_probe,
        &PlanRequest {
            target_format: "yaml".to_owned(),
            output_path: Some(output_root.join("structured-template.yaml")),
            ..PlanRequest::default()
        },
        &structured_engine,
    )
    .expect("structured Plan");
    let image_template = plan_conversion(
        &image_probe,
        &PlanRequest {
            target_format: "webp".to_owned(),
            output_path: Some(output_root.join("image-template.webp")),
            width: Some(320),
            quality: Some(75),
            ..PlanRequest::default()
        },
        &ffmpeg,
    )
    .expect("image Plan");
    let media_template = plan_conversion(
        &media_probe,
        &PlanRequest {
            target_format: "mp4".to_owned(),
            output_path: Some(output_root.join("media-template.mp4")),
            ..PlanRequest::default()
        },
        &ffmpeg,
    )
    .expect("media Plan");

    let mut store = SqliteJobStore::open(&database).expect("job store");
    let structured_batch = create_batch_from_template(
        &mut store,
        "mixed-structured",
        STRUCTURED_JOBS,
        &structured_input,
        &input_root,
        &output_root,
        "structured",
        "json",
        "yaml",
        &structured_template,
    );
    let image_batch = create_batch_from_template(
        &mut store,
        "mixed-image",
        IMAGE_JOBS,
        &image_input,
        &input_root,
        &output_root,
        "image",
        "png",
        "webp",
        &image_template,
    );
    let media_batch = create_batch_from_template(
        &mut store,
        "mixed-media",
        MEDIA_JOBS,
        &media_input,
        &input_root,
        &output_root,
        "media",
        "mkv",
        "mp4",
        &media_template,
    );
    let all_ids = [
        structured_batch.as_slice(),
        image_batch.as_slice(),
        media_batch.as_slice(),
    ]
    .concat();
    store
        .queue_jobs(&all_ids, "MIXED_RELEASE_GATE_QUEUED")
        .expect("queue mixed corpus");
    drop(all_ids);
    let injected = [
        structured_batch
            .iter()
            .take(10)
            .copied()
            .collect::<Vec<_>>(),
        image_batch.iter().take(5).copied().collect::<Vec<_>>(),
        media_batch.iter().take(5).copied().collect::<Vec<_>>(),
    ]
    .concat();
    assert_eq!(injected.len(), INJECTED_BLOCKED_JOBS);
    for job_id in &injected {
        let job = store.get_job(*job_id).expect("injected job").expect("job");
        if structured_batch.contains(job_id) {
            fs::write(
                &job.input_path,
                r#"[{"id":2,"name":"changed-after-queueing"}]"#,
            )
            .expect("inject structured input change");
        } else if image_batch.contains(job_id) {
            fs::copy(&changed_image_input, &job.input_path).expect("inject image input change");
        } else {
            fs::copy(&changed_media_input, &job.input_path).expect("inject media input change");
        }
    }
    let planning_ms = planning_started.elapsed().as_millis();

    let early_window = store
        .list_queued_jobs_fair(SCHEDULING_WINDOW)
        .expect("fair preview window");
    let early_window_structured = count_job_ids(&early_window, &structured_batch);
    let early_window_image = count_job_ids(&early_window, &image_batch);
    let early_window_media = count_job_ids(&early_window, &media_batch);
    assert!(early_window_structured > 0);
    assert!(early_window_image > 0);
    assert!(early_window_media > 0);
    let early_counts = [
        early_window_structured,
        early_window_image,
        early_window_media,
    ];
    assert!(
        early_counts.iter().max().expect("largest fair share")
            - early_counts.iter().min().expect("smallest fair share")
            <= 1
    );

    let reports = ReportService::new(&report_root);
    let execution_started = Instant::now();
    let mut completed = 0_usize;
    let mut maximum_hydrated = 0_usize;
    let mut maximum_peak_active = 0_usize;
    let mut injected_blocked = 0_usize;
    loop {
        let remaining = store
            .list_jobs_by_state(JobState::Queued, 1)
            .expect("queued probe");
        if remaining.is_empty() {
            break;
        }
        let report_service = reports.clone();
        let run = JobExecutionService::run_window_observed(
            &mut store,
            SCHEDULING_WINDOW,
            PARALLELISM,
            QueueWindowControl::new(),
            move |job_id, report| report_service.save(job_id, report).map(drop),
        )
        .await
        .expect("mixed scheduling window");
        assert_eq!(run.failed + run.cancelled + run.contended, 0);
        assert_eq!(run.warning, 0);
        assert!(run.selected <= SCHEDULING_WINDOW);
        maximum_hydrated = maximum_hydrated.max(run.selected);
        maximum_peak_active = maximum_peak_active.max(run.peak_active);
        completed += run.completed;
        injected_blocked += run.blocked;
        assert!(
            run.completed + run.blocked > 0,
            "a queue window made no progress"
        );
        #[allow(clippy::manual_is_multiple_of)]
        if completed % 1_000 == 0 || completed == JOB_COUNT {
            println!(
                "FORMATWRIGHT_MIXED_10000_PROGRESS completed={completed} elapsed_ms={}",
                execution_started.elapsed().as_millis()
            );
        }
    }
    assert_eq!(injected_blocked, INJECTED_BLOCKED_JOBS);
    for job_id in &injected {
        let job = store
            .get_job(*job_id)
            .expect("blocked injected job")
            .expect("job");
        assert_eq!(job.state, JobState::Blocked);
        let details = store
            .get_job_details(*job_id)
            .expect("blocked details")
            .expect("job details");
        assert_eq!(
            details.events.last().expect("blocked event").code,
            "INPUT_CHANGED"
        );
        let source = if structured_batch.contains(job_id) {
            &structured_input
        } else if image_batch.contains(job_id) {
            &image_input
        } else {
            &media_input
        };
        fs::copy(source, &job.input_path).expect("repair injected input");
        store
            .transition(*job_id, JobState::Queued, "MIXED_RELEASE_GATE_RESUMED")
            .expect("resume repaired job");
    }
    let repair_report_service = reports.clone();
    let repaired = JobExecutionService::run_window_observed(
        &mut store,
        SCHEDULING_WINDOW,
        PARALLELISM,
        QueueWindowControl::new(),
        move |job_id, report| repair_report_service.save(job_id, report).map(drop),
    )
    .await
    .expect("repair scheduling window");
    assert_eq!(repaired.completed, INJECTED_BLOCKED_JOBS);
    assert_eq!(
        repaired.warning
            + repaired.blocked
            + repaired.failed
            + repaired.cancelled
            + repaired.contended,
        0
    );
    maximum_hydrated = maximum_hydrated.max(repaired.selected);
    maximum_peak_active = maximum_peak_active.max(repaired.peak_active);
    completed += repaired.completed;
    let execution_ms = execution_started.elapsed().as_millis();
    assert_eq!(completed, JOB_COUNT);
    assert_eq!(
        store.count_jobs().expect("job count"),
        u64::try_from(JOB_COUNT).expect("bounded job count")
    );

    let (mut latencies, first_starts) =
        collect_queue_latencies(&store, [&structured_batch, &image_batch, &media_batch]);
    latencies.sort_unstable();
    assert_eq!(latencies.len(), JOB_COUNT);
    let queue_latency_p50_ms = percentile(&latencies, 50);
    let queue_latency_p95_ms = percentile(&latencies, 95);
    let queue_latency_max_ms = *latencies.last().expect("latency max");
    let batch_first_start_spread_ms = first_starts
        .iter()
        .max()
        .expect("latest batch start")
        .saturating_sub(*first_starts.iter().min().expect("earliest batch start"));
    assert!(
        batch_first_start_spread_ms <= 30_000,
        "one workload lane waited too long for its first engine start"
    );
    let outputs = count_files(&output_root);
    let report_count = count_files(&report_root);
    let staged_outputs_remaining = count_staged_files(&suite);
    assert_eq!(outputs, JOB_COUNT);
    assert_eq!(report_count, JOB_COUNT);
    assert_eq!(staged_outputs_remaining, 0);
    let throughput_jobs_per_second =
        f64::from(u32::try_from(JOB_COUNT).expect("mixed release-gate job count fits in u32"))
            / Duration::from_millis(u64::try_from(execution_ms).unwrap_or(u64::MAX)).as_secs_f64();
    let result = MixedTenThousandResult {
        schema_version: 1,
        jobs: JOB_COUNT,
        structured_jobs: STRUCTURED_JOBS,
        image_jobs: IMAGE_JOBS,
        media_jobs: MEDIA_JOBS,
        scheduling_window: SCHEDULING_WINDOW,
        parallelism: PARALLELISM,
        planning_ms,
        execution_ms,
        throughput_jobs_per_second,
        queue_latency_p50_ms,
        queue_latency_p95_ms,
        queue_latency_max_ms,
        batch_first_start_spread_ms,
        early_window_structured,
        early_window_image,
        early_window_media,
        maximum_hydrated,
        maximum_peak_active,
        injected_blocked,
        resumed_after_repair: repaired.completed,
        completed,
        outputs,
        reports: report_count,
        database_bytes: file_size(&database),
        wal_bytes: file_size(&database.with_extension("sqlite3-wal")),
        staged_outputs_remaining,
    };
    println!(
        "FORMATWRIGHT_MIXED_10000_RESULT {}",
        serde_json::to_string(&result).expect("serialize result")
    );
}

fn required_fixture(name: &str) -> PathBuf {
    let path = std::env::var_os(name).map_or_else(
        || panic!("{name} must point to a generated local release-gate fixture"),
        PathBuf::from,
    );
    assert!(path.is_file(), "{name} does not name a file");
    path
}

fn required_directory(name: &str) -> PathBuf {
    let path = std::env::var_os(name).map_or_else(
        || panic!("{name} must name an isolated release-gate directory"),
        PathBuf::from,
    );
    fs::create_dir_all(&path).expect("create release-gate directory");
    assert!(path.is_dir(), "{name} does not name a directory");
    path
}

#[allow(clippy::too_many_arguments)]
fn create_batch_from_template(
    store: &mut SqliteJobStore,
    name: &str,
    count: usize,
    source: &Path,
    input_root: &Path,
    output_root: &Path,
    output_prefix: &str,
    input_extension: &str,
    extension: &str,
    template: &Plan,
) -> Vec<uuid::Uuid> {
    let requests = (0..count)
        .map(|index| {
            let input = input_root.join(format!(
                "{output_prefix}-input-{index:05}.{input_extension}"
            ));
            fs::copy(source, &input).expect("copy distinct input fixture");
            let output = output_root.join(format!("{output_prefix}-{index:05}.{extension}"));
            let mut plan = template.clone();
            plan.plan_id = uuid::Uuid::new_v4();
            plan.output_path = Some(output.clone());
            JobCreateRequest {
                input_path: input,
                output_path: output,
                plan,
            }
        })
        .collect::<Vec<_>>();
    let batch = store.create_batch(name, &requests).expect("create batch");
    let jobs = store
        .list_batch_jobs_page(batch.id, count, 0)
        .expect("list batch jobs");
    assert_eq!(jobs.len(), count);
    jobs.into_iter().map(|job| job.id).collect()
}

fn count_job_ids(jobs: &[formatwright_core::JobRecord], ids: &[uuid::Uuid]) -> usize {
    let ids = ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    jobs.iter().filter(|job| ids.contains(&job.id)).count()
}

fn collect_queue_latencies<'a>(
    store: &SqliteJobStore,
    batches: impl IntoIterator<Item = &'a Vec<uuid::Uuid>>,
) -> (Vec<i64>, Vec<i64>) {
    let mut latencies = Vec::with_capacity(JOB_COUNT);
    let mut first_starts = Vec::new();
    for batch in batches {
        let mut first_start = i64::MAX;
        for job_id in batch {
            let details = store
                .get_job_details(*job_id)
                .expect("job details")
                .expect("stored job");
            assert_eq!(details.job.state, JobState::Completed);
            let queued = details
                .events
                .iter()
                .find(|event| event.code == "MIXED_RELEASE_GATE_QUEUED")
                .expect("queued event")
                .timestamp_unix_ms;
            let started = details
                .events
                .iter()
                .find(|event| event.code == "ENGINE_STARTED")
                .expect("start event")
                .timestamp_unix_ms;
            first_start = first_start.min(started);
            latencies.push(started.saturating_sub(queued));
        }
        first_starts.push(first_start);
    }
    (latencies, first_starts)
}

fn percentile(sorted: &[i64], percent: usize) -> i64 {
    let index = sorted
        .len()
        .saturating_mul(percent)
        .div_ceil(100)
        .saturating_sub(1);
    sorted[index.min(sorted.len().saturating_sub(1))]
}

fn count_files(root: &Path) -> usize {
    fs::read_dir(root)
        .expect("read directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .count()
}

fn count_staged_files(root: &Path) -> usize {
    let mut pending = vec![root.to_path_buf()];
    let mut count = 0;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)
            .expect("read staged-output tree")
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".formatwright-partial-")
            {
                count += 1;
            }
        }
    }
    count
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |metadata| metadata.len())
}
