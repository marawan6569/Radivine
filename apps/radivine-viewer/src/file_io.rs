//! Safe non-blocking file opening (`O_NONBLOCK` + `fstat` TOCTOU verification) and text encoding
//! detection/decoding (`ACTION_SYSTEM.md` §4.2, §8.1).

use std::fs::{File, Metadata};
use std::io::ErrorKind;
use std::os::unix::fs::{FileExt, FileTypeExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use encoding_rs::{UTF_16BE, UTF_16LE};
use rustix::fs::{Mode, OFlags};

/// Exit code for normal viewer termination (`ACTION_SYSTEM.md` §8.1).
pub const EXIT_OK: u8 = 0;

/// Exit code for CLI usage error (`ACTION_SYSTEM.md` §8.1).
pub const EXIT_USAGE: u8 = 2;

/// Exit code when the target file cannot be opened or is not a valid previewable text file (`ACTION_SYSTEM.md` §8.1).
pub const EXIT_CANNOT_OPEN: u8 = 3;

/// Number of initial bytes inspected for BOM, binary magic, and text encoding (`8 KiB`).
pub const SNIFF_BYTES: usize = 8192;

/// Maximum length in bytes of a single line rendered in the UI before truncating with `"…"` (`64 KiB`, `ACTION_SYSTEM.md` §8.1).
pub const MAX_LINE_BYTES: usize = 64 * 1024;

/// Errors that can occur when opening or reading a file in `radivine-viewer`.
#[derive(thiserror::Error, Debug)]
pub enum ViewerError {
    /// File does not exist.
    #[error("File not found: {path}")]
    NotFound {
        /// Target path.
        path: PathBuf,
    },
    /// Path is a symbolic link whose target does not exist.
    #[error("Broken symbolic link: {path}")]
    BrokenSymlink {
        /// Target path.
        path: PathBuf,
    },
    /// Insufficient permissions to read the file.
    #[error("Permission denied: {path}")]
    PermissionDenied {
        /// Target path.
        path: PathBuf,
    },
    /// Path is a directory rather than a regular file.
    #[error("Folders cannot be previewed: {path}")]
    IsDirectory {
        /// Target path.
        path: PathBuf,
    },
    /// Path is a special file (FIFO, socket, character/block device).
    #[error("Special file ({kind}) cannot be previewed: {path}")]
    SpecialFile {
        /// Target path.
        path: PathBuf,
        /// Kind description (`FIFO`, `socket`, `device`, etc.).
        kind: &'static str,
    },
    /// File is a binary file rather than text.
    #[error("Binary file cannot be previewed as text: {path}")]
    BinaryFile {
        /// Target path.
        path: PathBuf,
    },
    /// Underlying I/O error.
    #[error("I/O error reading {path}: {source}")]
    Io {
        /// Target path.
        path: PathBuf,
        /// OS error.
        #[source]
        source: std::io::Error,
    },
}

/// Text encoding detected from the file's initial bytes (`ACTION_SYSTEM.md` §4.2, §8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DetectedEncoding {
    /// Valid UTF-8 without BOM.
    Utf8,
    /// UTF-8 with `EF BB BF` BOM (3 bytes).
    Utf8Bom,
    /// UTF-16 Little Endian (with optional `FF FE` 2-byte BOM).
    Utf16Le {
        /// Whether the file begins with `FF FE`.
        has_bom: bool,
    },
    /// UTF-16 Big Endian (with optional `FE FF` 2-byte BOM).
    Utf16Be {
        /// Whether the file begins with `FE FF`.
        has_bom: bool,
    },
    /// Non-UTF-8 8-bit text (≤ 10% control bytes); decoded lossily with replacement characters and a warning banner.
    LossyUtf8,
}

impl DetectedEncoding {
    /// Human-readable label shown in the viewer status bar.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf8Bom => "UTF-8 BOM",
            Self::Utf16Le { .. } => "UTF-16 LE",
            Self::Utf16Be { .. } => "UTF-16 BE",
            Self::LossyUtf8 => "UTF-8 (Lossy)",
        }
    }

    /// Number of leading BOM bytes at byte offset 0 of the file.
    #[must_use]
    pub const fn bom_len(self) -> u64 {
        match self {
            Self::Utf8 | Self::LossyUtf8 => 0,
            Self::Utf8Bom => 3,
            Self::Utf16Le { has_bom } | Self::Utf16Be { has_bom } => {
                if has_bom {
                    2
                } else {
                    0
                }
            }
        }
    }

    /// Number of bytes per newline code unit (`1` for UTF-8/8-bit, `2` for UTF-16).
    #[must_use]
    pub const fn code_unit_bytes(self) -> usize {
        match self {
            Self::Utf8 | Self::Utf8Bom | Self::LossyUtf8 => 1,
            Self::Utf16Le { .. } | Self::Utf16Be { .. } => 2,
        }
    }

    /// Returns `true` if this encoding is UTF-16 (LE or BE).
    #[must_use]
    pub const fn is_utf16(self) -> bool {
        matches!(self, Self::Utf16Le { .. } | Self::Utf16Be { .. })
    }
}

/// Validated, read-only regular text file handle with detected metadata.
#[derive(Debug)]
pub struct OpenedTextFile {
    /// Underlying read-only file descriptor (`O_RDONLY | O_CLOEXEC`, verified regular via `fstat`).
    pub file: File,
    /// File path.
    pub path: PathBuf,
    /// Display file name.
    pub display_name: String,
    /// Initial file size in bytes from `fstat`.
    pub initial_size: u64,
    /// Initial modification time from `fstat`.
    pub initial_mtime: Option<SystemTime>,
    /// Detected text encoding.
    pub encoding: DetectedEncoding,
    /// Whether `\r\n` (CRLF) line endings were observed in the initial sniff buffer.
    pub has_crlf: bool,
    /// Whether the initial sniff buffer contained malformed byte sequences requiring replacement characters.
    pub initial_encoding_warning: bool,
}

impl OpenedTextFile {
    /// Safely opens `path` in read-only, non-blocking mode, validates the opened file descriptor via
    /// `fstat` to prevent TOCTOU races, and sniffs the first `8 KiB` to verify text encoding.
    ///
    /// Never blocks on FIFOs, sockets, or device nodes (`ACTION_SYSTEM.md` §4.2, §8.1).
    pub fn open(path: &Path) -> Result<Self, ViewerError> {
        // 1. Pre-check symlink metadata so we can report `BrokenSymlink` or non-regular types clearly
        // before even attempting `open`.
        let is_symlink = match std::fs::symlink_metadata(path) {
            Ok(lmeta) => {
                let ft = lmeta.file_type();
                if !ft.is_symlink() {
                    if ft.is_dir() {
                        return Err(ViewerError::IsDirectory {
                            path: path.to_path_buf(),
                        });
                    }
                    if let Some(kind) = special_file_kind(&lmeta) {
                        return Err(ViewerError::SpecialFile {
                            path: path.to_path_buf(),
                            kind,
                        });
                    }
                }
                ft.is_symlink()
            }
            Err(err) => {
                return Err(map_io_error(path, err, false));
            }
        };

        // 2. Open with `O_RDONLY | O_NONBLOCK | O_CLOEXEC` so that even if `path` is a symlink to a FIFO
        // or was swapped for a FIFO between `lstat` and `open` (TOCTOU), `open` never blocks.
        let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let owned_fd = rustix::fs::open(path, flags, Mode::empty()).map_err(|errno| {
            let io_err = std::io::Error::from(errno);
            map_io_error(path, io_err, is_symlink)
        })?;

        let file = File::from(owned_fd);

        // 3. Mandatory `fstat` on the opened file descriptor (`TOCTOU` defense).
        let fd_meta = file.metadata().map_err(|source| ViewerError::Io {
            path: path.to_path_buf(),
            source,
        })?;

        if fd_meta.file_type().is_dir() {
            return Err(ViewerError::IsDirectory {
                path: path.to_path_buf(),
            });
        }
        if let Some(kind) = special_file_kind(&fd_meta) {
            return Err(ViewerError::SpecialFile {
                path: path.to_path_buf(),
                kind,
            });
        }
        if !fd_meta.file_type().is_file() {
            return Err(ViewerError::SpecialFile {
                path: path.to_path_buf(),
                kind: "non-regular file",
            });
        }

        // 4. Sniff up to `8 KiB` using `read_at(0)` (`pread`).
        let mut sniff_buf = [0u8; SNIFF_BYTES];
        let sniff_len = read_at_full(&file, &mut sniff_buf, 0).map_err(|source| {
            if source.kind() == ErrorKind::PermissionDenied {
                ViewerError::PermissionDenied {
                    path: path.to_path_buf(),
                }
            } else {
                ViewerError::Io {
                    path: path.to_path_buf(),
                    source,
                }
            }
        })?;
        let sample = &sniff_buf[..sniff_len];

        let (encoding, initial_encoding_warning) =
            classify_text_sample(sample).ok_or_else(|| ViewerError::BinaryFile {
                path: path.to_path_buf(),
            })?;

        let has_crlf = detect_crlf(sample, encoding);
        let display_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        Ok(Self {
            file,
            path: path.to_path_buf(),
            display_name,
            initial_size: fd_meta.len(),
            initial_mtime: fd_meta.modified().ok(),
            encoding,
            has_crlf,
            initial_encoding_warning,
        })
    }

    /// Queries the current length and modification time of the opened file descriptor via `fstat`.
    pub fn current_fd_metadata(&self) -> std::io::Result<Metadata> {
        self.file.metadata()
    }
}

/// Single decoded line prepared for display in `radivine-viewer`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PreviewLine {
    /// 1-based line number in the file (or within the tail window when file size exceeds the fast-count threshold).
    pub line_number: u64,
    /// Decoded line text with trailing `\r`/`\n` removed (and truncated at `64 KiB` with `"…"` if over [`MAX_LINE_BYTES`]).
    pub text: String,
    /// `true` if this line exceeded [`MAX_LINE_BYTES`] (`64 KiB`) and was truncated with `"…"`.
    pub line_truncated: bool,
}

/// Reads up to `buf.len()` bytes from `file` starting at `offset` using `pread` (`FileExt::read_at`),
/// retrying on `EINTR` and stopping cleanly at EOF.
pub fn read_at_full(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    let mut total = 0usize;
    while total < buf.len() {
        match file.read_at(&mut buf[total..], offset + total as u64) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

/// Classifies up to `8 KiB` of initial file bytes into `(DetectedEncoding, has_encoding_warning)`,
/// or returns `None` if the sample is binary (`ACTION_SYSTEM.md` §4.2, §8.1).
#[must_use]
pub fn classify_text_sample(bytes: &[u8]) -> Option<(DetectedEncoding, bool)> {
    if bytes.is_empty() {
        return Some((DetectedEncoding::Utf8, false));
    }

    // 1. Check explicit BOMs
    if let Some(after_bom) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        if has_magic_binary(bytes) || after_bom.contains(&0x00) || has_excess_control(after_bom) {
            return None;
        }
        let valid = is_valid_utf8_allowing_trailing_incomplete(after_bom);
        return Some((DetectedEncoding::Utf8Bom, !valid));
    }
    if let Some(after_bom) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let even_len = after_bom.len() & !1;
        let (_, _, had_errors) = UTF_16LE.decode(&after_bom[..even_len]);
        return Some((DetectedEncoding::Utf16Le { has_bom: true }, had_errors));
    }
    if let Some(after_bom) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let even_len = after_bom.len() & !1;
        let (_, _, had_errors) = UTF_16BE.decode(&after_bom[..even_len]);
        return Some((DetectedEncoding::Utf16Be { has_bom: true }, had_errors));
    }

    // 2. Check magic-number binary signatures
    if has_magic_binary(bytes) {
        return None;
    }

    // 3. Check for BOM-less UTF-16 LE / BE before rejecting NUL bytes
    if let Some(utf16_enc) = detect_bomless_utf16(bytes) {
        return Some((utf16_enc, false));
    }

    // 4. Any NUL byte in non-UTF-16 data -> Binary
    if bytes.contains(&0x00) {
        return None;
    }

    // 5. > 10% disallowed C0/DEL control bytes -> Binary
    if has_excess_control(bytes) {
        return None;
    }

    // 6. Valid UTF-8 (allowing an incomplete multi-byte sequence truncated at the end of `bytes`)
    if is_valid_utf8_allowing_trailing_incomplete(bytes) {
        return Some((DetectedEncoding::Utf8, false));
    }

    // 7. Non-UTF-8 8-bit text (<= 10% control bytes): display lossily with warning banner
    Some((DetectedEncoding::LossyUtf8, true))
}

/// Decodes a slice of file bytes according to `encoding`, returning `(decoded_string, had_encoding_errors)`.
///
/// If `trim_trailing_incomplete_utf8` is `true`, an incomplete multi-byte UTF-8 sequence at the very
/// end of `bytes` (caused by a fixed-size chunk boundary) is excluded rather than treated as an error.
#[must_use]
pub fn decode_bytes(
    bytes: &[u8],
    encoding: DetectedEncoding,
    trim_trailing_incomplete_utf8: bool,
) -> (String, bool) {
    match encoding {
        DetectedEncoding::Utf8 | DetectedEncoding::Utf8Bom | DetectedEncoding::LossyUtf8 => {
            let slice = if trim_trailing_incomplete_utf8 {
                trim_incomplete_utf8_suffix(bytes)
            } else {
                bytes
            };
            match std::str::from_utf8(slice) {
                Ok(s) => (
                    s.to_string(),
                    matches!(encoding, DetectedEncoding::LossyUtf8),
                ),
                Err(_) => (String::from_utf8_lossy(slice).into_owned(), true),
            }
        }
        DetectedEncoding::Utf16Le { .. } => {
            let even_len = bytes.len() & !1;
            let odd_tail = bytes.len() != even_len && !trim_trailing_incomplete_utf8;
            let (cow, _, had_errors) = UTF_16LE.decode(&bytes[..even_len]);
            let mut out = cow.into_owned();
            if odd_tail {
                out.push('\u{FFFD}');
            }
            (out, had_errors || odd_tail)
        }
        DetectedEncoding::Utf16Be { .. } => {
            let even_len = bytes.len() & !1;
            let odd_tail = bytes.len() != even_len && !trim_trailing_incomplete_utf8;
            let (cow, _, had_errors) = UTF_16BE.decode(&bytes[..even_len]);
            let mut out = cow.into_owned();
            if odd_tail {
                out.push('\u{FFFD}');
            }
            (out, had_errors || odd_tail)
        }
    }
}

/// Converts a decoded text block into [`PreviewLine`] rows starting at `first_line_number` (1-based),
/// stripping `\r`/`\n` and truncating any line longer than `max_line_bytes` with `"…"`.
///
/// Returns `(lines, any_line_truncated)`.
#[must_use]
pub fn split_decoded_lines(
    decoded: &str,
    first_line_number: u64,
    max_line_bytes: usize,
) -> (Vec<PreviewLine>, bool) {
    if decoded.is_empty() {
        return (Vec::new(), false);
    }

    let mut lines = Vec::new();
    let mut any_truncated = false;
    let raw_parts: Vec<&str> = decoded.split('\n').collect();
    // If `decoded` ends with `\n`, `split('\n')` produces a trailing empty `""` after the last newline;
    // that trailing `""` is the terminator of the final line, not an extra line.
    let effective_len = if decoded.ends_with('\n') && !raw_parts.is_empty() {
        raw_parts.len() - 1
    } else {
        raw_parts.len()
    };

    for (idx, &raw_line) in raw_parts[..effective_len].iter().enumerate() {
        let stripped = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let (text, line_truncated) = truncate_line_for_display(stripped, max_line_bytes);
        if line_truncated {
            any_truncated = true;
        }
        lines.push(PreviewLine {
            line_number: first_line_number + idx as u64,
            text,
            line_truncated,
        });
    }

    (lines, any_truncated)
}

/// Truncates a single line's display string if its byte length exceeds `max_line_bytes`,
/// appending `"…"` on a valid UTF-8 character boundary.
#[must_use]
pub fn truncate_line_for_display(line: &str, max_line_bytes: usize) -> (String, bool) {
    if line.len() <= max_line_bytes {
        return (line.to_string(), false);
    }
    let cut = floor_char_boundary_str(line, max_line_bytes);
    let mut s = String::with_capacity(cut + 3);
    s.push_str(&line[..cut]);
    s.push('…');
    (s, true)
}

fn floor_char_boundary_str(s: &str, max_bytes: usize) -> usize {
    if max_bytes >= s.len() {
        return s.len();
    }
    let mut idx = max_bytes;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn trim_incomplete_utf8_suffix(bytes: &[u8]) -> &[u8] {
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes,
        Err(err) => {
            if err.error_len().is_none() {
                &bytes[..err.valid_up_to()]
            } else {
                bytes
            }
        }
    }
}

fn is_valid_utf8_allowing_trailing_incomplete(bytes: &[u8]) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(err) => err.error_len().is_none(),
    }
}

fn has_excess_control(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let bad = bytes
        .iter()
        .filter(|&&b| (b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0C | 0x1B)) || b == 0x7F)
        .count();
    bad * 10 > bytes.len()
}

fn detect_bomless_utf16(bytes: &[u8]) -> Option<DetectedEncoding> {
    if bytes.len() < 4 || !bytes.len().is_multiple_of(2) {
        return None;
    }
    let pairs = bytes.len() / 2;
    let mut even_zeros = 0usize;
    let mut odd_zeros = 0usize;
    for chunk in bytes.chunks_exact(2) {
        if chunk[0] == 0 {
            even_zeros += 1;
        }
        if chunk[1] == 0 {
            odd_zeros += 1;
        }
    }
    // Heuristic for ASCII-heavy BOM-less UTF-16: >= 60% of high bytes are 0 and 0% of low bytes are 0
    if odd_zeros * 10 >= pairs * 6 && even_zeros == 0 {
        let (_, _, had_err) = UTF_16LE.decode(bytes);
        if !had_err {
            return Some(DetectedEncoding::Utf16Le { has_bom: false });
        }
    }
    if even_zeros * 10 >= pairs * 6 && odd_zeros == 0 {
        let (_, _, had_err) = UTF_16BE.decode(bytes);
        if !had_err {
            return Some(DetectedEncoding::Utf16Be { has_bom: false });
        }
    }
    None
}

fn detect_crlf(sample: &[u8], encoding: DetectedEncoding) -> bool {
    match encoding {
        DetectedEncoding::Utf8 | DetectedEncoding::Utf8Bom | DetectedEncoding::LossyUtf8 => {
            sample.windows(2).any(|w| w == b"\r\n")
        }
        DetectedEncoding::Utf16Le { has_bom } => {
            let start = if has_bom && sample.len() >= 2 { 2 } else { 0 };
            sample[start..]
                .chunks_exact(2)
                .collect::<Vec<_>>()
                .windows(2)
                .any(|w| w[0] == [0x0D, 0x00] && w[1] == [0x0A, 0x00])
        }
        DetectedEncoding::Utf16Be { has_bom } => {
            let start = if has_bom && sample.len() >= 2 { 2 } else { 0 };
            sample[start..]
                .chunks_exact(2)
                .collect::<Vec<_>>()
                .windows(2)
                .any(|w| w[0] == [0x00, 0x0D] && w[1] == [0x00, 0x0A])
        }
    }
}

fn has_magic_binary(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xFF\xD8\xFF")
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || (bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP")
        || bytes.starts_with(b"%PDF-")
        || bytes.starts_with(b"\x7FELF")
        || bytes.starts_with(b"\x00asm")
        || bytes.starts_with(b"\xCA\xFE\xBA\xBE")
        || bytes.starts_with(b"SQLite format 3\x00")
        || bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
        || bytes.starts_with(b"\x1F\x8B\x08")
        || bytes.starts_with(b"BZh")
        || bytes.starts_with(b"\xFD7zXZ\x00")
        || bytes.starts_with(b"\x28\xB5\x2F\xFD")
        || bytes.starts_with(b"7z\xBC\xAF\x27\x1C")
        || bytes.starts_with(b"Rar!\x1A\x07")
        || (bytes.len() >= 262 && &bytes[257..262] == b"ustar")
        || bytes.starts_with(b"fLaC")
        || bytes.starts_with(b"OggS")
        || bytes.starts_with(b"wOFF")
        || bytes.starts_with(b"wOF2")
        || bytes.starts_with(b"OTTO")
}

fn special_file_kind(meta: &Metadata) -> Option<&'static str> {
    let ft = meta.file_type();
    if ft.is_fifo() {
        Some("FIFO")
    } else if ft.is_socket() {
        Some("socket")
    } else if ft.is_char_device() {
        Some("character device")
    } else if ft.is_block_device() {
        Some("block device")
    } else {
        None
    }
}

fn map_io_error(path: &Path, err: std::io::Error, is_symlink: bool) -> ViewerError {
    match err.kind() {
        ErrorKind::NotFound => {
            if is_symlink {
                ViewerError::BrokenSymlink {
                    path: path.to_path_buf(),
                }
            } else {
                ViewerError::NotFound {
                    path: path.to_path_buf(),
                }
            }
        }
        ErrorKind::PermissionDenied => ViewerError::PermissionDenied {
            path: path.to_path_buf(),
        },
        _ => {
            // On Linux, opening a directory or special file in certain modes can also return EISDIR / ENXIO
            if err.raw_os_error() == Some(21) {
                ViewerError::IsDirectory {
                    path: path.to_path_buf(),
                }
            } else if err.raw_os_error() == Some(6) {
                // ENXIO (e.g. FIFO with O_NONBLOCK and no writer, or device node)
                ViewerError::SpecialFile {
                    path: path.to_path_buf(),
                    kind: "FIFO/device",
                }
            } else {
                ViewerError::Io {
                    path: path.to_path_buf(),
                    source: err,
                }
            }
        }
    }
}

/// Formats a byte count into a concise human-readable string (`1.24 KB`, `5.00 GB`, etc.).
#[must_use]
pub fn format_byte_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < MB {
        format!("{:.1} KiB", b / KB)
    } else if b < GB {
        format!("{:.2} MiB", b / MB)
    } else {
        format!("{:.2} GiB", b / GB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn make_temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "radivine_viewer_fio_{label}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn test_encodings_utf8_bom_utf16_and_lossy() {
        let dir = make_temp_dir("encodings");

        // 1. Plain UTF-8
        let p_utf8 = dir.join("plain.txt");
        std::fs::write(&p_utf8, "hello\nworld\n").expect("write");
        let f_utf8 = OpenedTextFile::open(&p_utf8).expect("open utf8");
        assert_eq!(f_utf8.encoding, DetectedEncoding::Utf8);
        assert!(!f_utf8.initial_encoding_warning);
        assert!(!f_utf8.has_crlf);

        // 2. UTF-8 BOM + CRLF
        let p_bom = dir.join("bom.txt");
        let mut bom_bytes = vec![0xEF, 0xBB, 0xBF];
        bom_bytes.extend_from_slice(b"first\r\nsecond\r\n");
        std::fs::write(&p_bom, &bom_bytes).expect("write");
        let f_bom = OpenedTextFile::open(&p_bom).expect("open bom");
        assert_eq!(f_bom.encoding, DetectedEncoding::Utf8Bom);
        assert!(f_bom.has_crlf);
        assert!(!f_bom.initial_encoding_warning);

        // 3. UTF-16 LE with BOM
        let p_u16le = dir.join("u16le.txt");
        let mut u16le_bytes = vec![0xFF, 0xFE];
        for ch in "αβγ\nδεζ\n".encode_utf16() {
            u16le_bytes.extend_from_slice(&ch.to_le_bytes());
        }
        std::fs::write(&p_u16le, &u16le_bytes).expect("write");
        let f_u16le = OpenedTextFile::open(&p_u16le).expect("open u16le");
        assert_eq!(
            f_u16le.encoding,
            DetectedEncoding::Utf16Le { has_bom: true }
        );
        let (decoded_le, had_err_le) = decode_bytes(&u16le_bytes[2..], f_u16le.encoding, false);
        assert!(!had_err_le);
        assert_eq!(decoded_le, "αβγ\nδεζ\n");

        // 4. UTF-16 BE with BOM
        let p_u16be = dir.join("u16be.txt");
        let mut u16be_bytes = vec![0xFE, 0xFF];
        for ch in "Hello UTF-16BE\r\nLine 2\r\n".encode_utf16() {
            u16be_bytes.extend_from_slice(&ch.to_be_bytes());
        }
        std::fs::write(&p_u16be, &u16be_bytes).expect("write");
        let f_u16be = OpenedTextFile::open(&p_u16be).expect("open u16be");
        assert_eq!(
            f_u16be.encoding,
            DetectedEncoding::Utf16Be { has_bom: true }
        );
        assert!(f_u16be.has_crlf);
        let (decoded_be, had_err_be) = decode_bytes(&u16be_bytes[2..], f_u16be.encoding, false);
        assert!(!had_err_be);
        assert_eq!(decoded_be, "Hello UTF-16BE\r\nLine 2\r\n");

        // 5. Invalid UTF-8 (8-bit text) -> LossyUtf8 with warning
        let p_lossy = dir.join("lossy.txt");
        std::fs::write(&p_lossy, b"valid ascii then \xFF\xFE invalid utf8\n").expect("write");
        let f_lossy = OpenedTextFile::open(&p_lossy).expect("open lossy");
        assert_eq!(f_lossy.encoding, DetectedEncoding::LossyUtf8);
        assert!(f_lossy.initial_encoding_warning);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rejects_missing_broken_symlink_directory_fifo_and_binary() {
        let dir = make_temp_dir("safety");

        // 1. Missing file
        let missing = dir.join("does_not_exist.txt");
        assert!(matches!(
            OpenedTextFile::open(&missing),
            Err(ViewerError::NotFound { .. })
        ));

        // 2. Broken symlink
        let broken = dir.join("broken_link.txt");
        std::os::unix::fs::symlink(&missing, &broken).expect("symlink");
        assert!(matches!(
            OpenedTextFile::open(&broken),
            Err(ViewerError::BrokenSymlink { .. })
        ));

        // 3. Directory
        assert!(matches!(
            OpenedTextFile::open(&dir),
            Err(ViewerError::IsDirectory { .. })
        ));

        // 4. FIFO (must NOT block!)
        let fifo = dir.join("test_pipe.fifo");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::FileType::Fifo,
            Mode::RUSR | Mode::WUSR,
            0,
        )
        .expect("mkfifo");
        assert!(matches!(
            OpenedTextFile::open(&fifo),
            Err(ViewerError::SpecialFile { .. })
        ));

        // 5. Symlink pointing to FIFO (tests O_NONBLOCK + fstat TOCTOU path)
        let fifo_link = dir.join("fifo_link.txt");
        std::os::unix::fs::symlink(&fifo, &fifo_link).expect("symlink to fifo");
        assert!(matches!(
            OpenedTextFile::open(&fifo_link),
            Err(ViewerError::SpecialFile { .. })
        ));

        // 6. Binary PNG file
        let png = dir.join("image.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR").expect("write png");
        assert!(matches!(
            OpenedTextFile::open(&png),
            Err(ViewerError::BinaryFile { .. })
        ));

        // 7. Unreadable file (0o000) when not running as root
        if rustix::process::geteuid().as_raw() != 0 {
            let unread = dir.join("unreadable.txt");
            std::fs::write(&unread, "secret\n").expect("write");
            std::fs::set_permissions(&unread, std::fs::Permissions::from_mode(0o000))
                .expect("chmod 000");
            assert!(matches!(
                OpenedTextFile::open(&unread),
                Err(ViewerError::PermissionDenied { .. })
            ));
            let _ = std::fs::set_permissions(&unread, std::fs::Permissions::from_mode(0o600));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
