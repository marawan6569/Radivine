//! Deterministic crash-injection and filesystem fault-injection hooks for `C0a`–`C6` testing.
//!
//! Controlled by the `RADIVINE_WORKER_TEST_CRASH_AT` environment variable (used only in isolated
//! integration tests; never set in production).

use std::env;
use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use radivine_convert::{DirSyncRole, FsOps, RealFsOps};

static CRASH_ARMED: AtomicBool = AtomicBool::new(true);

/// Named crash-injection points corresponding to the `C0a`–`C6` recovery matrix (`JOB_SYSTEM_DESIGN.md` §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashPoint {
    /// After `C0a` (`stage_dir_path` committed to SQLite), before `mkdirat(stage_dir)`.
    C0a,
    /// After `mkdirat(stage_dir)` and `fsync(dest_dir)`, before `C0b` (`stage_dev, stage_ino`) SQLite commit.
    C0b,
    /// After `openat(.part)` creates the `.part` file, before `C1` (`temp_path, temp_dev, temp_ino`) SQLite commit.
    C1Unjournaled,
    /// After `C1` (`temp_path, temp_dev, temp_ino`) SQLite commit, while item is `Running`.
    C1Journaled,
    /// After `C2` (`state='committing'`, candidate `output_path`) SQLite commit, before `commit_no_replace`.
    C2,
    /// Inside Rung B after `linkat(temp -> output)` + `fsync(dest_dir)`, before `unlinkat(temp)`.
    C3,
    /// Immediately after `commit_no_replace` publishes the final output and unlinks `.part`, before `Succeeded` SQLite commit.
    C4,
    /// After `Succeeded` SQLite commit, before `cleanup_staging_dir_if_empty` removes `.rvstage-*`.
    C5,
    /// After item `Succeeded` SQLite commit and `.rvstage-*` cleanup, before parent `jobs.state` transitions from `running` to terminal.
    C6,
}

impl CrashPoint {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::C0a => "C0a",
            Self::C0b => "C0b",
            Self::C1Unjournaled => "C1_unjournaled",
            Self::C1Journaled => "C1",
            Self::C2 => "C2",
            Self::C3 => "C3",
            Self::C4 => "C4",
            Self::C5 => "C5",
            Self::C6 => "C6",
        }
    }

    #[must_use]
    pub fn matches_env(self) -> bool {
        let Ok(val) = env::var("RADIVINE_WORKER_TEST_CRASH_AT") else {
            return false;
        };
        let trimmed = val.trim();
        match self {
            Self::C1Journaled => trimmed == "C1" || trimmed == "C1_journaled",
            other => trimmed == other.as_str(),
        }
    }
}

/// Immediately aborts the worker process (`std::process::abort()`) if `RADIVINE_WORKER_TEST_CRASH_AT`
/// matches `point`.
pub fn maybe_crash_at(point: CrashPoint) {
    if CRASH_ARMED.load(Ordering::Relaxed) && point.matches_env() {
        std::process::abort();
    }
}

/// [`FsOps`] wrapper that supports `C3`/`C4`/`C5` crash injection and forced Rung B fallback (`RADIVINE_WORKER_TEST_FORCE_RUNG_B=1`).
#[derive(Debug, Default)]
pub struct WorkerFsOps {
    inner: RealFsOps,
}

impl WorkerFsOps {
    #[must_use]
    pub fn new() -> Self {
        Self { inner: RealFsOps }
    }

    fn force_rung_b() -> bool {
        env::var("RADIVINE_WORKER_TEST_FORCE_RUNG_B")
            .ok()
            .is_some_and(|v| v.trim() == "1" || v == "true")
            || CrashPoint::C3.matches_env()
    }
}

impl FsOps for WorkerFsOps {
    fn rename_noreplace(
        &self,
        stage_dir: &File,
        temp_name: &OsStr,
        dest_dir: &File,
        dest_name: &OsStr,
    ) -> Result<(), rustix::io::Errno> {
        if Self::force_rung_b() {
            return Err(rustix::io::Errno::INVAL);
        }
        self.inner
            .rename_noreplace(stage_dir, temp_name, dest_dir, dest_name)
    }

    fn hardlink(
        &self,
        stage_dir: &File,
        temp_name: &OsStr,
        dest_dir: &File,
        dest_name: &OsStr,
    ) -> Result<(), rustix::io::Errno> {
        self.inner
            .hardlink(stage_dir, temp_name, dest_dir, dest_name)
    }

    fn unlink_in_dir(&self, dir: &File, name: &OsStr) -> Result<(), rustix::io::Errno> {
        // In Rung B, hardlink + sync_dir(DestDir) have just completed before unlink_in_dir(stage_dir, temp_name).
        maybe_crash_at(CrashPoint::C3);
        self.inner.unlink_in_dir(dir, name)
    }

    fn sync_dir(&self, dir: &File, role: DirSyncRole) -> io::Result<()> {
        self.inner.sync_dir(dir, role)?;
        // In Rung A, DestDir is synced last inside commit_no_replace.
        // In Rung B, StageDir is synced last inside commit_no_replace (after unlink_in_dir).
        if (role == DirSyncRole::DestDir && !Self::force_rung_b())
            || (role == DirSyncRole::StageDir && Self::force_rung_b())
        {
            maybe_crash_at(CrashPoint::C4);
        }
        Ok(())
    }
}
