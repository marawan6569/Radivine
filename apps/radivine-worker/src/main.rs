//! `radivine-worker` — Standalone on-demand background job daemon & CLI (`JOB_SYSTEM_DESIGN.md` §9).

#![forbid(unsafe_code)]

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use radivine_convert::{FormatId, OutputLocation, PresetOverrides, QualityIntent, QualitySetting};
use radivine_jobs::{
    CONVERT_IMAGE_KIND, JobId, JobUpdate, JobsClient, JobsSettings, Origin, RequestId, RetryMode,
    WirePath, WorkerResponse, WorkerRuntimePaths,
};
use radivine_worker::server::{WorkerServerConfig, run_server};
use radivine_worker::store::JobStore;

#[derive(Debug, Parser)]
#[command(
    name = "radivine-worker",
    version,
    about = "Durable background job worker and management CLI for Radivine"
)]
struct Cli {
    /// Override worker Unix domain socket path
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    /// Override SQLite database path
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    /// Override single-instance lockfile path
    #[arg(long, global = true)]
    lock: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run the background worker server loop
    Serve {
        /// Idle timeout in seconds before automatic clean exit
        #[arg(long)]
        idle_timeout: Option<u64>,

        /// Idle timeout in milliseconds (for fast automated lifecycle tests)
        #[arg(long)]
        idle_timeout_ms: Option<u64>,

        /// Optional wait time in milliseconds to acquire worker.lock (used during exit(75) handoff)
        #[arg(long, default_value_t = 0)]
        wait_for_lock_ms: u64,
    },

    /// Query worker daemon status (or report offline if idle-exited)
    Status {
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// List recent jobs
    List {
        /// Maximum number of jobs to return
        #[arg(long, default_value_t = 25)]
        limit: u32,

        /// Only show active (queued/preparing/running) jobs
        #[arg(long)]
        active_only: bool,

        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Show detailed status and per-item results for a job
    Show {
        /// Job ID to inspect
        job_id: u64,

        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Cooperatively cancel a queued or running job
    Cancel {
        /// Job ID to cancel
        job_id: u64,

        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Retry failed/interrupted (or cancelled/all) items of a terminal job
    Retry {
        /// Job ID to retry
        job_id: u64,

        /// Retry selection mode
        #[arg(long, value_enum, default_value_t = CliRetryMode::FailedOnly)]
        mode: CliRetryMode,

        /// Shorthand for `--mode all`
        #[arg(long)]
        all: bool,

        /// Optional target format override (`png`, `jpeg`, `webp`)
        #[arg(long)]
        to: Option<String>,

        /// Optional quality intent (`max`, `high`, `balanced`, `compact`, `lossless`) or numeric `1..=100`
        #[arg(long)]
        quality: Option<String>,

        /// Optional output directory override
        #[arg(long)]
        output_dir: Option<PathBuf>,

        /// Optional explicit idempotency request ID (`16..=64` ASCII `[0-9a-zA-Z_-]`)
        #[arg(long)]
        request_id: Option<String>,

        /// Wait for the retried job to reach a terminal state
        #[arg(long)]
        wait: bool,

        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Submit an image conversion job to the background worker
    SubmitConvert {
        /// Target format (`png`, `jpeg`, `webp`)
        #[arg(long)]
        to: Option<String>,

        /// Built-in or user preset ID (e.g., `quick.webp`, `quick.png`, `quick.jpg`, `web.optimized`)
        #[arg(long)]
        preset: Option<String>,

        /// Optional quality intent (`max`, `high`, `balanced`, `compact`, `lossless`) or numeric `1..=100`
        #[arg(long)]
        quality: Option<String>,

        /// Optional output directory override
        #[arg(long)]
        output_dir: Option<PathBuf>,

        /// Optional explicit idempotency request ID (`16..=64` ASCII `[0-9a-zA-Z_-]`)
        #[arg(long)]
        request_id: Option<String>,

        /// Subscribe and wait until the job finishes
        #[arg(long)]
        wait: bool,

        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,

        /// Input image file paths
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },

    /// Request the worker to shut down cleanly if no jobs are active
    ShutdownIfIdle {
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliRetryMode {
    #[value(alias = "failed_only", alias = "failed")]
    FailedOnly,
    #[value(alias = "failed_and_cancelled")]
    FailedAndCancelled,
    All,
}

impl From<CliRetryMode> for RetryMode {
    fn from(m: CliRetryMode) -> Self {
        match m {
            CliRetryMode::FailedOnly => Self::FailedOnly,
            CliRetryMode::FailedAndCancelled => Self::FailedAndCancelled,
            CliRetryMode::All => Self::All,
        }
    }
}

fn main() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_env("RADIVINE_LOG")
        .or_else(|_| tracing_subscriber::EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();

    let cli = Cli::parse();
    let mut paths = WorkerRuntimePaths::from_env();
    if let Some(s) = cli.socket {
        paths.socket_path = s;
    }
    if let Some(d) = cli.db {
        paths.db_path = d;
    }
    if let Some(l) = cli.lock {
        paths.lock_path = l;
    } else if let Some(parent) = paths.socket_path.parent() {
        paths.lock_path = parent.join("worker.lock");
    }

    match cli.command {
        Commands::Serve {
            idle_timeout,
            idle_timeout_ms,
            wait_for_lock_ms,
        } => {
            let (jobs_settings, _) =
                JobsSettings::load_lenient(&JobsSettings::default_config_path());
            let resolved_idle = if let Some(ms) = idle_timeout_ms {
                Duration::from_millis(ms.max(50))
            } else if let Ok(env_ms) = env::var("RADIVINE_WORKER_IDLE_TIMEOUT_MS")
                && let Ok(ms) = env_ms.trim().parse::<u64>()
            {
                Duration::from_millis(ms.max(50))
            } else if let Some(secs) = idle_timeout {
                Duration::from_secs(secs.max(1))
            } else {
                Duration::from_secs(jobs_settings.idle_timeout_secs)
            };

            run_server(WorkerServerConfig {
                socket_path: paths.socket_path,
                lock_path: paths.lock_path,
                db_path: paths.db_path,
                idle_timeout: resolved_idle,
                wait_for_lock_ms,
            })
        }
        Commands::Status { json } => cmd_status(&paths, json),
        Commands::List {
            limit,
            active_only,
            json,
        } => cmd_list(&paths, limit, !active_only, json),
        Commands::Show { job_id, json } => cmd_show(&paths, JobId(job_id), json),
        Commands::Cancel { job_id, json } => cmd_cancel(&paths, JobId(job_id), json),
        Commands::Retry {
            job_id,
            mode,
            all,
            to,
            quality,
            output_dir,
            request_id,
            wait,
            json,
        } => {
            let resolved_mode = if all {
                RetryMode::All
            } else {
                RetryMode::from(mode)
            };
            cmd_retry(
                &paths,
                JobId(job_id),
                resolved_mode,
                to.as_deref(),
                quality.as_deref(),
                output_dir.as_deref(),
                request_id,
                wait,
                json,
            )
        }
        Commands::SubmitConvert {
            to,
            preset,
            quality,
            output_dir,
            request_id,
            wait,
            json,
            files,
        } => cmd_submit_convert(
            &paths,
            to.as_deref(),
            preset.as_deref(),
            quality.as_deref(),
            output_dir.as_deref(),
            request_id,
            wait,
            json,
            &files,
        ),
        Commands::ShutdownIfIdle { json } => cmd_shutdown_if_idle(&paths, json),
    }
}

fn cmd_status(paths: &WorkerRuntimePaths, json: bool) -> Result<()> {
    match JobsClient::connect_with_timeout(&paths.socket_path, Duration::from_millis(500))
        .and_then(|mut c| c.ping())
    {
        Ok(WorkerResponse::Pong {
            pid,
            version,
            active_jobs,
            queued_items,
            schema_version,
            ..
        }) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "running": true,
                        "pid": pid,
                        "version": version,
                        "active_jobs": active_jobs,
                        "queued_items": queued_items,
                        "schema_version": schema_version,
                        "socket": paths.socket_path,
                        "db": paths.db_path,
                    }))?
                );
            } else {
                println!(
                    "radivine-worker: running (pid {pid}, v{version}, schema v{schema_version})"
                );
                println!("  socket:       {}", paths.socket_path.display());
                println!("  database:     {}", paths.db_path.display());
                println!("  active jobs:  {active_jobs}");
                println!("  queued items: {queued_items}");
            }
        }
        _ => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "running": false,
                        "socket": paths.socket_path,
                        "db": paths.db_path,
                    }))?
                );
            } else {
                println!("radivine-worker: offline (on-demand idle)");
                println!("  socket:   {}", paths.socket_path.display());
                println!("  database: {}", paths.db_path.display());
            }
        }
    }
    Ok(())
}

fn cmd_list(
    paths: &WorkerRuntimePaths,
    limit: u32,
    include_terminal: bool,
    json: bool,
) -> Result<()> {
    let jobs = if let Ok(mut client) =
        JobsClient::connect_with_timeout(&paths.socket_path, Duration::from_millis(500))
    {
        client.list_jobs(limit, include_terminal)?
    } else if paths.db_path.exists() {
        let store = JobStore::open(&paths.db_path)?;
        store.list_jobs(limit, include_terminal)?
    } else {
        Vec::new()
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&jobs)?);
        return Ok(());
    }

    if jobs.is_empty() {
        println!("No jobs found.");
        return Ok(());
    }

    println!(
        "{:<6} {:<22} {:<16} {:<10} {:<10} {:<10} SUMMARY",
        "ID", "STATE", "KIND", "TOTAL", "OK", "FAILED"
    );
    for j in jobs {
        let failed = j.counts.failed_items + j.counts.interrupted_items;
        println!(
            "{:<6} {:<22} {:<16} {:<10} {:<10} {:<10} {}",
            j.job_id.0,
            j.state.as_str(),
            j.kind,
            j.counts.total_items,
            j.counts.succeeded_items,
            failed,
            j.summary_text.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

fn cmd_show(paths: &WorkerRuntimePaths, job_id: JobId, json: bool) -> Result<()> {
    let (job, items, _total) = if let Ok(mut client) =
        JobsClient::connect_with_timeout(&paths.socket_path, Duration::from_millis(500))
    {
        client.get_job(job_id, None, None)?
    } else if paths.db_path.exists() {
        let store = JobStore::open(&paths.db_path)?;
        let Some(detail) = store.get_job_detail(job_id, None, None)? else {
            bail!("job {job_id} not found");
        };
        detail
    } else {
        bail!("job {job_id} not found (database does not exist)");
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "job": job,
                "items": items,
            }))?
        );
        return Ok(());
    }

    println!("Job #{}: {} ({})", job.job_id.0, job.state, job.kind);
    println!("  Origin:      {}", job.origin);
    if let Some(parent) = job.retry_of {
        println!("  Retry of:    #{}", parent.0);
    }
    println!("  Request ID:  {}", job.request_id);
    println!(
        "  Progress:    {} total ({} succeeded, {} skipped, {} failed, {} cancelled, {} interrupted, {} pending)",
        job.counts.total_items,
        job.counts.succeeded_items,
        job.counts.skipped_items,
        job.counts.failed_items,
        job.counts.cancelled_items,
        job.counts.interrupted_items,
        job.counts.pending_items
    );
    if let Some(ref sum) = job.summary_text {
        println!("  Summary:     {sum}");
    }
    println!("\nItems:");
    for it in items {
        let out_disp = it
            .output_path
            .as_ref()
            .map(|p| p.lossy_display())
            .unwrap_or_else(|| "-".to_string());
        print!(
            "  [#{:04}] {:<12} {} -> {}",
            it.item_index,
            it.state.as_str(),
            it.source_path.lossy_display(),
            out_disp
        );
        if let Some(ref err) = it.error_message {
            print!(" (error: {err})");
        }
        println!();
    }
    Ok(())
}

fn cmd_cancel(paths: &WorkerRuntimePaths, job_id: JobId, json: bool) -> Result<()> {
    let mut client = JobsClient::connect_or_launch(paths)?;
    let state = client.cancel(job_id)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "job_id": job_id,
                "state": state,
            }))?
        );
    } else {
        println!("Job #{job_id} state: {state}");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_retry(
    paths: &WorkerRuntimePaths,
    job_id: JobId,
    mode: RetryMode,
    to: Option<&str>,
    quality: Option<&str>,
    output_dir: Option<&std::path::Path>,
    request_id: Option<String>,
    wait: bool,
    json: bool,
) -> Result<()> {
    let options_override = if to.is_some() || quality.is_some() || output_dir.is_some() {
        Some(build_convert_options_json(to, None, quality, output_dir)?)
    } else {
        None
    };

    let mut client = JobsClient::connect_or_launch(paths)?;
    let (new_job_id, state, reset_items, deduplicated) = match request_id {
        Some(s) => client.retry_with_request_id(RequestId(s), job_id, mode, options_override)?,
        None => {
            let (nid, st, cnt) = client.retry(job_id, mode, options_override)?;
            (nid, st, cnt, false)
        }
    };

    if wait {
        wait_for_job_completion(paths, new_job_id)?;
        return cmd_show(paths, new_job_id, json);
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "job_id": new_job_id,
                "retry_of": job_id,
                "state": state,
                "reset_items": reset_items,
                "deduplicated": deduplicated,
            }))?
        );
    } else {
        println!(
            "Created retry job #{new_job_id} (retry of job #{job_id}): queued {reset_items} item(s), state={state}"
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_submit_convert(
    paths: &WorkerRuntimePaths,
    to: Option<&str>,
    preset: Option<&str>,
    quality: Option<&str>,
    output_dir: Option<&std::path::Path>,
    request_id: Option<String>,
    wait: bool,
    json: bool,
    files: &[PathBuf],
) -> Result<()> {
    let options_json = build_convert_options_json(to, preset, quality, output_dir)?;
    let mut wire_items = Vec::with_capacity(files.len());
    for f in files {
        let abs = if f.is_absolute() {
            f.clone()
        } else {
            env::current_dir()
                .context("failed to resolve current working directory")?
                .join(f)
        };
        wire_items.push(WirePath::from_absolute_path(&abs)?);
    }

    let req_id = match request_id {
        Some(s) => RequestId(s),
        None => RequestId::generate(),
    };

    let mut client = JobsClient::connect_or_launch(paths)?;
    let (job_id, state, deduplicated) = client.submit(
        req_id,
        Origin::Cli,
        CONVERT_IMAGE_KIND,
        wire_items,
        options_json,
    )?;

    if wait {
        wait_for_job_completion(paths, job_id)?;
        return cmd_show(paths, job_id, json);
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "job_id": job_id,
                "state": state,
                "deduplicated": deduplicated,
            }))?
        );
    } else {
        println!("Submitted job #{job_id} (state={state}, deduplicated={deduplicated})");
    }
    Ok(())
}

fn cmd_shutdown_if_idle(paths: &WorkerRuntimePaths, json: bool) -> Result<()> {
    match JobsClient::connect_with_timeout(&paths.socket_path, Duration::from_millis(500)) {
        Ok(mut client) => {
            let (shutting_down, active_jobs) = client.shutdown_if_idle()?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "shutting_down": shutting_down,
                        "active_jobs": active_jobs,
                    }))?
                );
            } else if shutting_down {
                println!("Worker is idle and shutting down cleanly.");
            } else {
                println!("Worker has {active_jobs} active job(s); not shutting down.");
            }
        }
        Err(_) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "shutting_down": true,
                        "active_jobs": 0,
                    }))?
                );
            } else {
                println!("Worker is already offline.");
            }
        }
    }
    Ok(())
}

fn wait_for_job_completion(paths: &WorkerRuntimePaths, job_id: JobId) -> Result<()> {
    let sub_client = JobsClient::connect(&paths.socket_path)?;
    let (active_jobs, mut sub) = sub_client.subscribe(Some(job_id))?;
    // Check if the job already finished before our subscription registered
    if !active_jobs.iter().any(|j| j.job_id == job_id) {
        let mut check_client = JobsClient::connect(&paths.socket_path)?;
        let (job, _, _) = check_client.get_job(job_id, Some(0), Some(1))?;
        if job.state.is_terminal() {
            return Ok(());
        }
    }

    let _ = sub.set_read_timeout(Some(Duration::from_secs(120)));
    while let Some(ev) = sub.next_event()? {
        if let JobUpdate::JobFinished { job, .. } = ev
            && job.job_id == job_id
        {
            break;
        }
    }
    Ok(())
}

fn build_convert_options_json(
    to: Option<&str>,
    preset: Option<&str>,
    quality: Option<&str>,
    output_dir: Option<&std::path::Path>,
) -> Result<serde_json::Value> {
    let mut overrides = PresetOverrides::default();
    let target_opt = if let Some(t_str) = to {
        let fmt = match t_str.trim().to_ascii_lowercase().as_str() {
            "png" => FormatId::Png,
            "jpg" | "jpeg" => FormatId::Jpeg,
            "webp" => FormatId::WebP,
            other => bail!("unsupported target format '{other}' (expected png, jpeg, or webp)"),
        };
        Some(fmt)
    } else {
        None
    };

    if let Some(dir) = output_dir {
        let abs = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            env::current_dir()
                .context("failed to resolve current working directory")?
                .join(dir)
        };
        overrides.output_location = Some(OutputLocation::Directory(abs));
    }

    if let Some(q_str) = quality {
        let q = match q_str.trim().to_ascii_lowercase().as_str() {
            "max" | "maximum" => QualitySetting::Intent(QualityIntent::Maximum),
            "high" => QualitySetting::Intent(QualityIntent::High),
            "balanced" => QualitySetting::Intent(QualityIntent::Balanced),
            "compact" | "smaller" => QualitySetting::Intent(QualityIntent::Compact),
            "lossless" => QualitySetting::Intent(QualityIntent::Lossless),
            other => {
                let num: u8 = other.parse().with_context(|| {
                    format!(
                        "invalid --quality '{other}' (expected max|high|balanced|compact|lossless or 1..=100)"
                    )
                })?;
                if !(1..=100).contains(&num) {
                    bail!("numeric --quality must be in 1..=100");
                }
                let target_fmt = target_opt.unwrap_or(FormatId::WebP);
                match target_fmt {
                    FormatId::Jpeg => {
                        QualitySetting::PerFormat(radivine_convert::FormatQuality::Jpeg {
                            quality: num,
                            progressive: true,
                            optimize_coding: true,
                            subsampling: radivine_convert::ChromaSubsampling::Auto,
                        })
                    }
                    FormatId::WebP => {
                        QualitySetting::PerFormat(radivine_convert::FormatQuality::WebP {
                            mode: radivine_convert::WebPMode::Lossy,
                            quality: num,
                            alpha_quality: 100,
                            method: 4,
                        })
                    }
                    FormatId::Png => QualitySetting::Intent(QualityIntent::Lossless),
                    _ => bail!("numeric quality not supported for target {target_fmt}"),
                }
            }
        };
        overrides.quality = Some(q);
    }

    Ok(serde_json::json!({
        "target": target_opt,
        "preset_ref": preset,
        "overrides": overrides,
    }))
}
