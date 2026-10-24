//! Structured error taxonomy and user-facing error summaries for conversion operations.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::backend::{BackendId, NoRoute};
use crate::format::FormatId;
use crate::options::OptionIssue;

/// High-level classification of a [`ConversionError`] for retry, UI, and metrics logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    UserInput,
    Unsupported,
    CorruptInput,
    ResourceLimit,
    OutputConflict,
    Io,
    Cancelled,
    Internal,
}

impl ErrorClass {
    /// Returns the snake_case string representation of this [`ErrorClass`].
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorClass::UserInput => "user_input",
            ErrorClass::Unsupported => "unsupported",
            ErrorClass::CorruptInput => "corrupt_input",
            ErrorClass::ResourceLimit => "resource_limit",
            ErrorClass::OutputConflict => "output_conflict",
            ErrorClass::Io => "io",
            ErrorClass::Cancelled => "cancelled",
            ErrorClass::Internal => "internal",
        }
    }
}

/// Filesystem operation stage where an I/O failure occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsOp {
    OpenSource,
    StatSource,
    ReadSource,
    CreateStageDir,
    CreateTemp,
    WriteTemp,
    SyncTemp,
    CommitOutput,
    SyncDir,
}

impl fmt::Display for FsOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            FsOp::OpenSource => "opening source file",
            FsOp::StatSource => "inspecting source file metadata",
            FsOp::ReadSource => "reading source file",
            FsOp::CreateStageDir => "creating staging directory",
            FsOp::CreateTemp => "creating temporary output file",
            FsOp::WriteTemp => "writing temporary output file",
            FsOp::SyncTemp => "flushing temporary output file to disk",
            FsOp::CommitOutput => "committing output file to destination",
            FsOp::SyncDir => "flushing directory metadata to disk",
        };
        f.write_str(s)
    }
}

/// Specific media feature that is unsupported by the source/target format or backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedFeature {
    Animation,
    ColorSpace(String),
    BitDepth(u8),
    ArithmeticCoding,
    TargetFeature(String),
}

impl fmt::Display for UnsupportedFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnsupportedFeature::Animation => f.write_str("animated images are not supported"),
            UnsupportedFeature::ColorSpace(cs) => write!(f, "unsupported color space: {cs}"),
            UnsupportedFeature::BitDepth(bits) => write!(f, "unsupported bit depth: {bits}-bit"),
            UnsupportedFeature::ArithmeticCoding => {
                f.write_str("arithmetic-coded JPEG is not supported")
            }
            UnsupportedFeature::TargetFeature(feat) => {
                write!(f, "unsupported target feature: {feat}")
            }
        }
    }
}

/// Resource limit exceeded before or during conversion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Limit {
    Dimensions { width: u32, height: u32, max: u32 },
    PixelCount { pixels: u64, max: u64 },
    MemoryBytes { needed: u64, max: u64 },
    SourceFileBytes { size: u64, max: u64 },
    Timeout { limit: Duration },
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Limit::Dimensions { width, height, max } => {
                write!(
                    f,
                    "image dimensions {width}x{height} exceed maximum {max}x{max} px"
                )
            }
            Limit::PixelCount { pixels, max } => {
                write!(f, "image pixel count {pixels} exceeds maximum {max} pixels")
            }
            Limit::MemoryBytes { needed, max } => {
                write!(
                    f,
                    "estimated memory {} MiB exceeds budget {} MiB",
                    needed / (1024 * 1024),
                    max / (1024 * 1024)
                )
            }
            Limit::SourceFileBytes { size, max } => {
                write!(
                    f,
                    "source file size {} MiB exceeds maximum {} MiB",
                    size / (1024 * 1024),
                    max / (1024 * 1024)
                )
            }
            Limit::Timeout { limit } => {
                write!(f, "conversion exceeded timeout of {}s", limit.as_secs())
            }
        }
    }
}

/// Structured error returned by planning, inspection, configuration, and conversion operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConversionError {
    #[error("unknown or unsupported input format for {path}")]
    UnknownInputFormat { path: PathBuf },

    #[error("no conversion route available: {0:?}")]
    NoRoute(NoRoute),

    #[error("unsupported feature: {feature}")]
    UnsupportedFeature { feature: UnsupportedFeature },

    #[error("invalid conversion options: {issues:?}")]
    InvalidOptions { issues: Vec<OptionIssue> },

    #[error("corrupt {format} input: {detail}")]
    CorruptInput { format: FormatId, detail: String },

    #[error("resource limit exceeded: {limit}")]
    LimitExceeded { limit: Limit },

    #[error("source file modified during conversion: {path}")]
    SourceModifiedDuringConversion { path: PathBuf },

    #[error("output file already exists: {path}")]
    OutputExists { path: PathBuf },

    #[error("could not find an available filename after {attempts} attempts near {base}")]
    CollisionExhausted { base: PathBuf, attempts: u32 },

    #[error("output path resolves to source file: {path}")]
    OutputEqualsSource { path: PathBuf },

    #[error("filesystem at {path} does not support safe atomic no-replace commit ({detail})")]
    FilesystemUnsupported { path: PathBuf, detail: String },

    #[error("I/O error while {op} ({path}): {source}")]
    Io {
        op: FsOp,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("backend {} failed: {detail}", backend.0)]
    BackendFailure { backend: BackendId, detail: String },

    #[error("produced output failed validation: {detail}")]
    OutputValidationFailed { detail: String },

    #[error("conversion cancelled")]
    Cancelled,
}

impl ConversionError {
    /// Returns the coarse [`ErrorClass`] for this error.
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            ConversionError::UnknownInputFormat { .. }
            | ConversionError::InvalidOptions { .. }
            | ConversionError::OutputEqualsSource { .. } => ErrorClass::UserInput,
            ConversionError::NoRoute(_) | ConversionError::UnsupportedFeature { .. } => {
                ErrorClass::Unsupported
            }
            ConversionError::CorruptInput { .. } => ErrorClass::CorruptInput,
            ConversionError::LimitExceeded { .. } => ErrorClass::ResourceLimit,
            ConversionError::OutputExists { .. } | ConversionError::CollisionExhausted { .. } => {
                ErrorClass::OutputConflict
            }
            ConversionError::SourceModifiedDuringConversion { .. }
            | ConversionError::FilesystemUnsupported { .. }
            | ConversionError::Io { .. } => ErrorClass::Io,
            ConversionError::Cancelled => ErrorClass::Cancelled,
            ConversionError::BackendFailure { .. }
            | ConversionError::OutputValidationFailed { .. } => ErrorClass::Internal,
        }
    }

    /// Stable machine-readable error code persisted in the job store (`JOB_SYSTEM_DESIGN.md` §4.3, §5).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            ConversionError::UnknownInputFormat { .. } => "unsupported_format",
            ConversionError::NoRoute(_) => "no_route",
            ConversionError::UnsupportedFeature { .. } => "unsupported_feature",
            ConversionError::InvalidOptions { .. } => "invalid_options",
            ConversionError::CorruptInput { .. } => "corrupt_input",
            ConversionError::LimitExceeded {
                limit: Limit::Timeout { .. },
            } => "timed_out",
            ConversionError::LimitExceeded { .. } => "limit_exceeded",
            ConversionError::SourceModifiedDuringConversion { .. } => "source_changed",
            ConversionError::OutputExists { .. } => "output_exists",
            ConversionError::CollisionExhausted { .. } => "collision_exhausted",
            ConversionError::OutputEqualsSource { .. } => "output_equals_source",
            ConversionError::FilesystemUnsupported { .. } => "filesystem_unsupported",
            ConversionError::Io { source, .. } => match source.raw_os_error() {
                Some(raw) if raw == rustix::io::Errno::NOSPC.raw_os_error() => "no_space",
                Some(raw)
                    if raw == rustix::io::Errno::ACCESS.raw_os_error()
                        || raw == rustix::io::Errno::PERM.raw_os_error()
                        || raw == rustix::io::Errno::ROFS.raw_os_error() =>
                {
                    "permission_denied"
                }
                Some(raw) if raw == rustix::io::Errno::NOENT.raw_os_error() => "source_not_found",
                _ => "io_error",
            },
            ConversionError::BackendFailure { .. } => "backend_failure",
            ConversionError::OutputValidationFailed { .. } => "output_validation_failed",
            ConversionError::Cancelled => "cancelled",
        }
    }

    /// Whether retrying the item with identical settings (or after freeing disk space / fixing permissions)
    /// could plausibly succeed (`CONVERSION_PRODUCT_SPEC.md` §12).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            ConversionError::SourceModifiedDuringConversion { .. } | ConversionError::Cancelled => {
                true
            }
            ConversionError::LimitExceeded {
                limit: Limit::Timeout { .. },
            } => true,
            ConversionError::Io { source, .. } => {
                if matches!(
                    source.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::PermissionDenied
                ) {
                    return true;
                }
                matches!(
                    source.raw_os_error(),
                    Some(raw)
                        if raw == rustix::io::Errno::NOSPC.raw_os_error()
                            || raw == rustix::io::Errno::ACCESS.raw_os_error()
                            || raw == rustix::io::Errno::PERM.raw_os_error()
                            || raw == rustix::io::Errno::ROFS.raw_os_error()
                )
            }
            _ => false,
        }
    }

    /// Concise, user-safe one-line summary suitable for notifications and job history.
    #[must_use]
    pub fn user_summary(&self) -> String {
        match self {
            ConversionError::UnknownInputFormat { .. } => {
                "Unsupported or unrecognized file format.".to_string()
            }
            ConversionError::NoRoute(NoRoute::BackendUnavailable {
                install_hint: Some(hint),
                ..
            }) => format!("Required conversion backend is not installed ({hint})."),
            ConversionError::NoRoute(_) => {
                "No conversion backend supports the requested format and options.".to_string()
            }
            ConversionError::UnsupportedFeature { feature } => {
                format!("Unsupported image feature ({feature}).")
            }
            ConversionError::InvalidOptions { .. } => {
                "Conversion settings are invalid.".to_string()
            }
            ConversionError::CorruptInput { format, .. } => {
                format!(
                    "The {} file appears to be damaged or incomplete.",
                    format.descriptor().display_name
                )
            }
            ConversionError::LimitExceeded { limit } => {
                format!("Image exceeds safety limit ({limit}).")
            }
            ConversionError::SourceModifiedDuringConversion { .. } => {
                "Source file changed while conversion was in progress.".to_string()
            }
            ConversionError::OutputExists { .. } => {
                "Destination file already exists and overwrite is disabled.".to_string()
            }
            ConversionError::CollisionExhausted { attempts, .. } => {
                format!("Could not find an unused output filename after {attempts} attempts.")
            }
            ConversionError::OutputEqualsSource { .. } => {
                "Destination path cannot be the same as the source file.".to_string()
            }
            ConversionError::FilesystemUnsupported { .. } => {
                "Destination filesystem does not support safe atomic file creation.".to_string()
            }
            ConversionError::Io { op, source, .. } => match source.raw_os_error() {
                Some(libc_enosys) if libc_enosys == rustix::io::Errno::NOSPC.raw_os_error() => {
                    "Not enough free disk space in destination folder.".to_string()
                }
                Some(libc_eacces)
                    if libc_eacces == rustix::io::Errno::ACCESS.raw_os_error()
                        || libc_eacces == rustix::io::Errno::PERM.raw_os_error() =>
                {
                    format!("Permission denied while {op}.")
                }
                _ => format!("File error while {op}."),
            },
            ConversionError::BackendFailure { .. } => {
                "Encoder failed while processing this image.".to_string()
            }
            ConversionError::OutputValidationFailed { .. } => {
                "Converted output did not pass integrity validation.".to_string()
            }
            ConversionError::Cancelled => "Conversion was cancelled.".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_classes_and_summaries() {
        let err = ConversionError::UnsupportedFeature {
            feature: UnsupportedFeature::Animation,
        };
        assert_eq!(err.class(), ErrorClass::Unsupported);
        assert!(!err.is_retryable());
        assert!(err.user_summary().contains("animated images"));

        let modified = ConversionError::SourceModifiedDuringConversion {
            path: PathBuf::from("/tmp/a.png"),
        };
        assert_eq!(modified.class(), ErrorClass::Io);
        assert!(modified.is_retryable());
    }
}
