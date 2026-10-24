//! IPC client, progress subscription handle, and on-demand worker launcher (`JOB_SYSTEM_DESIGN.md` §2.2, §5).

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::protocol::{
    FrameReadError, JobItemReport, JobSummary, JobUpdate, MAX_EVENT_LINE_BYTES,
    MAX_RESPONSE_LINE_BYTES, PROTOCOL_VERSION, WorkerErrorCode, WorkerRequest, WorkerResponse,
    read_bounded_line,
};
use crate::state::{JobId, JobState, Origin, RequestId, RetryMode};
use crate::wire_path::WirePath;

const MAX_WORKER_LOG_BYTES: u64 = 2 * 1024 * 1024;

/// Errors returned by [`JobsClient`], [`Subscription`], and [`WorkerLauncher`].
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("failed to connect to worker socket {}: {source}", path.display())]
    Connect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("worker launch timed out after {elapsed:?} waiting for {}", socket.display())]
    LaunchTimeout {
        socket: PathBuf,
        elapsed: Duration,
        detail: String,
    },
    #[error("failed to spawn worker binary {}: {source}", bin.display())]
    SpawnFailed {
        bin: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("I/O error communicating with worker: {0}")]
    Io(#[from] io::Error),
    #[error("protocol frame error: {0}")]
    Frame(#[from] FrameReadError),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("worker returned error [{code:?}]: {message}")]
    WorkerError {
        code: WorkerErrorCode,
        message: String,
    },
    #[error("unexpected worker response variant: {0}")]
    UnexpectedResponse(String),
}

/// Launch strategy override (`RADIVINE_WORKER_LAUNCH=systemd|spawn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkerLaunchMode {
    #[default]
    Auto,
    Systemd,
    Spawn,
}

impl WorkerLaunchMode {
    #[must_use]
    pub fn from_env() -> Self {
        if env::var("RADIVINE_DISABLE_SYSTEMD_USER_SPAWN")
            .ok()
            .is_some_and(|v| v.trim() == "1" || v.eq_ignore_ascii_case("true"))
        {
            return Self::Spawn;
        }
        match env::var("RADIVINE_WORKER_LAUNCH")
            .ok()
            .as_deref()
            .map(str::trim)
        {
            Some("systemd") => Self::Systemd,
            Some("spawn") => Self::Spawn,
            _ => Self::Auto,
        }
    }
}

/// Resolved filesystem paths for the worker socket, lockfile, SQLite database, and log file.
#[derive(Debug, Clone)]
pub struct WorkerRuntimePaths {
    pub socket_path: PathBuf,
    pub lock_path: PathBuf,
    pub db_path: PathBuf,
    pub log_path: PathBuf,
    pub worker_bin: Option<PathBuf>,
    pub launch_mode: WorkerLaunchMode,
}

impl Default for WorkerRuntimePaths {
    fn default() -> Self {
        Self::from_env()
    }
}

impl WorkerRuntimePaths {
    /// Resolves worker paths from `RADIVINE_WORKER_*` and `XDG_*` environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        let runtime_dir = env::var("XDG_RUNTIME_DIR")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let uid = rustix::process::getuid().as_raw();
                PathBuf::from(format!("/tmp/radivine-runtime-{uid}"))
            })
            .join("radivine");

        let state_dir = env::var("XDG_STATE_HOME")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                env::var("HOME")
                    .ok()
                    .filter(|s| !s.trim().is_empty())
                    .map(|h| PathBuf::from(h).join(".local").join("state"))
            })
            .unwrap_or_else(|| {
                let uid = rustix::process::getuid().as_raw();
                PathBuf::from(format!("/tmp/radivine-state-{uid}"))
            })
            .join("radivine");

        let socket_path = env::var("RADIVINE_WORKER_SOCKET")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| runtime_dir.join("worker.sock"));

        let lock_path = env::var("RADIVINE_WORKER_LOCK")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                socket_path
                    .parent()
                    .unwrap_or(&runtime_dir)
                    .join("worker.lock")
            });

        let db_path = env::var("RADIVINE_WORKER_DB")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| state_dir.join("jobs.sqlite3"));

        let log_path = env::var("RADIVINE_WORKER_LOG")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| db_path.parent().unwrap_or(&state_dir).join("worker.log"));

        let worker_bin = env::var("RADIVINE_WORKER_BIN")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from);

        Self {
            socket_path,
            lock_path,
            db_path,
            log_path,
            worker_bin,
            launch_mode: WorkerLaunchMode::from_env(),
        }
    }

    /// Resolves the `radivine-worker` executable path.
    #[must_use]
    pub fn resolve_worker_binary(&self) -> PathBuf {
        if let Some(ref b) = self.worker_bin {
            return b.clone();
        }
        if let Ok(exe) = env::current_exe() {
            if exe
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == "radivine-worker")
            {
                return exe;
            }
            if let Some(dir) = exe.parent() {
                let sibling = dir.join("radivine-worker");
                if sibling.is_file() {
                    return sibling;
                }
                if let Some(parent_dir) = dir.parent() {
                    let parent_sibling = parent_dir.join("radivine-worker");
                    if parent_sibling.is_file() {
                        return parent_sibling;
                    }
                }
            }
        }
        PathBuf::from("radivine-worker")
    }
}

/// On-demand launcher for `radivine-worker` (`JOB_SYSTEM_DESIGN.md` §2.2).
#[derive(Debug, Default, Clone)]
pub struct WorkerLauncher;

impl WorkerLauncher {
    /// Ensures a live worker is listening at `paths.socket_path`, launching one on demand if needed.
    pub fn ensure_running(paths: &WorkerRuntimePaths) -> Result<(), ClientError> {
        if Self::try_ping(&paths.socket_path, Duration::from_millis(200)) {
            return Ok(());
        }

        let start = Instant::now();
        let mut detail = String::new();

        match paths.launch_mode {
            WorkerLaunchMode::Systemd => {
                Self::launch_via_systemd(paths, true, &mut detail)?;
            }
            WorkerLaunchMode::Spawn => {
                Self::launch_via_detached_spawn(paths)?;
            }
            WorkerLaunchMode::Auto => {
                if Self::launch_via_systemd(paths, true, &mut detail).is_err() {
                    Self::launch_via_detached_spawn(paths)?;
                }
            }
        }

        let deadline = start + Duration::from_secs(3);
        while Instant::now() < deadline {
            if Self::try_ping(&paths.socket_path, Duration::from_millis(200)) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(25));
        }

        if Self::try_ping(&paths.socket_path, Duration::from_millis(250)) {
            return Ok(());
        }

        Err(ClientError::LaunchTimeout {
            socket: paths.socket_path.clone(),
            elapsed: start.elapsed(),
            detail,
        })
    }

    fn try_ping(socket_path: &Path, timeout: Duration) -> bool {
        let Ok(mut client) = JobsClient::connect_with_timeout(socket_path, timeout) else {
            return false;
        };
        client.ping().is_ok()
    }

    fn launch_via_systemd(
        paths: &WorkerRuntimePaths,
        allow_transient_unit: bool,
        detail: &mut String,
    ) -> Result<(), ClientError> {
        let has_custom_paths = env::var("RADIVINE_WORKER_SOCKET").is_ok()
            || env::var("RADIVINE_WORKER_DB").is_ok()
            || paths.worker_bin.is_some();

        if !has_custom_paths {
            let status = Command::new("systemctl")
                .args(["--user", "start", "--no-block", "radivine-worker.service"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if let Ok(st) = status
                && st.success()
            {
                return Ok(());
            }
        }

        if allow_transient_unit {
            let bin = paths.resolve_worker_binary();
            let unit_name = env::var("RADIVINE_WORKER_SYSTEMD_UNIT")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("radivine-worker-transient-{:016x}", fastrand::u64(..)));
            let mut cmd = Command::new("systemd-run");
            cmd.args(["--user", "--collect", &format!("--unit={unit_name}")]);
            for var_name in [
                "RADIVINE_WORKER_IDLE_TIMEOUT_MS",
                "RADIVINE_SUPPRESS_DESKTOP_NOTIFY",
                "RADIVINE_WORKER_TEST_ITEM_SLEEP_MS",
                "RADIVINE_WORKER_ITEM_TIMEOUT_MS",
                "XDG_CONFIG_HOME",
                "XDG_RUNTIME_DIR",
                "XDG_STATE_HOME",
            ] {
                if let Ok(val) = env::var(var_name) {
                    cmd.arg(format!("--setenv={var_name}={val}"));
                }
            }
            cmd.arg(&bin)
                .arg("serve")
                .arg("--socket")
                .arg(&paths.socket_path)
                .arg("--db")
                .arg(&paths.db_path)
                .arg("--lock")
                .arg(&paths.lock_path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());

            let status = cmd
                .status()
                .map_err(|source| ClientError::SpawnFailed { bin, source })?;
            if status.success() {
                return Ok(());
            }
            *detail = format!("systemd-run exited with {status}");
        }

        Err(ClientError::LaunchTimeout {
            socket: paths.socket_path.clone(),
            elapsed: Duration::ZERO,
            detail: "systemd start failed".to_string(),
        })
    }

    fn launch_via_detached_spawn(paths: &WorkerRuntimePaths) -> Result<(), ClientError> {
        let bin = paths.resolve_worker_binary();
        if let Some(parent) = paths.log_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        rotate_log_if_needed(&paths.log_path);

        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&paths.log_path)
            .or_else(|_| File::open("/dev/null"))
            .map_err(|source| ClientError::SpawnFailed {
                bin: bin.clone(),
                source,
            })?;
        let log_err = log_file
            .try_clone()
            .map_err(|source| ClientError::SpawnFailed {
                bin: bin.clone(),
                source,
            })?;

        let mut cmd = Command::new(&bin);
        cmd.arg("serve")
            .arg("--socket")
            .arg(&paths.socket_path)
            .arg("--db")
            .arg(&paths.db_path)
            .arg("--lock")
            .arg(&paths.lock_path)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_err))
            .process_group(0);

        let _child = cmd
            .spawn()
            .map_err(|source| ClientError::SpawnFailed { bin, source })?;
        Ok(())
    }
}

fn rotate_log_if_needed(log_path: &Path) {
    if let Ok(meta) = fs::metadata(log_path)
        && meta.len() > MAX_WORKER_LOG_BYTES
    {
        let rotated = log_path.with_extension("log.1");
        let _ = fs::rename(log_path, rotated);
    }
}

/// Synchronous JSON-lines client for `radivine-worker`.
pub struct JobsClient {
    socket_path: PathBuf,
    timeout: Duration,
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl JobsClient {
    /// Connects to an already-running worker socket with a default 5-second timeout.
    pub fn connect(socket_path: &Path) -> Result<Self, ClientError> {
        Self::connect_with_timeout(socket_path, Duration::from_secs(5))
    }

    /// Connects to an already-running worker socket with an explicit read/write timeout.
    pub fn connect_with_timeout(
        socket_path: &Path,
        timeout: Duration,
    ) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(socket_path).map_err(|source| ClientError::Connect {
            path: socket_path.to_path_buf(),
            source,
        })?;
        let _ = stream.set_read_timeout(Some(timeout));
        let _ = stream.set_write_timeout(Some(timeout));
        let writer = stream.try_clone()?;
        Ok(Self {
            socket_path: socket_path.to_path_buf(),
            timeout,
            reader: BufReader::new(stream),
            writer,
        })
    }

    /// Connects to the worker, launching it on demand if not already running.
    pub fn connect_or_launch(paths: &WorkerRuntimePaths) -> Result<Self, ClientError> {
        if let Ok(mut client) =
            Self::connect_with_timeout(&paths.socket_path, Duration::from_secs(5))
            && client.ping().is_ok()
        {
            return Ok(client);
        }
        WorkerLauncher::ensure_running(paths)?;
        Self::connect(&paths.socket_path)
    }

    fn send_and_read_once(&mut self, line_bytes: &[u8]) -> Result<String, ClientError> {
        self.writer.write_all(line_bytes)?;
        self.writer.flush()?;
        Ok(read_bounded_line(
            &mut self.reader,
            MAX_RESPONSE_LINE_BYTES,
        )?)
    }

    /// Sends a [`WorkerRequest`] and reads the corresponding [`WorkerResponse`].
    pub fn request(&mut self, req: &WorkerRequest) -> Result<WorkerResponse, ClientError> {
        let mut line = serde_json::to_string(req)?;
        line.push('\n');
        let raw_resp = match self.send_and_read_once(line.as_bytes()) {
            Ok(s) => s,
            Err(
                ClientError::Io(_)
                | ClientError::Frame(FrameReadError::Eof | FrameReadError::Io(_)),
            ) if !matches!(req, WorkerRequest::Ping { .. }) => {
                // Reconnect once if the server closed an idle keep-alive connection between calls
                let fresh = Self::connect_with_timeout(&self.socket_path, self.timeout)?;
                self.reader = fresh.reader;
                self.writer = fresh.writer;
                self.send_and_read_once(line.as_bytes())?
            }
            Err(e) => return Err(e),
        };
        let resp: WorkerResponse = serde_json::from_str(&raw_resp)?;
        if let WorkerResponse::Error { code, message, .. } = resp {
            return Err(ClientError::WorkerError { code, message });
        }
        Ok(resp)
    }

    /// Sends a `ping` request and returns the `Pong` response.
    pub fn ping(&mut self) -> Result<WorkerResponse, ClientError> {
        self.request(&WorkerRequest::Ping {
            v: PROTOCOL_VERSION,
        })
    }

    /// Submits a new job to the worker.
    pub fn submit(
        &mut self,
        request_id: RequestId,
        origin: Origin,
        kind: impl Into<String>,
        items: Vec<WirePath>,
        options: serde_json::Value,
    ) -> Result<(JobId, JobState, bool), ClientError> {
        let resp = self.request(&WorkerRequest::Submit {
            v: PROTOCOL_VERSION,
            request_id,
            origin,
            kind: kind.into(),
            items,
            options,
        })?;
        match resp {
            WorkerResponse::Submitted {
                job_id,
                state,
                deduplicated,
                ..
            } => Ok((job_id, state, deduplicated)),
            other => Err(ClientError::UnexpectedResponse(format!("{other:?}"))),
        }
    }

    /// Requests cooperative cancellation of `job_id`.
    pub fn cancel(&mut self, job_id: JobId) -> Result<JobState, ClientError> {
        let resp = self.request(&WorkerRequest::Cancel {
            v: PROTOCOL_VERSION,
            job_id,
        })?;
        match resp {
            WorkerResponse::Cancelled { state, .. } => Ok(state),
            other => Err(ClientError::UnexpectedResponse(format!("{other:?}"))),
        }
    }

    /// Retries items of `job_id` according to `mode`.
    pub fn retry(
        &mut self,
        job_id: JobId,
        mode: RetryMode,
        options_override: Option<serde_json::Value>,
    ) -> Result<(JobState, u32), ClientError> {
        let resp = self.request(&WorkerRequest::Retry {
            v: PROTOCOL_VERSION,
            job_id,
            mode,
            options_override,
        })?;
        match resp {
            WorkerResponse::Retried {
                state, reset_items, ..
            } => Ok((state, reset_items)),
            other => Err(ClientError::UnexpectedResponse(format!("{other:?}"))),
        }
    }

    /// Queries detailed status and item reports for `job_id`.
    pub fn get_job(
        &mut self,
        job_id: JobId,
        item_offset: Option<u32>,
        item_limit: Option<u32>,
    ) -> Result<(JobSummary, Vec<JobItemReport>, u32), ClientError> {
        let resp = self.request(&WorkerRequest::GetJob {
            v: PROTOCOL_VERSION,
            job_id,
            item_offset,
            item_limit,
        })?;
        match resp {
            WorkerResponse::JobDetail {
                job,
                items,
                total_items,
                ..
            } => Ok((*job, items, total_items)),
            other => Err(ClientError::UnexpectedResponse(format!("{other:?}"))),
        }
    }

    /// Lists recent jobs.
    pub fn list_jobs(
        &mut self,
        limit: u32,
        include_terminal: bool,
    ) -> Result<Vec<JobSummary>, ClientError> {
        let resp = self.request(&WorkerRequest::ListJobs {
            v: PROTOCOL_VERSION,
            limit,
            include_terminal,
        })?;
        match resp {
            WorkerResponse::JobList { jobs, .. } => Ok(jobs),
            other => Err(ClientError::UnexpectedResponse(format!("{other:?}"))),
        }
    }

    /// Requests the worker to shut down if and only if no jobs are currently active.
    pub fn shutdown_if_idle(&mut self) -> Result<(bool, u32), ClientError> {
        let resp = self.request(&WorkerRequest::ShutdownIfIdle {
            v: PROTOCOL_VERSION,
        })?;
        match resp {
            WorkerResponse::ShutdownAck {
                shutting_down,
                active_jobs,
                ..
            } => Ok((shutting_down, active_jobs)),
            other => Err(ClientError::UnexpectedResponse(format!("{other:?}"))),
        }
    }

    /// Transitions this client connection into a push-only [`Subscription`] stream.
    pub fn subscribe(
        mut self,
        job_id: Option<JobId>,
    ) -> Result<(Vec<JobSummary>, Subscription), ClientError> {
        let resp = self.request(&WorkerRequest::Subscribe {
            v: PROTOCOL_VERSION,
            job_id,
        })?;
        match resp {
            WorkerResponse::Subscribed { active_jobs, .. } => {
                let _ = self.reader.get_ref().set_read_timeout(None);
                Ok((
                    active_jobs,
                    Subscription {
                        reader: self.reader,
                    },
                ))
            }
            other => Err(ClientError::UnexpectedResponse(format!("{other:?}"))),
        }
    }
}

/// Push-only subscription stream yielding [`JobUpdate`] events from the worker.
pub struct Subscription {
    reader: BufReader<UnixStream>,
}

impl Subscription {
    /// Sets an optional read timeout on the underlying Unix socket.
    pub fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.reader.get_ref().set_read_timeout(dur)
    }

    /// Reads the next [`JobUpdate`] event, or returns `Ok(None)` on clean EOF.
    pub fn next_event(&mut self) -> Result<Option<JobUpdate>, ClientError> {
        match read_bounded_line(&mut self.reader, MAX_EVENT_LINE_BYTES) {
            Ok(line) => {
                let ev: JobUpdate = serde_json::from_str(&line)?;
                Ok(Some(ev))
            }
            Err(FrameReadError::Eof) => Ok(None),
            Err(e) => Err(ClientError::Frame(e)),
        }
    }
}
