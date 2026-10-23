//! Single-instance Unix domain socket IPC server and client (`ARCHITECTURE.md` §3.4).

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Maximum JSON line length accepted over the IPC socket (`64 KiB`).
const MAX_IPC_LINE_BYTES: usize = 65_536;

/// IPC request sent from the `radivine` CLI to the running daemon (`ARCHITECTURE.md` §3.4).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum IpcRequest {
    /// Summon the radial wheel in Invoke mode at the cursor (or `position`).
    Invoke {
        /// Normalised `file://` or scheme URIs passed on the CLI.
        #[serde(default)]
        uris: Vec<String>,
        /// Optional explicit `(x, y)` root physical pixel coordinates.
        #[serde(default)]
        position: Option<(i32, i32)>,
    },
    /// Query daemon health, session state, overlay window XID, and last committed action.
    Status,
    /// Reload `config.toml` without restarting the daemon.
    Reload,
    /// Dismiss any currently active radial wheel session.
    Dismiss,
    /// Cleanly shut down the daemon.
    Quit,
}

/// Snapshot of a single leaf action's live enablement state in the active session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStateSnapshot {
    /// Action identifier (`"radivine.preview.head"`, etc.).
    pub id: String,
    /// Item path in the wheel hierarchy (e.g. `[0, 1]`).
    pub path: Vec<u8>,
    /// Whether the item is currently enabled.
    pub enabled: bool,
    /// Disabled reason shown in the caption (`Some("Checking…")`, `Some("Not a text file")`, etc.).
    pub disabled_reason: Option<String>,
}

/// Live status snapshot returned by `radivine status`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DaemonStatus {
    /// Daemon process ID.
    pub pid: u32,
    /// Persistent X11 overlay window XID (`0x...`).
    pub overlay_window_id: u32,
    /// Current session state (`"Idle"`, `"Active"`, etc.).
    pub session_state: String,
    /// Active UI scale factor.
    pub scale: f32,
    /// Active monitor geometry `(x, y, w, h)` for the current or last session.
    pub monitor_rect: (i32, i32, u32, u32),
    /// Root wheel center `(x, y)` in overlay-local physical pixels for the current or last session.
    pub root_wheel_center_px: Option<(f32, f32)>,
    /// Root wheel outer radius in physical pixels for the current or last session.
    pub root_wheel_outer_radius_px: Option<f32>,
    /// Total number of Invoke sessions started since daemon boot.
    pub invoke_count: u64,
    /// Total number of Drag sessions started since daemon boot.
    #[serde(default)]
    pub drag_session_count: u64,
    /// Last committed `ActionId` string, if any.
    pub last_committed_action: Option<String>,
    /// Last committed `ItemPath` indices (e.g. `Some([0, 1])`), if any.
    #[serde(default)]
    pub last_committed_path: Option<Vec<u8>>,
    /// Last committed context item descriptions, if any.
    pub last_committed_items: Vec<String>,
    /// Source application of the last committed action (e.g. `Some("nemo")`).
    #[serde(default)]
    pub last_committed_source_app: Option<String>,
    /// Invocation mode of the last committed action (`"Drag"` or `"Invoke"`).
    #[serde(default)]
    pub last_committed_mode: Option<String>,
    /// ActionId of the most recently finished background job, if any.
    #[serde(default)]
    pub last_job_action: Option<String>,
    /// Error message of the most recently finished background job (`None` if it succeeded).
    #[serde(default)]
    pub last_job_error: Option<String>,
    /// Most recent desktop notification formatted as `"summary: body"` (or `"summary"`), if any.
    #[serde(default)]
    pub last_notification: Option<String>,
    /// Recent desktop notifications emitted by the daemon (capped at 32).
    #[serde(default)]
    pub notifications: Vec<String>,
    /// Configuration or `actions.toml` warnings from the most recent load/reload.
    #[serde(default)]
    pub config_warnings: Vec<String>,
    /// Live enablement state of leaf actions in the current or most recent session.
    #[serde(default)]
    pub active_item_states: Vec<ItemStateSnapshot>,
    /// Latency in microseconds from summon/trigger to overlay mapped + initial frame presented.
    #[serde(default)]
    pub last_summon_us: Option<u64>,
    /// Latency in microseconds from `XdndPosition` receipt to `XdndStatus` reply.
    #[serde(default)]
    pub last_dnd_status_us: Option<u64>,
    /// Latency in microseconds from `XdndDrop` receipt to `XdndFinished` + job dispatch.
    #[serde(default)]
    pub last_drop_dispatch_us: Option<u64>,
    /// Duration in microseconds of the most recent frame render + SHM presentation.
    #[serde(default)]
    pub last_frame_render_us: Option<u64>,
    /// Daemon binary version (`"0.1.1"`).
    #[serde(default)]
    pub version: String,
    /// Seconds elapsed since the daemon started.
    #[serde(default)]
    pub uptime_secs: u64,
    /// Desktop session type (`"x11"`, `"wayland"`, etc.).
    #[serde(default)]
    pub desktop_session: String,
    /// Active desktop environment (`"X-Cinnamon"`, etc.).
    #[serde(default)]
    pub desktop_environment: String,
    /// Active `DISPLAY` string (`":0"`, `":1"`, etc.).
    #[serde(default)]
    pub display: String,
    /// Number of loaded user-defined custom actions.
    #[serde(default)]
    pub custom_action_count: usize,
    /// Resident memory usage in KiB (`VmRSS` from `/proc/self/status`), if available.
    #[serde(default)]
    pub rss_kib: Option<u64>,
}

/// IPC response returned by the daemon.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum IpcResponse {
    /// Request succeeded.
    Ok {
        /// Human-readable summary message.
        message: String,
        /// Optional daemon status payload.
        #[serde(skip_serializing_if = "Option::is_none")]
        daemon: Option<DaemonStatus>,
    },
    /// Request failed.
    Error {
        /// Error description.
        message: String,
    },
}

/// Returns the default IPC socket path (`$XDG_RUNTIME_DIR/radivine/radivine.sock`).
#[must_use]
pub fn default_socket_path() -> PathBuf {
    if let Ok(xdg_runtime) = std::env::var("XDG_RUNTIME_DIR")
        && !xdg_runtime.trim().is_empty()
    {
        return PathBuf::from(xdg_runtime)
            .join("radivine")
            .join("radivine.sock");
    }
    let uid = std::env::var("UID").unwrap_or_else(|_| "1000".to_string());
    PathBuf::from(format!("/tmp/radivine-{uid}")).join("radivine.sock")
}

/// Binds the non-blocking Unix domain socket listener at `sock_path` (`0700` dir, `0600` socket).
///
/// If an existing daemon is already listening on `sock_path`, returns `Ok(None)` so the caller can
/// exit cleanly with code `0`. If `sock_path` is a stale socket from a crashed process, removes it
/// and binds a fresh listener.
pub fn bind_single_instance(sock_path: &Path) -> anyhow::Result<Option<UnixListener>> {
    if sock_path.exists() {
        if send_request(sock_path, &IpcRequest::Status).is_ok() {
            return Ok(None);
        }
        let _ = fs::remove_file(sock_path);
    }

    if let Some(parent) = sock_path.parent() {
        fs::create_dir_all(parent)?;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }

    let listener = UnixListener::bind(sock_path)?;
    let _ = fs::set_permissions(sock_path, fs::Permissions::from_mode(0o600));
    listener.set_nonblocking(true)?;
    Ok(Some(listener))
}

/// Sends a single [`IpcRequest`] to `sock_path` and waits for the [`IpcResponse`].
pub fn send_request(sock_path: &Path, req: &IpcRequest) -> anyhow::Result<IpcResponse> {
    let mut stream = UnixStream::connect(sock_path)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;

    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line)?;
    if resp_line.trim().is_empty() {
        anyhow::bail!("empty IPC response from daemon");
    }
    let resp: IpcResponse = serde_json::from_str(&resp_line)?;
    Ok(resp)
}

/// Reads a single [`IpcRequest`] from an accepted client [`UnixStream`].
pub fn read_client_request(stream: &mut UnixStream) -> anyhow::Result<IpcRequest> {
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 || n > MAX_IPC_LINE_BYTES {
        anyhow::bail!("invalid IPC request length ({n} bytes)");
    }
    let req: IpcRequest = serde_json::from_str(&line)?;
    Ok(req)
}

/// Writes an [`IpcResponse`] to a client [`UnixStream`].
pub fn write_client_response(stream: &mut UnixStream, resp: &IpcResponse) {
    if let Ok(mut line) = serde_json::to_string(resp) {
        line.push('\n');
        let _ = stream.write_all(line.as_bytes());
        let _ = stream.flush();
    }
}
