//! Test utilities for building synthetic [`Context`] instances and configurable [`Action`] stubs (`ACTION_SYSTEM.md` §13).

use crate::action::{Action, ActionError, ActionId, ActionMeta, Enablement, JobHandle, Outcome};
use crate::context::{BasicContext, Context, ContextKind, FileFacts, InvocationMode, SourceInfo};

/// Builds a synthetic [`Context`] from a slice of URI strings and optional [`FileFacts`].
#[must_use]
pub fn fake_context<I, S>(uris: I, facts: Option<Vec<FileFacts>>) -> Context
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let basic = BasicContext::from_uris(uris, InvocationMode::Invoke, SourceInfo::default());
    let len = basic.items.len();
    Context {
        basic,
        facts,
        symlink: vec![false; len],
    }
}

type VisiblePred = Box<dyn Fn(&BasicContext) -> bool + Send + Sync>;
type EnablementPred = Box<dyn Fn(&Context) -> Enablement + Send + Sync>;
type RunFn = Box<dyn Fn(&Context, &JobHandle) -> Result<Outcome, ActionError> + Send + Sync>;

/// Configurable [`Action`] stub for unit testing menu building, enablement transitions, and dispatching.
pub struct FakeAction {
    meta: ActionMeta,
    visible: VisiblePred,
    enablement_fn: EnablementPred,
    run_fn: RunFn,
}

impl FakeAction {
    /// Creates a [`FakeAction`] with default visibility (`true`), enablement (`Enabled`), and outcome (`Done`).
    #[must_use]
    pub fn new(id: impl Into<ActionId>, label: impl Into<String>) -> Self {
        Self {
            meta: ActionMeta {
                id: id.into(),
                label: label.into(),
                icon: None,
                description: None,
                kinds: &[ContextKind::Files],
            },
            visible: Box::new(|_| true),
            enablement_fn: Box::new(|_| Enablement::Enabled),
            run_fn: Box::new(|_, _| Ok(Outcome::Done)),
        }
    }

    /// Overrides the visibility predicate.
    #[must_use]
    pub fn with_visible<F>(mut self, f: F) -> Self
    where
        F: Fn(&BasicContext) -> bool + Send + Sync + 'static,
    {
        self.visible = Box::new(f);
        self
    }

    /// Overrides the enablement predicate.
    #[must_use]
    pub fn with_enablement<F>(mut self, f: F) -> Self
    where
        F: Fn(&Context) -> Enablement + Send + Sync + 'static,
    {
        self.enablement_fn = Box::new(f);
        self
    }

    /// Overrides the execution handler.
    #[must_use]
    pub fn with_run<F>(mut self, f: F) -> Self
    where
        F: Fn(&Context, &JobHandle) -> Result<Outcome, ActionError> + Send + Sync + 'static,
    {
        self.run_fn = Box::new(f);
        self
    }
}

impl Action for FakeAction {
    fn meta(&self) -> &ActionMeta {
        &self.meta
    }

    fn is_visible(&self, ctx: &BasicContext) -> bool {
        (self.visible)(ctx)
    }

    fn enablement(&self, ctx: &Context) -> Enablement {
        (self.enablement_fn)(ctx)
    }

    fn run(&self, ctx: &Context, job: &JobHandle) -> Result<Outcome, ActionError> {
        (self.run_fn)(ctx, job)
    }
}
