//! Full Preview engine with background sparse line indexing (`SPARSE_STRIDE = 1024`), chunked `pread`
//! (`FileExt::read_at`) virtualized row loading, and concurrent truncation/modification safety
//! (`ACTION_SYSTEM.md` §8.1, `AC-5.2`, `AC-5.4`).
//!
//! Safety guarantees:
//! - Never uses `mmap` (prevents `SIGBUS` when an open file is concurrently truncated).
//! - Displays the first `64 KiB` immediately (`< 5 ms`) while indexing multi-gigabyte files in the background.
//! - Stores only 1 `u64` byte offset per `1024` lines (`~160 KiB` RAM for 20 million lines).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Instant, SystemTime};

use memchr::memchr_iter;

use crate::file_io::{
    DetectedEncoding, MAX_LINE_BYTES, OpenedTextFile, PreviewLine, ViewerError, decode_bytes,
    format_byte_size, read_at_full, truncate_line_for_display,
};

/// Number of lines between sparse index checkpoints (`ACTION_SYSTEM.md` §8.1).
pub const SPARSE_STRIDE: usize = 1024;

/// Bytes read synchronously on open so the first screen renders immediately (`64 KiB`, `ACTION_SYSTEM.md` §8.1).
pub const INITIAL_PREVIEW_BYTES: usize = 64 * 1024;

/// Chunk size used by the background line indexer (`1 MiB`).
const INDEX_CHUNK_BYTES: usize = 1024 * 1024;

/// Chunk size used when resolving visible rows from the nearest sparse checkpoint (`64 KiB`).
const FETCH_CHUNK_BYTES: usize = 64 * 1024;

/// Number of decoded lines cached in the sliding viewport window (`256` lines).
const WINDOW_CACHE_LINES: usize = 256;

/// Shared progress and sparse checkpoint table updated by the background indexer thread.
#[derive(Clone, Debug)]
pub struct SparseIndexSnapshot {
    /// Byte offset of every `SPARSE_STRIDE`-th line (`checkpoints[k]` = byte offset of 0-based line `k * 1024`).
    pub checkpoints: Vec<u64>,
    /// Number of bytes indexed so far (from `0` to `file_size`).
    pub indexed_bytes: u64,
    /// Total file size in bytes when indexing started.
    pub file_size: u64,
    /// Total number of lines discovered in `0..indexed_bytes`.
    pub total_lines: usize,
    /// `true` once the background indexer has reached the end of the file.
    pub complete: bool,
    /// `true` if the background indexer detected premature EOF (concurrent file truncation).
    pub truncated_during_index: bool,
    /// Optional I/O error encountered during background indexing.
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
struct CachedLineWindow {
    start_line: usize,
    lines: Vec<PreviewLine>,
}

/// Interactive Full Preview document state with background sparse line index and sliding row cache.
pub struct FullDocument {
    opened: OpenedTextFile,
    shared_index: Arc<Mutex<SparseIndexSnapshot>>,
    cancel_index: Arc<AtomicBool>,
    cached_window: Option<CachedLineWindow>,
    known_file_size: u64,
    known_mtime: Option<SystemTime>,
    last_disk_check: Instant,
    /// Warning/error banner set if the file was truncated while open (`AC-5.4`).
    pub truncated_banner: Option<String>,
    /// Warning banner set if the file was modified/appended or removed while open.
    pub modified_banner: Option<String>,
    /// Whether any fetched line exceeded `64 KiB` and was truncated with `"…"`.
    pub has_long_line_truncation: bool,
    /// Whether any fetched chunk contained malformed encoding sequences.
    pub has_encoding_warnings: bool,
}

impl FullDocument {
    /// Opens `opened` for Full Preview, synchronously indexing the first `64 KiB` for immediate first-frame
    /// display and spawning a background thread to build the full sparse line index (`ACTION_SYSTEM.md` §8.1).
    pub fn new(opened: OpenedTextFile) -> Result<Self, ViewerError> {
        Self::with_repaint_hook(opened, None)
    }

    /// Opens `opened` for Full Preview with an optional wakeup callback invoked as background indexing progresses.
    pub fn with_repaint_hook(
        opened: OpenedTextFile,
        repaint_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<Self, ViewerError> {
        let file_size = opened.initial_size;
        let content_start = opened.encoding.bom_len().min(file_size);
        let unit = opened.encoding.code_unit_bytes();

        // Synchronously read and index the first `64 KiB` so the initial viewport is ready immediately (< 2 ms).
        let initial_to_read = ((file_size - content_start) as usize).min(INITIAL_PREVIEW_BYTES);
        let aligned_initial = if unit == 2 {
            initial_to_read & !1
        } else {
            initial_to_read
        };

        let mut init_buf = vec![0u8; aligned_initial];
        let n = read_at_full(&opened.file, &mut init_buf, content_start).map_err(|source| {
            ViewerError::Io {
                path: opened.path.clone(),
                source,
            }
        })?;
        init_buf.truncate(n);

        let mut checkpoints = vec![content_start];
        let mut newline_count = 0usize;
        let mut last_byte_was_nl = false;

        scan_newlines_for_index(
            &init_buf,
            content_start,
            opened.encoding,
            &mut newline_count,
            &mut checkpoints,
            &mut last_byte_was_nl,
        );

        let initial_indexed_end = content_start + n as u64;
        let is_complete = initial_indexed_end >= file_size;
        let initial_total_lines = compute_total_lines(
            initial_indexed_end,
            content_start,
            newline_count,
            last_byte_was_nl,
        );

        let snapshot = SparseIndexSnapshot {
            checkpoints,
            indexed_bytes: initial_indexed_end,
            file_size,
            total_lines: initial_total_lines,
            complete: is_complete,
            truncated_during_index: false,
            error: None,
        };

        let shared_index = Arc::new(Mutex::new(snapshot));
        let cancel_index = Arc::new(AtomicBool::new(false));

        let initial_encoding_warning = opened.initial_encoding_warning;
        let initial_mtime = opened.initial_mtime;

        let mut doc = Self {
            opened,
            shared_index,
            cancel_index,
            cached_window: None,
            known_file_size: file_size,
            known_mtime: initial_mtime,
            last_disk_check: Instant::now(),
            truncated_banner: None,
            modified_banner: None,
            has_long_line_truncation: false,
            has_encoding_warnings: initial_encoding_warning,
        };

        // Pre-warm the first window of decoded lines from the initial 64 KiB
        let _ = doc.ensure_window_cached(0);

        if !is_complete {
            doc.spawn_background_indexer(
                initial_indexed_end,
                newline_count,
                last_byte_was_nl,
                repaint_hook,
            )?;
        }

        Ok(doc)
    }

    fn spawn_background_indexer(
        &self,
        start_offset: u64,
        initial_newlines: usize,
        initial_last_nl: bool,
        repaint_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<(), ViewerError> {
        let worker_file = self
            .opened
            .file
            .try_clone()
            .map_err(|source| ViewerError::Io {
                path: self.opened.path.clone(),
                source,
            })?;
        let shared = Arc::clone(&self.shared_index);
        let cancel = Arc::clone(&self.cancel_index);
        let encoding = self.opened.encoding;
        let content_start = encoding.bom_len();
        let file_size = self.known_file_size;

        let _ = thread::Builder::new()
            .name("radivine-viewer-indexer".to_string())
            .spawn(move || {
                let mut buf = vec![0u8; INDEX_CHUNK_BYTES];
                let mut offset = start_offset;
                let mut newline_count = initial_newlines;
                let mut last_was_nl = initial_last_nl;
                let mut pending_checkpoints = Vec::new();
                let mut bytes_since_publish = 0usize;

                while offset < file_size {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let remaining = (file_size - offset) as usize;
                    let to_read = remaining.min(buf.len());
                    let aligned_read = if encoding.code_unit_bytes() == 2 {
                        to_read & !1
                    } else {
                        to_read
                    };
                    if aligned_read == 0 {
                        break;
                    }

                    match read_at_full(&worker_file, &mut buf[..aligned_read], offset) {
                        Ok(0) => {
                            // Premature EOF: file was truncated while background indexing was running!
                            if let Ok(mut guard) = shared.lock() {
                                guard.checkpoints.append(&mut pending_checkpoints);
                                guard.indexed_bytes = offset;
                                guard.file_size = offset;
                                guard.total_lines = compute_total_lines(
                                    offset,
                                    content_start,
                                    newline_count,
                                    last_was_nl,
                                );
                                guard.truncated_during_index = true;
                                guard.complete = true;
                            }
                            if let Some(ref cb) = repaint_hook {
                                cb();
                            }
                            return;
                        }
                        Ok(n) => {
                            let chunk = &buf[..n];
                            scan_newlines_for_index(
                                chunk,
                                offset,
                                encoding,
                                &mut newline_count,
                                &mut pending_checkpoints,
                                &mut last_was_nl,
                            );
                            offset += n as u64;
                            bytes_since_publish += n;

                            // Publish progress every 16 MiB or on completion
                            if bytes_since_publish >= 16 * 1024 * 1024 || offset >= file_size {
                                bytes_since_publish = 0;
                                if let Ok(mut guard) = shared.lock() {
                                    guard.checkpoints.append(&mut pending_checkpoints);
                                    guard.indexed_bytes = offset;
                                    guard.total_lines = compute_total_lines(
                                        offset,
                                        content_start,
                                        newline_count,
                                        last_was_nl,
                                    );
                                    if offset >= file_size {
                                        guard.complete = true;
                                    }
                                }
                                if let Some(ref cb) = repaint_hook {
                                    cb();
                                }
                            }
                        }
                        Err(err) => {
                            if let Ok(mut guard) = shared.lock() {
                                guard.checkpoints.append(&mut pending_checkpoints);
                                guard.indexed_bytes = offset;
                                guard.error = Some(err.to_string());
                                guard.complete = true;
                            }
                            if let Some(ref cb) = repaint_hook {
                                cb();
                            }
                            return;
                        }
                    }
                }

                if let Ok(mut guard) = shared.lock()
                    && !guard.complete
                {
                    guard.checkpoints.append(&mut pending_checkpoints);
                    guard.indexed_bytes = offset;
                    guard.total_lines =
                        compute_total_lines(offset, content_start, newline_count, last_was_nl);
                    guard.complete = true;
                }
                if let Some(ref cb) = repaint_hook {
                    cb();
                }
            });

        Ok(())
    }

    /// Returns a snapshot of the current sparse line index and indexing progress.
    #[must_use]
    pub fn index_snapshot(&self) -> SparseIndexSnapshot {
        self.shared_index
            .lock()
            .map(|g| g.clone())
            .unwrap_or(SparseIndexSnapshot {
                checkpoints: vec![self.opened.encoding.bom_len()],
                indexed_bytes: 0,
                file_size: self.known_file_size,
                total_lines: 0,
                complete: true,
                truncated_during_index: false,
                error: None,
            })
    }

    /// Blocks until background sparse line indexing completes (used in tests and benchmarks).
    pub fn wait_until_indexed(&mut self) {
        while !self.index_snapshot().complete {
            thread::sleep(std::time::Duration::from_millis(2));
        }
        self.poll_disk_state_now();
    }

    /// Returns a reference to the underlying [`OpenedTextFile`].
    #[must_use]
    pub fn opened_file(&self) -> &OpenedTextFile {
        &self.opened
    }

    /// Returns the currently known file size in bytes.
    #[must_use]
    pub fn file_size(&self) -> u64 {
        self.known_file_size
    }

    /// Periodically checks whether the underlying file was truncated, modified, or removed on disk (`AC-5.4`).
    pub fn poll_disk_state_if_due(&mut self) {
        if self.last_disk_check.elapsed() < std::time::Duration::from_millis(350) {
            return;
        }
        self.poll_disk_state_now();
    }

    /// Immediately inspects `fstat` and path metadata to detect concurrent file truncation or modification (`AC-5.4`).
    pub fn poll_disk_state_now(&mut self) {
        self.last_disk_check = Instant::now();

        let snap = self.index_snapshot();
        if snap.truncated_during_index && self.truncated_banner.is_none() {
            self.truncated_banner = Some(format!(
                "File was truncated on disk while open (now {}).",
                format_byte_size(snap.file_size)
            ));
            self.known_file_size = snap.file_size;
            self.cached_window = None;
        }

        match self.opened.current_fd_metadata() {
            Ok(meta) => {
                let current_len = meta.len();
                let current_mtime = meta.modified().ok();
                if current_len < self.known_file_size {
                    let prev = self.known_file_size;
                    self.truncated_banner = Some(format!(
                        "Warning: File was truncated while open (reduced from {} to {}).",
                        format_byte_size(prev),
                        format_byte_size(current_len)
                    ));
                    self.handle_file_truncation(current_len);
                } else if current_len > self.known_file_size
                    || (current_mtime.is_some() && current_mtime != self.known_mtime)
                {
                    self.known_file_size = current_len;
                    self.known_mtime = current_mtime;
                    self.modified_banner = Some(
                        "File was modified on disk while open. Click Reload to refresh."
                            .to_string(),
                    );
                }
            }
            Err(err) => {
                self.modified_banner = Some(format!("Warning: Could not stat open file ({err})."));
            }
        }

        // Also check path metadata in case the file was unlinked or had permissions revoked
        if let Err(err) = std::fs::metadata(&self.opened.path) {
            self.modified_banner = Some(format!(
                "Warning: File path status changed on disk ({err})."
            ));
        }
    }

    fn handle_file_truncation(&mut self, new_len: u64) {
        self.cancel_index.store(true, Ordering::SeqCst);
        self.known_file_size = new_len;
        self.cached_window = None;

        let content_start = self.opened.encoding.bom_len().min(new_len);
        // Rebuild a safe clamped index for the truncated length
        let mut checkpoints = vec![content_start];
        let mut newline_count = 0usize;
        let mut last_was_nl = false;
        let mut offset = content_start;
        let mut buf = vec![0u8; FETCH_CHUNK_BYTES];

        while offset < new_len {
            let to_read = ((new_len - offset) as usize).min(buf.len());
            match read_at_full(&self.opened.file, &mut buf[..to_read], offset) {
                Ok(0) => break,
                Ok(n) => {
                    scan_newlines_for_index(
                        &buf[..n],
                        offset,
                        self.opened.encoding,
                        &mut newline_count,
                        &mut checkpoints,
                        &mut last_was_nl,
                    );
                    offset += n as u64;
                }
                Err(_) => break,
            }
        }

        let total_lines = compute_total_lines(offset, content_start, newline_count, last_was_nl);
        if let Ok(mut guard) = self.shared_index.lock() {
            guard.checkpoints = checkpoints;
            guard.indexed_bytes = offset;
            guard.file_size = offset;
            guard.total_lines = total_lines;
            guard.complete = true;
            guard.truncated_during_index = true;
        }
    }

    /// Fetches `count` lines starting at 0-based `start_line` using the sparse line index and chunked `pread`.
    ///
    /// Never panics or crashes even if the underlying file is concurrently truncated (`AC-5.4`).
    pub fn fetch_lines(&mut self, start_line: usize, count: usize) -> Vec<PreviewLine> {
        if count == 0 {
            return Vec::new();
        }
        let end_line = start_line.saturating_add(count);

        let needs_load = match &self.cached_window {
            Some(win) => start_line < win.start_line || end_line > win.start_line + win.lines.len(),
            None => true,
        };

        if needs_load && let Err(err) = self.ensure_window_cached(start_line) {
            self.truncated_banner = Some(format!("Error reading file lines: {err}"));
            return Vec::new();
        }

        let Some(ref win) = self.cached_window else {
            return Vec::new();
        };
        if start_line < win.start_line {
            return Vec::new();
        }
        let rel_start = start_line - win.start_line;
        if rel_start >= win.lines.len() {
            return Vec::new();
        }
        let rel_end = (rel_start + count).min(win.lines.len());
        win.lines[rel_start..rel_end].to_vec()
    }

    /// Maps a byte offset in the file to its 0-based line index using the sparse line index + chunked `pread`.
    pub fn line_index_for_byte_offset(&self, target_offset: u64) -> usize {
        let content_start = self.opened.encoding.bom_len();
        if target_offset <= content_start {
            return 0;
        }
        let snap = self.index_snapshot();
        if snap.checkpoints.is_empty() {
            return 0;
        }
        // Binary search for the last checkpoint <= target_offset
        let cp_idx = match snap.checkpoints.binary_search(&target_offset) {
            Ok(exact) => exact,
            Err(ins) => ins.saturating_sub(1),
        };
        let mut offset = snap.checkpoints[cp_idx];
        let mut line_idx = cp_idx * SPARSE_STRIDE;
        let mut buf = vec![0u8; FETCH_CHUNK_BYTES];

        while offset < target_offset {
            let to_read = ((target_offset - offset) as usize).min(buf.len());
            let Ok(n) = read_at_full(&self.opened.file, &mut buf[..to_read], offset) else {
                break;
            };
            if n == 0 {
                break;
            }
            line_idx += count_newlines_in_slice(&buf[..n], self.opened.encoding);
            offset += n as u64;
        }
        line_idx
    }

    fn ensure_window_cached(&mut self, requested_line: usize) -> Result<(), ViewerError> {
        let window_start = requested_line & !63;
        let snap = self.index_snapshot();
        if snap.total_lines == 0 || window_start >= snap.total_lines {
            self.cached_window = Some(CachedLineWindow {
                start_line: window_start,
                lines: Vec::new(),
            });
            return Ok(());
        }

        let cp_idx = (window_start / SPARSE_STRIDE).min(snap.checkpoints.len().saturating_sub(1));
        let mut offset = snap.checkpoints[cp_idx];
        let mut current_line = cp_idx * SPARSE_STRIDE;
        let unit = self.opened.encoding.code_unit_bytes();

        let mut buf = vec![0u8; FETCH_CHUNK_BYTES];

        // 1. Advance from `checkpoints[cp_idx]` to the exact byte offset of `window_start`.
        while current_line < window_start && offset < snap.file_size {
            let remaining_file = (snap.file_size - offset) as usize;
            let to_read = remaining_file.min(buf.len());
            let aligned_read = if unit == 2 { to_read & !1 } else { to_read };
            if aligned_read == 0 {
                break;
            }
            let n = read_at_full(&self.opened.file, &mut buf[..aligned_read], offset).map_err(
                |source| ViewerError::Io {
                    path: self.opened.path.clone(),
                    source,
                },
            )?;
            if n == 0 {
                // File was truncated on disk!
                self.poll_disk_state_now();
                return Ok(());
            }
            let chunk = &buf[..n];
            if let Some(advance_bytes) = skip_lines_in_chunk(
                chunk,
                self.opened.encoding,
                window_start - current_line,
                &mut current_line,
            ) {
                offset += advance_bytes as u64;
                break;
            }
            offset += n as u64;
        }

        // 2. Read and decode up to `WINDOW_CACHE_LINES` lines starting at `offset`.
        let mut lines = Vec::with_capacity(WINDOW_CACHE_LINES);
        let mut raw_line_buf: Vec<u8> = Vec::with_capacity(256);
        let mut current_line_overflowed = false;
        let mut any_long_line = false;
        let mut any_encoding_err = false;

        while lines.len() < WINDOW_CACHE_LINES && offset < snap.file_size {
            let remaining_file = (snap.file_size - offset) as usize;
            let to_read = remaining_file.min(buf.len());
            let aligned_read = if unit == 2 { to_read & !1 } else { to_read };
            if aligned_read == 0 {
                break;
            }
            let n = read_at_full(&self.opened.file, &mut buf[..aligned_read], offset).map_err(
                |source| ViewerError::Io {
                    path: self.opened.path.clone(),
                    source,
                },
            )?;
            if n == 0 {
                // File truncated mid-read!
                self.poll_disk_state_now();
                break;
            }
            offset += n as u64;
            let chunk = &buf[..n];

            let mut slice_start = 0usize;
            while slice_start < chunk.len() && lines.len() < WINDOW_CACHE_LINES {
                let rem = &chunk[slice_start..];
                if let Some(nl_rel) = find_first_newline(rem, self.opened.encoding) {
                    let line_part = &rem[..nl_rel];
                    append_capped_line_bytes(
                        &mut raw_line_buf,
                        line_part,
                        &mut current_line_overflowed,
                    );
                    let line_num = (window_start + lines.len() + 1) as u64;
                    let (preview_line, enc_err) = finalize_preview_line(
                        &raw_line_buf,
                        line_num,
                        self.opened.encoding,
                        current_line_overflowed,
                    );
                    if preview_line.line_truncated {
                        any_long_line = true;
                    }
                    if enc_err {
                        any_encoding_err = true;
                    }
                    lines.push(preview_line);
                    raw_line_buf.clear();
                    current_line_overflowed = false;
                    slice_start += nl_rel + unit;
                } else {
                    append_capped_line_bytes(&mut raw_line_buf, rem, &mut current_line_overflowed);
                    break;
                }
            }
        }

        // Flush final unterminated line at EOF if non-empty
        if !raw_line_buf.is_empty() && lines.len() < WINDOW_CACHE_LINES {
            let line_num = (window_start + lines.len() + 1) as u64;
            let (preview_line, enc_err) = finalize_preview_line(
                &raw_line_buf,
                line_num,
                self.opened.encoding,
                current_line_overflowed,
            );
            if preview_line.line_truncated {
                any_long_line = true;
            }
            if enc_err {
                any_encoding_err = true;
            }
            lines.push(preview_line);
        }

        if any_long_line {
            self.has_long_line_truncation = true;
        }
        if any_encoding_err {
            self.has_encoding_warnings = true;
        }

        self.cached_window = Some(CachedLineWindow {
            start_line: window_start,
            lines,
        });
        Ok(())
    }
}

impl Drop for FullDocument {
    fn drop(&mut self) {
        self.cancel_index.store(true, Ordering::SeqCst);
    }
}

fn compute_total_lines(
    indexed_end: u64,
    content_start: u64,
    newline_count: usize,
    last_was_nl: bool,
) -> usize {
    if indexed_end <= content_start {
        0
    } else if last_was_nl {
        newline_count
    } else {
        newline_count + 1
    }
}

fn scan_newlines_for_index(
    chunk: &[u8],
    chunk_file_offset: u64,
    encoding: DetectedEncoding,
    newline_count: &mut usize,
    checkpoints: &mut Vec<u64>,
    last_was_nl: &mut bool,
) {
    if chunk.is_empty() {
        return;
    }
    match encoding {
        DetectedEncoding::Utf8 | DetectedEncoding::Utf8Bom | DetectedEncoding::LossyUtf8 => {
            for pos in memchr_iter(b'\n', chunk) {
                *newline_count += 1;
                if (*newline_count).is_multiple_of(SPARSE_STRIDE) {
                    checkpoints.push(chunk_file_offset + (pos + 1) as u64);
                }
            }
            *last_was_nl = chunk.last() == Some(&b'\n');
        }
        DetectedEncoding::Utf16Le { .. } => {
            for (idx, pair) in chunk.chunks_exact(2).enumerate() {
                if pair == [0x0A, 0x00] {
                    *newline_count += 1;
                    if (*newline_count).is_multiple_of(SPARSE_STRIDE) {
                        checkpoints.push(chunk_file_offset + ((idx + 1) * 2) as u64);
                    }
                }
            }
            let len = chunk.len() & !1;
            if len >= 2 {
                *last_was_nl = chunk[len - 2..len] == [0x0A, 0x00];
            }
        }
        DetectedEncoding::Utf16Be { .. } => {
            for (idx, pair) in chunk.chunks_exact(2).enumerate() {
                if pair == [0x00, 0x0A] {
                    *newline_count += 1;
                    if (*newline_count).is_multiple_of(SPARSE_STRIDE) {
                        checkpoints.push(chunk_file_offset + ((idx + 1) * 2) as u64);
                    }
                }
            }
            let len = chunk.len() & !1;
            if len >= 2 {
                *last_was_nl = chunk[len - 2..len] == [0x00, 0x0A];
            }
        }
    }
}

fn count_newlines_in_slice(chunk: &[u8], encoding: DetectedEncoding) -> usize {
    match encoding {
        DetectedEncoding::Utf8 | DetectedEncoding::Utf8Bom | DetectedEncoding::LossyUtf8 => {
            memchr_iter(b'\n', chunk).count()
        }
        DetectedEncoding::Utf16Le { .. } => {
            chunk.chunks_exact(2).filter(|&p| p == [0x0A, 0x00]).count()
        }
        DetectedEncoding::Utf16Be { .. } => {
            chunk.chunks_exact(2).filter(|&p| p == [0x00, 0x0A]).count()
        }
    }
}

fn skip_lines_in_chunk(
    chunk: &[u8],
    encoding: DetectedEncoding,
    lines_to_skip: usize,
    current_line: &mut usize,
) -> Option<usize> {
    if lines_to_skip == 0 {
        return Some(0);
    }
    let mut rem = lines_to_skip;
    match encoding {
        DetectedEncoding::Utf8 | DetectedEncoding::Utf8Bom | DetectedEncoding::LossyUtf8 => {
            for pos in memchr_iter(b'\n', chunk) {
                *current_line += 1;
                rem -= 1;
                if rem == 0 {
                    return Some(pos + 1);
                }
            }
            None
        }
        DetectedEncoding::Utf16Le { .. } => {
            for (idx, pair) in chunk.chunks_exact(2).enumerate() {
                if pair == [0x0A, 0x00] {
                    *current_line += 1;
                    rem -= 1;
                    if rem == 0 {
                        return Some((idx + 1) * 2);
                    }
                }
            }
            None
        }
        DetectedEncoding::Utf16Be { .. } => {
            for (idx, pair) in chunk.chunks_exact(2).enumerate() {
                if pair == [0x00, 0x0A] {
                    *current_line += 1;
                    rem -= 1;
                    if rem == 0 {
                        return Some((idx + 1) * 2);
                    }
                }
            }
            None
        }
    }
}

fn find_first_newline(slice: &[u8], encoding: DetectedEncoding) -> Option<usize> {
    match encoding {
        DetectedEncoding::Utf8 | DetectedEncoding::Utf8Bom | DetectedEncoding::LossyUtf8 => {
            memchr::memchr(b'\n', slice)
        }
        DetectedEncoding::Utf16Le { .. } => slice
            .chunks_exact(2)
            .position(|p| p == [0x0A, 0x00])
            .map(|idx| idx * 2),
        DetectedEncoding::Utf16Be { .. } => slice
            .chunks_exact(2)
            .position(|p| p == [0x00, 0x0A])
            .map(|idx| idx * 2),
    }
}

fn append_capped_line_bytes(buf: &mut Vec<u8>, addition: &[u8], overflowed: &mut bool) {
    // Allow up to `MAX_LINE_BYTES * 2` raw bytes before dropping excess so UTF-16 lines also
    // decode up to `MAX_LINE_BYTES` UTF-8 bytes before truncating.
    let cap = MAX_LINE_BYTES * 2;
    if buf.len() >= cap {
        *overflowed = true;
        return;
    }
    let avail = cap - buf.len();
    if addition.len() > avail {
        buf.extend_from_slice(&addition[..avail]);
        *overflowed = true;
    } else {
        buf.extend_from_slice(addition);
    }
}

fn finalize_preview_line(
    raw_bytes: &[u8],
    line_number: u64,
    encoding: DetectedEncoding,
    overflowed_raw_cap: bool,
) -> (PreviewLine, bool) {
    let (decoded, enc_err) = decode_bytes(raw_bytes, encoding, overflowed_raw_cap);
    let stripped = decoded.strip_suffix('\r').unwrap_or(&decoded);
    let (mut text, mut line_truncated) = truncate_line_for_display(stripped, MAX_LINE_BYTES);
    if overflowed_raw_cap && !line_truncated {
        text.push('…');
        line_truncated = true;
    }
    (
        PreviewLine {
            line_number,
            text,
            line_truncated,
        },
        enc_err,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "radivine_viewer_full_{label}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn test_full_document_sparse_index_and_random_row_access() {
        let dir = make_temp_dir("sparse");
        let path = dir.join("five_thousand.log");

        let mut content = String::new();
        for i in 1..=5_000 {
            content.push_str(&format!("Log entry #{i:05} — payload check\n"));
        }
        std::fs::write(&path, content.as_bytes()).expect("write 5000 lines");

        let opened = OpenedTextFile::open(&path).expect("open");
        let mut doc = FullDocument::new(opened).expect("FullDocument::new");

        // Immediate first screen is available even before waiting for background indexer
        let first_rows = doc.fetch_lines(0, 20);
        assert_eq!(first_rows.len(), 20);
        assert_eq!(first_rows[0].line_number, 1);
        assert_eq!(first_rows[0].text, "Log entry #00001 — payload check");

        doc.wait_until_indexed();
        let snap = doc.index_snapshot();
        assert!(snap.complete);
        assert_eq!(snap.total_lines, 5_000);
        // 5000 lines with stride 1024 -> checkpoints at 0, 1024, 2048, 3072, 4096 (5 checkpoints)
        assert_eq!(snap.checkpoints.len(), 5);

        // Fetch across checkpoint boundaries (e.g. around line 1024, 2048, 4990)
        for &start in &[0usize, 1020, 1024, 2047, 3500, 4980] {
            let rows = doc.fetch_lines(start, 15);
            for (idx, row) in rows.iter().enumerate() {
                let expected_num = start + idx + 1;
                assert_eq!(row.line_number, expected_num as u64);
                assert_eq!(
                    row.text,
                    format!("Log entry #{expected_num:05} — payload check")
                );
            }
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_full_document_handles_concurrent_truncation_without_crash_ac_5_4() {
        let dir = make_temp_dir("truncate");
        let path = dir.join("concurrent_truncate.log");

        let mut content = String::new();
        for i in 1..=3_000 {
            content.push_str(&format!("Line {i:04}: initial data before truncation\n"));
        }
        std::fs::write(&path, content.as_bytes()).expect("write");

        let opened = OpenedTextFile::open(&path).expect("open");
        let mut doc = FullDocument::new(opened).expect("FullDocument::new");
        doc.wait_until_indexed();
        assert_eq!(doc.index_snapshot().total_lines, 3_000);

        // 1. Concurrently truncate the file on disk to only 10 lines!
        let short_content: String = (1..=10).map(|i| format!("Short line {i}\n")).collect();
        std::fs::write(&path, short_content.as_bytes()).expect("truncate to 10 lines");

        // Trying to fetch lines near line 2500 (past the new EOF) must NOT crash or panic
        let _ = doc.fetch_lines(2500, 50);
        doc.poll_disk_state_now();
        assert!(
            doc.truncated_banner.is_some(),
            "expected truncation banner after file shrunk on disk"
        );
        assert_eq!(doc.index_snapshot().total_lines, 10);

        let valid_rows = doc.fetch_lines(0, 10);
        assert_eq!(valid_rows.len(), 10);
        assert_eq!(valid_rows[0].text, "Short line 1");
        assert_eq!(valid_rows[9].text, "Short line 10");

        // 2. Truncate the file to 0 bytes while still open
        std::fs::File::create(&path).expect("truncate to 0 bytes");
        doc.poll_disk_state_now();
        assert!(doc.truncated_banner.is_some());
        assert_eq!(doc.index_snapshot().total_lines, 0);
        assert!(doc.fetch_lines(0, 10).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
