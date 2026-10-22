//! Desktop session state machine (`Idle`, `Summoning`, `Active`, `Committing`, `Dismissing`)
//! (`ARCHITECTURE.md` §3.3, `PLATFORM_INTEGRATION.md` §3.4, §3.12).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use smallvec::SmallVec;

use radivine_core::{
    ActionId, ActionIndex, ActionRegistry, BasicContext, Context, Dispatcher, InvocationMode,
    JobEvent, MenuLimits, SourceInfo as CoreSourceInfo,
};
use radivine_platform::{
    FrameRef as PlatFrameRef, Key as PlatKey, OverlayInfo, PhysPoint, Platform, PlatformEvent,
    RectPx as PlatRectPx, ShapePx,
};
use radivine_render::{RectPx as RenderRectPx, Renderer, SizePx};
use radivine_wheel::{
    DismissReason, InputMode, ItemPath, Millis, NavKey, Point, Shape, Viewport, WheelConfig,
    WheelEvent, WheelInput, WheelSession,
};

use crate::config::AppConfig;
use crate::ipc::DaemonStatus;

/// Maximum duration an active session may remain open without interaction (`60 s`).
const ACTIVE_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(60);

/// Idle delay before releasing shared-memory pixel buffers (`30 s`).
const IDLE_BUFFER_RELEASE_DELAY: Duration = Duration::from_secs(30);

/// Session lifecycle state (`ARCHITECTURE.md` §3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// Waiting for an Invoke command or drag trigger.
    Idle,
    /// Preparing and mapping the overlay window.
    Summoning,
    /// Overlay is mapped and interactive.
    Active,
    /// Unmapping overlay and dispatching the committed leaf action.
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
            Self::Summoning => "Summoning",
            Self::Active => "Active",
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
    /// Context item URIs at commit time.
    pub items: Vec<String>,
}

/// Orchestrates the platform backend, radial wheel engine, software renderer, and action dispatcher.
pub struct SessionController {
    platform: Box<dyn Platform>,
    renderer: Renderer,
    wheel_cfg: WheelConfig,
    wheel_session: Option<WheelSession>,
    action_index: ActionIndex,
    registry: Arc<ActionRegistry>,
    dispatcher: Dispatcher,
    config: AppConfig,
    state: SessionState,
    active_context: Option<Arc<Context>>,
    overlay_info: Option<OverlayInfo>,
    effective_scale: f32,
    invoke_count: u64,
    last_commit: Arc<Mutex<LastCommitRecord>>,
    session_epoch: Instant,
    last_interaction: Instant,
    next_frame_deadline: Option<Instant>,
    idle_release_deadline: Option<Instant>,
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

        let dispatcher = Dispatcher::new(
            Box::new(|ev: JobEvent| match ev {
                JobEvent::Finished {
                    job,
                    action,
                    result: Ok(_),
                } => {
                    tracing::info!(job_id = job.0, action_id = %action, "dispatched action completed");
                }
                JobEvent::Finished {
                    job,
                    action,
                    result: Err(err),
                } => {
                    tracing::warn!(job_id = job.0, action_id = %action, error = %err, "dispatched action failed");
                }
            }),
            4,
        );

        let now = Instant::now();
        Self {
            platform,
            renderer,
            wheel_cfg,
            wheel_session: None,
            action_index: ActionIndex::default(),
            registry,
            dispatcher,
            config,
            state: SessionState::Idle,
            active_context: None,
            overlay_info: None,
            effective_scale,
            invoke_count: 0,
            last_commit: Arc::new(Mutex::new(LastCommitRecord::default())),
            session_epoch: now,
            last_interaction: now,
            next_frame_deadline: None,
            idle_release_deadline: None,
            debug_overlay,
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

    /// Builds a [`DaemonStatus`] snapshot for `radivine status`.
    #[must_use]
    pub fn status(&self) -> DaemonStatus {
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
            last_committed_action: commit.action_id,
            last_committed_items: commit.items,
        }
    }

    /// Summons the radial wheel in Invoke mode at `explicit_pos` (or the current X11 pointer).
    pub fn summon_invoke(
        &mut self,
        uris: Vec<String>,
        explicit_pos: Option<PhysPoint>,
    ) -> anyhow::Result<()> {
        let t0 = Instant::now();
        if self.state != SessionState::Idle {
            self.dismiss_immediate();
        }

        self.state = SessionState::Summoning;
        self.idle_release_deadline = None;
        self.session_epoch = t0;
        self.last_interaction = t0;

        let pointer_phys = match explicit_pos {
            Some(pt) => pt,
            None => self.platform.pointer_position()?,
        };

        let info = self.platform.prepare_overlay(pointer_phys)?;
        let scale = self.config.effective_scale(info.scale);
        self.effective_scale = scale;

        // Convert root physical pointer to overlay-local logical coordinates
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
        let ctx = Arc::new(Context::from_basic(basic));

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

        // Apply input region before mapping
        self.sync_input_region()?;

        // Render and upload the first frame before mapping so the window appears already populated
        self.render_and_present(true)?;
        self.platform.map_overlay()?;

        if let Err(err) = self.platform.grab_input() {
            tracing::error!("failed to grab input in Invoke mode ({err}); aborting summon");
            self.platform.hide_overlay();
            self.state = SessionState::Idle;
            self.idle_release_deadline = Some(Instant::now() + IDLE_BUFFER_RELEASE_DELAY);
            anyhow::bail!("failed to grab X11 pointer/keyboard: {err}");
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

    /// Immediately ungrabs input, unmaps the overlay, and transitions to `Idle`.
    pub fn dismiss_immediate(&mut self) {
        self.state = SessionState::Dismissing;
        self.platform.ungrab_input();
        self.platform.hide_overlay();
        self.next_frame_deadline = None;
        self.state = SessionState::Idle;
        self.idle_release_deadline = Some(Instant::now() + IDLE_BUFFER_RELEASE_DELAY);
    }

    /// Handles a batch of [`PlatformEvent`]s emitted by the display server.
    pub fn handle_platform_events(&mut self, events: Vec<PlatformEvent>) {
        let now = Instant::now();
        for ev in events {
            match ev {
                PlatformEvent::PointerMoved { at } if self.state == SessionState::Active => {
                    self.last_interaction = now;
                    let pos = self.root_phys_to_logical(at);
                    self.feed_wheel_input(WheelInput::PointerMoved(pos), now);
                }
                PlatformEvent::Button {
                    button,
                    pressed,
                    at,
                } if self.state == SessionState::Active => {
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
                            // Secondary (right) click cancels Invoke mode immediately
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
                    self.state = SessionState::Idle;
                }
                _ => {}
            }
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

        if self.state == SessionState::Active {
            if let Some(ref ws) = self.wheel_session
                && let Some(dl_ms) = ws.next_deadline()
            {
                merge(Some(self.session_epoch + Duration::from_millis(dl_ms)));
            }
            merge(self.next_frame_deadline);
            merge(Some(self.last_interaction + ACTIVE_WATCHDOG_TIMEOUT));
        } else if self.state == SessionState::Idle {
            merge(self.idle_release_deadline);
        }

        earliest
    }

    /// Advances timers/animations due at `now`.
    pub fn on_timer(&mut self, now: Instant) {
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
                self.renderer.end_session();
                tracing::debug!("released idle SHM and renderer pixel buffers");
            }
            return;
        }

        if self.state != SessionState::Active {
            return;
        }

        // Check 60 s inactivity watchdog
        if now.duration_since(self.last_interaction) >= ACTIVE_WATCHDOG_TIMEOUT {
            tracing::warn!("active session hit 60 s inactivity watchdog; force-dismissing");
            let ms = self.elapsed_millis(now);
            if let Some(ref mut ws) = self.wheel_session {
                ws.dismiss(DismissReason::Timeout, ms);
            }
            self.dismiss_immediate();
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
            if self.state == SessionState::Active {
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
        if self.state == SessionState::Active {
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
        self.state = SessionState::Committing;
        // Immediately release input grabs and unmap overlay within < 5 ms (`AC-2.3`, `AC-2.4`)
        self.platform.ungrab_input();
        self.platform.hide_overlay();
        self.next_frame_deadline = None;

        let action_id: ActionId = self
            .action_index
            .get(path)
            .cloned()
            .unwrap_or_else(|| ActionId::from(format!("path:{path:?}")));

        let uris: Vec<String> = self
            .active_context
            .as_ref()
            .map(|ctx| ctx.basic.items.iter().map(|it| it.uri.clone()).collect())
            .unwrap_or_default();

        tracing::info!(
            action_id = %action_id,
            path = ?path,
            uris = ?uris,
            "committed radial wheel action"
        );

        if let Ok(mut guard) = self.last_commit.lock() {
            guard.action_id = Some(action_id.as_str().to_string());
            guard.items = uris;
        }

        if let Some(action) = self.registry.get(&action_id)
            && let Some(ref ctx) = self.active_context
        {
            self.dispatcher.dispatch(action, Arc::clone(ctx));
        }

        self.state = SessionState::Idle;
        self.idle_release_deadline = Some(now + IDLE_BUFFER_RELEASE_DELAY);
    }

    fn elapsed_millis(&self, now: Instant) -> Millis {
        now.saturating_duration_since(self.session_epoch)
            .as_millis() as Millis
    }

    fn schedule_next_frame_if_animating(&mut self, _now: Instant) {
        if self.state != SessionState::Active {
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
        if !self.config.input.passthrough {
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
