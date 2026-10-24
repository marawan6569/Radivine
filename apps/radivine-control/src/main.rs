#![forbid(unsafe_code)]

use std::ffi::OsStr;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use radivine_control::ControlPage;
use radivine_control::service::{ServiceController, ServicePaths};
use radivine_control::single_instance::{ControlInstanceLock, default_control_socket_path};
use radivine_control::ui::{
    export_all_page_screenshots, parse_convert_cli_args, read_bounded_stdin_paths,
    run_advanced_convert_window, run_control_window,
};

fn print_help() {
    println!(
        "radivine-control {} — Radivine Control Center",
        env!("CARGO_PKG_VERSION")
    );
    println!();
    println!("USAGE:");
    println!("    radivine-control [OPTIONS]");
    println!("    radivine-control convert [--stdin] [PATHS...]");
    println!();
    println!("OPTIONS:");
    println!(
        "    --page <NAME>                Open directly to a page: dashboard, general, wheel,"
    );
    println!("                                 actions, conversion, jobs, diagnostics, about");
    println!("    --config <PATH>              Override path to config.toml");
    println!("    --socket <PATH>              Override path to daemon IPC socket");
    println!("    --control-socket <PATH>      Override path to control single-instance socket");
    println!(
        "    --export-screenshots <DIR>   Render deterministic UI screenshots to <DIR> and exit"
    );
    println!(
        "    --check-status-json          Print service & daemon health snapshot as JSON and exit"
    );
    println!("    -V, --version                Print version information and exit");
    println!("    -h, --help                   Print this help message and exit");
}

fn main() -> ExitCode {
    let raw_os_args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if raw_os_args.first().map(|a| a.as_os_str()) == Some(OsStr::new("convert")) {
        let parsed = match parse_convert_cli_args(&raw_os_args[1..]) {
            Ok(p) => p,
            Err(err) => {
                eprintln!("Error: {err}");
                return ExitCode::from(2);
            }
        };
        let mut sources = parsed.paths;
        if parsed.read_stdin {
            let stdin = io::stdin();
            match read_bounded_stdin_paths(stdin.lock()) {
                Ok(mut stdin_paths) => sources.append(&mut stdin_paths),
                Err(err) => {
                    eprintln!("Error reading --stdin: {err}");
                    return ExitCode::from(2);
                }
            }
        }
        let paths = ServicePaths::default_user(parsed.config_path, None);
        // Standalone Advanced Convert window bypasses ControlInstanceLock (`ADR-C14`, `ADR-C15`).
        if let Err(err) = run_advanced_convert_window(sources, paths) {
            eprintln!("Radivine Advanced Convert error: {err}");
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }

    let mut initial_page = ControlPage::Dashboard;
    let mut explicit_page: Option<ControlPage> = None;
    let mut config_override: Option<PathBuf> = None;
    let mut socket_override: Option<PathBuf> = None;
    let mut control_socket_override: Option<PathBuf> = None;
    let mut export_screenshots_dir: Option<PathBuf> = None;
    let mut check_status_json = false;

    let mut args = raw_os_args
        .into_iter()
        .map(|os| os.to_string_lossy().into_owned());
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("radivine-control {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--page" => {
                let Some(val) = args.next() else {
                    eprintln!(
                        "Error: --page requires a value (dashboard|general|wheel|actions|conversion|jobs|diagnostics|about)"
                    );
                    return ExitCode::from(2);
                };
                let Some(parsed) = ControlPage::from_slug(&val) else {
                    eprintln!(
                        "Error: unknown page '{val}'. Expected one of: dashboard, general, wheel, actions, conversion, jobs, diagnostics, about"
                    );
                    return ExitCode::from(2);
                };
                initial_page = parsed;
                explicit_page = Some(parsed);
            }
            "--config" => {
                let Some(val) = args.next() else {
                    eprintln!("Error: --config requires a file path");
                    return ExitCode::from(2);
                };
                config_override = Some(PathBuf::from(val));
            }
            "--socket" => {
                let Some(val) = args.next() else {
                    eprintln!("Error: --socket requires a socket path");
                    return ExitCode::from(2);
                };
                socket_override = Some(PathBuf::from(val));
            }
            "--control-socket" => {
                let Some(val) = args.next() else {
                    eprintln!("Error: --control-socket requires a socket path");
                    return ExitCode::from(2);
                };
                control_socket_override = Some(PathBuf::from(val));
            }
            "--export-screenshots" => {
                let Some(val) = args.next() else {
                    eprintln!("Error: --export-screenshots requires an output directory path");
                    return ExitCode::from(2);
                };
                export_screenshots_dir = Some(PathBuf::from(val));
            }
            "--check-status-json" => {
                check_status_json = true;
            }
            other => {
                eprintln!(
                    "Error: unknown option '{other}'. Run 'radivine-control --help' for usage."
                );
                return ExitCode::from(2);
            }
        }
    }

    let service_paths = ServicePaths::default_user(config_override, socket_override);

    if let Some(out_dir) = export_screenshots_dir {
        match export_all_page_screenshots(&out_dir, service_paths) {
            Ok(paths) => {
                for p in paths {
                    println!("Exported screenshot: {}", p.display());
                }
                return ExitCode::SUCCESS;
            }
            Err(err) => {
                eprintln!("Failed to export screenshots: {err}");
                return ExitCode::FAILURE;
            }
        }
    }

    if check_status_json {
        let ctrl = ServiceController::new(service_paths);
        let snap = ctrl.query_snapshot(None);
        println!(
            "{{\"health\":\"{}\",\"ipc_reachable\":{},\"systemd_available\":{},\"systemd_active\":\"{}\",\"systemd_sub\":\"{}\",\"autostart\":\"{:?}\",\"socket_path\":\"{}\"}}",
            snap.health.label(),
            snap.daemon.is_some(),
            snap.systemd.available,
            snap.systemd.active_state,
            snap.systemd.sub_state,
            snap.autostart,
            snap.socket_path.display(),
        );
        return ExitCode::SUCCESS;
    }

    let control_sock = control_socket_override.unwrap_or_else(default_control_socket_path);
    let instance_lock =
        match ControlInstanceLock::acquire_or_notify_existing(&control_sock, explicit_page) {
            Ok(None) => {
                println!("Focused existing Radivine Control Center window.");
                return ExitCode::SUCCESS;
            }
            Ok(Some(lock)) => Some(lock),
            Err(err) => {
                eprintln!("Warning: could not acquire single-instance lock: {err}");
                None
            }
        };

    if let Err(err) = run_control_window(initial_page, service_paths, instance_lock) {
        eprintln!("Radivine Control Center error: {err}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
