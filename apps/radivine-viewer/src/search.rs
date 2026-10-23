//! Streaming file search engine (`Ctrl+F`) with `memchr::memmem` chunk scanning, boundary overlap,
//! case-sensitivity toggle, and next/previous navigation (`ACTION_SYSTEM.md` §8.1, `AC-5.2`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use memchr::memchr_iter;
use memchr::memmem::Finder;

use crate::file_io::{
    DetectedEncoding, OpenedTextFile, PreviewLine, ViewerError, decode_bytes, read_at_full,
};

/// Chunk size used for streaming file search (`1 MiB`, `ACTION_SYSTEM.md` §8.1).
pub const SEARCH_CHUNK_BYTES: usize = 1024 * 1024;

/// Maximum number of individual match entries retained in memory for jump navigation (`1_000_000`).
/// Even if a search exceeds this cap, `total_occurrences` and `matching_line_count` remain exact.
pub const MAX_STORED_MATCHES: usize = 1_000_000;

/// Single search match location.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    /// 0-based line index in the document (or in the Head/Tail slice).
    pub line_index: usize,
    /// 1-based display line number.
    pub line_number: u64,
    /// Byte offset of the match in the file (or 0 for in-memory slice search).
    pub byte_offset: u64,
}

/// Snapshot of current search results and streaming progress.
#[derive(Clone, Debug, Default)]
pub struct SearchSnapshot {
    /// Query string that produced these results.
    pub query: String,
    /// Whether this search was case-sensitive.
    pub case_sensitive: bool,
    /// Stored match locations (capped at [`MAX_STORED_MATCHES`]).
    pub matches: Vec<SearchMatch>,
    /// Total number of occurrences found across the file.
    pub total_occurrences: usize,
    /// Total number of distinct lines containing at least one match (matches `grep -c`).
    pub matching_line_count: usize,
    /// Bytes scanned so far.
    pub scanned_bytes: u64,
    /// Total file bytes to scan.
    pub file_size: u64,
    /// `true` once the search worker has finished scanning the entire file.
    pub complete: bool,
}

/// Manages background streaming search and active match navigation (`Enter` / `Shift+Enter`).
pub struct SearchController {
    shared: Arc<Mutex<SearchSnapshot>>,
    cancel: Option<Arc<AtomicBool>>,
    active_match_idx: Option<usize>,
}

impl Default for SearchController {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchController {
    /// Creates an idle [`SearchController`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(SearchSnapshot {
                complete: true,
                ..SearchSnapshot::default()
            })),
            cancel: None,
            active_match_idx: None,
        }
    }

    /// Cancels any running search and clears results.
    pub fn clear(&mut self) {
        if let Some(flag) = self.cancel.take() {
            flag.store(true, Ordering::SeqCst);
        }
        self.active_match_idx = None;
        if let Ok(mut guard) = self.shared.lock() {
            *guard = SearchSnapshot {
                complete: true,
                ..SearchSnapshot::default()
            };
        }
    }

    /// Runs an immediate in-memory search across `lines` (used in `Head` and `Tail` modes).
    pub fn search_in_memory_lines(
        &mut self,
        lines: &[PreviewLine],
        query: &str,
        case_sensitive: bool,
    ) {
        self.clear();
        if query.is_empty() {
            return;
        }

        let mut matches = Vec::new();
        let mut total_occurrences = 0usize;
        let mut matching_line_count = 0usize;
        let needle_folded = if case_sensitive {
            String::new()
        } else {
            query.to_lowercase()
        };

        for (line_idx, line) in lines.iter().enumerate() {
            let count = count_line_occurrences(&line.text, query, &needle_folded, case_sensitive);
            if count > 0 {
                matching_line_count += 1;
                total_occurrences += count;
                for _ in 0..count {
                    if matches.len() < MAX_STORED_MATCHES {
                        matches.push(SearchMatch {
                            line_index: line_idx,
                            line_number: line.line_number,
                            byte_offset: 0,
                        });
                    }
                }
            }
        }

        self.active_match_idx = if matches.is_empty() { None } else { Some(0) };
        if let Ok(mut guard) = self.shared.lock() {
            *guard = SearchSnapshot {
                query: query.to_string(),
                case_sensitive,
                matches,
                total_occurrences,
                matching_line_count,
                scanned_bytes: 0,
                file_size: 0,
                complete: true,
            };
        }
    }

    /// Starts a background streaming search over `opened` in `1 MiB` chunks (`ACTION_SYSTEM.md` §8.1).
    pub fn start_file_search(
        &mut self,
        opened: &OpenedTextFile,
        query: &str,
        case_sensitive: bool,
        repaint_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<(), ViewerError> {
        self.clear();
        if query.is_empty() {
            return Ok(());
        }

        let worker_file = opened.file.try_clone().map_err(|source| ViewerError::Io {
            path: opened.path.clone(),
            source,
        })?;
        let file_size = opened
            .current_fd_metadata()
            .map(|m| m.len())
            .unwrap_or(opened.initial_size);
        let encoding = opened.encoding;
        let query_owned = query.to_string();

        let shared = Arc::new(Mutex::new(SearchSnapshot {
            query: query_owned.clone(),
            case_sensitive,
            matches: Vec::new(),
            total_occurrences: 0,
            matching_line_count: 0,
            scanned_bytes: 0,
            file_size,
            complete: false,
        }));
        let cancel = Arc::new(AtomicBool::new(false));

        self.shared = Arc::clone(&shared);
        self.cancel = Some(Arc::clone(&cancel));
        self.active_match_idx = None;

        let _ = thread::Builder::new()
            .name("radivine-viewer-search".to_string())
            .spawn(move || {
                run_streaming_search_worker(
                    worker_file,
                    file_size,
                    encoding,
                    &query_owned,
                    case_sensitive,
                    &shared,
                    &cancel,
                    repaint_hook,
                );
            });

        Ok(())
    }

    /// Returns a snapshot of the current search state.
    #[must_use]
    pub fn snapshot(&self) -> SearchSnapshot {
        self.shared.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Blocks until the active background search finishes and returns the final [`SearchSnapshot`].
    pub fn wait_until_complete(&mut self) -> SearchSnapshot {
        loop {
            let snap = self.snapshot();
            if snap.complete {
                if self.active_match_idx.is_none() && !snap.matches.is_empty() {
                    self.active_match_idx = Some(0);
                }
                return snap;
            }
            thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Returns the currently selected match index (`0..matches.len()`) and its [`SearchMatch`].
    pub fn active_match(&mut self) -> Option<(usize, SearchMatch)> {
        let snap = self.snapshot();
        if snap.matches.is_empty() {
            self.active_match_idx = None;
            return None;
        }
        let idx = match self.active_match_idx {
            Some(i) if i < snap.matches.len() => i,
            _ => {
                self.active_match_idx = Some(0);
                0
            }
        };
        Some((idx, snap.matches[idx]))
    }

    /// Advances to the next search match (`Enter`), wrapping around to `0` at the end.
    pub fn next_match(&mut self) -> Option<SearchMatch> {
        let snap = self.snapshot();
        if snap.matches.is_empty() {
            self.active_match_idx = None;
            return None;
        }
        let next = match self.active_match_idx {
            Some(i) => (i + 1) % snap.matches.len(),
            None => 0,
        };
        self.active_match_idx = Some(next);
        Some(snap.matches[next])
    }

    /// Moves to the previous search match (`Shift+Enter`), wrapping around to the last match.
    pub fn prev_match(&mut self) -> Option<SearchMatch> {
        let snap = self.snapshot();
        if snap.matches.is_empty() {
            self.active_match_idx = None;
            return None;
        }
        let prev = match self.active_match_idx {
            Some(0) | None => snap.matches.len() - 1,
            Some(i) => i - 1,
        };
        self.active_match_idx = Some(prev);
        Some(snap.matches[prev])
    }
}

impl Drop for SearchController {
    fn drop(&mut self) {
        if let Some(flag) = self.cancel.take() {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

/// Counts occurrences of `query` in a single decoded line (`case_sensitive` or folded).
#[must_use]
pub fn count_line_occurrences(
    line: &str,
    query: &str,
    query_lowercase: &str,
    case_sensitive: bool,
) -> usize {
    if query.is_empty() || line.is_empty() {
        return 0;
    }
    if case_sensitive {
        Finder::new(query.as_bytes())
            .find_iter(line.as_bytes())
            .count()
    } else if query.is_ascii() && line.is_ascii() {
        let folded_line = line.to_ascii_lowercase();
        let folded_needle = query.to_ascii_lowercase();
        Finder::new(folded_needle.as_bytes())
            .find_iter(folded_line.as_bytes())
            .count()
    } else {
        let folded_line = line.to_lowercase();
        folded_line.matches(query_lowercase).count()
    }
}

/// Finds all `(start_byte, end_byte)` highlight ranges of `query` inside `line` for UI rendering.
#[must_use]
pub fn find_highlight_ranges_in_line(
    line: &str,
    query: &str,
    case_sensitive: bool,
) -> Vec<(usize, usize)> {
    if query.is_empty() || line.is_empty() {
        return Vec::new();
    }
    if case_sensitive {
        let pat = query.as_bytes();
        Finder::new(pat)
            .find_iter(line.as_bytes())
            .map(|pos| (pos, pos + pat.len()))
            .collect()
    } else if query.is_ascii() && line.is_ascii() {
        let folded_line = line.to_ascii_lowercase();
        let folded_pat = query.to_ascii_lowercase();
        let len = folded_pat.len();
        Finder::new(folded_pat.as_bytes())
            .find_iter(folded_line.as_bytes())
            .map(|pos| (pos, pos + len))
            .collect()
    } else {
        // Unicode case-insensitive fallback: match char-by-char when lowercase lengths match
        let needle_lower = query.to_lowercase();
        if needle_lower.is_empty() {
            return Vec::new();
        }
        let mut ranges = Vec::new();
        for (byte_idx, _) in line.char_indices() {
            let tail = &line[byte_idx..];
            for (end_rel, ch) in tail.char_indices() {
                let candidate_end = end_rel + ch.len_utf8();
                if candidate_end > query.len() + 8 {
                    break;
                }
                if tail[..candidate_end].to_lowercase() == needle_lower {
                    ranges.push((byte_idx, byte_idx + candidate_end));
                    break;
                }
            }
        }
        ranges
    }
}

#[allow(clippy::too_many_arguments)]
fn run_streaming_search_worker(
    file: std::fs::File,
    file_size: u64,
    encoding: DetectedEncoding,
    query: &str,
    case_sensitive: bool,
    shared: &Arc<Mutex<SearchSnapshot>>,
    cancel: &Arc<AtomicBool>,
    repaint_hook: Option<Arc<dyn Fn() + Send + Sync>>,
) {
    let use_byte_memmem = !encoding.is_utf16() && (case_sensitive || query.is_ascii());
    if use_byte_memmem {
        run_memmem_byte_stream_search(
            &file,
            file_size,
            encoding,
            query,
            case_sensitive,
            shared,
            cancel,
            repaint_hook,
        );
    } else {
        run_decoded_line_stream_search(
            &file,
            file_size,
            encoding,
            query,
            case_sensitive,
            shared,
            cancel,
            repaint_hook,
        );
    }
}

/// High-throughput streaming search using `memchr::memmem::Finder` over `1 MiB` chunks with
/// `pattern.len() - 1` boundary overlap (`ACTION_SYSTEM.md` §8.1).
#[allow(clippy::too_many_arguments)]
fn run_memmem_byte_stream_search(
    file: &std::fs::File,
    file_size: u64,
    encoding: DetectedEncoding,
    query: &str,
    case_sensitive: bool,
    shared: &Arc<Mutex<SearchSnapshot>>,
    cancel: &Arc<AtomicBool>,
    repaint_hook: Option<Arc<dyn Fn() + Send + Sync>>,
) {
    let content_start = encoding.bom_len().min(file_size);
    let needle_bytes: Vec<u8> = if case_sensitive {
        query.as_bytes().to_vec()
    } else {
        query.to_ascii_lowercase().into_bytes()
    };
    if needle_bytes.is_empty() {
        if let Ok(mut g) = shared.lock() {
            g.complete = true;
        }
        return;
    }

    let finder = Finder::new(&needle_bytes);
    let pat_len = needle_bytes.len();
    let max_overlap = pat_len.saturating_sub(1);

    let mut work_buf = Vec::with_capacity(SEARCH_CHUNK_BYTES + max_overlap);
    let mut fold_buf = if case_sensitive {
        Vec::new()
    } else {
        Vec::with_capacity(SEARCH_CHUNK_BYTES + max_overlap)
    };
    let mut read_buf = vec![0u8; SEARCH_CHUNK_BYTES];

    let mut offset = content_start;
    let mut current_line = 0usize;
    let mut last_matched_line: Option<usize> = None;
    let mut pending_matches = Vec::new();
    let mut total_occurrences = 0usize;
    let mut matching_line_count = 0usize;
    let mut bytes_since_publish = 0usize;

    while offset < file_size {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let to_read = ((file_size - offset) as usize).min(read_buf.len());
        let Ok(n) = read_at_full(file, &mut read_buf[..to_read], offset) else {
            break;
        };
        if n == 0 {
            break;
        }

        let carry_len = work_buf.len();
        work_buf.extend_from_slice(&read_buf[..n]);

        let search_slice: &[u8] = if case_sensitive {
            &work_buf
        } else {
            fold_buf.clear();
            fold_buf.extend(work_buf.iter().map(u8::to_ascii_lowercase));
            &fold_buf
        };

        // `nl_cursor` tracks how far into `work_buf[carry_len..]` we have advanced `current_line`.
        // Because `work_buf[..carry_len]` never contains `\n` (trimmed below), every byte in
        // `0..carry_len` belongs to `current_line` at the start of this iteration.
        let mut nl_cursor = carry_len;

        for match_pos in finder.find_iter(search_slice) {
            let match_end = match_pos + pat_len;
            if match_end <= carry_len {
                // Already reported in the previous chunk
                continue;
            }
            if match_pos > nl_cursor {
                current_line += memchr_iter(b'\n', &work_buf[nl_cursor..match_pos]).count();
                nl_cursor = match_pos;
            }
            total_occurrences += 1;
            if last_matched_line != Some(current_line) {
                last_matched_line = Some(current_line);
                matching_line_count += 1;
            }
            let file_byte_offset = (offset - carry_len as u64) + match_pos as u64;
            pending_matches.push(SearchMatch {
                line_index: current_line,
                line_number: (current_line + 1) as u64,
                byte_offset: file_byte_offset,
            });
        }

        // Advance `current_line` across the rest of `new_bytes` (`nl_cursor..work_buf.len()`)
        if nl_cursor < work_buf.len() {
            current_line += memchr_iter(b'\n', &work_buf[nl_cursor..]).count();
        }

        // Keep at most `max_overlap` bytes from the end of `work_buf`, and trim after any `\n`
        // since single-line search queries never cross a newline.
        if max_overlap == 0 {
            work_buf.clear();
        } else {
            let tail_start = work_buf.len().saturating_sub(max_overlap);
            let after_nl_start = match memchr::memrchr(b'\n', &work_buf[tail_start..]) {
                Some(rel_nl) => tail_start + rel_nl + 1,
                None => tail_start,
            };
            work_buf.drain(0..after_nl_start);
        }

        offset += n as u64;
        bytes_since_publish += n;

        if bytes_since_publish >= 16 * 1024 * 1024 || offset >= file_size {
            bytes_since_publish = 0;
            if let Ok(mut guard) = shared.lock() {
                let room = MAX_STORED_MATCHES.saturating_sub(guard.matches.len());
                if room > 0 {
                    let take = pending_matches.len().min(room);
                    guard.matches.extend_from_slice(&pending_matches[..take]);
                }
                pending_matches.clear();
                guard.total_occurrences = total_occurrences;
                guard.matching_line_count = matching_line_count;
                guard.scanned_bytes = offset;
                if offset >= file_size {
                    guard.complete = true;
                }
            }
            if let Some(ref cb) = repaint_hook {
                cb();
            }
        }
    }

    if let Ok(mut guard) = shared.lock() {
        let room = MAX_STORED_MATCHES.saturating_sub(guard.matches.len());
        if room > 0 && !pending_matches.is_empty() {
            let take = pending_matches.len().min(room);
            guard.matches.extend_from_slice(&pending_matches[..take]);
        }
        guard.total_occurrences = total_occurrences;
        guard.matching_line_count = matching_line_count;
        guard.scanned_bytes = offset;
        guard.complete = true;
    }
    if let Some(ref cb) = repaint_hook {
        cb();
    }
}

/// Streaming search for UTF-16 LE/BE or non-ASCII case-insensitive Unicode queries.
#[allow(clippy::too_many_arguments)]
fn run_decoded_line_stream_search(
    file: &std::fs::File,
    file_size: u64,
    encoding: DetectedEncoding,
    query: &str,
    case_sensitive: bool,
    shared: &Arc<Mutex<SearchSnapshot>>,
    cancel: &Arc<AtomicBool>,
    repaint_hook: Option<Arc<dyn Fn() + Send + Sync>>,
) {
    let content_start = encoding.bom_len().min(file_size);
    let unit = encoding.code_unit_bytes();
    let query_lower = if case_sensitive {
        String::new()
    } else {
        query.to_lowercase()
    };

    let mut offset = content_start;
    let mut read_buf = vec![0u8; SEARCH_CHUNK_BYTES];
    let mut raw_line = Vec::new();
    let mut current_line = 0usize;
    let mut total_occurrences = 0usize;
    let mut matching_line_count = 0usize;
    let mut matches = Vec::new();

    while offset < file_size {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let rem = (file_size - offset) as usize;
        let to_read = rem.min(read_buf.len());
        let aligned = if unit == 2 { to_read & !1 } else { to_read };
        if aligned == 0 {
            break;
        }
        let Ok(n) = read_at_full(file, &mut read_buf[..aligned], offset) else {
            break;
        };
        if n == 0 {
            break;
        }
        let chunk = &read_buf[..n];
        let mut cursor = 0usize;
        while cursor < chunk.len() {
            let slice = &chunk[cursor..];
            let nl_pos = match encoding {
                DetectedEncoding::Utf16Le { .. } => slice
                    .chunks_exact(2)
                    .position(|p| p == [0x0A, 0x00])
                    .map(|i| i * 2),
                DetectedEncoding::Utf16Be { .. } => slice
                    .chunks_exact(2)
                    .position(|p| p == [0x00, 0x0A])
                    .map(|i| i * 2),
                _ => memchr::memchr(b'\n', slice),
            };
            if let Some(pos) = nl_pos {
                raw_line.extend_from_slice(&slice[..pos]);
                let (decoded, _) = decode_bytes(&raw_line, encoding, false);
                let clean = decoded.strip_suffix('\r').unwrap_or(&decoded);
                let occ = count_line_occurrences(clean, query, &query_lower, case_sensitive);
                if occ > 0 {
                    matching_line_count += 1;
                    total_occurrences += occ;
                    for _ in 0..occ {
                        if matches.len() < MAX_STORED_MATCHES {
                            matches.push(SearchMatch {
                                line_index: current_line,
                                line_number: (current_line + 1) as u64,
                                byte_offset: offset + cursor as u64,
                            });
                        }
                    }
                }
                raw_line.clear();
                current_line += 1;
                cursor += pos + unit;
            } else {
                raw_line.extend_from_slice(slice);
                break;
            }
        }
        offset += n as u64;
    }

    if !raw_line.is_empty() {
        let (decoded, _) = decode_bytes(&raw_line, encoding, false);
        let clean = decoded.strip_suffix('\r').unwrap_or(&decoded);
        let occ = count_line_occurrences(clean, query, &query_lower, case_sensitive);
        if occ > 0 {
            matching_line_count += 1;
            total_occurrences += occ;
            for _ in 0..occ {
                if matches.len() < MAX_STORED_MATCHES {
                    matches.push(SearchMatch {
                        line_index: current_line,
                        line_number: (current_line + 1) as u64,
                        byte_offset: offset,
                    });
                }
            }
        }
    }

    if let Ok(mut guard) = shared.lock() {
        guard.matches = matches;
        guard.total_occurrences = total_occurrences;
        guard.matching_line_count = matching_line_count;
        guard.scanned_bytes = offset;
        guard.complete = true;
    }
    if let Some(ref cb) = repaint_hook {
        cb();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    fn make_temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "radivine_viewer_search_{label}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn test_streaming_search_matches_grep_c_and_boundary_overlap_ac_5_2() {
        let dir = make_temp_dir("grep_c");
        let path = dir.join("multi_megabyte_search.log");

        // Build a ~2.5 MiB file (> 2 x 1 MiB chunks) and place a needle right across the 1 MiB boundary
        let mut bytes = Vec::with_capacity(2_600_000);
        let mut line_no = 1usize;
        while bytes.len() < SEARCH_CHUNK_BYTES - 5 {
            let line = format!("line {line_no:06}: normal telemetry log message\n");
            if bytes.len() + line.len() >= SEARCH_CHUNK_BYTES - 5 {
                let pad_needed = (SEARCH_CHUNK_BYTES - 5) - bytes.len();
                bytes.extend(std::iter::repeat_n(b'x', pad_needed));
                // Now we are at `SEARCH_CHUNK_BYTES - 5`. Write `"RADIVINE_TOKEN"` so it straddles `1048576`!
                bytes.extend_from_slice(b"RADIVINE_TOKEN end of boundary line\n");
                line_no += 1;
                break;
            }
            bytes.extend_from_slice(line.as_bytes());
            line_no += 1;
        }

        for i in 0..10_000 {
            if i % 97 == 0 {
                bytes.extend_from_slice(
                    format!("line {line_no:06}: hit RADIVINE_TOKEN and again RADIVINE_TOKEN\n")
                        .as_bytes(),
                );
            } else if i % 131 == 0 {
                bytes.extend_from_slice(
                    format!("line {line_no:06}: lowercase radivine_token entry\n").as_bytes(),
                );
            } else {
                bytes.extend_from_slice(
                    format!("line {line_no:06}: routine background entry\n").as_bytes(),
                );
            }
            line_no += 1;
        }

        std::fs::write(&path, &bytes).expect("write search log");
        let opened = OpenedTextFile::open(&path).expect("open");

        // 1. Case-sensitive search for `RADIVINE_TOKEN`
        let mut ctrl = SearchController::new();
        ctrl.start_file_search(&opened, "RADIVINE_TOKEN", true, None)
            .expect("start search");
        let snap_cs = ctrl.wait_until_complete();

        let grep_cs = Command::new("grep")
            .args(["-c", "RADIVINE_TOKEN"])
            .arg(&path)
            .output()
            .expect("run grep -c");
        let grep_cs_count: usize = String::from_utf8_lossy(&grep_cs.stdout)
            .trim()
            .parse()
            .expect("parse grep -c");

        assert_eq!(snap_cs.matching_line_count, grep_cs_count);
        // Every `i % 97 == 0` line has 2 occurrences, plus 1 on the straddling boundary line
        assert!(snap_cs.total_occurrences > snap_cs.matching_line_count);

        // Verify next/prev navigation wraps around cleanly
        let first = ctrl.active_match().expect("active match").1;
        let second = ctrl.next_match().expect("next match");
        assert!(second.line_index >= first.line_index);
        let back = ctrl.prev_match().expect("prev match");
        assert_eq!(back, first);

        // 2. Case-insensitive search (`case_sensitive = false`) compared against `grep -i -c`
        ctrl.start_file_search(&opened, "RADIVINE_TOKEN", false, None)
            .expect("start case-insensitive search");
        let snap_ci = ctrl.wait_until_complete();

        let grep_ci = Command::new("grep")
            .args(["-i", "-c", "RADIVINE_TOKEN"])
            .arg(&path)
            .output()
            .expect("run grep -i -c");
        let grep_ci_count: usize = String::from_utf8_lossy(&grep_ci.stdout)
            .trim()
            .parse()
            .expect("parse grep -i -c");

        assert_eq!(snap_ci.matching_line_count, grep_ci_count);
        assert!(snap_ci.matching_line_count > snap_cs.matching_line_count);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
