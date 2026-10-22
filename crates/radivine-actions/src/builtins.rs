//! Built-in actions (`ACTION_SYSTEM.md` §8):
//! - `PreviewPlaceholderAction` (`radivine.preview.full`, `radivine.preview.head`, `radivine.preview.tail`)
//! - `CopyAction` (`radivine.copy`)
//! - `PropertiesAction` (`radivine.properties`)

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

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

/// Milestone M4 context-aware preview action (`radivine.preview.full`, `radivine.preview.head`, `radivine.preview.tail`).
///
/// Implements full M4 visibility and asynchronous `FileFacts` enablement rules (`ACTION_SYSTEM.md` §8),
/// while remaining a clearly identified M5 placeholder on execution (`IMPLEMENTATION_PLAN.md` §M4).
#[derive(Clone, Debug)]
pub struct PreviewPlaceholderAction {
    meta: ActionMeta,
}

impl PreviewPlaceholderAction {
    /// Creates a new [`PreviewPlaceholderAction`] with the given metadata.
    #[must_use]
    pub fn new(id: &str, label: &str, icon: &str, description: &str) -> Self {
        Self {
            meta: ActionMeta {
                id: ActionId::from(id),
                label: label.to_string(),
                icon: Some(IconKey::from(icon)),
                description: Some(description.to_string()),
                kinds: &[ContextKind::Files],
            },
        }
    }
}

impl Action for PreviewPlaceholderAction {
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
        let uris: Vec<&str> = ctx.basic.items.iter().map(|i| i.uri.as_str()).collect();
        tracing::info!(
            action_id = %self.meta.id,
            job_id = ?job.id(),
            mode = ?ctx.basic.mode,
            source_app = ?ctx.basic.source.app,
            uris = ?uris,
            "preview placeholder action selected (radivine-viewer is scheduled for M5)"
        );
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
