#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use formatwright_core::{
    EngineIdentity, ErrorCode, ExecutionMilestone, FormatWrightError, JobCreateRequest,
    JobExecutionService, JobRecord, JobState, Plan, PlanRequest, Probe, SqliteJobStore,
    ValidationStatus, cleanup_staged_output, doctor, execute_plan_observed, identify_artifact,
    inspect_builtin_engine, inspect_document, inspect_engine, inspect_media, inspect_office,
    inspect_pdf, inspect_structured, office_format_hint, pdf_format_hint, plan_conversion,
    plan_heic_conversion, plan_markup_to_docx, plan_markup_to_pdf, plan_metadata_clean,
    plan_office_to_pdf, plan_pdf_render, plan_structured_conversion, resolve_output_path,
    staged_output_candidates, structured_format_hint, verify_engine_pack,
};
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "formatwright",
    version,
    about = "Local-first file conversion with explainable plans and validation"
)]
struct Cli {
    #[arg(long, global = true, help = "Emit stable JSON on stdout")]
    json: bool,

    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Override the persistent job database path"
    )]
    state_db: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Detect conversion engines and report their provenance.
    Doctor,
    /// Compute a bounded-cost local artifact identity without format engines.
    Identify {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
    },
    /// Inspect the real media format and streams with ffprobe.
    Inspect {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
    },
    /// Build an explainable conversion Plan without executing it.
    Plan {
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        #[arg(long, value_name = "FORMAT")]
        to: String,

        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,

        #[arg(
            long,
            value_name = "INDEX",
            help = "Select an absolute audio stream index"
        )]
        audio_stream: Option<u32>,

        #[arg(
            long,
            help = "Permit the Plan to drop streams the target cannot contain"
        )]
        allow_stream_drop: bool,

        #[arg(long, value_name = "MILLISECONDS", help = "GIF start position")]
        start_ms: Option<u64>,

        #[arg(long, value_name = "MILLISECONDS", help = "GIF clip duration")]
        duration_ms: Option<u64>,

        #[arg(long, value_name = "PIXELS", help = "GIF output width")]
        width: Option<u32>,

        #[arg(long, value_name = "1-100", help = "Image output quality")]
        quality: Option<u8>,

        #[arg(long, value_name = "36-600", help = "PDF render resolution")]
        dpi: Option<u16>,

        #[arg(long, value_name = "rgb|gray", help = "PDF render color mode")]
        color_mode: Option<String>,

        #[arg(long, value_name = "FPS", help = "GIF frame rate")]
        fps: Option<u32>,

        #[arg(
            long,
            value_name = "COUNT",
            help = "GIF loop count; zero means infinite"
        )]
        loop_count: Option<u16>,

        #[arg(long, help = "Allow explicitly reported lossy structured-data mapping")]
        allow_lossy_data: bool,
    },
    /// Inspect, plan, execute, validate, and commit a conversion.
    Convert {
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        #[arg(long, value_name = "FORMAT")]
        to: String,

        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,

        #[arg(
            long,
            value_name = "INDEX",
            help = "Select an absolute audio stream index"
        )]
        audio_stream: Option<u32>,

        #[arg(long, help = "Permit dropping streams the target cannot contain")]
        allow_stream_drop: bool,

        #[arg(long, value_name = "MILLISECONDS", help = "GIF start position")]
        start_ms: Option<u64>,

        #[arg(long, value_name = "MILLISECONDS", help = "GIF clip duration")]
        duration_ms: Option<u64>,

        #[arg(long, value_name = "PIXELS", help = "GIF output width")]
        width: Option<u32>,

        #[arg(long, value_name = "1-100", help = "Image output quality")]
        quality: Option<u8>,

        #[arg(long, value_name = "36-600", help = "PDF render resolution")]
        dpi: Option<u16>,

        #[arg(long, value_name = "rgb|gray", help = "PDF render color mode")]
        color_mode: Option<String>,

        #[arg(long, value_name = "FPS", help = "GIF frame rate")]
        fps: Option<u32>,

        #[arg(
            long,
            value_name = "COUNT",
            help = "GIF loop count; zero means infinite"
        )]
        loop_count: Option<u16>,

        #[arg(long, help = "Allow explicitly reported lossy structured-data mapping")]
        allow_lossy_data: bool,

        #[arg(long, help = "Print the Plan and do not execute")]
        dry_run: bool,

        #[arg(
            long,
            help = "Persist the immutable Plan in the durable queue without executing"
        )]
        queue_only: bool,

        #[arg(
            long,
            value_name = "SECONDS",
            help = "Cancel the conversion after a wall-clock timeout"
        )]
        timeout_seconds: Option<u64>,
    },
    /// Remove declared private/secret metadata into a new verified output.
    Clean {
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,

        #[arg(long, help = "Print the metadata-clean Plan and do not execute")]
        dry_run: bool,

        #[arg(
            long,
            value_name = "SECONDS",
            help = "Cancel after a wall-clock timeout"
        )]
        timeout_seconds: Option<u64>,
    },
    /// Recursively plan, reserve, and run a bounded image batch.
    BatchImages {
        #[arg(value_name = "INPUT_DIRECTORY")]
        input: PathBuf,

        #[arg(long, value_name = "OUTPUT_DIRECTORY")]
        output_dir: PathBuf,

        #[arg(long, value_name = "FORMAT")]
        to: String,

        #[arg(long, value_name = "PIXELS")]
        width: Option<u32>,

        #[arg(long, value_name = "1-100")]
        quality: Option<u8>,

        #[arg(long, help = "Reserve and queue every output without executing")]
        queue_only: bool,

        #[arg(long, value_name = "COUNT", help = "Stop scheduling after COUNT jobs")]
        pause_after: Option<usize>,
    },
    /// Inspect or recover the durable local job queue.
    Jobs {
        #[command(subcommand)]
        command: JobsCommand,
    },
    /// Inspect and verify importable engine packs.
    Engines {
        #[command(subcommand)]
        command: EnginesCommand,
    },
}

#[derive(Debug, Subcommand)]
enum JobsCommand {
    /// List recent jobs in reverse update order.
    List {
        #[arg(long, default_value_t = 100)]
        limit: usize,

        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Show one job, its immutable Plan, and ordered events.
    Show {
        #[arg(value_name = "JOB_ID")]
        job_id: Uuid,
    },
    /// Mark jobs abandoned by a prior process as interrupted and clean staging files.
    Recover,
    /// Requeue a failed, cancelled, or interrupted job using its immutable Plan.
    Retry {
        #[arg(value_name = "JOB_ID")]
        job_id: Uuid,
    },
    /// Cancel a job that has not started running in another process.
    Cancel {
        #[arg(value_name = "JOB_ID")]
        job_id: Uuid,
    },
    /// Resume a blocked or interrupted job by returning it to the durable queue.
    Resume {
        #[arg(value_name = "JOB_ID")]
        job_id: Uuid,
    },
    /// Run a bounded FIFO window of queued jobs; Ctrl+C stops further scheduling.
    Run {
        #[arg(long, default_value_t = 100)]
        limit: usize,

        #[arg(
            long,
            default_value_t = 4,
            help = "Maximum runnable processes; resource-class limits may reduce it"
        )]
        parallel: usize,
    },
}

#[derive(Debug, Serialize)]
struct RecoveryReport {
    interrupted_jobs: Vec<JobRecord>,
    removed_staged_outputs: Vec<PathBuf>,
}

#[derive(Debug, Serialize)]
struct ImageBatchReport {
    schema_version: u32,
    discovered: usize,
    planned: usize,
    skipped: usize,
    completed: usize,
    warning: usize,
    failed: usize,
    cancelled: usize,
    queued: usize,
    paused: bool,
    job_ids: Vec<Uuid>,
}

#[derive(Debug, Subcommand)]
enum EnginesCommand {
    /// Verify manifest invariants, host target, file hashes, and license files.
    Verify {
        #[arg(value_name = "MANIFEST")]
        manifest: PathBuf,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.json);
    let json = cli.json;
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if json {
                let rendered = serde_json::to_string_pretty(&error)
                    .unwrap_or_else(|_| format!(r#"{{"code":"INTERNAL","message":"{error}"}}"#));
                println!("{rendered}");
            } else {
                eprintln!("error [{}]: {}", error.code, error.message);
                eprintln!("action: {}", error.user_action);
                if let Some(diagnostic) = &error.diagnostic {
                    eprintln!("diagnostic: {diagnostic}");
                }
            }
            ExitCode::from(error.code.exit_code())
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn run(cli: Cli) -> Result<(), FormatWrightError> {
    match cli.command {
        Command::Doctor => {
            let report = doctor().await;
            if cli.json {
                print_json(&report)?;
            } else {
                for (name, health) in report.engines {
                    if let Some(identity) = health.identity {
                        println!(
                            "{name}: available\n  version: {}\n  path: {}\n  sha256: {}",
                            identity.version,
                            identity.binary_path.display(),
                            identity.binary_sha256
                        );
                    } else {
                        println!("{name}: unavailable\n  reason: {}", health.message);
                    }
                }
            }
            Ok(())
        }
        Command::Identify { input } => {
            let artifact = identify_artifact(input).await?;
            if cli.json {
                print_json(&artifact)
            } else {
                println!("path: {}", artifact.canonical_path.display());
                println!("size: {} bytes", artifact.size_bytes);
                println!("fingerprint: {}", artifact.fast_fingerprint);
                Ok(())
            }
        }
        Command::Inspect { input } => {
            let probe = if pdf_format_hint(&input)? {
                let pdfinfo = inspect_engine("pdfinfo").await?;
                inspect_pdf(input, &pdfinfo).await?
            } else if office_format_hint(&input)?.is_some() {
                inspect_office(input).await?
            } else if structured_format_hint(&input).is_some() {
                inspect_structured(input).await?
            } else if is_document_path(&input) {
                inspect_document(input).await?
            } else {
                let ffprobe = inspect_engine("ffprobe").await?;
                inspect_media(input, &ffprobe).await?
            };
            if cli.json {
                print_json(&probe)
            } else {
                print_probe(&probe);
                Ok(())
            }
        }
        Command::Plan {
            input,
            to,
            output,
            audio_stream,
            allow_stream_drop,
            start_ms,
            duration_ms,
            width,
            quality,
            dpi,
            color_mode,
            fps,
            loop_count,
            allow_lossy_data,
        } => {
            let output = output.unwrap_or_else(|| default_output_path(&input, &to));
            let request = PlanRequest {
                target_format: to,
                output_path: Some(output),
                preserve_all_streams: !allow_stream_drop,
                audio_stream_index: audio_stream,
                start_millis: start_ms,
                duration_millis: duration_ms,
                width,
                quality,
                dpi,
                color_mode,
                frames_per_second: fps,
                loop_count,
                allow_lossy_data,
            };
            let (_, plan, _) = prepare_conversion(&input, &request).await?;
            if cli.json {
                print_json(&plan)
            } else {
                print_plan(&plan);
                Ok(())
            }
        }
        Command::Convert {
            input,
            to,
            output,
            audio_stream,
            allow_stream_drop,
            start_ms,
            duration_ms,
            width,
            quality,
            dpi,
            color_mode,
            fps,
            loop_count,
            allow_lossy_data,
            dry_run,
            queue_only,
            timeout_seconds,
        } => {
            let output = output.unwrap_or_else(|| default_output_path(&input, &to));
            let request = PlanRequest {
                target_format: to,
                output_path: Some(output.clone()),
                preserve_all_streams: !allow_stream_drop,
                audio_stream_index: audio_stream,
                start_millis: start_ms,
                duration_millis: duration_ms,
                width,
                quality,
                dpi,
                color_mode,
                frames_per_second: fps,
                loop_count,
                allow_lossy_data,
            };
            let (probe, plan, validation_engine) = prepare_conversion(&input, &request).await?;
            if dry_run && queue_only {
                return Err(FormatWrightError::new(
                    ErrorCode::InputInvalid,
                    formatwright_core::Stage::Plan,
                    "--dry-run and --queue-only cannot be combined",
                    "Choose either Plan preview or durable queueing.",
                ));
            }
            if queue_only && timeout_seconds.is_some() {
                return Err(FormatWrightError::new(
                    ErrorCode::InputInvalid,
                    formatwright_core::Stage::Plan,
                    "--timeout-seconds applies only to immediate execution",
                    "Remove the timeout when queueing, or execute the conversion immediately.",
                ));
            }
            if dry_run {
                if cli.json {
                    return print_json(&plan);
                }
                print_plan(&plan);
                return Ok(());
            }
            if queue_only {
                return queue_stored_plan(&probe, &plan, cli.state_db, cli.json);
            }

            execute_stored_plan(
                &probe,
                &plan,
                &validation_engine,
                cli.state_db,
                timeout_seconds,
                cli.json,
            )
            .await
        }
        Command::Clean {
            input,
            output,
            dry_run,
            timeout_seconds,
        } => {
            let ffprobe = inspect_engine("ffprobe").await?;
            let ffmpeg = inspect_engine("ffmpeg").await?;
            let probe = inspect_media(&input, &ffprobe).await?;
            let output = output.unwrap_or_else(|| default_clean_output_path(&input));
            let plan = plan_metadata_clean(&probe, output, &ffmpeg)?;
            if dry_run {
                if cli.json {
                    return print_json(&plan);
                }
                print_plan(&plan);
                return Ok(());
            }
            execute_stored_plan(
                &probe,
                &plan,
                &ffprobe,
                cli.state_db,
                timeout_seconds,
                cli.json,
            )
            .await
        }
        Command::BatchImages {
            input,
            output_dir,
            to,
            width,
            quality,
            queue_only,
            pause_after,
        } => {
            run_image_batch(
                &input,
                &output_dir,
                &to,
                width,
                quality,
                queue_only,
                pause_after,
                cli.state_db,
                cli.json,
            )
            .await
        }
        Command::Jobs { command } => {
            let database_path = cli.state_db.unwrap_or_else(default_state_db);
            let mut store = open_job_store(&database_path)?;
            match command {
                JobsCommand::List { limit, offset } => {
                    let jobs = store.list_jobs_page(limit, offset)?;
                    if cli.json {
                        print_json(&jobs)
                    } else {
                        for job in jobs {
                            println!(
                                "{}\t{:?}\t{} -> {}",
                                job.id,
                                job.state,
                                job.input_path.display(),
                                job.output_path.display()
                            );
                        }
                        Ok(())
                    }
                }
                JobsCommand::Show { job_id } => {
                    let details = store.get_job_details(job_id)?.ok_or_else(|| {
                        FormatWrightError::new(
                            ErrorCode::StorageFailed,
                            formatwright_core::Stage::Store,
                            format!("Job does not exist: {job_id}"),
                            "Run `formatwright jobs list` and choose an existing job.",
                        )
                    })?;
                    if cli.json {
                        print_json(&details)
                    } else {
                        println!("job: {}", details.job.id);
                        println!("state: {:?}", details.job.state);
                        println!("plan: {}", details.plan.plan_hash);
                        for event in details.events {
                            println!(
                                "{}\t{}\t{:?} -> {:?}",
                                event.sequence, event.code, event.previous_state, event.next_state
                            );
                        }
                        Ok(())
                    }
                }
                JobsCommand::Recover => {
                    let interrupted_jobs = store.interrupt_active_jobs()?;
                    let mut removed_staged_outputs = Vec::new();
                    for job in &interrupted_jobs {
                        let existing_candidates =
                            staged_output_candidates(&job.output_path, job.id)?
                                .into_iter()
                                .filter(|path| path.exists())
                                .collect::<Vec<_>>();
                        if cleanup_staged_output(&job.output_path, job.id)? {
                            removed_staged_outputs.extend(existing_candidates);
                        }
                    }
                    let report = RecoveryReport {
                        interrupted_jobs,
                        removed_staged_outputs,
                    };
                    if cli.json {
                        print_json(&report)
                    } else {
                        println!("interrupted jobs: {}", report.interrupted_jobs.len());
                        println!(
                            "removed staged outputs: {}",
                            report.removed_staged_outputs.len()
                        );
                        Ok(())
                    }
                }
                JobsCommand::Retry { job_id } => {
                    let job = transition_stored_job(
                        &mut store,
                        job_id,
                        &[JobState::Failed, JobState::Cancelled, JobState::Interrupted],
                        JobState::Queued,
                        "JOB_RETRIED",
                        true,
                    )?;
                    print_job_action(&job, cli.json)
                }
                JobsCommand::Cancel { job_id } => {
                    let job = transition_stored_job(
                        &mut store,
                        job_id,
                        &[
                            JobState::Planned,
                            JobState::Queued,
                            JobState::Blocked,
                            JobState::Interrupted,
                        ],
                        JobState::Cancelled,
                        "USER_CANCELLED",
                        true,
                    )?;
                    print_job_action(&job, cli.json)
                }
                JobsCommand::Resume { job_id } => {
                    let job = transition_stored_job(
                        &mut store,
                        job_id,
                        &[JobState::Blocked, JobState::Interrupted],
                        JobState::Queued,
                        "JOB_RESUMED",
                        true,
                    )?;
                    print_job_action(&job, cli.json)
                }
                JobsCommand::Run { limit, parallel } => {
                    let cancellation = CancellationToken::new();
                    let signal = cancellation.clone();
                    tokio::spawn(async move {
                        if tokio::signal::ctrl_c().await.is_ok() {
                            signal.cancel();
                        }
                    });
                    let report =
                        JobExecutionService::run_window(&mut store, limit, parallel, cancellation)
                            .await?;
                    if cli.json {
                        print_json(&report)
                    } else {
                        println!("selected: {}", report.selected);
                        println!("completed: {}", report.completed);
                        println!("warning: {}", report.warning);
                        println!("blocked: {}", report.blocked);
                        println!("failed: {}", report.failed);
                        println!("cancelled: {}", report.cancelled);
                        println!("stopped: {}", report.stopped);
                        println!("parallelism: {}", report.parallelism);
                        println!("peak active: {}", report.peak_active);
                        Ok(())
                    }
                }
            }
        }
        Command::Engines { command } => match command {
            EnginesCommand::Verify { manifest } => {
                let verified = verify_engine_pack(manifest)?;
                if cli.json {
                    print_json(&verified)
                } else {
                    println!("engine: {}", verified.manifest.engine_id);
                    println!("version: {}", verified.manifest.version);
                    println!("manifest sha256: {}", verified.manifest_sha256);
                    println!("executables verified: {}", verified.executables.len());
                    println!("signature present: {}", verified.signature_present);
                    Ok(())
                }
            }
        },
    }
}

fn queue_stored_plan(
    probe: &Probe,
    plan: &Plan,
    state_db: Option<PathBuf>,
    json: bool,
) -> Result<(), FormatWrightError> {
    let database_path = state_db.unwrap_or_else(default_state_db);
    let mut store = open_job_store(&database_path)?;
    let resolved_output = resolve_output_path(plan)?;
    let job = store.create_job(&probe.artifact.canonical_path, &resolved_output, plan)?;
    let queued = store.transition(job.id, JobState::Queued, "JOB_ENQUEUED")?;
    print_job_action(&queued, json)
}

#[allow(clippy::too_many_arguments)]
async fn execute_stored_plan(
    probe: &Probe,
    plan: &Plan,
    validation_engine: &EngineIdentity,
    state_db: Option<PathBuf>,
    timeout_seconds: Option<u64>,
    json: bool,
) -> Result<(), FormatWrightError> {
    let database_path = state_db.unwrap_or_else(default_state_db);
    let mut store = open_job_store(&database_path)?;
    let resolved_output = resolve_output_path(plan)?;
    let job = store.create_job(&probe.artifact.canonical_path, &resolved_output, plan)?;
    store.transition(job.id, JobState::Running, "ENGINE_STARTED")?;

    let cancellation = CancellationToken::new();
    let signal_token = cancellation.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_token.cancel();
        }
    });
    if let Some(seconds) = timeout_seconds {
        let timeout_token = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
            timeout_token.cancel();
        });
    }
    let result = execute_plan_observed(
        probe,
        plan,
        validation_engine,
        job.id,
        cancellation,
        |milestone| match milestone {
            ExecutionMilestone::EngineFinished => store
                .transition(job.id, JobState::Validating, "ENGINE_FINISHED")
                .map(|_| ()),
        },
    )
    .await;
    match result {
        Ok(result) => {
            let final_state = match result.report.status {
                ValidationStatus::Pass => JobState::Completed,
                ValidationStatus::Warning | ValidationStatus::Unknown => JobState::Warning,
                ValidationStatus::Fail => JobState::Failed,
            };
            store.transition(job.id, final_state, "VALIDATION_FINISHED")?;
            if json {
                print_json(&result.report)?;
            } else {
                println!("output: {}", result.output_path.display());
                println!("validation: {:?}", result.report.status);
                println!("plan: {}", result.report.plan_hash);
            }
            Ok(())
        }
        Err(error) => {
            let state = if error.code == ErrorCode::Cancelled {
                JobState::Cancelled
            } else {
                JobState::Failed
            };
            let _ = store.transition(job.id, state, "EXECUTION_STOPPED");
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_image_batch(
    input_root: &Path,
    output_root: &Path,
    target: &str,
    width: Option<u32>,
    quality: Option<u8>,
    queue_only: bool,
    pause_after: Option<usize>,
    state_db: Option<PathBuf>,
    json: bool,
) -> Result<(), FormatWrightError> {
    let target = target.trim().trim_start_matches('.').to_ascii_lowercase();
    if !matches!(target.as_str(), "jpg" | "jpeg" | "png" | "webp" | "avif") {
        return Err(FormatWrightError::new(
            ErrorCode::Unsupported,
            formatwright_core::Stage::Plan,
            format!("Image batch target is unsupported: {target}"),
            "Choose JPG, PNG, WebP, or AVIF.",
        ));
    }
    if width.is_some_and(|value| !(1..=16_384).contains(&value))
        || quality.is_some_and(|value| !(1..=100).contains(&value))
        || (target == "png" && quality.is_some())
    {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            formatwright_core::Stage::Plan,
            "Image batch settings are outside the supported target contract",
            "Use width 1–16384, quality 1–100, and omit quality for PNG.",
        ));
    }
    let input_root = input_root.canonicalize().map_err(|error| {
        FormatWrightError::new(
            ErrorCode::InputInvalid,
            formatwright_core::Stage::Inspect,
            format!("Input directory is unavailable: {}", input_root.display()),
            "Choose a readable local directory.",
        )
        .with_diagnostic(error.to_string())
    })?;
    if !input_root.is_dir() {
        return Err(FormatWrightError::new(
            ErrorCode::InputInvalid,
            formatwright_core::Stage::Inspect,
            "Batch input is not a directory",
            "Choose a directory containing images.",
        ));
    }
    std::fs::create_dir_all(output_root).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            formatwright_core::Stage::Store,
            format!("Cannot create output directory: {}", output_root.display()),
            "Choose a writable output directory.",
        )
        .with_diagnostic(error.to_string())
    })?;
    let output_root = output_root.canonicalize().map_err(|error| {
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            formatwright_core::Stage::Store,
            "Cannot resolve the image batch output directory",
            "Choose a writable local directory.",
        )
        .with_diagnostic(error.to_string())
    })?;
    if output_root == input_root {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            formatwright_core::Stage::Plan,
            "Batch output directory cannot equal the input directory",
            "Choose a separate output root.",
        ));
    }
    let (inputs, discovered, enumerator_skipped) =
        enumerate_image_inputs(&input_root, &output_root)?;
    let ffprobe = inspect_engine("ffprobe").await?;
    let ffmpeg = inspect_engine("ffmpeg").await?;
    let canonical_target = if matches!(target.as_str(), "jpg" | "jpeg") {
        "jpeg"
    } else {
        target.as_str()
    };
    let target_extension = if canonical_target == "jpeg" {
        "jpg"
    } else {
        canonical_target
    };
    let mut reserved_names = HashSet::new();
    let mut planned = Vec::new();
    let mut requests = Vec::new();
    let mut skipped = enumerator_skipped;
    for input in inputs {
        let relative = input.strip_prefix(&input_root).map_err(|error| {
            FormatWrightError::new(
                ErrorCode::Internal,
                formatwright_core::Stage::Plan,
                "Enumerated image escaped the batch root",
                "Report this internal error.",
            )
            .with_diagnostic(error.to_string())
        })?;
        let output = unique_batch_output(
            &output_root,
            relative,
            target_extension,
            &mut reserved_names,
        )?;
        if output.exists() {
            return Err(FormatWrightError::new(
                ErrorCode::OutputConflict,
                formatwright_core::Stage::Plan,
                format!("Batch output already exists: {}", output.display()),
                "Choose an empty output directory or another conflict policy.",
            ));
        }
        let probe = match inspect_media(&input, &ffprobe).await {
            Ok(probe) if probe.format.kind == formatwright_core::FormatKind::Image => probe,
            Ok(_) | Err(_) => {
                skipped = skipped.saturating_add(1);
                continue;
            }
        };
        let request = PlanRequest {
            target_format: canonical_target.to_owned(),
            output_path: Some(output.clone()),
            preserve_all_streams: true,
            width,
            quality,
            ..PlanRequest::default()
        };
        let Ok(plan) = plan_conversion(&probe, &request, &ffmpeg) else {
            skipped = skipped.saturating_add(1);
            continue;
        };
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                FormatWrightError::new(
                    ErrorCode::StorageFailed,
                    formatwright_core::Stage::Store,
                    format!("Cannot create batch directory: {}", parent.display()),
                    "Choose a writable output directory.",
                )
                .with_diagnostic(error.to_string())
            })?;
        }
        requests.push(JobCreateRequest {
            input_path: probe.artifact.canonical_path.clone(),
            output_path: output,
            plan: plan.clone(),
        });
        planned.push((probe, plan));
    }
    let database_path = state_db.unwrap_or_else(default_state_db);
    let mut store = open_job_store(&database_path)?;
    let jobs = store.create_jobs(&requests)?;
    store.queue_jobs(
        &jobs.iter().map(|job| job.id).collect::<Vec<_>>(),
        "BATCH_QUEUED",
    )?;
    let cancellation = CancellationToken::new();
    if !queue_only {
        let signal = cancellation.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                signal.cancel();
            }
        });
    }
    let scheduling_limit = if queue_only {
        0
    } else {
        pause_after.unwrap_or(usize::MAX)
    };
    let mut completed = 0_usize;
    let mut warning = 0_usize;
    let mut failed = 0_usize;
    let mut cancelled = 0_usize;
    for ((job, (probe, plan)), position) in jobs.iter().zip(planned.iter()).zip(0_usize..) {
        if position >= scheduling_limit || cancellation.is_cancelled() {
            break;
        }
        store.transition(job.id, JobState::Inspecting, "BATCH_REINSPECTING")?;
        let current_probe = match inspect_media(&probe.artifact.canonical_path, &ffprobe).await {
            Ok(current) if current.artifact.fast_fingerprint == plan.input_fingerprint => current,
            Ok(_) => {
                store.transition(job.id, JobState::Blocked, "INPUT_CHANGED")?;
                failed = failed.saturating_add(1);
                continue;
            }
            Err(_) => {
                store.transition(job.id, JobState::Failed, "REINSPECTION_FAILED")?;
                failed = failed.saturating_add(1);
                continue;
            }
        };
        store.transition(job.id, JobState::Planned, "PLAN_REVALIDATED")?;
        store.transition(job.id, JobState::Running, "ENGINE_STARTED")?;
        let result = execute_plan_observed(
            &current_probe,
            plan,
            &ffprobe,
            job.id,
            cancellation.clone(),
            |milestone| match milestone {
                ExecutionMilestone::EngineFinished => store
                    .transition(job.id, JobState::Validating, "ENGINE_FINISHED")
                    .map(|_| ()),
            },
        )
        .await;
        match result {
            Ok(result) => match result.report.status {
                ValidationStatus::Pass => {
                    store.transition(job.id, JobState::Completed, "VALIDATION_FINISHED")?;
                    completed = completed.saturating_add(1);
                }
                ValidationStatus::Warning | ValidationStatus::Unknown => {
                    store.transition(job.id, JobState::Warning, "VALIDATION_FINISHED")?;
                    warning = warning.saturating_add(1);
                }
                ValidationStatus::Fail => {
                    store.transition(job.id, JobState::Failed, "VALIDATION_FINISHED")?;
                    failed = failed.saturating_add(1);
                }
            },
            Err(error) => {
                if error.code == ErrorCode::Cancelled {
                    store.transition(job.id, JobState::Cancelled, "BATCH_CANCELLED")?;
                    cancelled = cancelled.saturating_add(1);
                    break;
                }
                store.transition(job.id, JobState::Failed, "EXECUTION_STOPPED")?;
                failed = failed.saturating_add(1);
            }
        }
    }
    let terminal = completed
        .saturating_add(warning)
        .saturating_add(failed)
        .saturating_add(cancelled);
    let queued = jobs.len().saturating_sub(terminal);
    let report = ImageBatchReport {
        schema_version: 1,
        discovered,
        planned: jobs.len(),
        skipped,
        completed,
        warning,
        failed,
        cancelled,
        queued,
        paused: queued > 0,
        job_ids: jobs.iter().map(|job| job.id).collect(),
    };
    if json {
        print_json(&report)
    } else {
        println!("discovered: {}", report.discovered);
        println!("planned: {}", report.planned);
        println!("skipped: {}", report.skipped);
        println!("completed: {}", report.completed);
        println!("warning: {}", report.warning);
        println!("failed: {}", report.failed);
        println!("cancelled: {}", report.cancelled);
        println!("queued: {}", report.queued);
        Ok(())
    }
}

fn enumerate_image_inputs(
    input_root: &Path,
    output_root: &Path,
) -> Result<(Vec<PathBuf>, usize, usize), FormatWrightError> {
    let mut directories = vec![input_root.to_owned()];
    let mut inputs = Vec::new();
    let mut discovered = 0_usize;
    let mut skipped = 0_usize;
    while let Some(directory) = directories.pop() {
        let mut entries = std::fs::read_dir(&directory)
            .map_err(|error| {
                FormatWrightError::new(
                    ErrorCode::InputInvalid,
                    formatwright_core::Stage::Inspect,
                    format!("Cannot enumerate directory: {}", directory.display()),
                    "Check directory permissions and retry.",
                )
                .with_diagnostic(error.to_string())
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| {
                FormatWrightError::new(
                    ErrorCode::InputInvalid,
                    formatwright_core::Stage::Inspect,
                    "Cannot read a directory entry",
                    "Check directory permissions and retry.",
                )
                .with_diagnostic(error.to_string())
            })?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries.into_iter().rev() {
            let path = entry.path();
            let file_type = entry.file_type().map_err(|error| {
                FormatWrightError::new(
                    ErrorCode::InputInvalid,
                    formatwright_core::Stage::Inspect,
                    format!("Cannot inspect directory entry: {}", path.display()),
                    "Check filesystem permissions and retry.",
                )
                .with_diagnostic(error.to_string())
            })?;
            if file_type.is_symlink() {
                discovered = discovered.saturating_add(1);
                skipped = skipped.saturating_add(1);
            } else if file_type.is_dir() {
                if !path.starts_with(output_root) {
                    directories.push(path);
                }
            } else if file_type.is_file() {
                discovered = discovered.saturating_add(1);
                if is_image_extension(&path) {
                    inputs.push(path);
                } else {
                    skipped = skipped.saturating_add(1);
                }
            }
        }
    }
    inputs.sort();
    Ok((inputs, discovered, skipped))
}

fn is_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "avif" | "heic" | "heif"
            )
        })
}

fn unique_batch_output(
    output_root: &Path,
    relative_input: &Path,
    target_extension: &str,
    reserved: &mut HashSet<String>,
) -> Result<PathBuf, FormatWrightError> {
    let stem = relative_input
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            FormatWrightError::new(
                ErrorCode::InputInvalid,
                formatwright_core::Stage::Plan,
                "Batch input filename is not valid Unicode",
                "Rename the file and retry.",
            )
        })?;
    let relative_parent = relative_input.parent().unwrap_or_else(|| Path::new(""));
    let mut output = output_root
        .join(relative_parent)
        .join(format!("{stem}.{target_extension}"));
    let mut key = batch_output_key(&output);
    if !reserved.insert(key.clone()) {
        let source_extension = relative_input
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("source")
            .to_ascii_lowercase();
        output = output_root
            .join(relative_parent)
            .join(format!("{stem}.from-{source_extension}.{target_extension}"));
        key = batch_output_key(&output);
        if !reserved.insert(key) {
            return Err(FormatWrightError::new(
                ErrorCode::OutputConflict,
                formatwright_core::Stage::Plan,
                format!("Batch outputs collide at: {}", output.display()),
                "Rename duplicate source stems or choose another output directory.",
            ));
        }
    }
    Ok(output)
}

fn batch_output_key(path: &Path) -> String {
    let rendered = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        rendered.to_ascii_lowercase()
    } else {
        rendered
    }
}

async fn prepare_conversion(
    input: &Path,
    request: &PlanRequest,
) -> Result<(Probe, Plan, EngineIdentity), FormatWrightError> {
    if is_structured_target(&request.target_format) {
        let probe = inspect_structured(input).await?;
        let engine = inspect_builtin_engine("formatwright.structured").await?;
        let plan = plan_structured_conversion(&probe, request, &engine)?;
        return Ok((probe, plan, engine));
    }
    if request
        .target_format
        .trim()
        .trim_start_matches('.')
        .eq_ignore_ascii_case("docx")
    {
        let probe = inspect_document(input).await?;
        let pandoc = inspect_engine("pandoc").await?;
        let output = request.output_path.clone().ok_or_else(|| {
            FormatWrightError::new(
                ErrorCode::InputInvalid,
                formatwright_core::Stage::Plan,
                "DOCX conversion requires an output path",
                "Choose an output path.",
            )
        })?;
        let plan = plan_markup_to_docx(&probe, output, &pandoc)?;
        return Ok((probe, plan, pandoc));
    }
    if request
        .target_format
        .trim()
        .trim_start_matches('.')
        .eq_ignore_ascii_case("pdf")
        && office_format_hint(input)?.is_some()
    {
        let probe = inspect_office(input).await?;
        let soffice = inspect_engine("soffice").await?;
        let pdftoppm = inspect_engine("pdftoppm").await?;
        let pdfinfo = inspect_engine("pdfinfo").await?;
        let output = request.output_path.clone().ok_or_else(|| {
            FormatWrightError::new(
                ErrorCode::InputInvalid,
                formatwright_core::Stage::Plan,
                "Office-to-PDF conversion requires an output path",
                "Choose an output path.",
            )
        })?;
        let plan = plan_office_to_pdf(&probe, output, &soffice, &pdfinfo, &pdftoppm)?;
        return Ok((probe, plan, pdfinfo));
    }
    if request
        .target_format
        .trim()
        .trim_start_matches('.')
        .eq_ignore_ascii_case("pdf")
    {
        match inspect_document(input).await {
            Ok(probe) if matches!(probe.format.id.as_str(), "markdown" | "html") => {
                let pandoc = inspect_engine("pandoc").await?;
                let soffice = inspect_engine("soffice").await?;
                let pdfinfo = inspect_engine("pdfinfo").await?;
                let pdftoppm = inspect_engine("pdftoppm").await?;
                let output = request.output_path.clone().ok_or_else(|| {
                    FormatWrightError::new(
                        ErrorCode::InputInvalid,
                        formatwright_core::Stage::Plan,
                        "Markup-to-PDF conversion requires an output path",
                        "Choose an output path.",
                    )
                })?;
                let plan =
                    plan_markup_to_pdf(&probe, output, &pandoc, &soffice, &pdfinfo, &pdftoppm)?;
                return Ok((probe, plan, pdfinfo));
            }
            Ok(_) | Err(_) => {}
        }
    }
    if pdf_format_hint(input)? {
        let pdfinfo = inspect_engine("pdfinfo").await?;
        let pdftoppm = inspect_engine("pdftoppm").await?;
        let ffprobe = inspect_engine("ffprobe").await?;
        let probe = inspect_pdf(input, &pdfinfo).await?;
        let plan = plan_pdf_render(&probe, request, &pdftoppm)?;
        return Ok((probe, plan, ffprobe));
    }
    let ffprobe = inspect_engine("ffprobe").await?;
    let probe = inspect_media(input, &ffprobe).await?;
    let normalized_target = request
        .target_format
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if probe.format.id == "heic" && matches!(normalized_target.as_str(), "jpg" | "jpeg" | "png") {
        let heif_convert = inspect_engine("heif-convert").await?;
        let plan = plan_heic_conversion(&probe, request, &heif_convert)?;
        return Ok((probe, plan, ffprobe));
    }
    let ffmpeg = inspect_engine("ffmpeg").await?;
    let plan = plan_conversion(&probe, request, &ffmpeg)?;
    Ok((probe, plan, ffprobe))
}

fn is_structured_target(target: &str) -> bool {
    matches!(
        target
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str(),
        "csv" | "json" | "yaml" | "yml" | "xml"
    )
}

fn is_document_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "md" | "markdown" | "html" | "htm" | "docx"
            )
        })
}

fn open_job_store(database_path: &Path) -> Result<SqliteJobStore, FormatWrightError> {
    if let Some(parent) = database_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            FormatWrightError::new(
                ErrorCode::StorageFailed,
                formatwright_core::Stage::Store,
                format!("Cannot create state directory: {}", parent.display()),
                "Choose a writable state database path.",
            )
            .with_diagnostic(error.to_string())
        })?;
    }
    SqliteJobStore::open(database_path)
}

fn transition_stored_job(
    store: &mut SqliteJobStore,
    job_id: Uuid,
    allowed_states: &[JobState],
    next_state: JobState,
    code: &str,
    cleanup_staged: bool,
) -> Result<JobRecord, FormatWrightError> {
    let job = store.get_job(job_id)?.ok_or_else(|| {
        FormatWrightError::new(
            ErrorCode::StorageFailed,
            formatwright_core::Stage::Store,
            format!("Job does not exist: {job_id}"),
            "Run `formatwright jobs list` and choose an existing job.",
        )
    })?;
    if !allowed_states.contains(&job.state) {
        return Err(FormatWrightError::new(
            ErrorCode::PolicyBlocked,
            formatwright_core::Stage::Store,
            format!("Job action is not valid while the job is {:?}", job.state),
            "Refresh the job and choose an action allowed for its current state.",
        ));
    }
    if cleanup_staged {
        cleanup_staged_output(&job.output_path, job.id)?;
    }
    store.transition(job_id, next_state, code)
}

fn print_job_action(job: &JobRecord, json: bool) -> Result<(), FormatWrightError> {
    if json {
        print_json(job)
    } else {
        println!("job: {}", job.id);
        println!("state: {:?}", job.state);
        println!("sequence: {}", job.sequence);
        Ok(())
    }
}

fn init_tracing(json: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    if json {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .compact()
            .init();
    }
}

fn print_json(value: &impl serde::Serialize) -> Result<(), FormatWrightError> {
    let rendered = serde_json::to_string_pretty(value).map_err(|error| {
        FormatWrightError::new(
            ErrorCode::Internal,
            formatwright_core::Stage::Commit,
            "Unable to serialize command output",
            "Report this internal error.",
        )
        .with_diagnostic(error.to_string())
    })?;
    println!("{rendered}");
    Ok(())
}

fn print_probe(probe: &formatwright_core::Probe) {
    println!("input: {}", probe.artifact.display_path);
    println!(
        "detected: {} ({:?}, confidence {:.0}%)",
        probe.format.id,
        probe.format.kind,
        probe.format.confidence * 100.0
    );
    println!("size: {} bytes", probe.artifact.size_bytes);
    if let Some(duration) = probe.duration_seconds {
        println!("duration: {duration:.3} seconds");
    }
    for stream in &probe.streams {
        println!(
            "stream {}: {:?} codec={}",
            stream.index,
            stream.kind,
            stream.codec.as_deref().unwrap_or("unknown")
        );
    }
    for warning in &probe.warnings {
        println!("warning [{}]: {}", warning.code, warning.message);
    }
}

fn print_plan(plan: &formatwright_core::Plan) {
    println!("plan: {}", plan.plan_hash);
    println!("target: {}", plan.target_format);
    if let Some(output) = &plan.output_path {
        println!("output: {}", output.display());
    }
    for step in &plan.steps {
        println!(
            "{}: {:?} via {} ({:?})",
            step.step_id, step.operation, step.engine.engine_id, step.loss_class
        );
        for (key, value) in &step.arguments {
            println!("  {key}: {value}");
        }
    }
    if !plan.changes.changed.is_empty() {
        println!("planned changes:");
        for change in &plan.changes.changed {
            println!("  - {change}");
        }
    }
}

fn default_output_path(input: &Path, target: &str) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("output");
    let extension = target.trim().trim_start_matches('.');
    if pdf_format_hint(input).unwrap_or(false)
        && matches!(
            extension.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg"
        )
    {
        return parent.join(format!("{stem}.pages-{extension}"));
    }
    parent.join(format!("{stem}.converted.{extension}"))
}

fn default_clean_output_path(input: &Path) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("output");
    let extension = input
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("bin");
    parent.join(format!("{stem}.cleaned.{extension}"))
}

fn default_state_db() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(root) = env::var_os("LOCALAPPDATA") {
            return PathBuf::from(root)
                .join("FormatWright")
                .join("jobs.sqlite3");
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(root) = env::var_os("HOME") {
            return PathBuf::from(root)
                .join("Library")
                .join("Application Support")
                .join("FormatWright")
                .join("jobs.sqlite3");
        }
    }

    if let Some(root) = env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(root)
            .join("formatwright")
            .join("jobs.sqlite3");
    }
    PathBuf::from(".formatwright-jobs.sqlite3")
}
