//! Context model and RFC 2483 `text/uri-list` parsing (`ACTION_SYSTEM.md` §4).

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, percent_encode};

/// Maximum number of items retained in a single [`BasicContext`] (`ACTION_SYSTEM.md` §4).
pub const MAX_CONTEXT_ITEMS: usize = 10_000;

/// Reason string recorded on [`FileFacts::error`] when an item's index exceeds
/// [`MAX_ENRICHED_ITEMS`](crate::MAX_ENRICHED_ITEMS) (`64`).
pub const CAP_EXCEEDED_REASON: &str = "Too many items to inspect";

/// Reason string recorded on [`FileFacts::error`] when background enrichment
/// exceeds the per-session deadline before inspecting the item.
pub const ENRICH_TIMEOUT_REASON: &str = "Couldn't inspect the file";

/// Character set percent-encoded in `file://` URI path segments (RFC 3986 / RFC 8089).
const PATH_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// High-level kind of context payload (`ACTION_SYSTEM.md` §4).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContextKind {
    /// File / URI list (`text/uri-list`).
    Files,
    /// Selected plain text (future extension).
    Text,
    /// Web URL (future extension).
    Url,
}

/// How the session was invoked (`ACTION_SYSTEM.md` §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InvocationMode {
    /// Summoned mid-drag via modifier trigger.
    Drag,
    /// Summoned via `radivine invoke` CLI / IPC.
    Invoke,
}

/// Metadata identifying the source application of a drag or invocation (`ACTION_SYSTEM.md` §4).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceInfo {
    /// Process executable name (e.g. `Some("nemo".into())`).
    pub app: Option<String>,
    /// Process ID, if known.
    pub pid: Option<u32>,
}

/// Synchronous, zero-I/O context built directly from the URI payload (`ACTION_SYSTEM.md` §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasicContext {
    /// Payload kind (`ContextKind::Files` in v0.1).
    pub kind: ContextKind,
    /// Whether this session was started via Drag or Invoke mode.
    pub mode: InvocationMode,
    /// Source application info.
    pub source: SourceInfo,
    /// Parsed items (capped at [`MAX_CONTEXT_ITEMS`]).
    pub items: Vec<Item>,
    /// `true` if more than [`MAX_CONTEXT_ITEMS`] items were present in the payload.
    pub truncated: bool,
}

impl BasicContext {
    /// Builds a [`BasicContext`] from raw `text/uri-list` payload bytes per RFC 2483 (`ACTION_SYSTEM.md` §4.1).
    ///
    /// Performs **zero filesystem I/O**.
    #[must_use]
    pub fn from_uri_list_bytes(bytes: &[u8], mode: InvocationMode, source: SourceInfo) -> Self {
        let text = String::from_utf8_lossy(bytes);
        let uris = parse_uri_list(&text);
        Self::from_uris(uris, mode, source)
    }

    /// Builds a [`BasicContext`] from an iterator of URI strings.
    ///
    /// Performs **zero filesystem I/O**.
    #[must_use]
    pub fn from_uris<I, S>(uris: I, mode: InvocationMode, source: SourceInfo) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut items = Vec::new();
        let mut truncated = false;

        for raw in uris {
            let trimmed = raw.as_ref().trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if items.len() >= MAX_CONTEXT_ITEMS {
                truncated = true;
                break;
            }
            items.push(Item::from_uri(trimmed));
        }

        Self {
            kind: ContextKind::Files,
            mode,
            source,
            items,
            truncated,
        }
    }
}

/// Single URI item inside a [`BasicContext`] (`ACTION_SYSTEM.md` §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Normalised URI string (without trailing CR/LF).
    pub uri: String,
    /// Local filesystem path (`Some` only for `file://` URIs with empty or `localhost` host and an absolute path).
    pub local_path: Option<PathBuf>,
    /// Last path segment, percent-decoded into lossy UTF-8 for display.
    pub name: String,
    /// Lowercase file extension without leading dot, if any.
    pub ext: Option<String>,
    /// Extension-based MIME guess (`mime_guess`), if known.
    pub guessed_mime: Option<&'static str>,
}

impl Item {
    /// Parses a single URI into an [`Item`] without touching the filesystem (`ACTION_SYSTEM.md` §4.1).
    #[must_use]
    pub fn from_uri(uri: &str) -> Self {
        let norm_uri = uri.trim_end_matches(['\r', '\n']).to_string();
        let local_path = parse_file_uri_to_local_path(&norm_uri);

        let name = if let Some(ref path) = local_path {
            path.file_name()
                .map(|os| os.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".to_string())
        } else {
            let without_query = norm_uri.split(['?', '#']).next().unwrap_or(&norm_uri);
            let seg = without_query
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(&norm_uri);
            percent_decode_str(seg).decode_utf8_lossy().into_owned()
        };

        let ext = extract_lowercase_ext(&name);
        let guessed_mime = ext
            .as_deref()
            .and_then(|e| mime_guess::from_ext(e).first_raw());

        Self {
            uri: norm_uri,
            local_path,
            name,
            ext,
            guessed_mime,
        }
    }
}

/// Filesystem inspection facts produced asynchronously by the Enricher (`ACTION_SYSTEM.md` §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFacts {
    /// Resolved file kind.
    pub kind: FileKind,
    /// File size in bytes (`Some` for regular files).
    pub size: Option<u64>,
    /// Whether the file could be opened for reading.
    pub readable: bool,
    /// Content-based text vs. binary verdict.
    pub text: TextVerdict,
    /// Magic-number MIME type from content sniffing, if any.
    pub sniffed_mime: Option<&'static str>,
    /// Short user-presentable error string, if inspection failed.
    pub error: Option<String>,
}

impl FileFacts {
    /// Sentinel representing an item whose background enrichment has not completed yet.
    #[must_use]
    pub fn pending() -> Self {
        Self {
            kind: FileKind::Other,
            size: None,
            readable: false,
            text: TextVerdict::Unknown,
            sniffed_mime: None,
            error: None,
        }
    }

    /// Facts recorded when enrichment exceeds the per-session deadline (`ACTION_SYSTEM.md` §3, §4.2).
    #[must_use]
    pub fn timed_out() -> Self {
        Self {
            kind: FileKind::Other,
            size: None,
            readable: false,
            text: TextVerdict::Unknown,
            sniffed_mime: None,
            error: Some(ENRICH_TIMEOUT_REASON.to_string()),
        }
    }

    /// Facts recorded when an item's index is `>= MAX_ENRICHED_ITEMS` (`64`).
    #[must_use]
    pub fn cap_exceeded() -> Self {
        Self {
            kind: FileKind::Other,
            size: None,
            readable: false,
            text: TextVerdict::Unknown,
            sniffed_mime: None,
            error: Some(CAP_EXCEEDED_REASON.to_string()),
        }
    }

    /// Returns `true` if this entry is still awaiting background inspection.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.kind == FileKind::Other
            && self.size.is_none()
            && !self.readable
            && self.text == TextVerdict::Unknown
            && self.sniffed_mime.is_none()
            && self.error.is_none()
    }

    /// Returns `true` if this item was never inspected because its index exceeded
    /// [`MAX_ENRICHED_ITEMS`](crate::MAX_ENRICHED_ITEMS) (`"Too many items to inspect"`)
    /// or because enrichment timed out (`"Couldn't inspect the file"`).
    ///
    /// Actions that execute in an out-of-process worker (such as `ConvertAction`)
    /// treat uninspected items as optimistically eligible rather than hard-failed,
    /// letting the worker inspect and report per-item results at execution time.
    #[must_use]
    pub fn is_uninspected(&self) -> bool {
        self.kind == FileKind::Other
            && self.size.is_none()
            && !self.readable
            && self.text == TextVerdict::Unknown
            && self.sniffed_mime.is_none()
            && matches!(
                self.error.as_deref(),
                Some(CAP_EXCEEDED_REASON | ENRICH_TIMEOUT_REASON)
            )
    }
}

/// Kind of filesystem entry (`ACTION_SYSTEM.md` §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileKind {
    /// Regular file.
    Regular,
    /// Directory / folder.
    Directory,
    /// Named pipe (FIFO).
    Fifo,
    /// Unix domain socket.
    Socket,
    /// Character device.
    CharDevice,
    /// Block device.
    BlockDevice,
    /// Symlink whose target does not exist.
    BrokenSymlink,
    /// Path does not exist.
    Missing,
    /// Any other special file type.
    Other,
}

/// Verdict on whether a regular file is human-readable text (`ACTION_SYSTEM.md` §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextVerdict {
    /// File is text with the detected encoding.
    Text(TextEncoding),
    /// File is binary.
    Binary,
    /// Not yet inspected or not applicable.
    Unknown,
}

/// Detected text encoding (`ACTION_SYSTEM.md` §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextEncoding {
    /// UTF-8 without BOM.
    Utf8,
    /// UTF-8 with `EF BB BF` BOM.
    Utf8Bom,
    /// UTF-16 Little Endian (`FF FE` BOM).
    Utf16Le,
    /// UTF-16 Big Endian (`FE FF` BOM).
    Utf16Be,
    /// 8-bit text with <10% control bytes.
    Unknown8Bit,
}

/// Full session context combining [`BasicContext`] and optional [`FileFacts`] (`ACTION_SYSTEM.md` §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    /// Synchronous basic context.
    pub basic: BasicContext,
    /// Index-aligned per-item filesystem facts (`None` until enriched).
    pub facts: Option<Vec<FileFacts>>,
    /// Per-item flag indicating whether the path itself was a symbolic link.
    pub symlink: Vec<bool>,
}

impl Context {
    /// Creates an unenriched [`Context`] from a [`BasicContext`].
    #[must_use]
    pub fn from_basic(basic: BasicContext) -> Self {
        let len = basic.items.len();
        Self {
            basic,
            facts: None,
            symlink: vec![false; len],
        }
    }

    /// Converts any still-pending item facts into [`FileFacts::timed_out`] (`"Couldn't inspect the file"`).
    pub fn apply_enrichment_timeout(&mut self) {
        let len = self.basic.items.len();
        match &mut self.facts {
            None => {
                self.facts = Some(vec![FileFacts::timed_out(); len]);
            }
            Some(facts) => {
                for f in facts.iter_mut() {
                    if f.is_pending() {
                        *f = FileFacts::timed_out();
                    }
                }
                while facts.len() < len {
                    facts.push(FileFacts::timed_out());
                }
            }
        }
    }
}

/// Parses an RFC 2483 `text/uri-list` string into normalised URI strings.
///
/// Splits on `\r\n` or `\n`, ignores empty lines and lines starting with `#`.
#[must_use]
pub fn parse_uri_list(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in input.lines() {
        let trimmed = line.trim_end_matches('\r').trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        out.push(trimmed.to_string());
    }
    out
}

/// Conversion trait for CLI path or URI arguments (`&str`, `&OsStr`, `&Path`, `&Cow<'_, str>`, etc.).
pub trait CliArg {
    /// Borrows the argument as an [`OsStr`].
    fn as_cli_os_str(&self) -> &OsStr;
}

impl CliArg for str {
    fn as_cli_os_str(&self) -> &OsStr {
        OsStr::new(self)
    }
}

impl CliArg for String {
    fn as_cli_os_str(&self) -> &OsStr {
        OsStr::new(self.as_str())
    }
}

impl CliArg for OsStr {
    fn as_cli_os_str(&self) -> &OsStr {
        self
    }
}

impl CliArg for OsString {
    fn as_cli_os_str(&self) -> &OsStr {
        self.as_os_str()
    }
}

impl CliArg for Path {
    fn as_cli_os_str(&self) -> &OsStr {
        self.as_os_str()
    }
}

impl CliArg for PathBuf {
    fn as_cli_os_str(&self) -> &OsStr {
        self.as_os_str()
    }
}

impl CliArg for std::borrow::Cow<'_, str> {
    fn as_cli_os_str(&self) -> &OsStr {
        OsStr::new(self.as_ref())
    }
}

impl CliArg for std::borrow::Cow<'_, OsStr> {
    fn as_cli_os_str(&self) -> &OsStr {
        self.as_ref()
    }
}

/// Converts a CLI path or URI argument into a canonical `file://` (or existing scheme) URI (`ACTION_SYSTEM.md` §4.1).
///
/// Relative paths are resolved against `base_dir` without following symlinks (`std::fs::canonicalize`
/// is intentionally avoided so symlinks preserve their link path and broken symlinks can still be invoked).
/// Supports arbitrary non-UTF-8 Linux [`OsStr`] paths losslessly via percent-encoding.
pub fn normalize_cli_arg_to_uri<A: CliArg + ?Sized>(
    arg: &A,
    base_dir: &Path,
) -> Result<String, String> {
    let os = arg.as_cli_os_str();
    if os.is_empty() {
        return Err("empty path or URI argument".to_string());
    }
    if let Some(s) = os.to_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err("empty path or URI argument".to_string());
        }
        if let Some((scheme, _rest)) = trimmed.split_once("://")
            && !scheme.is_empty()
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
        {
            if trimmed.starts_with("file://")
                && let Some(local) = parse_file_uri_to_local_path(trimmed)
            {
                return Ok(local_path_to_file_uri(&local));
            }
            return Ok(trimmed.to_string());
        }

        let raw_path = Path::new(trimmed);
        let abs_path = if raw_path.is_absolute() {
            clean_lexical_path(raw_path)
        } else {
            clean_lexical_path(&base_dir.join(raw_path))
        };
        return Ok(local_path_to_file_uri(&abs_path));
    }

    let raw_path = Path::new(os);
    let abs_path = if raw_path.is_absolute() {
        clean_lexical_path(raw_path)
    } else {
        clean_lexical_path(&base_dir.join(raw_path))
    };

    Ok(local_path_to_file_uri(&abs_path))
}

/// Encodes an absolute local [`Path`] into a `file:///...` URI (RFC 8089),
/// losslessly percent-encoding raw [`OsStr`] bytes on Unix so non-UTF-8 filenames
/// round-trip through [`Item::from_uri`] without `U+FFFD` substitution.
#[must_use]
pub fn local_path_to_file_uri(path: &Path) -> String {
    #[cfg(unix)]
    let raw: &[u8] = std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str());
    #[cfg(not(unix))]
    let lossy = path.to_string_lossy();
    #[cfg(not(unix))]
    let raw: &[u8] = lossy.as_bytes();

    let mut uri = String::with_capacity(raw.len() + 16);
    uri.push_str("file://");
    for (i, seg) in raw.split(|&b| b == b'/').enumerate() {
        if i > 0 {
            uri.push('/');
        }
        if !seg.is_empty() {
            uri.push_str(&percent_encode(seg, PATH_ENCODE_SET).to_string());
        }
    }
    if !uri.starts_with("file:///") {
        uri = format!("file:///{}", uri.trim_start_matches("file://"));
    }
    uri
}

fn clean_lexical_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from("/")
    } else {
        out
    }
}

fn parse_file_uri_to_local_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path_part = if rest.starts_with('/') {
        rest
    } else if rest.starts_with("localhost/") {
        // Prepend the leading slash back
        &rest[("localhost".len())..]
    } else {
        return None;
    };

    // Strip any query or fragment if present in a URI, though rare in file://
    let decoded_bytes: Vec<u8> = percent_decode_str(path_part).collect();
    if decoded_bytes.first() != Some(&b'/') || decoded_bytes.contains(&0) {
        return None;
    }

    #[cfg(unix)]
    let os_str = {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(decoded_bytes)
    };
    #[cfg(not(unix))]
    let os_str = OsString::from(String::from_utf8_lossy(&decoded_bytes).into_owned());

    let pb = PathBuf::from(os_str);
    if pb.is_absolute() { Some(pb) } else { None }
}

fn extract_lowercase_ext(name: &str) -> Option<String> {
    let dot_pos = name.rfind('.')?;
    if dot_pos == 0 || dot_pos + 1 >= name.len() {
        return None;
    }
    Some(name[dot_pos + 1..].to_ascii_lowercase())
}
