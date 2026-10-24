//! Two-rung atomic output commit protocol, staging directory hygiene, and crash-safe temp cleanup.
//!
//! Normative specification: `docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md` §3–§5 (`ADR-034`).

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustix::fs::{
    AtFlags, Mode, OFlags, RenameFlags, linkat, openat, renameat_with, statat, unlinkat,
};
use serde::{Deserialize, Serialize};

use crate::error::{ConversionError, FsOp};
use crate::format::FormatId;
use crate::options::{CollisionPolicy, NamingPolicy, OutputLocation, OutputPolicy};

/// Captured `(dev, ino, size, mtime)` identity of an open source file (`SECURITY_AND_FILE_SAFETY.md` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileIdentity {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime_sec: i64,
    pub mtime_nsec: i64,
}

impl FileIdentity {
    /// Reads [`FileIdentity`] from an open [`File`] handle (`fstat`).
    pub fn from_file(file: &File) -> io::Result<Self> {
        let meta = file.metadata()?;
        Ok(Self::from_metadata(&meta))
    }

    /// Reads [`FileIdentity`] from [`fs::Metadata`].
    #[must_use]
    pub fn from_metadata(meta: &fs::Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            size: meta.len(),
            mtime_sec: meta.mtime(),
            mtime_nsec: meta.mtime_nsec(),
        }
    }

    /// Returns `true` if `self` and `other` identify the exact same unmodified file state.
    #[must_use]
    pub const fn matches(&self, other: &Self) -> bool {
        self.dev == other.dev
            && self.ino == other.ino
            && self.size == other.size
            && self.mtime_sec == other.mtime_sec
            && self.mtime_nsec == other.mtime_nsec
    }
}

/// Pre-`mkdirat` staging directory intent journal payload (`SECURITY_AND_FILE_SAFETY.md` §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageIntentJournal<'a> {
    pub stage_dir_path: &'a Path,
}

/// Post-`mkdirat` staging directory identity journal payload (`SECURITY_AND_FILE_SAFETY.md` §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageCreatedJournal<'a> {
    pub stage_dir_path: &'a Path,
    pub stage_dir_dev: u64,
    pub stage_dir_ino: u64,
}

/// Newly created `.part` temporary file identity journal payload (`SECURITY_AND_FILE_SAFETY.md` §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TempCreatedJournal<'a> {
    pub stage_dir_path: &'a Path,
    pub temp_path: &'a Path,
    pub temp_dev: u64,
    pub temp_ino: u64,
}

/// Pre-commit request presented to [`crate::engine::ItemControl::acquire_commit_permit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitRequest<'a> {
    pub attempt: u32,
    pub stage_dir_path: &'a Path,
    pub temp_path: &'a Path,
    pub temp_dev: u64,
    pub temp_ino: u64,
    pub final_output_path: &'a Path,
}

impl<'a> CommitRequest<'a> {
    /// Returns the 0-based collision probe attempt number.
    #[must_use]
    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Returns the candidate output filename (`final_name`) in `dest_dir`.
    #[must_use]
    pub fn candidate(&self) -> &'a OsStr {
        self.final_output_path
            .file_name()
            .unwrap_or(self.final_output_path.as_os_str())
    }

    /// Returns the device ID of the staged temporary `.part` file.
    #[must_use]
    pub const fn temp_dev(&self) -> u64 {
        self.temp_dev
    }

    /// Returns the inode number of the staged temporary `.part` file.
    #[must_use]
    pub const fn temp_ino(&self) -> u64 {
        self.temp_ino
    }

    /// Authorizes this request and issues a lifetime-scoped [`CommitPermit`].
    #[must_use]
    pub fn authorize(&self) -> CommitPermit<'a> {
        CommitPermit::issue(self)
    }
}

/// Linear, non-`Send`, non-`Sync`, lifetime-scoped token proving pre-commit journaling succeeded.
#[derive(Debug)]
pub struct CommitPermit<'a> {
    _not_send_sync: PhantomData<*mut ()>,
    _lifetime: PhantomData<&'a ()>,
}

impl<'a> CommitPermit<'a> {
    /// Constructs a [`CommitPermit`] bound to the borrow lifetime `'a` of a [`CommitRequest`].
    ///
    /// Intended to be called only by [`crate::engine::ItemControl::acquire_commit_permit`]
    /// implementations after persisting `Committing` state and verifying non-cancellation.
    #[must_use]
    pub fn issue(_req: &CommitRequest<'a>) -> Self {
        Self {
            _not_send_sync: PhantomData,
            _lifetime: PhantomData,
        }
    }
}

/// Role of a directory being `fsync`ed during commit or cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirSyncRole {
    StageDir,
    DestDir,
}

/// Which rung of the two-rung atomic commit protocol succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitRung {
    RungARenameNoReplace,
    RungBHardlinkUnlink,
}

/// Internal outcome of a single `commit_no_replace` attempt.
#[derive(Debug)]
pub enum CommitAttemptError {
    DestinationExists,
    FilesystemUnsupported(String),
    Io(io::Error),
}

/// Seam over low-level directory syscalls enabling deterministic fault-injection testing (`P8`).
pub trait FsOps: Send + Sync + std::fmt::Debug {
    fn rename_noreplace(
        &self,
        stage_dir: &File,
        temp_name: &OsStr,
        dest_dir: &File,
        dest_name: &OsStr,
    ) -> Result<(), rustix::io::Errno>;

    fn hardlink(
        &self,
        stage_dir: &File,
        temp_name: &OsStr,
        dest_dir: &File,
        dest_name: &OsStr,
    ) -> Result<(), rustix::io::Errno>;

    fn unlink_in_dir(&self, dir: &File, name: &OsStr) -> Result<(), rustix::io::Errno>;

    fn sync_dir(&self, dir: &File, role: DirSyncRole) -> io::Result<()>;
}

/// Production [`FsOps`] implementation backed by Linux `renameat2(RENAME_NOREPLACE)`, `linkat`, `unlinkat`, and `fsync`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealFsOps;

impl FsOps for RealFsOps {
    fn rename_noreplace(
        &self,
        stage_dir: &File,
        temp_name: &OsStr,
        dest_dir: &File,
        dest_name: &OsStr,
    ) -> Result<(), rustix::io::Errno> {
        renameat_with(
            stage_dir,
            temp_name,
            dest_dir,
            dest_name,
            RenameFlags::NOREPLACE,
        )
    }

    fn hardlink(
        &self,
        stage_dir: &File,
        temp_name: &OsStr,
        dest_dir: &File,
        dest_name: &OsStr,
    ) -> Result<(), rustix::io::Errno> {
        linkat(stage_dir, temp_name, dest_dir, dest_name, AtFlags::empty())
    }

    fn unlink_in_dir(&self, dir: &File, name: &OsStr) -> Result<(), rustix::io::Errno> {
        unlinkat(dir, name, AtFlags::empty())
    }

    fn sync_dir(&self, dir: &File, _role: DirSyncRole) -> io::Result<()> {
        dir.sync_all()
    }
}

/// Executes a single two-rung atomic no-replace publication from `(stage_dir, temp_name)` to
/// `(dest_dir, dest_name)` (`SECURITY_AND_FILE_SAFETY.md` §3.1).
///
/// Consumes `_permit` to prove that the caller acquired a [`CommitPermit`] immediately prior to
/// publication.
pub fn commit_no_replace(
    stage_dir: &File,
    temp_name: &OsStr,
    dest_dir: &File,
    dest_name: &OsStr,
    _permit: CommitPermit<'_>,
    fs_ops: &dyn FsOps,
) -> Result<CommitRung, CommitAttemptError> {
    // Rung A: renameat2(..., RENAME_NOREPLACE)
    match fs_ops.rename_noreplace(stage_dir, temp_name, dest_dir, dest_name) {
        Ok(()) => {
            fs_ops
                .sync_dir(stage_dir, DirSyncRole::StageDir)
                .map_err(CommitAttemptError::Io)?;
            fs_ops
                .sync_dir(dest_dir, DirSyncRole::DestDir)
                .map_err(CommitAttemptError::Io)?;
            return Ok(CommitRung::RungARenameNoReplace);
        }
        Err(rustix::io::Errno::EXIST) => {
            return Err(CommitAttemptError::DestinationExists);
        }
        Err(rustix::io::Errno::INVAL | rustix::io::Errno::NOSYS | rustix::io::Errno::OPNOTSUPP) => {
            // Proceed to Rung B
        }
        Err(rustix::io::Errno::XDEV) => {
            return Err(CommitAttemptError::FilesystemUnsupported(
                "cross-device rename (EXDEV)".to_string(),
            ));
        }
        Err(errno) => {
            return Err(CommitAttemptError::Io(io::Error::from_raw_os_error(
                errno.raw_os_error(),
            )));
        }
    }

    // Rung B: linkat (fails with EEXIST if destination exists) -> fsync(dest_dir) -> unlinkat(temp) -> fsync(stage_dir)
    match fs_ops.hardlink(stage_dir, temp_name, dest_dir, dest_name) {
        Ok(()) => {
            fs_ops
                .sync_dir(dest_dir, DirSyncRole::DestDir)
                .map_err(CommitAttemptError::Io)?;
            let _ = fs_ops.unlink_in_dir(stage_dir, temp_name);
            fs_ops
                .sync_dir(stage_dir, DirSyncRole::StageDir)
                .map_err(CommitAttemptError::Io)?;
            Ok(CommitRung::RungBHardlinkUnlink)
        }
        Err(rustix::io::Errno::EXIST) => Err(CommitAttemptError::DestinationExists),
        Err(
            rustix::io::Errno::OPNOTSUPP
            | rustix::io::Errno::NOSYS
            | rustix::io::Errno::PERM
            | rustix::io::Errno::ACCESS
            | rustix::io::Errno::XDEV
            | rustix::io::Errno::MLINK,
        ) => Err(CommitAttemptError::FilesystemUnsupported(
            "filesystem supports neither RENAME_NOREPLACE nor hard links".to_string(),
        )),
        Err(errno) => Err(CommitAttemptError::Io(io::Error::from_raw_os_error(
            errno.raw_os_error(),
        ))),
    }
}

/// Safely unlinks `temp_name` inside `stage_dir` ONLY if its `(st_dev, st_ino)` matches
/// `(expected_dev, expected_ino)` (`SECURITY_AND_FILE_SAFETY.md` §4.2, `T-TEMP-INODE-RACE`).
pub fn cleanup_temp_safely(
    stage_dir: &File,
    temp_name: &OsStr,
    expected_dev: u64,
    expected_ino: u64,
    fs_ops: &dyn FsOps,
) -> io::Result<bool> {
    let stat = match statat(stage_dir, temp_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(s) => s,
        Err(rustix::io::Errno::NOENT) => return Ok(false),
        Err(errno) => return Err(io::Error::from_raw_os_error(errno.raw_os_error())),
    };

    #[allow(clippy::useless_conversion)]
    let dev = u64::from(stat.st_dev);
    #[allow(clippy::useless_conversion)]
    let ino = u64::from(stat.st_ino);
    if dev != expected_dev || ino != expected_ino {
        return Ok(false);
    }

    match fs_ops.unlink_in_dir(stage_dir, temp_name) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => {
            fs_ops.sync_dir(stage_dir, DirSyncRole::StageDir)?;
            Ok(true)
        }
        Err(errno) => Err(io::Error::from_raw_os_error(errno.raw_os_error())),
    }
}

/// Removes `stage_dir_path` if and only if its `(dev, ino)` matches `(expected_dev, expected_ino)`
/// and it is empty (`rmdir` semantics, `T-STAGE-SHARED-DIR`), then `fsync`s the parent directory.
pub fn cleanup_staging_dir_if_empty(
    stage_dir_path: &Path,
    expected_dev: u64,
    expected_ino: u64,
) -> io::Result<bool> {
    let meta = match fs::symlink_metadata(stage_dir_path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if !meta.is_dir() || meta.dev() != expected_dev || meta.ino() != expected_ino {
        return Ok(false);
    }
    match fs::remove_dir(stage_dir_path) {
        Ok(()) => {
            if let Some(parent) = stage_dir_path.parent()
                && let Ok(parent_fd) = File::open(parent)
            {
                let _ = parent_fd.sync_all();
            }
            Ok(true)
        }
        // Directory not empty (e.g. another item or user file is still inside)
        Err(e)
            if e.raw_os_error() == Some(rustix::io::Errno::NOTEMPTY.raw_os_error())
                || e.raw_os_error() == Some(rustix::io::Errno::EXIST.raw_os_error()) =>
        {
            Ok(false)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Generates a 16-hex-character random token for `.rvstage-{job_id}-{rand16}` and `item-{idx}-{rand16}.part`.
#[must_use]
pub fn random_hex16() -> String {
    format!("{:016x}", fastrand::u64(..))
}

/// Validates whether a filename matches the canonical temporary file pattern `item-{idx}-{rand16}.part`.
#[must_use]
pub fn is_canonical_temp_part_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("item-") else {
        return false;
    };
    let Some(middle) = rest.strip_suffix(".part") else {
        return false;
    };
    let Some((idx_str, rand_str)) = middle.rsplit_once('-') else {
        return false;
    };
    !idx_str.is_empty()
        && idx_str.bytes().all(|b| b.is_ascii_digit())
        && rand_str.len() == 16
        && rand_str
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Validates whether a directory name matches the canonical staging directory pattern
/// `.rvstage-{job_id}-{rand16}` (`^\.rvstage-[0-9]+-[0-9a-f]{16}$`).
#[must_use]
pub fn is_canonical_stage_dir_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(".rvstage-") else {
        return false;
    };
    let Some((job_str, rand_str)) = rest.rsplit_once('-') else {
        return false;
    };
    !job_str.is_empty()
        && job_str.bytes().all(|b| b.is_ascii_digit())
        && rand_str.len() == 16
        && rand_str
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Resolves the target directory and base filename `(dest_dir, stem, ext)` for `source_path`,
/// preserving arbitrary non-UTF-8 `OsStr` bytes in the stem (`AC-C2.16`).
pub fn resolve_output_destination_os(
    source_path: &Path,
    target_format: FormatId,
    output_policy: &OutputPolicy,
) -> Result<(PathBuf, OsString, String), ConversionError> {
    let source_parent = source_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let dest_dir = match &output_policy.location {
        OutputLocation::SameDirectory => source_parent.to_path_buf(),
        OutputLocation::Subfolder(sub) => {
            let trimmed = sub.trim();
            if trimmed.is_empty()
                || trimmed == "."
                || trimmed == ".."
                || trimmed.contains('/')
                || trimmed.contains('\\')
                || trimmed.contains('\0')
                || trimmed.contains("..")
            {
                return Err(ConversionError::InvalidOptions {
                    issues: vec![crate::options::OptionIssue {
                        field: "output.location",
                        message: format!("invalid subfolder {sub:?}"),
                        severity: crate::options::Severity::Error,
                    }],
                });
            }
            source_parent.join(trimmed)
        }
        OutputLocation::Directory(dir) => {
            if !dir.is_absolute() {
                return Err(ConversionError::InvalidOptions {
                    issues: vec![crate::options::OptionIssue {
                        field: "output.location",
                        message: format!("output directory must be absolute: {}", dir.display()),
                        severity: crate::options::Severity::Error,
                    }],
                });
            }
            dir.clone()
        }
    };

    let raw_stem: &OsStr = source_path
        .file_stem()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| OsStr::new("converted"));

    let (stem, jpeg_ext) = match &output_policy.naming {
        NamingPolicy::PreserveStem { jpeg_ext } => (raw_stem.to_os_string(), *jpeg_ext),
        NamingPolicy::AppendSuffix { suffix, jpeg_ext } => {
            let mut s = raw_stem.to_os_string();
            s.push(suffix);
            (s, *jpeg_ext)
        }
    };

    let ext = match target_format {
        FormatId::Jpeg => jpeg_ext.as_str().to_string(),
        other => other.descriptor().canonical_ext.to_string(),
    };

    Ok((dest_dir, stem, ext))
}

/// Resolves the target directory and base filename `(dest_dir, stem, ext)` for `source_path`.
pub fn resolve_output_destination(
    source_path: &Path,
    target_format: FormatId,
    output_policy: &OutputPolicy,
) -> Result<(PathBuf, String, String), ConversionError> {
    let (dest_dir, stem_os, ext) =
        resolve_output_destination_os(source_path, target_format, output_policy)?;
    let stem = stem_os.to_str().unwrap_or("converted").to_string();
    Ok((dest_dir, stem, ext))
}

/// Formats a candidate output filename for attempt `attempt` (`0` -> `{stem}.{ext}`, `n >= 1` -> `{stem} ({n}).{ext}`),
/// preserving arbitrary non-UTF-8 `OsStr` bytes in `stem`.
#[must_use]
pub fn derive_suffixed_filename_os(stem: &OsStr, ext: &str, attempt: u32) -> OsString {
    let mut out = stem.to_os_string();
    if attempt == 0 {
        out.push(".");
        out.push(ext);
    } else {
        out.push(format!(" ({attempt}).{ext}"));
    }
    out
}

/// Formats a candidate output filename for attempt `attempt` (`0` -> `{stem}.{ext}`, `n >= 1` -> `{stem} ({n}).{ext}`).
#[must_use]
pub fn derive_suffixed_filename(stem: &str, ext: &str, attempt: u32) -> String {
    if attempt == 0 {
        format!("{stem}.{ext}")
    } else {
        format!("{stem} ({attempt}).{ext}")
    }
}

/// RAII handle for an active staged `.part` file opened `O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC`.
///
/// If dropped before [`PendingOutput::commit`] succeeds, automatically unlinks the `.part` file
/// via [`cleanup_temp_safely`] after verifying `(dev, ino)`.
pub struct PendingOutput {
    stage_dir_fd: File,
    stage_dir_path: PathBuf,
    stage_dir_dev: u64,
    stage_dir_ino: u64,
    dest_dir_fd: File,
    dest_dir_path: PathBuf,
    temp_file: File,
    temp_name: OsString,
    temp_path: PathBuf,
    temp_dev: u64,
    temp_ino: u64,
    committed: bool,
    fs_ops: Arc<dyn FsOps>,
}

impl PendingOutput {
    /// Creates the staging directory (if needed) and opens a fresh `item-{idx}-{rand16}.part`
    /// file in `O_RDWR` mode, recording `StageIntent`, `StageCreated`, and `TempCreated` via
    /// `ctrl` before returning.
    pub fn create(
        dest_dir_path: &Path,
        stage_dir_name: &str,
        item_index: u32,
        ctrl: &mut dyn crate::engine::ItemControl,
        fs_ops: Arc<dyn FsOps>,
    ) -> Result<Self, ConversionError> {
        fs::create_dir_all(dest_dir_path).map_err(|source| ConversionError::Io {
            op: FsOp::CreateStageDir,
            path: dest_dir_path.to_path_buf(),
            source,
        })?;

        let dest_dir_fd = File::open(dest_dir_path).map_err(|source| ConversionError::Io {
            op: FsOp::CreateStageDir,
            path: dest_dir_path.to_path_buf(),
            source,
        })?;

        let stage_dir_path = dest_dir_path.join(stage_dir_name);
        ctrl.record_stage_intent(&StageIntentJournal {
            stage_dir_path: &stage_dir_path,
        })?;

        match OpenOptions::new()
            .read(true)
            .custom_flags(libc_o_directory_nofollow())
            .open(&stage_dir_path)
        {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                rustix::fs::mkdirat(&dest_dir_fd, stage_dir_name, Mode::from_raw_mode(0o700))
                    .or_else(|errno| {
                        if errno == rustix::io::Errno::EXIST {
                            Ok(())
                        } else {
                            Err(ConversionError::Io {
                                op: FsOp::CreateStageDir,
                                path: stage_dir_path.clone(),
                                source: io::Error::from_raw_os_error(errno.raw_os_error()),
                            })
                        }
                    })?;
                fs_ops
                    .sync_dir(&dest_dir_fd, DirSyncRole::DestDir)
                    .map_err(|source| ConversionError::Io {
                        op: FsOp::SyncDir,
                        path: dest_dir_path.to_path_buf(),
                        source,
                    })?;
            }
            Err(source) => {
                return Err(ConversionError::Io {
                    op: FsOp::CreateStageDir,
                    path: stage_dir_path,
                    source,
                });
            }
        }

        let stage_dir_fd = OpenOptions::new()
            .read(true)
            .custom_flags(libc_o_directory_nofollow())
            .open(&stage_dir_path)
            .map_err(|source| ConversionError::Io {
                op: FsOp::CreateStageDir,
                path: stage_dir_path.clone(),
                source,
            })?;

        let stage_meta = stage_dir_fd
            .metadata()
            .map_err(|source| ConversionError::Io {
                op: FsOp::CreateStageDir,
                path: stage_dir_path.clone(),
                source,
            })?;
        if !stage_meta.is_dir() {
            return Err(ConversionError::Io {
                op: FsOp::CreateStageDir,
                path: stage_dir_path,
                source: io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "staging path is not a directory",
                ),
            });
        }
        let stage_dir_dev = stage_meta.dev();
        let stage_dir_ino = stage_meta.ino();

        if let Err(e) = ctrl.record_stage_created(&StageCreatedJournal {
            stage_dir_path: &stage_dir_path,
            stage_dir_dev,
            stage_dir_ino,
        }) {
            if !matches!(
                e,
                ConversionError::LimitExceeded {
                    limit: crate::error::Limit::Timeout { .. }
                }
            ) {
                let _ = cleanup_staging_dir_if_empty(&stage_dir_path, stage_dir_dev, stage_dir_ino);
            }
            return Err(e);
        }

        let temp_name_str = format!("item-{item_index}-{}.part", random_hex16());
        let temp_name = OsString::from(&temp_name_str);
        let temp_path = stage_dir_path.join(&temp_name_str);

        let owned_fd = match openat(
            &stage_dir_fd,
            &temp_name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        ) {
            Ok(fd) => fd,
            Err(errno) => {
                let _ = cleanup_staging_dir_if_empty(&stage_dir_path, stage_dir_dev, stage_dir_ino);
                return Err(ConversionError::Io {
                    op: FsOp::CreateTemp,
                    path: temp_path,
                    source: io::Error::from_raw_os_error(errno.raw_os_error()),
                });
            }
        };

        let temp_file = File::from(owned_fd);
        let temp_meta = match temp_file.metadata() {
            Ok(m) => m,
            Err(source) => {
                let _ = fs_ops.unlink_in_dir(&stage_dir_fd, &temp_name);
                let _ = cleanup_staging_dir_if_empty(&stage_dir_path, stage_dir_dev, stage_dir_ino);
                return Err(ConversionError::Io {
                    op: FsOp::CreateTemp,
                    path: temp_path,
                    source,
                });
            }
        };
        let temp_dev = temp_meta.dev();
        let temp_ino = temp_meta.ino();

        let mut pending = Self {
            stage_dir_fd,
            stage_dir_path,
            stage_dir_dev,
            stage_dir_ino,
            dest_dir_fd,
            dest_dir_path: dest_dir_path.to_path_buf(),
            temp_file,
            temp_name,
            temp_path,
            temp_dev,
            temp_ino,
            committed: false,
            fs_ops,
        };

        if let Err(e) = ctrl.record_temp_created(&TempCreatedJournal {
            stage_dir_path: &pending.stage_dir_path,
            temp_path: &pending.temp_path,
            temp_dev,
            temp_ino,
        }) {
            if matches!(
                e,
                ConversionError::LimitExceeded {
                    limit: crate::error::Limit::Timeout { .. }
                }
            ) {
                pending.disarm_drop_cleanup();
            }
            return Err(e);
        }

        Ok(pending)
    }

    /// Returns a mutable reference to the open `O_RDWR` temporary [`File`].
    pub fn file_mut(&mut self) -> &mut File {
        &mut self.temp_file
    }

    /// Returns an immutable reference to the open `O_RDWR` temporary [`File`].
    #[must_use]
    pub fn file(&self) -> &File {
        &self.temp_file
    }

    /// Returns the path of the temporary `.part` file.
    #[must_use]
    pub fn temp_path(&self) -> &Path {
        &self.temp_path
    }

    /// Returns `(temp_dev, temp_ino)` of the open `.part` file.
    #[must_use]
    pub const fn temp_identity(&self) -> (u64, u64) {
        (self.temp_dev, self.temp_ino)
    }

    /// Returns `(stage_dir_path, stage_dir_dev, stage_dir_ino)`.
    #[must_use]
    pub fn stage_dir_identity(&self) -> (&Path, u64, u64) {
        (&self.stage_dir_path, self.stage_dir_dev, self.stage_dir_ino)
    }

    /// Flushes and `fsync`s the `.part` file, verifies non-zero byte length, and rewinds to offset 0
    /// so the caller can run post-encode validation directly on the open file descriptor (`T-VALIDATE-RDWR`).
    pub fn sync_and_rewind_for_validation(&mut self) -> Result<u64, ConversionError> {
        self.temp_file
            .flush()
            .map_err(|source| ConversionError::Io {
                op: FsOp::WriteTemp,
                path: self.temp_path.clone(),
                source,
            })?;
        self.temp_file
            .sync_all()
            .map_err(|source| ConversionError::Io {
                op: FsOp::SyncTemp,
                path: self.temp_path.clone(),
                source,
            })?;
        let meta = self
            .temp_file
            .metadata()
            .map_err(|source| ConversionError::Io {
                op: FsOp::SyncTemp,
                path: self.temp_path.clone(),
                source,
            })?;
        let len = meta.len();
        if len == 0 {
            return Err(ConversionError::OutputValidationFailed {
                detail: "encoded output file is 0 bytes".to_string(),
            });
        }
        self.temp_file
            .seek(SeekFrom::Start(0))
            .map_err(|source| ConversionError::Io {
                op: FsOp::SyncTemp,
                path: self.temp_path.clone(),
                source,
            })?;
        Ok(len)
    }

    /// Disarms automatic `.part` deletion on `Drop` (used when an item times out so cleanup is
    /// deferred to startup crash recovery after worker recycling per `SECURITY_AND_FILE_SAFETY.md` §3.6).
    pub fn disarm_drop_cleanup(&mut self) {
        self.committed = true;
    }

    /// Atomically publishes the validated `.part` file into `dest_dir_path` according to `output_policy`,
    /// preserving arbitrary non-UTF-8 `OsStr` bytes in `stem` and acquiring a fresh [`CommitPermit`]
    /// from `ctrl` immediately before each `commit_no_replace` call.
    ///
    /// Returns `Ok(Some((final_path, rung, attempt)))` if published, or `Ok(None)` if `CollisionPolicy::Skip`
    /// skipped an existing destination file.
    pub fn commit_with_policy_os(
        mut self,
        stem: &OsStr,
        ext: &str,
        output_policy: &OutputPolicy,
        ctrl: &mut dyn crate::engine::ItemControl,
    ) -> Result<Option<(PathBuf, CommitRung, u32)>, ConversionError> {
        let max_probes = match output_policy.collision {
            CollisionPolicy::Suffix => output_policy.max_collision_probes.max(1),
            CollisionPolicy::Skip | CollisionPolicy::Fail => 0,
        };

        for attempt in 0..=max_probes {
            let candidate_name = derive_suffixed_filename_os(stem, ext, attempt);
            let candidate_path = self.dest_dir_path.join(&candidate_name);

            let req = CommitRequest {
                stage_dir_path: &self.stage_dir_path,
                temp_path: &self.temp_path,
                temp_dev: self.temp_dev,
                temp_ino: self.temp_ino,
                final_output_path: &candidate_path,
                attempt,
            };
            let permit = match ctrl.acquire_commit_permit(&req) {
                Ok(p) => p,
                Err(e) => {
                    if matches!(
                        e,
                        ConversionError::LimitExceeded {
                            limit: crate::error::Limit::Timeout { .. }
                        }
                    ) {
                        self.disarm_drop_cleanup();
                    }
                    return Err(e);
                }
            };

            match commit_no_replace(
                &self.stage_dir_fd,
                &self.temp_name,
                &self.dest_dir_fd,
                &candidate_name,
                permit,
                self.fs_ops.as_ref(),
            ) {
                Ok(rung) => {
                    self.committed = true;
                    if std::env::var("RADIVINE_WORKER_TEST_CRASH_AT")
                        .ok()
                        .as_deref()
                        != Some("C5")
                    {
                        let _ = cleanup_staging_dir_if_empty(
                            &self.stage_dir_path,
                            self.stage_dir_dev,
                            self.stage_dir_ino,
                        );
                    }
                    return Ok(Some((candidate_path, rung, attempt)));
                }
                Err(CommitAttemptError::DestinationExists) => match output_policy.collision {
                    CollisionPolicy::Suffix => continue,
                    CollisionPolicy::Skip => {
                        return Ok(None);
                    }
                    CollisionPolicy::Fail => {
                        return Err(ConversionError::OutputExists {
                            path: candidate_path,
                        });
                    }
                },
                Err(CommitAttemptError::FilesystemUnsupported(detail)) => {
                    return Err(ConversionError::FilesystemUnsupported {
                        path: self.dest_dir_path.clone(),
                        detail,
                    });
                }
                Err(CommitAttemptError::Io(source)) => {
                    return Err(ConversionError::Io {
                        op: FsOp::CommitOutput,
                        path: candidate_path,
                        source,
                    });
                }
            }
        }

        let base = self
            .dest_dir_path
            .join(derive_suffixed_filename_os(stem, ext, 0));
        Err(ConversionError::CollisionExhausted {
            base,
            attempts: max_probes,
        })
    }

    /// Atomically publishes the validated `.part` file into `dest_dir_path` according to `output_policy`,
    /// acquiring a fresh [`CommitPermit`] from `ctrl` immediately before each `commit_no_replace` call.
    ///
    /// Returns `Ok(Some((final_path, rung)))` if published, or `Ok(None)` if `CollisionPolicy::Skip`
    /// skipped an existing destination file.
    pub fn commit_with_policy(
        self,
        stem: &str,
        ext: &str,
        output_policy: &OutputPolicy,
        ctrl: &mut dyn crate::engine::ItemControl,
    ) -> Result<Option<(PathBuf, CommitRung)>, ConversionError> {
        self.commit_with_policy_os(OsStr::new(stem), ext, output_policy, ctrl)
            .map(|opt| opt.map(|(path, rung, _attempt)| (path, rung)))
    }
}

impl Drop for PendingOutput {
    fn drop(&mut self) {
        if !self.committed {
            let _ = cleanup_temp_safely(
                &self.stage_dir_fd,
                &self.temp_name,
                self.temp_dev,
                self.temp_ino,
                self.fs_ops.as_ref(),
            );
            let _ = cleanup_staging_dir_if_empty(
                &self.stage_dir_path,
                self.stage_dir_dev,
                self.stage_dir_ino,
            );
        }
    }
}

const fn libc_o_directory_nofollow() -> i32 {
    // O_DIRECTORY (0o200000 on Linux) | O_NOFOLLOW (0o400000 on Linux) | O_CLOEXEC (0o2000000)
    (OFlags::DIRECTORY.bits() | OFlags::NOFOLLOW.bits() | OFlags::CLOEXEC.bits()) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::NoopControl;
    use std::io::Read;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum RecordedOp {
        RenameNoreplace(String, String),
        Hardlink(String, String),
        UnlinkInDir(String),
        SyncDir(DirSyncRole),
    }

    #[derive(Debug)]
    struct MockFsOps {
        inner: RealFsOps,
        ops: Mutex<Vec<RecordedOp>>,
        fail_rung_a_with: Option<rustix::io::Errno>,
        fail_rung_b_with: Option<rustix::io::Errno>,
    }

    impl MockFsOps {
        fn new(
            fail_rung_a_with: Option<rustix::io::Errno>,
            fail_rung_b_with: Option<rustix::io::Errno>,
        ) -> Self {
            Self {
                inner: RealFsOps,
                ops: Mutex::new(Vec::new()),
                fail_rung_a_with,
                fail_rung_b_with,
            }
        }

        fn recorded(&self) -> Vec<RecordedOp> {
            self.ops.lock().unwrap().clone()
        }
    }

    impl FsOps for MockFsOps {
        fn rename_noreplace(
            &self,
            stage_dir: &File,
            temp_name: &OsStr,
            dest_dir: &File,
            dest_name: &OsStr,
        ) -> Result<(), rustix::io::Errno> {
            self.ops.lock().unwrap().push(RecordedOp::RenameNoreplace(
                temp_name.to_string_lossy().into_owned(),
                dest_name.to_string_lossy().into_owned(),
            ));
            if let Some(err) = self.fail_rung_a_with {
                return Err(err);
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
            self.ops.lock().unwrap().push(RecordedOp::Hardlink(
                temp_name.to_string_lossy().into_owned(),
                dest_name.to_string_lossy().into_owned(),
            ));
            if let Some(err) = self.fail_rung_b_with {
                return Err(err);
            }
            self.inner
                .hardlink(stage_dir, temp_name, dest_dir, dest_name)
        }

        fn unlink_in_dir(&self, dir: &File, name: &OsStr) -> Result<(), rustix::io::Errno> {
            self.ops
                .lock()
                .unwrap()
                .push(RecordedOp::UnlinkInDir(name.to_string_lossy().into_owned()));
            self.inner.unlink_in_dir(dir, name)
        }

        fn sync_dir(&self, dir: &File, role: DirSyncRole) -> io::Result<()> {
            self.ops.lock().unwrap().push(RecordedOp::SyncDir(role));
            self.inner.sync_dir(dir, role)
        }
    }

    #[test]
    fn canonical_temp_part_name_regex_validation() {
        assert!(is_canonical_temp_part_name("item-0-0123456789abcdef.part"));
        assert!(is_canonical_temp_part_name("item-42-deadbeefcafebabe.part"));
        assert!(!is_canonical_temp_part_name(
            ".rvtmp-0-0123456789abcdef.part"
        ));
        assert!(!is_canonical_temp_part_name("item-0-short.part"));
        assert!(!is_canonical_temp_part_name("item-0-0123456789ABCDEF.part"));
        assert!(!is_canonical_temp_part_name("photo.webp"));

        assert!(is_canonical_stage_dir_name(".rvstage-0-0123456789abcdef"));
        assert!(is_canonical_stage_dir_name(".rvstage-42-deadbeefcafebabe"));
        assert!(!is_canonical_stage_dir_name(
            ".rvstage-plan-0123456789abcdef"
        ));
        assert!(!is_canonical_stage_dir_name(".rvstage-42-short"));
        assert!(!is_canonical_stage_dir_name(".rvstage-42-0123456789ABCDEF"));

        use std::os::unix::ffi::OsStrExt;
        let raw_path = Path::new(OsStr::from_bytes(b"/tmp/dir\x80/photo\xffname.png"));
        let (dest, stem, ext) =
            resolve_output_destination_os(raw_path, FormatId::WebP, &OutputPolicy::default())
                .unwrap();
        assert_eq!(dest.as_os_str().as_bytes(), b"/tmp/dir\x80");
        assert_eq!(stem.as_bytes(), b"photo\xffname");
        assert_eq!(ext, "webp");
        let cand0 = derive_suffixed_filename_os(&stem, &ext, 0);
        assert_eq!(cand0.as_bytes(), b"photo\xffname.webp");
        let cand1 = derive_suffixed_filename_os(&stem, &ext, 1);
        assert_eq!(cand1.as_bytes(), b"photo\xffname (1).webp");
    }

    /// `T-COMMIT-NO-PARTIAL` & `T-STAGE-DIR-FSYNC`:
    /// Verify Rung A and Rung B directory fsync ordering, EEXIST suffix probing, and
    /// `FilesystemUnsupported` when both rungs fail with `EOPNOTSUPP`.
    #[test]
    fn t_commit_no_partial_and_t_stage_dir_fsync() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctrl = NoopControl;

        // 1. Rung A normal commit: verify fsync(StageDir) then fsync(DestDir)
        let mock_a = Arc::new(MockFsOps::new(None, None));
        let mut pending = PendingOutput::create(
            tmp.path(),
            ".rvstage-test-0000000000000001",
            0,
            &mut ctrl,
            mock_a.clone(),
        )
        .unwrap();
        pending.file_mut().write_all(b"rung-a-bytes").unwrap();
        pending.sync_and_rewind_for_validation().unwrap();
        let (final_path, rung) = pending
            .commit_with_policy("photo", "webp", &OutputPolicy::default(), &mut ctrl)
            .unwrap()
            .unwrap();
        assert_eq!(rung, CommitRung::RungARenameNoReplace);
        assert_eq!(fs::read(&final_path).unwrap(), b"rung-a-bytes");

        let ops_a = mock_a.recorded();
        let rename_pos = ops_a
            .iter()
            .position(|op| matches!(op, RecordedOp::RenameNoreplace(_, _)))
            .unwrap();
        assert_eq!(
            &ops_a[rename_pos + 1..rename_pos + 3],
            &[
                RecordedOp::SyncDir(DirSyncRole::StageDir),
                RecordedOp::SyncDir(DirSyncRole::DestDir),
            ]
        );

        // 2. Rung B fallback when Rung A returns EINVAL: verify Hardlink -> SyncDir(DestDir) -> UnlinkInDir -> SyncDir(StageDir)
        let mock_b = Arc::new(MockFsOps::new(Some(rustix::io::Errno::INVAL), None));
        let mut pending_b = PendingOutput::create(
            tmp.path(),
            ".rvstage-test-0000000000000002",
            1,
            &mut ctrl,
            mock_b.clone(),
        )
        .unwrap();
        pending_b.file_mut().write_all(b"rung-b-bytes").unwrap();
        pending_b.sync_and_rewind_for_validation().unwrap();
        // Notice "photo.webp" already exists from step 1! So Suffix policy will probe "photo.webp" (EEXIST) and commit to "photo (1).webp"!
        let (final_b, rung_b) = pending_b
            .commit_with_policy("photo", "webp", &OutputPolicy::default(), &mut ctrl)
            .unwrap()
            .unwrap();
        assert_eq!(rung_b, CommitRung::RungBHardlinkUnlink);
        assert_eq!(final_b, tmp.path().join("photo (1).webp"));
        assert_eq!(fs::read(&final_b).unwrap(), b"rung-b-bytes");
        // Original photo.webp untouched!
        assert_eq!(fs::read(&final_path).unwrap(), b"rung-a-bytes");

        let ops_b = mock_b.recorded();
        let link_pos = ops_b
            .iter()
            .rposition(|op| matches!(op, RecordedOp::Hardlink(_, _)))
            .unwrap();
        assert_eq!(
            ops_b[link_pos + 1],
            RecordedOp::SyncDir(DirSyncRole::DestDir)
        );
        assert!(matches!(ops_b[link_pos + 2], RecordedOp::UnlinkInDir(_)));
        assert_eq!(
            ops_b[link_pos + 3],
            RecordedOp::SyncDir(DirSyncRole::StageDir)
        );

        // 3. Both rungs unsupported (EINVAL + EOPNOTSUPP) -> FilesystemUnsupported and staging .part cleaned up!
        let mock_unsup = Arc::new(MockFsOps::new(
            Some(rustix::io::Errno::INVAL),
            Some(rustix::io::Errno::OPNOTSUPP),
        ));
        let mut pending_u = PendingOutput::create(
            tmp.path(),
            ".rvstage-test-0000000000000003",
            2,
            &mut ctrl,
            mock_unsup,
        )
        .unwrap();
        let temp_u_path = pending_u.temp_path().to_path_buf();
        pending_u.file_mut().write_all(b"unsup-bytes").unwrap();
        pending_u.sync_and_rewind_for_validation().unwrap();
        let err = pending_u
            .commit_with_policy("unsup", "webp", &OutputPolicy::default(), &mut ctrl)
            .unwrap_err();
        assert!(matches!(err, ConversionError::FilesystemUnsupported { .. }));
        assert!(
            !temp_u_path.exists(),
            "temp .part must be unlinked on failure"
        );
        assert!(!tmp.path().join("unsup.webp").exists());
    }

    /// `T-VALIDATE-RDWR`:
    /// Verify `PendingOutput` opens `.part` with `O_RDWR`, rewinds to offset 0 after `sync_all`,
    /// and can be read back directly from the same file descriptor without re-opening by path.
    #[test]
    fn t_validate_rdwr() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctrl = NoopControl;
        let mut pending = PendingOutput::create(
            tmp.path(),
            ".rvstage-rdwr-0000000000000001",
            0,
            &mut ctrl,
            Arc::new(RealFsOps),
        )
        .unwrap();
        pending
            .file_mut()
            .write_all(b"\x89PNG\r\n\x1a\nhello")
            .unwrap();
        let len = pending.sync_and_rewind_for_validation().unwrap();
        assert_eq!(len, 13);

        let mut read_back = Vec::new();
        pending.file_mut().read_to_end(&mut read_back).unwrap();
        assert_eq!(read_back, b"\x89PNG\r\n\x1a\nhello");
    }

    /// `T-TEMP-INODE-RACE`:
    /// Verify `cleanup_temp_safely` refuses to unlink if the filename was replaced by a different inode.
    #[test]
    fn t_temp_inode_race() {
        let tmp = tempfile::tempdir().unwrap();
        let stage_dir_fd = File::open(tmp.path()).unwrap();
        let path = tmp.path().join("item-0-0123456789abcdef.part");
        fs::write(&path, b"original").unwrap();
        let orig_meta = fs::metadata(&path).unwrap();
        let orig_dev = orig_meta.dev();
        let orig_ino = orig_meta.ino();

        // Replace with a new file (different inode)
        fs::remove_file(&path).unwrap();
        let replacement = tmp.path().join("other.tmp");
        fs::write(&replacement, b"keep-me-alive-different-inode").unwrap();
        // Hold original inode number in another file so the filesystem cannot reuse orig_ino
        let _holder = File::create(tmp.path().join("inode-holder")).unwrap();
        fs::write(&path, b"impostor").unwrap();
        let new_meta = fs::metadata(&path).unwrap();
        if new_meta.ino() != orig_ino {
            let removed = cleanup_temp_safely(
                &stage_dir_fd,
                OsStr::new("item-0-0123456789abcdef.part"),
                orig_dev,
                orig_ino,
                &RealFsOps,
            )
            .unwrap();
            assert!(!removed, "must not unlink when inode does not match");
            assert!(path.exists(), "replacement file must remain untouched");
        }
    }

    /// `T-STAGE-SHARED-DIR`:
    /// Verify `cleanup_staging_dir_if_empty` leaves a staging directory intact if another file
    /// resides inside it, and removes it once empty.
    #[test]
    fn t_stage_shared_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let stage = tmp.path().join(".rvstage-shared-0000000000000001");
        fs::create_dir(&stage).unwrap();
        let meta = fs::metadata(&stage).unwrap();
        let other_file = stage.join("user-or-sibling.txt");
        fs::write(&other_file, b"do not delete").unwrap();

        let removed_nonempty =
            cleanup_staging_dir_if_empty(&stage, meta.dev(), meta.ino()).unwrap();
        assert!(!removed_nonempty);
        assert!(other_file.exists());

        fs::remove_file(&other_file).unwrap();
        let removed_empty = cleanup_staging_dir_if_empty(&stage, meta.dev(), meta.ino()).unwrap();
        assert!(removed_empty);
        assert!(!stage.exists());
    }

    /// `AC-C1.9`: Property-based verification (`>= 500` cases) of `INV-SRC` (source never modified),
    /// `INV-NOOVERWRITE` (existing files never overwritten), and `INV-NOTEMP` (no `.rvstage-*` or `.part`
    /// files remain after commit or rollback) under fault injection (`ENOSPC`, `EIO`, `EINVAL`, `EPERM`,
    /// cancellation, and simulated panic unwind).
    #[test]
    fn proptest_inv_src_nooverwrite_notemp_500_cases() {
        use proptest::prelude::*;
        use proptest::test_runner::{Config as ProptestConfig, TestRunner};

        let mut runner = TestRunner::new(ProptestConfig {
            cases: 512,
            ..ProptestConfig::default()
        });

        runner
            .run(
                &(0u8..8, 0u8..3, any::<bool>(), 1usize..64),
                |(fault_mode, collision_idx, pre_create_target, payload_len)| {
                    let tmp = tempfile::tempdir().unwrap();
                    let src_path = tmp.path().join("photo.png");
                    let src_bytes = b"immutable-source-bytes-v1";
                    fs::write(&src_path, src_bytes).unwrap();
                    let src_meta_before = fs::metadata(&src_path).unwrap();

                    let pre_existing_path = tmp.path().join("photo.webp");
                    let pre_existing_bytes = b"pre-existing-precious-user-file";
                    if pre_create_target {
                        fs::write(&pre_existing_path, pre_existing_bytes).unwrap();
                    }

                    let collision = match collision_idx {
                        0 => CollisionPolicy::Suffix,
                        1 => CollisionPolicy::Skip,
                        _ => CollisionPolicy::Fail,
                    };
                    let policy = OutputPolicy {
                        collision,
                        ..OutputPolicy::default()
                    };

                    let fs_ops: Arc<dyn FsOps> = match fault_mode {
                        // 0: normal Rung A
                        0 => Arc::new(MockFsOps::new(None, None)),
                        // 1: Rung A EINVAL -> Rung B fallback
                        1 => Arc::new(MockFsOps::new(Some(rustix::io::Errno::INVAL), None)),
                        // 2: Both Rung A (EINVAL) and Rung B (EPERM) unsupported
                        2 => Arc::new(MockFsOps::new(
                            Some(rustix::io::Errno::INVAL),
                            Some(rustix::io::Errno::PERM),
                        )),
                        // 3: ENOSPC during Rung A
                        3 => Arc::new(MockFsOps::new(Some(rustix::io::Errno::NOSPC), None)),
                        // 4: EIO during Rung A
                        4 => Arc::new(MockFsOps::new(Some(rustix::io::Errno::IO), None)),
                        _ => Arc::new(MockFsOps::new(None, None)),
                    };

                    let mut ctrl = NoopControl;
                    let stage_name = format!(".rvstage-prop-{:016x}", random_hex16_u64());
                    let pending_res =
                        PendingOutput::create(tmp.path(), &stage_name, 0, &mut ctrl, fs_ops);

                    if let Ok(mut pending) = pending_res {
                        let payload = vec![0xABu8; payload_len];
                        let _ = pending.file_mut().write_all(&payload);

                        match fault_mode {
                            // 5: Cancel before commit (drop PendingOutput directly)
                            5 => {
                                drop(pending);
                            }
                            // 6: Simulated panic unwind while PendingOutput is live
                            6 => {
                                let _ =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        let _held = pending;
                                        panic!("simulated mid-encode panic");
                                    }));
                            }
                            // Other modes: validate and attempt commit
                            _ => {
                                if pending.sync_and_rewind_for_validation().is_ok() {
                                    let _ = pending
                                        .commit_with_policy("photo", "webp", &policy, &mut ctrl);
                                }
                            }
                        }
                    }

                    // 1. INV-SRC: source file bytes and inode untouched
                    prop_assert_eq!(fs::read(&src_path).unwrap(), src_bytes);
                    let src_meta_after = fs::metadata(&src_path).unwrap();
                    prop_assert_eq!(src_meta_before.ino(), src_meta_after.ino());

                    // 2. INV-NOOVERWRITE: pre-existing destination file never overwritten
                    if pre_create_target {
                        prop_assert_eq!(fs::read(&pre_existing_path).unwrap(), pre_existing_bytes);
                    }

                    // 3. INV-NOTEMP: zero `.rvstage-*` directories or `.part` files left behind
                    for entry in fs::read_dir(tmp.path()).unwrap() {
                        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                        prop_assert!(
                            !name.starts_with(".rvstage-") && !name.ends_with(".part"),
                            "leftover staging artifact: {name}"
                        );
                    }

                    Ok(())
                },
            )
            .unwrap();
    }

    #[test]
    fn pending_output_create_cancellation_cleans_up_stage_and_part() {
        struct CancelDuringCreateControl {
            cancel_on_stage_created: bool,
            cancel_on_temp_created: bool,
        }

        impl crate::engine::ItemControl for CancelDuringCreateControl {
            fn check_cancelled(&self) -> Result<(), ConversionError> {
                Ok(())
            }
            fn on_stage(&mut self, _stage: crate::engine::Stage) {}
            fn reserve(
                &mut self,
                _estimate: crate::engine::ResourceEstimate,
            ) -> Result<crate::engine::Reservation, ConversionError> {
                Ok(crate::engine::Reservation::empty())
            }
            fn record_stage_intent(
                &mut self,
                _stage: &StageIntentJournal<'_>,
            ) -> Result<(), ConversionError> {
                Ok(())
            }
            fn record_stage_created(
                &mut self,
                _stage: &StageCreatedJournal<'_>,
            ) -> Result<(), ConversionError> {
                if self.cancel_on_stage_created {
                    return Err(ConversionError::Cancelled);
                }
                Ok(())
            }
            fn record_temp_created(
                &mut self,
                _temp: &TempCreatedJournal<'_>,
            ) -> Result<(), ConversionError> {
                if self.cancel_on_temp_created {
                    return Err(ConversionError::Cancelled);
                }
                Ok(())
            }
            fn acquire_commit_permit<'a>(
                &mut self,
                req: &CommitRequest<'a>,
            ) -> Result<CommitPermit<'a>, ConversionError> {
                Ok(CommitPermit::issue(req))
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let fs_ops: Arc<dyn FsOps> = Arc::new(RealFsOps);

        // 1. Cancel right after stage_dir creation (`record_stage_created`)
        let mut ctrl1 = CancelDuringCreateControl {
            cancel_on_stage_created: true,
            cancel_on_temp_created: false,
        };
        let stage1 = format!(".rvstage-1-{}", random_hex16());
        let res1 = PendingOutput::create(tmp.path(), &stage1, 0, &mut ctrl1, fs_ops.clone());
        assert!(matches!(res1, Err(ConversionError::Cancelled)));
        assert!(!tmp.path().join(&stage1).exists());

        // 2. Cancel right after .part creation (`record_temp_created`)
        let mut ctrl2 = CancelDuringCreateControl {
            cancel_on_stage_created: false,
            cancel_on_temp_created: true,
        };
        let stage2 = format!(".rvstage-2-{}", random_hex16());
        let res2 = PendingOutput::create(tmp.path(), &stage2, 0, &mut ctrl2, fs_ops);
        assert!(matches!(res2, Err(ConversionError::Cancelled)));
        assert!(!tmp.path().join(&stage2).exists());
    }

    fn random_hex16_u64() -> u64 {
        u64::from_str_radix(&random_hex16(), 16).unwrap_or(1)
    }
}
