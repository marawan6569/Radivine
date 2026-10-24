//! Quick Convert wheel actions (`radivine.convert.image.{png,jpeg,webp}`) and
//! shared [`ConvertMenuContext`] (`CONVERSION_ARCHITECTURE.md` §7, `CONVERSION_UX.md` §1).
//!
//! Enforces strict UI-thread invariants (`AC-C3.4`, `AC-C3.5`, `AC-C3.6`):
//! - [`ConvertAction::is_visible`] and [`ConvertAction::description_for`] perform **zero
//!   filesystem I/O** and inspect only [`BasicContext`] and the cached [`CapabilityCatalog`].
//! - [`ConvertAction::enablement`] performs **zero filesystem I/O** and inspects only
//!   already-populated [`FileFacts`] from background enrichment.
//! - [`ConvertAction::run`] never reads `convert.toml` and never decodes images in the daemon;
//!   it submits a durable `convert.image` job to `radivine-worker` via [`JobsClient`].

use std::borrow::Cow;
use std::sync::{Arc, RwLock};

use radivine_convert::{
    CapabilityCatalog, FormatId, TargetStatus, format, format_from_extension, format_from_mime,
};
use radivine_core::{
    Action, ActionError, ActionId, ActionMeta, BasicContext, Context, ContextKind, Enablement,
    FileKind, IconKey, JobHandle, Outcome, RegistryError,
};
use radivine_jobs::{
    CONVERT_IMAGE_KIND, JobsClient, Origin, RequestId, WirePath, WorkerRuntimePaths,
};

/// Shared, hot-updatable context for conversion actions in the radial wheel (`E3`).
///
/// Constructed once at daemon startup with [`CapabilityCatalog::default_builtin`] (zero
/// filesystem or config I/O).
#[derive(Clone, Debug)]
pub struct ConvertMenuContext {
    catalog: Arc<RwLock<Arc<CapabilityCatalog>>>,
    runtime_paths: Option<WorkerRuntimePaths>,
}

impl ConvertMenuContext {
    /// Creates a new [`ConvertMenuContext`] backed by the given [`CapabilityCatalog`].
    #[must_use]
    pub fn new(catalog: Arc<CapabilityCatalog>) -> Self {
        Self {
            catalog: Arc::new(RwLock::new(catalog)),
            runtime_paths: None,
        }
    }

    /// Creates a [`ConvertMenuContext`] with explicit [`WorkerRuntimePaths`] (used by isolated tests).
    #[must_use]
    pub fn with_runtime_paths(
        catalog: Arc<CapabilityCatalog>,
        runtime_paths: WorkerRuntimePaths,
    ) -> Self {
        Self {
            catalog: Arc::new(RwLock::new(catalog)),
            runtime_paths: Some(runtime_paths),
        }
    }

    /// Atomically replaces the cached [`CapabilityCatalog`].
    pub fn update_catalog(&self, catalog: Arc<CapabilityCatalog>) {
        if let Ok(mut guard) = self.catalog.write() {
            *guard = catalog;
        }
    }

    /// Returns a clone of the current [`CapabilityCatalog`] [`Arc`].
    #[must_use]
    pub fn catalog(&self) -> Arc<CapabilityCatalog> {
        self.catalog
            .read()
            .expect("ConvertMenuContext RwLock poisoned")
            .clone()
    }

    /// Returns the configured [`WorkerRuntimePaths`] override, if any.
    #[must_use]
    pub fn runtime_paths(&self) -> Option<WorkerRuntimePaths> {
        self.runtime_paths.clone()
    }
}

impl Default for ConvertMenuContext {
    fn default() -> Self {
        Self::new(Arc::new(CapabilityCatalog::default_builtin()))
    }
}

/// Quick Convert leaf action for a single target [`FormatId`] (`png`, `jpeg`, or `webp`).
#[derive(Debug)]
pub struct ConvertAction {
    target: FormatId,
    meta: ActionMeta,
    menu_ctx: ConvertMenuContext,
}

impl ConvertAction {
    /// Creates a new [`ConvertAction`] for `target` sharing `menu_ctx`.
    #[must_use]
    pub fn new(target: FormatId, menu_ctx: ConvertMenuContext) -> Self {
        let info = format(target);
        let id = format!("radivine.convert.image.{}", info.id.as_str());
        Self {
            target,
            meta: ActionMeta {
                id: ActionId::from(id),
                label: info.display_name.to_string(),
                icon: Some(IconKey::from("lucide:file-image")),
                description: Some(format!("Convert to {}", info.display_name)),
                kinds: &[ContextKind::Files],
            },
            menu_ctx,
        }
    }

    /// Returns the target [`FormatId`] of this action.
    #[must_use]
    pub const fn target(&self) -> FormatId {
        self.target
    }

    #[inline]
    const fn fmt_idx(fmt: FormatId) -> usize {
        match fmt {
            FormatId::Png => 0,
            FormatId::Jpeg => 1,
            FormatId::WebP => 2,
            _ => 3,
        }
    }

    /// Precomputes `(is_input, is_routable_to_target)` for each of `[Png, Jpeg, WebP]` in `O(1)`
    /// before scanning up to 10,000 items (`AC-C3.5`).
    fn precompute_format_table(&self) -> ([bool; 4], [bool; 4]) {
        let catalog = self.menu_ctx.catalog();
        let inputs = catalog.inputs();
        let mut is_input = [false; 4];
        let mut is_routable = [false; 4];
        for &fmt in FormatId::ALL {
            let idx = Self::fmt_idx(fmt);
            if inputs.contains(&fmt) {
                is_input[idx] = true;
                if fmt != self.target
                    && !matches!(
                        catalog.target_status(fmt, self.target),
                        TargetStatus::Unsupported
                    )
                {
                    is_routable[idx] = true;
                }
            }
        }
        (is_input, is_routable)
    }
}

impl Action for ConvertAction {
    fn meta(&self) -> &ActionMeta {
        &self.meta
    }

    /// Fast, zero-I/O visibility check using only [`BasicContext`] and [`CapabilityCatalog`].
    ///
    /// Visible when at least one selected item is a local file whose lowercase extension maps to
    /// a supported input image format `src_fmt != self.target` with a non-`Unsupported` route.
    fn is_visible(&self, ctx: &BasicContext) -> bool {
        if ctx.kind != ContextKind::Files || ctx.items.is_empty() {
            return false;
        }
        let (_, is_routable) = self.precompute_format_table();
        ctx.items.iter().any(|item| {
            item.local_path.is_some()
                && item
                    .ext
                    .as_deref()
                    .and_then(format_from_extension)
                    .is_some_and(|src_fmt| is_routable[Self::fmt_idx(src_fmt)])
        })
    }

    /// Fast, zero-I/O per-session caption computed purely from [`BasicContext`] extensions.
    ///
    /// Never changes during progressive enrichment (`AC-C3.3`, `AC-C3.4`).
    fn description_for(&self, ctx: &BasicContext) -> Option<Cow<'_, str>> {
        let t_name = format(self.target).display_name;
        let total = ctx.items.len();
        if total == 0 {
            return Some(Cow::Owned(format!("Convert to {t_name}")));
        }

        let (is_input, is_routable) = self.precompute_format_table();

        let mut convertible = 0usize;
        let mut same_fmt = 0usize;
        for item in &ctx.items {
            if item.local_path.is_none() {
                continue;
            }
            if let Some(src_fmt) = item.ext.as_deref().and_then(format_from_extension) {
                let idx = Self::fmt_idx(src_fmt);
                if is_input[idx] {
                    if src_fmt == self.target {
                        same_fmt += 1;
                    } else if is_routable[idx] {
                        convertible += 1;
                    }
                }
            }
        }
        let unsupported = total.saturating_sub(convertible + same_fmt);

        let desc = if convertible == 0 {
            format!("Convert to {t_name}")
        } else if total == 1 && convertible == 1 {
            format!("Convert {} to {t_name}", ctx.items[0].name)
        } else if unsupported == 0 && same_fmt == 0 {
            format!("Convert {total} images to {t_name}")
        } else if unsupported == 0 {
            format!(
                "Convert {convertible} of {total} images to {t_name} \u{00b7} {same_fmt} already {t_name}"
            )
        } else if same_fmt == 0 {
            format!(
                "Convert {convertible} of {total} files to {t_name} \u{00b7} {unsupported} not supported"
            )
        } else {
            format!(
                "Convert {convertible} of {total} files to {t_name} \u{00b7} {same_fmt} already {t_name} \u{00b7} {unsupported} not supported"
            )
        };

        Some(Cow::Owned(desc))
    }

    /// Progressive, zero-I/O enablement evaluation using [`Context::facts`] (`AC-C3.2`).
    fn enablement(&self, ctx: &Context) -> Enablement {
        let catalog = self.menu_ctx.catalog();
        let inputs = catalog.inputs();

        // Check if all extension-eligible routes are blocked by a missing backend
        let mut any_available_route = false;
        let mut missing_backend_reason: Option<String> = None;
        for item in &ctx.basic.items {
            if item.local_path.is_none() {
                continue;
            }
            if let Some(src_fmt) = item.ext.as_deref().and_then(format_from_extension)
                && inputs.contains(&src_fmt)
                && src_fmt != self.target
            {
                match catalog.target_status(src_fmt, self.target) {
                    TargetStatus::Available { .. } => {
                        any_available_route = true;
                    }
                    TargetStatus::BackendMissing {
                        reason,
                        install_hint,
                        ..
                    } => {
                        if missing_backend_reason.is_none() {
                            missing_backend_reason = Some(match install_hint {
                                Some(hint) => format!("{reason} ({hint})"),
                                None => reason,
                            });
                        }
                    }
                    TargetStatus::Unsupported => {}
                }
            }
        }
        if !any_available_route && let Some(reason) = missing_backend_reason {
            return Enablement::Disabled {
                reason: Cow::Owned(reason),
            };
        }

        let Some(facts) = ctx.facts.as_ref() else {
            return Enablement::Pending;
        };

        let mut confirmed_convertible = 0usize;
        let mut pending = 0usize;
        let mut uninspected = 0usize;
        let mut already_target = 0usize;
        let mut not_convertible = 0usize;
        let mut no_permission = 0usize;
        let mut folders = 0usize;
        let mut broken_links = 0usize;
        let mut missing_files = 0usize;
        let mut other_errors = 0usize;
        let mut candidate_count = 0usize;

        for (item, fact) in ctx.basic.items.iter().zip(facts.iter()) {
            if item.local_path.is_none() {
                continue;
            }
            let Some(ext_fmt) = item.ext.as_deref().and_then(format_from_extension) else {
                continue;
            };
            if !inputs.contains(&ext_fmt) {
                continue;
            }
            candidate_count += 1;

            if fact.is_pending() {
                pending += 1;
            } else if fact.is_uninspected() {
                uninspected += 1;
            } else if fact.kind == FileKind::Regular && fact.readable {
                match fact.sniffed_mime.and_then(format_from_mime) {
                    Some(sniffed_fmt) if inputs.contains(&sniffed_fmt) => {
                        if sniffed_fmt == self.target {
                            already_target += 1;
                        } else if matches!(
                            catalog.target_status(sniffed_fmt, self.target),
                            TargetStatus::Available { .. }
                        ) {
                            confirmed_convertible += 1;
                        } else {
                            not_convertible += 1;
                        }
                    }
                    _ => {
                        not_convertible += 1;
                    }
                }
            } else if fact.kind == FileKind::Directory {
                folders += 1;
            } else if fact.error.as_deref() == Some("No permission to read") {
                no_permission += 1;
            } else if fact.kind == FileKind::BrokenSymlink {
                broken_links += 1;
            } else if fact.kind == FileKind::Missing {
                missing_files += 1;
            } else {
                other_errors += 1;
            }
        }

        if candidate_count == 0 {
            return Enablement::Disabled {
                reason: Cow::Borrowed("No convertible images"),
            };
        }

        // Progressive enablement rule (CONVERSION_ARCHITECTURE.md §7.3, AC-C3.2):
        // 1. Enable as soon as >= 1 item is confirmed convertible (even if others are still Pending).
        if confirmed_convertible > 0 {
            return Enablement::Enabled;
        }
        // 2. While any candidate item is still pending inspection, remain Pending ("Checking…").
        if pending > 0 {
            return Enablement::Pending;
        }
        // 3. If uninspected items remain (>64 cap or enrichment timeout), enable optimistically.
        if uninspected > 0 {
            return Enablement::Enabled;
        }

        // 4. All candidate items were inspected and 0 are convertible — report specific reason:
        let non_target_failures =
            not_convertible + no_permission + folders + broken_links + missing_files + other_errors;
        if already_target > 0 && non_target_failures == 0 {
            return Enablement::Disabled {
                reason: Cow::Owned(format!("Already {}", format(self.target).display_name)),
            };
        }
        if no_permission > 0
            && already_target == 0
            && not_convertible == 0
            && folders == 0
            && broken_links == 0
            && missing_files == 0
            && other_errors == 0
        {
            return Enablement::Disabled {
                reason: Cow::Borrowed("No permission to read"),
            };
        }
        if folders > 0
            && already_target == 0
            && not_convertible == 0
            && no_permission == 0
            && broken_links == 0
            && missing_files == 0
            && other_errors == 0
        {
            return Enablement::Disabled {
                reason: Cow::Borrowed("Folders can't be converted"),
            };
        }
        if broken_links > 0
            && already_target == 0
            && not_convertible == 0
            && no_permission == 0
            && folders == 0
            && missing_files == 0
            && other_errors == 0
        {
            return Enablement::Disabled {
                reason: Cow::Borrowed("Broken link"),
            };
        }
        if missing_files > 0
            && already_target == 0
            && not_convertible == 0
            && no_permission == 0
            && folders == 0
            && broken_links == 0
            && other_errors == 0
        {
            return Enablement::Disabled {
                reason: Cow::Borrowed("File not found"),
            };
        }

        Enablement::Disabled {
            reason: Cow::Borrowed("No convertible images"),
        }
    }

    /// Submits a durable `convert.image` job to `radivine-worker` and returns immediately (`AC-C3.6`).
    fn run(&self, ctx: &Context, job: &JobHandle) -> Result<Outcome, ActionError> {
        if job.is_cancelled() {
            return Err(ActionError::Cancelled);
        }

        let mut wire_items = Vec::with_capacity(ctx.basic.items.len());
        for item in &ctx.basic.items {
            let Some(ref path) = item.local_path else {
                continue;
            };
            let wp = WirePath::from_absolute_path(path)
                .map_err(|e| ActionError::Other(format!("Invalid source path: {e}")))?;
            wire_items.push(wp);
        }

        if wire_items.is_empty() {
            return Err(ActionError::NotApplicable(
                "No local files to convert".to_string(),
            ));
        }

        let runtime_paths = self
            .menu_ctx
            .runtime_paths()
            .unwrap_or_else(WorkerRuntimePaths::from_env);

        let mut client = JobsClient::connect_or_launch(&runtime_paths).map_err(|e| {
            ActionError::Other(format!("Couldn't start the conversion service: {e}"))
        })?;

        #[derive(serde::Serialize)]
        struct QuickConvertOptions<'a> {
            target: &'a str,
        }

        let options = QuickConvertOptions {
            target: self.target.as_str(),
        };

        let (job_id, state, deduplicated) = client
            .submit_typed(
                RequestId::generate(),
                Origin::Wheel,
                CONVERT_IMAGE_KIND,
                &wire_items,
                &options,
            )
            .map_err(|e| ActionError::Other(format!("Couldn't submit conversion job: {e}")))?;

        tracing::info!(
            job_id = job_id.0,
            ?state,
            deduplicated,
            target = self.target.as_str(),
            "submitted Quick Convert job to radivine-worker"
        );

        Ok(Outcome::Done)
    }
}

/// Registers the Quick Convert actions (`radivine.convert.image.{png,jpeg,webp}`) into `registry`.
pub fn register_convert_actions(
    registry: &mut radivine_core::ActionRegistry,
    convert_ctx: &ConvertMenuContext,
) -> Result<(), RegistryError> {
    for &target in FormatId::ALL {
        registry.register(Arc::new(ConvertAction::new(target, convert_ctx.clone())))?;
    }
    Ok(())
}
