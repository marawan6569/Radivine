//! Navigation state machine and session lifecycle (`WHEEL_ENGINE_DESIGN.md` §7, §8, §9).

use std::sync::Arc;

use smallvec::SmallVec;

use crate::anim::{Ease, Millis, Tween, timings};
use crate::concentric::{ConcentricPlan, plan_concentric};
use crate::config::{
    InvokeSelect, LayoutFallback, LayoutSelection, SubmenuOpen, WheelConfig, WheelLayout,
};
use crate::geometry::{
    Point, SegmentSpan, Viewport, anchor_corridor_polygon, child_corridor_polygon,
    child_segment_spans, circle_tangent_points, point_in_triangle, root_segment_spans,
    segment_content_radius,
};
use crate::hit::{WheelHitGeometry, Zone, hit_test};
use crate::layout::{
    Placement, compute_stem_points, nominal_radii, place_caption, place_child, place_root,
    scaled_inner_radius,
};
use crate::menu::{IconKey, ItemPath, MenuNode, MenuTree, NodeKind};
use crate::scene::{
    CaptionVisual, HubVisual, Scene, SegmentVisual, Shape, StemVisual, WheelRole, WheelVisual,
};

/// Input operating mode of a [`WheelSession`] (`WHEEL_ENGINE_DESIGN.md` §1, §9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    /// Drag mode: pointer positions come from `XdndPosition`; selection happens via `XdndDrop`.
    Drag,
    /// Pointer/Invoke mode: real pointer and keyboard events; selection via click/release/keyboard.
    Pointer,
}

/// Logical input event fed into [`WheelSession::handle`] (`WHEEL_ENGINE_DESIGN.md` §9).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WheelInput {
    /// Pointer moved to `Point` in overlay-local logical pixels.
    PointerMoved(Point),
    /// Pointer left the overlay input region (e.g. `XdndLeave`).
    PointerLeft,
    /// Pointer entered the overlay input region.
    PointerEntered,
    /// Primary mouse button pressed (Pointer mode).
    Press,
    /// Primary mouse button released (Pointer mode).
    Release,
    /// Keyboard navigation key pressed (Pointer mode).
    Nav(NavKey),
}

/// Logical keyboard navigation keys for Pointer/Invoke mode (`WHEEL_ENGINE_DESIGN.md` §7.6, §9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavKey {
    /// Next item clockwise (`Right`, `Down`, `Tab`).
    Next,
    /// Previous item counter-clockwise (`Left`, `Up`, `Shift+Tab`).
    Prev,
    /// Activate focused item (`Enter`, `Space`).
    Activate,
    /// Close active child wheel (`Backspace`).
    Back,
    /// Cancel session (`Escape`).
    Cancel,
    /// Direct 1-based item activation (`1`..=`9`).
    Digit(u8),
}

/// Output event emitted by [`WheelSession::handle`] and [`WheelSession::tick`] (`WHEEL_ENGINE_DESIGN.md` §9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WheelEvent {
    /// Hovered/focused item path changed.
    HoverChanged(Option<ItemPath>),
    /// A submenu child wheel was opened at `ItemPath`.
    SubmenuOpened(ItemPath),
    /// A submenu child wheel at `ItemPath` was closed.
    SubmenuClosed(ItemPath),
    /// An executable leaf action was committed in Pointer/Keyboard mode.
    Commit(ItemPath),
    /// The user requested cancellation in Pointer/Keyboard mode (click outside, root hub, or `Esc`).
    CancelRequested,
    /// The set of interactive shapes in [`WheelSession::input_region`] changed.
    InputRegionChanged,
    /// The dismiss/commit animation completed; the host may unmap the overlay.
    Finished,
}

/// Current XDND drop target status in Drag mode (`WHEEL_ENGINE_DESIGN.md` §6.4, §9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropTarget {
    /// Drop will execute the action at `ItemPath`.
    Accept(ItemPath),
    /// Drop is rejected at the current position/state.
    Reject,
}

/// Reason a [`WheelSession`] is being dismissed (`WHEEL_ENGINE_DESIGN.md` §9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DismissReason {
    /// Action committed.
    Committed,
    /// User cancelled (`Esc`, click outside, root hub).
    Cancelled,
    /// Source drag ended or left past grace timeout.
    DragEnded,
    /// User toggled the wheel off via modifier re-press or release.
    Toggled,
    /// Watchdog or session timeout expired.
    Timeout,
    /// Unrecoverable platform/protocol error.
    Error,
}

/// High-level lifecycle phase of a [`WheelSession`] (`WHEEL_ENGINE_DESIGN.md` §8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPhase {
    /// Root wheel appear animation is in progress; input is already accepted.
    Appearing,
    /// Root wheel is fully open; normal interactive state.
    Open,
    /// Commit flash animation (`90 ms`) is playing; inputs ignored.
    Committing,
    /// Dismiss fade-out animation is playing; inputs ignored.
    Dismissing,
    /// Dismiss animation completed; session is finished.
    Finished,
}

/// Snapshot of internal session state for testing and debug overlays (`WHEEL_ENGINE_DESIGN.md` §16).
#[derive(Clone, Debug)]
pub struct DebugState {
    /// Requested and effective layout selection for this session.
    pub layout_selection: LayoutSelection,
    /// Current session lifecycle phase.
    pub phase: SessionPhase,
    /// Whether the pointer has moved past `activation_slop` since summon.
    pub moved_past_slop: bool,
    /// Raw hit-test zone at the last pointer position.
    pub raw_zone: Zone,
    /// Currently hovered/focused item path.
    pub hovered_item: Option<ItemPath>,
    /// Open wheel paths from root (`[]`) to deepest active child.
    pub open_chain: SmallVec<[ItemPath; 6]>,
    /// Hit-test geometry of all open non-retracting wheels.
    pub wheels: SmallVec<[WheelHitGeometry; 6]>,
    /// Active intent-protection triangle `(apex, t1, t2)` and deadline, if any.
    pub intent_triangle: Option<([Point; 3], Millis)>,
    /// Pending submenu dwell deadline, if any.
    pub dwell_deadline: Option<Millis>,
    /// Pending hub-close deadline, if any.
    pub hub_close_deadline: Option<Millis>,
}

#[derive(Clone, Debug)]
struct SegmentState {
    a0: f32,
    a1: f32,
    highlight: Tween,
    pop: Tween,
    enabled: bool,
    has_children: bool,
    label: Arc<str>,
    icon: Option<IconKey>,
}

#[derive(Clone, Debug)]
struct StemState {
    p0: Point,
    ctrl: Point,
    p1: Point,
    progress: Tween,
    opacity: Tween,
}

#[derive(Clone, Debug)]
struct WheelInstance {
    /// Path of the submenu node represented by this wheel (`[]` for root).
    path: ItemPath,
    /// Segment index in the parent wheel that opened this wheel (`0` for root).
    parent_item: u8,
    /// Computed placement.
    placement: Placement,
    /// Inner hub radius in logical pixels.
    r_in: f32,
    /// Outer ring radius in logical pixels.
    r_out: f32,
    /// Transform origin (`anchor` for root, bridge point for child).
    origin: Point,
    /// Scale tween.
    scale: Tween,
    /// Opacity tween.
    opacity: Tween,
    /// Current role (`Active` or `Ancestor`).
    role: WheelRole,
    /// Optional bridge span (present on child wheels).
    bridge: Option<SegmentSpan>,
    /// Hub icon and optional count badge.
    hub: HubVisual,
    /// Segment states in clockwise order.
    segments: SmallVec<[SegmentState; 12]>,
    /// Optional connecting stem from the parent wheel.
    stem: Option<StemState>,
    /// Precomputed hit geometry for this wheel.
    hit_geom: WheelHitGeometry,
}

#[derive(Clone, Copy, Debug)]
struct DwellTimer {
    wheel: u8,
    item: u8,
    deadline: Millis,
}

#[derive(Clone, Copy, Debug)]
struct HubCloseTimer {
    wheel: u8,
    deadline: Millis,
}

#[derive(Clone, Copy, Debug)]
struct IntentProtection {
    /// Wheel index `w` whose child `w + 1` is being protected.
    wheel: u8,
    /// Segment index `k` on wheel `w` that opened child `w + 1`.
    open_item: u8,
    /// Last pointer position inside `Item(w, k)` (`p_exit`).
    p_exit: Point,
    /// First tangent point on child `w + 1`'s `hit_radius` circle.
    tangent_a: Point,
    /// Second tangent point on child `w + 1`'s `hit_radius` circle.
    tangent_b: Point,
    /// Child `w + 1`'s center.
    child_center: Point,
    /// Distance from `p_exit` to `child_center`.
    exit_dist_to_child: f32,
    /// Expiration timestamp (`t_exit + grace_ms`).
    deadline: Millis,
}

/// Layout-specific session state (`ARCHITECTURE.md` §A2).
///
/// Uses closed enum dispatch rather than runtime strategy trait objects (`dyn WheelLayoutStrategy`),
/// keeping menu ownership, session lifecycle, and output contracts unified while isolating
/// layout-specific geometry, hit testing, and navigation transitions.
#[derive(Clone, Debug)]
enum LayoutState {
    /// Classic adjacent-wheel branching / vine layout.
    Branching,
    /// Concentric / nested full-circle annular layout (populated in Milestone N2).
    #[allow(dead_code)]
    Concentric(ConcentricState),
}

/// Nested-specific session state (`ARCHITECTURE.md` §A2, §A3).
#[derive(Clone, Debug)]
struct ConcentricState {
    plan: ConcentricPlan,
}

/// Deterministic, platform-independent radial wheel session state machine (`WHEEL_ENGINE_DESIGN.md` §9).
pub struct WheelSession {
    cfg: WheelConfig,
    layout_selection: LayoutSelection,
    layout_state: LayoutState,
    menu: Option<MenuTree>,
    has_hovered: bool,
    anchor: Point,
    summon_point: Point,
    last_pointer: Point,
    moved_past_slop: bool,
    viewport: Viewport,
    mode: InputMode,
    phase: SessionPhase,
    appear_done_at: Millis,
    commit_flash_end: Option<Millis>,
    dismiss_end: Option<Millis>,
    finished_emitted: bool,

    raw_zone: Zone,
    hovered_item: Option<ItemPath>,
    pressed_zone: Option<Zone>,
    pressed_item: Option<ItemPath>,
    last_inside_item_pos: Option<(u8, u8, Point)>,

    dwell: Option<DwellTimer>,
    hub_close: Option<HubCloseTimer>,
    intent_protection: Option<IntentProtection>,

    wheels: SmallVec<[WheelInstance; 6]>,
    retracting: SmallVec<[WheelInstance; 4]>,
    hit_cache: SmallVec<[WheelHitGeometry; 6]>,

    caption_opacity: Tween,
    scene: Scene,
    input_shapes: Vec<Shape>,
    root_badge: Option<u32>,
    default_child_hub_icon: IconKey,
    animating_flag: bool,
}

impl WheelSession {
    /// Creates a new [`WheelSession`] at `anchor` inside `viewport`.
    ///
    /// If `menu` is `None`, a hub-only placeholder root wheel is shown until [`Self::set_menu`] is called.
    #[must_use]
    pub fn new(
        menu: Option<MenuTree>,
        anchor: Point,
        viewport: Viewport,
        mode: InputMode,
        raw_cfg: &WheelConfig,
        now: Millis,
    ) -> Self {
        let cfg = raw_cfg.validated();
        let (layout_selection, layout_state) =
            select_initial_layout(raw_cfg, &cfg, anchor, viewport);
        let sanitized_menu = menu.map(|mut m| {
            m.sanitize(&cfg);
            m
        });

        let (r_in_root, r_out_root) = nominal_radii(0, &cfg);
        let (root_center, root_origin, appear_dur, scale_tween, opacity_tween, is_concentric) =
            match &layout_state {
                LayoutState::Branching => {
                    let center = place_root(anchor, r_out_root, viewport);
                    let dur = if cfg.reduced_motion {
                        timings::REDUCED_MOTION_FADE_MS
                    } else {
                        timings::ROOT_APPEAR_MS
                    };
                    let scale = if cfg.reduced_motion {
                        Tween::constant(1.0)
                    } else {
                        Tween::new(0.86, 1.0, now, timings::ROOT_APPEAR_MS, Ease::OutCubic)
                    };
                    let opacity = Tween::new(0.0, 1.0, now, dur, Ease::OutCubic);
                    (center, anchor, dur, scale, opacity, false)
                }
                LayoutState::Concentric(state) => {
                    let center = state.plan.center;
                    let dur = if cfg.reduced_motion {
                        timings::REDUCED_MOTION_FADE_MS
                    } else {
                        timings::CHILD_EMERGE_MS
                    };
                    let scale = Tween::constant(1.0);
                    let opacity = Tween::new(0.35, 1.0, now, dur, Ease::OutCubic);
                    (center, center, dur, scale, opacity, true)
                }
            };

        let mut root_wheel = WheelInstance {
            path: ItemPath::root(),
            parent_item: 0,
            placement: Placement {
                center: root_center,
                r_out: r_out_root,
                attach_angle: 0.0,
                fallback: false,
            },
            r_in: r_in_root,
            r_out: r_out_root,
            origin: root_origin,
            scale: scale_tween,
            opacity: opacity_tween,
            role: WheelRole::Active,
            bridge: None,
            hub: HubVisual {
                icon: sanitized_menu
                    .as_ref()
                    .and_then(|m| m.root.icon.clone())
                    .or_else(|| Some(IconKey::from("lucide:file"))),
                badge: None,
            },
            segments: SmallVec::new(),
            stem: None,
            hit_geom: WheelHitGeometry {
                index: 0,
                parent_index: None,
                center: root_center,
                r_in: r_in_root,
                r_out: r_out_root,
                hit_radius: r_out_root + cfg.hit_margin,
                bridge: None,
                spans: SmallVec::new(),
                corridor: None,
            },
        };

        let mut stagger_extra: u16 = 0;
        if let Some(ref m) = sanitized_menu
            && let Some(children) = m.root.children()
        {
            populate_root_segments(&mut root_wheel, children, &cfg);
            if !cfg.reduced_motion && !is_concentric {
                stagger_extra = ((children.len() as u16).saturating_sub(1)
                    * timings::SEGMENT_STAGGER_MS)
                    .min(timings::SEGMENT_STAGGER_MAX_MS);
            }
        }

        let mut wheels: SmallVec<[WheelInstance; 6]> = SmallVec::new();
        wheels.push(root_wheel);

        let scene = Scene::default();
        let input_shapes = Vec::with_capacity(24);
        let mut hit_cache: SmallVec<[WheelHitGeometry; 6]> = SmallVec::new();
        hit_cache.push(wheels[0].hit_geom.clone());

        let mut session = Self {
            appear_done_at: now + u64::from(appear_dur + stagger_extra),
            cfg,
            layout_selection,
            layout_state,
            menu: sanitized_menu,
            has_hovered: false,
            anchor,
            summon_point: anchor,
            last_pointer: anchor,
            moved_past_slop: false,
            viewport,
            mode,
            phase: SessionPhase::Appearing,
            commit_flash_end: None,
            dismiss_end: None,
            finished_emitted: false,
            raw_zone: Zone::Outside,
            hovered_item: None,
            pressed_zone: None,
            pressed_item: None,
            last_inside_item_pos: None,
            dwell: None,
            hub_close: None,
            intent_protection: None,
            wheels,
            retracting: SmallVec::new(),
            hit_cache,
            caption_opacity: Tween::constant(0.0),
            scene,
            input_shapes,
            root_badge: None,
            default_child_hub_icon: IconKey::from("lucide:chevron-left"),
            animating_flag: true,
        };

        session.rebuild_input_region();
        session.rebuild_scene(now);
        session
    }

    /// Returns the requested and effective [`LayoutSelection`] for this session (`ARCHITECTURE.md` §A2, §A3).
    #[inline]
    #[must_use]
    pub fn layout_selection(&self) -> LayoutSelection {
        self.layout_selection
    }

    /// Records a platform-level startup fallback reason (such as [`LayoutFallback::NoCompositor`])
    /// when a requested [`WheelLayout::Concentric`] session is forced to run as [`WheelLayout::Branching`]
    /// before mapping (`ARCHITECTURE.md` §A7).
    pub fn record_layout_fallback(&mut self, reason: LayoutFallback) {
        self.layout_selection = LayoutSelection::fallback_to_branching(reason);
        if matches!(self.layout_state, LayoutState::Concentric(_)) {
            self.layout_state = LayoutState::Branching;
            let now = self.wheels.first().map_or(0, |w| w.opacity.start);
            if let Some(root) = self.wheels.first_mut() {
                let root_center = place_root(self.anchor, root.r_out, self.viewport);
                root.placement.center = root_center;
                root.origin = self.anchor;
                root.hit_geom.center = root_center;
            }
            self.sync_hit_cache();
            self.rebuild_input_region();
            self.rebuild_scene(now);
        }
    }

    /// Sets an optional item count badge (`> 1`) displayed on the root wheel's center hub.
    pub fn set_root_badge(&mut self, badge: Option<u32>, now: Millis) {
        self.root_badge = badge.filter(|&b| b > 1);
        if let Some(root) = self.wheels.first_mut() {
            root.hub.badge = self.root_badge;
        }
        self.rebuild_scene(now);
    }

    /// Supplies the [`MenuTree`] after session creation (only allowed once, before any item is hovered).
    pub fn set_menu(&mut self, menu: MenuTree, now: Millis) {
        self.set_menu_at_pointer(menu, self.last_pointer, now);
    }

    /// Supplies the [`MenuTree`] after session creation along with the host's last observed pointer
    /// position (`ARCHITECTURE.md` §A3).
    ///
    /// Only allowed once, before any item is hovered. For [`WheelLayout::Branching`], behavior is
    /// identical to [`Self::set_menu`].
    pub fn set_menu_at_pointer(&mut self, mut menu: MenuTree, _pointer: Point, now: Millis) {
        if self.menu.is_some() || self.has_hovered {
            debug_assert!(
                false,
                "WheelSession::set_menu called after menu was already set or hovered"
            );
            return;
        }
        menu.sanitize(&self.cfg);
        match self.layout_state {
            LayoutState::Branching | LayoutState::Concentric(_) => {
                if let Some(root_wheel) = self.wheels.first_mut() {
                    if let Some(icon) = menu.root.icon.clone() {
                        root_wheel.hub.icon = Some(icon);
                    }
                    if let Some(children) = menu.root.children() {
                        populate_root_segments(root_wheel, children, &self.cfg);
                    }
                }
            }
        }
        self.menu = Some(menu);
        self.sync_hit_cache();
        self.rebuild_input_region();
        self.rebuild_scene(now);
    }

    /// Processes a single [`WheelInput`] at timestamp `now`, appending any resulting [`WheelEvent`]s to `out`.
    pub fn handle(&mut self, input: WheelInput, now: Millis, out: &mut Vec<WheelEvent>) {
        if !matches!(self.phase, SessionPhase::Appearing | SessionPhase::Open) {
            return;
        }

        match input {
            WheelInput::PointerMoved(p) => self.handle_pointer_moved(p, now, out),
            WheelInput::PointerLeft => {
                self.dwell = None;
                self.hub_close = None;
                self.intent_protection = None;
                self.raw_zone = Zone::Outside;
                self.set_hovered_item(None, now, out);
            }
            WheelInput::PointerEntered => {}
            WheelInput::Press => {
                if self.mode == InputMode::Pointer {
                    self.pressed_zone = Some(self.raw_zone);
                    self.pressed_item = self.hovered_item.clone();
                }
            }
            WheelInput::Release => {
                if self.mode == InputMode::Pointer {
                    self.handle_pointer_release(now, out);
                }
            }
            WheelInput::Nav(nav_key) => {
                if self.mode == InputMode::Pointer {
                    self.handle_nav_key(nav_key, now, out);
                }
            }
        }

        self.rebuild_scene(now);
    }

    /// Advances timers and animations to timestamp `now`, appending any resulting [`WheelEvent`]s to `out`.
    pub fn tick(&mut self, now: Millis, out: &mut Vec<WheelEvent>) {
        if self.phase == SessionPhase::Finished {
            return;
        }

        if self.phase == SessionPhase::Appearing && now >= self.appear_done_at {
            self.phase = SessionPhase::Open;
        }

        // Handle commit flash -> dismiss fade transition
        if self.phase == SessionPhase::Committing
            && let Some(flash_end) = self.commit_flash_end
            && now >= flash_end
        {
            self.commit_flash_end = None;
            let fade_dur = if self.cfg.reduced_motion {
                timings::REDUCED_MOTION_FADE_MS
            } else {
                timings::COMMIT_FADE_MS
            };
            self.start_cluster_dismiss(0.94, fade_dur, now);
        }

        // Handle dismiss completion
        if self.phase == SessionPhase::Dismissing
            && let Some(end) = self.dismiss_end
            && now >= end
        {
            self.phase = SessionPhase::Finished;
            if !self.finished_emitted {
                self.finished_emitted = true;
                out.push(WheelEvent::Finished);
            }
            self.rebuild_scene(now);
            return;
        }

        // Interactive hover-intent timers only run in Appearing / Open
        if matches!(self.phase, SessionPhase::Appearing | SessionPhase::Open) {
            match self.layout_state {
                LayoutState::Branching | LayoutState::Concentric(_) => {
                    self.tick_interactive_branching(now, out);
                }
            }
        }

        // Prune finished retracting wheels
        self.retracting.retain(|w| !w.opacity.done(now));

        self.rebuild_scene(now);
    }

    /// Returns the current XDND drop target status (`WHEEL_ENGINE_DESIGN.md` §6.4).
    #[must_use]
    pub fn drop_target(&self) -> DropTarget {
        match self.layout_state {
            LayoutState::Branching | LayoutState::Concentric(_) => self.drop_target_branching(),
        }
    }

    /// Commits a drop in Drag mode (`WHEEL_ENGINE_DESIGN.md` §6.4, §7.5, §9).
    ///
    /// Always re-evaluates [`Self::drop_target`]; if `Accept(path)`, starts the commit animation and returns `Some(path)`.
    pub fn commit_drop(&mut self, now: Millis) -> Option<ItemPath> {
        let DropTarget::Accept(path) = self.drop_target() else {
            return None;
        };
        self.start_commit_animation(&path, now);
        self.input_shapes.clear();
        self.rebuild_scene(now);
        Some(path)
    }

    /// Dynamically updates the `enabled` status and `disabled_reason` of the node at `path` (`WHEEL_ENGINE_DESIGN.md` §9).
    ///
    /// Never changes layout or `description`.
    pub fn set_enabled(
        &mut self,
        path: &ItemPath,
        enabled: bool,
        disabled_reason: Option<Arc<str>>,
    ) {
        let now = self.wheels.first().map_or(0, |w| w.opacity.start);
        self.set_enabled_at(path, enabled, disabled_reason, now);
    }

    /// Dynamically updates the `enabled` status and `disabled_reason` of the node at `path` at
    /// explicit timestamp `now` (`ARCHITECTURE.md` §A3).
    ///
    /// Never changes layout or `description`.
    pub fn set_enabled_at(
        &mut self,
        path: &ItemPath,
        enabled: bool,
        disabled_reason: Option<Arc<str>>,
        now: Millis,
    ) {
        if let Some(ref mut menu) = self.menu
            && let Some(node) = menu.node_mut(path)
        {
            node.enabled = enabled;
            node.disabled_reason = disabled_reason;
        }

        match self.layout_state {
            LayoutState::Branching | LayoutState::Concentric(_) => {
                // Update any currently open wheel segment matching `path`
                if !path.is_empty() {
                    let parent_len = path.len() - 1;
                    let seg_idx = path.as_slice()[parent_len] as usize;
                    if let Some(wheel) = self.wheels.get_mut(parent_len)
                        && wheel.path.as_slice() == &path.as_slice()[..parent_len]
                        && let Some(seg) = wheel.segments.get_mut(seg_idx)
                    {
                        seg.enabled = enabled;
                        if !enabled {
                            seg.highlight = Tween::constant(0.0);
                            seg.pop = Tween::constant(0.0);
                        }
                    }
                }

                // If a disabled item had a pending dwell timer, cancel it
                if !enabled
                    && let Some(dwell) = self.dwell
                    && let Some(w) = self.wheels.get(dwell.wheel as usize)
                    && w.path.child(dwell.item) == *path
                {
                    self.dwell = None;
                }
            }
        }

        self.rebuild_scene(now);
    }

    /// Suspends interactive selection and navigation timers for an active concentric session
    /// while preserving scene geometry and input capture (`ARCHITECTURE.md` §A3, §A7).
    ///
    /// Nested-only guard; a no-op for [`WheelLayout::Branching`].
    pub fn suspend_interaction(&mut self, _now: Millis) {
        match self.layout_state {
            LayoutState::Branching | LayoutState::Concentric(_) => {}
        }
    }

    /// Starts the dismiss animation for `reason` (`WHEEL_ENGINE_DESIGN.md` §8.1, §9).
    ///
    /// Immediately clears [`Self::input_region`].
    pub fn dismiss(&mut self, _reason: DismissReason, now: Millis) {
        if matches!(
            self.phase,
            SessionPhase::Dismissing | SessionPhase::Finished
        ) {
            return;
        }
        self.dwell = None;
        self.hub_close = None;
        self.intent_protection = None;
        self.input_shapes.clear();

        let dur = if self.cfg.reduced_motion {
            timings::REDUCED_MOTION_FADE_MS
        } else {
            timings::CANCEL_DISMISS_MS
        };
        self.start_cluster_dismiss(0.96, dur, now);
        self.rebuild_scene(now);
    }

    /// Returns a reference to the current [`Scene`] (`WHEEL_ENGINE_DESIGN.md` §9, §12).
    #[inline]
    #[must_use]
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// Returns the slice of interactive [`Shape`]s in overlay-local logical pixels (`WHEEL_ENGINE_DESIGN.md` §6.3, §9).
    #[inline]
    #[must_use]
    pub fn input_region(&self) -> &[Shape] {
        &self.input_shapes
    }

    /// Returns the earliest pending hover-intent timer deadline (`dwell`, `grace`, or `hub_close`), if any.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Millis> {
        if !matches!(self.phase, SessionPhase::Appearing | SessionPhase::Open) {
            return None;
        }
        match self.layout_state {
            LayoutState::Branching | LayoutState::Concentric(_) => self.next_deadline_branching(),
        }
    }

    /// Returns `true` if any wheel, stem, segment, or session tween is currently active (`WHEEL_ENGINE_DESIGN.md` §11.3).
    #[must_use]
    pub fn is_animating(&self) -> bool {
        matches!(
            self.phase,
            SessionPhase::Appearing | SessionPhase::Committing | SessionPhase::Dismissing
        ) || !self.retracting.is_empty()
            || self.animating_flag
    }

    /// Returns `true` once the dismiss animation has finished (`WHEEL_ENGINE_DESIGN.md` §8.1, §9).
    #[inline]
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.phase == SessionPhase::Finished
    }

    /// Returns a diagnostic snapshot of internal state for tests and debug overlays (`WHEEL_ENGINE_DESIGN.md` §16).
    #[must_use]
    pub fn debug_state(&self) -> DebugState {
        let mut open_chain = SmallVec::new();
        for w in &self.wheels {
            open_chain.push(w.path.clone());
        }
        let intent_triangle = self
            .intent_protection
            .map(|ip| ([ip.p_exit, ip.tangent_a, ip.tangent_b], ip.deadline));
        DebugState {
            layout_selection: self.layout_selection,
            phase: self.phase,
            moved_past_slop: self.moved_past_slop,
            raw_zone: self.raw_zone,
            hovered_item: self.hovered_item.clone(),
            open_chain,
            wheels: self.hit_cache.clone(),
            intent_triangle,
            dwell_deadline: self.dwell.map(|d| d.deadline),
            hub_close_deadline: self.hub_close.map(|h| h.deadline),
        }
    }

    /// Returns the center point in overlay-local logical pixels of the segment at `path`, if open.
    #[must_use]
    pub fn item_center(&self, path: &ItemPath) -> Option<Point> {
        if path.is_empty() {
            return None;
        }
        let parent_depth = path.len() - 1;
        let item_idx = path.as_slice()[parent_depth] as usize;
        let w = self.wheels.get(parent_depth)?;
        if w.path.as_slice() != &path.as_slice()[..parent_depth] {
            return None;
        }
        let span = w.hit_geom.spans.get(item_idx)?;
        let r_mid = match self.layout_state {
            LayoutState::Branching => (w.r_in + w.r_out) * 0.5,
            LayoutState::Concentric(_) => segment_content_radius(w.r_in, w.r_out),
        };
        Some(w.placement.center.offset_polar(r_mid, span.center))
    }

    /// Returns the hit-test geometry of the open wheel whose submenu path is `path` (`[]` for root).
    #[must_use]
    pub fn wheel_geometry(&self, path: &ItemPath) -> Option<&WheelHitGeometry> {
        self.wheels
            .iter()
            .find(|w| &w.path == path)
            .map(|w| &w.hit_geom)
    }

    // --- Private helpers ---

    fn handle_pointer_moved(&mut self, p: Point, now: Millis, out: &mut Vec<WheelEvent>) {
        match self.layout_state {
            LayoutState::Branching | LayoutState::Concentric(_) => {
                self.handle_pointer_moved_branching(p, now, out);
            }
        }
    }

    fn handle_pointer_moved_branching(&mut self, p: Point, now: Millis, out: &mut Vec<WheelEvent>) {
        if !self.moved_past_slop {
            if p.distance(self.summon_point) < self.cfg.activation_slop {
                self.last_pointer = p;
                return;
            }
            self.moved_past_slop = true;
        }

        let prev_pointer = self.last_pointer;
        self.last_pointer = p;

        let hysteresis_rad = self.cfg.hysteresis_deg.to_radians();
        let zone = hit_test(p, &self.hit_cache, self.raw_zone, hysteresis_rad);
        let prev_zone = self.raw_zone;
        self.raw_zone = zone;

        match zone {
            Zone::Corridor { child } | Zone::Bridge { wheel: child } => {
                self.dwell = None;
                self.hub_close = None;
                self.intent_protection = None;
                let c_idx = child as usize;
                if c_idx > 0 && c_idx < self.wheels.len() {
                    // If there were deeper wheels below `child`, keep `child` open and keep its parent item highlighted
                    let parent_item_path = self.wheels[c_idx].path.clone();
                    self.set_hovered_item(Some(parent_item_path), now, out);
                }
            }
            Zone::Empty { .. } | Zone::Outside => {
                self.dwell = None;
                self.hub_close = None;
                self.intent_protection = None;
                self.set_hovered_item(None, now, out);
            }
            Zone::Hub { wheel } => {
                self.dwell = None;
                self.intent_protection = None;
                self.set_hovered_item(None, now, out);
                let w_idx = wheel as usize;
                if w_idx + 1 < self.wheels.len() {
                    let already_for_wheel = self.hub_close.is_some_and(|h| h.wheel == wheel);
                    if !already_for_wheel {
                        if self.cfg.hub_close_ms == 0 {
                            self.truncate_after(wheel, now, out);
                        } else {
                            self.hub_close = Some(HubCloseTimer {
                                wheel,
                                deadline: now + u64::from(self.cfg.hub_close_ms),
                            });
                        }
                    }
                } else {
                    self.hub_close = None;
                }
            }
            Zone::Item { wheel, index } => {
                self.hub_close = None;
                let w_idx = wheel as usize;
                if w_idx >= self.wheels.len() {
                    return;
                }

                // Does wheel `w` currently have an open child `C = w + 1`?
                if w_idx + 1 < self.wheels.len() {
                    let open_item = self.wheels[w_idx + 1].parent_item;
                    if index == open_item {
                        // Pointer is inside the parent item that opened the chain below `w`: keep chain!
                        self.intent_protection = None;
                        self.dwell = None;
                        self.last_inside_item_pos = Some((wheel, index, p));
                        let item_path = self.wheels[w_idx].path.child(index);
                        self.set_hovered_item(Some(item_path), now, out);
                        return;
                    }

                    // Pointer is in a sibling segment `index != open_item` of wheel `w`.
                    // Intent protection applies only to `w`'s immediate child (`w_idx + 2 == self.wheels.len()`).
                    if w_idx + 2 == self.wheels.len() {
                        // Establish intent protection if we just exited `Item(wheel, open_item)`
                        if self.intent_protection.is_none()
                            && prev_zone
                                == (Zone::Item {
                                    wheel,
                                    index: open_item,
                                })
                        {
                            let p_exit = self
                                .last_inside_item_pos
                                .filter(|&(lw, li, _)| lw == wheel && li == open_item)
                                .map_or(prev_pointer, |(_, _, pos)| pos);
                            let child_wheel = &self.wheels[w_idx + 1];
                            let c_center = child_wheel.placement.center;
                            let c_hit_r = child_wheel.hit_geom.hit_radius;
                            let (ta, tb) = circle_tangent_points(p_exit, c_center, c_hit_r);
                            self.intent_protection = Some(IntentProtection {
                                wheel,
                                open_item,
                                p_exit,
                                tangent_a: ta,
                                tangent_b: tb,
                                child_center: c_center,
                                exit_dist_to_child: p_exit.distance(c_center),
                                deadline: now + u64::from(self.cfg.grace_ms),
                            });
                        }

                        if let Some(ip) = self.intent_protection {
                            let in_grace = now < ip.deadline;
                            let in_tri =
                                point_in_triangle(p, ip.p_exit, ip.tangent_a, ip.tangent_b);
                            let closer = p.distance(ip.child_center) < ip.exit_dist_to_child;
                            if ip.wheel == wheel && in_grace && in_tri && closer {
                                // Keep C open and keep `open_item` hovered!
                                let protected_path = self.wheels[w_idx].path.child(ip.open_item);
                                self.set_hovered_item(Some(protected_path), now, out);
                                return;
                            }
                        }
                    }

                    // Intent protection did not hold (or deeper chain existed): truncate below `w`!
                    self.intent_protection = None;
                    self.truncate_after(wheel, now, out);
                }

                // Now wheel `w` is the deepest active wheel and `Item(wheel, index)` is hovered.
                self.intent_protection = None;
                self.last_inside_item_pos = Some((wheel, index, p));
                let item_path = self.wheels[w_idx].path.child(index);
                let hover_changed = self.hovered_item.as_ref() != Some(&item_path);
                self.set_hovered_item(Some(item_path), now, out);

                // Check §7.2 submenu opening rules (Outer band / Dwell)
                if self.is_enabled_submenu(wheel, index) {
                    if self.should_open_by_outer_band(wheel, prev_pointer, p) {
                        self.dwell = None;
                        self.open_child_wheel(wheel, index, false, now, out);
                    } else if hover_changed {
                        self.schedule_dwell_if_applicable(wheel, index, now);
                    }
                } else {
                    self.dwell = None;
                }
            }
        }
    }

    fn handle_pointer_release(&mut self, now: Millis, out: &mut Vec<WheelEvent>) {
        match self.layout_state {
            LayoutState::Branching | LayoutState::Concentric(_) => {
                self.handle_pointer_release_branching(now, out);
            }
        }
    }

    fn handle_pointer_release_branching(&mut self, now: Millis, out: &mut Vec<WheelEvent>) {
        let press_zone = self.pressed_zone.take();
        let press_item = self.pressed_item.take();

        match self.raw_zone {
            Zone::Outside => {
                out.push(WheelEvent::CancelRequested);
                return;
            }
            Zone::Hub { wheel: 0 } => {
                if matches!(press_zone, Some(Zone::Hub { wheel: 0 }) | None) {
                    out.push(WheelEvent::CancelRequested);
                }
                return;
            }
            Zone::Hub { wheel } => {
                // Clicking a child hub closes that child (§7.4)
                if wheel >= 1 && (wheel as usize) < self.wheels.len() {
                    let parent_w = wheel - 1;
                    let parent_item = self.wheels[wheel as usize].parent_item;
                    self.truncate_after(parent_w, now, out);
                    let parent_path = self.wheels[parent_w as usize].path.child(parent_item);
                    self.set_hovered_item(Some(parent_path), now, out);
                }
                return;
            }
            _ => {}
        }

        let Some(path) = self.hovered_item.clone() else {
            return;
        };
        let Some(node_kind_and_enabled) = self
            .menu
            .as_ref()
            .and_then(|m| m.node(&path))
            .map(|n| (n.enabled, matches!(n.kind, NodeKind::Submenu { .. })))
        else {
            return;
        };
        let (enabled, is_submenu) = node_kind_and_enabled;
        if !enabled {
            return;
        }

        if is_submenu {
            // Explicit click on a submenu opens it if not already open (§7.2)
            let w_idx = path.len() - 1;
            let item_idx = path.as_slice()[w_idx];
            let already_open = self.wheels.get(w_idx + 1).is_some_and(|cw| cw.path == path);
            if !already_open {
                self.dwell = None;
                self.open_child_wheel(w_idx as u8, item_idx, false, now, out);
            }
            return;
        }

        // Enabled leaf item
        let can_commit = match self.cfg.invoke_select {
            InvokeSelect::Release => true,
            InvokeSelect::Click => {
                press_item.as_ref() == Some(&path) || matches!(press_zone, Some(Zone::Hub { .. }))
            }
        };
        if can_commit {
            self.start_commit_animation(&path, now);
            self.input_shapes.clear();
            out.push(WheelEvent::InputRegionChanged);
            out.push(WheelEvent::Commit(path));
        }
    }

    fn handle_nav_key(&mut self, nav_key: NavKey, now: Millis, out: &mut Vec<WheelEvent>) {
        match self.layout_state {
            LayoutState::Branching | LayoutState::Concentric(_) => {
                self.handle_nav_key_branching(nav_key, now, out);
            }
        }
    }

    fn handle_nav_key_branching(
        &mut self,
        nav_key: NavKey,
        now: Millis,
        out: &mut Vec<WheelEvent>,
    ) {
        if self.wheels.is_empty() {
            return;
        }
        let active_w = self.wheels.len() - 1;
        let n = self.wheels[active_w].segments.len();

        let cur_idx: Option<usize> = self.hovered_item.as_ref().and_then(|path| {
            if path.len() == active_w + 1
                && path.as_slice()[..active_w] == *self.wheels[active_w].path.as_slice()
            {
                Some(path.as_slice()[active_w] as usize)
            } else {
                None
            }
        });

        match nav_key {
            NavKey::Next => {
                if n == 0 {
                    return;
                }
                let next = cur_idx.map_or(0, |i| (i + 1) % n);
                self.dwell = None;
                self.intent_protection = None;
                self.hub_close = None;
                let path = self.wheels[active_w].path.child(next as u8);
                self.set_hovered_item(Some(path), now, out);
            }
            NavKey::Prev => {
                if n == 0 {
                    return;
                }
                let prev = cur_idx.map_or(n - 1, |i| (i + n - 1) % n);
                self.dwell = None;
                self.intent_protection = None;
                self.hub_close = None;
                let path = self.wheels[active_w].path.child(prev as u8);
                self.set_hovered_item(Some(path), now, out);
            }
            NavKey::Activate => {
                if let Some(i) = cur_idx {
                    self.activate_keyboard_item(active_w as u8, i as u8, now, out);
                }
            }
            NavKey::Back => {
                if active_w >= 1 {
                    let parent_w = (active_w - 1) as u8;
                    let parent_item = self.wheels[active_w].parent_item;
                    self.dwell = None;
                    self.intent_protection = None;
                    self.hub_close = None;
                    self.truncate_after(parent_w, now, out);
                    let parent_path = self.wheels[parent_w as usize].path.child(parent_item);
                    self.set_hovered_item(Some(parent_path), now, out);
                }
            }
            NavKey::Digit(d) => {
                if d >= 1 && (d as usize) <= n {
                    let idx = d - 1;
                    let path = self.wheels[active_w].path.child(idx);
                    self.set_hovered_item(Some(path), now, out);
                    self.activate_keyboard_item(active_w as u8, idx, now, out);
                }
            }
            NavKey::Cancel => {
                out.push(WheelEvent::CancelRequested);
            }
        }
    }

    fn activate_keyboard_item(
        &mut self,
        wheel: u8,
        item: u8,
        now: Millis,
        out: &mut Vec<WheelEvent>,
    ) {
        let path = self.wheels[wheel as usize].path.child(item);
        let Some(info) = self.menu.as_ref().and_then(|m| m.node(&path)).map(|node| {
            (
                node.enabled,
                matches!(&node.kind, NodeKind::Submenu { children, .. } if !children.is_empty()),
            )
        }) else {
            return;
        };
        let (enabled, is_submenu) = info;
        if !enabled {
            return;
        }
        if is_submenu {
            self.dwell = None;
            self.open_child_wheel(wheel, item, true, now, out);
        } else {
            self.start_commit_animation(&path, now);
            self.input_shapes.clear();
            out.push(WheelEvent::InputRegionChanged);
            out.push(WheelEvent::Commit(path));
        }
    }

    fn is_enabled_submenu(&self, wheel: u8, item: u8) -> bool {
        let Some(w) = self.wheels.get(wheel as usize) else {
            return false;
        };
        let path = w.path.child(item);
        let Some(ref menu) = self.menu else {
            return false;
        };
        let Some(node) = menu.node(&path) else {
            return false;
        };
        node.enabled && node.has_children()
    }

    fn should_open_by_outer_band(&self, wheel: u8, prev_p: Point, cur_p: Point) -> bool {
        if !matches!(
            self.cfg.submenu_open,
            SubmenuOpen::Band | SubmenuOpen::DwellAndBand
        ) {
            return false;
        }
        let Some(w) = self.wheels.get(wheel as usize) else {
            return false;
        };
        let d_now = cur_p.distance(w.placement.center);
        let d_prev = prev_p.distance(w.placement.center);
        let threshold = w.r_in + self.cfg.band * (w.r_out - w.r_in);
        d_now >= threshold && d_now > d_prev
    }

    fn schedule_dwell_if_applicable(&mut self, wheel: u8, item: u8, now: Millis) {
        if !matches!(
            self.cfg.submenu_open,
            SubmenuOpen::Dwell | SubmenuOpen::DwellAndBand
        ) {
            return;
        }
        if !self.is_enabled_submenu(wheel, item) {
            return;
        }
        self.dwell = Some(DwellTimer {
            wheel,
            item,
            deadline: now + u64::from(self.cfg.dwell_ms),
        });
    }

    fn open_child_wheel(
        &mut self,
        parent_wheel: u8,
        item: u8,
        focus_first_child: bool,
        now: Millis,
        out: &mut Vec<WheelEvent>,
    ) {
        let p_idx = parent_wheel as usize;
        if p_idx >= self.wheels.len() {
            return;
        }

        // First close any existing open child of `parent_wheel`
        if p_idx + 1 < self.wheels.len() {
            self.truncate_after(parent_wheel, now, out);
        }

        let child_path = self.wheels[p_idx].path.child(item);
        let Some(ref menu) = self.menu else {
            return;
        };
        let Some(submenu_node) = menu.node(&child_path) else {
            return;
        };
        if !submenu_node.enabled {
            return;
        }
        let Some(children) = submenu_node.children() else {
            return;
        };
        if children.is_empty() {
            return;
        }

        let level = p_idx + 1;
        let (r_in_nom, r_out_nom) = nominal_radii(level, &self.cfg);
        let c_p = self.wheels[p_idx].placement.center;
        let r_p = self.wheels[p_idx].r_out;
        let parent_span = self.wheels[p_idx].hit_geom.spans[item as usize];
        let phi = parent_span.center;

        let mut other_wheels: SmallVec<[(Point, f32); 6]> = SmallVec::new();
        for w in &self.wheels[..p_idx] {
            other_wheels.push((w.placement.center, w.r_out));
        }

        let placement = place_child(
            c_p,
            r_p,
            phi,
            r_out_nom,
            &other_wheels,
            self.viewport,
            &self.cfg,
        );
        let r_out = placement.r_out;
        let r_in = scaled_inner_radius(r_out, r_in_nom, r_out_nom);

        let beta = placement.center.angle_to(c_p);
        let (bridge_span, spans) =
            child_segment_spans(children.len(), beta, self.cfg.bridge_angle_deg.to_radians());

        let (stem_p0, stem_ctrl, stem_p1) = compute_stem_points(
            c_p,
            r_p,
            phi,
            placement.center,
            r_out,
            placement.attach_angle,
            self.cfg.child_gap,
        );

        let hit_radius = r_out + self.cfg.hit_margin;
        let corridor = child_corridor_polygon(
            c_p,
            r_p,
            parent_span,
            self.cfg.segment_gap,
            placement.center,
            hit_radius,
        );

        let (scale_tween, opacity_tween, stem_progress, stem_opacity) = if self.cfg.reduced_motion {
            (
                Tween::constant(1.0),
                Tween::new(
                    0.0,
                    1.0,
                    now,
                    timings::REDUCED_MOTION_FADE_MS,
                    Ease::OutCubic,
                ),
                Tween::constant(1.0),
                Tween::new(
                    0.0,
                    1.0,
                    now,
                    timings::REDUCED_MOTION_FADE_MS,
                    Ease::OutCubic,
                ),
            )
        } else {
            let emerge_start = now + u64::from(timings::CHILD_EMERGE_DELAY_MS);
            (
                Tween::new(
                    0.6,
                    1.0,
                    emerge_start,
                    timings::CHILD_EMERGE_MS,
                    Ease::OutCubic,
                ),
                Tween::new(
                    0.0,
                    1.0,
                    emerge_start,
                    timings::CHILD_EMERGE_MS,
                    Ease::OutCubic,
                ),
                Tween::new(0.0, 1.0, now, timings::STEM_GROW_MS, Ease::OutCubic),
                Tween::constant(1.0),
            )
        };

        // Dim parent wheel to Ancestor role (0.72 opacity)
        let dim_dur = if self.cfg.reduced_motion {
            timings::REDUCED_MOTION_FADE_MS
        } else {
            timings::ANCESTOR_DIM_MS
        };
        for ancestor in &mut self.wheels {
            ancestor.role = WheelRole::Ancestor;
            ancestor
                .opacity
                .retarget(0.72, now, dim_dur, Ease::OutCubic);
        }

        let mut segments = SmallVec::new();
        for (k, child_node) in children.iter().enumerate() {
            let sp = spans[k];
            segments.push(SegmentState {
                a0: sp.a0,
                a1: sp.a1,
                highlight: Tween::constant(0.0),
                pop: Tween::constant(0.0),
                enabled: child_node.enabled,
                has_children: child_node.has_children(),
                label: child_node.label.clone(),
                icon: child_node.icon.clone(),
            });
        }

        let hit_geom = WheelHitGeometry {
            index: level as u8,
            parent_index: Some(parent_wheel),
            center: placement.center,
            r_in,
            r_out,
            hit_radius,
            bridge: Some(bridge_span),
            spans,
            corridor: Some(corridor),
        };

        let child_instance = WheelInstance {
            path: child_path.clone(),
            parent_item: item,
            placement,
            r_in,
            r_out,
            origin: stem_p1,
            scale: scale_tween,
            opacity: opacity_tween,
            role: WheelRole::Active,
            bridge: Some(bridge_span),
            hub: HubVisual {
                icon: submenu_node
                    .icon
                    .clone()
                    .or_else(|| Some(self.default_child_hub_icon.clone())),
                badge: None,
            },
            segments,
            stem: Some(StemState {
                p0: stem_p0,
                ctrl: stem_ctrl,
                p1: stem_p1,
                progress: stem_progress,
                opacity: stem_opacity,
            }),
            hit_geom,
        };

        self.wheels.push(child_instance);
        self.sync_hit_cache();
        self.rebuild_input_region();

        out.push(WheelEvent::SubmenuOpened(child_path.clone()));
        out.push(WheelEvent::InputRegionChanged);

        if focus_first_child {
            let first_child_path = child_path.child(0);
            self.set_hovered_item(Some(first_child_path), now, out);
        }
    }

    fn truncate_after(&mut self, keep_wheel: u8, now: Millis, out: &mut Vec<WheelEvent>) {
        let target_len = (keep_wheel as usize) + 1;
        if self.wheels.len() <= target_len {
            return;
        }

        while self.wheels.len() > target_len {
            let Some(mut closed) = self.wheels.pop() else {
                break;
            };
            let closed_path = closed.path.clone();
            if self.cfg.reduced_motion {
                closed
                    .opacity
                    .retarget(0.0, now, timings::REDUCED_MOTION_FADE_MS, Ease::OutCubic);
                if let Some(ref mut stem) = closed.stem {
                    stem.opacity.retarget(
                        0.0,
                        now,
                        timings::REDUCED_MOTION_FADE_MS,
                        Ease::OutCubic,
                    );
                }
            } else {
                closed
                    .scale
                    .retarget(0.6, now, timings::CHILD_RETRACT_MS, Ease::InOutCubic);
                closed
                    .opacity
                    .retarget(0.0, now, timings::CHILD_RETRACT_MS, Ease::InOutCubic);
                if let Some(ref mut stem) = closed.stem {
                    stem.progress
                        .retarget(0.0, now, timings::STEM_RETRACT_MS, Ease::InOutCubic);
                    stem.opacity
                        .retarget(0.0, now, timings::CHILD_RETRACT_MS, Ease::InOutCubic);
                }
            }
            if self.retracting.len() < 4 {
                self.retracting.push(closed);
            }
            out.push(WheelEvent::SubmenuClosed(closed_path));
        }

        if let Some(active) = self.wheels.last_mut() {
            active.role = WheelRole::Active;
            let dur = if self.cfg.reduced_motion {
                timings::REDUCED_MOTION_FADE_MS
            } else {
                timings::ANCESTOR_DIM_MS
            };
            active.opacity.retarget(1.0, now, dur, Ease::OutCubic);
        }

        self.sync_hit_cache();
        self.rebuild_input_region();
        out.push(WheelEvent::InputRegionChanged);
    }

    fn set_hovered_item(
        &mut self,
        new_hover: Option<ItemPath>,
        now: Millis,
        out: &mut Vec<WheelEvent>,
    ) {
        if self.hovered_item == new_hover {
            return;
        }

        let in_dur = if self.cfg.reduced_motion {
            timings::REDUCED_MOTION_FADE_MS
        } else {
            timings::HOVER_IN_MS
        };
        let out_dur = if self.cfg.reduced_motion {
            timings::REDUCED_MOTION_FADE_MS
        } else {
            timings::HOVER_OUT_MS
        };

        // Retarget old hovered segment out
        if let Some(ref old_path) = self.hovered_item
            && !old_path.is_empty()
        {
            let w_idx = old_path.len() - 1;
            let s_idx = old_path.as_slice()[w_idx] as usize;
            if let Some(w) = self.wheels.get_mut(w_idx)
                && w.path.as_slice() == &old_path.as_slice()[..w_idx]
                && let Some(seg) = w.segments.get_mut(s_idx)
            {
                seg.highlight.retarget(0.0, now, out_dur, Ease::OutCubic);
                seg.pop.retarget(0.0, now, out_dur, Ease::OutCubic);
            }
        }

        // Retarget new hovered segment in (only if enabled)
        if let Some(ref new_path) = new_hover
            && !new_path.is_empty()
        {
            self.has_hovered = true;
            let w_idx = new_path.len() - 1;
            let s_idx = new_path.as_slice()[w_idx] as usize;
            if let Some(w) = self.wheels.get_mut(w_idx)
                && w.path.as_slice() == &new_path.as_slice()[..w_idx]
                && let Some(seg) = w.segments.get_mut(s_idx)
                && seg.enabled
            {
                seg.highlight.retarget(1.0, now, in_dur, Ease::OutCubic);
                let target_pop = if self.cfg.reduced_motion { 0.0 } else { 4.0 };
                seg.pop.retarget(target_pop, now, in_dur, Ease::OutCubic);
            }
            self.caption_opacity = Tween::new(
                0.0,
                1.0,
                now,
                if self.cfg.reduced_motion {
                    timings::REDUCED_MOTION_FADE_MS
                } else {
                    timings::CAPTION_FADE_MS
                },
                Ease::Linear,
            );
        } else {
            self.caption_opacity.retarget(
                0.0,
                now,
                if self.cfg.reduced_motion {
                    timings::REDUCED_MOTION_FADE_MS
                } else {
                    timings::CAPTION_FADE_MS
                },
                Ease::Linear,
            );
        }

        self.hovered_item = new_hover.clone();
        out.push(WheelEvent::HoverChanged(new_hover));
    }

    fn start_commit_animation(&mut self, committed_path: &ItemPath, now: Millis) {
        self.dwell = None;
        self.hub_close = None;
        self.intent_protection = None;
        self.phase = SessionPhase::Committing;

        let flash_dur = if self.cfg.reduced_motion {
            timings::REDUCED_MOTION_FADE_MS
        } else {
            timings::COMMIT_FLASH_MS
        };
        self.commit_flash_end = Some(now + u64::from(flash_dur));

        if !committed_path.is_empty() {
            let w_idx = committed_path.len() - 1;
            let s_idx = committed_path.as_slice()[w_idx] as usize;
            if let Some(w) = self.wheels.get_mut(w_idx)
                && w.path.as_slice() == &committed_path.as_slice()[..w_idx]
                && let Some(seg) = w.segments.get_mut(s_idx)
            {
                seg.highlight.retarget(1.25, now, flash_dur, Ease::OutCubic);
            }
        }
    }

    fn start_cluster_dismiss(&mut self, target_scale: f32, dur: u16, now: Millis) {
        self.phase = SessionPhase::Dismissing;
        self.dismiss_end = Some(now + u64::from(dur));
        let scale_to = if self.cfg.reduced_motion {
            1.0
        } else {
            target_scale
        };
        for w in &mut self.wheels {
            w.opacity.retarget(0.0, now, dur, Ease::OutCubic);
            w.scale.retarget(scale_to, now, dur, Ease::OutCubic);
            if let Some(ref mut stem) = w.stem {
                stem.opacity.retarget(0.0, now, dur, Ease::OutCubic);
            }
        }
    }

    fn tick_interactive_branching(&mut self, now: Millis, out: &mut Vec<WheelEvent>) {
        // 1. Intent-triangle grace expiration
        if let Some(intent) = self.intent_protection
            && now >= intent.deadline
        {
            self.intent_protection = None;
            self.truncate_after(intent.wheel, now, out);
            // Re-evaluate hover at the last pointer position
            if let Zone::Item { wheel, index } = self.raw_zone
                && wheel == intent.wheel
            {
                let new_path = self.wheels[wheel as usize].path.child(index);
                self.set_hovered_item(Some(new_path), now, out);
                self.schedule_dwell_if_applicable(wheel, index, now);
            } else {
                self.set_hovered_item(None, now, out);
            }
        }

        // 2. Hub-close timer expiration
        if let Some(hc) = self.hub_close
            && now >= hc.deadline
        {
            self.hub_close = None;
            if self.raw_zone == (Zone::Hub { wheel: hc.wheel })
                && (hc.wheel as usize) + 1 < self.wheels.len()
            {
                self.truncate_after(hc.wheel, now, out);
            }
        }

        // 3. Dwell submenu-open timer expiration
        if let Some(dwell) = self.dwell
            && now >= dwell.deadline
        {
            self.dwell = None;
            if (dwell.wheel as usize) < self.wheels.len() {
                let expected_path = self.wheels[dwell.wheel as usize].path.child(dwell.item);
                if self.hovered_item.as_ref() == Some(&expected_path) {
                    self.open_child_wheel(dwell.wheel, dwell.item, false, now, out);
                }
            }
        }
    }

    fn drop_target_branching(&self) -> DropTarget {
        if !self.moved_past_slop
            || !matches!(self.phase, SessionPhase::Appearing | SessionPhase::Open)
        {
            return DropTarget::Reject;
        }
        let Some(ref path) = self.hovered_item else {
            return DropTarget::Reject;
        };
        let Some(ref menu) = self.menu else {
            return DropTarget::Reject;
        };
        let Some(node) = menu.node(path) else {
            return DropTarget::Reject;
        };
        if !node.enabled {
            return DropTarget::Reject;
        }
        match &node.kind {
            NodeKind::Leaf => DropTarget::Accept(path.clone()),
            NodeKind::Submenu {
                children,
                default_child: Some(dc),
            } => {
                if let Some(child) = children.get(*dc)
                    && child.enabled
                    && matches!(child.kind, NodeKind::Leaf)
                {
                    DropTarget::Accept(path.child(*dc as u8))
                } else {
                    DropTarget::Reject
                }
            }
            NodeKind::Submenu {
                default_child: None,
                ..
            } => DropTarget::Reject,
        }
    }

    fn next_deadline_branching(&self) -> Option<Millis> {
        let mut earliest: Option<Millis> = None;
        for cand in [
            self.dwell.map(|d| d.deadline),
            self.hub_close.map(|h| h.deadline),
            self.intent_protection.map(|i| i.deadline),
        ]
        .into_iter()
        .flatten()
        {
            earliest = Some(earliest.map_or(cand, |e| e.min(cand)));
        }
        earliest
    }

    fn sync_hit_cache(&mut self) {
        self.hit_cache.clear();
        for w in &self.wheels {
            self.hit_cache.push(w.hit_geom.clone());
        }
    }

    fn rebuild_input_region(&mut self) {
        match self.layout_state {
            LayoutState::Branching | LayoutState::Concentric(_) => {
                self.rebuild_input_region_branching();
            }
        }
    }

    fn rebuild_input_region_branching(&mut self) {
        self.input_shapes.clear();
        if !matches!(self.phase, SessionPhase::Appearing | SessionPhase::Open) {
            return;
        }

        if let Some(root) = self.wheels.first() {
            // Always include anchor disc (24 lp) and, when displaced (> 0.5 lp), displacement corridor (§5.1, §6.3)
            let anchor_r = 24.0;
            self.input_shapes.push(Shape::Circle {
                c: self.anchor,
                r: anchor_r,
            });
            let root_hit_r = root.hit_geom.hit_radius;
            if self.anchor.distance(root.placement.center) > 0.5 {
                let corr = anchor_corridor_polygon(
                    self.anchor,
                    anchor_r,
                    root.placement.center,
                    root_hit_r,
                );
                self.input_shapes.push(Shape::Polygon(corr));
            }
        }

        for w in &self.wheels {
            self.input_shapes.push(Shape::Circle {
                c: w.placement.center,
                r: w.hit_geom.hit_radius,
            });
            if let Some(ref poly) = w.hit_geom.corridor {
                self.input_shapes.push(Shape::Polygon(poly.clone()));
            }
        }
    }

    fn rebuild_scene(&mut self, now: Millis) {
        match self.layout_state {
            LayoutState::Branching | LayoutState::Concentric(_) => {
                self.rebuild_scene_branching(now);
            }
        }
    }

    fn rebuild_scene_branching(&mut self, now: Millis) {
        self.scene.generation = self.scene.generation.wrapping_add(1);
        self.scene.stems.clear();
        self.scene.wheels.clear();

        let mut any_tween_active = false;

        // 1. Active stems
        let num_wheels = self.wheels.len();
        for w in &self.wheels {
            if let Some(ref stem) = w.stem {
                if !stem.progress.done(now) || !stem.opacity.done(now) {
                    any_tween_active = true;
                }
                self.scene.stems.push(StemVisual {
                    p0: stem.p0,
                    ctrl: stem.ctrl,
                    p1: stem.p1,
                    progress: stem.progress.value(now),
                    opacity: stem.opacity.value(now),
                    on_path: true,
                });
            }
        }

        // 2. Retracting stems
        for rw in &self.retracting {
            if let Some(ref stem) = rw.stem {
                if !stem.progress.done(now) || !stem.opacity.done(now) {
                    any_tween_active = true;
                }
                if self.scene.stems.len() < 6 {
                    self.scene.stems.push(StemVisual {
                        p0: stem.p0,
                        ctrl: stem.ctrl,
                        p1: stem.p1,
                        progress: stem.progress.value(now),
                        opacity: stem.opacity.value(now),
                        on_path: false,
                    });
                }
            }
        }

        // 3. Active wheels (root -> deepest)
        for (w_idx, w) in self.wheels.iter().enumerate() {
            if !w.scale.done(now) || !w.opacity.done(now) {
                any_tween_active = true;
            }
            let open_child_item = if w_idx + 1 < num_wheels {
                Some(self.wheels[w_idx + 1].parent_item as usize)
            } else {
                None
            };

            let mut seg_visuals = SmallVec::new();
            for (s_idx, seg) in w.segments.iter().enumerate() {
                if !seg.highlight.done(now) || !seg.pop.done(now) {
                    any_tween_active = true;
                }
                seg_visuals.push(SegmentVisual {
                    a0: seg.a0,
                    a1: seg.a1,
                    highlight: seg.highlight.value(now),
                    pop: seg.pop.value(now),
                    on_path: open_child_item == Some(s_idx),
                    enabled: seg.enabled,
                    has_children: seg.has_children,
                    label: seg.label.clone(),
                    icon: seg.icon.clone(),
                });
            }

            self.scene.wheels.push(WheelVisual {
                center: w.placement.center,
                r_in: w.r_in,
                r_out: w.r_out,
                scale: w.scale.value(now),
                origin: w.origin,
                opacity: w.opacity.value(now),
                role: w.role,
                bridge: w.bridge.map(|b| (b.a0, b.a1)),
                hub: w.hub.clone(),
                segments: seg_visuals,
            });
        }

        // 4. Retracting wheels
        for rw in &self.retracting {
            if !rw.scale.done(now) || !rw.opacity.done(now) {
                any_tween_active = true;
            }
            if self.scene.wheels.len() < 6 {
                let mut seg_visuals = SmallVec::new();
                for seg in &rw.segments {
                    seg_visuals.push(SegmentVisual {
                        a0: seg.a0,
                        a1: seg.a1,
                        highlight: seg.highlight.value(now),
                        pop: seg.pop.value(now),
                        on_path: false,
                        enabled: seg.enabled,
                        has_children: seg.has_children,
                        label: seg.label.clone(),
                        icon: seg.icon.clone(),
                    });
                }
                self.scene.wheels.push(WheelVisual {
                    center: rw.placement.center,
                    r_in: rw.r_in,
                    r_out: rw.r_out,
                    scale: rw.scale.value(now),
                    origin: rw.origin,
                    opacity: rw.opacity.value(now),
                    role: rw.role,
                    bridge: rw.bridge.map(|b| (b.a0, b.a1)),
                    hub: rw.hub.clone(),
                    segments: seg_visuals,
                });
            }
        }

        // 5. Caption pill on the active wheel when an item is hovered
        self.scene.caption = None;
        if matches!(self.phase, SessionPhase::Appearing | SessionPhase::Open)
            && let Some(ref hov_path) = self.hovered_item
            && let Some(ref menu) = self.menu
            && let Some(node) = menu.node(hov_path)
            && let Some(active_w) = self.wheels.last()
        {
            if !self.caption_opacity.done(now) {
                any_tween_active = true;
            }
            let mut hit_discs: SmallVec<[(Point, f32); 6]> = SmallVec::new();
            let mut stem_pts: SmallVec<[(Point, Point, Point); 6]> = SmallVec::new();
            for w in &self.wheels {
                hit_discs.push((w.placement.center, w.hit_geom.hit_radius));
                if let Some(ref st) = w.stem {
                    stem_pts.push((st.p0, st.ctrl, st.p1));
                }
            }
            let (box_, align) = place_caption(
                active_w.placement.center,
                active_w.r_out,
                &hit_discs,
                &stem_pts,
                self.viewport,
            );
            let detail = if node.enabled {
                node.description.clone()
            } else {
                node.disabled_reason.clone()
            };
            self.scene.caption = Some(CaptionVisual {
                box_,
                align,
                title: node.label.clone(),
                detail,
                disabled: !node.enabled,
                opacity: self.caption_opacity.value(now),
            });
        }

        self.animating_flag = any_tween_active;
    }
}

fn select_initial_layout(
    raw_cfg: &WheelConfig,
    validated_cfg: &WheelConfig,
    anchor: Point,
    viewport: Viewport,
) -> (LayoutSelection, LayoutState) {
    match validated_cfg.layout {
        WheelLayout::Branching => (LayoutSelection::branching(), LayoutState::Branching),
        WheelLayout::Concentric => match plan_concentric(anchor, viewport, raw_cfg) {
            Ok(plan) => (
                LayoutSelection::concentric(),
                LayoutState::Concentric(ConcentricState { plan }),
            ),
            Err(reason) => (
                LayoutSelection::fallback_to_branching(reason),
                LayoutState::Branching,
            ),
        },
    }
}

fn populate_root_segments(
    root_wheel: &mut WheelInstance,
    children: &[MenuNode],
    cfg: &WheelConfig,
) {
    let spans = root_segment_spans(children.len());
    root_wheel.segments.clear();
    for (k, node) in children.iter().enumerate() {
        let sp = spans[k];
        root_wheel.segments.push(SegmentState {
            a0: sp.a0,
            a1: sp.a1,
            highlight: Tween::constant(0.0),
            pop: Tween::constant(0.0),
            enabled: node.enabled,
            has_children: node.has_children(),
            label: node.label.clone(),
            icon: node.icon.clone(),
        });
    }
    root_wheel.hit_geom.spans = spans;
    root_wheel.hit_geom.hit_radius = root_wheel.r_out + cfg.hit_margin;
}
