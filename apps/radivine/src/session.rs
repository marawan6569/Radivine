//! Desktop session state machine (`Idle`, `Armed`, `Summoning`, `Active`, `LeftGrace`,
//! `Committing`, `Dismissing`) (`ARCHITECTURE.md` §3.3, §5, `PLATFORM_INTEGRATION.md` §3.4–§3.12).

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use smallvec::SmallVec;

use radivine_actions::{
    ConvertMenuContext, call_desktop_notify, load_actions_file, registry_from_options,
};
use radivine_core::{
    ActionId, ActionIndex, ActionRegistry, BasicContext, ClipOp, ClipboardRequest, Context,
    Dispatcher, Effect, Enablement, EnrichEvent, Enricher, FileFacts, FileKind, InvocationMode,
    JobEvent, MenuLimits, Outcome, SourceInfo as CoreSourceInfo, TextEncoding, TextVerdict,
};
use radivine_platform::{
    ClipOp as PlatClipOp, ClipboardContent, FrameRef as PlatFrameRef, Key as PlatKey, OverlayInfo,
    PhysPoint, Platform, PlatformEvent, RectPx as PlatRectPx, ShapePx,
};
use radivine_render::{RectPx as RenderRectPx, Renderer, SizePx, Theme};
use radivine_wheel::{
    DismissReason, DropTarget, InputMode, ItemPath, LayoutFallback, LayoutSelection, MenuTree,
    Millis, NavKey, Point, Shape, Viewport, WheelConfig, WheelEvent, WheelInput, WheelLayout,
    WheelSession, plan_concentric,
};

use crate::config::AppConfig;
use crate::ipc::{DaemonStatus, ItemStateSnapshot};

/// Maximum duration an active Invoke session may remain open without interaction (`60 s`).
const ACTIVE_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum duration any session may remain open (`120 s`).
const MAX_SESSION_DURATION: Duration = Duration::from_secs(120);

/// Idle delay before releasing shared-memory pixel buffers (`30 s`).
const IDLE_BUFFER_RELEASE_DELAY: Duration = Duration::from_secs(30);

/// Brief non-blocking window to collect fast local enrichment results on summon (`8 ms` max; returns immediately on completion in ~0.2 ms).
const FAST_LOCAL_ENRICH_WINDOW: Duration = Duration::from_millis(8);

/// Brief non-blocking window to collect fast synchronous job effects (e.g. `SetClipboard`) on commit (`20 ms`).
const FAST_COMMIT_EFFECT_WINDOW: Duration = Duration::from_millis(20);

/// Asynchronous event delivered from background [`Enricher`] or [`Dispatcher`] threads.
enum AsyncControllerEvent {
    Enrich(EnrichEvent),
    Job(JobEvent),
}

type WakeupCallback = Arc<Mutex<Option<Box<dyn Fn() + Send + Sync>>>>;

/// Session lifecycle state (`ARCHITECTURE.md` §3.3, §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// Waiting for an Invoke command or foreign drag.
    Idle,
    /// Foreign client owns `XdndSelection` with mouse button held; waiting for fresh trigger key.
    Armed,
    /// Preparing and mapping the overlay window; prefetching drag payload.
    Summoning,
    /// Overlay is mapped and interactive.
    Active,
    /// Pointer left the shaped wheel cluster during a drag (`passthrough = true`); grace timer running.
    LeftGrace,
    /// Completing drop / unmapping overlay and dispatching the committed leaf action.
    Committing,
    /// Unmapping overlay after cancellation.
    Dismissing,
}

impl SessionState {
    /// Returns the canonical state name for `radivine status`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Armed => "Armed",
            Self::Summoning => "Summoning",
            Self::Active => "Active",
            Self::LeftGrace => "LeftGrace",
            Self::Committing => "Committing",
            Self::Dismissing => "Dismissing",
        }
    }
}

/// Record of the most recently committed action.
#[derive(Clone, Debug, Default)]
pub struct LastCommitRecord {
    /// Committed `ActionId` string.
    pub action_id: Option<String>,
    /// Committed `ItemPath` indices.
    pub path: Option<Vec<u8>>,
    /// Context item URIs at commit time.
    pub items: Vec<String>,
    /// Drag source application name (`Some("nemo")`), if any.
    pub source_app: Option<String>,
    /// Invocation mode (`Some("Drag")` or `Some("Invoke")`).
    pub mode: Option<String>,
}

/// Immutable per-session configuration, layout, theme, and [`ActionRegistry`] snapshot
/// captured at wheel activation (`ARCHITECTURE.md` §A6).
///
/// Ensures live configuration or `actions.toml` reloads affect only future sessions and
/// never mutate, hide, remap, or re-index an active wheel session.
#[derive(Clone)]
pub struct ActiveSessionSettings {
    /// Cloned [`AppConfig`] at session start.
    pub config: AppConfig,
    /// Session [`WheelConfig`] derived from `config` and `reduced_motion`.
    pub wheel_cfg: WheelConfig,
    /// Requested and effective [`LayoutSelection`] for this session.
    pub layout_selection: LayoutSelection,
    /// Effective UI scale factor for this session.
    pub scale: f32,
    /// Resolved [`Theme`] for this session.
    pub theme: Theme,
    /// Effective reduced-motion preference for this session.
    pub reduced_motion: bool,
    /// Input passthrough policy (`input.passthrough`) captured at session start.
    pub passthrough: bool,
    /// Release-outside-cancels policy (`input.release_outside_cancels`) captured at session start.
    pub release_outside_cancels: bool,
    /// [`MenuLimits`] derived from `wheel_cfg` (`max_items_per_wheel`, `max_depth`).
    pub menu_limits: MenuLimits,
    /// Cloned [`ActionRegistry`] snapshot at session start.
    pub registry: Arc<ActionRegistry>,
}

/// Captured drop selection awaiting deferred `text/uri-list` payload resolution (`ARCHITECTURE.md` §A6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDropSelection {
    /// Menu path committed at drop time.
    pub path: ItemPath,
    /// Captured [`ActionId`] from the provisional [`ActionIndex`] at drop time.
    pub action_id: Option<ActionId>,
}

/// Orchestrates the platform backend, radial wheel engine, software renderer, context enricher, and action dispatcher.
pub struct SessionController {
    platform: Box<dyn Platform>,
    renderer: Renderer,
    wheel_cfg: WheelConfig,
    wheel_session: Option<WheelSession>,
    active_session_settings: Option<ActiveSessionSettings>,
    active_menu_tree: Option<MenuTree>,
    action_index: ActionIndex,
    registry: Arc<ActionRegistry>,
    convert_ctx: ConvertMenuContext,
    enricher: Enricher,
    enrich_session_seq: u64,
    active_enrich_session_id: u64,
    dispatcher: Dispatcher,
    async_rx: Receiver<AsyncControllerEvent>,
    wakeup: WakeupCallback,
    notify_tx: Sender<(String, String)>,
    config: AppConfig,
    state: SessionState,
    active_mode: Option<InvocationMode>,
    active_context: Option<Arc<Context>>,
    context_is_provisional: bool,
    overlay_info: Option<OverlayInfo>,
    effective_scale: f32,
    invoke_count: u64,
    drag_session_count: u64,
    last_commit: Arc<Mutex<LastCommitRecord>>,
    last_job_action: Option<String>,
    last_job_error: Option<String>,
    last_notification: Option<String>,
    notifications: Vec<String>,
    config_warnings: Vec<String>,
    active_item_states: Vec<ItemStateSnapshot>,
    daemon_started_at: Instant,
    custom_action_count: usize,
    session_epoch: Instant,
    last_interaction: Instant,
    next_frame_deadline: Option<Instant>,
    idle_release_deadline: Option<Instant>,
    leave_grace_deadline: Option<Instant>,
    drag_armed: bool,
    drag_source: CoreSourceInfo,
    dnd_entered: bool,
    offered_uri_list: bool,
    payload_in_flight: bool,
    retried_payload_after_position: bool,
    last_dnd_pos: Option<PhysPoint>,
    last_dnd_accept: Option<bool>,
    pending_drop_commit: Option<PendingDropSelection>,
    reject_until_drag_end: bool,
    config_reloaded_during_session: bool,
    last_layout_selection: Option<LayoutSelection>,
    bounding_region_clipped: bool,
    debug_overlay: bool,
    trace_timings: bool,
    last_summon_us: Option<u64>,
    last_dnd_status_us: Option<u64>,
    last_drop_dispatch_us: Option<u64>,
    last_frame_render_us: Option<u64>,
    summon_samples_us: Vec<u64>,
    dnd_status_samples_us: Vec<u64>,
    drop_dispatch_samples_us: Vec<u64>,
    frame_render_samples_us: Vec<u64>,
}

impl SessionController {
    /// Creates a new [`SessionController`] in the `Idle` state.
    pub fn new(
        platform: Box<dyn Platform>,
        renderer: Renderer,
        registry: Arc<ActionRegistry>,
        config: AppConfig,
    ) -> Self {
        let debug_overlay = std::env::var("RADIVINE_DEBUG_OVERLAY")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        let trace_timings = std::env::var("RADIVINE_TRACE_TIMINGS")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

        let reduced_motion = config.effective_reduced_motion(platform.reduced_motion());
        let wheel_cfg = config.to_wheel_config(reduced_motion);
        let effective_scale = config.effective_scale(platform.scale());

        let (async_tx, async_rx) = mpsc::channel::<AsyncControllerEvent>();
        let wakeup: WakeupCallback = Arc::new(Mutex::new(None));

        let enrich_tx = async_tx.clone();
        let enrich_wakeup = Arc::clone(&wakeup);
        let enricher = Enricher::new(Box::new(move |ev: EnrichEvent| {
            let _ = enrich_tx.send(AsyncControllerEvent::Enrich(ev));
            if let Ok(guard) = enrich_wakeup.lock()
                && let Some(ref cb) = *guard
            {
                cb();
            }
        }));

        let job_tx = async_tx;
        let job_wakeup = Arc::clone(&wakeup);
        let dispatcher = Dispatcher::new(
            Box::new(move |ev: JobEvent| {
                match &ev {
                    JobEvent::Finished {
                        job,
                        action,
                        result: Ok(_),
                    } => {
                        tracing::info!(
                            job_id = job.0,
                            action_id = %action,
                            "dispatched action completed"
                        );
                    }
                    JobEvent::Finished {
                        job,
                        action,
                        result: Err(err),
                    } => {
                        tracing::warn!(
                            job_id = job.0,
                            action_id = %action,
                            error = %err,
                            "dispatched action failed"
                        );
                    }
                }
                let _ = job_tx.send(AsyncControllerEvent::Job(ev));
                if let Ok(guard) = job_wakeup.lock()
                    && let Some(ref cb) = *guard
                {
                    cb();
                }
            }),
            4,
        );

        let is_live_platform = platform.name() == "x11";
        let (notify_tx, notify_rx) = mpsc::channel::<(String, String)>();
        let _ = thread::Builder::new()
            .name("radivine-notify".to_string())
            .spawn(move || {
                while let Ok((summary, body)) = notify_rx.recv() {
                    let suppress = !is_live_platform
                        || std::env::var("RADIVINE_SUPPRESS_DESKTOP_NOTIFY")
                            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
                    if !suppress
                        && let Err(err) =
                            call_desktop_notify(None, "Radivine", "radivine", &summary, &body, 4000)
                    {
                        tracing::debug!(error = %err, "desktop notification call skipped/failed");
                    }
                }
            });

        let now = Instant::now();
        Self {
            platform,
            renderer,
            wheel_cfg,
            wheel_session: None,
            active_session_settings: None,
            active_menu_tree: None,
            action_index: ActionIndex::default(),
            registry,
            convert_ctx: ConvertMenuContext::default(),
            enricher,
            enrich_session_seq: 0,
            active_enrich_session_id: 0,
            dispatcher,
            async_rx,
            wakeup,
            notify_tx,
            config,
            state: SessionState::Idle,
            active_mode: None,
            active_context: None,
            context_is_provisional: false,
            overlay_info: None,
            effective_scale,
            invoke_count: 0,
            drag_session_count: 0,
            last_commit: Arc::new(Mutex::new(LastCommitRecord::default())),
            last_job_action: None,
            last_job_error: None,
            last_notification: None,
            notifications: Vec::new(),
            config_warnings: Vec::new(),
            active_item_states: Vec::new(),
            daemon_started_at: now,
            custom_action_count: 0,
            session_epoch: now,
            last_interaction: now,
            next_frame_deadline: None,
            idle_release_deadline: None,
            leave_grace_deadline: None,
            drag_armed: false,
            drag_source: CoreSourceInfo::default(),
            dnd_entered: false,
            offered_uri_list: false,
            payload_in_flight: false,
            retried_payload_after_position: false,
            last_dnd_pos: None,
            last_dnd_accept: None,
            pending_drop_commit: None,
            reject_until_drag_end: false,
            config_reloaded_during_session: false,
            last_layout_selection: None,
            bounding_region_clipped: false,
            debug_overlay,
            trace_timings,
            last_summon_us: None,
            last_dnd_status_us: None,
            last_drop_dispatch_us: None,
            last_frame_render_us: None,
            summon_samples_us: Vec::new(),
            dnd_status_samples_us: Vec::new(),
            drop_dispatch_samples_us: Vec::new(),
            frame_render_samples_us: Vec::new(),
        }
    }

    /// Returns the immutable [`ActiveSessionSettings`] snapshot for the current wheel session, if active.
    #[inline]
    #[must_use]
    pub fn active_session_settings(&self) -> Option<&ActiveSessionSettings> {
        self.active_session_settings.as_ref()
    }

    /// Returns the current [`WheelSession`] instance, if any.
    #[inline]
    #[must_use]
    pub fn wheel_session(&self) -> Option<&WheelSession> {
        self.wheel_session.as_ref()
    }

    /// Returns the pending drop selection awaiting deferred payload resolution, if any.
    #[inline]
    #[must_use]
    pub fn pending_drop_selection(&self) -> Option<&PendingDropSelection> {
        self.pending_drop_commit.as_ref()
    }

    /// Returns `true` if an active Concentric Drag session is currently in the display-change
    /// `reject_until_drag_end` safety guard (`ARCHITECTURE.md` §A7).
    #[inline]
    #[must_use]
    pub fn is_rejecting_until_drag_end(&self) -> bool {
        self.reject_until_drag_end
    }

    fn is_effective_concentric_session(&self) -> bool {
        self.active_session_settings
            .as_ref()
            .is_some_and(|s| s.layout_selection.effective == WheelLayout::Concentric)
    }

    fn session_config(&self) -> &AppConfig {
        self.active_session_settings
            .as_ref()
            .map_or(&self.config, |s| &s.config)
    }

    fn session_registry(&self) -> &Arc<ActionRegistry> {
        self.active_session_settings
            .as_ref()
            .map_or(&self.registry, |s| &s.registry)
    }

    fn session_scale(&self) -> f32 {
        self.active_session_settings
            .as_ref()
            .map_or(self.effective_scale, |s| s.scale)
    }

    fn session_passthrough(&self) -> bool {
        self.active_session_settings
            .as_ref()
            .map_or(self.config.input.passthrough, |s| s.passthrough)
    }

    fn session_menu_limits(&self) -> MenuLimits {
        self.active_session_settings.as_ref().map_or_else(
            || menu_limits_from_wheel_config(&self.wheel_cfg),
            |s| s.menu_limits,
        )
    }

    fn clear_active_session_snapshots(&mut self) {
        self.active_session_settings = None;
        self.active_menu_tree = None;
        self.reject_until_drag_end = false;
        self.config_reloaded_during_session = false;
        self.renderer.set_theme(self.config.resolve_theme());
        self.effective_scale = self.config.effective_scale(self.platform.scale());
    }

    /// Emits a startup or diagnostic notification through the daemon notification channel.
    pub fn notify_startup(&mut self, summary: &str, body: &str) {
        self.emit_notification(summary, body);
    }

    /// Logs p50/p95/max latency summaries if `RADIVINE_TRACE_TIMINGS=1` was set (`TESTING_STRATEGY.md` §7).
    pub fn log_timing_summary_if_enabled(&self) {
        if !self.trace_timings {
            return;
        }
        let summarize = |name: &str, samples: &[u64]| {
            if samples.is_empty() {
                tracing::info!(metric = name, samples = 0, "timing summary (no samples)");
                return;
            }
            let mut sorted = samples.to_vec();
            sorted.sort_unstable();
            let n = sorted.len();
            let p50 = sorted[n / 2];
            let p95 = sorted[((n * 95) / 100).min(n - 1)];
            let max = sorted[n - 1];
            tracing::info!(
                metric = name,
                samples = n,
                p50_us = p50,
                p95_us = p95,
                max_us = max,
                "RADIVINE_TRACE_TIMINGS summary"
            );
        };
        summarize("trigger_to_present_us", &self.summon_samples_us);
        summarize("dnd_position_to_status_us", &self.dnd_status_samples_us);
        summarize("drop_to_dispatch_us", &self.drop_dispatch_samples_us);
        summarize("frame_render_present_us", &self.frame_render_samples_us);
    }

    /// Registers a thread-safe wakeup callback invoked whenever background enrichment or job events arrive.
    pub fn set_wakeup_callback(&mut self, callback: Box<dyn Fn() + Send + Sync>) {
        if let Ok(mut guard) = self.wakeup.lock() {
            *guard = Some(callback);
        }
    }

    /// Returns a mutable reference to the underlying [`Platform`].
    pub fn platform_mut(&mut self) -> &mut dyn Platform {
        &mut *self.platform
    }

    /// Returns a reference to the underlying [`Platform`].
    #[must_use]
    pub fn platform(&self) -> &dyn Platform {
        &*self.platform
    }

    /// Reloads [`AppConfig`] and applies updated wheel, theme, and scale parameters.
    ///
    /// If a radial wheel session is currently active, `self.config` and `self.wheel_cfg` are
    /// updated for subsequent sessions while preserving the active session's scale and theme so an
    /// in-progress drag-and-drop or invoke interaction is never interrupted.
    pub fn reload_config(&mut self, config: AppConfig) {
        if self.active_session_settings.is_some() {
            self.config_reloaded_during_session = true;
        }
        self.config = config;
        let reduced_motion = self
            .config
            .effective_reduced_motion(self.platform.reduced_motion());
        self.wheel_cfg = self.config.to_wheel_config(reduced_motion);
        if matches!(self.state, SessionState::Idle | SessionState::Armed) {
            self.renderer.set_theme(self.config.resolve_theme());
            self.effective_scale = self.config.effective_scale(self.platform.scale());
        }
        tracing::info!(
            scale = self.effective_scale,
            passthrough = self.config.input.passthrough,
            "reloaded configuration"
        );
    }

    /// Sets the shared [`ConvertMenuContext`] used when reloading `actions.toml`.
    pub fn set_convert_menu_context(&mut self, convert_ctx: ConvertMenuContext) {
        self.convert_ctx = convert_ctx;
    }

    /// Loads or reloads `actions.toml` from `actions_path`.
    ///
    /// If `actions_path` is invalid or fails validation, logs and notifies the error with line
    /// information while preserving the previously active [`ActionRegistry`] (`AC-4.6`).
    pub fn load_actions_from_path(&mut self, actions_path: &Path) {
        let preview_settings = self.config.to_preview_settings();
        let notify_warn = self.config.notifications.notify_on_config_warning;
        match load_actions_file(actions_path) {
            Ok(actions_cfg) => {
                match registry_from_options(&actions_cfg, &preview_settings, &self.convert_ctx) {
                    Ok(new_reg) => {
                        self.registry = Arc::new(new_reg);
                        self.custom_action_count = actions_cfg
                            .actions
                            .iter()
                            .filter(|a| a.is_enabled())
                            .count();
                        self.config_warnings.clone_from(&actions_cfg.warnings);
                        for warning in &actions_cfg.warnings {
                            tracing::warn!("{warning}");
                            if notify_warn {
                                self.emit_notification("Actions configuration warning", warning);
                            } else {
                                self.record_notification_only(
                                    "Actions configuration warning",
                                    warning,
                                );
                            }
                        }
                        tracing::info!(
                            path = %actions_path.display(),
                            custom_actions = actions_cfg.actions.len(),
                            warnings = actions_cfg.warnings.len(),
                            "loaded actions.toml"
                        );
                    }
                    Err(err) => {
                        let msg = format!("{}: {err}", actions_path.display());
                        tracing::warn!("{msg}");
                        self.config_warnings = vec![msg.clone()];
                        if notify_warn {
                            self.emit_notification("Invalid actions.toml", &msg);
                        } else {
                            self.record_notification_only("Invalid actions.toml", &msg);
                        }
                    }
                }
            }
            Err(err) => {
                let msg = err.to_string();
                tracing::warn!("{msg}");
                self.config_warnings = vec![msg.clone()];
                if notify_warn {
                    self.emit_notification("Invalid actions.toml", &msg);
                } else {
                    self.record_notification_only("Invalid actions.toml", &msg);
                }
            }
        }
    }

    /// Reloads both `config.toml` and `actions.toml` (`AC-4.6`).
    pub fn reload_all(&mut self, config: AppConfig, actions_path: &Path) {
        self.reload_config(config);
        self.load_actions_from_path(actions_path);
    }

    /// Builds a [`DaemonStatus`] snapshot for `radivine status`.
    pub fn status(&mut self) -> DaemonStatus {
        self.drain_async_events(Instant::now());
        self.platform.sync_graphical_state();
        if matches!(self.state, SessionState::Idle | SessionState::Armed) {
            self.effective_scale = self.config.effective_scale(self.platform.scale());
        }
        let caps = self.platform.capabilities();
        let graphical_ready = !matches!(caps.invoke_at_pointer, radivine_platform::Support::No(_));
        let compositor_active = matches!(caps.translucency, radivine_platform::Support::Yes);
        let translucent_overlay = if matches!(self.state, SessionState::Idle | SessionState::Armed)
        {
            compositor_active
        } else {
            self.overlay_info
                .as_ref()
                .map_or(compositor_active, |info| info.translucent)
        };
        let monitor_count = self.platform.monitors().len();
        let monitor_rect = self
            .overlay_info
            .as_ref()
            .map(|info| {
                (
                    info.monitor.rect.x,
                    info.monitor.rect.y,
                    info.monitor.rect.w,
                    info.monitor.rect.h,
                )
            })
            .or_else(|| {
                self.platform
                    .monitors()
                    .first()
                    .map(|m| (m.rect.x, m.rect.y, m.rect.w, m.rect.h))
            })
            .unwrap_or((0, 0, 0, 0));

        let active_scale = self.session_scale();
        let (root_wheel_center_px, root_wheel_outer_radius_px) = self
            .wheel_session
            .as_ref()
            .and_then(|ws| ws.scene().wheels.first())
            .map_or((None, None), |rw| {
                (
                    Some((rw.center.x * active_scale, rw.center.y * active_scale)),
                    Some(rw.r_out * active_scale),
                )
            });

        let commit = self
            .last_commit
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();

        let requested_layout = self.wheel_cfg.layout.as_str().to_string();
        let (active_layout, layout_fallback_reason, layout_pending) = if let Some(ref settings) =
            self.active_session_settings
        {
            let pending = self.config_reloaded_during_session
                || settings.wheel_cfg.layout != self.wheel_cfg.layout;
            (
                settings.layout_selection.effective.as_str().to_string(),
                settings
                    .layout_selection
                    .fallback
                    .map(|f| f.as_str().to_string()),
                pending,
            )
        } else {
            let sel =
                self.evaluate_idle_layout_selection(compositor_active, monitor_rect, active_scale);
            (
                sel.effective.as_str().to_string(),
                sel.fallback.map(|f| f.as_str().to_string()),
                false,
            )
        };

        DaemonStatus {
            pid: std::process::id(),
            overlay_window_id: self.platform.overlay_window_id(),
            session_state: self.state.as_str().to_string(),
            scale: active_scale,
            monitor_rect,
            root_wheel_center_px,
            root_wheel_outer_radius_px,
            invoke_count: self.invoke_count,
            drag_session_count: self.drag_session_count,
            last_committed_action: commit.action_id,
            last_committed_path: commit.path,
            last_committed_items: commit.items,
            last_committed_source_app: commit.source_app,
            last_committed_mode: commit.mode,
            last_job_action: self.last_job_action.clone(),
            last_job_error: self.last_job_error.clone(),
            last_notification: self.last_notification.clone(),
            notifications: self.notifications.clone(),
            config_warnings: self.config_warnings.clone(),
            active_item_states: self.active_item_states.clone(),
            last_summon_us: self.last_summon_us,
            last_dnd_status_us: self.last_dnd_status_us,
            last_drop_dispatch_us: self.last_drop_dispatch_us,
            last_frame_render_us: self.last_frame_render_us,
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_secs: self.daemon_started_at.elapsed().as_secs(),
            desktop_session: std::env::var("XDG_SESSION_TYPE")
                .unwrap_or_else(|_| "unknown".to_string()),
            desktop_environment: std::env::var("XDG_CURRENT_DESKTOP")
                .or_else(|_| std::env::var("DESKTOP_SESSION"))
                .unwrap_or_else(|_| "unknown".to_string()),
            display: std::env::var("DISPLAY").unwrap_or_else(|_| "(unset)".to_string()),
            custom_action_count: self.custom_action_count,
            rss_kib: query_self_rss_kib(),
            graphical_ready,
            compositor_active,
            translucent_overlay,
            monitor_count,
            requested_layout,
            active_layout,
            layout_fallback_reason,
            layout_pending,
        }
    }

    fn evaluate_idle_layout_selection(
        &self,
        compositor_active: bool,
        monitor_rect: (i32, i32, u32, u32),
        scale: f32,
    ) -> LayoutSelection {
        if self.wheel_cfg.layout == WheelLayout::Branching {
            return LayoutSelection::branching();
        }
        if !compositor_active {
            return LayoutSelection::fallback_to_branching(LayoutFallback::NoCompositor);
        }
        if monitor_rect.2 > 0 && monitor_rect.3 > 0 {
            let s = scale.max(0.5);
            let vp = Viewport::new((monitor_rect.2 as f32) / s, (monitor_rect.3 as f32) / s, s);
            let anchor = Point::new(vp.bounds.w * 0.5, vp.bounds.h * 0.5);
            let raw_cfg = self.config.to_wheel_config(false);
            return match plan_concentric(anchor, vp, &raw_cfg) {
                Ok(_) => LayoutSelection::concentric(),
                Err(reason) => LayoutSelection::fallback_to_branching(reason),
            };
        }
        self.last_layout_selection
            .unwrap_or_else(LayoutSelection::concentric)
    }

    /// Summons the radial wheel in Invoke mode at `explicit_pos` (or the current X11 pointer).
    pub fn summon_invoke(
        &mut self,
        uris: Vec<String>,
        explicit_pos: Option<PhysPoint>,
    ) -> anyhow::Result<()> {
        let t0 = Instant::now();
        self.drain_async_events(t0);
        if !matches!(self.state, SessionState::Idle | SessionState::Armed) {
            self.dismiss_immediate();
        }

        self.state = SessionState::Summoning;
        self.active_mode = Some(InvocationMode::Invoke);
        self.context_is_provisional = false;
        self.idle_release_deadline = None;
        self.leave_grace_deadline = None;
        self.pending_drop_commit = None;
        self.reject_until_drag_end = false;
        self.config_reloaded_during_session = false;
        self.session_epoch = t0;
        self.last_interaction = t0;

        let pointer_phys = match explicit_pos {
            Some(pt) => pt,
            None => self.platform.pointer_position()?,
        };

        let info = self.platform.prepare_overlay(pointer_phys)?;
        let config_snapshot = self.config.clone();
        let scale = config_snapshot.effective_scale(info.scale);
        self.effective_scale = scale;
        let reduced_motion =
            config_snapshot.effective_reduced_motion(self.platform.reduced_motion());
        let wheel_cfg = config_snapshot.to_wheel_config(reduced_motion);
        self.wheel_cfg = wheel_cfg.clone();
        let theme = config_snapshot.resolve_theme();
        self.renderer.set_theme(theme.clone());
        let menu_limits = menu_limits_from_wheel_config(&wheel_cfg);
        let registry_snapshot = Arc::clone(&self.registry);
        let mut session_settings = ActiveSessionSettings {
            passthrough: config_snapshot.input.passthrough,
            release_outside_cancels: config_snapshot.input.release_outside_cancels,
            config: config_snapshot,
            wheel_cfg,
            layout_selection: LayoutSelection::branching(),
            scale,
            theme,
            reduced_motion,
            menu_limits,
            registry: registry_snapshot,
        };
        self.active_session_settings = Some(session_settings.clone());

        let local_phys_x = (pointer_phys.x - info.origin.x) as f32;
        let local_phys_y = (pointer_phys.y - info.origin.y) as f32;
        let cursor_logical = Point::new(local_phys_x / scale, local_phys_y / scale);
        let viewport = Viewport::new(
            (info.size.0 as f32) / scale,
            (info.size.1 as f32) / scale,
            scale,
        );

        let basic =
            BasicContext::from_uris(&uris, InvocationMode::Invoke, CoreSourceInfo::default());
        let item_count = basic.items.len() as u32;
        let initial_ctx = Arc::new(Context::from_basic(basic.clone()));

        self.enrich_session_seq = self.enrich_session_seq.wrapping_add(1);
        let enrich_sid = self.enrich_session_seq;
        self.active_enrich_session_id = enrich_sid;
        self.enricher.start(enrich_sid, basic);

        // Collect fast local enrichment results (< 3 ms) before building the menu so normal local
        // files start with their final enablement while slow/hung files map immediately with "Checking…".
        let ctx = self.wait_for_initial_enrichment_context(
            enrich_sid,
            initial_ctx,
            FAST_LOCAL_ENRICH_WINDOW,
        );

        let (menu_tree, action_index) = session_settings
            .registry
            .build_menu(&ctx, session_settings.menu_limits)
            .ok_or_else(|| anyhow::anyhow!("no actions available for context"))?;

        let mut ws = WheelSession::new(
            Some(menu_tree.clone()),
            cursor_logical,
            viewport,
            InputMode::Pointer,
            &session_settings.wheel_cfg,
            0,
        );
        if session_settings.wheel_cfg.layout == WheelLayout::Concentric && !info.translucent {
            ws.record_layout_fallback(LayoutFallback::NoCompositor);
        }
        if item_count > 1 {
            ws.set_root_badge(Some(item_count), 0);
        }
        session_settings.layout_selection = ws.layout_selection();
        self.last_layout_selection = Some(session_settings.layout_selection);
        self.active_session_settings = Some(session_settings);

        self.renderer.set_opaque_fallback(!info.translucent);
        self.renderer
            .begin_session(SizePx::new(info.size.0, info.size.1), scale);
        self.overlay_info = Some(info.clone());
        self.active_context = Some(ctx);
        self.active_menu_tree = Some(menu_tree);
        self.action_index = action_index;
        self.wheel_session = Some(ws);
        self.refresh_active_item_states();

        self.sync_input_region()?;
        self.render_and_present(true)?;
        self.platform.map_overlay()?;

        if let Err(err) = self.platform.grab_input() {
            tracing::warn!(
                "failed to grab input in Invoke mode ({err}); continuing ungrabbed per PLATFORM_INTEGRATION.md §3.9"
            );
        }

        self.state = SessionState::Active;
        self.invoke_count += 1;
        self.schedule_next_frame_if_animating(Instant::now());

        let elapsed_us = t0.elapsed().as_micros() as u64;
        self.last_summon_us = Some(elapsed_us);
        if self.trace_timings && self.summon_samples_us.len() < 16_384 {
            self.summon_samples_us.push(elapsed_us);
        }
        tracing::info!(
            pointer_x = pointer_phys.x,
            pointer_y = pointer_phys.y,
            scale = scale,
            monitor = %info.monitor.name,
            elapsed_us = elapsed_us,
            "summoned radial wheel in Invoke mode"
        );
        Ok(())
    }

    /// Summons the radial wheel in Drag mode at `pointer_phys` (`ARCHITECTURE.md` §5, `AC-3.1`).
    ///
    /// Never calls `grab_input` (`AC-3.10`): the drag source retains its active pointer grab and
    /// streams `XdndPosition` / `XdndDrop` client messages to the mapped Radivine overlay.
    fn summon_drag(&mut self, pointer_phys: PhysPoint) -> anyhow::Result<()> {
        let t0 = Instant::now();
        self.state = SessionState::Summoning;
        self.active_mode = Some(InvocationMode::Drag);
        self.active_context = None;
        self.active_menu_tree = None;
        self.context_is_provisional = false;
        self.idle_release_deadline = None;
        self.leave_grace_deadline = None;
        self.pending_drop_commit = None;
        self.reject_until_drag_end = false;
        self.config_reloaded_during_session = false;
        self.dnd_entered = false;
        self.offered_uri_list = false;
        self.retried_payload_after_position = false;
        self.last_dnd_pos = None;
        self.last_dnd_accept = None;
        self.session_epoch = t0;
        self.last_interaction = t0;

        let info = self.platform.prepare_overlay(pointer_phys)?;
        let config_snapshot = self.config.clone();
        let scale = config_snapshot.effective_scale(info.scale);
        self.effective_scale = scale;
        let reduced_motion =
            config_snapshot.effective_reduced_motion(self.platform.reduced_motion());
        let wheel_cfg = config_snapshot.to_wheel_config(reduced_motion);
        self.wheel_cfg = wheel_cfg.clone();
        let theme = config_snapshot.resolve_theme();
        self.renderer.set_theme(theme.clone());
        let menu_limits = menu_limits_from_wheel_config(&wheel_cfg);
        let registry_snapshot = Arc::clone(&self.registry);
        let mut session_settings = ActiveSessionSettings {
            passthrough: config_snapshot.input.passthrough,
            release_outside_cancels: config_snapshot.input.release_outside_cancels,
            config: config_snapshot,
            wheel_cfg,
            layout_selection: LayoutSelection::branching(),
            scale,
            theme,
            reduced_motion,
            menu_limits,
            registry: registry_snapshot,
        };

        let local_phys_x = (pointer_phys.x - info.origin.x) as f32;
        let local_phys_y = (pointer_phys.y - info.origin.y) as f32;
        let cursor_logical = Point::new(local_phys_x / scale, local_phys_y / scale);
        let viewport = Viewport::new(
            (info.size.0 as f32) / scale,
            (info.size.1 as f32) / scale,
            scale,
        );

        // Start with a hub-only placeholder wheel (`menu = None`) while `request_payload` is in flight;
        // segments populate in ~0.2 ms when `SelectionNotify` arrives (`ARCHITECTURE.md` §5).
        let mut ws = WheelSession::new(
            None,
            cursor_logical,
            viewport,
            InputMode::Drag,
            &session_settings.wheel_cfg,
            0,
        );
        if session_settings.wheel_cfg.layout == WheelLayout::Concentric && !info.translucent {
            ws.record_layout_fallback(LayoutFallback::NoCompositor);
        }
        session_settings.layout_selection = ws.layout_selection();
        self.last_layout_selection = Some(session_settings.layout_selection);
        let passthrough = session_settings.passthrough;
        self.active_session_settings = Some(session_settings);

        self.renderer.set_opaque_fallback(!info.translucent);
        self.renderer
            .begin_session(SizePx::new(info.size.0, info.size.1), scale);
        self.overlay_info = Some(info.clone());
        self.action_index = ActionIndex::default();
        self.wheel_session = Some(ws);

        self.sync_input_region()?;
        self.render_and_present(true)?;
        self.platform.map_overlay()?;

        // Prefetch `text/uri-list` using the XFixes selection timestamp (`S-3`)
        self.payload_in_flight = true;
        self.platform.request_payload("text/uri-list");

        self.drag_session_count += 1;
        self.schedule_next_frame_if_animating(Instant::now());

        let elapsed_us = t0.elapsed().as_micros() as u64;
        self.last_summon_us = Some(elapsed_us);
        if self.trace_timings && self.summon_samples_us.len() < 16_384 {
            self.summon_samples_us.push(elapsed_us);
        }
        tracing::info!(
            target: "radivine::summon",
            pointer_x = pointer_phys.x,
            pointer_y = pointer_phys.y,
            scale = scale,
            monitor = %info.monitor.name,
            source_app = ?self.drag_source.app,
            source_pid = ?self.drag_source.pid,
            passthrough = passthrough,
            elapsed_us = elapsed_us,
            "summoned radial wheel in Drag mode"
        );
        Ok(())
    }

    /// Immediately ungrabs input (if in Invoke mode), unmaps the overlay, and transitions to
    /// `Armed` (if a drag is still active) or `Idle`.
    pub fn dismiss_immediate(&mut self) {
        self.state = SessionState::Dismissing;
        if self.active_mode == Some(InvocationMode::Invoke) {
            self.platform.ungrab_input();
        }
        self.platform.hide_overlay();
        self.next_frame_deadline = None;
        self.leave_grace_deadline = None;
        self.pending_drop_commit = None;
        self.payload_in_flight = false;
        self.dnd_entered = false;
        self.last_dnd_accept = None;
        self.active_mode = None;
        self.clear_active_session_snapshots();
        self.state = if self.drag_armed {
            SessionState::Armed
        } else {
            SessionState::Idle
        };
        if self.state == SessionState::Idle {
            self.idle_release_deadline = Some(Instant::now() + IDLE_BUFFER_RELEASE_DELAY);
        }
    }

    fn dismiss_drag_session(&mut self, reason: DismissReason, now: Instant) {
        let ms = self.elapsed_millis(now);
        if let Some(ref mut ws) = self.wheel_session {
            ws.dismiss(reason, ms);
        }
        tracing::info!(
            reason = ?reason,
            drag_armed = self.drag_armed,
            "dismissed drag radial wheel session"
        );
        self.dismiss_immediate();
    }

    /// Handles a batch of [`PlatformEvent`]s emitted by the display server.
    pub fn handle_platform_events(&mut self, events: Vec<PlatformEvent>) {
        let now = Instant::now();
        self.drain_async_events(now);
        for ev in events {
            match ev {
                PlatformEvent::DragArmed { source } => {
                    self.drag_armed = true;
                    self.drag_source = CoreSourceInfo {
                        app: source.app,
                        pid: source.pid,
                    };
                    self.idle_release_deadline = None;
                    if self.state == SessionState::Idle {
                        self.state = SessionState::Armed;
                    }
                    tracing::info!(
                        source_app = ?self.drag_source.app,
                        source_pid = ?self.drag_source.pid,
                        "drag armed"
                    );
                }
                PlatformEvent::DragDisarmed => {
                    self.drag_armed = false;
                    self.drag_source = CoreSourceInfo::default();
                    tracing::info!(state = %self.state.as_str(), "drag disarmed");
                    if self.state == SessionState::Armed {
                        self.state = SessionState::Idle;
                        self.idle_release_deadline = Some(now + IDLE_BUFFER_RELEASE_DELAY);
                    } else if self.active_mode == Some(InvocationMode::Drag)
                        && self.state != SessionState::Committing
                    {
                        self.dismiss_drag_session(DismissReason::DragEnded, now);
                    }
                }
                PlatformEvent::Trigger { at } => {
                    if self.active_mode == Some(InvocationMode::Invoke) {
                        // Ignore drag triggers while an Invoke session is running (`ARCHITECTURE.md` §5)
                        continue;
                    }
                    if self.state == SessionState::Armed
                        || (self.state == SessionState::Idle && self.drag_armed)
                    {
                        if let Err(err) = self.summon_drag(at) {
                            tracing::warn!("failed to summon drag radial wheel: {err}");
                            self.dismiss_immediate();
                        }
                    } else if self.active_mode == Some(InvocationMode::Drag)
                        && matches!(
                            self.state,
                            SessionState::Summoning
                                | SessionState::Active
                                | SessionState::LeftGrace
                        )
                        && self.session_config().trigger.toggle_off_on_repress
                    {
                        // Second Shift press hides the wheel and lets the Nemo drag continue (`AC-3.8`)
                        self.dismiss_drag_session(DismissReason::Toggled, now);
                    }
                }
                PlatformEvent::TriggerReleased
                    if self.active_mode == Some(InvocationMode::Drag)
                        && matches!(
                            self.state,
                            SessionState::Summoning
                                | SessionState::Active
                                | SessionState::LeftGrace
                        )
                        && self.session_config().trigger.hold_to_keep_open =>
                {
                    self.dismiss_drag_session(DismissReason::Toggled, now);
                }
                PlatformEvent::DndEnter { types }
                    if self.active_mode == Some(InvocationMode::Drag) =>
                {
                    self.last_interaction = now;
                    self.dnd_entered = true;
                    self.offered_uri_list = types.iter().any(|t| t == "text/uri-list");
                    tracing::debug!(
                        offered_types = ?types,
                        offered_uri_list = self.offered_uri_list,
                        state = %self.state.as_str(),
                        "XdndEnter received on overlay"
                    );

                    if !self.offered_uri_list {
                        // Non-file drag (e.g. text selection in an editor): never activate (`AC-3.1`)
                        tracing::info!(
                            offered_types = ?types,
                            "drag source does not offer text/uri-list; dismissing"
                        );
                        self.dismiss_drag_session(DismissReason::Cancelled, now);
                        continue;
                    }

                    if self.reject_until_drag_end {
                        self.last_dnd_accept = Some(false);
                        self.platform.dnd_reply(false);
                        continue;
                    }

                    if self.state == SessionState::LeftGrace {
                        self.leave_grace_deadline = None;
                        self.state = SessionState::Active;
                        self.feed_wheel_input(WheelInput::PointerEntered, now);
                    } else if self.state == SessionState::Summoning
                        && self.active_context.is_none()
                        && !self.payload_in_flight
                    {
                        self.payload_in_flight = true;
                        self.platform.request_payload("text/uri-list");
                    }
                }
                PlatformEvent::DndPosition { at }
                    if self.active_mode == Some(InvocationMode::Drag) =>
                {
                    let pos_t0 = Instant::now();
                    self.last_interaction = now;
                    self.dnd_entered = true;
                    self.last_dnd_pos = Some(at);

                    if self.reject_until_drag_end {
                        self.last_dnd_accept = Some(false);
                        self.platform.dnd_reply(false);
                        let status_us = pos_t0.elapsed().as_micros() as u64;
                        self.last_dnd_status_us = Some(status_us);
                        if self.trace_timings && self.dnd_status_samples_us.len() < 16_384 {
                            self.dnd_status_samples_us.push(status_us);
                        }
                        continue;
                    }

                    match self.state {
                        SessionState::Active => {
                            let pos = self.root_phys_to_logical(at);
                            let ms = self.elapsed_millis(now);
                            let mut wheel_events = Vec::new();
                            let accept = if let Some(ref mut ws) = self.wheel_session {
                                ws.handle(WheelInput::PointerMoved(pos), ms, &mut wheel_events);
                                matches!(ws.drop_target(), DropTarget::Accept(_))
                            } else {
                                false
                            };

                            // Ordering rule (`ARCHITECTURE.md` §4, §5): reply XdndStatus BEFORE rendering!
                            self.last_dnd_accept = Some(accept);
                            self.platform.dnd_reply(accept);
                            let status_us = pos_t0.elapsed().as_micros() as u64;
                            self.last_dnd_status_us = Some(status_us);
                            if self.trace_timings && self.dnd_status_samples_us.len() < 16_384 {
                                self.dnd_status_samples_us.push(status_us);
                            }

                            self.process_wheel_events(wheel_events, now);
                            if self.state == SessionState::Active {
                                let _ = self.render_and_present(false);
                                let _ = self.platform.flush();
                                self.schedule_next_frame_if_animating(now);
                            }
                        }
                        SessionState::Summoning => {
                            // In Summoning, always reply reject (`ARCHITECTURE.md` §5)
                            self.last_dnd_accept = Some(false);
                            self.platform.dnd_reply(false);

                            if self.active_context.is_none()
                                && !self.payload_in_flight
                                && !self.retried_payload_after_position
                            {
                                self.retried_payload_after_position = true;
                                self.payload_in_flight = true;
                                self.platform.request_payload("text/uri-list");
                            }
                        }
                        SessionState::LeftGrace => {
                            // Pointer re-entered the overlay
                            self.leave_grace_deadline = None;
                            self.state = SessionState::Active;
                            let pos = self.root_phys_to_logical(at);
                            let ms = self.elapsed_millis(now);
                            let mut wheel_events = Vec::new();
                            let accept = if let Some(ref mut ws) = self.wheel_session {
                                ws.handle(WheelInput::PointerEntered, ms, &mut wheel_events);
                                ws.handle(WheelInput::PointerMoved(pos), ms, &mut wheel_events);
                                matches!(ws.drop_target(), DropTarget::Accept(_))
                            } else {
                                false
                            };
                            self.last_dnd_accept = Some(accept);
                            self.platform.dnd_reply(accept);
                            self.process_wheel_events(wheel_events, now);
                            if self.state == SessionState::Active {
                                let _ = self.render_and_present(false);
                                let _ = self.platform.flush();
                                self.schedule_next_frame_if_animating(now);
                            }
                        }
                        _ => {
                            self.last_dnd_accept = Some(false);
                            self.platform.dnd_reply(false);
                        }
                    }
                }
                PlatformEvent::DndLeave { drag_ended }
                    if self.active_mode == Some(InvocationMode::Drag) =>
                {
                    self.dnd_entered = false;
                    self.last_dnd_accept = None;
                    if drag_ended {
                        // Released on a reject zone, pressed Escape, or source vanished (`AC-3.8`)
                        self.dismiss_drag_session(DismissReason::DragEnded, now);
                    } else if !self.reject_until_drag_end && self.state == SessionState::Active {
                        // Pointer left the shaped wheel cluster (`passthrough = true`)
                        self.feed_wheel_input(WheelInput::PointerLeft, now);
                        self.state = SessionState::LeftGrace;
                        let leave_grace_ms = self.session_config().wheel.leave_grace_ms;
                        self.leave_grace_deadline =
                            Some(now + Duration::from_millis(leave_grace_ms));
                    }
                }
                PlatformEvent::DndDrop if self.active_mode == Some(InvocationMode::Drag) => {
                    self.handle_dnd_drop(now);
                }
                PlatformEvent::Payload { mime, result }
                    if self.active_mode == Some(InvocationMode::Drag)
                        && mime == "text/uri-list" =>
                {
                    self.payload_in_flight = false;
                    self.handle_uri_list_payload(result, now);
                }
                PlatformEvent::PointerMoved { at }
                    if self.state == SessionState::Active
                        && self.active_mode == Some(InvocationMode::Invoke) =>
                {
                    self.last_interaction = now;
                    let pos = self.root_phys_to_logical(at);
                    self.feed_wheel_input(WheelInput::PointerMoved(pos), now);
                }
                PlatformEvent::Button {
                    button,
                    pressed,
                    at,
                } if self.state == SessionState::Active
                    && self.active_mode == Some(InvocationMode::Invoke) =>
                {
                    self.last_interaction = now;
                    let pos = self.root_phys_to_logical(at);
                    self.feed_wheel_input(WheelInput::PointerMoved(pos), now);
                    if self.state != SessionState::Active {
                        continue;
                    }
                    match (button, pressed) {
                        (1, true) => {
                            self.feed_wheel_input(WheelInput::Press, now);
                        }
                        (1, false) => {
                            self.feed_wheel_input(WheelInput::Release, now);
                        }
                        (3, true) => {
                            tracing::info!("radial wheel session cancelled via secondary click");
                            self.dismiss_immediate();
                        }
                        _ => {}
                    }
                }
                PlatformEvent::Key { key, pressed: true } if self.state == SessionState::Active => {
                    self.last_interaction = now;
                    let nav = translate_platform_key(key);
                    self.feed_wheel_input(WheelInput::Nav(nav), now);
                }
                PlatformEvent::SettingsChanged => {
                    self.platform.sync_graphical_state();
                    let reduced_motion = self
                        .config
                        .effective_reduced_motion(self.platform.reduced_motion());
                    self.wheel_cfg = self.config.to_wheel_config(reduced_motion);
                    if matches!(self.state, SessionState::Idle | SessionState::Armed) {
                        self.effective_scale = self.config.effective_scale(self.platform.scale());
                    } else if matches!(
                        self.state,
                        SessionState::Summoning | SessionState::Active | SessionState::LeftGrace
                    ) {
                        if self.is_effective_concentric_session() {
                            // `ARCHITECTURE.md` §A7: defer Xft DPI / scale / reduced-motion changes
                            // during an active Concentric session, unless compositor translucency was lost.
                            let compositor_active = matches!(
                                self.platform.capabilities().translucency,
                                radivine_platform::Support::Yes
                            );
                            if !compositor_active {
                                self.handle_concentric_display_invalidation(now);
                            }
                        } else {
                            self.refresh_active_branching_overlay();
                        }
                    }
                    tracing::info!(scale = self.session_scale(), "updated display settings");
                }
                PlatformEvent::MonitorsChanged => {
                    self.platform.sync_graphical_state();
                    let reduced_motion = self
                        .config
                        .effective_reduced_motion(self.platform.reduced_motion());
                    self.wheel_cfg = self.config.to_wheel_config(reduced_motion);
                    if matches!(self.state, SessionState::Idle | SessionState::Armed) {
                        self.effective_scale = self.config.effective_scale(self.platform.scale());
                    } else if matches!(
                        self.state,
                        SessionState::Summoning | SessionState::Active | SessionState::LeftGrace
                    ) {
                        if self.is_effective_concentric_session() {
                            self.handle_concentric_display_invalidation(now);
                        } else {
                            self.refresh_active_branching_overlay();
                        }
                    }
                    tracing::info!(scale = self.session_scale(), "updated display monitors");
                }
                PlatformEvent::ConnectionLost => {
                    tracing::error!("X11 display server connection lost");
                    self.drag_armed = false;
                    self.active_mode = None;
                    self.clear_active_session_snapshots();
                    self.state = SessionState::Idle;
                }
                _ => {}
            }
        }
    }

    fn refresh_active_branching_overlay(&mut self) {
        let ref_pt = self
            .last_dnd_pos
            .or_else(|| self.overlay_info.as_ref().map(|i| i.origin))
            .unwrap_or_else(|| PhysPoint::new(0, 0));
        if let Ok(info) = self.platform.prepare_overlay(ref_pt) {
            let scale = self.session_config().effective_scale(info.scale);
            self.effective_scale = scale;
            if let Some(ref mut s) = self.active_session_settings {
                s.scale = scale;
            }
            self.renderer.set_opaque_fallback(!info.translucent);
            self.renderer
                .begin_session(SizePx::new(info.size.0, info.size.1), scale);
            self.overlay_info = Some(info);
            let _ = self.sync_input_region();
            let _ = self.render_and_present(true);
            let _ = self.platform.flush();
        }
    }

    fn handle_concentric_display_invalidation(&mut self, now: Instant) {
        if self.active_mode == Some(InvocationMode::Invoke) {
            tracing::info!(
                "display or compositor changed during Concentric Invoke session; cancelling immediately"
            );
            self.dismiss_immediate();
            return;
        }
        if self.active_mode == Some(InvocationMode::Drag) {
            tracing::info!(
                "display or compositor changed during Concentric Drag session; entering reject_until_drag_end guard"
            );
            self.reject_until_drag_end = true;
            self.leave_grace_deadline = None;
            if self.state == SessionState::LeftGrace {
                self.state = SessionState::Active;
            }
            let ms = self.elapsed_millis(now);
            if let Some(ref mut ws) = self.wheel_session {
                ws.suspend_interaction(ms);
            }
            self.last_dnd_accept = Some(false);
            self.platform.dnd_reply(false);
            let _ = self.render_and_present(false);
            let _ = self.platform.flush();
        }
    }

    fn handle_dnd_drop(&mut self, now: Instant) {
        let drop_t0 = Instant::now();
        self.drain_async_events(now);
        if self.reject_until_drag_end || self.state != SessionState::Active {
            // `ARCHITECTURE.md` §5, §A7: A DndDrop arriving while `reject_until_drag_end` is active or
            // in any state other than Active is answered with dnd_finish(false).
            self.platform.dnd_finish(false);
            self.dismiss_drag_session(DismissReason::Cancelled, now);
            return;
        }

        let ms = self.elapsed_millis(now);
        let committed = self
            .wheel_session
            .as_mut()
            .and_then(|ws| ws.commit_drop(ms));

        let Some(path) = committed else {
            tracing::info!("XdndDrop rejected by drop-time re-check; finishing with success=false");
            self.platform.dnd_finish(false);
            self.dismiss_drag_session(DismissReason::Cancelled, now);
            return;
        };

        if self.active_context.is_some() && !self.context_is_provisional {
            let Some(action_id) = self.action_index.get(&path).cloned() else {
                tracing::info!(
                    path = ?path,
                    "XdndDrop path missing from session ActionIndex; finishing with success=false"
                );
                self.platform.dnd_finish(false);
                self.dismiss_drag_session(DismissReason::Cancelled, now);
                return;
            };
            if !self.can_commit_action(&path, &action_id) {
                tracing::info!(
                    action_id = %action_id,
                    path = ?path,
                    "XdndDrop action not eligible at synchronous pre-dispatch check; finishing with success=false"
                );
                self.platform.dnd_finish(false);
                self.dismiss_drag_session(DismissReason::Cancelled, now);
                return;
            }
            self.platform.dnd_finish(true);
            self.commit_resolved_action(&path, action_id, now);
            let drop_us = drop_t0.elapsed().as_micros() as u64;
            self.last_drop_dispatch_us = Some(drop_us);
            if self.trace_timings && self.drop_dispatch_samples_us.len() < 16_384 {
                self.drop_dispatch_samples_us.push(drop_us);
            }
        } else {
            // Capture the provisional ActionId before requesting the deferred real payload (`ARCHITECTURE.md` §A6).
            let captured_action_id = self.action_index.get(&path).cloned();
            self.state = SessionState::Committing;
            self.pending_drop_commit = Some(PendingDropSelection {
                path,
                action_id: captured_action_id,
            });
            if !self.payload_in_flight {
                self.payload_in_flight = true;
                self.platform.request_payload("text/uri-list");
            }
        }
    }

    fn handle_uri_list_payload(
        &mut self,
        result: Result<Vec<u8>, radivine_platform::PayloadError>,
        now: Instant,
    ) {
        match result {
            Ok(bytes) => {
                let basic = BasicContext::from_uri_list_bytes(
                    &bytes,
                    InvocationMode::Drag,
                    self.drag_source.clone(),
                );
                if basic.items.is_empty() {
                    tracing::info!("text/uri-list payload contained 0 items; dismissing");
                    if self.state == SessionState::Committing {
                        self.platform.dnd_finish(false);
                    }
                    self.dismiss_drag_session(DismissReason::Cancelled, now);
                    return;
                }

                let item_count = basic.items.len() as u32;
                let initial_ctx = Arc::new(Context::from_basic(basic.clone()));

                self.enrich_session_seq = self.enrich_session_seq.wrapping_add(1);
                let enrich_sid = self.enrich_session_seq;
                self.active_enrich_session_id = enrich_sid;
                self.enricher.start(enrich_sid, basic);

                let ctx = self.wait_for_initial_enrichment_context(
                    enrich_sid,
                    initial_ctx,
                    FAST_LOCAL_ENRICH_WINDOW,
                );

                let session_registry = Arc::clone(self.session_registry());
                let menu_limits = self.session_menu_limits();
                let Some((menu_tree, action_index)) =
                    session_registry.build_menu(&ctx, menu_limits)
                else {
                    tracing::info!("no applicable actions for dragged items; dismissing");
                    if self.state == SessionState::Committing {
                        self.platform.dnd_finish(false);
                    }
                    self.dismiss_drag_session(DismissReason::Cancelled, now);
                    return;
                };

                self.active_context = Some(ctx);
                self.context_is_provisional = false;
                self.active_menu_tree = Some(menu_tree.clone());
                self.action_index = action_index;
                self.refresh_active_item_states();

                if self.state == SessionState::Committing {
                    if let Some(pending) = self.pending_drop_commit.take() {
                        let resolved = pending.action_id.and_then(|target_id| {
                            let real_path = self
                                .action_index
                                .entries()
                                .iter()
                                .find(|(_, id)| *id == target_id)
                                .map(|(p, _)| p.clone())?;
                            if self.can_commit_action(&real_path, &target_id) {
                                Some((real_path, target_id))
                            } else {
                                None
                            }
                        });
                        if let Some((real_path, target_id)) = resolved {
                            self.platform.dnd_finish(true);
                            self.commit_resolved_action(&real_path, target_id, now);
                        } else {
                            tracing::info!(
                                "deferred payload resolved but captured action is absent or ineligible; finishing with success=false"
                            );
                            self.platform.dnd_finish(false);
                            self.dismiss_drag_session(DismissReason::Cancelled, now);
                        }
                    } else {
                        self.platform.dnd_finish(false);
                        self.dismiss_drag_session(DismissReason::Error, now);
                    }
                    return;
                }

                let ms = self.elapsed_millis(now);
                let initial_pos = self.last_dnd_pos.map(|at| self.root_phys_to_logical(at));
                let mut wheel_events = Vec::new();
                if let Some(ref mut ws) = self.wheel_session {
                    if let Some(pos) = initial_pos {
                        ws.set_menu_at_pointer(menu_tree, pos, ms);
                    } else {
                        ws.set_menu(menu_tree, ms);
                    }
                    if item_count > 1 {
                        ws.set_root_badge(Some(item_count), ms);
                    }
                    if self.reject_until_drag_end {
                        ws.suspend_interaction(ms);
                    } else if let Some(pos) = initial_pos {
                        ws.handle(WheelInput::PointerMoved(pos), ms, &mut wheel_events);
                    }
                }

                if self.state == SessionState::Summoning {
                    self.state = SessionState::Active;
                }

                self.sync_dnd_status_if_changed();
                let _ = self.sync_input_region();
                self.process_wheel_events(wheel_events, now);
                if self.state == SessionState::Active {
                    let _ = self.render_and_present(false);
                    let _ = self.platform.flush();
                    self.schedule_next_frame_if_animating(now);
                }
            }
            Err(err) => {
                if self.state == SessionState::Committing {
                    tracing::warn!("drop-time payload retrieval failed ({err}); aborting drop");
                    self.platform.dnd_finish(false);
                    self.dismiss_drag_session(DismissReason::Error, now);
                    return;
                }

                if self.state == SessionState::Summoning {
                    if self.dnd_entered && !self.offered_uri_list {
                        self.dismiss_drag_session(DismissReason::Cancelled, now);
                        return;
                    }
                    if !self.retried_payload_after_position {
                        if self.last_dnd_pos.is_some() {
                            self.retried_payload_after_position = true;
                            self.payload_in_flight = true;
                            self.platform.request_payload("text/uri-list");
                        }
                        return;
                    }
                    if self.offered_uri_list {
                        // Source offered `text/uri-list` on XdndEnter but refused pre-drop conversion;
                        // populate a provisional file menu and fetch the real URIs on XdndDrop.
                        self.activate_provisional_file_menu(now);
                    } else {
                        tracing::warn!("payload prefetch failed ({err}); dismissing drag session");
                        self.dismiss_drag_session(DismissReason::Error, now);
                    }
                }
            }
        }
    }

    fn activate_provisional_file_menu(&mut self, now: Instant) {
        let provisional_basic = BasicContext::from_uris(
            ["file:///provisional.txt"],
            InvocationMode::Drag,
            self.drag_source.clone(),
        );
        let mut provisional_ctx = Context::from_basic(provisional_basic);
        provisional_ctx.facts = Some(vec![FileFacts {
            kind: FileKind::Regular,
            size: Some(0),
            readable: true,
            text: TextVerdict::Text(TextEncoding::Utf8),
            sniffed_mime: Some("text/plain"),
            error: None,
        }]);
        let ctx = Arc::new(provisional_ctx);
        let session_registry = Arc::clone(self.session_registry());
        let menu_limits = self.session_menu_limits();
        let Some((menu_tree, action_index)) = session_registry.build_menu(&ctx, menu_limits) else {
            self.dismiss_drag_session(DismissReason::Error, now);
            return;
        };

        self.active_context = Some(ctx);
        self.context_is_provisional = true;
        self.active_menu_tree = Some(menu_tree.clone());
        self.action_index = action_index;
        self.refresh_active_item_states();

        let ms = self.elapsed_millis(now);
        let initial_pos = self.last_dnd_pos.map(|at| self.root_phys_to_logical(at));
        let mut wheel_events = Vec::new();
        if let Some(ref mut ws) = self.wheel_session {
            if let Some(pos) = initial_pos {
                ws.set_menu_at_pointer(menu_tree, pos, ms);
            } else {
                ws.set_menu(menu_tree, ms);
            }
            if self.reject_until_drag_end {
                ws.suspend_interaction(ms);
            } else if let Some(pos) = initial_pos {
                ws.handle(WheelInput::PointerMoved(pos), ms, &mut wheel_events);
            }
        }
        self.state = SessionState::Active;
        self.sync_dnd_status_if_changed();
        let _ = self.sync_input_region();
        self.process_wheel_events(wheel_events, now);
        let _ = self.render_and_present(false);
        let _ = self.platform.flush();
        self.schedule_next_frame_if_animating(now);
    }

    /// Sends an unsolicited `XdndStatus` reply if `drop_target()` changed while a drag source is entered
    /// (`ARCHITECTURE.md` §5, verified in Spike `S-1`).
    fn sync_dnd_status_if_changed(&mut self) {
        if self.active_mode != Some(InvocationMode::Drag) || !self.dnd_entered {
            return;
        }
        let accept = !self.reject_until_drag_end
            && self.state == SessionState::Active
            && self
                .wheel_session
                .as_ref()
                .is_some_and(|ws| matches!(ws.drop_target(), DropTarget::Accept(_)));
        if self.last_dnd_accept != Some(accept) {
            self.last_dnd_accept = Some(accept);
            self.platform.dnd_reply(accept);
        }
    }

    /// Returns the earliest timer deadline needed by the session controller or wheel engine.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        let mut earliest: Option<Instant> = self.platform.next_deadline();

        let mut merge = |candidate: Option<Instant>| {
            if let Some(c) = candidate {
                earliest = Some(match earliest {
                    Some(cur) => cur.min(c),
                    None => c,
                });
            }
        };

        match self.state {
            SessionState::Summoning | SessionState::Active | SessionState::LeftGrace => {
                if let Some(ref ws) = self.wheel_session
                    && let Some(dl_ms) = ws.next_deadline()
                {
                    merge(Some(self.session_epoch + Duration::from_millis(dl_ms)));
                }
                merge(self.next_frame_deadline);
                merge(self.leave_grace_deadline);
                merge(Some(self.session_epoch + MAX_SESSION_DURATION));
                if self.active_mode == Some(InvocationMode::Invoke) {
                    merge(Some(self.last_interaction + ACTIVE_WATCHDOG_TIMEOUT));
                }
            }
            SessionState::Committing => {
                merge(Some(self.session_epoch + MAX_SESSION_DURATION));
            }
            SessionState::Idle => {
                merge(self.idle_release_deadline);
            }
            SessionState::Armed | SessionState::Dismissing => {}
        }

        earliest
    }

    /// Advances timers/animations due at `now`.
    pub fn on_timer(&mut self, now: Instant) {
        self.drain_async_events(now);
        let mut plat_events = Vec::new();
        self.platform.on_deadline(now, &mut plat_events);
        if !plat_events.is_empty() {
            self.handle_platform_events(plat_events);
        }

        if self.state == SessionState::Idle {
            if let Some(release_at) = self.idle_release_deadline
                && now >= release_at
            {
                self.idle_release_deadline = None;
                self.platform.release_buffers();
                self.renderer.release_buffers();
                tracing::debug!("released idle SHM and renderer pixel buffers");
            }
            return;
        }

        if !matches!(
            self.state,
            SessionState::Summoning | SessionState::Active | SessionState::LeftGrace
        ) {
            return;
        }

        // 120 s max session duration watchdog (`ARCHITECTURE.md` §5)
        if now.duration_since(self.session_epoch) >= MAX_SESSION_DURATION {
            tracing::warn!("session hit 120 s max duration watchdog; force-dismissing");
            self.dismiss_drag_session(DismissReason::Timeout, now);
            return;
        }

        // 60 s Invoke mode inactivity watchdog
        if self.active_mode == Some(InvocationMode::Invoke)
            && now.duration_since(self.last_interaction) >= ACTIVE_WATCHDOG_TIMEOUT
        {
            tracing::warn!("Invoke session hit 60 s inactivity watchdog; force-dismissing");
            let ms = self.elapsed_millis(now);
            if let Some(ref mut ws) = self.wheel_session {
                ws.dismiss(DismissReason::Timeout, ms);
            }
            self.dismiss_immediate();
            return;
        }

        // Drag mode leave-grace timeout (`AC-3.8`)
        if self.state == SessionState::LeftGrace
            && let Some(leave_dl) = self.leave_grace_deadline
            && now >= leave_dl
        {
            self.leave_grace_deadline = None;
            tracing::info!("pointer remained outside wheel past leave_grace_ms; dismissing");
            self.dismiss_drag_session(DismissReason::DragEnded, now);
            return;
        }

        let engine_due = self
            .wheel_session
            .as_ref()
            .and_then(WheelSession::next_deadline)
            .is_some_and(|dl_ms| now >= self.session_epoch + Duration::from_millis(dl_ms));
        let frame_due = self.next_frame_deadline.is_some_and(|dl| now >= dl);

        if engine_due || frame_due {
            self.next_frame_deadline = None;
            let ms = self.elapsed_millis(now);
            let mut wheel_events = Vec::new();
            if let Some(ref mut ws) = self.wheel_session {
                ws.tick(ms, &mut wheel_events);
            }
            self.process_wheel_events(wheel_events, now);
            self.sync_dnd_status_if_changed();
            if matches!(
                self.state,
                SessionState::Summoning | SessionState::Active | SessionState::LeftGrace
            ) {
                let _ = self.render_and_present(false);
                let _ = self.platform.flush();
                self.schedule_next_frame_if_animating(now);
            }
        }
    }

    fn feed_wheel_input(&mut self, input: WheelInput, now: Instant) {
        let ms = self.elapsed_millis(now);
        let mut wheel_events = Vec::new();
        if let Some(ref mut ws) = self.wheel_session {
            ws.handle(input, ms, &mut wheel_events);
        }
        self.process_wheel_events(wheel_events, now);
        self.sync_dnd_status_if_changed();
        if matches!(
            self.state,
            SessionState::Summoning | SessionState::Active | SessionState::LeftGrace
        ) {
            let _ = self.render_and_present(false);
            let _ = self.platform.flush();
            self.schedule_next_frame_if_animating(now);
        }
    }

    fn process_wheel_events(&mut self, events: Vec<WheelEvent>, now: Instant) {
        let non_translucent = self.overlay_info.as_ref().is_some_and(|i| !i.translucent);
        for ev in events {
            match ev {
                WheelEvent::HoverChanged(_)
                | WheelEvent::SubmenuOpened(_)
                | WheelEvent::SubmenuClosed(_) => {
                    if non_translucent {
                        let _ = self.sync_input_region();
                    }
                }
                WheelEvent::InputRegionChanged => {
                    let _ = self.sync_input_region();
                }
                WheelEvent::Commit(path) => {
                    self.commit_path(&path, now);
                    return;
                }
                WheelEvent::CancelRequested | WheelEvent::Finished => {
                    tracing::info!("radial wheel session cancelled");
                    self.dismiss_immediate();
                    return;
                }
            }
        }
    }

    fn can_commit_action(&self, path: &ItemPath, action_id: &ActionId) -> bool {
        let Some(ref ctx) = self.active_context else {
            return false;
        };
        if let Some(ref menu_tree) = self.active_menu_tree
            && !menu_tree_path_and_ancestors_enabled(menu_tree, path)
        {
            return false;
        }
        let Some(action) = self.session_registry().get(action_id) else {
            return false;
        };
        action.is_visible(&ctx.basic) && matches!(action.enablement(ctx), Enablement::Enabled)
    }

    fn commit_path(&mut self, path: &ItemPath, now: Instant) {
        self.drain_async_events(now);
        let action_id: ActionId = self
            .action_index
            .get(path)
            .cloned()
            .unwrap_or_else(|| ActionId::from(format!("path:{path:?}")));
        self.commit_resolved_action(path, action_id, now);
    }

    fn commit_resolved_action(&mut self, path: &ItemPath, action_id: ActionId, now: Instant) {
        self.drain_async_events(now);
        let was_invoke = self.active_mode == Some(InvocationMode::Invoke);
        self.state = SessionState::Committing;
        if was_invoke {
            self.platform.ungrab_input();
        }
        self.platform.hide_overlay();
        self.next_frame_deadline = None;
        self.leave_grace_deadline = None;
        self.pending_drop_commit = None;
        self.dnd_entered = false;
        self.last_dnd_accept = None;

        let (uris, source_app, source_pid, mode_str) = self.active_context.as_ref().map_or_else(
            || (Vec::new(), None, None, "Invoke"),
            |ctx| {
                let items: Vec<String> = ctx.basic.items.iter().map(|it| it.uri.clone()).collect();
                let mode = match ctx.basic.mode {
                    InvocationMode::Drag => "Drag",
                    InvocationMode::Invoke => "Invoke",
                };
                (
                    items,
                    ctx.basic.source.app.clone(),
                    ctx.basic.source.pid,
                    mode,
                )
            },
        );

        let elapsed_ms = self.elapsed_millis(now);
        tracing::info!(
            target: "radivine::commit",
            action_id = %action_id,
            path = ?path,
            mode = mode_str,
            source_app = ?source_app,
            source_pid = ?source_pid,
            uris = ?uris,
            elapsed_ms = elapsed_ms,
            "committed radial wheel action"
        );

        if let Ok(mut guard) = self.last_commit.lock() {
            guard.action_id = Some(action_id.as_str().to_string());
            guard.path = Some(path.as_slice().to_vec());
            guard.items = uris;
            guard.source_app = source_app;
            guard.mode = Some(mode_str.to_string());
        }

        let session_registry = Arc::clone(self.session_registry());
        if let Some(action) = session_registry.get(&action_id)
            && let Some(ref ctx) = self.active_context
        {
            // Pre-dispatch enablement re-check (`ACTION_SYSTEM.md` §7)
            match action.enablement(ctx) {
                Enablement::Enabled => {
                    self.dispatcher.dispatch(action, Arc::clone(ctx));
                    self.wait_for_fast_job(FAST_COMMIT_EFFECT_WINDOW, now);
                }
                Enablement::Disabled { reason } => {
                    let msg = reason.to_string();
                    tracing::warn!(
                        action_id = %action_id,
                        reason = %msg,
                        "action became disabled before dispatch; skipping"
                    );
                    self.last_job_action = Some(action_id.as_str().to_string());
                    self.last_job_error = Some(msg.clone());
                    self.emit_notification("Action unavailable", &msg);
                }
                Enablement::Pending => {
                    let msg = "Still inspecting file…".to_string();
                    tracing::warn!(
                        action_id = %action_id,
                        "action still pending enrichment at commit time; skipping"
                    );
                    self.last_job_action = Some(action_id.as_str().to_string());
                    self.last_job_error = Some(msg.clone());
                    self.emit_notification("Action unavailable", &msg);
                }
            }
        }

        self.active_mode = None;
        self.clear_active_session_snapshots();
        self.state = if self.drag_armed {
            SessionState::Armed
        } else {
            SessionState::Idle
        };
        if self.state == SessionState::Idle {
            self.idle_release_deadline = Some(now + IDLE_BUFFER_RELEASE_DELAY);
        }
    }

    /// Drains all pending [`EnrichEvent`] and [`JobEvent`] messages from background workers.
    pub fn drain_async_events(&mut self, now: Instant) {
        while let Ok(ev) = self.async_rx.try_recv() {
            self.process_async_event(ev, now);
        }
    }

    fn wait_for_initial_enrichment_context(
        &mut self,
        target_session_id: u64,
        mut latest_ctx: Arc<Context>,
        timeout: Duration,
    ) -> Arc<Context> {
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let remaining = deadline.saturating_duration_since(now);
            match self.async_rx.recv_timeout(remaining) {
                Ok(AsyncControllerEvent::Enrich(EnrichEvent::Updated {
                    session_id,
                    context,
                    complete,
                })) => {
                    if session_id == target_session_id {
                        latest_ctx = context;
                        if complete {
                            break;
                        }
                    }
                }
                Ok(ev) => {
                    self.process_async_event(ev, Instant::now());
                }
                Err(_) => break,
            }
        }
        latest_ctx
    }

    fn wait_for_fast_job(&mut self, timeout: Duration, now: Instant) {
        let deadline = Instant::now() + timeout;
        loop {
            let cur = Instant::now();
            if cur >= deadline {
                break;
            }
            let remaining = deadline.saturating_duration_since(cur);
            match self.async_rx.recv_timeout(remaining) {
                Ok(AsyncControllerEvent::Job(job_ev)) => {
                    self.handle_job_event(job_ev);
                    break;
                }
                Ok(ev) => {
                    self.process_async_event(ev, now);
                }
                Err(_) => break,
            }
        }
        self.drain_async_events(now);
    }

    fn process_async_event(&mut self, ev: AsyncControllerEvent, now: Instant) {
        match ev {
            AsyncControllerEvent::Enrich(EnrichEvent::Updated {
                session_id,
                context,
                ..
            }) => {
                if session_id == self.active_enrich_session_id {
                    self.apply_enrichment_context(context, now);
                }
            }
            AsyncControllerEvent::Job(job_ev) => {
                self.handle_job_event(job_ev);
            }
        }
    }

    fn apply_enrichment_context(&mut self, new_ctx: Arc<Context>, now: Instant) {
        self.active_context = Some(Arc::clone(&new_ctx));

        let updates = self
            .session_registry()
            .enablement_updates(&new_ctx, &self.action_index);
        let new_states: Vec<ItemStateSnapshot> = updates
            .iter()
            .filter_map(|u| {
                let id = self.action_index.get(&u.path)?;
                Some(ItemStateSnapshot {
                    id: id.as_str().to_string(),
                    path: u.path.as_slice().to_vec(),
                    enabled: u.enabled,
                    disabled_reason: u.disabled_reason.as_ref().map(ToString::to_string),
                })
            })
            .collect();

        let ms = self.elapsed_millis(now);
        let mut any_changed = false;
        for u in updates {
            let reason_str = u.disabled_reason.as_ref().map(ToString::to_string);
            let unchanged = self.active_item_states.iter().any(|prev| {
                prev.path.as_slice() == u.path.as_slice()
                    && prev.enabled == u.enabled
                    && prev.disabled_reason == reason_str
            });
            if !unchanged {
                if let Some(ref mut menu_tree) = self.active_menu_tree
                    && let Some(node) = menu_tree.node_mut(&u.path)
                {
                    node.enabled = u.enabled;
                    node.disabled_reason = u.disabled_reason.clone();
                }
                if let Some(ref mut ws) = self.wheel_session {
                    ws.set_enabled_at(&u.path, u.enabled, u.disabled_reason, ms);
                }
                any_changed = true;
            }
        }
        self.active_item_states = new_states;

        if any_changed {
            self.sync_dnd_status_if_changed();
            if matches!(self.state, SessionState::Active | SessionState::LeftGrace)
                && self.overlay_info.is_some()
            {
                let _ = self.render_and_present(false);
                let _ = self.platform.flush();
            }
        }
    }

    fn refresh_active_item_states(&mut self) {
        let Some(ref ctx) = self.active_context else {
            self.active_item_states.clear();
            return;
        };
        let updates = self
            .session_registry()
            .enablement_updates(ctx, &self.action_index);
        self.active_item_states = updates
            .iter()
            .filter_map(|u| {
                let id = self.action_index.get(&u.path)?;
                Some(ItemStateSnapshot {
                    id: id.as_str().to_string(),
                    path: u.path.as_slice().to_vec(),
                    enabled: u.enabled,
                    disabled_reason: u.disabled_reason.as_ref().map(ToString::to_string),
                })
            })
            .collect();
    }

    fn handle_job_event(&mut self, ev: JobEvent) {
        let JobEvent::Finished { action, result, .. } = ev;
        self.last_job_action = Some(action.as_str().to_string());
        match result {
            Ok(Outcome::Done) => {
                self.last_job_error = None;
            }
            Ok(Outcome::Message(msg)) => {
                self.last_job_error = None;
                self.emit_notification(action.as_str(), &msg);
            }
            Ok(Outcome::Effects(effects)) => {
                self.last_job_error = None;
                for effect in effects {
                    match effect {
                        Effect::SetClipboard(ClipboardRequest {
                            uris,
                            local_paths,
                            op: ClipOp::Copy,
                        }) => {
                            if let Err(err) = self.platform.set_clipboard(ClipboardContent {
                                uris,
                                local_paths,
                                operation: PlatClipOp::Copy,
                            }) {
                                let err_msg = err.to_string();
                                tracing::warn!(error = %err_msg, "failed to set X11 clipboard");
                                self.last_job_error = Some(err_msg.clone());
                                self.emit_notification("Clipboard error", &err_msg);
                            } else {
                                let _ = self.platform.flush();
                            }
                        }
                        Effect::Notify { summary, body } => {
                            self.emit_notification(&summary, body.as_deref().unwrap_or(""));
                        }
                    }
                }
            }
            Err(err) => {
                let err_msg = err.to_string();
                self.last_job_error = Some(err_msg.clone());
                let title = format!("Action failed ({action})");
                if self.config.notifications.notify_on_action_error {
                    self.emit_notification(&title, &err_msg);
                } else {
                    self.record_notification_only(&title, &err_msg);
                }
            }
        }
    }

    fn record_notification_only(&mut self, summary: &str, body: &str) {
        let formatted = if body.is_empty() {
            summary.to_string()
        } else {
            format!("{summary}: {body}")
        };
        self.last_notification = Some(formatted.clone());
        if self.notifications.len() >= 32 {
            self.notifications.remove(0);
        }
        self.notifications.push(formatted);
    }

    fn emit_notification(&mut self, summary: &str, body: &str) {
        self.record_notification_only(summary, body);
        if self.config.notifications.enabled {
            let _ = self.notify_tx.send((summary.to_string(), body.to_string()));
        }
    }

    fn elapsed_millis(&self, now: Instant) -> Millis {
        now.saturating_duration_since(self.session_epoch)
            .as_millis() as Millis
    }

    fn schedule_next_frame_if_animating(&mut self, _now: Instant) {
        if !matches!(
            self.state,
            SessionState::Summoning | SessionState::Active | SessionState::LeftGrace
        ) {
            self.next_frame_deadline = None;
            return;
        }
        let animating = self
            .wheel_session
            .as_ref()
            .is_some_and(WheelSession::is_animating);
        if animating {
            let refresh_mhz = self
                .overlay_info
                .as_ref()
                .map_or(60_000, |i| i.monitor.refresh_mhz.max(30_000));
            let frame_ns = 1_000_000_000_000u64 / u64::from(refresh_mhz);
            self.next_frame_deadline = Some(Instant::now() + Duration::from_nanos(frame_ns));
        } else {
            self.next_frame_deadline = None;
        }
    }

    fn root_phys_to_logical(&self, root_pt: PhysPoint) -> Point {
        let (ox, oy) = self
            .overlay_info
            .as_ref()
            .map_or((0, 0), |i| (i.origin.x, i.origin.y));
        let scale = self.session_scale().max(0.5);
        Point::new(
            ((root_pt.x - ox) as f32) / scale,
            ((root_pt.y - oy) as f32) / scale,
        )
    }

    fn sync_input_region(&mut self) -> anyhow::Result<()> {
        let translucent = self.overlay_info.as_ref().is_none_or(|i| i.translucent);
        if translucent && self.bounding_region_clipped {
            self.platform.set_bounding_region(None)?;
            self.bounding_region_clipped = false;
        }
        let passthrough = self.session_passthrough();
        // Fast path: with a compositor (`translucent == true`) in Invoke mode or capture-all Drag mode
        // (`input.passthrough == false`, safe default per ADR-016 / AGENTS.md §2.4), `ShapeBounding`
        // is already unclipped and `ShapeInput` is full-window (`None`).
        if translucent && (self.active_mode == Some(InvocationMode::Invoke) || !passthrough) {
            self.platform.set_input_region(None)?;
            return Ok(());
        }

        let Some(ref ws) = self.wheel_session else {
            return Ok(());
        };
        let scale = self.session_scale();
        let shapes = ws.input_region();
        let mut shapes_px: Vec<ShapePx> = Vec::with_capacity(shapes.len() + 1);
        for s in shapes {
            match s {
                Shape::Circle { c, r } => {
                    shapes_px.push(ShapePx::Circle {
                        cx: c.x * scale,
                        cy: c.y * scale,
                        r: r * scale,
                    });
                }
                Shape::Polygon(vertices) => {
                    let mut poly: SmallVec<[(f32, f32); 6]> = SmallVec::new();
                    for v in vertices {
                        poly.push((v.x * scale, v.y * scale));
                    }
                    shapes_px.push(ShapePx::Polygon(poly));
                }
            }
        }

        // No-compositor fallback (`PLATFORM_INTEGRATION.md` §3.11): clip `ShapeBounding` strictly to
        // the visible wheel disks + caption box (excluding invisible pointer hit-slop corridors) when
        // per-pixel alpha translucency is unavailable.
        if !translucent {
            let scene = ws.scene();
            let mut bounding_px: Vec<ShapePx> = Vec::with_capacity(scene.wheels.len() + 2);
            for w in &scene.wheels {
                let visual_r = (w.r_out + 4.0) * scale;
                bounding_px.push(ShapePx::Circle {
                    cx: w.center.x * scale,
                    cy: w.center.y * scale,
                    r: visual_r,
                });
            }
            if bounding_px.is_empty() {
                bounding_px.clone_from(&shapes_px);
            }
            if let Some(ref cap) = scene.caption {
                let x0 = cap.box_.x * scale;
                let y0 = cap.box_.y * scale;
                let x1 = (cap.box_.x + cap.box_.w) * scale;
                let y1 = (cap.box_.y + cap.box_.h) * scale;
                let mut rect_poly: SmallVec<[(f32, f32); 6]> = SmallVec::new();
                rect_poly.push((x0, y0));
                rect_poly.push((x1, y0));
                rect_poly.push((x1, y1));
                rect_poly.push((x0, y1));
                bounding_px.push(ShapePx::Polygon(rect_poly));
            }
            self.platform.set_bounding_region(Some(&bounding_px))?;
            self.bounding_region_clipped = true;
        }

        if self.active_mode == Some(InvocationMode::Invoke) || !passthrough {
            self.platform.set_input_region(None)?;
            return Ok(());
        }
        self.platform.set_input_region(Some(&shapes_px))?;
        Ok(())
    }

    fn render_and_present(&mut self, force_full: bool) -> anyhow::Result<()> {
        let frame_t0 = Instant::now();
        let Some(ref ws) = self.wheel_session else {
            return Ok(());
        };
        let dmg_slice = self.renderer.render(ws.scene());
        let mut damage: Vec<PlatRectPx> = dmg_slice
            .iter()
            .map(|r| PlatRectPx::new(r.x, r.y, r.w, r.h))
            .collect();

        if self.debug_overlay {
            let debug_rects: Vec<RenderRectPx> = dmg_slice.to_vec();
            if let Some(pm) = self.renderer.pixmap_mut() {
                draw_debug_damage_boxes(pm, &debug_rects);
            }
        }

        let frame = self.renderer.frame();
        if force_full && damage.is_empty() && frame.width > 0 && frame.height > 0 {
            damage.push(PlatRectPx::new(0, 0, frame.width, frame.height));
        }

        if !damage.is_empty() {
            let plat_frame = PlatFrameRef {
                data: frame.data,
                width: frame.width,
                height: frame.height,
                stride: frame.stride,
            };
            self.platform.present(plat_frame, &damage)?;
        }
        let frame_us = frame_t0.elapsed().as_micros() as u64;
        self.last_frame_render_us = Some(frame_us);
        if self.trace_timings && self.frame_render_samples_us.len() < 16_384 {
            self.frame_render_samples_us.push(frame_us);
        }
        Ok(())
    }
}

fn menu_limits_from_wheel_config(wheel_cfg: &WheelConfig) -> MenuLimits {
    let validated = wheel_cfg.validated();
    MenuLimits {
        max_items_per_wheel: validated.max_items_per_wheel,
        max_depth: validated.max_depth,
    }
}

fn menu_tree_path_and_ancestors_enabled(menu: &MenuTree, path: &ItemPath) -> bool {
    if path.is_empty() {
        return menu.root.enabled;
    }
    let slice = path.as_slice();
    for len in 1..=slice.len() {
        let prefix = ItemPath::from_slice(&slice[..len]);
        if !menu.node(&prefix).is_some_and(|n| n.enabled) {
            return false;
        }
    }
    true
}

fn query_self_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let num_str = rest.split_whitespace().next()?;
            return num_str.parse::<u64>().ok();
        }
    }
    None
}

fn draw_debug_damage_boxes(pm: &mut tiny_skia::Pixmap, rects: &[RenderRectPx]) {
    let w = pm.width() as i32;
    let h = pm.height() as i32;
    let stride = (w as usize) * 4;
    let data = pm.data_mut();
    for r in rects {
        let x0 = r.x.clamp(0, w - 1) as usize;
        let y0 = r.y.clamp(0, h - 1) as usize;
        let x1 = (r.right() - 1).clamp(0, w - 1) as usize;
        let y1 = (r.bottom() - 1).clamp(0, h - 1) as usize;
        for x in x0..=x1 {
            for &y in &[y0, y1] {
                let off = y * stride + x * 4;
                data[off..off + 4].copy_from_slice(&[0x00, 0xFF, 0x80, 0xFF]);
            }
        }
        for y in y0..=y1 {
            for &x in &[x0, x1] {
                let off = y * stride + x * 4;
                data[off..off + 4].copy_from_slice(&[0x00, 0xFF, 0x80, 0xFF]);
            }
        }
    }
}

const fn translate_platform_key(key: PlatKey) -> NavKey {
    match key {
        PlatKey::Right | PlatKey::Down | PlatKey::Tab => NavKey::Next,
        PlatKey::Left | PlatKey::Up | PlatKey::ShiftTab => NavKey::Prev,
        PlatKey::Enter | PlatKey::Space => NavKey::Activate,
        PlatKey::Backspace => NavKey::Back,
        PlatKey::Escape => NavKey::Cancel,
        PlatKey::Digit(d) => NavKey::Digit(d),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use radivine_actions::builtin_registry;
    use radivine_platform::{
        Capabilities, ClipboardContent, Monitor, PlatformError, RectPx, SourceInfo, Support,
    };
    use radivine_render::AssetSource;

    struct MockPlatformState {
        mapped: bool,
        grabbed: bool,
        dnd_replies: Vec<bool>,
        dnd_finishes: Vec<bool>,
        requested_payloads: Vec<String>,
        input_region_is_shaped: bool,
        translucent: bool,
        scale: f32,
        monitor_size: (u32, u32),
        prepare_overlay_calls: usize,
    }

    impl Default for MockPlatformState {
        fn default() -> Self {
            Self {
                mapped: false,
                grabbed: false,
                dnd_replies: Vec::new(),
                dnd_finishes: Vec::new(),
                requested_payloads: Vec::new(),
                input_region_is_shaped: false,
                translucent: true,
                scale: 1.0,
                monitor_size: (1920, 1080),
                prepare_overlay_calls: 0,
            }
        }
    }

    struct MockPlatform {
        caps: Capabilities,
        monitors: Vec<Monitor>,
        state: Arc<Mutex<MockPlatformState>>,
    }

    impl MockPlatform {
        fn new() -> (Self, Arc<Mutex<MockPlatformState>>) {
            let state = Arc::new(Mutex::new(MockPlatformState::default()));
            let plat = Self {
                caps: Capabilities {
                    drag_session: Support::Yes,
                    invoke_at_pointer: Support::Yes,
                    translucency: Support::Yes,
                    input_passthrough: Support::Yes,
                    keyboard_in_drag: Support::No("drag source holds keyboard grab"),
                    clipboard_files: Support::Yes,
                },
                monitors: vec![Monitor {
                    id: 0,
                    name: "HDMI-0".into(),
                    rect: RectPx::new(0, 0, 1920, 1080),
                    refresh_mhz: 60_000,
                    primary: true,
                }],
                state: Arc::clone(&state),
            };
            (plat, state)
        }
    }

    impl Platform for MockPlatform {
        fn name(&self) -> &'static str {
            "mock"
        }

        fn capabilities(&self) -> &Capabilities {
            &self.caps
        }

        #[cfg(unix)]
        fn poll_fd(&self) -> std::os::fd::RawFd {
            0
        }

        fn dispatch_pending(&mut self, _out: &mut Vec<PlatformEvent>) -> Result<(), PlatformError> {
            Ok(())
        }

        fn monitors(&self) -> &[Monitor] {
            &self.monitors
        }

        fn scale(&self) -> f32 {
            self.state.lock().expect("lock").scale
        }

        fn reduced_motion(&self) -> Option<bool> {
            Some(true)
        }

        fn pointer_position(&mut self) -> Result<PhysPoint, PlatformError> {
            Ok(PhysPoint::new(960, 540))
        }

        fn overlay_window_id(&self) -> u32 {
            0x0500_0001
        }

        fn prepare_overlay(&mut self, _at: PhysPoint) -> Result<OverlayInfo, PlatformError> {
            let mut st = self.state.lock().expect("lock");
            st.prepare_overlay_calls += 1;
            let (w, h) = st.monitor_size;
            let scale = st.scale;
            let translucent = st.translucent;
            drop(st);
            let mut monitor = self.monitors[0].clone();
            monitor.rect = RectPx::new(0, 0, w, h);
            Ok(OverlayInfo {
                origin: PhysPoint::new(0, 0),
                size: (w, h),
                scale,
                monitor,
                translucent,
            })
        }

        fn sync_graphical_state(&mut self) {
            let st = self.state.lock().expect("lock");
            let (w, h) = st.monitor_size;
            let translucent = st.translucent;
            drop(st);
            self.monitors[0].rect = RectPx::new(0, 0, w, h);
            self.caps.translucency = if translucent {
                Support::Yes
            } else {
                Support::Degraded("no compositor")
            };
        }

        fn map_overlay(&mut self) -> Result<(), PlatformError> {
            self.state.lock().expect("lock").mapped = true;
            Ok(())
        }

        fn present(
            &mut self,
            _frame: PlatFrameRef<'_>,
            _damage: &[PlatRectPx],
        ) -> Result<(), PlatformError> {
            Ok(())
        }

        fn set_input_region(&mut self, shapes: Option<&[ShapePx]>) -> Result<(), PlatformError> {
            self.state.lock().expect("lock").input_region_is_shaped = shapes.is_some();
            Ok(())
        }

        fn hide_overlay(&mut self) {
            self.state.lock().expect("lock").mapped = false;
        }

        fn release_buffers(&mut self) {}

        fn dnd_reply(&mut self, accept: bool) {
            self.state.lock().expect("lock").dnd_replies.push(accept);
        }

        fn request_payload(&mut self, mime: &str) {
            self.state
                .lock()
                .expect("lock")
                .requested_payloads
                .push(mime.to_string());
        }

        fn dnd_finish(&mut self, success: bool) {
            self.state.lock().expect("lock").dnd_finishes.push(success);
        }

        fn grab_input(&mut self) -> Result<(), PlatformError> {
            self.state.lock().expect("lock").grabbed = true;
            Ok(())
        }

        fn ungrab_input(&mut self) {
            self.state.lock().expect("lock").grabbed = false;
        }

        fn set_clipboard(&mut self, _content: ClipboardContent) -> Result<(), PlatformError> {
            Ok(())
        }

        fn flush(&mut self) -> Result<(), PlatformError> {
            Ok(())
        }

        fn next_deadline(&self) -> Option<Instant> {
            None
        }

        fn on_deadline(&mut self, _now: Instant, _out: &mut Vec<PlatformEvent>) {}
    }

    #[test]
    fn drag_session_lifecycle_commit_reject_and_toggle_off() {
        let (mock, plat_state) = MockPlatform::new();
        let cfg = AppConfig::default();
        let renderer = Renderer::new(cfg.resolve_theme(), AssetSource::Embedded).expect("renderer");
        let registry = Arc::new(builtin_registry());
        let mut ctrl = SessionController::new(Box::new(mock), renderer, registry, cfg);
        assert_eq!(ctrl.status().session_state, "Idle");

        // 1. Arm drag from Nemo
        ctrl.handle_platform_events(vec![PlatformEvent::DragArmed {
            source: SourceInfo {
                app: Some("nemo".to_string()),
                pid: Some(4242),
            },
        }]);
        assert_eq!(ctrl.status().session_state, "Armed");

        // 2. Press fresh Shift at (960, 540) -> Summoning + prefetch requested + never grabbed!
        ctrl.handle_platform_events(vec![PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        }]);
        assert_eq!(ctrl.status().session_state, "Summoning");
        {
            let st = plat_state.lock().expect("lock");
            assert!(st.mapped);
            assert!(!st.grabbed);
            assert_eq!(st.requested_payloads, vec!["text/uri-list".to_string()]);
        }

        // 3. Deliver prefetched payload -> transitions Summoning -> Active
        ctrl.handle_platform_events(vec![
            PlatformEvent::DndEnter {
                types: vec!["text/uri-list".to_string()],
            },
            PlatformEvent::Payload {
                mime: "text/uri-list".to_string(),
                result: Ok(b"file:///tmp/demo%20file.txt\r\n".to_vec()),
            },
        ]);
        assert_eq!(ctrl.status().session_state, "Active");

        // 4. Move pointer onto center hub (960, 540) -> status reply is false (reject)
        ctrl.handle_platform_events(vec![PlatformEvent::DndPosition {
            at: PhysPoint::new(960, 540),
        }]);
        assert_eq!(
            plat_state.lock().expect("lock").dnd_replies.last().copied(),
            Some(false)
        );

        // 5. Move pointer onto top-right leaf segment (`Copy`, index 1 at ~+90 deg -> x=960+80, y=540)
        ctrl.handle_platform_events(vec![PlatformEvent::DndPosition {
            at: PhysPoint::new(1040, 540),
        }]);
        assert_eq!(
            plat_state.lock().expect("lock").dnd_replies.last().copied(),
            Some(true)
        );

        // 6. Drop on `Copy` -> dnd_finish(true), overlay unmapped, action committed with source_app="nemo"
        ctrl.handle_platform_events(vec![PlatformEvent::DndDrop]);
        {
            let st = plat_state.lock().expect("lock");
            assert!(!st.mapped);
            assert_eq!(st.dnd_finishes, vec![true]);
        }
        let status = ctrl.status();
        assert_eq!(
            status.last_committed_action.as_deref(),
            Some("radivine.copy")
        );
        assert_eq!(
            status.last_committed_items,
            vec!["file:///tmp/demo%20file.txt".to_string()]
        );
        assert_eq!(status.last_committed_source_app.as_deref(), Some("nemo"));
        assert_eq!(status.last_committed_mode.as_deref(), Some("Drag"));

        // 7. Trigger again while still armed, then press second Shift to toggle off -> unmapped, Armed
        ctrl.handle_platform_events(vec![
            PlatformEvent::Trigger {
                at: PhysPoint::new(960, 540),
            },
            PlatformEvent::Payload {
                mime: "text/uri-list".to_string(),
                result: Ok(b"file:///tmp/demo%20file.txt\r\n".to_vec()),
            },
        ]);
        assert_eq!(ctrl.status().session_state, "Active");
        ctrl.handle_platform_events(vec![PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        }]);
        assert_eq!(ctrl.status().session_state, "Armed");
        assert!(!plat_state.lock().expect("lock").mapped);

        // 8. Trigger again and release on center hub (reject zone) -> dnd_finish(false), no new commit
        ctrl.handle_platform_events(vec![
            PlatformEvent::Trigger {
                at: PhysPoint::new(960, 540),
            },
            PlatformEvent::Payload {
                mime: "text/uri-list".to_string(),
                result: Ok(b"file:///tmp/other.txt\r\n".to_vec()),
            },
            PlatformEvent::DndPosition {
                at: PhysPoint::new(960, 540),
            },
            PlatformEvent::DndDrop,
        ]);
        assert_eq!(
            plat_state
                .lock()
                .expect("lock")
                .dnd_finishes
                .last()
                .copied(),
            Some(false)
        );
        // Last committed item is still the earlier commit, not `other.txt`
        assert_eq!(
            ctrl.status().last_committed_items,
            vec!["file:///tmp/demo%20file.txt".to_string()]
        );

        // 9. Disarm returns to Idle
        ctrl.handle_platform_events(vec![PlatformEvent::DragDisarmed]);
        assert_eq!(ctrl.status().session_state, "Idle");
    }

    #[test]
    fn active_session_settings_snapshot_across_phases_and_reloads() {
        let (mock, plat_state) = MockPlatform::new();
        let mut initial_cfg = AppConfig::default();
        initial_cfg.input.passthrough = true;
        initial_cfg.trigger.toggle_off_on_repress = true;
        initial_cfg.wheel.leave_grace_ms = 400;
        let renderer =
            Renderer::new(initial_cfg.resolve_theme(), AssetSource::Embedded).expect("renderer");
        let registry = Arc::new(builtin_registry());
        let mut ctrl =
            SessionController::new(Box::new(mock), renderer, Arc::clone(&registry), initial_cfg);

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let actions_v1 = temp_dir.join_path_or_fallback("actions_v1.toml");
        let actions_v2 = temp_dir.join_path_or_fallback("actions_v2.toml");
        std::fs::write(
            &actions_v1,
            r#"
version = 1

[[action]]
id = "custom.alpha"
label = "Alpha Action"
exec = ["true"]

[menu]
order = ["radivine.copy", "radivine.properties", "radivine.preview", "radivine.custom"]
"#,
        )
        .expect("write actions_v1");
        std::fs::write(
            &actions_v2,
            r#"
version = 1

[[action]]
id = "custom.beta"
label = "Beta Action"
exec = ["true"]

[menu]
order = ["radivine.properties", "radivine.copy", "radivine.preview", "radivine.custom"]
"#,
        )
        .expect("write actions_v2");

        ctrl.load_actions_from_path(&actions_v1);

        // Arm drag and summon into Summoning phase
        ctrl.handle_platform_events(vec![
            PlatformEvent::DragArmed {
                source: SourceInfo {
                    app: Some("nemo".to_string()),
                    pid: Some(100),
                },
            },
            PlatformEvent::Trigger {
                at: PhysPoint::new(960, 540),
            },
        ]);
        assert_eq!(ctrl.status().session_state, "Summoning");
        assert!(ctrl.active_session_settings().is_some());
        assert!(ctrl.active_session_settings().unwrap().passthrough);

        // Reload config and actions during Summoning: snapshot must remain untouched
        let mut reloaded_cfg = AppConfig::default();
        reloaded_cfg.wheel.layout = "concentric".to_string();
        reloaded_cfg.appearance.scale = crate::config::ScaleSetting::Fixed(2.0);
        reloaded_cfg.input.passthrough = false;
        reloaded_cfg.trigger.toggle_off_on_repress = false;
        ctrl.reload_all(reloaded_cfg.clone(), &actions_v2);

        let snap_summoning = ctrl
            .active_session_settings()
            .expect("snapshot in Summoning");
        assert_eq!(snap_summoning.config.wheel.layout, "branching");
        assert_eq!(snap_summoning.scale, 1.0);
        assert!(snap_summoning.passthrough);
        assert!(
            snap_summoning
                .registry
                .get(&ActionId::from("custom.alpha"))
                .is_some()
        );
        assert!(
            snap_summoning
                .registry
                .get(&ActionId::from("custom.beta"))
                .is_none()
        );
        assert!(plat_state.lock().expect("lock").mapped);

        // Deliver payload -> Active phase; menu built from snapshotted registry (`radivine.copy` at `[0]`)
        ctrl.handle_platform_events(vec![
            PlatformEvent::DndEnter {
                types: vec!["text/uri-list".to_string()],
            },
            PlatformEvent::Payload {
                mime: "text/uri-list".to_string(),
                result: Ok(b"file:///tmp/demo.txt\r\n".to_vec()),
            },
        ]);
        assert_eq!(ctrl.status().session_state, "Active");
        assert_eq!(
            ctrl.action_index.get(&ItemPath::from_slice(&[0])),
            Some(&ActionId::from("radivine.copy"))
        );

        // Transition to LeftGrace and reload again: still uses snapshot
        ctrl.handle_platform_events(vec![PlatformEvent::DndLeave { drag_ended: false }]);
        assert_eq!(ctrl.status().session_state, "LeftGrace");
        ctrl.reload_all(reloaded_cfg.clone(), &actions_v2);
        assert_eq!(ctrl.status().scale, 1.0);
        assert!(plat_state.lock().expect("lock").mapped);

        // Re-enter Active, move to top item `[0]` (960, 460), and drop: must commit `radivine.copy`, NOT `radivine.properties`
        ctrl.handle_platform_events(vec![
            PlatformEvent::DndPosition {
                at: PhysPoint::new(960, 460),
            },
            PlatformEvent::DndDrop,
        ]);
        assert_eq!(
            ctrl.status().last_committed_action.as_deref(),
            Some("radivine.copy")
        );
        assert!(ctrl.active_session_settings().is_none());
        assert_eq!(ctrl.status().scale, 2.0);
    }

    #[test]
    fn deferred_payload_pins_action_id_and_rejects_when_ineligible_or_reordered() {
        let (mock, plat_state) = MockPlatform::new();
        let cfg = AppConfig::default();
        let renderer = Renderer::new(cfg.resolve_theme(), AssetSource::Embedded).expect("renderer");
        let registry = Arc::new(builtin_registry());
        let mut ctrl = SessionController::new(Box::new(mock), renderer, registry, cfg);

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let real_dir = temp_dir.path().join("folder_payload");
        std::fs::create_dir_all(&real_dir).expect("create dir");
        let actions_path = temp_dir.path().join("actions.toml");
        // In `radivine.custom` submenu (`[3]` when Convert is hidden for `.txt`):
        // `custom.text_only` is visible for provisional `.txt` file at `[3, 0]`,
        // while `custom.any_item` is at `[3, 1]` for `.txt` but moves to `[3, 0]` (or `[2, 0]`) for a directory!
        std::fs::write(
            &actions_path,
            r#"
version = 1

[[action]]
id = "custom.text_only"
label = "Text Only"
exec = ["true"]
[action.when]
extensions = ["txt"]

[[action]]
id = "custom.any_item"
label = "Any Item"
exec = ["true"]

[menu]
order = ["radivine.custom", "radivine.copy", "radivine.properties"]
hide = ["radivine.preview", "radivine.convert"]
"#,
        )
        .expect("write actions.toml");
        ctrl.load_actions_from_path(&actions_path);

        // Start drag and force provisional menu (source offers text/uri-list but refuses pre-drop conversion)
        ctrl.handle_platform_events(vec![
            PlatformEvent::DragArmed {
                source: SourceInfo {
                    app: Some("nemo".to_string()),
                    pid: Some(200),
                },
            },
            PlatformEvent::Trigger {
                at: PhysPoint::new(960, 540),
            },
            PlatformEvent::DndEnter {
                types: vec!["text/uri-list".to_string()],
            },
            PlatformEvent::DndPosition {
                at: PhysPoint::new(960, 460),
            },
            PlatformEvent::Payload {
                mime: "text/uri-list".to_string(),
                result: Err(radivine_platform::PayloadError::Timeout),
            },
            PlatformEvent::Payload {
                mime: "text/uri-list".to_string(),
                result: Err(radivine_platform::PayloadError::Timeout),
            },
        ]);
        assert_eq!(ctrl.status().session_state, "Active");
        assert!(ctrl.context_is_provisional);
        assert_eq!(
            ctrl.action_index.get(&ItemPath::from_slice(&[0, 0])),
            Some(&ActionId::from("custom.text_only"))
        );

        // Open submenu `[0]` (`Custom`) by dwelling on top segment (960, 460), then move to child `[0, 0]` and drop
        ctrl.on_timer(Instant::now() + Duration::from_millis(200));
        let child_pt = ctrl
            .wheel_session
            .as_ref()
            .and_then(|ws| ws.item_center(&ItemPath::from_slice(&[0, 0])))
            .expect("child [0, 0] center");
        ctrl.handle_platform_events(vec![
            PlatformEvent::DndPosition {
                at: PhysPoint::new(child_pt.x.round() as i32, child_pt.y.round() as i32),
            },
            PlatformEvent::DndDrop,
        ]);
        assert_eq!(ctrl.status().session_state, "Committing");
        assert_eq!(
            ctrl.pending_drop_selection(),
            Some(&PendingDropSelection {
                path: ItemPath::from_slice(&[0, 0]),
                action_id: Some(ActionId::from("custom.text_only")),
            })
        );

        // Deliver real payload for a directory (`real_dir`):
        // In the rebuilt menu for `real_dir`, `custom.text_only` is invisible and `custom.any_item`
        // now occupies `[0, 0]`. Controller MUST reject (`dnd_finish(false)`) and NEVER execute `custom.any_item`!
        let dir_uri = format!("file://{}\r\n", real_dir.display());
        ctrl.handle_platform_events(vec![PlatformEvent::Payload {
            mime: "text/uri-list".to_string(),
            result: Ok(dir_uri.into_bytes()),
        }]);
        assert_eq!(
            plat_state
                .lock()
                .expect("lock")
                .dnd_finishes
                .last()
                .copied(),
            Some(false)
        );
        assert!(ctrl.status().last_committed_action.is_none());
    }

    #[test]
    fn concentric_activation_fallback_and_display_change_guards() {
        let (mock, plat_state) = MockPlatform::new();
        let mut cfg = AppConfig::default();
        cfg.wheel.layout = "concentric".to_string();
        let renderer = Renderer::new(cfg.resolve_theme(), AssetSource::Embedded).expect("renderer");
        let registry = Arc::new(builtin_registry());
        let mut ctrl = SessionController::new(Box::new(mock), renderer, registry, cfg.clone());

        // 1. Summon Concentric Drag session on 1920x1080 translucent overlay
        ctrl.handle_platform_events(vec![
            PlatformEvent::DragArmed {
                source: SourceInfo {
                    app: Some("nemo".to_string()),
                    pid: Some(500),
                },
            },
            PlatformEvent::Trigger {
                at: PhysPoint::new(960, 500),
            },
            PlatformEvent::DndEnter {
                types: vec!["text/uri-list".to_string()],
            },
            PlatformEvent::DndPosition {
                at: PhysPoint::new(960, 500),
            },
            PlatformEvent::Payload {
                mime: "text/uri-list".to_string(),
                result: Ok(b"file:///tmp/demo.txt\r\n".to_vec()),
            },
        ]);
        let st_active = ctrl.status();
        assert_eq!(st_active.session_state, "Active");
        assert_eq!(st_active.requested_layout, "concentric");
        assert_eq!(st_active.active_layout, "concentric");
        assert_eq!(st_active.layout_fallback_reason, None);
        assert!(!st_active.layout_pending);

        // Move onto Copy segment (`[1]`, ~+90 deg -> x=960+84, y=500) to arm selection
        ctrl.handle_platform_events(vec![PlatformEvent::DndPosition {
            at: PhysPoint::new(1044, 500),
        }]);
        assert_eq!(
            plat_state.lock().expect("lock").dnd_replies.last().copied(),
            Some(true)
        );

        // 2. SettingsChanged (DPI scale 1.0 -> 1.5, compositor still active) is deferred until session end
        let prep_before = plat_state.lock().expect("lock").prepare_overlay_calls;
        plat_state.lock().expect("lock").scale = 1.5;
        ctrl.handle_platform_events(vec![PlatformEvent::SettingsChanged]);
        assert_eq!(
            plat_state.lock().expect("lock").prepare_overlay_calls,
            prep_before,
            "active Concentric session must not call prepare_overlay on SettingsChanged"
        );
        assert_eq!(ctrl.status().scale, 1.0);
        assert!(!ctrl.is_rejecting_until_drag_end());

        // 3. MonitorsChanged during active Concentric Drag session enters `reject_until_drag_end`
        ctrl.handle_platform_events(vec![PlatformEvent::MonitorsChanged]);
        assert!(ctrl.is_rejecting_until_drag_end());
        assert!(plat_state.lock().expect("lock").mapped);
        assert_eq!(
            plat_state.lock().expect("lock").dnd_replies.last().copied(),
            Some(false)
        );

        // Subsequent DndPosition and DndLeave(drag_ended: false) keep overlay mapped and reply false
        ctrl.handle_platform_events(vec![
            PlatformEvent::DndPosition {
                at: PhysPoint::new(1044, 500),
            },
            PlatformEvent::DndLeave { drag_ended: false },
        ]);
        assert_eq!(ctrl.status().session_state, "Active");
        assert!(plat_state.lock().expect("lock").mapped);
        assert_eq!(
            plat_state.lock().expect("lock").dnd_replies.last().copied(),
            Some(false)
        );

        // DndDrop while rejecting replies dnd_finish(false), unmaps, and adopts deferred scale 1.5
        ctrl.handle_platform_events(vec![PlatformEvent::DndDrop]);
        assert_eq!(
            plat_state
                .lock()
                .expect("lock")
                .dnd_finishes
                .last()
                .copied(),
            Some(false)
        );
        assert!(!plat_state.lock().expect("lock").mapped);
        assert!(!ctrl.is_rejecting_until_drag_end());
        assert_eq!(ctrl.status().scale, 1.5);
        assert!(ctrl.status().last_committed_action.is_none());

        // 4. InsufficientSpace fallback on 1280x720 @ 1.0x shows fallback caption and reason
        {
            let mut st = plat_state.lock().expect("lock");
            st.scale = 1.0;
            st.monitor_size = (1280, 720);
        }
        ctrl.summon_invoke(
            vec!["file:///tmp/demo.txt".to_string()],
            Some(PhysPoint::new(640, 360)),
        )
        .expect("summon_invoke 720p");
        let st_720p = ctrl.status();
        assert_eq!(st_720p.requested_layout, "concentric");
        assert_eq!(st_720p.active_layout, "branching");
        assert_eq!(
            st_720p.layout_fallback_reason.as_deref(),
            Some("insufficient_space")
        );
        let fallback_cap = ctrl
            .wheel_session
            .as_ref()
            .and_then(|ws| ws.scene().caption.as_ref())
            .expect("fallback caption on 720p");
        assert_eq!(
            fallback_cap.title.as_ref(),
            "Using Branching: nested rings need more space"
        );
        ctrl.dismiss_immediate();

        // 5. NoCompositor fallback (`translucent = false`) on 1920x1080 records `no_compositor`
        {
            let mut st = plat_state.lock().expect("lock");
            st.monitor_size = (1920, 1080);
            st.translucent = false;
        }
        ctrl.summon_invoke(
            vec!["file:///tmp/demo.txt".to_string()],
            Some(PhysPoint::new(960, 500)),
        )
        .expect("summon_invoke no compositor");
        let st_nocomp = ctrl.status();
        assert_eq!(st_nocomp.requested_layout, "concentric");
        assert_eq!(st_nocomp.active_layout, "branching");
        assert_eq!(
            st_nocomp.layout_fallback_reason.as_deref(),
            Some("no_compositor")
        );
        ctrl.dismiss_immediate();

        // 6. Invoke mode Concentric session cancels immediately on MonitorsChanged
        plat_state.lock().expect("lock").translucent = true;
        ctrl.summon_invoke(
            vec!["file:///tmp/demo.txt".to_string()],
            Some(PhysPoint::new(960, 500)),
        )
        .expect("summon_invoke concentric");
        assert_eq!(ctrl.status().active_layout, "concentric");
        ctrl.handle_platform_events(vec![PlatformEvent::MonitorsChanged]);
        assert!(!plat_state.lock().expect("lock").mapped);
    }

    trait TempDirPathExt {
        fn join_path_or_fallback(&self, name: &str) -> std::path::PathBuf;
    }

    impl TempDirPathExt for tempfile::TempDir {
        fn join_path_or_fallback(&self, name: &str) -> std::path::PathBuf {
            self.path().join(name)
        }
    }
}
