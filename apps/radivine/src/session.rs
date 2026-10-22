//! Desktop session state machine (`Idle`, `Armed`, `Summoning`, `Active`, `LeftGrace`,
//! `Committing`, `Dismissing`) (`ARCHITECTURE.md` §3.3, §5, `PLATFORM_INTEGRATION.md` §3.4–§3.12).

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use smallvec::SmallVec;

use radivine_actions::{call_desktop_notify, load_actions_file, registry_from_actions_config};
use radivine_core::{
    ActionId, ActionIndex, ActionRegistry, BasicContext, ClipOp, ClipboardRequest, Context,
    Dispatcher, Effect, Enablement, EnrichEvent, Enricher, FileFacts, FileKind, InvocationMode,
    JobEvent, MenuLimits, Outcome, SourceInfo as CoreSourceInfo, TextEncoding, TextVerdict,
};
use radivine_platform::{
    ClipOp as PlatClipOp, ClipboardContent, FrameRef as PlatFrameRef, Key as PlatKey, OverlayInfo,
    PhysPoint, Platform, PlatformEvent, RectPx as PlatRectPx, ShapePx,
};
use radivine_render::{RectPx as RenderRectPx, Renderer, SizePx};
use radivine_wheel::{
    DismissReason, DropTarget, InputMode, ItemPath, Millis, NavKey, Point, Shape, Viewport,
    WheelConfig, WheelEvent, WheelInput, WheelSession,
};

use crate::config::AppConfig;
use crate::ipc::{DaemonStatus, ItemStateSnapshot};

/// Maximum duration an active Invoke session may remain open without interaction (`60 s`).
const ACTIVE_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum duration any session may remain open (`120 s`).
const MAX_SESSION_DURATION: Duration = Duration::from_secs(120);

/// Idle delay before releasing shared-memory pixel buffers (`30 s`).
const IDLE_BUFFER_RELEASE_DELAY: Duration = Duration::from_secs(30);

/// Brief non-blocking window to collect fast local enrichment results on summon (`3 ms`).
const FAST_LOCAL_ENRICH_WINDOW: Duration = Duration::from_millis(3);

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

/// Orchestrates the platform backend, radial wheel engine, software renderer, context enricher, and action dispatcher.
pub struct SessionController {
    platform: Box<dyn Platform>,
    renderer: Renderer,
    wheel_cfg: WheelConfig,
    wheel_session: Option<WheelSession>,
    action_index: ActionIndex,
    registry: Arc<ActionRegistry>,
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
    pending_drop_commit: Option<ItemPath>,
    debug_overlay: bool,
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

        let (notify_tx, notify_rx) = mpsc::channel::<(String, String)>();
        let _ = thread::Builder::new()
            .name("radivine-notify".to_string())
            .spawn(move || {
                while let Ok((summary, body)) = notify_rx.recv() {
                    let suppress = std::env::var("RADIVINE_SUPPRESS_DESKTOP_NOTIFY")
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
            action_index: ActionIndex::default(),
            registry,
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
            debug_overlay,
        }
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
    pub fn reload_config(&mut self, config: AppConfig) {
        self.config = config;
        let reduced_motion = self
            .config
            .effective_reduced_motion(self.platform.reduced_motion());
        self.wheel_cfg = self.config.to_wheel_config(reduced_motion);
        self.renderer.set_theme(self.config.resolve_theme());
        self.effective_scale = self.config.effective_scale(self.platform.scale());
        tracing::info!(
            scale = self.effective_scale,
            passthrough = self.config.input.passthrough,
            "reloaded configuration"
        );
    }

    /// Loads or reloads `actions.toml` from `actions_path`.
    ///
    /// If `actions_path` is invalid or fails validation, logs and notifies the error with line
    /// information while preserving the previously active [`ActionRegistry`] (`AC-4.6`).
    pub fn load_actions_from_path(&mut self, actions_path: &Path) {
        match load_actions_file(actions_path) {
            Ok(actions_cfg) => match registry_from_actions_config(&actions_cfg) {
                Ok(new_reg) => {
                    self.registry = Arc::new(new_reg);
                    self.config_warnings.clone_from(&actions_cfg.warnings);
                    for warning in &actions_cfg.warnings {
                        tracing::warn!("{warning}");
                        self.emit_notification("Actions configuration warning", warning);
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
                    self.emit_notification("Invalid actions.toml", &msg);
                }
            },
            Err(err) => {
                let msg = err.to_string();
                tracing::warn!("{msg}");
                self.config_warnings = vec![msg.clone()];
                self.emit_notification("Invalid actions.toml", &msg);
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

        let (root_wheel_center_px, root_wheel_outer_radius_px) = self
            .wheel_session
            .as_ref()
            .and_then(|ws| ws.scene().wheels.first())
            .map_or((None, None), |rw| {
                (
                    Some((
                        rw.center.x * self.effective_scale,
                        rw.center.y * self.effective_scale,
                    )),
                    Some(rw.r_out * self.effective_scale),
                )
            });

        let commit = self
            .last_commit
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();

        DaemonStatus {
            pid: std::process::id(),
            overlay_window_id: self.platform.overlay_window_id(),
            session_state: self.state.as_str().to_string(),
            scale: self.effective_scale,
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
        }
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
        self.session_epoch = t0;
        self.last_interaction = t0;

        let pointer_phys = match explicit_pos {
            Some(pt) => pt,
            None => self.platform.pointer_position()?,
        };

        let info = self.platform.prepare_overlay(pointer_phys)?;
        let scale = self.config.effective_scale(info.scale);
        self.effective_scale = scale;

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

        let (menu_tree, action_index) = self
            .registry
            .build_menu(&ctx, MenuLimits::default())
            .ok_or_else(|| anyhow::anyhow!("no actions available for context"))?;

        let mut ws = WheelSession::new(
            Some(menu_tree),
            cursor_logical,
            viewport,
            InputMode::Pointer,
            &self.wheel_cfg,
            0,
        );
        if item_count > 1 {
            ws.set_root_badge(Some(item_count), 0);
        }

        self.renderer
            .begin_session(SizePx::new(info.size.0, info.size.1), scale);
        self.overlay_info = Some(info.clone());
        self.active_context = Some(ctx);
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

        let elapsed_us = t0.elapsed().as_micros();
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
        self.context_is_provisional = false;
        self.idle_release_deadline = None;
        self.leave_grace_deadline = None;
        self.pending_drop_commit = None;
        self.dnd_entered = false;
        self.offered_uri_list = false;
        self.retried_payload_after_position = false;
        self.last_dnd_pos = None;
        self.last_dnd_accept = None;
        self.session_epoch = t0;
        self.last_interaction = t0;

        let info = self.platform.prepare_overlay(pointer_phys)?;
        let scale = self.config.effective_scale(info.scale);
        self.effective_scale = scale;

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
        let ws = WheelSession::new(
            None,
            cursor_logical,
            viewport,
            InputMode::Drag,
            &self.wheel_cfg,
            0,
        );

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

        let elapsed_us = t0.elapsed().as_micros();
        tracing::info!(
            target: "radivine::summon",
            pointer_x = pointer_phys.x,
            pointer_y = pointer_phys.y,
            scale = scale,
            monitor = %info.monitor.name,
            source_app = ?self.drag_source.app,
            source_pid = ?self.drag_source.pid,
            passthrough = self.config.input.passthrough,
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
                        && self.config.trigger.toggle_off_on_repress
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
                        && self.config.trigger.hold_to_keep_open =>
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
                    self.last_interaction = now;
                    self.dnd_entered = true;
                    self.last_dnd_pos = Some(at);

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
                    } else if self.state == SessionState::Active {
                        // Pointer left the shaped wheel cluster (`passthrough = true`)
                        self.feed_wheel_input(WheelInput::PointerLeft, now);
                        self.state = SessionState::LeftGrace;
                        self.leave_grace_deadline =
                            Some(now + Duration::from_millis(self.config.wheel.leave_grace_ms));
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
                PlatformEvent::SettingsChanged | PlatformEvent::MonitorsChanged => {
                    self.effective_scale = self.config.effective_scale(self.platform.scale());
                    tracing::info!(
                        scale = self.effective_scale,
                        "updated display settings / monitors"
                    );
                }
                PlatformEvent::ConnectionLost => {
                    tracing::error!("X11 display server connection lost");
                    self.drag_armed = false;
                    self.active_mode = None;
                    self.state = SessionState::Idle;
                }
                _ => {}
            }
        }
    }

    fn handle_dnd_drop(&mut self, now: Instant) {
        self.drain_async_events(now);
        if self.state != SessionState::Active {
            // `ARCHITECTURE.md` §5: A DndDrop arriving in any state other than Active is answered with dnd_finish(false).
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
            self.platform.dnd_finish(true);
            self.commit_path(&path, now);
        } else {
            // Fetch the real `text/uri-list` payload at drop time before dispatching
            self.state = SessionState::Committing;
            self.pending_drop_commit = Some(path);
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

                let Some((menu_tree, action_index)) =
                    self.registry.build_menu(&ctx, MenuLimits::default())
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
                self.action_index = action_index;
                self.refresh_active_item_states();

                if self.state == SessionState::Committing {
                    if let Some(path) = self.pending_drop_commit.take() {
                        self.platform.dnd_finish(true);
                        self.commit_path(&path, now);
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
                    ws.set_menu(menu_tree, ms);
                    if item_count > 1 {
                        ws.set_root_badge(Some(item_count), ms);
                    }
                    if let Some(pos) = initial_pos {
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
        let Some((menu_tree, action_index)) = self.registry.build_menu(&ctx, MenuLimits::default())
        else {
            self.dismiss_drag_session(DismissReason::Error, now);
            return;
        };

        self.active_context = Some(ctx);
        self.context_is_provisional = true;
        self.action_index = action_index;
        self.refresh_active_item_states();

        let ms = self.elapsed_millis(now);
        let initial_pos = self.last_dnd_pos.map(|at| self.root_phys_to_logical(at));
        let mut wheel_events = Vec::new();
        if let Some(ref mut ws) = self.wheel_session {
            ws.set_menu(menu_tree, ms);
            if let Some(pos) = initial_pos {
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
        let accept = self.state == SessionState::Active
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
        for ev in events {
            match ev {
                WheelEvent::HoverChanged(_)
                | WheelEvent::SubmenuOpened(_)
                | WheelEvent::SubmenuClosed(_) => {}
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

    fn commit_path(&mut self, path: &ItemPath, now: Instant) {
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

        let action_id: ActionId = self
            .action_index
            .get(path)
            .cloned()
            .unwrap_or_else(|| ActionId::from(format!("path:{path:?}")));

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

        if let Some(action) = self.registry.get(&action_id)
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

    fn process_async_event(&mut self, ev: AsyncControllerEvent, _now: Instant) {
        match ev {
            AsyncControllerEvent::Enrich(EnrichEvent::Updated {
                session_id,
                context,
                ..
            }) => {
                if session_id == self.active_enrich_session_id {
                    self.apply_enrichment_context(context);
                }
            }
            AsyncControllerEvent::Job(job_ev) => {
                self.handle_job_event(job_ev);
            }
        }
    }

    fn apply_enrichment_context(&mut self, new_ctx: Arc<Context>) {
        self.active_context = Some(Arc::clone(&new_ctx));

        let updates = self
            .registry
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

        let mut any_changed = false;
        if let Some(ref mut ws) = self.wheel_session {
            for u in updates {
                let reason_str = u.disabled_reason.as_ref().map(ToString::to_string);
                let unchanged = self.active_item_states.iter().any(|prev| {
                    prev.path.as_slice() == u.path.as_slice()
                        && prev.enabled == u.enabled
                        && prev.disabled_reason == reason_str
                });
                if !unchanged {
                    ws.set_enabled(&u.path, u.enabled, u.disabled_reason);
                    any_changed = true;
                }
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
        let updates = self.registry.enablement_updates(ctx, &self.action_index);
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
                self.emit_notification(&format!("Action failed ({action})"), &err_msg);
            }
        }
    }

    fn emit_notification(&mut self, summary: &str, body: &str) {
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
        let _ = self.notify_tx.send((summary.to_string(), body.to_string()));
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
        let scale = self.effective_scale.max(0.5);
        Point::new(
            ((root_pt.x - ox) as f32) / scale,
            ((root_pt.y - oy) as f32) / scale,
        )
    }

    fn sync_input_region(&mut self) -> anyhow::Result<()> {
        // In Invoke mode, or when `input.passthrough == false` (safe default per ADR-016 / AGENTS.md §2.4),
        // the entire overlay captures input (`None`).
        if self.active_mode == Some(InvocationMode::Invoke) || !self.config.input.passthrough {
            self.platform.set_input_region(None)?;
            return Ok(());
        }
        let Some(ref ws) = self.wheel_session else {
            return Ok(());
        };
        let scale = self.effective_scale;
        let shapes = ws.input_region();
        let mut shapes_px: Vec<ShapePx> = Vec::with_capacity(shapes.len());
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
        self.platform.set_input_region(Some(&shapes_px))?;
        Ok(())
    }

    fn render_and_present(&mut self, force_full: bool) -> anyhow::Result<()> {
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
        Ok(())
    }
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

    #[derive(Default)]
    struct MockPlatformState {
        mapped: bool,
        grabbed: bool,
        dnd_replies: Vec<bool>,
        dnd_finishes: Vec<bool>,
        requested_payloads: Vec<String>,
        input_region_is_shaped: bool,
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
            1.0
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
            Ok(OverlayInfo {
                origin: PhysPoint::new(0, 0),
                size: (1920, 1080),
                scale: 1.0,
                monitor: self.monitors[0].clone(),
                translucent: true,
            })
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
}
