//! Background daemon lifecycle, `systemd --user` integration, detached fallback, and autostart
//! management (`CONTROL_CENTER_DESIGN.md` §3).

use std::fs::{self, OpenOptions};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use radivine::ipc::{DaemonStatus, IpcRequest, IpcResponse, default_socket_path, send_request};

/// Maximum size of `~/.local/state/radivine/daemon.log` before rotating to `daemon.log.1` (`2 MiB`).
const MAX_DAEMON_LOG_BYTES: u64 = 2 * 1024 * 1024;

/// High-level daemon health status combining systemd unit state and live IPC responsiveness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaemonHealthState {
    /// Daemon is running and responsive over its Unix domain socket (`radivine.sock`).
    Running,
    /// Daemon is stopped and no systemd unit is active.
    Stopped,
    /// Start or Restart transition is in progress.
    Starting,
    /// Stop transition is in progress.
    Stopping,
    /// Systemd unit entered `failed` state or the last startup attempt failed.
    Failed,
    /// Systemd reports the unit as `active`, but the daemon IPC socket is unresponsive.
    Unreachable,
}

impl DaemonHealthState {
    /// Human-readable status badge text.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Stopped => "Stopped",
            Self::Starting => "Starting…",
            Self::Stopping => "Stopping…",
            Self::Failed => "Failed",
            Self::Unreachable => "Unreachable / Degraded",
        }
    }
}

/// Detected autostart conflict or legacy configuration requiring safe user resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AutostartConflict {
    /// Both `systemd --user` (`radivine.service`) and an XDG autostart `.desktop` entry are enabled.
    DuplicateXdgAndSystemd {
        /// Path to the active XDG autostart `.desktop` file.
        xdg_path: PathBuf,
    },
    /// Legacy v0.1.0 `~/.config/systemd/user/radivine.service` targets only `graphical-session.target`
    /// (which Cinnamon does not activate at login).
    LegacyGraphicalSessionTargetOnly {
        /// Path to the user systemd unit override file.
        unit_path: PathBuf,
    },
}

impl AutostartConflict {
    /// Human-readable explanation of the conflict and its resolution.
    #[must_use]
    pub fn description(&self) -> String {
        match self {
            Self::DuplicateXdgAndSystemd { xdg_path } => format!(
                "Both the systemd user service (radivine.service) and the XDG autostart entry ({}) are enabled. \
                 Resolve this to keep systemd as the single startup mechanism (backs up the XDG file to .bak and sets X-GNOME-Autostart-enabled=false).",
                xdg_path.display()
            ),
            Self::LegacyGraphicalSessionTargetOnly { unit_path } => format!(
                "User service override ({}) targets only graphical-session.target, which Cinnamon does not activate on login. \
                 Upgrade the unit file to include WantedBy=default.target (backs up the existing file to .bak).",
                unit_path.display()
            ),
        }
    }
}

/// Current login autostart configuration and health.
#[derive(Clone, Debug)]
pub struct AutostartStatus {
    /// Whether Radivine is configured to launch automatically on desktop login.
    pub enabled: bool,
    /// Active startup mechanism description.
    pub method_label: String,
    /// Whether `systemd --user` is available on this system.
    pub systemd_available: bool,
    /// Whether `radivine.service` is enabled in `systemd --user`.
    pub systemd_enabled: bool,
    /// Whether an XDG autostart `.desktop` file is enabled in `~/.config/autostart/`.
    pub xdg_enabled: bool,
    /// Any detected duplicate or legacy startup conflicts.
    pub conflicts: Vec<AutostartConflict>,
}

impl Default for AutostartStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            method_label: "Disabled".to_string(),
            systemd_available: false,
            systemd_enabled: false,
            xdg_enabled: false,
            conflicts: Vec::new(),
        }
    }
}

/// Probed systemd unit properties (`systemctl --user show radivine.service`).
#[derive(Clone, Debug, Default)]
pub struct SystemdUnitInfo {
    /// Whether `systemd --user` responded to queries.
    pub available: bool,
    /// `LoadState` (`"loaded"`, `"not-found"`, etc.).
    pub load_state: String,
    /// `ActiveState` (`"active"`, `"inactive"`, `"failed"`, `"activating"`, `"deactivating"`).
    pub active_state: String,
    /// `SubState` (`"running"`, `"dead"`, `"failed"`, etc.).
    pub sub_state: String,
    /// `UnitFileState` (`"enabled"`, `"disabled"`, `"static"`, etc.).
    pub unit_file_state: String,
    /// `MainPID` (`0` when not running).
    pub main_pid: u32,
    /// `Result` (`"success"`, `"exit-code"`, etc.).
    pub result: String,
}

/// Complete snapshot of daemon service state, IPC status, and autostart configuration.
#[derive(Clone, Debug)]
pub struct ServiceSnapshot {
    /// Combined health state.
    pub health: DaemonHealthState,
    /// Live IPC status payload if the daemon is reachable over `radivine.sock`.
    pub daemon: Option<DaemonStatus>,
    /// Probed `systemd --user` unit properties.
    pub systemd: SystemdUnitInfo,
    /// Human-readable startup management mode (`"Systemd User Service"`, `"Standalone Process"`, etc.).
    pub management_mode: String,
    /// Login autostart configuration and conflict audit.
    pub autostart: AutostartStatus,
    /// Desktop session type (`"x11"`, `"wayland"`, etc.).
    pub session_type: String,
    /// Active desktop environment (`"X-Cinnamon"`, etc.).
    pub desktop_environment: String,
    /// IPC socket path being monitored.
    pub socket_path: PathBuf,
    /// Last meaningful service or daemon error message, if any.
    pub last_error: Option<String>,
}

/// Configurable filesystem and socket paths for [`ServiceController`] (supports isolated testing).
#[derive(Clone, Debug)]
pub struct ServicePaths {
    /// Daemon Unix domain socket path (`$XDG_RUNTIME_DIR/radivine/radivine.sock`).
    pub ipc_socket: PathBuf,
    /// Optional explicit `config.toml` path passed to `radivine`.
    pub config_path: Option<PathBuf>,
    /// User systemd directory (`~/.config/systemd/user`).
    pub systemd_user_dir: PathBuf,
    /// System-installed systemd user unit path (`/usr/lib/systemd/user/radivine.service`).
    pub system_unit_path: PathBuf,
    /// User XDG autostart directory (`~/.config/autostart`).
    pub autostart_dir: PathBuf,
    /// User state directory for fallback logs (`~/.local/state/radivine`).
    pub state_dir: PathBuf,
    /// Resolved `radivine` executable path.
    pub daemon_bin: PathBuf,
    /// Whether to use `systemctl --user` when available (`false` forces detached fallback mode in tests).
    pub allow_systemd: bool,
}

impl ServicePaths {
    /// Resolves standard XDG paths for the current user session.
    #[must_use]
    pub fn default_user(config_path: Option<PathBuf>, socket_override: Option<PathBuf>) -> Self {
        let ipc_socket = socket_override.unwrap_or_else(default_socket_path);
        let config_base = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| PathBuf::from(h).join(".config"))
            })
            .unwrap_or_else(|| PathBuf::from(".config"));

        let state_base = std::env::var("XDG_STATE_HOME")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| PathBuf::from(h).join(".local").join("state"))
            })
            .unwrap_or_else(|| PathBuf::from(".local/state"));

        let force_fallback = std::env::var("RADIVINE_CONTROL_FORCE_FALLBACK")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

        Self {
            ipc_socket,
            config_path,
            systemd_user_dir: config_base.join("systemd").join("user"),
            system_unit_path: PathBuf::from("/usr/lib/systemd/user/radivine.service"),
            autostart_dir: config_base.join("autostart"),
            state_dir: state_base.join("radivine"),
            daemon_bin: resolve_daemon_binary(),
            allow_systemd: !force_fallback,
        }
    }

    /// Returns the path to `~/.local/state/radivine/daemon.log`.
    #[must_use]
    pub fn daemon_log_path(&self) -> PathBuf {
        self.state_dir.join("daemon.log")
    }
}

/// Manages daemon lifecycle (`Start`, `Stop`, `Restart`, `Reload`) and login autostart preferences.
#[derive(Clone, Debug)]
pub struct ServiceController {
    paths: ServicePaths,
}

impl ServiceController {
    /// Creates a new [`ServiceController`] with the given [`ServicePaths`].
    #[must_use]
    pub const fn new(paths: ServicePaths) -> Self {
        Self { paths }
    }

    /// Returns a reference to the configured [`ServicePaths`].
    #[must_use]
    pub const fn paths(&self) -> &ServicePaths {
        &self.paths
    }

    /// Probes the current [`ServiceSnapshot`] without modifying any state.
    #[must_use]
    pub fn query_snapshot(&self, last_op_error: Option<String>) -> ServiceSnapshot {
        let systemd = if self.paths.allow_systemd {
            probe_systemd_unit()
        } else {
            SystemdUnitInfo::default()
        };

        let ipc_status = match send_request(&self.paths.ipc_socket, &IpcRequest::Status) {
            Ok(IpcResponse::Ok { daemon, .. }) => daemon,
            _ => None,
        };

        let autostart = self.query_autostart(&systemd);

        let (health, management_mode) = match (&ipc_status, systemd.active_state.as_str()) {
            (Some(d), "active") => {
                let mode = if systemd.main_pid == d.pid || systemd.main_pid != 0 {
                    "Systemd User Service (radivine.service)".to_string()
                } else {
                    "Standalone Background Process".to_string()
                };
                (DaemonHealthState::Running, mode)
            }
            (Some(_), _) => (
                DaemonHealthState::Running,
                if systemd.available {
                    "Standalone Background Process".to_string()
                } else {
                    "Detached Background Process (Fallback)".to_string()
                },
            ),
            (None, "activating") => (
                DaemonHealthState::Starting,
                "Systemd User Service (radivine.service)".to_string(),
            ),
            (None, "deactivating") => (
                DaemonHealthState::Stopping,
                "Systemd User Service (radivine.service)".to_string(),
            ),
            (None, "active") => (
                DaemonHealthState::Unreachable,
                "Systemd User Service (IPC Unresponsive)".to_string(),
            ),
            (None, "failed") => (
                DaemonHealthState::Failed,
                "Systemd User Service (Failed)".to_string(),
            ),
            (None, _) => (
                DaemonHealthState::Stopped,
                if systemd.available {
                    "Systemd User Service (Inactive)".to_string()
                } else {
                    "Detached Background Process".to_string()
                },
            ),
        };

        let last_error = last_op_error
            .or_else(|| {
                ipc_status
                    .as_ref()
                    .and_then(|d| d.last_job_error.clone())
                    .or_else(|| {
                        ipc_status
                            .as_ref()
                            .and_then(|d| d.config_warnings.first().cloned())
                    })
            })
            .or_else(|| {
                if health == DaemonHealthState::Failed && !systemd.result.is_empty() {
                    Some(format!(
                        "radivine.service failed (result: {})",
                        systemd.result
                    ))
                } else if health == DaemonHealthState::Unreachable {
                    Some(format!(
                        "systemd reports radivine.service as active (PID {}), but IPC socket {} is not responding.",
                        systemd.main_pid,
                        self.paths.ipc_socket.display()
                    ))
                } else {
                    None
                }
            });

        let session_type = ipc_status
            .as_ref()
            .filter(|d| !d.desktop_session.is_empty() && d.desktop_session != "unknown")
            .map(|d| d.desktop_session.clone())
            .unwrap_or_else(|| {
                std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string())
            });

        let desktop_environment = ipc_status
            .as_ref()
            .filter(|d| !d.desktop_environment.is_empty() && d.desktop_environment != "unknown")
            .map(|d| d.desktop_environment.clone())
            .unwrap_or_else(|| {
                std::env::var("XDG_CURRENT_DESKTOP")
                    .or_else(|_| std::env::var("DESKTOP_SESSION"))
                    .unwrap_or_else(|_| "unknown".to_string())
            });

        ServiceSnapshot {
            health,
            daemon: ipc_status,
            systemd,
            management_mode,
            autostart,
            session_type,
            desktop_environment,
            socket_path: self.paths.ipc_socket.clone(),
            last_error,
        }
    }

    /// Starts the background daemon without terminal dependency and verifies IPC readiness.
    pub fn start_daemon(&self) -> Result<ServiceSnapshot, String> {
        // Single-instance check: if already responsive over IPC, return current snapshot immediately
        if let Ok(IpcResponse::Ok {
            daemon: Some(_), ..
        }) = send_request(&self.paths.ipc_socket, &IpcRequest::Status)
        {
            return Ok(self.query_snapshot(None));
        }

        let systemd = if self.paths.allow_systemd {
            probe_systemd_unit()
        } else {
            SystemdUnitInfo::default()
        };

        if systemd.available && self.paths.config_path.is_none() {
            self.ensure_systemd_unit_available(&systemd)?;
            import_graphical_environment_to_systemd();
            let output = Command::new("systemctl")
                .args(["--user", "start", "radivine.service"])
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("Failed to invoke systemctl --user start: {e}"))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(format!(
                    "systemctl --user start radivine.service failed: {stderr}"
                ));
            }
        } else {
            self.spawn_detached_fallback_daemon()?;
        }

        self.wait_for_ipc_ready(Duration::from_millis(4500))
    }

    /// Stops the running background daemon cleanly and verifies shutdown.
    pub fn stop_daemon(&self) -> Result<ServiceSnapshot, String> {
        let systemd = if self.paths.allow_systemd {
            probe_systemd_unit()
        } else {
            SystemdUnitInfo::default()
        };

        if systemd.available
            && matches!(systemd.active_state.as_str(), "active" | "activating")
            && self.paths.config_path.is_none()
        {
            let output = Command::new("systemctl")
                .args(["--user", "stop", "radivine.service"])
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("Failed to invoke systemctl --user stop: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let _ = send_request(&self.paths.ipc_socket, &IpcRequest::Quit);
                return Err(format!(
                    "systemctl --user stop radivine.service reported an error: {stderr}"
                ));
            }
        }

        // Also send Quit over IPC in case a standalone process was running outside systemd
        if self.paths.ipc_socket.exists() {
            let _ = send_request(&self.paths.ipc_socket, &IpcRequest::Quit);
        }

        self.wait_for_ipc_stopped(Duration::from_millis(4000))
    }

    /// Restarts the background daemon cleanly and verifies the new instance is responsive over IPC.
    pub fn restart_daemon(&self) -> Result<ServiceSnapshot, String> {
        let systemd = if self.paths.allow_systemd {
            probe_systemd_unit()
        } else {
            SystemdUnitInfo::default()
        };

        if systemd.available
            && matches!(
                systemd.active_state.as_str(),
                "active" | "activating" | "failed"
            )
            && self.paths.config_path.is_none()
        {
            // Also stop any standalone process holding the socket if MainPID differs
            if let Ok(IpcResponse::Ok {
                daemon: Some(d), ..
            }) = send_request(&self.paths.ipc_socket, &IpcRequest::Status)
                && systemd.main_pid != 0
                && systemd.main_pid != d.pid
            {
                let _ = send_request(&self.paths.ipc_socket, &IpcRequest::Quit);
                let _ = self.wait_for_ipc_stopped(Duration::from_millis(2000));
            }

            self.ensure_systemd_unit_available(&systemd)?;
            import_graphical_environment_to_systemd();
            let output = Command::new("systemctl")
                .args(["--user", "restart", "radivine.service"])
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("Failed to invoke systemctl --user restart: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(format!(
                    "systemctl --user restart radivine.service failed: {stderr}"
                ));
            }
            return self.wait_for_ipc_ready(Duration::from_millis(4500));
        }

        let _ = self.stop_daemon();
        self.start_daemon()
    }

    /// Sends `IpcRequest::Reload` to the running daemon if reachable.
    pub fn reload_daemon(&self) -> Result<ServiceSnapshot, String> {
        match send_request(&self.paths.ipc_socket, &IpcRequest::Reload) {
            Ok(IpcResponse::Ok { .. }) => Ok(self.query_snapshot(None)),
            Ok(IpcResponse::Error { message }) => Err(format!("Daemon reload failed: {message}")),
            Err(err) => Err(format!("Daemon is not running or unreachable: {err}")),
        }
    }

    /// Enables or disables **Launch Radivine at Login** without stopping or starting the running daemon.
    pub fn set_autostart(&self, enable: bool) -> Result<ServiceSnapshot, String> {
        let systemd = if self.paths.allow_systemd {
            probe_systemd_unit()
        } else {
            SystemdUnitInfo::default()
        };

        if systemd.available {
            if enable {
                self.ensure_systemd_unit_available(&systemd)?;
                let output = Command::new("systemctl")
                    .args(["--user", "enable", "radivine.service"])
                    .stdin(Stdio::null())
                    .output()
                    .map_err(|e| format!("Failed to run systemctl --user enable: {e}"))?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    return Err(format!(
                        "Failed to enable systemd autostart (radivine.service): {stderr}"
                    ));
                }
            } else {
                let output = Command::new("systemctl")
                    .args(["--user", "disable", "radivine.service"])
                    .stdin(Stdio::null())
                    .output()
                    .map_err(|e| format!("Failed to run systemctl --user disable: {e}"))?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    return Err(format!(
                        "Failed to disable systemd autostart (radivine.service): {stderr}"
                    ));
                }
                // Also disable any active XDG autostart entries without deleting user files
                self.disable_xdg_autostart_files()?;
            }
        } else if enable {
            self.write_xdg_autostart_file(true)?;
        } else {
            self.disable_xdg_autostart_files()?;
        }

        Ok(self.query_snapshot(None))
    }

    /// Safely resolves a detected [`AutostartConflict`] by creating a `.bak` backup and updating the
    /// conflicting configuration without deleting user files.
    pub fn resolve_conflict(
        &self,
        conflict: &AutostartConflict,
    ) -> Result<ServiceSnapshot, String> {
        match conflict {
            AutostartConflict::DuplicateXdgAndSystemd { xdg_path } => {
                disable_single_xdg_file_with_backup(xdg_path)
                    .map_err(|e| format!("Failed to update {}: {e}", xdg_path.display()))?;
            }
            AutostartConflict::LegacyGraphicalSessionTargetOnly { unit_path } => {
                let mut bak_os = unit_path.as_os_str().to_os_string();
                bak_os.push(".bak");
                let bak_path = PathBuf::from(bak_os);
                fs::copy(unit_path, &bak_path)
                    .map_err(|e| format!("Failed to back up {}: {e}", unit_path.display()))?;

                let unit_content = generate_systemd_unit_content(&self.paths.daemon_bin);
                fs::write(unit_path, unit_content)
                    .map_err(|e| format!("Failed to update {}: {e}", unit_path.display()))?;

                if self.paths.allow_systemd {
                    let _ = Command::new("systemctl")
                        .args(["--user", "daemon-reload"])
                        .stdin(Stdio::null())
                        .output();
                    let _ = Command::new("systemctl")
                        .args(["--user", "reenable", "radivine.service"])
                        .stdin(Stdio::null())
                        .output();
                }
            }
        }
        Ok(self.query_snapshot(None))
    }

    fn query_autostart(&self, systemd: &SystemdUnitInfo) -> AutostartStatus {
        let user_unit_path = self.paths.systemd_user_dir.join("radivine.service");
        let wants_default = self
            .paths
            .systemd_user_dir
            .join("default.target.wants")
            .join("radivine.service")
            .exists();
        let wants_graphical = self
            .paths
            .systemd_user_dir
            .join("graphical-session.target.wants")
            .join("radivine.service")
            .exists();

        let systemd_enabled = systemd.unit_file_state == "enabled"
            || systemd.unit_file_state == "enabled-runtime"
            || wants_default
            || wants_graphical;

        let active_xdg_paths = self.find_active_xdg_autostart_files();
        let xdg_enabled = !active_xdg_paths.is_empty();

        let mut conflicts = Vec::new();
        if systemd_enabled {
            for xdg_path in &active_xdg_paths {
                conflicts.push(AutostartConflict::DuplicateXdgAndSystemd {
                    xdg_path: xdg_path.clone(),
                });
            }
        }

        if user_unit_path.exists()
            && let Ok(text) = fs::read_to_string(&user_unit_path)
            && text.contains("WantedBy=graphical-session.target")
            && !text.contains("WantedBy=default.target")
        {
            conflicts.push(AutostartConflict::LegacyGraphicalSessionTargetOnly {
                unit_path: user_unit_path,
            });
        }

        let enabled = systemd_enabled || xdg_enabled;
        let method_label = match (systemd_enabled, xdg_enabled) {
            (true, true) => "Systemd User Service + XDG Autostart (Conflict)".to_string(),
            (true, false) => "Systemd User Service (radivine.service)".to_string(),
            (false, true) => "XDG Autostart (~/.config/autostart)".to_string(),
            (false, false) => {
                if systemd.available {
                    "Disabled (Systemd User Service ready)".to_string()
                } else {
                    "Disabled (XDG Autostart ready)".to_string()
                }
            }
        };

        AutostartStatus {
            enabled,
            method_label,
            systemd_available: systemd.available,
            systemd_enabled,
            xdg_enabled,
            conflicts,
        }
    }

    fn find_active_xdg_autostart_files(&self) -> Vec<PathBuf> {
        let mut active = Vec::new();
        for name in ["radivine-daemon.desktop", "radivine.desktop"] {
            let candidate = self.paths.autostart_dir.join(name);
            if candidate.exists() && is_xdg_desktop_file_autostart_enabled(&candidate) {
                active.push(candidate);
            }
        }
        active
    }

    fn write_xdg_autostart_file(&self, enabled: bool) -> Result<(), String> {
        fs::create_dir_all(&self.paths.autostart_dir).map_err(|e| {
            format!(
                "Failed to create {}: {e}",
                self.paths.autostart_dir.display()
            )
        })?;
        let target = self.paths.autostart_dir.join("radivine-daemon.desktop");
        let exec_bin = self.paths.daemon_bin.display();
        let content = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Version=1.0\n\
             Name=Radivine Daemon\n\
             GenericName=Radial Action Wheel Daemon\n\
             Comment=Start the Radivine background daemon on desktop login\n\
             Exec={exec_bin} daemon\n\
             TryExec={exec_bin}\n\
             Icon=radivine\n\
             Terminal=false\n\
             Categories=Utility;\n\
             StartupNotify=false\n\
             X-GNOME-Autostart-enabled={enabled}\n"
        );
        fs::write(&target, content)
            .map_err(|e| format!("Failed to write {}: {e}", target.display()))
    }

    fn disable_xdg_autostart_files(&self) -> Result<(), String> {
        for path in self.find_active_xdg_autostart_files() {
            disable_single_xdg_file_with_backup(&path)
                .map_err(|e| format!("Failed to disable {}: {e}", path.display()))?;
        }
        Ok(())
    }

    fn ensure_systemd_unit_available(&self, systemd: &SystemdUnitInfo) -> Result<(), String> {
        let user_unit = self.paths.systemd_user_dir.join("radivine.service");
        if self.paths.system_unit_path.exists()
            || user_unit.exists()
            || systemd.load_state == "loaded"
        {
            return Ok(());
        }
        fs::create_dir_all(&self.paths.systemd_user_dir).map_err(|e| {
            format!(
                "Failed to create {}: {e}",
                self.paths.systemd_user_dir.display()
            )
        })?;
        let content = generate_systemd_unit_content(&self.paths.daemon_bin);
        fs::write(&user_unit, content)
            .map_err(|e| format!("Failed to write {}: {e}", user_unit.display()))?;
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .stdin(Stdio::null())
            .output();
        Ok(())
    }

    fn spawn_detached_fallback_daemon(&self) -> Result<(), String> {
        fs::create_dir_all(&self.paths.state_dir)
            .map_err(|e| format!("Failed to create {}: {e}", self.paths.state_dir.display()))?;
        let log_path = self.paths.daemon_log_path();
        if let Ok(meta) = fs::metadata(&log_path)
            && meta.len() > MAX_DAEMON_LOG_BYTES
        {
            let rotated = self.paths.state_dir.join("daemon.log.1");
            let _ = fs::rename(&log_path, rotated);
        }

        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| format!("Failed to open {}: {e}", log_path.display()))?;
        let log_err = log_file
            .try_clone()
            .map_err(|e| format!("Failed to clone log file handle: {e}"))?;

        let mut cmd = Command::new(&self.paths.daemon_bin);
        if let Some(ref cfg) = self.paths.config_path {
            cmd.arg("--config").arg(cfg);
        }
        cmd.arg("--socket")
            .arg(&self.paths.ipc_socket)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_err))
            .process_group(0);

        let mut child = cmd.spawn().map_err(|e| {
            format!(
                "Failed to launch {} daemon: {e}",
                self.paths.daemon_bin.display()
            )
        })?;

        // Reap the background child if it exits so no zombie process ever accumulates
        let _ = thread::Builder::new()
            .name("radivine-control-reap".to_string())
            .spawn(move || {
                let _ = child.wait();
            });

        Ok(())
    }

    fn wait_for_ipc_ready(&self, timeout: Duration) -> Result<ServiceSnapshot, String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(IpcResponse::Ok {
                daemon: Some(_), ..
            }) = send_request(&self.paths.ipc_socket, &IpcRequest::Status)
            {
                return Ok(self.query_snapshot(None));
            }
            thread::sleep(Duration::from_millis(75));
        }
        let snap = self.query_snapshot(None);
        Err(snap.last_error.unwrap_or_else(|| {
            format!(
                "Daemon did not become ready on {} within {:.1}s. Check Diagnostics -> Logs for details.",
                self.paths.ipc_socket.display(),
                timeout.as_secs_f32()
            )
        }))
    }

    fn wait_for_ipc_stopped(&self, timeout: Duration) -> Result<ServiceSnapshot, String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if send_request(&self.paths.ipc_socket, &IpcRequest::Status).is_err() {
                return Ok(self.query_snapshot(None));
            }
            thread::sleep(Duration::from_millis(60));
        }
        Err(format!(
            "Daemon is still responding on {} after {:.1}s.",
            self.paths.ipc_socket.display(),
            timeout.as_secs_f32()
        ))
    }
}

/// Generates the systemd user service file content compatible with Cinnamon (`WantedBy=default.target`).
#[must_use]
pub fn generate_systemd_unit_content(daemon_bin: &Path) -> String {
    let bin = daemon_bin.display();
    format!(
        "[Unit]\n\
         Description=Radivine — Context-aware radial action wheel daemon\n\
         Documentation=https://github.com/marawan6569/Radivine\n\
         After=default.target graphical-session-pre.target\n\
         StartLimitIntervalSec=30\n\
         StartLimitBurst=5\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={bin} daemon\n\
         ExecReload={bin} reload\n\
         ExecStop={bin} quit\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         SuccessExitStatus=0 143\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

fn resolve_daemon_binary() -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("radivine");
        if sibling.exists() {
            return sibling;
        }
    }
    let usr_bin = PathBuf::from("/usr/bin/radivine");
    if usr_bin.exists() {
        return usr_bin;
    }
    PathBuf::from("radivine")
}

fn import_graphical_environment_to_systemd() {
    let vars = [
        "DISPLAY",
        "XAUTHORITY",
        "WAYLAND_DISPLAY",
        "XDG_SESSION_TYPE",
        "XDG_CURRENT_DESKTOP",
        "DBUS_SESSION_BUS_ADDRESS",
    ];
    let present: Vec<&str> = vars
        .into_iter()
        .filter(|k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty()))
        .collect();
    if present.is_empty() {
        return;
    }
    let mut cmd = Command::new("systemctl");
    cmd.args(["--user", "import-environment"])
        .args(&present)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = cmd.status();
}

fn probe_systemd_unit() -> SystemdUnitInfo {
    let Ok(output) = Command::new("systemctl")
        .args([
            "--user",
            "show",
            "radivine.service",
            "--property=LoadState,ActiveState,SubState,UnitFileState,MainPID,Result",
        ])
        .stdin(Stdio::null())
        .output()
    else {
        return SystemdUnitInfo::default();
    };

    if !output.status.success() {
        return SystemdUnitInfo::default();
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut info = SystemdUnitInfo {
        available: true,
        ..SystemdUnitInfo::default()
    };

    for line in text.lines() {
        let Some((k, v)) = line.trim().split_once('=') else {
            continue;
        };
        match k {
            "LoadState" => info.load_state = v.to_string(),
            "ActiveState" => info.active_state = v.to_string(),
            "SubState" => info.sub_state = v.to_string(),
            "UnitFileState" => info.unit_file_state = v.to_string(),
            "MainPID" => info.main_pid = v.parse::<u32>().unwrap_or(0),
            "Result" => info.result = v.to_string(),
            _ => {}
        }
    }

    // If `UnitFileState` is empty or `bad` on some systemd versions, also check `is-enabled`
    if info.unit_file_state.is_empty()
        && let Ok(en_out) = Command::new("systemctl")
            .args(["--user", "is-enabled", "radivine.service"])
            .stdin(Stdio::null())
            .output()
    {
        let s = String::from_utf8_lossy(&en_out.stdout).trim().to_string();
        if !s.is_empty() {
            info.unit_file_state = s;
        }
    }

    info
}

fn is_xdg_desktop_file_autostart_enabled(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("Hidden=")
            && val.trim().eq_ignore_ascii_case("true")
        {
            return false;
        }
        if let Some(val) = trimmed.strip_prefix("X-GNOME-Autostart-enabled=")
            && val.trim().eq_ignore_ascii_case("false")
        {
            return false;
        }
    }
    true
}

fn disable_single_xdg_file_with_backup(path: &Path) -> std::io::Result<()> {
    let text = fs::read_to_string(path)?;
    let mut bak_os = path.as_os_str().to_os_string();
    bak_os.push(".bak");
    fs::copy(path, PathBuf::from(bak_os))?;

    let mut replaced = false;
    let mut out_lines = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("X-GNOME-Autostart-enabled=") {
            out_lines.push("X-GNOME-Autostart-enabled=false".to_string());
            replaced = true;
        } else {
            out_lines.push(line.to_string());
        }
    }
    if !replaced {
        out_lines.push("X-GNOME-Autostart-enabled=false".to_string());
    }
    let mut new_content = out_lines.join("\n");
    new_content.push('\n');
    fs::write(path, new_content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_fallback_and_conflict_resolution_preserve_user_files() {
        let dir =
            std::env::temp_dir().join(format!("radivine-control-svc-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let paths = ServicePaths {
            ipc_socket: dir.join("radivine.sock"),
            config_path: Some(dir.join("config.toml")),
            systemd_user_dir: dir.join("systemd/user"),
            system_unit_path: dir.join("system-unit/radivine.service"),
            autostart_dir: dir.join("autostart"),
            state_dir: dir.join("state"),
            daemon_bin: PathBuf::from("/usr/bin/radivine"),
            allow_systemd: false,
        };
        let ctrl = ServiceController::new(paths.clone());

        // Initially stopped & autostart disabled
        let snap0 = ctrl.query_snapshot(None);
        assert_eq!(snap0.health, DaemonHealthState::Stopped);
        assert!(!snap0.autostart.enabled);

        // Enable fallback XDG autostart
        let snap1 = ctrl.set_autostart(true).unwrap();
        assert!(snap1.autostart.enabled);
        assert!(snap1.autostart.xdg_enabled);
        let xdg_file = paths.autostart_dir.join("radivine-daemon.desktop");
        assert!(xdg_file.exists());

        // Simulate duplicate systemd + XDG conflict and legacy unit target
        fs::create_dir_all(paths.systemd_user_dir.join("default.target.wants")).unwrap();
        fs::write(
            paths
                .systemd_user_dir
                .join("default.target.wants/radivine.service"),
            "",
        )
        .unwrap();
        let legacy_unit = paths.systemd_user_dir.join("radivine.service");
        fs::write(
            &legacy_unit,
            "[Unit]\nPartOf=graphical-session.target\n[Install]\nWantedBy=graphical-session.target\n",
        )
        .unwrap();

        let snap2 = ctrl.query_snapshot(None);
        assert_eq!(snap2.autostart.conflicts.len(), 2);

        // Resolve both conflicts and verify .bak backups are created without deleting original files
        for conflict in &snap2.autostart.conflicts {
            ctrl.resolve_conflict(conflict).unwrap();
        }
        assert!(
            paths
                .autostart_dir
                .join("radivine-daemon.desktop.bak")
                .exists()
        );
        assert!(paths.systemd_user_dir.join("radivine.service.bak").exists());

        let snap3 = ctrl.query_snapshot(None);
        assert!(
            snap3.autostart.conflicts.is_empty(),
            "all conflicts should be resolved"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
