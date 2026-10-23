//! Streaming `Head` and reverse-scanning `Tail` preview engines (`ACTION_SYSTEM.md` §8.1, `AC-5.1`).
//!
//! Guarantees:
//! - `read_head` streams in `64 KiB` blocks and stops immediately after `max_lines` lines or `8 MiB`.
//! - `read_tail` scans backwards from the end of the file in `64 KiB` blocks using `pread` (`read_at`)
//!   and `memchr::memrchr_iter`, so its cost is proportional to the size of the last `N` lines and
//!   completely independent of total file size (`< 5 ms` on a 5 GB file).
//! - For UTF-8 files within the `8 MiB` safety cap, `raw_output` is byte-for-byte identical to
//!   GNU `head -n N` and `tail -n N`.

use std::fs::File;

use memchr::{memchr_iter, memrchr_iter};

use crate::PreviewMode;
use crate::file_io::{
    DetectedEncoding, MAX_LINE_BYTES, OpenedTextFile, PreviewLine, ViewerError, decode_bytes,
    read_at_full, split_decoded_lines,
};

/// Block size used for forward streaming in `Head` and reverse scanning in `Tail` (`64 KiB`).
pub const READ_BLOCK_BYTES: usize = 64 * 1024;

/// Maximum bytes scanned/loaded in `Head` or `Tail` before stopping with an `"Output truncated"` banner (`8 MiB`).
pub const MAX_HEAD_TAIL_BYTES: usize = 8 * 1024 * 1024;

/// Threshold below which `Tail` computes exact 1-based file line numbers (`16 MiB`); above this threshold,
/// `Tail` avoids scanning the prefix of multi-gigabyte files so `Tail` remains `O(last N lines)`.
const EXACT_TAIL_LINE_NUMBER_THRESHOLD: u64 = 16 * 1024 * 1024;

/// Result of a `Head` or `Tail` preview operation.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HeadTailResult {
    /// `PreviewMode::Head` or `PreviewMode::Tail`.
    pub mode: PreviewMode,
    /// Requested line count `N`.
    pub requested_lines: usize,
    /// Decoded lines for display in the viewer table.
    pub lines: Vec<PreviewLine>,
    /// Exact decoded string of the selected range (byte-for-byte identical to `head -n N` / `tail -n N` for UTF-8).
    pub raw_output: String,
    /// Total file size in bytes at the time of inspection.
    pub file_size: u64,
    /// Number of bytes scanned to produce this result.
    pub bytes_scanned: u64,
    /// Detected file encoding.
    pub encoding: DetectedEncoding,
    /// Whether CRLF (`\r\n`) line endings were detected.
    pub has_crlf: bool,
    /// `true` if scanning stopped early due to the `8 MiB` [`MAX_HEAD_TAIL_BYTES`] safety limit.
    pub truncated_by_byte_limit: bool,
    /// `true` if one or more displayed lines exceeded [`MAX_LINE_BYTES`] (`64 KiB`) and were truncated with `"…"`.
    pub has_long_line_truncation: bool,
    /// `true` if malformed encoding sequences were replaced with `\u{FFFD}`.
    pub has_encoding_warnings: bool,
    /// `true` if `line_number` values in `lines` are relative to the tail slice (`1..=K`) rather than exact file line numbers.
    pub line_numbers_are_relative: bool,
}

/// Reads the first `max_lines` lines of `opened` using a `64 KiB` streaming buffer (`ACTION_SYSTEM.md` §8.1).
pub fn read_head(opened: &OpenedTextFile, max_lines: usize) -> Result<HeadTailResult, ViewerError> {
    read_head_with_limits(opened, max_lines, MAX_HEAD_TAIL_BYTES, MAX_LINE_BYTES)
}

/// Reads the first `max_lines` lines of `opened` with configurable byte and line-length safety limits.
pub fn read_head_with_limits(
    opened: &OpenedTextFile,
    max_lines: usize,
    max_total_bytes: usize,
    max_line_bytes: usize,
) -> Result<HeadTailResult, ViewerError> {
    let meta = opened
        .current_fd_metadata()
        .map_err(|source| ViewerError::Io {
            path: opened.path.clone(),
            source,
        })?;
    let file_size = meta.len();
    let content_start = opened.encoding.bom_len().min(file_size);
    let unit = opened.encoding.code_unit_bytes();

    if max_lines == 0 || file_size <= content_start {
        return Ok(HeadTailResult {
            mode: PreviewMode::Head,
            requested_lines: max_lines,
            lines: Vec::new(),
            raw_output: String::new(),
            file_size,
            bytes_scanned: 0,
            encoding: opened.encoding,
            has_crlf: opened.has_crlf,
            truncated_by_byte_limit: false,
            has_long_line_truncation: false,
            has_encoding_warnings: opened.initial_encoding_warning,
            line_numbers_are_relative: false,
        });
    }

    let max_bytes_aligned = if unit == 2 {
        max_total_bytes & !1
    } else {
        max_total_bytes
    };

    let mut collected = Vec::with_capacity(READ_BLOCK_BYTES.min(max_bytes_aligned));
    let mut buf = vec![0u8; READ_BLOCK_BYTES];
    let mut offset = content_start;
    let mut lines_found = 0usize;
    let mut truncated_by_byte_limit = false;

    while offset < file_size && lines_found < max_lines {
        let remaining_in_file = (file_size - offset) as usize;
        let remaining_budget = max_bytes_aligned.saturating_sub(collected.len());
        if remaining_budget == 0 {
            truncated_by_byte_limit = true;
            break;
        }

        let to_read = buf.len().min(remaining_in_file).min(remaining_budget);
        let n = read_at_full(&opened.file, &mut buf[..to_read], offset).map_err(|source| {
            ViewerError::Io {
                path: opened.path.clone(),
                source,
            }
        })?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];

        if let Some(end_in_chunk) = find_nth_newline_forward(
            chunk,
            opened.encoding,
            max_lines - lines_found,
            &mut lines_found,
        ) {
            collected.extend_from_slice(&chunk[..end_in_chunk]);
            break;
        }

        collected.extend_from_slice(chunk);
        offset += n as u64;

        if collected.len() >= max_bytes_aligned && offset < file_size && lines_found < max_lines {
            truncated_by_byte_limit = true;
            break;
        }
    }

    let bytes_scanned = collected.len() as u64;
    let (raw_output, had_decode_err) =
        decode_bytes(&collected, opened.encoding, truncated_by_byte_limit);
    let has_crlf = opened.has_crlf || raw_output.contains("\r\n");
    let (lines, has_long_line_truncation) = split_decoded_lines(&raw_output, 1, max_line_bytes);

    Ok(HeadTailResult {
        mode: PreviewMode::Head,
        requested_lines: max_lines,
        lines,
        raw_output,
        file_size,
        bytes_scanned,
        encoding: opened.encoding,
        has_crlf,
        truncated_by_byte_limit,
        has_long_line_truncation,
        has_encoding_warnings: opened.initial_encoding_warning || had_decode_err,
        line_numbers_are_relative: false,
    })
}

/// Reads the last `max_lines` lines of `opened` by scanning backwards from EOF in `64 KiB` blocks (`ACTION_SYSTEM.md` §8.1).
pub fn read_tail(opened: &OpenedTextFile, max_lines: usize) -> Result<HeadTailResult, ViewerError> {
    read_tail_with_limits(opened, max_lines, MAX_HEAD_TAIL_BYTES, MAX_LINE_BYTES)
}

/// Reads the last `max_lines` lines of `opened` with configurable byte and line-length safety limits.
pub fn read_tail_with_limits(
    opened: &OpenedTextFile,
    max_lines: usize,
    max_total_bytes: usize,
    max_line_bytes: usize,
) -> Result<HeadTailResult, ViewerError> {
    let meta = opened
        .current_fd_metadata()
        .map_err(|source| ViewerError::Io {
            path: opened.path.clone(),
            source,
        })?;
    let file_size = meta.len();
    let content_start = opened.encoding.bom_len().min(file_size);
    let unit = opened.encoding.code_unit_bytes();

    let effective_end = if unit == 2 {
        content_start + ((file_size - content_start) & !1)
    } else {
        file_size
    };

    if max_lines == 0 || effective_end <= content_start {
        return Ok(HeadTailResult {
            mode: PreviewMode::Tail,
            requested_lines: max_lines,
            lines: Vec::new(),
            raw_output: String::new(),
            file_size,
            bytes_scanned: 0,
            encoding: opened.encoding,
            has_crlf: opened.has_crlf,
            truncated_by_byte_limit: false,
            has_long_line_truncation: false,
            has_encoding_warnings: opened.initial_encoding_warning,
            line_numbers_are_relative: false,
        });
    }

    let max_bytes_aligned = if unit == 2 {
        (max_total_bytes & !1) as u64
    } else {
        max_total_bytes as u64
    };

    let min_offset = content_start.max(effective_end.saturating_sub(max_bytes_aligned));

    // Check if the very last code unit in the file is `\n`. A trailing newline terminates the
    // final line rather than starting a new empty line after it (`ACTION_SYSTEM.md` §8.1).
    let mut last_unit_buf = [0u8; 2];
    let last_n = read_at_full(
        &opened.file,
        &mut last_unit_buf[..unit],
        effective_end - unit as u64,
    )
    .map_err(|source| ViewerError::Io {
        path: opened.path.clone(),
        source,
    })?;
    let ends_with_nl = last_n == unit && is_newline_unit(&last_unit_buf[..unit], opened.encoding);

    let search_end = if ends_with_nl {
        effective_end.saturating_sub(unit as u64).max(content_start)
    } else {
        effective_end
    };

    let mut cursor = search_end;
    let mut newlines_found = 0usize;
    let mut start_offset = min_offset;
    let mut found_boundary = false;
    let mut block_buf = vec![0u8; READ_BLOCK_BYTES];

    while cursor > min_offset {
        let step = ((cursor - min_offset) as usize).min(READ_BLOCK_BYTES);
        let aligned_step = if unit == 2 { step & !1 } else { step };
        if aligned_step == 0 {
            break;
        }
        let block_start = cursor - aligned_step as u64;
        let n = read_at_full(&opened.file, &mut block_buf[..aligned_step], block_start).map_err(
            |source| ViewerError::Io {
                path: opened.path.clone(),
                source,
            },
        )?;
        if n == 0 {
            break;
        }
        let block = &block_buf[..n];

        if let Some(nl_offset_in_block) = find_nth_newline_backward(
            block,
            opened.encoding,
            max_lines - newlines_found,
            &mut newlines_found,
        ) {
            start_offset = block_start + (nl_offset_in_block + unit) as u64;
            found_boundary = true;
            break;
        }

        cursor = block_start;
    }

    let truncated_by_byte_limit = !found_boundary && min_offset > content_start;
    let bytes_to_read = (effective_end - start_offset) as usize;
    let mut tail_bytes = vec![0u8; bytes_to_read];
    let actual_len =
        read_at_full(&opened.file, &mut tail_bytes, start_offset).map_err(|source| {
            ViewerError::Io {
                path: opened.path.clone(),
                source,
            }
        })?;
    tail_bytes.truncate(actual_len);

    // If writing starts mid-character due to the 8 MiB byte cap on a UTF-8 file, advance to the first
    // valid UTF-8 character boundary (at most 3 bytes) so we don't emit a spurious replacement character.
    let decode_slice = if truncated_by_byte_limit && unit == 1 {
        skip_leading_utf8_continuation_bytes(&tail_bytes)
    } else {
        &tail_bytes
    };

    let (raw_output, had_decode_err) = decode_bytes(decode_slice, opened.encoding, false);
    let has_crlf = opened.has_crlf || raw_output.contains("\r\n");

    let (first_line_number, line_numbers_are_relative) = if start_offset == content_start {
        (1u64, false)
    } else if start_offset - content_start <= EXACT_TAIL_LINE_NUMBER_THRESHOLD {
        match count_newlines_in_range(&opened.file, content_start, start_offset, opened.encoding) {
            Ok(prefix_lines) => (prefix_lines + 1, false),
            Err(_) => (1, true),
        }
    } else {
        (1, true)
    };

    let (lines, has_long_line_truncation) =
        split_decoded_lines(&raw_output, first_line_number, max_line_bytes);

    Ok(HeadTailResult {
        mode: PreviewMode::Tail,
        requested_lines: max_lines,
        lines,
        raw_output,
        file_size,
        bytes_scanned: effective_end - cursor.min(start_offset),
        encoding: opened.encoding,
        has_crlf,
        truncated_by_byte_limit,
        has_long_line_truncation,
        has_encoding_warnings: opened.initial_encoding_warning || had_decode_err,
        line_numbers_are_relative,
    })
}

fn is_newline_unit(unit_bytes: &[u8], encoding: DetectedEncoding) -> bool {
    match encoding {
        DetectedEncoding::Utf8 | DetectedEncoding::Utf8Bom | DetectedEncoding::LossyUtf8 => {
            unit_bytes == [b'\n']
        }
        DetectedEncoding::Utf16Le { .. } => unit_bytes == [0x0A, 0x00],
        DetectedEncoding::Utf16Be { .. } => unit_bytes == [0x00, 0x0A],
    }
}

/// Scans `chunk` forward for up to `needed` newlines.
/// If `needed` newlines are reached inside `chunk`, returns `Some(byte_offset_after_nth_newline)`.
fn find_nth_newline_forward(
    chunk: &[u8],
    encoding: DetectedEncoding,
    needed: usize,
    lines_found: &mut usize,
) -> Option<usize> {
    if needed == 0 {
        return Some(0);
    }
    let mut remaining = needed;
    match encoding {
        DetectedEncoding::Utf8 | DetectedEncoding::Utf8Bom | DetectedEncoding::LossyUtf8 => {
            for pos in memchr_iter(b'\n', chunk) {
                *lines_found += 1;
                remaining -= 1;
                if remaining == 0 {
                    return Some(pos + 1);
                }
            }
            None
        }
        DetectedEncoding::Utf16Le { .. } => {
            for (idx, pair) in chunk.chunks_exact(2).enumerate() {
                if pair == [0x0A, 0x00] {
                    *lines_found += 1;
                    remaining -= 1;
                    if remaining == 0 {
                        return Some((idx + 1) * 2);
                    }
                }
            }
            None
        }
        DetectedEncoding::Utf16Be { .. } => {
            for (idx, pair) in chunk.chunks_exact(2).enumerate() {
                if pair == [0x00, 0x0A] {
                    *lines_found += 1;
                    remaining -= 1;
                    if remaining == 0 {
                        return Some((idx + 1) * 2);
                    }
                }
            }
            None
        }
    }
}

/// Scans `block` backwards for up to `needed` newlines.
/// If `needed` newlines are reached inside `block`, returns `Some(byte_offset_of_nth_newline_in_block)`.
fn find_nth_newline_backward(
    block: &[u8],
    encoding: DetectedEncoding,
    needed: usize,
    newlines_found: &mut usize,
) -> Option<usize> {
    if needed == 0 {
        return Some(block.len());
    }
    let mut remaining = needed;
    match encoding {
        DetectedEncoding::Utf8 | DetectedEncoding::Utf8Bom | DetectedEncoding::LossyUtf8 => {
            for pos in memrchr_iter(b'\n', block) {
                *newlines_found += 1;
                remaining -= 1;
                if remaining == 0 {
                    return Some(pos);
                }
            }
            None
        }
        DetectedEncoding::Utf16Le { .. } => {
            for (idx, pair) in block.chunks_exact(2).enumerate().rev() {
                if pair == [0x0A, 0x00] {
                    *newlines_found += 1;
                    remaining -= 1;
                    if remaining == 0 {
                        return Some(idx * 2);
                    }
                }
            }
            None
        }
        DetectedEncoding::Utf16Be { .. } => {
            for (idx, pair) in block.chunks_exact(2).enumerate().rev() {
                if pair == [0x00, 0x0A] {
                    *newlines_found += 1;
                    remaining -= 1;
                    if remaining == 0 {
                        return Some(idx * 2);
                    }
                }
            }
            None
        }
    }
}

fn count_newlines_in_range(
    file: &File,
    start: u64,
    end: u64,
    encoding: DetectedEncoding,
) -> std::io::Result<u64> {
    let mut buf = vec![0u8; READ_BLOCK_BYTES];
    let mut offset = start;
    let mut count = 0u64;
    while offset < end {
        let to_read = ((end - offset) as usize).min(buf.len());
        let n = read_at_full(file, &mut buf[..to_read], offset)?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        match encoding {
            DetectedEncoding::Utf8 | DetectedEncoding::Utf8Bom | DetectedEncoding::LossyUtf8 => {
                count += memchr_iter(b'\n', chunk).count() as u64;
            }
            DetectedEncoding::Utf16Le { .. } => {
                count += chunk.chunks_exact(2).filter(|&p| p == [0x0A, 0x00]).count() as u64;
            }
            DetectedEncoding::Utf16Be { .. } => {
                count += chunk.chunks_exact(2).filter(|&p| p == [0x00, 0x0A]).count() as u64;
            }
        }
        offset += n as u64;
    }
    Ok(count)
}

fn skip_leading_utf8_continuation_bytes(bytes: &[u8]) -> &[u8] {
    let mut i = 0usize;
    while i < bytes.len().min(3) && (bytes[i] & 0b1100_0000) == 0b1000_0000 {
        i += 1;
    }
    &bytes[i..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::path::PathBuf;
    use std::process::Command;

    fn make_temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("radivine_viewer_ht_{label}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// Naive reference implementation of `head -n N` on a UTF-8 string.
    fn naive_head(input: &str, n: usize) -> String {
        if n == 0 || input.is_empty() {
            return String::new();
        }
        let mut count = 0usize;
        for (idx, b) in input.bytes().enumerate() {
            if b == b'\n' {
                count += 1;
                if count == n {
                    return input[..=idx].to_string();
                }
            }
        }
        input.to_string()
    }

    /// Naive reference implementation of `tail -n N` on a UTF-8 string.
    fn naive_tail(input: &str, n: usize) -> String {
        if n == 0 || input.is_empty() {
            return String::new();
        }
        let bytes = input.as_bytes();
        let search_end = if bytes.last() == Some(&b'\n') {
            bytes.len() - 1
        } else {
            bytes.len()
        };
        let mut count = 0usize;
        for idx in (0..search_end).rev() {
            if bytes[idx] == b'\n' {
                count += 1;
                if count == n {
                    return input[idx + 1..].to_string();
                }
            }
        }
        input.to_string()
    }

    #[test]
    fn test_head_and_tail_corpus_matches_gnu_head_and_tail_ac_5_1() {
        let dir = make_temp_dir("corpus");

        let mut multi_block = String::new();
        for i in 1..=3500 {
            multi_block.push_str(&format!(
                "2026-10-23T00:00:00Z INFO [worker-{i:04}] processed payload line {i}\n"
            ));
        }

        let mut crlf_doc = String::new();
        for i in 1..=80 {
            crlf_doc.push_str(&format!("CRLF line number {i}\r\n"));
        }

        let cases: Vec<(&str, String)> = vec![
            ("empty.txt", String::new()),
            ("single_with_nl.txt", "only one line\n".to_string()),
            (
                "single_no_nl.txt",
                "only one line without newline".to_string(),
            ),
            ("only_newline.txt", "\n".to_string()),
            (
                "shorter_with_nl.txt",
                (1..=12).map(|i| format!("line {i}\n")).collect(),
            ),
            (
                "shorter_no_nl.txt",
                (1..=12)
                    .map(|i| format!("line {i}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            (
                "exact_50.txt",
                (1..=50).map(|i| format!("line {i}\n")).collect(),
            ),
            (
                "over_50_no_trailing_nl.txt",
                (1..=125)
                    .map(|i| format!("entry #{i}: café résumé 日本語"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            ("crlf_80.txt", crlf_doc),
            ("multi_block_3500.txt", multi_block),
        ];

        for (name, content) in cases {
            let path = dir.join(name);
            std::fs::write(&path, content.as_bytes()).expect("write corpus file");

            let opened = OpenedTextFile::open(&path).expect("open corpus file");
            let head_res = read_head(&opened, 50).expect("read_head");
            let tail_res = read_tail(&opened, 50).expect("read_tail");

            // 1. Compare against naive reference
            assert_eq!(
                head_res.raw_output,
                naive_head(&content, 50),
                "head mismatch on {name}"
            );
            assert_eq!(
                tail_res.raw_output,
                naive_tail(&content, 50),
                "tail mismatch on {name}"
            );

            // 2. Compare byte-for-byte against system `head -n 50` and `tail -n 50`
            let gnu_head = Command::new("head")
                .args(["-n", "50"])
                .arg(&path)
                .output()
                .expect("run gnu head");
            assert_eq!(
                head_res.raw_output.as_bytes(),
                gnu_head.stdout.as_slice(),
                "GNU head -n 50 mismatch on {name}"
            );

            let gnu_tail = Command::new("tail")
                .args(["-n", "50"])
                .arg(&path)
                .output()
                .expect("run gnu tail");
            assert_eq!(
                tail_res.raw_output.as_bytes(),
                gnu_tail.stdout.as_slice(),
                "GNU tail -n 50 mismatch on {name}"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_head_and_tail_utf16_le_and_be_and_long_line_truncation() {
        let dir = make_temp_dir("utf16_limits");

        // 1. UTF-16 LE with 70 lines
        let p_le = dir.join("u16le_70.txt");
        let mut le_bytes = vec![0xFF, 0xFE];
        for i in 1..=70 {
            for u in format!("UTF16-LE line {i}\n").encode_utf16() {
                le_bytes.extend_from_slice(&u.to_le_bytes());
            }
        }
        std::fs::write(&p_le, &le_bytes).expect("write u16le");
        let f_le = OpenedTextFile::open(&p_le).expect("open u16le");
        let head_le = read_head(&f_le, 10).expect("head u16le");
        assert_eq!(head_le.lines.len(), 10);
        assert_eq!(head_le.lines[0].text, "UTF16-LE line 1");
        assert_eq!(head_le.lines[9].text, "UTF16-LE line 10");

        let tail_le = read_tail(&f_le, 10).expect("tail u16le");
        assert_eq!(tail_le.lines.len(), 10);
        assert_eq!(tail_le.lines[0].text, "UTF16-LE line 61");
        assert_eq!(tail_le.lines[0].line_number, 61);
        assert_eq!(tail_le.lines[9].text, "UTF16-LE line 70");
        assert_eq!(tail_le.lines[9].line_number, 70);

        // 2. Very long line (> 64 KiB) + 8 MiB byte cap truncation banner
        let p_long = dir.join("long_line.txt");
        let long_str = format!("{}\nsecond line\n", "x".repeat(80_000));
        std::fs::write(&p_long, long_str.as_bytes()).expect("write long");
        let f_long = OpenedTextFile::open(&p_long).expect("open long");
        let head_long = read_head(&f_long, 50).expect("head long");
        assert_eq!(head_long.lines.len(), 2);
        assert!(head_long.has_long_line_truncation);
        assert!(head_long.lines[0].line_truncated);
        assert!(head_long.lines[0].text.ends_with('…'));
        assert_eq!(head_long.lines[1].text, "second line");

        // Test byte limit truncation
        let head_capped =
            read_head_with_limits(&f_long, 50, 32 * 1024, MAX_LINE_BYTES).expect("head capped");
        assert!(head_capped.truncated_by_byte_limit);

        let tail_capped =
            read_tail_with_limits(&f_long, 50, 32 * 1024, MAX_LINE_BYTES).expect("tail capped");
        assert!(tail_capped.truncated_by_byte_limit);

        let _ = std::fs::remove_dir_all(&dir);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(40))]
        #[test]
        fn prop_head_and_tail_match_naive_reference(
            lines in prop::collection::vec("[a-zA-Z0-9 _.-]{0,120}", 0..160),
            trailing_nl in any::<bool>(),
            use_crlf in any::<bool>(),
            n in 1usize..65,
        ) {
            let dir = make_temp_dir("prop");
            let sep = if use_crlf { "\r\n" } else { "\n" };
            let mut content = lines.join(sep);
            if trailing_nl && !lines.is_empty() {
                content.push_str(sep);
            }

            let path = dir.join("prop_input.txt");
            std::fs::write(&path, content.as_bytes()).expect("write prop file");
            let opened = OpenedTextFile::open(&path).expect("open prop file");

            let h = read_head(&opened, n).expect("read_head");
            let t = read_tail(&opened, n).expect("read_tail");

            prop_assert_eq!(h.raw_output, naive_head(&content, n));
            prop_assert_eq!(t.raw_output, naive_tail(&content, n));

            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
