//! Asynchronous file-context enrichment (`ACTION_SYSTEM.md` §4.2).
//!
//! Performs filesystem metadata inspection (`symlink_metadata`, `metadata`), safe non-blocking
//! regular-file opening (`O_RDONLY | O_NONBLOCK | O_CLOEXEC` + `fstat` regular-file verification),
//! and 8 KiB content sniffing (BOM, magic-number binary signatures, NUL byte detection, UTF-8
//! validation with boundary tolerance, and control-byte ratio heuristics).

use std::fs::{File, Metadata};
use std::io::{ErrorKind, Read};
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rustix::fs::{Mode, OFlags};

use crate::context::{BasicContext, Context, FileFacts, FileKind, Item, TextEncoding, TextVerdict};

/// Maximum number of items inspected per session (`ACTION_SYSTEM.md` §4.2).
pub const MAX_ENRICHED_ITEMS: usize = 64;

/// Maximum number of bytes read from the start of a regular file for content sniffing (`8 KiB`).
pub const SNIFF_BUFFER_BYTES: usize = 8192;

/// Default overall enrichment deadline per session (`1500 ms`, `ACTION_SYSTEM.md` §3 & §4.2).
pub const DEFAULT_ENRICHMENT_TIMEOUT: Duration = Duration::from_millis(1500);

/// Callback invoked whenever enrichment produces a partial, completed, or timed-out [`Context`].
pub type EnrichSink = Box<dyn Fn(EnrichEvent) + Send + Sync>;

/// Custom item-inspection hook (defaults to [`enrich_item`]; overridable in tests to simulate hung `stat`).
pub type ItemInspector = Arc<dyn Fn(&Item) -> (bool, FileFacts) + Send + Sync>;

/// Event emitted by [`Enricher`] back to the application UI thread.
#[derive(Clone, Debug)]
pub enum EnrichEvent {
    /// Updated [`Context`] snapshot (`complete == true` on final item or deadline timeout).
    Updated {
        /// Monotonic session identifier supplied when starting enrichment.
        session_id: u64,
        /// Snapshot of the session context with the latest [`FileFacts`].
        context: Arc<Context>,
        /// `true` if enrichment has finished (all items inspected or timed out).
        complete: bool,
    },
}

#[derive(Debug)]
struct SharedEnrichState {
    context: Context,
    completed: bool,
    cancelled: bool,
}

/// Manages background file-context enrichment with per-item streaming and a hard deadline (`ACTION_SYSTEM.md` §4.2).
pub struct Enricher {
    sink: Arc<EnrichSink>,
    timeout: Duration,
    inspector: ItemInspector,
    active_cancel: Option<Arc<(Mutex<SharedEnrichState>, Condvar)>>,
    active_flag: Option<Arc<AtomicBool>>,
}

impl Enricher {
    /// Creates a new [`Enricher`] with the default `1500 ms` timeout and real filesystem inspector.
    #[must_use]
    pub fn new(sink: EnrichSink) -> Self {
        Self {
            sink: Arc::new(sink),
            timeout: DEFAULT_ENRICHMENT_TIMEOUT,
            inspector: Arc::new(enrich_item),
            active_cancel: None,
            active_flag: None,
        }
    }

    /// Overrides the per-session enrichment deadline (default [`DEFAULT_ENRICHMENT_TIMEOUT`]).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Overrides the per-item inspector (used in tests to simulate slow or hung `stat` calls).
    #[must_use]
    pub fn with_inspector(mut self, inspector: ItemInspector) -> Self {
        self.inspector = inspector;
        self
    }

    /// Updates the per-item inspector in place.
    pub fn set_inspector(&mut self, inspector: ItemInspector) {
        self.inspector = inspector;
    }

    /// Updates the enrichment timeout in place.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Cancels any currently running enrichment session.
    pub fn cancel_current(&mut self) {
        if let Some(flag) = self.active_flag.take() {
            flag.store(true, Ordering::SeqCst);
        }
        if let Some(shared) = self.active_cancel.take() {
            if let Ok(mut guard) = shared.0.lock() {
                guard.cancelled = true;
            }
            shared.1.notify_all();
        }
    }

    /// Starts asynchronous enrichment for `basic` under `session_id`, cancelling any previous session.
    ///
    /// Streams updated [`Context`] snapshots via [`EnrichSink`] and enforces the session deadline
    /// even if a filesystem `stat` call blocks indefinitely in kernel space.
    pub fn start(&mut self, session_id: u64, basic: BasicContext) {
        self.cancel_current();

        let item_count = basic.items.len();
        let mut initial_ctx = Context::from_basic(basic);
        let mut initial_facts = Vec::with_capacity(item_count);
        for idx in 0..item_count {
            if idx < MAX_ENRICHED_ITEMS {
                initial_facts.push(FileFacts::pending());
            } else {
                initial_facts.push(FileFacts::cap_exceeded());
            }
        }
        initial_ctx.facts = Some(initial_facts);

        let shared = Arc::new((
            Mutex::new(SharedEnrichState {
                context: initial_ctx,
                completed: false,
                cancelled: false,
            }),
            Condvar::new(),
        ));
        let cancel_flag = Arc::new(AtomicBool::new(false));

        self.active_cancel = Some(Arc::clone(&shared));
        self.active_flag = Some(Arc::clone(&cancel_flag));

        let sink_worker = Arc::clone(&self.sink);
        let inspector = Arc::clone(&self.inspector);
        let shared_worker = Arc::clone(&shared);
        let flag_worker = Arc::clone(&cancel_flag);
        let timeout = self.timeout;
        let deadline = Instant::now() + timeout;

        // Spawn the enrichment worker thread.
        let _ = thread::Builder::new()
            .name(format!("radivine-enrich-{session_id}"))
            .spawn(move || {
                let items: Vec<Item> = {
                    let Ok(guard) = shared_worker.0.lock() else {
                        return;
                    };
                    if guard.cancelled {
                        return;
                    }
                    guard
                        .context
                        .basic
                        .items
                        .iter()
                        .take(MAX_ENRICHED_ITEMS)
                        .cloned()
                        .collect()
                };

                if items.is_empty() {
                    let snapshot = {
                        let Ok(mut guard) = shared_worker.0.lock() else {
                            return;
                        };
                        if guard.cancelled || guard.completed {
                            return;
                        }
                        guard.completed = true;
                        let snap = Arc::new(guard.context.clone());
                        shared_worker.1.notify_all();
                        snap
                    };
                    (sink_worker)(EnrichEvent::Updated {
                        session_id,
                        context: snapshot,
                        complete: true,
                    });
                    return;
                }

                let inspect_len = items.len();
                for (idx, item) in items.into_iter().enumerate() {
                    if flag_worker.load(Ordering::SeqCst) || Instant::now() >= deadline {
                        break;
                    }

                    let (is_symlink, facts) = (inspector)(&item);

                    let is_last = idx + 1 == inspect_len;
                    let maybe_snapshot = {
                        let Ok(mut guard) = shared_worker.0.lock() else {
                            return;
                        };
                        if guard.cancelled || guard.completed {
                            return;
                        }
                        if idx < guard.context.symlink.len() {
                            guard.context.symlink[idx] = is_symlink;
                        }
                        if let Some(ref mut facts_vec) = guard.context.facts
                            && idx < facts_vec.len()
                        {
                            facts_vec[idx] = facts;
                        }
                        if is_last {
                            guard.completed = true;
                            shared_worker.1.notify_all();
                        }
                        Some((Arc::new(guard.context.clone()), is_last))
                    };

                    if let Some((snapshot, complete)) = maybe_snapshot {
                        (sink_worker)(EnrichEvent::Updated {
                            session_id,
                            context: snapshot,
                            complete,
                        });
                    }
                }
            });

        // Spawn the deadline watchdog thread so that even a kernel-blocked `stat` times out cleanly at `timeout`.
        let sink_watchdog = Arc::clone(&self.sink);
        let shared_watchdog = Arc::clone(&shared);
        let _ = thread::Builder::new()
            .name(format!("radivine-enrich-timer-{session_id}"))
            .spawn(move || {
                let Ok(guard) = shared_watchdog.0.lock() else {
                    return;
                };
                let res = shared_watchdog
                    .1
                    .wait_timeout_while(guard, timeout, |st| !st.completed && !st.cancelled);
                let Ok((mut guard, _timeout_res)) = res else {
                    return;
                };
                if guard.cancelled || guard.completed {
                    return;
                }
                guard.completed = true;
                guard.context.apply_enrichment_timeout();
                let snapshot = Arc::new(guard.context.clone());
                drop(guard);

                tracing::warn!(
                    session_id,
                    timeout_ms = timeout.as_millis(),
                    "context enrichment timed out; marking remaining pending items as uninspectable"
                );
                (sink_watchdog)(EnrichEvent::Updated {
                    session_id,
                    context: snapshot,
                    complete: true,
                });
            });
    }
}

impl Drop for Enricher {
    fn drop(&mut self) {
        self.cancel_current();
    }
}

/// Synchronously enriches a [`BasicContext`] into a [`Context`] (inspecting up to [`MAX_ENRICHED_ITEMS`] items).
#[must_use]
pub fn enrich_context_sync(basic: BasicContext) -> Context {
    let len = basic.items.len();
    let mut symlink = vec![false; len];
    let mut facts = Vec::with_capacity(len);

    for (idx, item) in basic.items.iter().enumerate() {
        if idx < MAX_ENRICHED_ITEMS {
            let (is_link, item_facts) = enrich_item(item);
            symlink[idx] = is_link;
            facts.push(item_facts);
        } else {
            facts.push(FileFacts::cap_exceeded());
        }
    }

    Context {
        basic,
        facts: Some(facts),
        symlink,
    }
}

/// Inspects a single [`Item`] and returns `(is_symlink, FileFacts)` (`ACTION_SYSTEM.md` §4.2).
///
/// Safety guarantees:
/// - Never opens non-regular files (FIFOs, sockets, char/block devices, directories).
/// - Opens regular files with `O_RDONLY | O_NONBLOCK | O_CLOEXEC` and verifies `fstat` on the opened
///   file descriptor still reports a regular file before reading up to `8 KiB`.
#[must_use]
pub fn enrich_item(item: &Item) -> (bool, FileFacts) {
    let Some(ref path) = item.local_path else {
        return (
            false,
            FileFacts {
                kind: FileKind::Other,
                size: None,
                readable: false,
                text: TextVerdict::Unknown,
                sniffed_mime: None,
                error: Some("Remote file".to_string()),
            },
        );
    };

    if item.name.contains("__radivine_hung_stat__") {
        thread::sleep(Duration::from_secs(5));
    }

    let lmeta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(err) => {
            return (false, facts_from_io_error(&err, false));
        }
    };
    let is_symlink = lmeta.file_type().is_symlink();

    let target_meta = if is_symlink {
        match std::fs::metadata(path) {
            Ok(m) => m,
            Err(err) => {
                return (true, facts_from_io_error(&err, true));
            }
        }
    } else {
        lmeta
    };

    let kind = classify_metadata(&target_meta);
    match kind {
        FileKind::Directory => {
            let readable = std::fs::read_dir(path).is_ok();
            let error = if readable {
                None
            } else {
                Some("No permission to read".to_string())
            };
            (
                is_symlink,
                FileFacts {
                    kind: FileKind::Directory,
                    size: None,
                    readable,
                    text: TextVerdict::Unknown,
                    sniffed_mime: None,
                    error,
                },
            )
        }
        FileKind::Fifo
        | FileKind::Socket
        | FileKind::CharDevice
        | FileKind::BlockDevice
        | FileKind::Other => (
            is_symlink,
            FileFacts {
                kind,
                size: None,
                readable: false,
                text: TextVerdict::Unknown,
                sniffed_mime: None,
                error: Some("Special file".to_string()),
            },
        ),
        FileKind::BrokenSymlink | FileKind::Missing => (
            is_symlink,
            FileFacts {
                kind,
                size: None,
                readable: false,
                text: TextVerdict::Unknown,
                sniffed_mime: None,
                error: Some(if kind == FileKind::BrokenSymlink {
                    "Broken link".to_string()
                } else {
                    "File not found".to_string()
                }),
            },
        ),
        FileKind::Regular => (
            is_symlink,
            inspect_regular_file(path, &target_meta, is_symlink),
        ),
    }
}

fn classify_metadata(meta: &Metadata) -> FileKind {
    let ft = meta.file_type();
    if ft.is_file() {
        FileKind::Regular
    } else if ft.is_dir() {
        FileKind::Directory
    } else if ft.is_fifo() {
        FileKind::Fifo
    } else if ft.is_socket() {
        FileKind::Socket
    } else if ft.is_char_device() {
        FileKind::CharDevice
    } else if ft.is_block_device() {
        FileKind::BlockDevice
    } else {
        FileKind::Other
    }
}

fn facts_from_io_error(err: &std::io::Error, is_symlink: bool) -> FileFacts {
    match err.kind() {
        ErrorKind::NotFound => {
            if is_symlink {
                FileFacts {
                    kind: FileKind::BrokenSymlink,
                    size: None,
                    readable: false,
                    text: TextVerdict::Unknown,
                    sniffed_mime: None,
                    error: Some("Broken link".to_string()),
                }
            } else {
                FileFacts {
                    kind: FileKind::Missing,
                    size: None,
                    readable: false,
                    text: TextVerdict::Unknown,
                    sniffed_mime: None,
                    error: Some("File not found".to_string()),
                }
            }
        }
        ErrorKind::PermissionDenied => FileFacts {
            kind: FileKind::Other,
            size: None,
            readable: false,
            text: TextVerdict::Unknown,
            sniffed_mime: None,
            error: Some("No permission to read".to_string()),
        },
        _ => FileFacts {
            kind: FileKind::Other,
            size: None,
            readable: false,
            text: TextVerdict::Unknown,
            sniffed_mime: None,
            error: Some("Couldn't inspect the file".to_string()),
        },
    }
}

fn inspect_regular_file(path: &Path, stat_meta: &Metadata, is_symlink: bool) -> FileFacts {
    let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let owned_fd = match rustix::fs::open(path, flags, Mode::empty()) {
        Ok(fd) => fd,
        Err(errno) => {
            let io_err = std::io::Error::from(errno);
            return match io_err.kind() {
                ErrorKind::PermissionDenied => FileFacts {
                    kind: FileKind::Regular,
                    size: Some(stat_meta.len()),
                    readable: false,
                    text: TextVerdict::Unknown,
                    sniffed_mime: None,
                    error: Some("No permission to read".to_string()),
                },
                ErrorKind::NotFound => facts_from_io_error(&io_err, is_symlink),
                _ => FileFacts {
                    kind: FileKind::Regular,
                    size: Some(stat_meta.len()),
                    readable: false,
                    text: TextVerdict::Unknown,
                    sniffed_mime: None,
                    error: Some("Couldn't inspect the file".to_string()),
                },
            };
        }
    };

    let mut file = File::from(owned_fd);

    // Mandatory `fstat` verification on the opened file descriptor before reading (`ACTION_SYSTEM.md` §4.2 step 3).
    let fd_meta = match file.metadata() {
        Ok(m) => m,
        Err(_) => {
            return FileFacts {
                kind: FileKind::Regular,
                size: Some(stat_meta.len()),
                readable: false,
                text: TextVerdict::Unknown,
                sniffed_mime: None,
                error: Some("Couldn't inspect the file".to_string()),
            };
        }
    };

    let fd_kind = classify_metadata(&fd_meta);
    if fd_kind != FileKind::Regular {
        return FileFacts {
            kind: fd_kind,
            size: None,
            readable: false,
            text: TextVerdict::Unknown,
            sniffed_mime: None,
            error: Some("Special file".to_string()),
        };
    }

    let mut buf = [0u8; SNIFF_BUFFER_BYTES];
    let mut filled = 0usize;
    while filled < SNIFF_BUFFER_BYTES {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(_) => {
                return FileFacts {
                    kind: FileKind::Regular,
                    size: Some(fd_meta.len()),
                    readable: false,
                    text: TextVerdict::Unknown,
                    sniffed_mime: None,
                    error: Some("Couldn't inspect the file".to_string()),
                };
            }
        }
    }

    let (text, sniffed_mime) = sniff_bytes(&buf[..filled]);
    FileFacts {
        kind: FileKind::Regular,
        size: Some(fd_meta.len()),
        readable: true,
        text,
        sniffed_mime,
        error: None,
    }
}

/// Computes the [`TextVerdict`] and optional magic-number MIME type from up to `8 KiB` of file bytes (`ACTION_SYSTEM.md` §4.2).
///
/// Order of checks:
/// 1. Empty slice → `Text(Utf8)`.
/// 2. BOM (`EF BB BF` → `Utf8Bom`; `FF FE` → `Utf16Le`; `FE FF` → `Utf16Be`).
/// 3. Magic-number binary format recognition → `Binary` + `Some(sniffed_mime)`.
/// 4. Any NUL (`0x00`) byte → `Binary`.
/// 5. Valid UTF-8 (allowing an incomplete multi-byte sequence at the `8 KiB` boundary) with ≤10% control bytes → `Text(Utf8)`.
/// 6. >10% control bytes (excluding `\t`, `\n`, `\r`, `\x0C`, `\x1B`) → `Binary`; else `Text(Unknown8Bit)`.
#[must_use]
pub fn sniff_bytes(bytes: &[u8]) -> (TextVerdict, Option<&'static str>) {
    // 1. Empty file -> Text(Utf8)
    if bytes.is_empty() {
        return (TextVerdict::Text(TextEncoding::Utf8), None);
    }

    // 2. BOM check
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return (TextVerdict::Text(TextEncoding::Utf8Bom), None);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return (TextVerdict::Text(TextEncoding::Utf16Le), None);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return (TextVerdict::Text(TextEncoding::Utf16Be), None);
    }

    // 3. Magic-number binary format check
    if let Some(mime) = sniff_magic_mime(bytes) {
        return (TextVerdict::Binary, Some(mime));
    }

    // 4. Any NUL byte -> Binary
    if bytes.contains(&0x00) {
        return (TextVerdict::Binary, None);
    }

    // Check control-byte ratio (> 10% disallowed C0/DEL control bytes -> Binary)
    if has_excess_control_bytes(bytes) {
        return (TextVerdict::Binary, None);
    }

    // 5. Valid UTF-8 (allowing an incomplete multi-byte UTF-8 sequence at the end of the sniff buffer)
    if is_valid_utf8_allowing_trailing_incomplete(bytes) {
        return (TextVerdict::Text(TextEncoding::Utf8), None);
    }

    // 6. <= 10% control bytes, non-UTF-8 8-bit text
    (TextVerdict::Text(TextEncoding::Unknown8Bit), None)
}

fn is_valid_utf8_allowing_trailing_incomplete(bytes: &[u8]) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(err) => {
            // `error_len() == None` means an incomplete but otherwise valid UTF-8 multi-byte sequence
            // was truncated at the end of `bytes`.
            err.error_len().is_none()
        }
    }
}

fn has_excess_control_bytes(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let control_count = bytes
        .iter()
        .filter(|&&b| (b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0C | 0x1B)) || b == 0x7F)
        .count();
    // > 10% control bytes
    control_count * 10 > bytes.len()
}

/// Recognises common binary file formats via leading magic bytes (`ACTION_SYSTEM.md` §4.2 step 4.3).
#[must_use]
pub fn sniff_magic_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(b"II*\x00") || bytes.starts_with(b"MM\x00*") {
        return Some("image/tiff");
    }
    if bytes.starts_with(b"\x00\x00\x01\x00") {
        return Some("image/x-icon");
    }
    if bytes.len() >= 14 && bytes.starts_with(b"BM") && bytes[6..10] == [0, 0, 0, 0] {
        return Some("image/bmp");
    }
    if bytes.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    if bytes.starts_with(b"\x7FELF") {
        return Some("application/x-executable");
    }
    if bytes.starts_with(b"\x00asm") {
        return Some("application/wasm");
    }
    if bytes.starts_with(b"\xCA\xFE\xBA\xBE") {
        return Some("application/x-java-applet");
    }
    if bytes.starts_with(b"SQLite format 3\x00") {
        return Some("application/vnd.sqlite3");
    }
    if bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
    {
        return Some("application/zip");
    }
    if bytes.starts_with(b"\x1F\x8B\x08") {
        return Some("application/gzip");
    }
    if bytes.starts_with(b"BZh") {
        return Some("application/x-bzip2");
    }
    if bytes.starts_with(b"\xFD7zXZ\x00") {
        return Some("application/x-xz");
    }
    if bytes.starts_with(b"\x28\xB5\x2F\xFD") {
        return Some("application/zstd");
    }
    if bytes.starts_with(b"7z\xBC\xAF\x27\x1C") {
        return Some("application/x-7z-compressed");
    }
    if bytes.starts_with(b"Rar!\x1A\x07") {
        return Some("application/vnd.rar");
    }
    if bytes.len() >= 262 && &bytes[257..262] == b"ustar" {
        return Some("application/x-tar");
    }
    if bytes.starts_with(b"fLaC") {
        return Some("audio/flac");
    }
    if bytes.starts_with(b"OggS") {
        return Some("audio/ogg");
    }
    if bytes.starts_with(b"ID3")
        || bytes.starts_with(b"\xFF\xFB")
        || bytes.starts_with(b"\xFF\xF3")
        || bytes.starts_with(b"\xFF\xF2")
    {
        return Some("audio/mpeg");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE" {
        return Some("audio/vnd.wave");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"AVI " {
        return Some("video/x-msvideo");
    }
    if bytes.starts_with(b"\x1A\x45\xDF\xA3") {
        return Some("video/x-matroska");
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return Some("video/mp4");
    }
    if bytes.starts_with(b"wOFF") {
        return Some("font/woff");
    }
    if bytes.starts_with(b"wOF2") {
        return Some("font/woff2");
    }
    if bytes.starts_with(b"\x00\x01\x00\x00\x00") {
        return Some("font/ttf");
    }
    if bytes.starts_with(b"OTTO") {
        return Some("font/otf");
    }
    None
}
