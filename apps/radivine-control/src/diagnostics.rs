//! Diagnostics probe, bounded log reader (`journalctl --user` + `daemon.log`), and privacy-conscious
//! diagnostic report exporter (`CONTROL_CENTER_DESIGN.md` §2.4, §5).

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};

use radivine_platform::x11::doctor::{DoctorReport, probe_x11_doctor};

use crate::service::{DaemonHealthState, ServiceSnapshot};

/// Maximum number of log lines retained in memory for the Diagnostics log viewer.
const MAX_LOG_LINES: usize = 200;

/// Maximum byte slice read from the tail of `daemon.log` (`64 KiB`).
const MAX_LOG_READ_BYTES: u64 = 65_536;

/// Visual log severity level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogSeverity {
    /// Informational message (`INFO`).
    Info,
    /// Warning message (`WARN`).
    Warn,
    /// Error message (`ERROR`).
    Error,
    /// Debug or trace message (`DEBUG` / `TRACE`).
    Debug,
}

impl LogSeverity {
    /// Short badge label (`"INFO"`, `"WARN"`, `"ERROR"`, `"DEBUG"`).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Debug => "DEBUG",
        }
    }
}

/// Structured single log entry for the Diagnostics log viewer.
#[derive(Clone, Debug)]
pub struct LogEntry {
    /// Timestamp or prefix column, if present.
    pub timestamp: String,
    /// Parsed severity level.
    pub severity: LogSeverity,
    /// Log message body.
    pub message: String,
    /// Original raw line.
    pub raw: String,
}

/// Complete diagnostics and logs snapshot for the Diagnostics page.
#[derive(Clone, Debug)]
pub struct DiagnosticsSnapshot {
    /// Probed X11 doctor report, or error description if X11 is unavailable/unset.
    pub doctor: Result<DoctorReport, String>,
    ///Formatted `radivine doctor` text output.
    pub doctor_text: String,
    /// Source of retrieved log entries (`"systemd user journal (radivine.service)"`, etc.).
    pub log_source: String,
    /// Bounded recent log entries (up to 200 lines).
    pub logs: Vec<LogEntry>,
    /// Compatibility notes for the current desktop environment.
    pub compatibility_notes: Vec<String>,
}

/// Collects the [`DiagnosticsSnapshot`] on the background worker thread.
///
/// Only connects to X11 for `probe_x11_doctor` when `allow_x11_probe` is `true` and `DISPLAY` is non-empty.
#[must_use]
pub fn collect_diagnostics_snapshot(
    service: &ServiceSnapshot,
    daemon_log_path: &Path,
    allow_x11_probe: bool,
) -> DiagnosticsSnapshot {
    let display_set = std::env::var("DISPLAY").is_ok_and(|s| !s.trim().is_empty());
    let doctor = if allow_x11_probe && display_set {
        probe_x11_doctor().map_err(|e| e.to_string())
    } else if !display_set {
        Err("DISPLAY is not set (running in headless or non-X11 environment).".to_string())
    } else {
        Err("X11 probe skipped in isolated test mode.".to_string())
    };

    let doctor_text = match &doctor {
        Ok(rep) => rep.to_string(),
        Err(err) => format!("X11 Doctor Probe: {err}"),
    };

    let (log_source, raw_lines) = retrieve_bounded_logs(service.systemd.available, daemon_log_path);
    let logs = raw_lines
        .iter()
        .map(|line| parse_log_line(line))
        .collect::<Vec<_>>();

    let mut compatibility_notes = Vec::new();
    let is_wayland = service.session_type.eq_ignore_ascii_case("wayland")
        || doctor.as_ref().is_ok_and(DoctorReport::is_wayland_session);

    if is_wayland {
        compatibility_notes.push(
            "Wayland session detected: Global mid-drag 'Drag + Shift' interception requires an X11 desktop session. \
             On Wayland, use 'radivine invoke <FILES>' or Nemo's right-click 'Open in Radivine' action."
                .to_string(),
        );
    } else {
        compatibility_notes.push(
            "X11 session active: Full Nemo 'Drag + Shift' mid-drag interception and 'radivine invoke' are supported."
                .to_string(),
        );
    }

    if let Ok(ref rep) = doctor
        && !rep.compositor_active
    {
        compatibility_notes.push(
            "No X11 compositor detected (_NET_WM_CM_S0 unowned): Radivine automatically uses XShape bounding regions so wheels render cleanly without black rectangular corners."
                .to_string(),
        );
    }

    if !service.autostart.conflicts.is_empty() {
        for c in &service.autostart.conflicts {
            compatibility_notes.push(c.description());
        }
    }

    DiagnosticsSnapshot {
        doctor,
        doctor_text,
        log_source,
        logs,
        compatibility_notes,
    }
}

/// Redacts sensitive personal paths, usernames, hostnames, and file URIs from diagnostic text
/// before copying or exporting for GitHub issues.
#[must_use]
pub fn redact_sensitive_text(input: &str) -> String {
    let mut out = input.to_string();

    if let Ok(home) = std::env::var("HOME")
        && !home.trim().is_empty()
        && home != "/"
    {
        let home_uri = format!("file://{}", home.trim());
        out = out.replace(&home_uri, "file://~");
        out = out.replace(home.trim(), "~");
    }

    for var in ["USER", "LOGNAME"] {
        if let Ok(user) = std::env::var(var)
            && user.trim().len() >= 2
        {
            out = out.replace(&format!("/home/{}", user.trim()), "~");
            out = out.replace(user.trim(), "<USER>");
        }
    }

    if let Ok(host) = fs::read_to_string("/etc/hostname") {
        let h = host.trim();
        if h.len() >= 2 {
            out = out.replace(h, "<HOST>");
        }
    }

    // Redact /run/user/<digits>
    out = redact_run_user_uid(&out);
    // Redact file:///... URIs to avoid leaking personal filenames
    out = redact_file_uris(&out);
    // Redact any remaining /home/<username> path prefixes
    out = redact_home_user_paths(&out);
    out
}

/// Generates a Markdown diagnostic summary suitable for pasting into a GitHub issue.
#[must_use]
pub fn format_diagnostic_report(
    service: &ServiceSnapshot,
    diag: &DiagnosticsSnapshot,
    redact_privacy: bool,
) -> String {
    let mut report = String::with_capacity(4096);
    report.push_str("# Radivine Diagnostic Report\n\n");
    report.push_str(&format!(
        "- **Radivine Version:** `v{}`\n",
        env!("CARGO_PKG_VERSION")
    ));
    report.push_str(&format!(
        "- **Service Health:** `{}` ({})\n",
        service.health.label(),
        service.management_mode
    ));
    report.push_str(&format!(
        "- **Desktop Session:** `{}` (`{}`)\n",
        service.session_type, service.desktop_environment
    ));
    report.push_str(&format!(
        "- **Autostart:** `{}` (enabled={})\n",
        service.autostart.method_label, service.autostart.enabled
    ));
    if let Some(ref d) = service.daemon {
        report.push_str(&format!(
            "- **Daemon Runtime:** `pid={}` · `state={}` · `uptime={}s` · `scale={:.2}` · `rss={} KiB`\n",
            d.pid,
            d.session_state,
            d.uptime_secs,
            d.scale,
            d.rss_kib.unwrap_or(0)
        ));
    } else if service.health != DaemonHealthState::Stopped {
        report.push_str(&format!(
            "- **Last Error:** `{}`\n",
            service.last_error.as_deref().unwrap_or("none")
        ));
    }

    report.push_str("\n## Desktop & X11 Doctor (`radivine doctor`)\n\n```text\n");
    report.push_str(diag.doctor_text.trim());
    report.push_str("\n```\n\n");

    report.push_str(&format!(
        "## Recent Daemon Logs ({}, last {} lines)\n\n```text\n",
        diag.log_source,
        diag.logs.len().min(50)
    ));
    let start = diag.logs.len().saturating_sub(50);
    for entry in &diag.logs[start..] {
        report.push_str(&entry.raw);
        report.push('\n');
    }
    report.push_str("```\n");

    if redact_privacy {
        redact_sensitive_text(&report)
    } else {
        report
    }
}

fn retrieve_bounded_logs(systemd_available: bool, daemon_log_path: &Path) -> (String, Vec<String>) {
    let mut lines = Vec::new();
    let mut source_label = String::new();

    if systemd_available
        && let Ok(output) = Command::new("journalctl")
            .args([
                "--user",
                "-u",
                "radivine.service",
                "-n",
                "150",
                "--no-pager",
                "-o",
                "short-iso",
            ])
            .stdin(Stdio::null())
            .output()
        && output.status.success()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with("-- No entries --") {
                lines.push(trimmed.to_string());
            }
        }
        if !lines.is_empty() {
            source_label = "systemd user journal (radivine.service)".to_string();
        }
    }

    if daemon_log_path.exists()
        && let Ok(mut file) = File::open(daemon_log_path)
        && let Ok(meta) = file.metadata()
    {
        let len = meta.len();
        let seek_pos = len.saturating_sub(MAX_LOG_READ_BYTES);
        let _ = file.seek(SeekFrom::Start(seek_pos));
        let mut buf = Vec::with_capacity((len - seek_pos) as usize);
        if file.take(MAX_LOG_READ_BYTES).read_to_end(&mut buf).is_ok() {
            let text = String::from_utf8_lossy(&buf);
            let mut file_lines: Vec<String> = text
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect();
            if seek_pos > 0 && !file_lines.is_empty() {
                // Drop partial first line when seeking into the middle of the file
                file_lines.remove(0);
            }
            if !file_lines.is_empty() {
                if source_label.is_empty() {
                    source_label = format!("state log ({})", daemon_log_path.display());
                } else {
                    source_label = format!("{source_label} + {}", daemon_log_path.display());
                }
                lines.extend(file_lines);
            }
        }
    }

    if source_label.is_empty() {
        source_label = if systemd_available {
            "systemd user journal (no recent entries)".to_string()
        } else {
            format!("{} (no log file yet)", daemon_log_path.display())
        };
    }

    if lines.len() > MAX_LOG_LINES {
        let drain_count = lines.len() - MAX_LOG_LINES;
        lines.drain(0..drain_count);
    }

    (source_label, lines)
}

fn parse_log_line(raw: &str) -> LogEntry {
    let clean = strip_ansi_escapes(raw);
    let severity =
        if clean.contains(" ERROR ") || clean.contains("error:") || clean.contains("failed") {
            LogSeverity::Error
        } else if clean.contains(" WARN ") || clean.contains("warning:") {
            LogSeverity::Warn
        } else if clean.contains(" DEBUG ") || clean.contains(" TRACE ") {
            LogSeverity::Debug
        } else {
            LogSeverity::Info
        };

    let (timestamp, message) = if let Some((first_tok, rest)) = clean.split_once(' ')
        && (first_tok.contains('T') || first_tok.starts_with("20"))
    {
        (first_tok.to_string(), rest.trim().to_string())
    } else {
        (String::new(), clean.clone())
    };

    LogEntry {
        timestamp,
        severity,
        message,
        raw: clean,
    }
}

fn strip_ansi_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                let _ = chars.next();
                for ch in chars.by_ref() {
                    if ch.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn redact_run_user_uid(s: &str) -> String {
    let needle = "/run/user/";
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find(needle) {
        out.push_str(&rest[..pos + needle.len()]);
        let after = &rest[pos + needle.len()..];
        let digit_len = after.bytes().take_while(u8::is_ascii_digit).count();
        if digit_len > 0 {
            out.push_str("<UID>");
            rest = &after[digit_len..];
        } else {
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn redact_file_uris(s: &str) -> String {
    let needle = "file://";
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find(needle) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + needle.len()..];
        let end = after
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ']' | ','))
            .unwrap_or(after.len());
        out.push_str("file://<REDACTED_PATH>");
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn redact_home_user_paths(s: &str) -> String {
    let needle = "/home/";
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find(needle) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + needle.len()..];
        let user_len = after
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            .count();
        if user_len > 0 {
            out.push('~');
            rest = &after[user_len..];
        } else {
            out.push_str(needle);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_home_paths_uids_and_file_uris() {
        let raw = "Socket at /run/user/1000/radivine/radivine.sock opened file:///home/alice/secret_tax_2026.pdf";
        let redacted = redact_sensitive_text(raw);
        assert!(redacted.contains("/run/user/<UID>/radivine/radivine.sock"));
        assert!(redacted.contains("file://<REDACTED_PATH>"));
        assert!(!redacted.contains("secret_tax_2026.pdf"));
    }
}
