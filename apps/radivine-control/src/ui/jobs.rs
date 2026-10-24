//! Jobs page (`ControlPage::Jobs`, `CONVERSION_UX.md` §4) and non-blocking worker subscriber.
//!
//! Architecture:
//! - `JobsViewState` is a pure, deterministic view-model updated only via `apply_event(UiJobsEvent)`.
//! - All IPC (`radivine-jobs::JobsClient`), SQLite queries, and D-Bus `ShowItems` calls run on a
//!   dedicated background thread (`radivine-control-jobs`), never on the `egui` UI thread.
//! - Item lists are paginated (`ITEMS_PAGE_SIZE = 200` items/page) and virtualized using
//!   `ScrollArea::show_rows` so 10,000-item stress histories render in `< 5 ms` per frame (`AC-C4.2`).

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, CornerRadius, Layout, Margin, RichText, ScrollArea, Stroke,
};
use radivine_convert::{OutputLocation, PresetOverrides};
use radivine_dbus::call_show_items;
use radivine_jobs::{
    ItemState, JobId, JobItemReport, JobState, JobSummary, JobUpdate, JobsClient,
    MaxParallelSetting, OrphanedStageEntry, RetryMode, WorkerResponse, WorkerRuntimePaths,
};

use super::theme::{BrandPalette, section_card};

/// Supported SQLite job store schema version (`JOB_SYSTEM_DESIGN.md` §4).
pub const CURRENT_JOB_SCHEMA_VERSION: u32 = 1;

/// Canonical items page size (`CONVERSION_UX.md` §4.4, `IMPLEMENTATION_PLAN.md` §C4.4).
pub const ITEMS_PAGE_SIZE: u32 = 200;

/// Canonical job list page size (`CONVERSION_UX.md` §4.4).
pub const JOBS_PAGE_SIZE: u32 = 200;

/// Paginated slice of a job's item details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobDetailsPage {
    pub summary: JobSummary,
    pub items: Vec<JobItemReport>,
    pub total_items: u32,
}

/// Filter chip on the Jobs page (`CONVERSION_UX.md` §4.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JobsFilter {
    #[default]
    All,
    Active,
    Completed,
    Failed,
}

impl JobsFilter {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Active => "Active",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
        }
    }

    #[must_use]
    pub const fn matches_state(self, state: JobState) -> bool {
        match self {
            Self::All => true,
            Self::Active => state.is_active(),
            Self::Completed => matches!(state, JobState::Completed | JobState::CompletedWithErrors),
            Self::Failed => matches!(state, JobState::Failed | JobState::Cancelled),
        }
    }
}

/// Commands sent from the UI thread to the background `radivine-control-jobs` worker bridge.
#[derive(Clone, Debug)]
pub enum JobsCommand {
    /// Polls `ping` + `list_jobs` (and refreshes `selected_job_id` if set).
    RefreshAll {
        /// Whether to auto-start `radivine-worker` if not currently running.
        allow_auto_start: bool,
    },
    /// Loads a specific 200-item page of a job's item details.
    LoadJobDetails { job_id: JobId, page_index: usize },
    /// Requests cancellation of an active job (`AC-C4.3`).
    CancelJob { job_id: JobId },
    /// Submits a linked retry job (`AC-C4.3`).
    RetryJob {
        job_id: JobId,
        mode: RetryMode,
        destination_override: Option<PathBuf>,
    },
    /// Clears terminal job records from `jobs.sqlite3` without deleting converted files (`AC-C4.3`).
    ClearHistory,
    /// Sends `reload_settings` to the running worker (`AC-C4.4`).
    ReloadWorkerSettings,
    /// Inspects pending staging journals and preserved orphaned entries.
    InspectStaging,
    /// Cleans up verified staging entries for terminal jobs.
    CleanupStaging,
    /// Reveals a converted output file or folder in the file manager via D-Bus `ShowItems` (`AC-C4.3`).
    OpenInFileManager { target_path: PathBuf },
    /// Terminates the background bridge thread cleanly.
    Shutdown,
}

/// Events delivered from the background `radivine-control-jobs` thread to `JobsViewState`.
#[derive(Clone, Debug)]
pub enum UiJobsEvent {
    /// Snapshot of worker reachability, schema version, and recent jobs.
    SnapshotLoaded {
        worker_online: bool,
        worker_pid: Option<u32>,
        schema_version: u32,
        jobs: Vec<JobSummary>,
        error: Option<String>,
    },
    /// Live streaming event from `JobsClient::subscribe`.
    LiveJobUpdate {
        update: JobUpdate,
        emitted_at: Instant,
    },
    /// Paginated details for the currently selected job.
    JobDetailsLoaded {
        details: JobDetailsPage,
        page_index: usize,
    },
    /// Linked retry job created (`retry_of = original_job_id`).
    RetryCreated {
        original_job_id: JobId,
        new_job_id: JobId,
    },
    /// Terminal job history cleared.
    HistoryCleared { deleted_jobs: u32 },
    /// Staging inspection or cleanup report.
    StagingReportLoaded {
        pending_journals: u32,
        cleaned_files: u32,
        cleaned_dirs: u32,
        preserved_orphans: Vec<OrphanedStageEntry>,
    },
    /// Informational or error feedback banner from an asynchronous job command.
    CommandFeedback {
        banner_ok: Option<String>,
        banner_err: Option<String>,
    },
}

/// Pure view-model for the Jobs page (`ControlPage::Jobs`).
#[derive(Clone, Debug)]
pub struct JobsViewState {
    pub worker_online: bool,
    pub worker_pid: Option<u32>,
    pub schema_version: u32,
    /// `true` when `schema_version > CURRENT_JOB_SCHEMA_VERSION` (`AC-C4.6` read-only state).
    pub newer_schema_readonly: bool,
    pub worker_error: Option<String>,
    /// All known jobs ordered newest-first (`job_id` descending).
    pub jobs: Vec<JobSummary>,
    pub filter: JobsFilter,
    pub selected_job_id: Option<JobId>,
    pub selected_job_details: Option<JobDetailsPage>,
    pub items_page_index: usize,
    pub confirm_clear_history: bool,
    pub confirm_retry_all_job: Option<JobId>,
    /// `(job_id, mode, folder_path_text)` for `Retry to another folder…` (`AC-C4.3`).
    pub retry_folder_modal: Option<(JobId, RetryMode, String)>,
    pub staging_report: Option<(u32, u32, u32, Vec<OrphanedStageEntry>)>,
    pub banner_ok: Option<String>,
    pub banner_err: Option<String>,
    /// Pending `emitted_at` timestamps for live events awaiting the next UI frame repaint (`AC-C4.2`).
    pub pending_event_timestamps: Vec<Instant>,
    /// Recorded `event -> UI repaint` latencies in milliseconds (`AC-C4.2` target: `p95 < 250 ms`).
    pub repaint_latencies_ms: Vec<f64>,
    /// Duration of the most recent `render_jobs_page` call in milliseconds (`AC-C4.2` target: `< 50 ms`).
    pub last_frame_render_ms: f64,
}

impl Default for JobsViewState {
    fn default() -> Self {
        Self {
            worker_online: true,
            worker_pid: None,
            schema_version: CURRENT_JOB_SCHEMA_VERSION,
            newer_schema_readonly: false,
            worker_error: None,
            jobs: Vec::new(),
            filter: JobsFilter::All,
            selected_job_id: None,
            selected_job_details: None,
            items_page_index: 0,
            confirm_clear_history: false,
            confirm_retry_all_job: None,
            retry_folder_modal: None,
            staging_report: None,
            banner_ok: None,
            banner_err: None,
            pending_event_timestamps: Vec::new(),
            repaint_latencies_ms: Vec::new(),
            last_frame_render_ms: 0.0,
        }
    }
}

impl JobsViewState {
    /// Applies a [`UiJobsEvent`] to this view state deterministically (pure reducer).
    pub fn apply_event(&mut self, ev: UiJobsEvent) {
        match ev {
            UiJobsEvent::SnapshotLoaded {
                worker_online,
                worker_pid,
                schema_version,
                mut jobs,
                error,
            } => {
                self.worker_online = worker_online;
                self.worker_pid = worker_pid;
                self.schema_version = schema_version;
                self.newer_schema_readonly = schema_version > CURRENT_JOB_SCHEMA_VERSION;
                self.worker_error = error;
                jobs.sort_by_key(|b| std::cmp::Reverse(b.job_id.0));
                self.jobs = jobs;
                if let Some(sel_id) = self.selected_job_id
                    && !self.jobs.iter().any(|j| j.job_id == sel_id)
                {
                    self.selected_job_id = None;
                    self.selected_job_details = None;
                }
            }
            UiJobsEvent::LiveJobUpdate { update, emitted_at } => {
                self.worker_online = true;
                self.pending_event_timestamps.push(emitted_at);
                self.apply_live_job_update(update);
            }
            UiJobsEvent::JobDetailsLoaded {
                details,
                page_index,
            } => {
                self.selected_job_id = Some(details.summary.job_id);
                self.items_page_index = page_index;
                if let Some(existing) = self
                    .jobs
                    .iter_mut()
                    .find(|j| j.job_id == details.summary.job_id)
                {
                    *existing = details.summary.clone();
                }
                self.selected_job_details = Some(details);
            }
            UiJobsEvent::RetryCreated {
                original_job_id,
                new_job_id,
            } => {
                self.selected_job_id = Some(new_job_id);
                self.items_page_index = 0;
                self.banner_ok = Some(format!(
                    "Started linked retry job #{new_job_id} (retry of #{original_job_id})."
                ));
                self.banner_err = None;
            }
            UiJobsEvent::HistoryCleared { deleted_jobs } => {
                self.jobs.retain(|j| j.state.is_active());
                if let Some(sel) = self.selected_job_id
                    && !self.jobs.iter().any(|j| j.job_id == sel)
                {
                    self.selected_job_id = None;
                    self.selected_job_details = None;
                }
                self.confirm_clear_history = false;
                self.banner_ok = Some(format!(
                    "Cleared {deleted_jobs} terminal job record(s) from history. Converted files on disk were preserved."
                ));
                self.banner_err = None;
            }
            UiJobsEvent::StagingReportLoaded {
                pending_journals,
                cleaned_files,
                cleaned_dirs,
                preserved_orphans,
            } => {
                if cleaned_files > 0 || cleaned_dirs > 0 {
                    self.banner_ok = Some(format!(
                        "Cleaned {cleaned_files} staging file(s) and {cleaned_dirs} staging folder(s)."
                    ));
                }
                self.staging_report = Some((
                    pending_journals,
                    cleaned_files,
                    cleaned_dirs,
                    preserved_orphans,
                ));
            }
            UiJobsEvent::CommandFeedback {
                banner_ok,
                banner_err,
            } => {
                if let Some(ok) = banner_ok {
                    self.banner_ok = Some(ok);
                    self.banner_err = None;
                }
                if let Some(err) = banner_err {
                    self.banner_err = Some(err);
                }
            }
        }
    }

    fn apply_live_job_update(&mut self, update: JobUpdate) {
        match update {
            JobUpdate::JobCreated { job, .. } => {
                let summary = *job;
                if let Some(existing) = self.jobs.iter_mut().find(|j| j.job_id == summary.job_id) {
                    *existing = summary;
                } else {
                    self.jobs.insert(0, summary);
                    self.jobs.sort_by_key(|b| std::cmp::Reverse(b.job_id.0));
                }
            }
            JobUpdate::JobStateChanged {
                job_id,
                state,
                summary,
                ..
            } => {
                if let Some(job) = self.jobs.iter_mut().find(|j| j.job_id == job_id) {
                    job.state = state;
                    job.counts = summary;
                }
                if let Some(ref mut details) = self.selected_job_details
                    && details.summary.job_id == job_id
                {
                    details.summary.state = state;
                    details.summary.counts = summary;
                }
            }
            JobUpdate::ItemProgress {
                job_id,
                item_index,
                state,
                stage,
                summary,
                ..
            } => {
                if let Some(job) = self.jobs.iter_mut().find(|j| j.job_id == job_id) {
                    job.counts = summary;
                }
                if let Some(ref mut details) = self.selected_job_details
                    && details.summary.job_id == job_id
                {
                    details.summary.counts = summary;
                    if let Some(item) = details
                        .items
                        .iter_mut()
                        .find(|i| i.item_index == item_index)
                    {
                        item.state = state;
                        item.stage = stage;
                    }
                }
            }
            JobUpdate::ItemCompleted {
                job_id,
                item,
                summary,
                ..
            } => {
                if let Some(job) = self.jobs.iter_mut().find(|j| j.job_id == job_id) {
                    job.counts = summary;
                }
                if let Some(ref mut details) = self.selected_job_details
                    && details.summary.job_id == job_id
                {
                    details.summary.counts = summary;
                    if let Some(existing_item) = details
                        .items
                        .iter_mut()
                        .find(|i| i.item_index == item.item_index)
                    {
                        *existing_item = *item;
                    }
                }
            }
            JobUpdate::JobFinished { job, .. } => {
                let summary = *job;
                if let Some(existing) = self.jobs.iter_mut().find(|j| j.job_id == summary.job_id) {
                    *existing = summary.clone();
                }
                if let Some(ref mut details) = self.selected_job_details
                    && details.summary.job_id == summary.job_id
                {
                    details.summary = summary;
                }
            }
        }
    }

    /// Returns the number of currently active jobs (`Queued | Running | Cancelling`).
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.jobs.iter().filter(|j| j.state.is_active()).count()
    }

    /// Returns the number of terminal failed/interrupted jobs.
    #[must_use]
    pub fn failed_count(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| {
                j.state == JobState::Failed
                    || (j.state == JobState::CompletedWithErrors
                        && (j.counts.failed_items > 0 || j.counts.interrupted_items > 0))
            })
            .count()
    }

    /// Flushes any pending live-event timestamps and records their `event -> repaint` latency (`AC-C4.2`).
    pub fn record_repaint_latencies(&mut self) {
        let now = Instant::now();
        for ts in self.pending_event_timestamps.drain(..) {
            let ms = now.saturating_duration_since(ts).as_secs_f64() * 1000.0;
            self.repaint_latencies_ms.push(ms);
            if self.repaint_latencies_ms.len() > 2048 {
                self.repaint_latencies_ms.remove(0);
            }
        }
    }

    /// Computes the p95 `event -> repaint` latency in milliseconds (`AC-C4.2`).
    #[must_use]
    pub fn p95_repaint_latency_ms(&self) -> Option<f64> {
        if self.repaint_latencies_ms.is_empty() {
            return None;
        }
        let mut sorted = self.repaint_latencies_ms.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((sorted.len() as f64 * 0.95).ceil() as usize)
            .saturating_sub(1)
            .min(sorted.len() - 1);
        Some(sorted[idx])
    }
}

/// Converts a filesystem path into a percent-encoded `file://` URI suitable for `org.freedesktop.FileManager1.ShowItems`.
#[must_use]
pub fn path_to_file_uri(path: &Path) -> String {
    let bytes = path.as_os_str().as_bytes();
    let mut out = String::with_capacity("file://".len() + bytes.len() * 3);
    out.push_str("file://");
    for &b in bytes {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Reveals `target_path` in the user's file manager using D-Bus `org.freedesktop.FileManager1.ShowItems`,
/// falling back to `xdg-open` on the parent directory (`CONVERSION_UX.md` §4.3, `E6`, `AC-C4.3`).
pub fn reveal_path_in_file_manager(target_path: &Path) -> Result<String, String> {
    let dbus_override = std::env::var("RADIVINE_DBUS_SOCKET_OVERRIDE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from);
    let uri = path_to_file_uri(target_path);
    match call_show_items(dbus_override.as_deref(), &[uri], "radivine-control") {
        Ok(()) => Ok(format!(
            "Revealed {} in file manager via D-Bus ShowItems.",
            target_path.display()
        )),
        Err(dbus_err) => {
            if std::env::var("RADIVINE_DISABLE_XDG_OPEN_FALLBACK").is_ok_and(|v| v == "1") {
                return Err(format!("D-Bus ShowItems failed: {dbus_err}"));
            }
            let folder = if target_path.is_dir() {
                target_path
            } else {
                target_path.parent().unwrap_or(target_path)
            };
            match Command::new("xdg-open")
                .arg(folder.as_os_str())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(_) => Ok(format!("Opened folder {}.", folder.display())),
                Err(e) => Err(format!(
                    "Could not open folder {} (D-Bus: {dbus_err}; xdg-open: {e})",
                    folder.display()
                )),
            }
        }
    }
}

/// Builds a [`WorkerRuntimePaths`] rooted at `worker_socket_path` and `state_dir`.
#[must_use]
pub fn worker_paths_for(worker_socket_path: &Path, state_dir: &Path) -> WorkerRuntimePaths {
    let default_env = WorkerRuntimePaths::from_env();
    let mut paths = default_env.clone();
    if std::env::var("RADIVINE_WORKER_SOCKET").is_err() {
        paths.socket_path = worker_socket_path.to_path_buf();
    }
    if std::env::var("RADIVINE_WORKER_LOCK").is_err() {
        paths.lock_path = paths
            .socket_path
            .parent()
            .unwrap_or_else(|| Path::new("/tmp"))
            .join("worker.lock");
    }
    if std::env::var("RADIVINE_WORKER_DB").is_err() {
        paths.db_path = state_dir.join("jobs.sqlite3");
    }
    if std::env::var("RADIVINE_WORKER_LOG").is_err() {
        paths.log_path = state_dir.join("worker.log");
    }
    if paths.socket_path != default_env.socket_path || paths.db_path != default_env.db_path {
        paths.launch_mode = radivine_jobs::WorkerLaunchMode::Spawn;
    }
    paths
}

/// Spawns the non-blocking background worker bridge (`radivine-control-jobs`).
pub fn spawn_jobs_bridge(
    worker_socket_path: PathBuf,
    state_dir: PathBuf,
    egui_ctx: Option<egui::Context>,
) -> (Sender<JobsCommand>, Receiver<UiJobsEvent>) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let (cmd_tx, cmd_rx) = mpsc::channel::<JobsCommand>();
    let (ev_tx, ev_rx) = mpsc::channel::<UiJobsEvent>();
    let shutdown = Arc::new(AtomicBool::new(false));

    // Lightweight live-event subscriber thread that connects whenever the worker socket exists.
    let sub_sock = worker_socket_path.clone();
    let sub_ev_tx = ev_tx.clone();
    let sub_ctx = egui_ctx.clone();
    let sub_shutdown = Arc::clone(&shutdown);
    let _ = thread::Builder::new()
        .name("radivine-control-jobs-sub".to_string())
        .spawn(move || {
            while !sub_shutdown.load(Ordering::Relaxed) {
                if !sub_sock.exists() {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
                if let Ok(client) = JobsClient::connect(&sub_sock)
                    && let Ok((_active, mut sub)) = client.subscribe(None)
                {
                    let _ = sub.set_read_timeout(Some(Duration::from_millis(200)));
                    while !sub_shutdown.load(Ordering::Relaxed) {
                        match sub.next_event() {
                            Ok(Some(update)) => {
                                if sub_ev_tx
                                    .send(UiJobsEvent::LiveJobUpdate {
                                        update,
                                        emitted_at: Instant::now(),
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                                if let Some(ref ctx) = sub_ctx {
                                    ctx.request_repaint();
                                }
                            }
                            Ok(None) => break,
                            Err(radivine_jobs::ClientError::Frame(
                                radivine_jobs::FrameReadError::Io(ref io_err),
                            ))
                            | Err(radivine_jobs::ClientError::Io(ref io_err))
                                if matches!(
                                    io_err.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) =>
                            {
                                continue;
                            }
                            Err(_) => break,
                        }
                    }
                }
                thread::sleep(Duration::from_millis(100));
            }
        });

    let _ = thread::Builder::new()
        .name("radivine-control-jobs".to_string())
        .spawn(move || {
            while let Ok(cmd) = cmd_rx.recv() {
                let w_paths = worker_paths_for(&worker_socket_path, &state_dir);
                let connect_client = |allow_auto_start: bool| {
                    if allow_auto_start {
                        JobsClient::connect_or_launch(&w_paths)
                    } else {
                        JobsClient::connect(&w_paths.socket_path)
                    }
                };

                match cmd {
                    JobsCommand::Shutdown => break,
                    JobsCommand::RefreshAll { allow_auto_start } => {
                        match connect_client(allow_auto_start) {
                            Ok(mut client) => match client.ping() {
                                Ok(WorkerResponse::Pong {
                                    pid,
                                    schema_version,
                                    ..
                                }) => match client.list_jobs(JOBS_PAGE_SIZE, true) {
                                    Ok(jobs) => {
                                        let _ = ev_tx.send(UiJobsEvent::SnapshotLoaded {
                                            worker_online: true,
                                            worker_pid: Some(pid),
                                            schema_version,
                                            jobs,
                                            error: None,
                                        });
                                    }
                                    Err(e) => {
                                        let _ = ev_tx.send(UiJobsEvent::SnapshotLoaded {
                                            worker_online: false,
                                            worker_pid: None,
                                            schema_version: CURRENT_JOB_SCHEMA_VERSION,
                                            jobs: Vec::new(),
                                            error: Some(e.to_string()),
                                        });
                                    }
                                },
                                Ok(other) => {
                                    let _ = ev_tx.send(UiJobsEvent::SnapshotLoaded {
                                        worker_online: false,
                                        worker_pid: None,
                                        schema_version: CURRENT_JOB_SCHEMA_VERSION,
                                        jobs: Vec::new(),
                                        error: Some(format!("Unexpected ping response: {other:?}")),
                                    });
                                }
                                Err(e) => {
                                    let _ = ev_tx.send(UiJobsEvent::SnapshotLoaded {
                                        worker_online: false,
                                        worker_pid: None,
                                        schema_version: CURRENT_JOB_SCHEMA_VERSION,
                                        jobs: Vec::new(),
                                        error: Some(e.to_string()),
                                    });
                                }
                            },
                            Err(e) => {
                                let _ = ev_tx.send(UiJobsEvent::SnapshotLoaded {
                                    worker_online: false,
                                    worker_pid: None,
                                    schema_version: CURRENT_JOB_SCHEMA_VERSION,
                                    jobs: Vec::new(),
                                    error: Some(e.to_string()),
                                });
                            }
                        }
                    }
                    JobsCommand::LoadJobDetails { job_id, page_index } => {
                        let offset = (page_index as u32).saturating_mul(ITEMS_PAGE_SIZE);
                        match connect_client(true).and_then(|mut c| {
                            c.get_job(job_id, Some(offset), Some(ITEMS_PAGE_SIZE))
                        }) {
                            Ok((summary, items, total_items)) => {
                                let _ = ev_tx.send(UiJobsEvent::JobDetailsLoaded {
                                    details: JobDetailsPage {
                                        summary,
                                        items,
                                        total_items,
                                    },
                                    page_index,
                                });
                            }
                            Err(e) => {
                                let _ = ev_tx.send(UiJobsEvent::CommandFeedback {
                                    banner_ok: None,
                                    banner_err: Some(format!(
                                        "Failed to load details for job #{job_id}: {e}"
                                    )),
                                });
                            }
                        }
                    }
                    JobsCommand::CancelJob { job_id } => {
                        match connect_client(false).and_then(|mut c| c.cancel(job_id)) {
                            Ok(_) => {
                                let _ = ev_tx.send(UiJobsEvent::CommandFeedback {
                                    banner_ok: Some(format!(
                                        "Cancellation requested for job #{job_id}."
                                    )),
                                    banner_err: None,
                                });
                            }
                            Err(e) => {
                                let _ = ev_tx.send(UiJobsEvent::CommandFeedback {
                                    banner_ok: None,
                                    banner_err: Some(format!(
                                        "Failed to cancel job #{job_id}: {e}"
                                    )),
                                });
                            }
                        }
                    }
                    JobsCommand::RetryJob {
                        job_id,
                        mode,
                        destination_override,
                    } => {
                        let options_override = destination_override.and_then(|folder| {
                            let ov = PresetOverrides {
                                output_location: Some(OutputLocation::Directory(folder)),
                                ..PresetOverrides::default()
                            };
                            serde_json::to_value(ov).ok()
                        });

                        match connect_client(true) {
                            Ok(mut client) => match client.retry(job_id, mode, options_override) {
                                Ok((new_job_id, _, _)) => {
                                    let _ = ev_tx.send(UiJobsEvent::RetryCreated {
                                        original_job_id: job_id,
                                        new_job_id,
                                    });
                                    if let Ok(WorkerResponse::Pong {
                                        pid,
                                        schema_version,
                                        ..
                                    }) = client.ping()
                                        && let Ok(jobs) = client.list_jobs(JOBS_PAGE_SIZE, true)
                                    {
                                        let _ = ev_tx.send(UiJobsEvent::SnapshotLoaded {
                                            worker_online: true,
                                            worker_pid: Some(pid),
                                            schema_version,
                                            jobs,
                                            error: None,
                                        });
                                    }
                                    if let Ok((summary, items, total_items)) =
                                        client.get_job(new_job_id, Some(0), Some(ITEMS_PAGE_SIZE))
                                    {
                                        let _ = ev_tx.send(UiJobsEvent::JobDetailsLoaded {
                                            details: JobDetailsPage {
                                                summary,
                                                items,
                                                total_items,
                                            },
                                            page_index: 0,
                                        });
                                    }
                                }
                                Err(e) => {
                                    let _ = ev_tx.send(UiJobsEvent::CommandFeedback {
                                        banner_ok: None,
                                        banner_err: Some(format!(
                                            "Retry failed for job #{job_id}: {e}"
                                        )),
                                    });
                                }
                            },
                            Err(e) => {
                                let _ = ev_tx.send(UiJobsEvent::CommandFeedback {
                                    banner_ok: None,
                                    banner_err: Some(format!(
                                        "Could not connect to worker for retry: {e}"
                                    )),
                                });
                            }
                        }
                    }
                    JobsCommand::ClearHistory => {
                        match connect_client(true).and_then(|mut c| c.clear_history(None, None)) {
                            Ok(deleted_jobs) => {
                                let _ = ev_tx.send(UiJobsEvent::HistoryCleared { deleted_jobs });
                            }
                            Err(e) => {
                                let _ = ev_tx.send(UiJobsEvent::CommandFeedback {
                                    banner_ok: None,
                                    banner_err: Some(format!("Failed to clear history: {e}")),
                                });
                            }
                        }
                    }
                    JobsCommand::ReloadWorkerSettings => {
                        if w_paths.socket_path.exists()
                            && let Ok(mut client) = JobsClient::connect(&w_paths.socket_path)
                        {
                            let _ = client.reload_settings();
                        }
                    }
                    JobsCommand::InspectStaging => {
                        match connect_client(true).and_then(|mut c| c.inspect_staging(None)) {
                            Ok((pending_journals, preserved_orphans)) => {
                                let _ = ev_tx.send(UiJobsEvent::StagingReportLoaded {
                                    pending_journals,
                                    cleaned_files: 0,
                                    cleaned_dirs: 0,
                                    preserved_orphans,
                                });
                            }
                            Err(e) => {
                                let _ = ev_tx.send(UiJobsEvent::CommandFeedback {
                                    banner_ok: None,
                                    banner_err: Some(format!("Staging inspection failed: {e}")),
                                });
                            }
                        }
                    }
                    JobsCommand::CleanupStaging => {
                        match connect_client(true).and_then(|mut c| c.cleanup_staging(None)) {
                            Ok((cleaned_files, cleaned_dirs, preserved_orphans)) => {
                                let _ = ev_tx.send(UiJobsEvent::StagingReportLoaded {
                                    pending_journals: 0,
                                    cleaned_files,
                                    cleaned_dirs,
                                    preserved_orphans,
                                });
                            }
                            Err(e) => {
                                let _ = ev_tx.send(UiJobsEvent::CommandFeedback {
                                    banner_ok: None,
                                    banner_err: Some(format!("Staging cleanup failed: {e}")),
                                });
                            }
                        }
                    }
                    JobsCommand::OpenInFileManager { target_path } => {
                        match reveal_path_in_file_manager(&target_path) {
                            Ok(msg) => {
                                let _ = ev_tx.send(UiJobsEvent::CommandFeedback {
                                    banner_ok: Some(msg),
                                    banner_err: None,
                                });
                            }
                            Err(err) => {
                                let _ = ev_tx.send(UiJobsEvent::CommandFeedback {
                                    banner_ok: None,
                                    banner_err: Some(err),
                                });
                            }
                        }
                    }
                }

                if let Some(ref ctx) = egui_ctx {
                    ctx.request_repaint();
                }
            }
            shutdown.store(true, Ordering::Relaxed);
        });

    (cmd_tx, ev_rx)
}

/// Renders the complete Jobs page (`ControlPage::Jobs`, `CONVERSION_UX.md` §4).
///
/// Returns `true` if `max_parallel` in `draft_jobs` was changed so the caller can persist
/// `config.toml` and notify the worker.
pub fn render_jobs_page(
    ui: &mut egui::Ui,
    pal: &BrandPalette,
    state: &mut JobsViewState,
    max_parallel: &mut MaxParallelSetting,
    cmd_tx: &Sender<JobsCommand>,
    navigate_to_diagnostics: &mut bool,
    navigate_to_conversion: &mut bool,
) -> bool {
    let start_frame = Instant::now();
    state.record_repaint_latencies();
    let mut concurrency_changed = false;

    // Feedback banners specific to Jobs
    if let Some(ref err) = state.banner_err.clone() {
        egui::Frame::NONE
            .fill(pal.err_bg)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(12, 8))
            .stroke(Stroke::new(1.0, pal.err_fg))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(err).color(pal.err_fg).strong());
                    if ui.small_button("Dismiss").clicked() {
                        state.banner_err = None;
                    }
                });
            });
        ui.add_space(8.0);
    }
    if let Some(ref ok) = state.banner_ok.clone() {
        egui::Frame::NONE
            .fill(pal.ok_bg)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(12, 8))
            .stroke(Stroke::new(1.0, pal.ok_fg))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(ok).color(pal.ok_fg).strong());
                    if ui.small_button("Dismiss").clicked() {
                        state.banner_ok = None;
                    }
                });
            });
        ui.add_space(8.0);
    }

    // Special State 1: Newer DB schema read-only banner (`CONVERSION_UX.md` §4.6, `AC-C4.6`)
    if state.newer_schema_readonly {
        egui::Frame::NONE
            .fill(pal.warn_bg)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(12, 8))
            .stroke(Stroke::new(1.0, pal.warn_fg))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(format!(
                        "Job history was created by a newer version of Radivine (schema v{} > supported v{}). History is shown read-only.",
                        state.schema_version, CURRENT_JOB_SCHEMA_VERSION
                    ))
                    .color(pal.warn_fg)
                    .strong(),
                );
            });
        ui.add_space(10.0);
    }

    // Special State 2: Worker unavailable banner (`CONVERSION_UX.md` §4.6, `AC-C4.6`)
    if !state.worker_online {
        egui::Frame::NONE
            .fill(pal.err_bg)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(14, 10))
            .stroke(Stroke::new(1.0, pal.err_fg))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new("Conversion worker is not responding.")
                            .color(pal.err_fg)
                            .strong(),
                    );
                    if let Some(ref detail) = state.worker_error {
                        ui.label(
                            RichText::new(format!("({detail})"))
                                .color(pal.fg_secondary)
                                .size(11.5),
                        );
                    }
                    if ui.button("Retry connection").clicked() {
                        let _ = cmd_tx.send(JobsCommand::RefreshAll {
                            allow_auto_start: true,
                        });
                    }
                    if ui.button("Diagnostics").clicked() {
                        *navigate_to_diagnostics = true;
                    }
                });
            });
        ui.add_space(10.0);
    }

    // Top Toolbar: Filters, Clear History, Concurrency, Staging Maintenance
    egui::Frame::NONE
        .fill(pal.bg_elevated)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(14, 10))
        .stroke(Stroke::new(1.0, pal.border))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Filter:").color(pal.fg_secondary).size(12.5));
                for f in [
                    JobsFilter::All,
                    JobsFilter::Active,
                    JobsFilter::Completed,
                    JobsFilter::Failed,
                ] {
                    if ui.selectable_label(state.filter == f, f.label()).clicked() {
                        state.filter = f;
                    }
                }

                ui.separator();
                ui.label(
                    RichText::new("Max parallel:")
                        .color(pal.fg_secondary)
                        .size(12.5),
                );
                for (setting, label) in [
                    (MaxParallelSetting::Auto, "Auto"),
                    (MaxParallelSetting::Fixed(1), "1"),
                    (MaxParallelSetting::Fixed(2), "2"),
                    (MaxParallelSetting::Fixed(4), "4"),
                    (MaxParallelSetting::Fixed(8), "8"),
                ] {
                    if ui
                        .add_enabled(
                            !state.newer_schema_readonly,
                            egui::Button::selectable(*max_parallel == setting, label),
                        )
                        .clicked()
                    {
                        *max_parallel = setting;
                        concurrency_changed = true;
                    }
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let has_terminal = state.jobs.iter().any(|j| j.state.is_terminal());
                    if ui
                        .add_enabled(
                            has_terminal && !state.newer_schema_readonly,
                            egui::Button::new("Clear history"),
                        )
                        .clicked()
                    {
                        state.confirm_clear_history = true;
                    }
                    if ui.button("Staging check").clicked() {
                        let _ = cmd_tx.send(JobsCommand::InspectStaging);
                    }
                    if ui.button("Refresh").clicked() {
                        let _ = cmd_tx.send(JobsCommand::RefreshAll {
                            allow_auto_start: true,
                        });
                    }
                });
            });
        });

    // Confirmation bar for Clear History (`CONVERSION_UX.md` §4.3, `AC-C4.3`)
    if state.confirm_clear_history {
        ui.add_space(8.0);
        egui::Frame::NONE
            .fill(pal.warn_bg)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(12, 8))
            .stroke(Stroke::new(1.0, pal.warn_fg))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(
                            "Clear completed, failed, and cancelled job records from history? Converted files on disk will NOT be deleted.",
                        )
                        .color(pal.warn_fg)
                        .strong(),
                    );
                    if ui.button("Confirm Clear History").clicked() {
                        let _ = cmd_tx.send(JobsCommand::ClearHistory);
                    }
                    if ui.button("Cancel").clicked() {
                        state.confirm_clear_history = false;
                    }
                });
            });
    }

    // Confirmation bar for Retry All (`CONVERSION_UX.md` §4.5)
    if let Some(job_id) = state.confirm_retry_all_job {
        ui.add_space(8.0);
        egui::Frame::NONE
            .fill(pal.warn_bg)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(12, 8))
            .stroke(Stroke::new(1.0, pal.warn_fg))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "Retry all items in job #{job_id}? Already-converted files will be created again with numeric suffixes (-1, -2, …)."
                        ))
                        .color(pal.warn_fg)
                        .strong(),
                    );
                    if ui.button("Confirm Retry All").clicked() {
                        let _ = cmd_tx.send(JobsCommand::RetryJob {
                            job_id,
                            mode: RetryMode::All,
                            destination_override: None,
                        });
                        state.confirm_retry_all_job = None;
                    }
                    if ui.button("Cancel").clicked() {
                        state.confirm_retry_all_job = None;
                    }
                });
            });
    }

    // Modal bar for Retry to another folder (`CONVERSION_UX.md` §4.5, `AC-C4.3`)
    if let Some((job_id, mode, ref folder_str)) = state.retry_folder_modal.clone() {
        let mut updated_folder = folder_str.clone();
        let mut close_modal = false;
        ui.add_space(8.0);
        egui::Frame::NONE
            .fill(pal.bg_elevated)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(12, 8))
            .stroke(Stroke::new(1.0, pal.vine_green))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("Retry job #{job_id} to destination folder:"))
                            .strong()
                            .color(pal.fg_primary),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut updated_folder)
                            .desired_width(300.0)
                            .hint_text("/path/to/writable/folder"),
                    );
                    if ui.button("Start Retry").clicked() && !updated_folder.trim().is_empty() {
                        let _ = cmd_tx.send(JobsCommand::RetryJob {
                            job_id,
                            mode,
                            destination_override: Some(PathBuf::from(updated_folder.trim())),
                        });
                        close_modal = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close_modal = true;
                    }
                });
            });
        if close_modal {
            state.retry_folder_modal = None;
        } else {
            state.retry_folder_modal = Some((job_id, mode, updated_folder));
        }
    }

    // Staging inspection / cleanup report panel
    if let Some((pending_journals, cleaned_files, cleaned_dirs, ref orphans)) =
        state.staging_report.clone()
    {
        ui.add_space(8.0);
        egui::Frame::NONE
            .fill(pal.bg_elevated)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(12, 8))
            .stroke(Stroke::new(1.0, pal.border))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "Staging Check: {pending_journals} pending journal(s), {} preserved orphan entry(ies).",
                            orphans.len()
                        ))
                        .color(pal.fg_primary)
                        .strong(),
                    );
                    if cleaned_files > 0 || cleaned_dirs > 0 {
                        ui.label(
                            RichText::new(format!(
                                "Cleaned {cleaned_files} file(s), {cleaned_dirs} dir(s)."
                            ))
                            .color(pal.ok_fg),
                        );
                    }
                    if (pending_journals > 0 || !orphans.is_empty())
                        && !state.newer_schema_readonly
                        && ui.button("Clean up verified staging").clicked()
                    {
                        let _ = cmd_tx.send(JobsCommand::CleanupStaging);
                    }
                    if ui.small_button("Close").clicked() {
                        state.staging_report = None;
                    }
                });
            });
    }

    ui.add_space(12.0);

    // Special State 3: Empty state (`CONVERSION_UX.md` §4.6, `AC-C4.6`)
    if state.jobs.is_empty() {
        section_card(ui, pal, "No Conversion Jobs Yet", |ui| {
            ui.label(
                RichText::new(
                    "Select image files in your file manager, hold Shift while dragging to open the Radivine wheel, and choose Convert -> Image.",
                )
                .color(pal.fg_secondary)
                .size(13.0),
            );
            ui.add_space(8.0);
            if ui.button("Conversion Settings").clicked() {
                *navigate_to_conversion = true;
            }
        });
        state.last_frame_render_ms = start_frame.elapsed().as_secs_f64() * 1000.0;
        return concurrency_changed;
    }

    // Active Jobs section
    let active_jobs: Vec<JobSummary> = state
        .jobs
        .iter()
        .filter(|j| j.state.is_active() && state.filter.matches_state(j.state))
        .cloned()
        .collect();

    if !active_jobs.is_empty() {
        section_card(
            ui,
            pal,
            &format!("Active Jobs ({})", active_jobs.len()),
            |ui| {
                ui.label(
                    RichText::new("Running in the independent radivine-worker background process. Closing this window will not interrupt them.")
                        .color(pal.fg_muted)
                        .size(12.0),
                );
                ui.add_space(6.0);
                for job in &active_jobs {
                    render_job_summary_card(ui, pal, state, job, cmd_tx);
                    ui.add_space(8.0);
                }
            },
        );
        ui.add_space(14.0);
    }

    // Recent / Historical Jobs section
    let history_jobs: Vec<JobSummary> = state
        .jobs
        .iter()
        .filter(|j| !j.state.is_active() && state.filter.matches_state(j.state))
        .cloned()
        .collect();

    if !history_jobs.is_empty() {
        section_card(
            ui,
            pal,
            &format!("Recent Jobs ({})", history_jobs.len()),
            |ui| {
                ui.label(
                    RichText::new(
                        "Durable job history stored in ~/.local/state/radivine/jobs.sqlite3.",
                    )
                    .color(pal.fg_muted)
                    .size(12.0),
                );
                ui.add_space(6.0);
                for job in &history_jobs {
                    render_job_summary_card(ui, pal, state, job, cmd_tx);
                    ui.add_space(8.0);
                }
            },
        );
    }

    // Selected Job Per-Item Details Drawer (Paginated 200 items/page + Virtualized rows)
    if let Some(ref details) = state.selected_job_details.clone() {
        ui.add_space(14.0);
        render_job_details_panel(ui, pal, state, details, cmd_tx);
    }

    state.last_frame_render_ms = start_frame.elapsed().as_secs_f64() * 1000.0;
    concurrency_changed
}

fn render_job_summary_card(
    ui: &mut egui::Ui,
    pal: &BrandPalette,
    state: &mut JobsViewState,
    job: &JobSummary,
    cmd_tx: &Sender<JobsCommand>,
) {
    let is_selected = state.selected_job_id == Some(job.job_id);
    let border_color = if is_selected {
        pal.vine_green
    } else {
        pal.border
    };

    egui::Frame::NONE
        .fill(pal.bg_elevated)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(12, 10))
        .stroke(Stroke::new(
            if is_selected { 1.5 } else { 1.0 },
            border_color,
        ))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let (badge_text, badge_color) = job_state_badge(job.state, pal);
                ui.label(RichText::new(badge_text).color(badge_color).strong());

                let headline = job.summary_text.clone().unwrap_or_else(|| {
                    format!("Convert {} item(s) ({})", job.counts.total_items, job.kind)
                });
                ui.label(
                    RichText::new(format!("Job #{} — {}", job.job_id, headline))
                        .strong()
                        .color(pal.fg_primary),
                );

                if let Some(orig_id) = job.retry_of {
                    ui.label(
                        RichText::new(format!("[Retry of #{orig_id}]"))
                            .size(11.5)
                            .color(pal.leaf_green)
                            .strong(),
                    );
                }

                ui.label(
                    RichText::new(format!("Origin: {}", job.origin.as_str()))
                        .size(11.5)
                        .color(pal.fg_muted),
                );
            });

            if job.state.is_active() {
                ui.add_space(4.0);
                let done = job.counts.completed_items();
                let total = job.counts.total_items.max(1);
                let frac = (done as f32 / total as f32).clamp(0.0, 1.0);
                ui.add(
                    egui::ProgressBar::new(frac)
                        .text(format!("{done} / {} completed", job.counts.total_items)),
                );
            }

            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} succeeded · {} skipped · {} failed · {} interrupted · {} cancelled",
                        job.counts.succeeded_items,
                        job.counts.skipped_items,
                        job.counts.failed_items,
                        job.counts.interrupted_items,
                        job.counts.cancelled_items
                    ))
                    .size(12.0)
                    .color(pal.fg_secondary),
                );

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if job.state.is_active() {
                        let can_cancel = !state.newer_schema_readonly;
                        if ui
                            .add_enabled(can_cancel, egui::Button::new("Cancel"))
                            .clicked()
                        {
                            let _ = cmd_tx.send(JobsCommand::CancelJob { job_id: job.job_id });
                        }
                    } else {
                        let failed_or_interrupted =
                            job.counts.failed_items + job.counts.interrupted_items;
                        if failed_or_interrupted > 0
                            && ui
                                .add_enabled(
                                    !state.newer_schema_readonly,
                                    egui::Button::new(format!(
                                        "Retry failed ({failed_or_interrupted})"
                                    )),
                                )
                                .clicked()
                        {
                            let _ = cmd_tx.send(JobsCommand::RetryJob {
                                job_id: job.job_id,
                                mode: RetryMode::FailedOnly,
                                destination_override: None,
                            });
                        }

                        if failed_or_interrupted > 0
                            && ui
                                .add_enabled(
                                    !state.newer_schema_readonly,
                                    egui::Button::new("Retry to folder…"),
                                )
                                .clicked()
                        {
                            state.retry_folder_modal =
                                Some((job.job_id, RetryMode::FailedOnly, "/tmp".to_string()));
                        }

                        if job.counts.cancelled_items > 0
                            && ui
                                .add_enabled(
                                    !state.newer_schema_readonly,
                                    egui::Button::new("Retry failed & cancelled"),
                                )
                                .clicked()
                        {
                            let _ = cmd_tx.send(JobsCommand::RetryJob {
                                job_id: job.job_id,
                                mode: RetryMode::FailedAndCancelled,
                                destination_override: None,
                            });
                        }

                        if ui
                            .add_enabled(
                                !state.newer_schema_readonly,
                                egui::Button::new("Retry all…"),
                            )
                            .clicked()
                        {
                            state.confirm_retry_all_job = Some(job.job_id);
                        }
                    }

                    if ui
                        .button(if is_selected {
                            "Hide details"
                        } else {
                            "Details"
                        })
                        .clicked()
                    {
                        if is_selected {
                            state.selected_job_id = None;
                            state.selected_job_details = None;
                        } else {
                            state.selected_job_id = Some(job.job_id);
                            state.items_page_index = 0;
                            let _ = cmd_tx.send(JobsCommand::LoadJobDetails {
                                job_id: job.job_id,
                                page_index: 0,
                            });
                        }
                    }
                });
            });
        });
}

fn render_job_details_panel(
    ui: &mut egui::Ui,
    pal: &BrandPalette,
    state: &mut JobsViewState,
    details: &JobDetailsPage,
    cmd_tx: &Sender<JobsCommand>,
) {
    let total_items = details.total_items as usize;
    let page_size = ITEMS_PAGE_SIZE as usize;
    let total_pages = total_items.div_ceil(page_size).max(1);
    let current_page = state.items_page_index.min(total_pages - 1);

    // Find first available output path or source parent folder for `[Open folder]`
    let open_target: Option<PathBuf> = details
        .items
        .iter()
        .find_map(|i| i.output_path.as_ref().and_then(|wp| wp.to_path_buf().ok()))
        .or_else(|| {
            details
                .items
                .first()
                .and_then(|i| i.source_path.to_path_buf().ok())
        });

    section_card(
        ui,
        pal,
        &format!(
            "Job #{} Item Details ({} total items — Page {} of {})",
            details.summary.job_id,
            total_items,
            current_page + 1,
            total_pages
        ),
        |ui| {
            ui.label(
                RichText::new("Showing up to 200 items per page with virtualized row rendering.")
                    .color(pal.fg_muted)
                    .size(12.0),
            );
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if let Some(ref target) = open_target
                    && ui.button("Open folder").clicked()
                {
                    let _ = cmd_tx.send(JobsCommand::OpenInFileManager {
                        target_path: target.clone(),
                    });
                }

                if total_pages > 1 {
                    ui.separator();
                    if ui
                        .add_enabled(current_page > 0, egui::Button::new("◀ Previous 200"))
                        .clicked()
                    {
                        let prev = current_page.saturating_sub(1);
                        let _ = cmd_tx.send(JobsCommand::LoadJobDetails {
                            job_id: details.summary.job_id,
                            page_index: prev,
                        });
                    }
                    ui.label(
                        RichText::new(format!("Page {} / {}", current_page + 1, total_pages))
                            .strong()
                            .color(pal.fg_primary),
                    );
                    if ui
                        .add_enabled(
                            current_page + 1 < total_pages,
                            egui::Button::new("Next 200 ▶"),
                        )
                        .clicked()
                    {
                        let next = current_page + 1;
                        let _ = cmd_tx.send(JobsCommand::LoadJobDetails {
                            job_id: details.summary.job_id,
                            page_index: next,
                        });
                    }
                }
            });

            ui.add_space(8.0);
            let row_height = 28.0;
            let num_rows = details.items.len();

            ScrollArea::vertical()
                .id_salt((
                    "job_items_virtual_scroll",
                    details.summary.job_id.0,
                    current_page,
                ))
                .max_height(320.0)
                .auto_shrink([false, false])
                .show_rows(ui, row_height, num_rows, |ui, row_range| {
                    for idx in row_range {
                        let Some(item) = details.items.get(idx) else {
                            continue;
                        };
                        ui.horizontal(|ui| {
                            let (icon, col) = item_state_badge(item.state, pal);
                            ui.label(RichText::new(icon).color(col).strong());
                            let src_display = item.source_path.lossy_display();
                            ui.label(
                                RichText::new(format!("#{} {}", item.item_index + 1, src_display))
                                    .color(pal.fg_primary)
                                    .size(12.0),
                            );

                            if let Some(ref out_p) = item.output_path {
                                ui.label(
                                    RichText::new(format!("-> {}", out_p.lossy_display()))
                                        .color(pal.ok_fg)
                                        .size(12.0),
                                );
                                if let Some(bytes) = item.output_bytes {
                                    ui.label(
                                        RichText::new(format!(
                                            "({:.1} KiB)",
                                            bytes as f64 / 1024.0
                                        ))
                                        .color(pal.fg_muted)
                                        .size(11.5),
                                    );
                                }
                                if let Ok(pb) = out_p.to_path_buf()
                                    && ui.small_button("Show in folder").clicked()
                                {
                                    let _ = cmd_tx
                                        .send(JobsCommand::OpenInFileManager { target_path: pb });
                                }
                            } else if let Some(ref err_msg) = item.error_message {
                                let code_str = item
                                    .error_code
                                    .as_deref()
                                    .map(|c| format!("[{c}] "))
                                    .unwrap_or_default();
                                ui.label(
                                    RichText::new(format!("— {code_str}{err_msg}"))
                                        .color(if item.state == ItemState::Skipped {
                                            pal.warn_fg
                                        } else {
                                            pal.err_fg
                                        })
                                        .size(12.0),
                                );
                                if ui.small_button("Copy error").clicked() {
                                    ui.ctx().copy_text(format!("{code_str}{err_msg}"));
                                }
                            }
                        });
                    }
                });
        },
    );
}

fn job_state_badge(state: JobState, pal: &BrandPalette) -> (&'static str, Color32) {
    match state {
        JobState::Queued => ("● Queued", pal.fg_secondary),
        JobState::Preparing => ("● Preparing…", pal.fg_secondary),
        JobState::Running => ("● Running", pal.leaf_green),
        JobState::Completed => ("✓ Completed", pal.ok_fg),
        JobState::CompletedWithErrors => ("⚠ Completed with errors", pal.warn_fg),
        JobState::Failed => ("✗ Failed", pal.err_fg),
        JobState::Cancelled => ("⊘ Cancelled", pal.fg_muted),
    }
}

fn item_state_badge(state: ItemState, pal: &BrandPalette) -> (&'static str, Color32) {
    match state {
        ItemState::Pending => ("Pending", pal.fg_muted),
        ItemState::Inspecting => ("Inspecting", pal.fg_secondary),
        ItemState::Running => ("Running", pal.leaf_green),
        ItemState::Committing => ("Committing", pal.leaf_green),
        ItemState::Succeeded => ("✓ OK", pal.ok_fg),
        ItemState::Skipped => ("↷ Skipped", pal.warn_fg),
        ItemState::Failed => ("✗ Failed", pal.err_fg),
        ItemState::Cancelled => ("⊘ Cancelled", pal.fg_muted),
        ItemState::Interrupted => ("⚡ Interrupted", pal.warn_fg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use radivine_jobs::{JobProgressCounts, Origin, RequestId};

    fn sample_summary(id: u64, state: JobState, retry_of: Option<JobId>) -> JobSummary {
        JobSummary {
            job_id: JobId(id),
            request_id: RequestId::generate(),
            origin: Origin::Wheel,
            retry_of,
            kind: "convert.image".to_string(),
            state,
            created_at_ms: 1000,
            started_at_ms: Some(1010),
            finished_at_ms: Some(1200),
            updated_at_ms: 1200,
            counts: JobProgressCounts {
                total_items: 4,
                pending_items: 0,
                in_flight_items: 0,
                succeeded_items: 3,
                skipped_items: 0,
                failed_items: 1,
                cancelled_items: 0,
                interrupted_items: 0,
            },
            summary_text: Some("Convert 4 images -> WEBP".to_string()),
            options: serde_json::json!({}),
            resolved: None,
        }
    }

    #[test]
    fn reducer_handles_snapshots_live_events_and_clear_history() {
        let mut view = JobsViewState::default();
        view.apply_event(UiJobsEvent::SnapshotLoaded {
            worker_online: true,
            worker_pid: Some(4242),
            schema_version: 1,
            jobs: vec![
                sample_summary(1, JobState::CompletedWithErrors, None),
                sample_summary(2, JobState::Running, None),
            ],
            error: None,
        });
        assert_eq!(view.jobs.len(), 2);
        assert_eq!(view.active_count(), 1);
        assert!(!view.newer_schema_readonly);

        // Live event finishes job #2
        view.apply_event(UiJobsEvent::LiveJobUpdate {
            update: JobUpdate::JobFinished {
                v: 1,
                job: Box::new(sample_summary(2, JobState::Completed, None)),
            },
            emitted_at: Instant::now(),
        });
        view.record_repaint_latencies();
        assert_eq!(view.active_count(), 0);
        assert!(view.p95_repaint_latency_ms().unwrap() < 50.0);

        // Clear history keeps only active jobs
        view.apply_event(UiJobsEvent::HistoryCleared { deleted_jobs: 2 });
        assert!(view.jobs.is_empty());
    }

    #[test]
    fn newer_schema_sets_readonly_mode() {
        let mut view = JobsViewState::default();
        view.apply_event(UiJobsEvent::SnapshotLoaded {
            worker_online: true,
            worker_pid: Some(99),
            schema_version: CURRENT_JOB_SCHEMA_VERSION + 1,
            jobs: vec![sample_summary(10, JobState::Completed, None)],
            error: None,
        });
        assert!(view.newer_schema_readonly);
    }
}
