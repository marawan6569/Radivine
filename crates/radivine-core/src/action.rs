//! `Action` trait, metadata, outcomes, effects, and `LogAction` (`ACTION_SYSTEM.md` §5, §8).

use std::borrow::Cow;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use radivine_wheel::IconKey;

use crate::context::{BasicContext, Context, ContextKind};

/// Stable, namespaced identifier for an action (`"radivine.preview.head"`, `"custom.open-in-code"`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActionId(pub Arc<str>);

impl ActionId {
    /// Creates an [`ActionId`] from a string slice or `Arc<str>`.
    #[inline]
    #[must_use]
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        Self(id.into())
    }

    /// Returns the string slice of this action ID.
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ActionId {
    fn from(s: &str) -> Self {
        Self(Arc::from(s))
    }
}

impl From<String> for ActionId {
    fn from(s: String) -> Self {
        Self(Arc::from(s))
    }
}

impl fmt::Display for ActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Static metadata describing an [`Action`] (`ACTION_SYSTEM.md` §5).
#[derive(Clone, Debug)]
pub struct ActionMeta {
    /// Stable action identifier.
    pub id: ActionId,
    /// Short display label shown inside the radial segment (`<= 16` chars recommended).
    pub label: String,
    /// Optional icon key (`"lucide:eye"`, `"theme:document-properties"`).
    pub icon: Option<IconKey>,
    /// Optional longer description shown in the caption pill.
    pub description: Option<String>,
    /// Context kinds supported by this action.
    pub kinds: &'static [ContextKind],
}

/// Enablement state of an [`Action`] for a given [`Context`] (`ACTION_SYSTEM.md` §3, §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Enablement {
    /// Action is enabled and can be committed.
    Enabled,
    /// Action is visible in the wheel but disabled with a human-readable reason.
    Disabled {
        /// Reason shown in the wheel caption when hovered.
        reason: Cow<'static, str>,
    },
    /// Enablement depends on [`crate::FileFacts`] that are still being computed ("Checking…").
    Pending,
}

/// Unique identifier for a dispatched action job (`ACTION_SYSTEM.md` §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JobId(pub u64);

/// Cooperative cancellation handle passed to [`Action::run`] (`ACTION_SYSTEM.md` §5).
#[derive(Clone, Debug)]
pub struct JobHandle {
    id: JobId,
    cancelled: Arc<AtomicBool>,
}

impl JobHandle {
    /// Creates a new [`JobHandle`] for `id` backed by `cancelled`.
    #[must_use]
    pub fn new(id: JobId, cancelled: Arc<AtomicBool>) -> Self {
        Self { id, cancelled }
    }

    /// Returns `true` if cancellation has been requested for this job.
    #[inline]
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Returns the [`JobId`] of this execution.
    #[inline]
    #[must_use]
    pub fn id(&self) -> JobId {
        self.id
    }
}

/// Clipboard operation kind (`ACTION_SYSTEM.md` §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipOp {
    /// Copy files/URIs to the clipboard (`Cut` is intentionally omitted for data safety).
    Copy,
}

/// Clipboard ownership request returned by [`Effect::SetClipboard`] (`ACTION_SYSTEM.md` §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardRequest {
    /// Normalised URI strings (`file:///...`).
    pub uris: Vec<String>,
    /// Local filesystem paths corresponding to `uris`.
    pub local_paths: Vec<PathBuf>,
    /// Clipboard operation (`ClipOp::Copy`).
    pub op: ClipOp,
}

/// Side effect requested by an [`Action`] to be executed by the host application (`ACTION_SYSTEM.md` §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Own the system clipboard with the given files/URIs on the UI thread.
    SetClipboard(ClipboardRequest),
    /// Show a desktop notification via the notification worker.
    Notify {
        /// Notification summary line.
        summary: String,
        /// Optional notification body text.
        body: Option<String>,
    },
}

/// Successful return value of [`Action::run`] (`ACTION_SYSTEM.md` §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Completed silently.
    Done,
    /// Completed with a user-facing notification message.
    Message(String),
    /// Completed with one or more host [`Effect`]s.
    Effects(Vec<Effect>),
}

/// Error returned by [`Action::run`] (`ACTION_SYSTEM.md` §5).
#[derive(thiserror::Error, Debug)]
pub enum ActionError {
    /// Action is not applicable to the provided context.
    #[error("not applicable: {0}")]
    NotApplicable(String),
    /// Job was cooperatively cancelled.
    #[error("cancelled")]
    Cancelled,
    /// Underlying I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to spawn an external program.
    #[error("could not start {program}: {source}")]
    Spawn {
        /// Executable name or path.
        program: String,
        /// OS error from `Command::spawn`.
        source: std::io::Error,
    },
    /// Any other action failure.
    #[error("{0}")]
    Other(String),
}

/// Trait implemented by all built-in and custom actions (`ACTION_SYSTEM.md` §5).
pub trait Action: Send + Sync + 'static {
    /// Returns the static metadata of this action.
    fn meta(&self) -> &ActionMeta;

    /// Determines whether this action appears in the wheel for `ctx`.
    ///
    /// **MUST NOT perform I/O.** Called synchronously on the UI thread.
    fn is_visible(&self, ctx: &BasicContext) -> bool;

    /// Determines whether this action is enabled for `ctx`.
    ///
    /// **MUST NOT perform I/O.** Called on the UI thread.
    fn enablement(&self, ctx: &Context) -> Enablement {
        let _ = ctx;
        Enablement::Enabled
    }

    /// Executes the action on a background worker thread.
    fn run(&self, ctx: &Context, job: &JobHandle) -> Result<Outcome, ActionError>;
}

/// Placeholder action used in M1–M3 that logs the selected action ID, invocation mode,
/// source app, and exact item URIs at `INFO` level (`ACTION_SYSTEM.md` §8).
#[derive(Clone, Debug)]
pub struct LogAction {
    meta: ActionMeta,
    visibility_fn: Option<fn(&BasicContext) -> bool>,
}

impl LogAction {
    /// Creates a new [`LogAction`] with the given [`ActionMeta`].
    #[must_use]
    pub fn new(meta: ActionMeta) -> Self {
        Self {
            meta,
            visibility_fn: None,
        }
    }

    /// Attaches a custom visibility predicate to this [`LogAction`].
    #[must_use]
    pub fn with_visibility(mut self, pred: fn(&BasicContext) -> bool) -> Self {
        self.visibility_fn = Some(pred);
        self
    }
}

impl Action for LogAction {
    fn meta(&self) -> &ActionMeta {
        &self.meta
    }

    fn is_visible(&self, ctx: &BasicContext) -> bool {
        if !self.meta.kinds.contains(&ctx.kind) || ctx.items.is_empty() {
            return false;
        }
        if let Some(pred) = self.visibility_fn {
            pred(ctx)
        } else {
            true
        }
    }

    fn run(&self, ctx: &Context, job: &JobHandle) -> Result<Outcome, ActionError> {
        if job.is_cancelled() {
            return Err(ActionError::Cancelled);
        }
        let uris: Vec<&str> = ctx.basic.items.iter().map(|i| i.uri.as_str()).collect();
        tracing::info!(
            target: "radivine::commit",
            action_id = %self.meta.id,
            label = %self.meta.label,
            mode = ?ctx.basic.mode,
            source_app = ?ctx.basic.source.app,
            source_pid = ?ctx.basic.source.pid,
            uris = ?uris,
            "LogAction executed"
        );
        Ok(Outcome::Done)
    }
}
