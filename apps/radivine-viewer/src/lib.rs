//! `radivine-viewer` — Read-only file preview engine (`Full`, `Head`, `Tail`) for Radivine (`ACTION_SYSTEM.md` §8.1).
//!
//! Safety guarantees:
//! - `#![forbid(unsafe_code)]`
//! - Never uses `mmap` (prevents `SIGBUS` when files are concurrently truncated on disk).
//! - Opens files with `O_RDONLY | O_NONBLOCK | O_CLOEXEC` and verifies `fstat` is a regular file
//!   before reading (prevents TOCTOU races and hangs on FIFOs/devices/sockets).
//! - Read-only: never modifies source files.

#![forbid(unsafe_code)]

pub mod file_io;
pub mod full;
pub mod head_tail;

use clap::ValueEnum;

/// Preview mode (`--mode <full|head|tail>`, `ACTION_SYSTEM.md` §8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
pub enum PreviewMode {
    /// Interactive full-file preview with sparse line index and virtualized scrolling.
    Full,
    /// First `N` lines of the file (default 50).
    Head,
    /// Last `N` lines of the file via reverse block scanning (default 50).
    Tail,
}

impl PreviewMode {
    /// Short badge label shown in the viewer header.
    #[must_use]
    pub const fn badge_label(self) -> &'static str {
        match self {
            Self::Full => "FULL PREVIEW",
            Self::Head => "HEAD",
            Self::Tail => "TAIL",
        }
    }

    /// Lowercase CLI mode identifier (`"full"`, `"head"`, `"tail"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Head => "head",
            Self::Tail => "tail",
        }
    }
}

/// Appearance theme preference for the viewer window (`--theme <dark|light|auto>`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ViewerTheme {
    /// Radivine dark theme (default).
    #[default]
    Dark,
    /// Radivine light theme.
    Light,
    /// Follow system / default to dark.
    Auto,
}
