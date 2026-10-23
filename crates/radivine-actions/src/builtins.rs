//! Built-in actions (`ACTION_SYSTEM.md` §8):
//! - `PreviewAction` (`radivine.preview.full`, `radivine.preview.head`, `radivine.preview.tail`)
//! - `CopyAction` (`radivine.copy`)
//! - `PropertiesAction` (`radivine.properties`)

use std::borrow::Cow;
use std::ffi::OsString;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;

use radivine_core::{
    Action, ActionError, ActionId, ActionMeta, BasicContext, ClipOp, ClipboardRequest, Context,
    ContextKind, Effect, Enablement, FileFacts, FileKind, IconKey, JobHandle, Outcome, TextVerdict,
};

use crate::dbus::call_show_item_properties;

/// Returns `true` if `mime` belongs to a known binary/non-text MIME family (`ACTION_SYSTEM.md` §8).
#[must_use]
pub fn is_known_non_text_mime(mime: Option<&str>) -> bool {
    let Some(m) = mime else {
        return false;
    };
    if m.starts_with("image/")
        || m.starts_with("audio/")
        || m.starts_with("video/")
        || m.starts_with("font/")
        || m.starts_with("model/")
        || m.starts_with("application/vnd.openxmlformats-officedocument")
        || m.starts_with("application/vnd.oasis.opendocument")
    {
        return true;
    }
    matches!(
        m,
        "application/pdf"
            | "application/zip"
            | "application/gzip"
            | "application/x-gzip"
            | "application/x-tar"
            | "application/x-bzip"
            | "application/x-bzip2"
            | "application/x-xz"
            | "application/zstd"
            | "application/x-7z-compressed"
            | "application/vnd.rar"
            | "application/x-rar-compressed"
            | "application/x-executable"
            | "application/x-sharedlib"
            | "application/x-object"
            | "application/wasm"
            | "application/octet-stream"
            | "application/java-archive"
            | "application/vnd.sqlite3"
            | "application/x-sqlite3"
            | "application/msword"
            | "application/vnd.ms-excel"
            | "application/vnd.ms-powerpoint"
    )
}

/// Evaluates [`Enablement`] for a single [`FileFacts`] entry requiring a readable text file (`ACTION_SYSTEM.md` §8).
#[must_use]
pub fn evaluate_text_file_facts(facts: &FileFacts) -> Enablement {
    if facts.is_pending() {
        return Enablement::Pending;
    }
    match facts.kind {
        FileKind::Directory => Enablement::Disabled {
            reason: Cow::Borrowed("Folders can't be previewed"),
        },
        FileKind::BrokenSymlink => Enablement::Disabled {
            reason: Cow::Borrowed("Broken link"),
        },
        FileKind::Missing => Enablement::Disabled {
            reason: Cow::Borrowed("File not found"),
        },
        FileKind::Fifo | FileKind::Socket | FileKind::CharDevice | FileKind::BlockDevice => {
            Enablement::Disabled {
                reason: Cow::Borrowed("Special file"),
            }
        }
        FileKind::Other => {
            if let Some(ref err) = facts.error {
                if err == "Couldn't inspect the file" {
                    return Enablement::Disabled {
                        reason: Cow::Borrowed("Couldn't inspect the file"),
                    };
                }
                if err == "No permission to read" {
                    return Enablement::Disabled {
                        reason: Cow::Borrowed("No permission to read"),
                    };
                }
            }
            Enablement::Disabled {
                reason: Cow::Borrowed("Special file"),
            }
        }
        FileKind::Regular => {
            if !facts.readable {
                return Enablement::Disabled {
                    reason: Cow::Borrowed("No permission to read"),
                };
            }
            match facts.text {
                TextVerdict::Text(_) => Enablement::Enabled,
                TextVerdict::Binary => Enablement::Disabled {
                    reason: Cow::Borrowed("Not a text file"),
                },
                TextVerdict::Unknown => Enablement::Disabled {
                    reason: Cow::Borrowed("Couldn't inspect the file"),
                },
            }
        }
    }
}

/// Preview mode launched by [`PreviewAction`] (`ACTION_SYSTEM.md` §8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewKind {
    /// `radivine.preview.full` (`--mode full`).
    Full,
    /// `radivine.preview.head` (`--mode head --lines <N>`).
    Head,
    /// `radivine.preview.tail` (`--mode tail --lines <N>`).
    Tail,
}

impl PreviewKind {
    /// Returns the `--mode` CLI flag value for `radivine-viewer`.
    #[must_use]
    pub const fn mode_arg(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Head => "head",
            Self::Tail => "tail",
        }
    }
}

/// Preview behavior settings from `config.toml` (`[preview] head_lines`, `tail_lines`, and `[appearance] theme`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewSettings {
    /// Number of lines for `radivine.preview.head` (default: `50`).
    pub head_lines: usize,
    /// Number of lines for `radivine.preview.tail` (default: `50`).
    pub tail_lines: usize,
    /// Color theme passed to `radivine-viewer` (`"dark"`, `"light"`, or `"auto"`).
    pub theme: String,
}

impl Default for PreviewSettings {
    fn default() -> Self {
        Self {
            head_lines: 50,
            tail_lines: 50,
            theme: "dark".to_string(),
        }
    }
}

/// Resolves the path to the `radivine-viewer` executable (`ACTION_SYSTEM.md` §8.1):
/// 1. `RADIVINE_VIEWER_BIN` environment variable override (if set and non-empty),
/// 2. Sibling executable next to `current_exe()` (`current_exe().parent()/radivine-viewer`),
/// 3. Parent directory of `current_exe().parent()` (`target/debug/radivine-viewer` when running under `target/debug/deps/`),
/// 4. Fallback to `"radivine-viewer"` in `PATH`.
#[must_use]
pub fn resolve_viewer_binary() -> PathBuf {
    if let Some(env_bin) = std::env::var_os("RADIVINE_VIEWER_BIN")
        && !env_bin.is_empty()
    {
        return PathBuf::from(env_bin);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("radivine-viewer");
        if sibling.is_file() {
            return sibling;
        }
        if let Some(parent_dir) = dir.parent() {
            let parent_sibling = parent_dir.join("radivine-viewer");
            if parent_sibling.is_file() {
                return parent_sibling;
            }
        }
    }
    PathBuf::from("radivine-viewer")
}

/// Real context-aware file preview action (`radivine.preview.full`, `radivine.preview.head`, `radivine.preview.tail`)
/// (`ACTION_SYSTEM.md` §8 & §8.1).
///
/// Spawns `radivine-viewer` as a detached process with `--mode <full|head|tail>`, `--lines <N>`,
/// `--theme <dark|light|auto>`, and `-- <path>`.
#[derive(Clone, Debug)]
pub struct PreviewAction {
    meta: ActionMeta,
    kind: PreviewKind,
    lines: usize,
    theme: String,
}

/// Backwards-compatible alias for [`PreviewAction`].
pub type PreviewPlaceholderAction = PreviewAction;

impl PreviewAction {
    /// Creates a new [`PreviewAction`] with the given metadata and default settings (`50` lines, `"dark"` theme).
    #[must_use]
    pub fn new(id: &str, label: &str, icon: &str, description: &str) -> Self {
        let kind = match id {
            "radivine.preview.head" => PreviewKind::Head,
            "radivine.preview.tail" => PreviewKind::Tail,
            _ => PreviewKind::Full,
        };
        Self::with_settings(id, label, icon, description, kind, 50, "dark")
    }

    /// Creates a new [`PreviewAction`] with explicit [`PreviewKind`], line count, and theme.
    #[must_use]
    pub fn with_settings(
        id: &str,
        label: &str,
        icon: &str,
        description: &str,
        kind: PreviewKind,
        lines: usize,
        theme: impl Into<String>,
    ) -> Self {
        Self {
            meta: ActionMeta {
                id: ActionId::from(id),
                label: label.to_string(),
                icon: Some(IconKey::from(icon)),
                description: Some(description.to_string()),
                kinds: &[ContextKind::Files],
            },
            kind,
            lines: lines.max(1),
            theme: theme.into(),
        }
    }

    /// Returns the [`PreviewKind`] of this preview action.
    #[must_use]
    pub const fn kind(&self) -> PreviewKind {
        self.kind
    }

    /// Returns the configured line count for `Head` or `Tail` mode.
    #[must_use]
    pub const fn lines(&self) -> usize {
        self.lines
    }

    /// Builds the exact CLI argument vector passed to `radivine-viewer` for `path`.
    #[must_use]
    pub fn build_viewer_args(&self, path: &Path) -> Vec<OsString> {
        let mut args = vec![
            OsString::from("--mode"),
            OsString::from(self.kind.mode_arg()),
        ];
        if matches!(self.kind, PreviewKind::Head | PreviewKind::Tail) {
            args.push(OsString::from("--lines"));
            args.push(OsString::from(self.lines.to_string()));
        }
        let normalized_theme = match self.theme.trim().to_ascii_lowercase().as_str() {
            "light" => "light",
            "auto" => "auto",
            _ => "dark",
        };
        args.push(OsString::from("--theme"));
        args.push(OsString::from(normalized_theme));
        args.push(OsString::from("--"));
        args.push(path.as_os_str().to_os_string());
        args
    }
}

impl Action for PreviewAction {
    fn meta(&self) -> &ActionMeta {
        &self.meta
    }

    fn is_visible(&self, ctx: &BasicContext) -> bool {
        if ctx.kind != ContextKind::Files || ctx.items.is_empty() {
            return false;
        }
        ctx.items
            .iter()
            .all(|it| it.local_path.is_some() && !is_known_non_text_mime(it.guessed_mime))
    }

    fn enablement(&self, ctx: &Context) -> Enablement {
        if ctx.basic.items.len() != 1 {
            return Enablement::Disabled {
                reason: Cow::Borrowed("Select a single file to preview"),
            };
        }
        let Some(ref facts_vec) = ctx.facts else {
            return Enablement::Pending;
        };
        let Some(first_facts) = facts_vec.first() else {
            return Enablement::Pending;
        };
        evaluate_text_file_facts(first_facts)
    }

    fn run(&self, ctx: &Context, job: &JobHandle) -> Result<Outcome, ActionError> {
        if job.is_cancelled() {
            return Err(ActionError::Cancelled);
        }
        if ctx.basic.items.len() != 1 {
            return Err(ActionError::NotApplicable(
                "Select a single file to preview".to_string(),
            ));
        }
        let Some(ref local_path) = ctx.basic.items[0].local_path else {
            return Err(ActionError::NotApplicable(
                "item is not a local file".to_string(),
            ));
        };
        if let Some(ref facts_vec) = ctx.facts
            && let Some(first_facts) = facts_vec.first()
            && let Enablement::Disabled { reason } = evaluate_text_file_facts(first_facts)
        {
            return Err(ActionError::NotApplicable(reason.into_owned()));
        }

        if std::env::var("RADIVINE_DRY_RUN_VIEWER").ok().as_deref() == Some("1") {
            tracing::info!(
                action_id = %self.meta.id,
                job_id = ?job.id(),
                path = %local_path.display(),
                "RADIVINE_DRY_RUN_VIEWER=1 set; skipping live radivine-viewer spawn"
            );
            return Ok(Outcome::Done);
        }

        let viewer_bin = resolve_viewer_binary();
        let program_display = viewer_bin.display().to_string();
        let args = self.build_viewer_args(local_path);

        tracing::info!(
            action_id = %self.meta.id,
            job_id = ?job.id(),
            viewer = %program_display,
            mode = self.kind.mode_arg(),
            lines = self.lines,
            path = %local_path.display(),
            "spawning detached radivine-viewer process"
        );

        let mut cmd = Command::new(&viewer_bin);
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .process_group(0);

        let mut child = cmd.spawn().map_err(|source| ActionError::Spawn {
            program: program_display.clone(),
            source,
        })?;

        let action_id_str = self.meta.id.as_str().to_string();
        let path_str = local_path.display().to_string();
        let _ = thread::Builder::new()
            .name("radivine-viewer-reap".to_string())
            .spawn(move || {
                let mut stderr_buf = String::new();
                if let Some(mut err_pipe) = child.stderr.take() {
                    let _ = err_pipe.read_to_string(&mut stderr_buf);
                }
                match child.wait() {
                    Ok(status) if !status.success() => {
                        tracing::warn!(
                            action_id = %action_id_str,
                            path = %path_str,
                            status = %status,
                            stderr = %stderr_buf.trim(),
                            "radivine-viewer exited with non-zero status"
                        );
                    }
                    Ok(_) => {
                        if !stderr_buf.trim().is_empty() {
                            tracing::debug!(
                                action_id = %action_id_str,
                                path = %path_str,
                                stderr = %stderr_buf.trim(),
                                "radivine-viewer stderr output"
                            );
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            action_id = %action_id_str,
                            path = %path_str,
                            error = %err,
                            "failed to wait on radivine-viewer child"
                        );
                    }
                }
            });

        Ok(Outcome::Done)
    }
}

/// Real `radivine.copy` action (`ACTION_SYSTEM.md` §8).
///
/// Emits `Effect::SetClipboard(ClipboardRequest { uris, local_paths, op: ClipOp::Copy })` so the
/// UI thread takes ownership of the X11 `CLIPBOARD` selection and serves `x-special/gnome-copied-files`,
/// `text/uri-list`, and `UTF8_STRING` without modifying the source files.
#[derive(Clone, Debug)]
pub struct CopyAction {
    meta: ActionMeta,
}

impl Default for CopyAction {
    fn default() -> Self {
        Self::new()
    }
}

impl CopyAction {
    /// Creates the built-in `radivine.copy` action.
    #[must_use]
    pub fn new() -> Self {
        Self {
            meta: ActionMeta {
                id: ActionId::from("radivine.copy"),
                label: "Copy".to_string(),
                icon: Some(IconKey::from("lucide:copy")),
                description: Some("Copy selected file to clipboard".to_string()),
                kinds: &[ContextKind::Files],
            },
        }
    }
}

impl Action for CopyAction {
    fn meta(&self) -> &ActionMeta {
        &self.meta
    }

    fn is_visible(&self, ctx: &BasicContext) -> bool {
        ctx.kind == ContextKind::Files && !ctx.items.is_empty()
    }

    fn enablement(&self, _ctx: &Context) -> Enablement {
        Enablement::Enabled
    }

    fn run(&self, ctx: &Context, job: &JobHandle) -> Result<Outcome, ActionError> {
        if job.is_cancelled() {
            return Err(ActionError::Cancelled);
        }
        let uris: Vec<String> = ctx.basic.items.iter().map(|i| i.uri.clone()).collect();
        let local_paths: Vec<PathBuf> = ctx
            .basic
            .items
            .iter()
            .filter_map(|i| i.local_path.clone())
            .collect();

        tracing::info!(
            action_id = %self.meta.id,
            job_id = ?job.id(),
            count = uris.len(),
            uris = ?uris,
            "copying file URIs to desktop clipboard"
        );

        Ok(Outcome::Effects(vec![Effect::SetClipboard(
            ClipboardRequest {
                uris,
                local_paths,
                op: ClipOp::Copy,
            },
        )]))
    }
}

/// Pluggable backend for [`PropertiesAction`] (`org.freedesktop.FileManager1.ShowItemProperties`).
pub trait PropertiesBackend: Send + Sync + 'static {
    /// Requests the desktop file manager to display the native properties dialog for `uris`.
    fn show_item_properties(&self, uris: &[String]) -> Result<(), ActionError>;
}

/// Default [`PropertiesBackend`] invoking `org.freedesktop.FileManager1.ShowItemProperties` over session D-Bus.
#[derive(Clone, Debug, Default)]
pub struct SessionDbusPropertiesBackend {
    socket_override: Option<PathBuf>,
}

impl SessionDbusPropertiesBackend {
    /// Creates a [`SessionDbusPropertiesBackend`] connected to the default session bus.
    #[must_use]
    pub fn new() -> Self {
        Self {
            socket_override: None,
        }
    }

    /// Creates a [`SessionDbusPropertiesBackend`] using a specific Unix domain socket path (for testing).
    #[must_use]
    pub fn with_socket(path: PathBuf) -> Self {
        Self {
            socket_override: Some(path),
        }
    }
}

impl PropertiesBackend for SessionDbusPropertiesBackend {
    fn show_item_properties(&self, uris: &[String]) -> Result<(), ActionError> {
        if std::env::var("RADIVINE_DRY_RUN_PROPERTIES").ok().as_deref() == Some("1") {
            tracing::info!(
                uris = ?uris,
                "RADIVINE_DRY_RUN_PROPERTIES=1 set; skipping live D-Bus ShowItemProperties call"
            );
            return Ok(());
        }

        let sock_override = self.socket_override.as_deref();
        match call_show_item_properties(sock_override, uris, "") {
            Ok(()) => Ok(()),
            Err(err) if err.is_service_unsupported() => Err(ActionError::Other(
                "File manager doesn't support properties".to_string(),
            )),
            Err(err) => Err(ActionError::Other(format!(
                "Could not open file properties: {err}"
            ))),
        }
    }
}

/// Real `radivine.properties` action (`ACTION_SYSTEM.md` §8).
pub struct PropertiesAction {
    meta: ActionMeta,
    backend: Arc<dyn PropertiesBackend>,
}

impl Default for PropertiesAction {
    fn default() -> Self {
        Self::new()
    }
}

impl PropertiesAction {
    /// Creates the built-in `radivine.properties` action using [`SessionDbusPropertiesBackend`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_backend(Arc::new(SessionDbusPropertiesBackend::new()))
    }

    /// Creates `radivine.properties` with a custom [`PropertiesBackend`] (used in unit tests).
    #[must_use]
    pub fn with_backend(backend: Arc<dyn PropertiesBackend>) -> Self {
        Self {
            meta: ActionMeta {
                id: ActionId::from("radivine.properties"),
                label: "Properties".to_string(),
                icon: Some(IconKey::from("lucide:info")),
                description: Some("Show file properties".to_string()),
                kinds: &[ContextKind::Files],
            },
            backend,
        }
    }
}

impl Action for PropertiesAction {
    fn meta(&self) -> &ActionMeta {
        &self.meta
    }

    fn is_visible(&self, ctx: &BasicContext) -> bool {
        ctx.kind == ContextKind::Files && !ctx.items.is_empty()
    }

    fn enablement(&self, _ctx: &Context) -> Enablement {
        Enablement::Enabled
    }

    fn run(&self, ctx: &Context, job: &JobHandle) -> Result<Outcome, ActionError> {
        if job.is_cancelled() {
            return Err(ActionError::Cancelled);
        }
        let uris: Vec<String> = ctx.basic.items.iter().map(|i| i.uri.clone()).collect();
        tracing::info!(
            action_id = %self.meta.id,
            job_id = ?job.id(),
            uris = ?uris,
            "invoking org.freedesktop.FileManager1.ShowItemProperties"
        );
        self.backend.show_item_properties(&uris)?;
        Ok(Outcome::Done)
    }
}
