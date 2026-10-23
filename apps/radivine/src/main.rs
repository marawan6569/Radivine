//! `radivine` — Daemon, CLI client, IPC server, and diagnostic tool (`ARCHITECTURE.md` §3, §5).

use radivine::{config, ipc, session};

use std::fs;
use std::io::ErrorKind;
use std::os::fd::{AsFd, BorrowedFd, RawFd};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use calloop::generic::Generic;
use calloop::ping::make_ping;
use calloop::{EventLoop, Interest, Mode, PostAction};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use radivine_actions::builtin_registry;
use radivine_core::normalize_cli_arg_to_uri;
use radivine_platform::x11::doctor::probe_x11_doctor;
use radivine_platform::{PhysPoint, connect_with_config};
use radivine_render::{AssetSource, Renderer};

use crate::config::AppConfig;
use crate::ipc::{
    IpcRequest, IpcResponse, bind_single_instance, default_socket_path, read_client_request,
    send_request, write_client_response,
};
use crate::session::SessionController;

/// Radivine — Context-aware radial action engine (`Branch into action.`).
#[derive(Debug, Parser)]
#[command(
    name = "radivine",
    version,
    about = "Context-aware radial action engine"
)]
struct Cli {
    /// Optional path to `config.toml`.
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    /// Optional path to the IPC Unix domain socket.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    /// Subcommand to execute.
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run the persistent background Radivine daemon.
    Daemon,
    /// Summon the radial wheel at the cursor in Invoke mode.
    Invoke {
        /// File paths or `file://` URIs to act upon.
        #[arg(value_name = "PATHS")]
        paths: Vec<String>,

        /// Optional explicit root pixel coordinates `X,Y` (e.g. `960,540`).
        #[arg(long, value_name = "X,Y")]
        at: Option<String>,
    },
    /// Query the status of the running Radivine daemon.
    Status {
        /// Output raw JSON status payload.
        #[arg(long)]
        json: bool,
    },
    /// Reload `config.toml` in the running daemon.
    Reload,
    /// Dismiss any active radial wheel session.
    Dismiss,
    /// Cleanly stop the running Radivine daemon.
    Quit,
    /// Probe X11 extensions, visuals, compositor, monitors, and capabilities.
    Doctor,
}

struct PollFdWrapper(RawFd);

#[allow(unsafe_code)]
impl AsFd for PollFdWrapper {
    fn as_fd(&self) -> BorrowedFd<'_> {
        // SAFETY: `self.0` is the live X11 connection socket file descriptor owned by
        // `SessionController::platform`, which outlives the `calloop::EventLoop` registration.
        unsafe { BorrowedFd::borrow_raw(self.0) }
    }
}

struct DaemonLoopState {
    controller: SessionController,
    config_path: Option<PathBuf>,
    should_quit: bool,
    x11_error: Option<String>,
}

fn main() -> ExitCode {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let sock_path = cli.socket.unwrap_or_else(default_socket_path);

    match cli.command {
        Commands::Doctor => run_doctor(),
        Commands::Daemon => match run_daemon(cli.config, &sock_path) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("radivine daemon error: {err:#}");
                ExitCode::FAILURE
            }
        },
        Commands::Invoke { paths, at } => {
            let position = match at.as_deref().map(parse_xy_pair).transpose() {
                Ok(pos) => pos,
                Err(err) => {
                    eprintln!("invalid --at coordinate: {err}");
                    return ExitCode::FAILURE;
                }
            };
            let uris = normalize_cli_paths(&paths);
            let req = IpcRequest::Invoke { uris, position };
            run_client_request(&sock_path, &req, false)
        }
        Commands::Status { json } => run_client_request(&sock_path, &IpcRequest::Status, json),
        Commands::Reload => run_client_request(&sock_path, &IpcRequest::Reload, false),
        Commands::Dismiss => run_client_request(&sock_path, &IpcRequest::Dismiss, false),
        Commands::Quit => run_client_request(&sock_path, &IpcRequest::Quit, false),
    }
}

fn run_doctor() -> ExitCode {
    match probe_x11_doctor() {
        Ok(report) => {
            print!("{report}");
            if report.is_healthy() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(err) => {
            eprintln!("radivine doctor failed: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run_client_request(sock_path: &Path, req: &IpcRequest, json_out: bool) -> ExitCode {
    match send_request(sock_path, req) {
        Ok(IpcResponse::Ok { message, daemon }) => {
            if json_out
                && let Some(ref d) = daemon
                && let Ok(s) = serde_json::to_string_pretty(d)
            {
                println!("{s}");
                return ExitCode::SUCCESS;
            }
            println!("{message}");
            if let Some(d) = daemon {
                println!(
                    "  pid={} window=0x{:08x} state={} scale={:.2} invokes={} last_action={:?}",
                    d.pid,
                    d.overlay_window_id,
                    d.session_state,
                    d.scale,
                    d.invoke_count,
                    d.last_committed_action
                );
            }
            ExitCode::SUCCESS
        }
        Ok(IpcResponse::Error { message }) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!(
                "failed to communicate with radivine daemon at {}: {err}",
                sock_path.display()
            );
            ExitCode::FAILURE
        }
    }
}

fn run_daemon(config_path: Option<PathBuf>, sock_path: &Path) -> anyhow::Result<()> {
    let Some(listener) = bind_single_instance(sock_path)? else {
        tracing::info!(
            socket = %sock_path.display(),
            "radivine daemon is already running; exiting"
        );
        return Ok(());
    };

    let config = AppConfig::load_or_default(config_path.as_deref());
    let platform = connect_with_config(config.to_x11_config())?;
    let overlay_win_id = platform.overlay_window_id();

    install_emergency_panic_hook(overlay_win_id);

    let renderer = Renderer::new(config.resolve_theme(), AssetSource::Embedded)?;
    let registry = Arc::new(builtin_registry());

    let x11_fd = platform.poll_fd();
    let mut controller = SessionController::new(platform, renderer, registry, config);
    let actions_path = AppConfig::actions_path_for(config_path.as_deref());
    controller.load_actions_from_path(&actions_path);
    check_and_emit_wayland_first_run_notice(&mut controller);

    let mut event_loop: EventLoop<'static, DaemonLoopState> = EventLoop::try_new()?;
    let handle = event_loop.handle();

    // Register background wakeup ping for Enricher and Dispatcher events
    let (ping, ping_source) =
        make_ping().map_err(|e| anyhow::anyhow!("failed to create calloop ping: {e}"))?;
    controller.set_wakeup_callback(Box::new(move || {
        ping.ping();
    }));
    handle
        .insert_source(
            ping_source,
            |_event, _metadata, state: &mut DaemonLoopState| {
                state.controller.drain_async_events(Instant::now());
            },
        )
        .map_err(|e| anyhow::anyhow!("failed to register ping source with calloop: {e}"))?;

    // Register X11 socket FD
    handle
        .insert_source(
            Generic::new(PollFdWrapper(x11_fd), Interest::READ, Mode::Level),
            |_readiness, _metadata, state: &mut DaemonLoopState| {
                let mut events = Vec::new();
                if let Err(err) = state
                    .controller
                    .platform_mut()
                    .dispatch_pending(&mut events)
                {
                    tracing::error!("X11 dispatch error: {err}");
                    state.x11_error = Some(err.to_string());
                    state.should_quit = true;
                }
                if !events.is_empty() {
                    state.controller.handle_platform_events(events);
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow::anyhow!("failed to register X11 fd with calloop: {e}"))?;

    // Register Unix domain socket IPC listener
    handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            |_readiness, listener_ref, state: &mut DaemonLoopState| {
                loop {
                    match listener_ref.accept() {
                        Ok((mut stream, _addr)) => {
                            handle_ipc_client(&mut stream, state);
                        }
                        Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                        Err(err) => {
                            tracing::warn!("IPC accept error: {err}");
                            break;
                        }
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow::anyhow!("failed to register IPC listener with calloop: {e}"))?;

    tracing::info!(
        pid = std::process::id(),
        overlay_window = format!("0x{overlay_win_id:08x}"),
        socket = %sock_path.display(),
        "radivine daemon ready"
    );

    let mut state = DaemonLoopState {
        controller,
        config_path,
        should_quit: false,
        x11_error: None,
    };

    while !state.should_quit {
        state.controller.drain_async_events(Instant::now());

        // Drain any X11 events already buffered in userspace by x11rb before sleeping
        let mut buffered = Vec::new();
        if let Err(err) = state
            .controller
            .platform_mut()
            .dispatch_pending(&mut buffered)
        {
            tracing::error!("X11 dispatch error: {err}");
            state.x11_error = Some(err.to_string());
            break;
        }
        if !buffered.is_empty() {
            state.controller.handle_platform_events(buffered);
        }

        let now = Instant::now();
        let timeout = state
            .controller
            .next_deadline()
            .map(|dl| dl.saturating_duration_since(now));

        event_loop.dispatch(timeout, &mut state)?;
        state.controller.on_timer(Instant::now());
    }

    state.controller.dismiss_immediate();
    state.controller.log_timing_summary_if_enabled();
    let _ = fs::remove_file(sock_path);
    if let Some(err) = state.x11_error {
        anyhow::bail!("X11 connection closed unexpectedly: {err}");
    }
    tracing::info!("radivine daemon stopped cleanly");
    Ok(())
}

fn check_and_emit_wayland_first_run_notice(controller: &mut SessionController) {
    let is_wayland = std::env::var("XDG_SESSION_TYPE")
        .is_ok_and(|s| s.trim().eq_ignore_ascii_case("wayland"))
        || std::env::var("WAYLAND_DISPLAY").is_ok_and(|s| !s.trim().is_empty());
    if !is_wayland {
        return;
    }

    let state_base = std::env::var("XDG_STATE_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local").join("state"))
        });

    let Some(base) = state_base else {
        return;
    };
    let marker = base.join("radivine").join("wayland-notified");
    if marker.exists() {
        return;
    }
    if let Some(parent) = marker.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&marker, "1\n");

    let msg = "Drag-and-Shift isn't available on Wayland. Use right-click -> Radivine..., or radivine invoke.";
    tracing::warn!("{msg}");
    controller.notify_startup("Wayland session detected", msg);
}

fn handle_ipc_client(stream: &mut std::os::unix::net::UnixStream, state: &mut DaemonLoopState) {
    let req = match read_client_request(stream) {
        Ok(r) => r,
        Err(err) => {
            write_client_response(
                stream,
                &IpcResponse::Error {
                    message: err.to_string(),
                },
            );
            return;
        }
    };

    match req {
        IpcRequest::Invoke { uris, position } => {
            let explicit_pos = position.map(|(x, y)| PhysPoint::new(x, y));
            match state.controller.summon_invoke(uris, explicit_pos) {
                Ok(()) => {
                    write_client_response(
                        stream,
                        &IpcResponse::Ok {
                            message: "summoned radial wheel".to_string(),
                            daemon: Some(state.controller.status()),
                        },
                    );
                }
                Err(err) => {
                    write_client_response(
                        stream,
                        &IpcResponse::Error {
                            message: err.to_string(),
                        },
                    );
                }
            }
        }
        IpcRequest::Status => {
            write_client_response(
                stream,
                &IpcResponse::Ok {
                    message: "radivine daemon active".to_string(),
                    daemon: Some(state.controller.status()),
                },
            );
        }
        IpcRequest::Reload => {
            let new_cfg = AppConfig::load_or_default(state.config_path.as_deref());
            let actions_path = AppConfig::actions_path_for(state.config_path.as_deref());
            state.controller.reload_all(new_cfg, &actions_path);
            write_client_response(
                stream,
                &IpcResponse::Ok {
                    message: "reloaded configuration".to_string(),
                    daemon: Some(state.controller.status()),
                },
            );
        }
        IpcRequest::Dismiss => {
            state.controller.dismiss_immediate();
            write_client_response(
                stream,
                &IpcResponse::Ok {
                    message: "dismissed active session".to_string(),
                    daemon: Some(state.controller.status()),
                },
            );
        }
        IpcRequest::Quit => {
            state.controller.dismiss_immediate();
            state.should_quit = true;
            write_client_response(
                stream,
                &IpcResponse::Ok {
                    message: "shutting down daemon".to_string(),
                    daemon: None,
                },
            );
        }
    }
}

fn normalize_cli_paths(raw_args: &[String]) -> Vec<String> {
    let base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let mut out = Vec::with_capacity(raw_args.len());
    for raw in raw_args {
        if let Ok(uri) = normalize_cli_arg_to_uri(raw, &base_dir) {
            out.push(uri);
        }
    }
    out
}

fn parse_xy_pair(s: &str) -> Result<(i32, i32), String> {
    let (xs, ys) = s
        .split_once(',')
        .ok_or_else(|| "expected X,Y format (e.g. 960,540)".to_string())?;
    let x = xs
        .trim()
        .parse::<i32>()
        .map_err(|e| format!("invalid X coordinate: {e}"))?;
    let y = ys
        .trim()
        .parse::<i32>()
        .map_err(|e| format!("invalid Y coordinate: {e}"))?;
    Ok((x, y))
}

fn install_emergency_panic_hook(overlay_win_id: u32) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok((conn, _)) = x11rb::rust_connection::RustConnection::connect(None) {
            use x11rb::connection::Connection as _;
            use x11rb::protocol::xproto::ConnectionExt as _;
            let _ = conn.ungrab_pointer(x11rb::CURRENT_TIME);
            let _ = conn.ungrab_keyboard(x11rb::CURRENT_TIME);
            let _ = conn.unmap_window(overlay_win_id);
            let _ = conn.flush();
        }
        default_hook(info);
    }));
}
