//! `radivine-control` — Native graphical Control Center, background service manager, and
//! format-preserving settings editor for Radivine (`CONTROL_CENTER_DESIGN.md`).

#![forbid(unsafe_code)]

pub mod diagnostics;
pub mod persistence;
pub mod screenshot;
pub mod service;
pub mod single_instance;
pub mod ui;

use clap::ValueEnum;

/// Navigation pages in the Radivine Control Center sidebar (`Section 2`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ControlPage {
    /// Service status, Start/Stop/Restart controls, and runtime telemetry.
    #[default]
    Dashboard,
    /// Login autostart, theme/scale appearance, notifications, preview lines, and config management.
    General,
    /// Branching radial wheel radii, dwell/grace timings, hysteresis, trigger key, and input safety.
    Wheel,
    /// Graphical Custom Actions Manager (`~/.config/radivine/actions.toml`).
    Actions,
    /// Conversion settings, default presets, output rules, metadata policies, and limits (`~/.config/radivine/convert.toml`).
    Conversion,
    /// Active and historical conversion jobs backed by `radivine-worker`.
    Jobs,
    /// `radivine doctor` diagnostics, compatibility notes, bounded log viewer, and redacted export.
    Diagnostics,
    /// Version, architecture overview, dual license (`MIT OR Apache-2.0`), and third-party notices.
    About,
}

impl ControlPage {
    /// All pages in sidebar display order.
    pub const ALL: [Self; 8] = [
        Self::Dashboard,
        Self::General,
        Self::Wheel,
        Self::Actions,
        Self::Conversion,
        Self::Jobs,
        Self::Diagnostics,
        Self::About,
    ];

    /// Canonical CLI/IPC slug (`"dashboard"`, `"general"`, etc.).
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
            Self::General => "general",
            Self::Wheel => "wheel",
            Self::Actions => "actions",
            Self::Conversion => "conversion",
            Self::Jobs => "jobs",
            Self::Diagnostics => "diagnostics",
            Self::About => "about",
        }
    }

    /// Parses a page slug (`"dashboard"`, `"general"`, `"wheel"`, `"actions"`, `"conversion"`, `"jobs"`, `"diagnostics"`, `"about"`).
    #[must_use]
    pub fn from_slug(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dashboard" => Some(Self::Dashboard),
            "general" => Some(Self::General),
            "wheel" => Some(Self::Wheel),
            "actions" => Some(Self::Actions),
            "conversion" => Some(Self::Conversion),
            "jobs" => Some(Self::Jobs),
            "diagnostics" => Some(Self::Diagnostics),
            "about" => Some(Self::About),
            _ => None,
        }
    }

    /// Human-readable sidebar title.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::General => "General",
            Self::Wheel => "Wheel",
            Self::Actions => "Custom Actions",
            Self::Conversion => "Conversion",
            Self::Jobs => "Jobs",
            Self::Diagnostics => "Diagnostics & Logs",
            Self::About => "About Radivine",
        }
    }

    /// Short subtitle shown in the page header.
    #[must_use]
    pub const fn subtitle(self) -> &'static str {
        match self {
            Self::Dashboard => {
                "Monitor background daemon health, runtime telemetry, and lifecycle controls."
            }
            Self::General => {
                "Configure login startup, appearance theme, notifications, and file preview defaults."
            }
            Self::Wheel => {
                "Fine-tune radial wheel radii, submenu timings, trigger modifier, and input safety."
            }
            Self::Actions => {
                "Create, edit, and validate shell-free custom actions in actions.toml."
            }
            Self::Conversion => {
                "Configure default conversion presets, output rules, metadata policies, and limits."
            }
            Self::Jobs => {
                "Monitor active file conversion jobs, inspect per-item outcomes, and manage history."
            }
            Self::Diagnostics => {
                "Inspect X11 desktop compatibility, service status, and recent daemon logs."
            }
            Self::About => {
                "Branch into action — fast, extensible, context-aware radial action wheel."
            }
        }
    }
}
