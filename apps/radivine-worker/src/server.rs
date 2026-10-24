//! Unix domain socket IPC server, `worker.lock` single-instance enforcement, `SO_PEERCRED` check,
//! and race-free idle / `SIGTERM` shutdown (`JOB_SYSTEM_DESIGN.md` §2.2–§2.3, §5, `SECURITY_AND_FILE_SAFETY.md` §7).

use std::env;
use std::fs::{self, File};
use std::io::{self, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use radivine_convert::ConvertSettings;
use radivine_jobs::{
    FrameReadError, JobUpdate, JobsSettings, MAX_ITEMS_PER_JOB, PROTOCOL_VERSION, WorkerErrorCode,
    WorkerRequest, WorkerResponse, read_worker_request,
};
use rustix::fs::{FlockOperation, Mode, OFlags, flock, ftruncate, openat};
use tracing::{debug, info, warn};

use crate::governor::ResourceGovernor;
use crate::handler::{ConvertImageHandler, JobHandler};
use crate::recovery::{read_current_boot_id, run_startup_recovery};
use crate::scheduler::{EventHub, JobScheduler};
use crate::store::{CURRENT_SCHEMA_VERSION, JobStore, StoreError};

/// Configuration for running the `radivine-worker` server loop.
#[derive(Debug, Clone)]
pub struct WorkerServerConfig {
    pub socket_path: PathBuf,
    pub lock_path: PathBuf,
    pub db_path: PathBuf,
    pub idle_timeout: Duration,
    pub wait_for_lock_ms: u64,
}

/// Acquires `worker.lock` with `O_CREAT | O_RDWR | O_CLOEXEC` (`0600`) and `flock(LOCK_EX | LOCK_NB)`.
///
/// Returns `Ok(Some(File))` if the lock was acquired, or `Ok(None)` if another worker instance
/// already holds the lock (`AC-C2.5` single-instance invariant).
pub fn acquire_single_instance_lock(
    lock_path: &Path,
    wait_for_lock_ms: u64,
) -> io::Result<Option<File>> {
    if let Some(parent) = lock_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }

    let owned_fd = openat(
        rustix::fs::CWD,
        lock_path,
        OFlags::CREATE | OFlags::RDWR | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))?;

    let mut file = File::from(owned_fd);
    let _ = fs::set_permissions(lock_path, fs::Permissions::from_mode(0o600));

    let start = Instant::now();
    let deadline = start + Duration::from_millis(wait_for_lock_ms);
    loop {
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {
                let _ = ftruncate(&file, 0);
                let _ = writeln!(file, "{}", std::process::id());
                let _ = file.flush();
                return Ok(Some(file));
            }
            Err(rustix::io::Errno::WOULDBLOCK | rustix::io::Errno::ACCESS) => {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                thread::sleep(Duration::from_millis(15));
            }
            Err(errno) => {
                return Err(io::Error::from_raw_os_error(errno.raw_os_error()));
            }
        }
    }
}

fn bind_worker_socket(socket_path: &Path) -> io::Result<UnixListener> {
    if let Some(parent) = socket_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }
    if socket_path.exists() || fs::symlink_metadata(socket_path).is_ok() {
        let _ = fs::remove_file(socket_path);
    }
    let listener = UnixListener::bind(socket_path)?;
    let _ = fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600));
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn verify_peer_credentials(stream: &UnixStream) -> bool {
    let my_uid = rustix::process::getuid().as_raw();
    if let Ok(fake_uid_str) = env::var("RADIVINE_WORKER_TEST_FAKE_PEER_UID")
        && let Ok(fake_uid) = fake_uid_str.trim().parse::<u32>()
    {
        return fake_uid == my_uid;
    }

    match rustix::net::sockopt::socket_peercred(stream) {
        Ok(cred) => cred.uid.as_raw() == my_uid,
        Err(_) => false,
    }
}

/// Runs the `radivine-worker` daemon until idle timeout, `ShutdownIfIdle`, or `SIGTERM`/`SIGINT`.
pub fn run_server(cfg: WorkerServerConfig) -> anyhow::Result<()> {
    // 1. Acquire single-instance worker.lock BEFORE opening SQLite or unlinking worker.sock
    let Some(_lock_file) = acquire_single_instance_lock(&cfg.lock_path, cfg.wait_for_lock_ms)?
    else {
        info!(
            lock = %cfg.lock_path.display(),
            "another radivine-worker instance already holds worker.lock; exiting 0"
        );
        return Ok(());
    };

    // 2. Register SIGTERM and SIGINT flags
    let term_flag = Arc::new(AtomicBool::new(false));
    let _ = signal_hook::flag::register(signal_hook::consts::SIGTERM, term_flag.clone());
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, term_flag.clone());

    // 3. Open SQLite WAL JobStore and execute C0a-C6 startup crash recovery BEFORE binding socket
    let store = Arc::new(JobStore::open(&cfg.db_path)?);
    let boot_id = read_current_boot_id();
    let _recovery = run_startup_recovery(&store, &boot_id)?;

    // 4. Load [jobs] and convert.toml settings and run retention pruning
    let (jobs_settings, _) = JobsSettings::load_lenient(&JobsSettings::default_config_path());
    let (convert_settings, _) =
        ConvertSettings::load_or_default(&ConvertImageHandler::resolve_convert_toml_path());
    let _ = store.prune_retention(
        jobs_settings.retention_max_jobs,
        jobs_settings.retention_days,
    );

    let max_parallel = if let Ok(mp_str) = env::var("RADIVINE_WORKER_MAX_PARALLEL")
        && let Ok(mp) = mp_str.trim().parse::<usize>()
    {
        mp.clamp(1, 16)
    } else {
        jobs_settings.max_parallel.resolve()
    };

    let memory_budget_bytes = if let Ok(mb_str) = env::var("RADIVINE_WORKER_MEMORY_BUDGET_MB")
        && let Ok(mb) = mb_str.trim().parse::<u64>()
    {
        mb.clamp(16, 4096) * 1024 * 1024
    } else {
        jobs_settings.memory_budget_mb.resolve_bytes()
    };
    let worker_memory_max_bytes = convert_settings
        .limits
        .worker_memory_max_bytes
        .max(memory_budget_bytes);

    let governor = ResourceGovernor::new(memory_budget_bytes, worker_memory_max_bytes);
    let events = Arc::new(EventHub::new());
    let handlers: Vec<Arc<dyn JobHandler>> = vec![Arc::new(ConvertImageHandler::new())];

    let recycle_args = vec![
        "serve".to_string(),
        "--socket".to_string(),
        cfg.socket_path.to_string_lossy().into_owned(),
        "--db".to_string(),
        cfg.db_path.to_string_lossy().into_owned(),
        "--lock".to_string(),
        cfg.lock_path.to_string_lossy().into_owned(),
        "--wait-for-lock-ms".to_string(),
        "3000".to_string(),
    ];

    let scheduler = JobScheduler::start(
        store.clone(),
        governor,
        handlers,
        events,
        max_parallel,
        Some(recycle_args),
    );

    // 5. Bind Unix domain socket (0600) only after C0a-C6 recovery has finished
    let mut listener = bind_worker_socket(&cfg.socket_path)?;
    info!(
        socket = %cfg.socket_path.display(),
        db = %cfg.db_path.display(),
        max_parallel,
        memory_budget_mb = memory_budget_bytes / (1024 * 1024),
        "radivine-worker listening"
    );

    // Wake scheduler in case startup recovery left resumed jobs in Queued/Running
    scheduler.notify_work_available();

    let active_clients = Arc::new(AtomicUsize::new(0));
    let explicit_shutdown_requested = Arc::new(AtomicBool::new(false));

    loop {
        // Check SIGTERM / SIGINT
        if term_flag.load(Ordering::Relaxed) {
            info!("SIGTERM/SIGINT received; draining in-flight work and shutting down cleanly");
            let _ = fs::remove_file(&cfg.socket_path);
            let drain_ms = env::var("RADIVINE_WORKER_SIGTERM_DRAIN_MS")
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(5_500);
            scheduler.shutdown_gracefully(Duration::from_millis(drain_ms));
            let _ = store.finish_clean_shutdown();
            return Ok(());
        }

        // Accept pending client connections
        loop {
            match listener.accept() {
                Ok((stream, _addr)) => {
                    if !verify_peer_credentials(&stream) {
                        warn!("rejected worker connection with mismatched SO_PEERCRED UID");
                        drop(stream);
                        continue;
                    }
                    scheduler.touch_activity();
                    active_clients.fetch_add(1, Ordering::SeqCst);
                    let store_c = store.clone();
                    let sched_c = scheduler.clone();
                    let active_c = active_clients.clone();
                    let shutdown_c = explicit_shutdown_requested.clone();
                    thread::spawn(move || {
                        handle_client_connection(stream, &store_c, &sched_c, &shutdown_c);
                        active_c.fetch_sub(1, Ordering::SeqCst);
                        sched_c.touch_activity();
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    debug!(error = %e, "accept error on worker socket");
                    break;
                }
            }
        }

        // Check explicit ShutdownIfIdle or automatic idle timeout
        let want_explicit_exit = explicit_shutdown_requested.load(Ordering::SeqCst);
        let want_idle_exit = !want_explicit_exit
            && scheduler.idle_duration() >= cfg.idle_timeout
            && active_clients.load(Ordering::SeqCst) == 0
            && scheduler.events().subscriber_count() == 0
            && scheduler.in_flight_count() == 0;

        if want_explicit_exit || want_idle_exit {
            let (active_jobs, queued_items) = store.active_counts().unwrap_or((0, 0));
            if active_jobs == 0 && queued_items == 0 && scheduler.in_flight_count() == 0 {
                // Race-free idle shutdown protocol (JOB_SYSTEM_DESIGN.md §2.3):
                // 1. Unlink worker.sock while holding worker.lock so new clients see ENOENT and relaunch.
                let _ = fs::remove_file(&cfg.socket_path);

                // 2. Drain any late connection sitting in the kernel backlog before unlink took effect.
                let mut late_activity = false;
                while let Ok((stream, _)) = listener.accept() {
                    if verify_peer_credentials(&stream) {
                        late_activity = true;
                        handle_client_connection(
                            stream,
                            &store,
                            &scheduler,
                            &explicit_shutdown_requested,
                        );
                    }
                }

                let (recheck_jobs, recheck_items) = store.active_counts().unwrap_or((0, 0));
                if (late_activity && !want_explicit_exit)
                    || recheck_jobs > 0
                    || recheck_items > 0
                    || scheduler.in_flight_count() > 0
                {
                    // A late client submitted new work! Re-bind worker.sock and continue running.
                    explicit_shutdown_requested.store(false, Ordering::SeqCst);
                    listener = bind_worker_socket(&cfg.socket_path)?;
                    scheduler.touch_activity();
                    continue;
                }

                // 3. Cleanly stop worker threads, checkpoint WAL, and exit 0.
                scheduler.shutdown_gracefully(Duration::from_secs(2));
                let _ = store.prune_retention(
                    jobs_settings.retention_max_jobs,
                    jobs_settings.retention_days,
                );
                let _ = store.finish_clean_shutdown();
                info!("radivine-worker exiting cleanly on idle");
                return Ok(());
            } else {
                explicit_shutdown_requested.store(false, Ordering::SeqCst);
                scheduler.touch_activity();
            }
        }

        thread::sleep(Duration::from_millis(25));
    }
}

fn handle_client_connection(
    stream: UnixStream,
    store: &Arc<JobStore>,
    scheduler: &Arc<JobScheduler>,
    explicit_shutdown_requested: &Arc<AtomicBool>,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);

    loop {
        let req = match read_worker_request(&mut reader) {
            Ok(r) => r,
            Err(FrameReadError::Eof) => return,
            Err(FrameReadError::MessageTooLarge { max_bytes }) => {
                let _ = write_response(
                    &mut writer,
                    &WorkerResponse::Error {
                        v: PROTOCOL_VERSION,
                        code: WorkerErrorCode::MessageTooLarge,
                        message: format!("request frame exceeds maximum size of {max_bytes} bytes"),
                    },
                );
                return;
            }
            Err(FrameReadError::MalformedJson(e)) => {
                let msg = e.to_string();
                let code = if msg.contains("unknown variant") && msg.contains("kind") {
                    WorkerErrorCode::UnknownJobKind
                } else {
                    WorkerErrorCode::MalformedRequest
                };
                let _ = write_response(
                    &mut writer,
                    &WorkerResponse::Error {
                        v: PROTOCOL_VERSION,
                        code,
                        message: format!("malformed request JSON: {e}"),
                    },
                );
                return;
            }
            Err(FrameReadError::InvalidUtf8(e)) => {
                let _ = write_response(
                    &mut writer,
                    &WorkerResponse::Error {
                        v: PROTOCOL_VERSION,
                        code: WorkerErrorCode::MalformedRequest,
                        message: format!("invalid UTF-8 in request: {e}"),
                    },
                );
                return;
            }
            Err(FrameReadError::Io(_)) => return,
        };

        scheduler.touch_activity();

        if req.version() != PROTOCOL_VERSION {
            let _ = write_response(
                &mut writer,
                &WorkerResponse::Error {
                    v: PROTOCOL_VERSION,
                    code: WorkerErrorCode::UnsupportedProtocolVersion,
                    message: format!(
                        "unsupported protocol version {}; worker supports v={PROTOCOL_VERSION}",
                        req.version()
                    ),
                },
            );
            continue;
        }

        match req {
            WorkerRequest::Ping { .. } => {
                let (active_jobs, queued_items) = store.active_counts().unwrap_or((0, 0));
                if write_response(
                    &mut writer,
                    &WorkerResponse::Pong {
                        v: PROTOCOL_VERSION,
                        pid: std::process::id(),
                        version: env!("CARGO_PKG_VERSION").to_string(),
                        active_jobs,
                        queued_items,
                        schema_version: CURRENT_SCHEMA_VERSION,
                    },
                )
                .is_err()
                {
                    return;
                }
            }
            WorkerRequest::Submit {
                request_id,
                origin,
                kind,
                items,
                options,
                ..
            } => {
                let resp = handle_submit(
                    store,
                    scheduler,
                    &request_id,
                    origin,
                    &kind,
                    &items,
                    &options,
                );
                if write_response(&mut writer, &resp).is_err() {
                    return;
                }
            }
            WorkerRequest::Cancel { job_id, .. } => {
                let resp = match scheduler.cancel_job(job_id) {
                    Ok(summary) => WorkerResponse::Cancelled {
                        v: PROTOCOL_VERSION,
                        job_id,
                        state: summary.state,
                    },
                    Err(e) => map_store_error(e),
                };
                if write_response(&mut writer, &resp).is_err() {
                    return;
                }
            }
            WorkerRequest::Retry {
                job_id,
                mode,
                options_override,
                ..
            } => {
                let resp = match store.retry_job(job_id, mode, options_override.as_ref()) {
                    Ok((summary, reset_items)) => {
                        scheduler.on_job_retried(job_id, &summary);
                        WorkerResponse::Retried {
                            v: PROTOCOL_VERSION,
                            job_id,
                            state: summary.state,
                            reset_items,
                        }
                    }
                    Err(e) => map_store_error(e),
                };
                if write_response(&mut writer, &resp).is_err() {
                    return;
                }
            }
            WorkerRequest::GetJob {
                job_id,
                item_offset,
                item_limit,
                ..
            } => {
                let resp = match store.get_job_detail(job_id, item_offset, item_limit) {
                    Ok(Some((job, items, total_items))) => WorkerResponse::JobDetail {
                        v: PROTOCOL_VERSION,
                        job: Box::new(job),
                        items,
                        total_items,
                    },
                    Ok(None) => WorkerResponse::Error {
                        v: PROTOCOL_VERSION,
                        code: WorkerErrorCode::JobNotFound,
                        message: format!("job {job_id} not found"),
                    },
                    Err(e) => map_store_error(e),
                };
                if write_response(&mut writer, &resp).is_err() {
                    return;
                }
            }
            WorkerRequest::ListJobs {
                limit,
                include_terminal,
                ..
            } => {
                let resp = match store.list_jobs(limit, include_terminal) {
                    Ok(jobs) => WorkerResponse::JobList {
                        v: PROTOCOL_VERSION,
                        jobs,
                    },
                    Err(e) => map_store_error(e),
                };
                if write_response(&mut writer, &resp).is_err() {
                    return;
                }
            }
            WorkerRequest::ShutdownIfIdle { .. } => {
                let (active_jobs, queued_items) = store.active_counts().unwrap_or((0, 0));
                let shutting_down =
                    active_jobs == 0 && queued_items == 0 && scheduler.in_flight_count() == 0;
                if shutting_down {
                    explicit_shutdown_requested.store(true, Ordering::SeqCst);
                }
                let _ = write_response(
                    &mut writer,
                    &WorkerResponse::ShutdownAck {
                        v: PROTOCOL_VERSION,
                        shutting_down,
                        active_jobs,
                    },
                );
                return;
            }
            WorkerRequest::Subscribe { job_id, .. } => {
                let active_jobs = store.list_active_jobs().unwrap_or_default();
                if write_response(
                    &mut writer,
                    &WorkerResponse::Subscribed {
                        v: PROTOCOL_VERSION,
                        active_jobs,
                    },
                )
                .is_ok()
                {
                    scheduler.events().add_subscriber(job_id, writer);
                }
                // Handed off writer stream to EventHub; exit request loop on this connection.
                return;
            }
        }
    }
}

fn handle_submit(
    store: &Arc<JobStore>,
    scheduler: &Arc<JobScheduler>,
    request_id: &radivine_jobs::RequestId,
    origin: radivine_jobs::Origin,
    kind: &str,
    items: &[radivine_jobs::WirePath],
    options: &serde_json::Value,
) -> WorkerResponse {
    if let Err(msg) = request_id.validate() {
        return WorkerResponse::Error {
            v: PROTOCOL_VERSION,
            code: WorkerErrorCode::InvalidRequestId,
            message: msg,
        };
    }
    if items.is_empty() || items.len() > MAX_ITEMS_PER_JOB {
        return WorkerResponse::Error {
            v: PROTOCOL_VERSION,
            code: WorkerErrorCode::TooManyItems,
            message: format!(
                "submit items count {} must be between 1 and {MAX_ITEMS_PER_JOB}",
                items.len()
            ),
        };
    }
    if !scheduler.has_handler(kind) {
        return WorkerResponse::Error {
            v: PROTOCOL_VERSION,
            code: WorkerErrorCode::UnknownJobKind,
            message: format!("unknown job kind '{kind}'"),
        };
    }

    let mut decoded_paths = Vec::with_capacity(items.len());
    for wp in items {
        match wp.to_absolute_path_buf() {
            Ok(pb) => decoded_paths.push(pb),
            Err(e) => {
                return WorkerResponse::Error {
                    v: PROTOCOL_VERSION,
                    code: WorkerErrorCode::InvalidPath,
                    message: format!("invalid item path '{wp}': {e}"),
                };
            }
        }
    }

    match store.submit_or_deduplicate(request_id, origin, kind, &decoded_paths, options) {
        Ok((job_id, state, deduplicated)) => {
            if !deduplicated {
                if let Ok(Some(job_sum)) = store.get_job_summary(job_id) {
                    scheduler.events().publish(&JobUpdate::JobCreated {
                        v: PROTOCOL_VERSION,
                        job: Box::new(job_sum),
                    });
                }
                scheduler.notify_work_available();
            }
            WorkerResponse::Submitted {
                v: PROTOCOL_VERSION,
                job_id,
                state,
                deduplicated,
            }
        }
        Err(e) => map_store_error(e),
    }
}

fn map_store_error(err: StoreError) -> WorkerResponse {
    match err {
        StoreError::Protocol { code, message } => WorkerResponse::Error {
            v: PROTOCOL_VERSION,
            code,
            message,
        },
        StoreError::Sqlite(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::DatabaseBusy
                || e.code == rusqlite::ErrorCode::DatabaseLocked =>
        {
            WorkerResponse::Error {
                v: PROTOCOL_VERSION,
                code: WorkerErrorCode::DatabaseBusy,
                message: "SQLite database busy".to_string(),
            }
        }
        other => WorkerResponse::Error {
            v: PROTOCOL_VERSION,
            code: WorkerErrorCode::InternalError,
            message: other.to_string(),
        },
    }
}

fn write_response(writer: &mut UnixStream, resp: &WorkerResponse) -> io::Result<()> {
    let mut line = serde_json::to_string(resp).map_err(io::Error::other)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}
