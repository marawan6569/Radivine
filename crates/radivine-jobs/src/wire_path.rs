//! Lossless transport encoding for Unix filesystem paths (`ADR-034`, `SECURITY_AND_FILE_SAFETY.md` §2.1).
//!
//! Linux `OsStr` paths are arbitrary non-NUL byte sequences (`[u8]`) and are not guaranteed to be
//! valid UTF-8. `WirePath` percent-encodes raw path bytes into an ASCII JSON string so non-UTF-8
//! paths round-trip through JSON-lines IPC and SQLite `BLOB` columns with zero byte loss.

use std::ffi::OsStr;
use std::fmt;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, percent_encode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Maximum decoded byte length of a Unix filesystem path (`PATH_MAX = 4096`).
pub const MAX_WIRE_PATH_BYTES: usize = 4096;

/// Characters that are percent-encoded in a [`WirePath`]: everything except ASCII alphanumeric
/// and `-`, `.`, `_`, `~`, `/`. Notice `%` is always encoded as `%25`.
const WIRE_PATH_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

/// Errors returned when encoding or decoding a [`WirePath`].
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WirePathError {
    #[error("path cannot be empty")]
    Empty,
    #[error("path contains a forbidden NUL (0x00) byte")]
    ContainsNul,
    #[error("path length {len} bytes exceeds maximum {max} bytes")]
    TooLong { len: usize, max: usize },
    #[error("path must be an absolute path starting with '/'")]
    NotAbsolute,
    #[error("invalid percent-encoding in wire path: {detail}")]
    InvalidEncoding { detail: String },
}

/// Lossless ASCII percent-encoded representation of a Unix filesystem path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WirePath(String);

impl WirePath {
    /// Encodes a [`Path`] into a [`WirePath`], validating non-empty, no NUL bytes, and `<= 4096` bytes.
    pub fn from_path(path: &Path) -> Result<Self, WirePathError> {
        Self::from_raw_bytes(path.as_os_str().as_bytes())
    }

    /// Encodes an absolute [`Path`] into a [`WirePath`], rejecting relative paths.
    pub fn from_absolute_path(path: &Path) -> Result<Self, WirePathError> {
        let wp = Self::from_path(path)?;
        if !path.is_absolute() {
            return Err(WirePathError::NotAbsolute);
        }
        Ok(wp)
    }

    /// Encodes raw Unix path bytes into a [`WirePath`].
    pub fn from_raw_bytes(bytes: &[u8]) -> Result<Self, WirePathError> {
        if bytes.is_empty() {
            return Err(WirePathError::Empty);
        }
        if bytes.contains(&0) {
            return Err(WirePathError::ContainsNul);
        }
        if bytes.len() > MAX_WIRE_PATH_BYTES {
            return Err(WirePathError::TooLong {
                len: bytes.len(),
                max: MAX_WIRE_PATH_BYTES,
            });
        }
        let encoded = percent_encode(bytes, WIRE_PATH_ENCODE_SET).to_string();
        Ok(Self(encoded))
    }

    /// Constructs a [`WirePath`] from an already-encoded wire string after validating that it
    /// decodes cleanly and obeys all length/NUL invariants.
    pub fn parse_encoded(encoded: impl Into<String>) -> Result<Self, WirePathError> {
        let s = encoded.into();
        let wp = Self(s);
        let _ = wp.decode_bytes()?;
        Ok(wp)
    }

    /// Returns the percent-encoded ASCII string representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Decodes the [`WirePath`] back into raw Unix path bytes, validating all invariants.
    pub fn decode_bytes(&self) -> Result<Vec<u8>, WirePathError> {
        if self.0.is_empty() {
            return Err(WirePathError::Empty);
        }
        // Validate that every '%' is followed by two valid hex digits
        let raw_ascii = self.0.as_bytes();
        let mut i = 0;
        while i < raw_ascii.len() {
            if raw_ascii[i] == b'%' {
                if i + 2 >= raw_ascii.len()
                    || !raw_ascii[i + 1].is_ascii_hexdigit()
                    || !raw_ascii[i + 2].is_ascii_hexdigit()
                {
                    return Err(WirePathError::InvalidEncoding {
                        detail: format!("incomplete or non-hex percent escape at byte {i}"),
                    });
                }
                i += 3;
            } else {
                i += 1;
            }
        }

        let decoded: Vec<u8> = percent_decode_str(&self.0).collect();
        if decoded.is_empty() {
            return Err(WirePathError::Empty);
        }
        if decoded.contains(&0) {
            return Err(WirePathError::ContainsNul);
        }
        if decoded.len() > MAX_WIRE_PATH_BYTES {
            return Err(WirePathError::TooLong {
                len: decoded.len(),
                max: MAX_WIRE_PATH_BYTES,
            });
        }
        Ok(decoded)
    }

    /// Decodes the [`WirePath`] into a [`PathBuf`] with zero byte loss.
    pub fn to_path_buf(&self) -> Result<PathBuf, WirePathError> {
        let bytes = self.decode_bytes()?;
        Ok(PathBuf::from(OsStr::from_bytes(&bytes)))
    }

    /// Decodes the [`WirePath`] into an absolute [`PathBuf`], rejecting relative paths.
    pub fn to_absolute_path_buf(&self) -> Result<PathBuf, WirePathError> {
        let pb = self.to_path_buf()?;
        if !pb.is_absolute() {
            return Err(WirePathError::NotAbsolute);
        }
        Ok(pb)
    }

    /// Returns a human-readable lossy display string (`U+FFFD` for non-UTF-8 bytes) for UI/CLI display only.
    #[must_use]
    pub fn lossy_display(&self) -> String {
        match self.to_path_buf() {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(_) => self.0.clone(),
        }
    }
}

impl fmt::Display for WirePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn non_utf8_and_special_characters_roundtrip_exact_bytes() {
        let cases: &[&[u8]] = &[
            b"/home/user/Pictures/photo.png",
            b"/tmp/non_utf8_\x80\xfe\xff_name.jpg",
            b"/tmp/with spaces & %20 literal percent/file (1).webp",
            b"/tmp/line\nbreak/tab\there/quote\"'\\backslash.png",
        ];

        for &raw in cases {
            let path = Path::new(OsStr::from_bytes(raw));
            let wp = WirePath::from_absolute_path(path).unwrap();
            let json = serde_json::to_string(&wp).unwrap();
            let back_wp: WirePath = serde_json::from_str(&json).unwrap();
            let decoded = back_wp.to_absolute_path_buf().unwrap();
            assert_eq!(decoded.as_os_str().as_bytes(), raw);
        }
    }

    #[test]
    fn rejects_empty_nul_relative_and_overlong_paths() {
        assert_eq!(
            WirePath::from_raw_bytes(b"").unwrap_err(),
            WirePathError::Empty
        );
        assert_eq!(
            WirePath::from_raw_bytes(b"/tmp/bad\0path.png").unwrap_err(),
            WirePathError::ContainsNul
        );
        assert_eq!(
            WirePath::parse_encoded("/tmp/bad%00path.png").unwrap_err(),
            WirePathError::ContainsNul
        );
        assert_eq!(
            WirePath::from_absolute_path(Path::new("relative/photo.png")).unwrap_err(),
            WirePathError::NotAbsolute
        );
        let overlong = vec![b'a'; MAX_WIRE_PATH_BYTES + 1];
        assert!(matches!(
            WirePath::from_raw_bytes(&overlong).unwrap_err(),
            WirePathError::TooLong { .. }
        ));
        assert!(matches!(
            WirePath::parse_encoded("/tmp/%ZZbad").unwrap_err(),
            WirePathError::InvalidEncoding { .. }
        ));
    }

    proptest! {
        #[test]
        fn proptest_wirepath_arbitrary_non_nul_bytes_roundtrip(
            tail in proptest::collection::vec(1u8..=255u8, 1..512)
        ) {
            let mut raw = vec![b'/'];
            raw.extend_from_slice(&tail);
            let wp = WirePath::from_raw_bytes(&raw).unwrap();
            let json = serde_json::to_string(&wp).unwrap();
            let parsed: WirePath = serde_json::from_str(&json).unwrap();
            let out = parsed.to_absolute_path_buf().unwrap();
            prop_assert_eq!(out.as_os_str().as_bytes(), raw.as_slice());
        }
    }
}
