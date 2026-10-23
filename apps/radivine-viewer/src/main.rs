//! `radivine-viewer` binary entrypoint (`ACTION_SYSTEM.md` §8.1).
//!
//! Usage:
//! `radivine-viewer --mode <full|head|tail> [--lines N] [--title TITLE] [--theme <dark|light|auto>] -- <PATH>`
//! Exit codes:
//! - `0`: normal close
//! - `2`: CLI usage error
//! - `3`: cannot open file (missing, broken symlink, permission denied, directory, special file, or binary file)

#![forbid(unsafe_code)]

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use radivine_viewer::file_io::{EXIT_CANNOT_OPEN, EXIT_OK, EXIT_USAGE, OpenedTextFile};
use radivine_viewer::head_tail::{read_head, read_tail};
use radivine_viewer::ui::{ViewerApp, run_viewer_window};
use radivine_viewer::{PreviewMode, ViewerTheme};

/// Radivine read-only file preview window (`Full`, `Head`, `Tail`).
#[derive(Debug, Parser)]
#[command(
    name = "radivine-viewer",
    version,
    about = "Read-only file preview window for Radivine"
)]
pub struct ViewerCli {
    /// Preview mode (`full`, `head`, or `tail`).
    #[arg(long, value_enum)]
    pub mode: PreviewMode,

    /// Number of lines to display in `head` or `tail` mode (default: 50).
    #[arg(long, default_value_t = 50)]
    pub lines: usize,

    /// Optional custom window title override.
    #[arg(long)]
    pub title: Option<String>,

    /// Color theme (`dark`, `light`, or `auto`).
    #[arg(long, value_enum, default_value_t = ViewerTheme::Dark)]
    pub theme: ViewerTheme,

    /// Print the preview output to stdout and exit without opening a GUI window (for CLI/testing).
    #[arg(long)]
    pub print: bool,

    /// Target file path to preview.
    #[arg(value_name = "PATH")]
    pub path: PathBuf,
}

fn main() -> ExitCode {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let cli = ViewerCli::parse();
    if cli.lines == 0 {
        eprintln!("radivine-viewer: --lines must be >= 1");
        return ExitCode::from(EXIT_USAGE);
    }

    let opened = match OpenedTextFile::open(&cli.path) {
        Ok(f) => f,
        Err(err) => {
            eprintln!("radivine-viewer: {err}");
            return ExitCode::from(EXIT_CANNOT_OPEN);
        }
    };

    tracing::info!(
        mode = %cli.mode.as_str(),
        path = %opened.path.display(),
        size = opened.initial_size,
        encoding = %opened.encoding.label(),
        "opened file for preview"
    );

    let headless_env = std::env::var("RADIVINE_VIEWER_HEADLESS")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

    if cli.print || headless_env {
        if cli.print {
            match cli.mode {
                PreviewMode::Head => match read_head(&opened, cli.lines) {
                    Ok(res) => {
                        let _ = std::io::stdout().write_all(res.raw_output.as_bytes());
                    }
                    Err(err) => {
                        eprintln!("radivine-viewer: {err}");
                        return ExitCode::from(EXIT_CANNOT_OPEN);
                    }
                },
                PreviewMode::Tail => match read_tail(&opened, cli.lines) {
                    Ok(res) => {
                        let _ = std::io::stdout().write_all(res.raw_output.as_bytes());
                    }
                    Err(err) => {
                        eprintln!("radivine-viewer: {err}");
                        return ExitCode::from(EXIT_CANNOT_OPEN);
                    }
                },
                PreviewMode::Full => {
                    if let Err(err) = ViewerApp::new(opened, cli.mode, cli.lines, cli.theme, None) {
                        eprintln!("radivine-viewer: {err}");
                        return ExitCode::from(EXIT_CANNOT_OPEN);
                    }
                    return ExitCode::from(EXIT_OK);
                }
            }
        }
        // Also initialize `ViewerApp` so `RADIVINE_VIEWER_DUMP_PATH` is populated if set
        if std::env::var("RADIVINE_VIEWER_DUMP_PATH").is_ok()
            && let Err(err) = ViewerApp::new(opened, cli.mode, cli.lines, cli.theme, None)
        {
            eprintln!("radivine-viewer: {err}");
            return ExitCode::from(EXIT_CANNOT_OPEN);
        }
        return ExitCode::from(EXIT_OK);
    }

    match run_viewer_window(opened, cli.mode, cli.lines, cli.title, cli.theme) {
        Ok(()) => ExitCode::from(EXIT_OK),
        Err(err) => {
            eprintln!("radivine-viewer window error: {err}");
            ExitCode::from(EXIT_CANNOT_OPEN)
        }
    }
}
