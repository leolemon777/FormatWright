use std::fs;
use std::time::Instant;

use formatwright_core::{
    ExecutionMilestone, JobCreateRequest, JobState, PlanRequest, SqliteJobStore, ValidationStatus,
    execute_plan_observed, inspect_builtin_engine, inspect_structured, plan_structured_conversion,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

const JOB_COUNT: usize = 10_000;
const SCHEDULING_WINDOW: usize = 128;

/// Release-gate workload; opt in because it creates and validates 10,000 real
/// files and intentionally exercises tens of thousands of `SQLite` transitions.
#[tokio::test]
#[ignore = "release gate: run explicitly for the 10,000-file certification"]
#[allow(clippy::too_many_lines)]
async fn converts_and_validates_ten_thousand_structured_files_in_bounded_windows() {
    let suite = tempdir().expect("10,000-file sandbox");
    let input_root = suite.path().join("input");
    let output_root = suite.path().join("output");
    fs::create_dir_all(&input_root).expect("input root");
    fs::create_dir_all(&output_root).expect("output root");
    let database = suite.path().join("jobs.sqlite3");
    let engine = inspect_builtin_engine("formatwright.structured")
        .await
        .expect("built-in structured engine");

    let planning_started = Instant::now();
    let mut requests = Vec::with_capacity(JOB_COUNT);
    for index in 0..JOB_COUNT {
        let input = input_root.join(format!("item-{index:05}.json"));
        let output = output_root.join(format!("item-{index:05}.yaml"));
        fs::write(&input, format!(r#"[{{"id":{index},"ok":true}}]"#)).expect("write JSON fixture");
        let probe = inspect_structured(&input).await.expect("inspect JSON");
        let plan = plan_structured_conversion(
            &probe,
            &PlanRequest {
                target_format: "yaml".to_owned(),
                output_path: Some(output.clone()),
                ..PlanRequest::default()
            },
            &engine,
        )
        .expect("plan JSON to YAML");
        requests.push(JobCreateRequest {
            input_path: input,
            output_path: output,
            plan,
        });
    }
    let planning_ms = planning_started.elapsed().as_millis();

    let mut store = SqliteJobStore::open(&database).expect("open job store");
    let jobs = store
        .create_jobs(&requests)
        .expect("atomically create jobs");
    assert_eq!(jobs.len(), JOB_COUNT);
    store
        .queue_jobs(
            &jobs.iter().map(|job| job.id).collect::<Vec<_>>(),
            "RELEASE_GATE_QUEUED",
        )
        .expect("atomically queue jobs");
    drop(requests);
    drop(store);

    let execution_started = Instant::now();
    let mut store = SqliteJobStore::open(&database).expect("reopen job store");
    let mut completed = 0_usize;
    let mut maximum_hydrated = 0_usize;
    loop {
        let window = store
            .list_jobs_by_state(JobState::Queued, SCHEDULING_WINDOW)
            .expect("load bounded scheduling window");
        if window.is_empty() {
            break;
        }
        maximum_hydrated = maximum_hydrated.max(window.len());
        for job in window {
            let details = store
                .get_job_details(job.id)
                .expect("read queued details")
                .expect("queued job exists");
            store
                .transition(job.id, JobState::Inspecting, "RELEASE_GATE_REINSPECTING")
                .expect("transition to inspecting");
            let probe = inspect_structured(&details.job.input_path)
                .await
                .expect("reinspect JSON");
            assert_eq!(
                probe.artifact.fast_fingerprint,
                details.plan.input_fingerprint
            );
            store
                .transition(job.id, JobState::Planned, "RELEASE_GATE_REVALIDATED")
                .expect("transition to planned");
            store
                .transition(job.id, JobState::Running, "RELEASE_GATE_RUNNING")
                .expect("transition to running");
            let result = execute_plan_observed(
                &probe,
                &details.plan,
                &engine,
                job.id,
                CancellationToken::new(),
                |milestone| match milestone {
                    ExecutionMilestone::EngineFinished => store
                        .transition(job.id, JobState::Validating, "RELEASE_GATE_VALIDATING")
                        .map(|_| ()),
                },
            )
            .await
            .expect("execute and validate JSON to YAML");
            assert_eq!(result.report.status, ValidationStatus::Pass);
            assert!(result.output_path.is_file());
            store
                .transition(job.id, JobState::Completed, "RELEASE_GATE_COMPLETED")
                .expect("transition to completed");
            completed += 1;
            #[allow(clippy::manual_is_multiple_of)]
            if completed % 1_000 == 0 {
                println!(
                    "FORMATWRIGHT_10000_PROGRESS completed={completed} elapsed_ms={}",
                    execution_started.elapsed().as_millis()
                );
            }
        }
    }
    let execution_ms = execution_started.elapsed().as_millis();
    assert_eq!(completed, JOB_COUNT);
    assert!(maximum_hydrated <= SCHEDULING_WINDOW);
    assert_eq!(
        store
            .list_jobs_by_state(JobState::Completed, JOB_COUNT)
            .expect("read completed count")
            .len(),
        JOB_COUNT
    );
    assert_eq!(
        fs::read_dir(&output_root)
            .expect("read outputs")
            .filter_map(Result::ok)
            .filter(|entry| entry
                .path()
                .extension()
                .is_some_and(|value| value == "yaml"))
            .count(),
        JOB_COUNT
    );
    assert!(
        fs::read_dir(&output_root)
            .expect("read staging check")
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().contains("partial"))
    );
    println!(
        "FORMATWRIGHT_10000_CONVERSIONS jobs={JOB_COUNT} window={SCHEDULING_WINDOW} planning_ms={planning_ms} execution_ms={execution_ms}"
    );
}
