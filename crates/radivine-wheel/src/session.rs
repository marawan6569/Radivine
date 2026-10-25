//! Navigation state machine and session lifecycle (`WHEEL_ENGINE_DESIGN.md` §7, §8, §9).

use std::sync::Arc;

use smallvec::SmallVec;

use crate::anim::{Ease, Millis, Tween, timings};
use crate::concentric::{
    CONCENTRIC_FRESH_MOTION_LP, CONCENTRIC_RADIAL_EPS, ConcentricPlan, NestedHit,
    NestedSegmentContent, compute_sector_content, hit_concentric, plan_concentric,
    retain_hover_hysteresis, ring_geometry, separator_half_angle_for_ring,
};
use crate::concentric_nav::{
    ConcentricDwellTimer, ConcentricHubCloseTimer, ConcentricInputSource, ConcentricPress,
    ConcentricState, PendingSibling,
};
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
    CaptionAlign, CaptionVisual, HubVisual, OriginGuideVisual, Scene, SegmentVisual,
    SelectionVisual, Shape, StemVisual, WheelRole, WheelVisual, WheelVisualKind,
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
    /// Optional constant angular half-gap in radians for concentric ring separators.
    separator_half_angle: Option<f32>,
    /// Precomputed sector content boxes and modes for concentric rings.
    nested_content: Option<SmallVec<[NestedSegmentContent; 12]>>,
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
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
enum LayoutState {
    /// Classic adjacent-wheel branching / vine layout.
    Branching,
    /// Concentric / nested full-circle annular layout (`INTERACTION_DESIGN.md` §I1–§I7).
    Concentric(ConcentricState),
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
    settled_path_hint: Arc<str>,
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
        let sanitized_menu = menu.map(|mut m| {
            m.sanitize(&cfg);
            m
        });
        let root_item_count = sanitized_menu
            .as_ref()
            .and_then(|m| m.root.children())
            .map_or(0, <[MenuNode]>::len);
        let (layout_selection, layout_state) =
            select_initial_layout(raw_cfg, &cfg, anchor, viewport, root_item_count, now);

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
            separator_half_angle: if is_concentric { Some(0.0) } else { None },
            nested_content: if is_concentric {
                Some(SmallVec::new())
            } else {
                None
            },
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
            populate_root_segments(&mut root_wheel, children, &cfg, is_concentric);
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

        let initial_raw_zone = match &layout_state {
            LayoutState::Branching => Zone::Outside,
            LayoutState::Concentric(state) => ConcentricState::hit_to_debug_zone(state.raw_hit),
        };

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
            raw_zone: initial_raw_zone,
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
            settled_path_hint: Arc::from("Move inward to go back."),
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
                root.separator_half_angle = None;
                root.nested_content = None;
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
    pub fn set_menu_at_pointer(&mut self, mut menu: MenuTree, pointer: Point, now: Millis) {
        if self.menu.is_some() || self.has_hovered {
            debug_assert!(
                false,
                "WheelSession::set_menu called after menu was already set or hovered"
            );
            return;
        }
        menu.sanitize(&self.cfg);
        let root_count = menu.root.children().map_or(0, <[MenuNode]>::len);
        let is_concentric = matches!(self.layout_state, LayoutState::Concentric(_));
        if let Some(root_wheel) = self.wheels.first_mut() {
            if let Some(icon) = menu.root.icon.clone() {
                root_wheel.hub.icon = Some(icon);
            }
            if let Some(children) = menu.root.children() {
                populate_root_segments(root_wheel, children, &self.cfg, is_concentric);
            }
        }
        if let LayoutState::Concentric(ref mut state) = self.layout_state {
            self.last_pointer = pointer;
            state.last_timestamp = now;
            state.geometry_gen = state.geometry_gen.wrapping_add(1);
            if let Some(rg0) = state.open_rings.first_mut() {
                *rg0 = ring_geometry(&state.plan, 0, root_count);
            }
            state.exposure_guards[0] = Some(pointer);
            state.plan.approach_required = !state.hub_visited_by_pointer;
            state.raw_hit = hit_concentric(pointer, &state.plan, &state.open_rings);
            self.raw_zone = ConcentricState::hit_to_debug_zone(state.raw_hit);
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
        if let LayoutState::Concentric(ref mut state) = self.layout_state {
            state.last_timestamp = now;
            if state.interaction_suspended {
                self.rebuild_scene(now);
                return;
            }
        }

        match input {
            WheelInput::PointerMoved(p) => self.handle_pointer_moved(p, now, out),
            WheelInput::PointerLeft => match self.layout_state {
                LayoutState::Branching => {
                    self.dwell = None;
                    self.hub_close = None;
                    self.intent_protection = None;
                    self.raw_zone = Zone::Outside;
                    self.set_hovered_item(None, now, out);
                }
                LayoutState::Concentric(ref mut state) => {
                    state.dwell = None;
                    state.pending_sibling = None;
                    state.hub_close = None;
                    state.raw_hit = NestedHit::Outside;
                    self.raw_zone = Zone::Outside;
                    self.set_hovered_item_concentric(None, now, out);
                }
            },
            WheelInput::PointerEntered => {}
            WheelInput::Press => {
                if self.mode == InputMode::Pointer {
                    match self.layout_state {
                        LayoutState::Branching => {
                            self.pressed_zone = Some(self.raw_zone);
                            self.pressed_item = self.hovered_item.clone();
                        }
                        LayoutState::Concentric(_) => {
                            self.handle_pointer_press_concentric();
                        }
                    }
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
        if let LayoutState::Concentric(ref mut state) = self.layout_state {
            state.last_timestamp = now;
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
                LayoutState::Branching => {
                    self.tick_interactive_branching(now, out);
                }
                LayoutState::Concentric(_) => {
                    self.tick_interactive_concentric(now, out);
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
            LayoutState::Branching => self.drop_target_branching(),
            LayoutState::Concentric(_) => self.drop_target_concentric(),
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
        let now = match &self.layout_state {
            LayoutState::Branching => self.wheels.first().map_or(0, |w| w.opacity.start),
            LayoutState::Concentric(state) => state.last_timestamp,
        };
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

        match self.layout_state {
            LayoutState::Branching => {
                // If a disabled item had a pending dwell timer, cancel it
                if !enabled
                    && let Some(dwell) = self.dwell
                    && let Some(w) = self.wheels.get(dwell.wheel as usize)
                    && w.path.child(dwell.item) == *path
                {
                    self.dwell = None;
                }
            }
            LayoutState::Concentric(ref mut state) => {
                state.last_timestamp = now;
                if !enabled {
                    // Cancel dwell if `path` or any ancestor of `dwell.path` became disabled
                    if let Some(ref dwell) = state.dwell
                        && dwell.path.as_slice().starts_with(path.as_slice())
                    {
                        state.dwell = None;
                    }
                    // Invalidate held press if `path` or any ancestor of the pressed item became disabled
                    if let Some(ConcentricPress::Item {
                        path: ref p_path,
                        ref mut eligible_at_press,
                        ..
                    }) = state.pressed
                        && p_path.as_slice().starts_with(path.as_slice())
                    {
                        *eligible_at_press = false;
                    }
                    // If a currently hovered descendant lost an enabled ancestor, clear its highlight
                    if let Some(ref hov_path) = self.hovered_item
                        && hov_path.as_slice().starts_with(path.as_slice())
                        && !hov_path.is_empty()
                    {
                        let w_idx = hov_path.len() - 1;
                        let s_idx = hov_path.as_slice()[w_idx] as usize;
                        if let Some(w) = self.wheels.get_mut(w_idx)
                            && w.path.as_slice() == &hov_path.as_slice()[..w_idx]
                            && let Some(seg) = w.segments.get_mut(s_idx)
                        {
                            seg.highlight = Tween::constant(0.0);
                            seg.pop = Tween::constant(0.0);
                        }
                    }
                } else if let Some(ref hov_path) = self.hovered_item
                    && hov_path.as_slice().starts_with(path.as_slice())
                    && !hov_path.is_empty()
                {
                    // If `hov_path` and all its ancestors are now enabled, restore its hover highlight
                    let all_enabled = self
                        .menu
                        .as_ref()
                        .is_some_and(|m| menu_path_and_ancestors_enabled(m, hov_path));
                    if all_enabled {
                        let w_idx = hov_path.len() - 1;
                        let s_idx = hov_path.as_slice()[w_idx] as usize;
                        let target_hl = if state
                            .pending_sibling
                            .as_ref()
                            .is_some_and(|ps| &ps.path == hov_path)
                        {
                            0.25
                        } else {
                            1.0
                        };
                        let in_dur = if self.cfg.reduced_motion {
                            timings::REDUCED_MOTION_FADE_MS
                        } else {
                            timings::HOVER_IN_MS
                        };
                        if let Some(w) = self.wheels.get_mut(w_idx)
                            && w.path.as_slice() == &hov_path.as_slice()[..w_idx]
                            && let Some(seg) = w.segments.get_mut(s_idx)
                        {
                            seg.highlight
                                .retarget(target_hl, now, in_dur, Ease::OutCubic);
                        }
                    }
                }
            }
        }

        self.rebuild_scene(now);
    }

    /// Suspends interactive selection and navigation timers for an active concentric session
    /// while preserving scene geometry and input capture (`ARCHITECTURE.md` §A3, §A7).
    ///
    /// Nested-only guard; a no-op for [`WheelLayout::Branching`].
    pub fn suspend_interaction(&mut self, now: Millis) {
        if let LayoutState::Concentric(ref mut state) = self.layout_state {
            state.last_timestamp = now;
            state.interaction_suspended = true;
            state.dwell = None;
            state.pending_sibling = None;
            state.hub_close = None;
            state.pressed = None;
            let out_dur = if self.cfg.reduced_motion {
                timings::REDUCED_MOTION_FADE_MS
            } else {
                timings::HOVER_OUT_MS
            };
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
                    seg.pop = Tween::constant(0.0);
                }
            }
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
            self.hovered_item = None;
            self.rebuild_scene(now);
        }
    }

    /// Returns `true` if [`Self::suspend_interaction`] has been called on a concentric session.
    #[inline]
    #[must_use]
    pub fn is_interaction_suspended(&self) -> bool {
        match &self.layout_state {
            LayoutState::Branching => false,
            LayoutState::Concentric(state) => state.interaction_suspended,
        }
    }

    /// Returns the latest strict [`NestedHit`] when running in [`WheelLayout::Concentric`].
    #[inline]
    #[must_use]
    pub fn concentric_hit(&self) -> Option<NestedHit> {
        match &self.layout_state {
            LayoutState::Branching => None,
            LayoutState::Concentric(state) => Some(state.raw_hit),
        }
    }

    /// Returns the active [`ConcentricPlan`] when running in [`WheelLayout::Concentric`].
    #[inline]
    #[must_use]
    pub fn concentric_plan(&self) -> Option<&ConcentricPlan> {
        match &self.layout_state {
            LayoutState::Branching => None,
            LayoutState::Concentric(state) => Some(&state.plan),
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
        if let LayoutState::Concentric(ref mut state) = self.layout_state {
            state.last_timestamp = now;
            state.dwell = None;
            state.pending_sibling = None;
            state.hub_close = None;
            state.pressed = None;
        }
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
        match &self.layout_state {
            LayoutState::Branching => self.next_deadline_branching(),
            LayoutState::Concentric(state) => state.next_deadline(),
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
        let (intent_triangle, dwell_deadline, hub_close_deadline) = match &self.layout_state {
            LayoutState::Branching => (
                self.intent_protection
                    .map(|ip| ([ip.p_exit, ip.tangent_a, ip.tangent_b], ip.deadline)),
                self.dwell.map(|d| d.deadline),
                self.hub_close.map(|h| h.deadline),
            ),
            LayoutState::Concentric(state) => (
                None,
                state
                    .dwell
                    .as_ref()
                    .map(|d| d.deadline)
                    .or_else(|| state.pending_sibling.as_ref().map(|p| p.deadline)),
                state.hub_close.map(|h| h.deadline),
            ),
        };
        DebugState {
            layout_selection: self.layout_selection,
            phase: self.phase,
            moved_past_slop: self.moved_past_slop,
            raw_zone: self.raw_zone,
            hovered_item: self.hovered_item.clone(),
            open_chain,
            wheels: self.hit_cache.clone(),
            intent_triangle,
            dwell_deadline,
            hub_close_deadline,
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
            LayoutState::Branching => {
                self.handle_pointer_moved_branching(p, now, out);
            }
            LayoutState::Concentric(_) => {
                self.handle_pointer_moved_concentric(p, now, out);
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
            LayoutState::Branching => {
                self.handle_pointer_release_branching(now, out);
            }
            LayoutState::Concentric(_) => {
                self.handle_pointer_release_concentric(now, out);
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
            LayoutState::Branching => {
                self.handle_nav_key_branching(nav_key, now, out);
            }
            LayoutState::Concentric(_) => {
                self.handle_nav_key_concentric(nav_key, now, out);
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
            separator_half_angle: None,
            nested_content: None,
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
        let is_concentric = matches!(self.layout_state, LayoutState::Concentric(_));
        let scale_to = if self.cfg.reduced_motion || is_concentric {
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
            LayoutState::Branching => {
                self.rebuild_input_region_branching();
            }
            LayoutState::Concentric(_) => {
                self.rebuild_input_region_concentric();
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
            LayoutState::Branching => {
                self.rebuild_scene_branching(now);
            }
            LayoutState::Concentric(_) => {
                self.rebuild_scene_concentric(now);
            }
        }
    }

    fn rebuild_scene_branching(&mut self, now: Millis) {
        self.scene.generation = self.scene.generation.wrapping_add(1);
        self.scene.origin_guide = None;
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
                    selection: SelectionVisual::None,
                    on_path: open_child_item == Some(s_idx),
                    enabled: seg.enabled,
                    has_children: seg.has_children,
                    label: seg.label.clone(),
                    icon: seg.icon.clone(),
                });
            }

            self.scene.wheels.push(WheelVisual {
                kind: WheelVisualKind::Branching,
                center: w.placement.center,
                r_in: w.r_in,
                r_out: w.r_out,
                scale: w.scale.value(now),
                origin: w.origin,
                opacity: w.opacity.value(now),
                role: w.role,
                bridge: w.bridge.map(|b| (b.a0, b.a1)),
                separator_half_angle: None,
                nested_content: None,
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
                        selection: SelectionVisual::None,
                        on_path: false,
                        enabled: seg.enabled,
                        has_children: seg.has_children,
                        label: seg.label.clone(),
                        icon: seg.icon.clone(),
                    });
                }
                self.scene.wheels.push(WheelVisual {
                    kind: WheelVisualKind::Branching,
                    center: rw.placement.center,
                    r_in: rw.r_in,
                    r_out: rw.r_out,
                    scale: rw.scale.value(now),
                    origin: rw.origin,
                    opacity: rw.opacity.value(now),
                    role: rw.role,
                    bridge: rw.bridge.map(|b| (b.a0, b.a1)),
                    separator_half_angle: None,
                    nested_content: None,
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
                breadcrumb: None,
                title: node.label.clone(),
                detail,
                disabled: !node.enabled,
                opacity: self.caption_opacity.value(now),
            });
        }

        self.animating_flag = any_tween_active;
    }

    // --- Concentric (nested) navigation & scene helpers ---

    fn handle_pointer_moved_concentric(
        &mut self,
        p: Point,
        now: Millis,
        out: &mut Vec<WheelEvent>,
    ) {
        let prev_pointer = self.last_pointer;

        // 1. Check keyboard -> pointer takeover (`>= 2.0 lp` from takeover_point)
        if let LayoutState::Concentric(ref mut state) = self.layout_state {
            if state.interaction_suspended {
                return;
            }
            if let ConcentricInputSource::Keyboard { takeover_point } = state.input_source {
                if p.distance(takeover_point) < CONCENTRIC_FRESH_MOTION_LP {
                    return;
                }
                state.input_source = ConcentricInputSource::Pointer;
                self.moved_past_slop = true;
            }
        }

        // 2. Check initial activation slop (`cfg.activation_slop`)
        if !self.moved_past_slop {
            if p.distance(self.summon_point) < self.cfg.activation_slop {
                self.last_pointer = p;
                return;
            }
            self.moved_past_slop = true;
        }

        self.last_pointer = p;

        // 3. Update exposure guards (`>= 2.0 lp` from exposed_at_pointer), release displaced-origin
        //    approach guard if this real motion sample enters the hub (`d < h - 1e-4`), and evaluate strict hit.
        let (prev_strict_hit, prev_exposure_guards, strict_hit) = match self.layout_state {
            LayoutState::Concentric(ref mut state) => {
                let prev_guards = state.exposure_guards;
                state.update_exposure_guards(p);
                let prev = state.raw_hit;
                if !state.hub_visited_by_pointer {
                    let h = state
                        .open_rings
                        .first()
                        .map(|r| r.r_in)
                        .or_else(|| state.plan.radii.first().map(|&(r_in, _)| r_in))
                        .unwrap_or(40.0);
                    if p.distance(state.plan.center) < h - CONCENTRIC_RADIAL_EPS {
                        state.hub_visited_by_pointer = true;
                    }
                }
                state.plan.approach_required = !state.hub_visited_by_pointer;
                let hit = hit_concentric(p, &state.plan, &state.open_rings);
                state.raw_hit = hit;
                self.raw_zone = ConcentricState::hit_to_debug_zone(hit);
                (prev, prev_guards, hit)
            }
            LayoutState::Branching => return,
        };

        match strict_hit {
            NestedHit::Approach => {
                if let LayoutState::Concentric(ref mut state) = self.layout_state {
                    state.dwell = None;
                    state.pending_sibling = None;
                    state.hub_close = None;
                }
                self.set_hovered_item_concentric(None, now, out);
            }
            NestedHit::Hub => {
                let has_open_children = self.wheels.len() > 1;
                if let LayoutState::Concentric(ref mut state) = self.layout_state {
                    state.hub_visited_by_pointer = true;
                    state.plan.approach_required = false;
                    state.dwell = None;
                    state.pending_sibling = None;
                    if has_open_children {
                        if state.hub_close.is_none() {
                            state.hub_close = Some(ConcentricHubCloseTimer {
                                generation: state.geometry_gen,
                                deadline: now + u64::from(self.cfg.hub_close_ms),
                            });
                        }
                    } else {
                        state.hub_close = None;
                    }
                }
                self.set_hovered_item_concentric(None, now, out);
            }
            NestedHit::RadialGap { .. }
            | NestedHit::RadialBoundary { .. }
            | NestedHit::Separator { .. }
            | NestedHit::OuterMargin
            | NestedHit::Outside => {
                // Check if previous hover was a non-pending item eligible for hover hysteresis retention
                let prev_hover_pair = self.non_pending_hover_pair();
                let retained = match self.layout_state {
                    LayoutState::Concentric(ref mut state) => {
                        state.dwell = None;
                        state.pending_sibling = None;
                        state.hub_close = None;
                        retain_hover_hysteresis(
                            p,
                            strict_hit,
                            prev_hover_pair,
                            &state.plan,
                            &state.open_rings,
                            self.cfg.hysteresis_deg,
                        )
                    }
                    LayoutState::Branching => None,
                };
                if let Some((h_lvl, h_idx)) = retained
                    && let Some(w) = self.wheels.get(h_lvl as usize)
                {
                    let retained_path = w.path.child(h_idx);
                    self.set_hovered_item_concentric(Some(retained_path), now, out);
                } else {
                    self.set_hovered_item_concentric(None, now, out);
                }
            }
            NestedHit::Item { level, index } => {
                let w_idx = level as usize;
                if w_idx >= self.wheels.len() {
                    return;
                }

                let is_guarded = match self.layout_state {
                    LayoutState::Concentric(ref mut state) => {
                        state.hub_close = None;
                        state.is_level_exposure_guarded(level)
                    }
                    LayoutState::Branching => false,
                };

                if is_guarded {
                    if let LayoutState::Concentric(ref mut state) = self.layout_state {
                        state.dwell = None;
                        state.pending_sibling = None;
                    }
                    self.set_hovered_item_concentric(None, now, out);
                    return;
                }

                let item_path = self.wheels[w_idx].path.child(index);

                // Does ring `level` currently have an open child ring `level + 1`?
                if w_idx + 1 < self.wheels.len() {
                    let open_item = self.wheels[w_idx + 1].parent_item;
                    if index == open_item {
                        // Strict hit on the open parent sector: keep all descendants, clear pending sibling
                        if let LayoutState::Concentric(ref mut state) = self.layout_state {
                            state.pending_sibling = None;
                            state.dwell = None;
                        }
                        self.set_hovered_item_concentric(Some(item_path), now, out);
                        return;
                    }

                    // Strict hit on a different ancestor sector while a child ring is open
                    let already_pending_same = match &self.layout_state {
                        LayoutState::Concentric(state) => state
                            .pending_sibling
                            .as_ref()
                            .is_some_and(|ps| ps.path == item_path),
                        LayoutState::Branching => false,
                    };

                    if already_pending_same {
                        // Check two-sample outward push shortcut (`INTERACTION_DESIGN.md` §I5)
                        let two_sample_same = prev_strict_hit == NestedHit::Item { level, index };
                        if two_sample_same
                            && self.should_open_by_outer_band_concentric(level, prev_pointer, p)
                        {
                            if let LayoutState::Concentric(ref mut state) = self.layout_state {
                                state.pending_sibling = None;
                                state.dwell = None;
                            }
                            self.truncate_after_concentric(level, now, out);
                            self.set_hovered_item_concentric(Some(item_path), now, out);
                            if self.is_enabled_submenu_with_ancestors(level, index) {
                                self.open_child_wheel_concentric(level, index, false, now, out);
                            }
                            return;
                        }

                        // Keep existing pending sibling deadline without resetting
                        self.set_hovered_item_concentric(Some(item_path), now, out);
                        return;
                    }

                    // First crossing into this candidate ancestor sibling
                    if let LayoutState::Concentric(ref mut state) = self.layout_state {
                        state.dwell = None;
                        state.pending_sibling = Some(PendingSibling {
                            level,
                            item: index,
                            path: item_path.clone(),
                            generation: state.geometry_gen,
                            entered_at: now,
                            deadline: now + u64::from(self.cfg.grace_ms),
                        });
                    }
                    self.set_hovered_item_concentric(Some(item_path), now, out);
                    return;
                }

                // Ring `level` is the deepest open ring
                if let LayoutState::Concentric(ref mut state) = self.layout_state {
                    state.pending_sibling = None;
                }
                let hover_changed = self.hovered_item.as_ref() != Some(&item_path);
                self.set_hovered_item_concentric(Some(item_path.clone()), now, out);

                if self.is_enabled_submenu_with_ancestors(level, index) {
                    let was_level_guarded = prev_exposure_guards
                        .get(usize::from(level))
                        .copied()
                        .flatten()
                        .is_some();
                    let was_already_on_sector =
                        !was_level_guarded && prev_strict_hit == NestedHit::Item { level, index };
                    if was_already_on_sector
                        && self.should_open_by_outer_band_concentric(level, prev_pointer, p)
                    {
                        if let LayoutState::Concentric(ref mut state) = self.layout_state {
                            state.dwell = None;
                        }
                        self.open_child_wheel_concentric(level, index, false, now, out);
                    } else {
                        let need_schedule = match &self.layout_state {
                            LayoutState::Concentric(state) => {
                                hover_changed
                                    || state.dwell.as_ref().is_none_or(|d| d.path != item_path)
                            }
                            LayoutState::Branching => false,
                        };
                        if need_schedule {
                            self.schedule_dwell_concentric(level, index, now, now);
                        }
                    }
                } else if let LayoutState::Concentric(ref mut state) = self.layout_state {
                    state.dwell = None;
                }
            }
        }
    }

    fn handle_pointer_press_concentric(&mut self) {
        let press = match &self.layout_state {
            LayoutState::Concentric(state) => {
                if state.interaction_suspended {
                    return;
                }
                match state.raw_hit {
                    NestedHit::Hub => ConcentricPress::Hub,
                    NestedHit::Outside => ConcentricPress::Outside,
                    NestedHit::Item { level, index } => {
                        if let Some(w) = self.wheels.get(level as usize) {
                            let path = w.path.child(index);
                            let eligible = self.is_concentric_item_eligible(level, index, &path);
                            ConcentricPress::Item {
                                path,
                                generation: state.geometry_gen,
                                eligible_at_press: eligible,
                            }
                        } else {
                            ConcentricPress::Neutral
                        }
                    }
                    NestedHit::RadialGap { .. }
                    | NestedHit::RadialBoundary { .. }
                    | NestedHit::Separator { .. }
                    | NestedHit::OuterMargin
                    | NestedHit::Approach => ConcentricPress::Neutral,
                }
            }
            LayoutState::Branching => return,
        };
        if let LayoutState::Concentric(ref mut state) = self.layout_state {
            state.pressed = Some(press);
        }
    }

    fn handle_pointer_release_concentric(&mut self, now: Millis, out: &mut Vec<WheelEvent>) {
        let (press, raw_hit, cur_gen, suspended, pointer_guards_ok) = match self.layout_state {
            LayoutState::Concentric(ref mut state) => (
                state.pressed.take(),
                state.raw_hit,
                state.geometry_gen,
                state.interaction_suspended,
                self.moved_past_slop
                    && state.hub_visited_by_pointer
                    && matches!(state.input_source, ConcentricInputSource::Pointer),
            ),
            LayoutState::Branching => return,
        };

        if suspended {
            return;
        }

        match raw_hit {
            NestedHit::Outside => {
                out.push(WheelEvent::CancelRequested);
            }
            NestedHit::Hub => {
                if matches!(self.cfg.invoke_select, InvokeSelect::Release)
                    || matches!(press, Some(ConcentricPress::Hub) | None)
                {
                    out.push(WheelEvent::CancelRequested);
                }
            }
            NestedHit::RadialGap { .. }
            | NestedHit::RadialBoundary { .. }
            | NestedHit::Separator { .. }
            | NestedHit::OuterMargin
            | NestedHit::Approach => {
                // Non-action neutral zones are a no-op on click
            }
            NestedHit::Item { level, index } => {
                if !pointer_guards_ok {
                    return;
                }
                let is_guarded = match &self.layout_state {
                    LayoutState::Concentric(state) => state.is_level_exposure_guarded(level),
                    LayoutState::Branching => true,
                };
                if is_guarded {
                    return;
                }
                let Some(w) = self.wheels.get(level as usize) else {
                    return;
                };
                let path = w.path.child(index);
                let Some(ref menu) = self.menu else {
                    return;
                };
                if !menu_path_and_ancestors_enabled(menu, &path) {
                    return;
                }
                let Some(node) = menu.node(&path) else {
                    return;
                };
                let is_submenu = matches!(node.kind, NodeKind::Submenu { .. });

                if is_submenu {
                    // Explicit click on an enabled submenu adopts/opens it (even during pending-sibling grace)
                    // and never commits its default_child (`INTERACTION_DESIGN.md` §I6).
                    let click_matches = match self.cfg.invoke_select {
                        InvokeSelect::Release => true,
                        InvokeSelect::Click => match &press {
                            Some(ConcentricPress::Item {
                                path: p_path,
                                generation: p_gen,
                                ..
                            }) => p_path == &path && *p_gen == cur_gen,
                            Some(ConcentricPress::Hub) => true,
                            _ => false,
                        },
                    };
                    if !click_matches {
                        return;
                    }
                    let w_idx = level as usize;
                    let already_open = self.wheels.get(w_idx + 1).is_some_and(|cw| cw.path == path);
                    if let LayoutState::Concentric(ref mut state) = self.layout_state {
                        state.dwell = None;
                        state.pending_sibling = None;
                    }
                    if !already_open {
                        if w_idx + 1 < self.wheels.len() {
                            self.truncate_after_concentric(level, now, out);
                        }
                        self.set_hovered_item_concentric(Some(path), now, out);
                        self.open_child_wheel_concentric(level, index, false, now, out);
                    }
                    return;
                }

                // Enabled leaf item: requires strict eligibility (including no pending sibling)
                if !self.is_concentric_item_eligible(level, index, &path) {
                    return;
                }

                let can_commit = match self.cfg.invoke_select {
                    InvokeSelect::Release => true,
                    InvokeSelect::Click => match &press {
                        Some(ConcentricPress::Item {
                            path: p_path,
                            generation: p_gen,
                            eligible_at_press: true,
                        }) => p_path == &path && *p_gen == cur_gen,
                        Some(ConcentricPress::Hub) => true,
                        _ => false,
                    },
                };
                if can_commit {
                    self.start_commit_animation(&path, now);
                    self.input_shapes.clear();
                    out.push(WheelEvent::InputRegionChanged);
                    out.push(WheelEvent::Commit(path));
                }
            }
        }
    }

    fn handle_nav_key_concentric(
        &mut self,
        nav_key: NavKey,
        now: Millis,
        out: &mut Vec<WheelEvent>,
    ) {
        if self.wheels.is_empty() {
            return;
        }
        if let LayoutState::Concentric(ref mut state) = self.layout_state {
            if state.interaction_suspended {
                return;
            }
            state.input_source = ConcentricInputSource::Keyboard {
                takeover_point: self.last_pointer,
            };
            state.dwell = None;
            state.pending_sibling = None;
            state.hub_close = None;
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
                let path = self.wheels[active_w].path.child(next as u8);
                self.set_hovered_item_concentric(Some(path), now, out);
            }
            NavKey::Prev => {
                if n == 0 {
                    return;
                }
                let prev = cur_idx.map_or(n - 1, |i| (i + n - 1) % n);
                let path = self.wheels[active_w].path.child(prev as u8);
                self.set_hovered_item_concentric(Some(path), now, out);
            }
            NavKey::Activate => {
                if let Some(i) = cur_idx {
                    self.activate_keyboard_item_concentric(active_w as u8, i as u8, now, out);
                }
            }
            NavKey::Back => {
                if active_w >= 1 {
                    let parent_w = (active_w - 1) as u8;
                    let parent_item = self.wheels[active_w].parent_item;
                    self.truncate_after_concentric(parent_w, now, out);
                    let parent_path = self.wheels[parent_w as usize].path.child(parent_item);
                    self.set_hovered_item_concentric(Some(parent_path), now, out);
                }
            }
            NavKey::Digit(d) => {
                if d >= 1 && (d as usize) <= n {
                    let idx = d - 1;
                    let path = self.wheels[active_w].path.child(idx);
                    self.set_hovered_item_concentric(Some(path), now, out);
                    self.activate_keyboard_item_concentric(active_w as u8, idx, now, out);
                }
            }
            NavKey::Cancel => {
                out.push(WheelEvent::CancelRequested);
            }
        }
    }

    fn activate_keyboard_item_concentric(
        &mut self,
        wheel: u8,
        item: u8,
        now: Millis,
        out: &mut Vec<WheelEvent>,
    ) {
        let Some(w) = self.wheels.get(wheel as usize) else {
            return;
        };
        let path = w.path.child(item);
        let Some(ref menu) = self.menu else {
            return;
        };
        // Keyboard activation also checks all ancestors' enablement (`INTERACTION_DESIGN.md` §I6)
        if !menu_path_and_ancestors_enabled(menu, &path) {
            return;
        }
        let Some(is_submenu) = menu.node(&path).map(
            |node| matches!(&node.kind, NodeKind::Submenu { children, .. } if !children.is_empty()),
        ) else {
            return;
        };
        if is_submenu {
            self.open_child_wheel_concentric(wheel, item, true, now, out);
        } else {
            self.start_commit_animation(&path, now);
            self.input_shapes.clear();
            out.push(WheelEvent::InputRegionChanged);
            out.push(WheelEvent::Commit(path));
        }
    }

    fn is_enabled_submenu_with_ancestors(&self, level: u8, item: u8) -> bool {
        let Some(w) = self.wheels.get(level as usize) else {
            return false;
        };
        let path = w.path.child(item);
        let Some(ref menu) = self.menu else {
            return false;
        };
        if !menu_path_and_ancestors_enabled(menu, &path) {
            return false;
        }
        menu.node(&path).is_some_and(MenuNode::has_children)
    }

    fn should_open_by_outer_band_concentric(&self, level: u8, prev_p: Point, cur_p: Point) -> bool {
        if !matches!(
            self.cfg.submenu_open,
            SubmenuOpen::Band | SubmenuOpen::DwellAndBand
        ) {
            return false;
        }
        let Some(w) = self.wheels.get(level as usize) else {
            return false;
        };
        let d_now = cur_p.distance(w.placement.center);
        let d_prev = prev_p.distance(w.placement.center);
        let threshold = w.r_in + self.cfg.band * (w.r_out - w.r_in);
        d_now >= threshold && (d_now - d_prev) >= CONCENTRIC_FRESH_MOTION_LP
    }

    fn schedule_dwell_concentric(&mut self, level: u8, item: u8, entered_at: Millis, now: Millis) {
        if !matches!(
            self.cfg.submenu_open,
            SubmenuOpen::Dwell | SubmenuOpen::DwellAndBand
        ) {
            return;
        }
        if !self.is_enabled_submenu_with_ancestors(level, item) {
            return;
        }
        let Some(w) = self.wheels.get(level as usize) else {
            return;
        };
        let path = w.path.child(item);
        let deadline = (entered_at + u64::from(self.cfg.dwell_ms)).max(now);
        if let LayoutState::Concentric(ref mut state) = self.layout_state {
            state.dwell = Some(ConcentricDwellTimer {
                path,
                generation: state.geometry_gen,
                entered_at,
                deadline,
            });
        }
    }

    fn open_child_wheel_concentric(
        &mut self,
        parent_level: u8,
        item: u8,
        focus_first_child: bool,
        now: Millis,
        out: &mut Vec<WheelEvent>,
    ) {
        let p_idx = parent_level as usize;
        if p_idx >= self.wheels.len() {
            return;
        }

        if p_idx + 1 < self.wheels.len() {
            self.truncate_after_concentric(parent_level, now, out);
        }

        let child_path = self.wheels[p_idx].path.child(item);
        let Some(ref menu) = self.menu else {
            return;
        };
        if !menu_path_and_ancestors_enabled(menu, &child_path) {
            return;
        }
        let Some(submenu_node) = menu.node(&child_path) else {
            return;
        };
        let Some(children) = submenu_node.children() else {
            return;
        };
        if children.is_empty() {
            return;
        }

        let level = (p_idx + 1) as u8;
        if usize::from(level) >= usize::from(self.cfg.max_depth) {
            return;
        }

        let (center, r_in, r_out, rg) = match &self.layout_state {
            LayoutState::Concentric(state) => {
                let rg = ring_geometry(&state.plan, level, children.len());
                (state.plan.center, rg.r_in, rg.r_out, rg)
            }
            LayoutState::Branching => return,
        };

        // Remove any retracting ghost at this radial slot (`INTERACTION_DESIGN.md` §I8)
        self.retracting
            .retain(|rw| rw.path.len() != usize::from(level));

        // Dim ancestor rings to 0.72 opacity
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

        let emerge_dur = if self.cfg.reduced_motion {
            timings::REDUCED_MOTION_FADE_MS
        } else {
            timings::CHILD_EMERGE_MS
        };
        let opacity_tween = Tween::new(0.35, 1.0, now, emerge_dur, Ease::OutCubic);

        let mut segments = SmallVec::new();
        let mut nested_content = SmallVec::new();
        for (k, child_node) in children.iter().enumerate() {
            let sp = rg.spans[k];
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
            nested_content.push(compute_sector_content(
                center,
                r_in,
                r_out,
                sp,
                rg.separator_half_angle,
                children.len(),
                child_node.icon.is_some(),
            ));
        }

        let hit_geom = WheelHitGeometry {
            index: level,
            parent_index: Some(parent_level),
            center,
            r_in,
            r_out,
            hit_radius: r_out + self.cfg.hit_margin,
            bridge: None,
            spans: rg.spans.clone(),
            corridor: None,
        };

        let child_instance = WheelInstance {
            path: child_path.clone(),
            parent_item: item,
            placement: Placement {
                center,
                r_out,
                attach_angle: 0.0,
                fallback: false,
            },
            r_in,
            r_out,
            origin: center,
            scale: Tween::constant(1.0),
            opacity: opacity_tween,
            role: WheelRole::Active,
            bridge: None,
            separator_half_angle: Some(rg.separator_half_angle),
            nested_content: Some(nested_content),
            hub: HubVisual {
                icon: submenu_node
                    .icon
                    .clone()
                    .or_else(|| Some(self.default_child_hub_icon.clone())),
                badge: None,
            },
            segments,
            stem: None,
            hit_geom,
        };

        self.wheels.push(child_instance);
        if self.caption_opacity.to < 1.0 {
            let cap_dur = if self.cfg.reduced_motion {
                timings::REDUCED_MOTION_FADE_MS
            } else {
                timings::CAPTION_FADE_MS
            };
            self.caption_opacity
                .retarget(1.0, now, cap_dur, Ease::Linear);
        }

        if let LayoutState::Concentric(ref mut state) = self.layout_state {
            state.open_rings.push(rg);
            state.geometry_gen = state.geometry_gen.wrapping_add(1);
            state.dwell = None;
            state.pending_sibling = None;
            if usize::from(level) < state.exposure_guards.len() {
                state.exposure_guards[usize::from(level)] = Some(self.last_pointer);
            }
            state.plan.approach_required = !state.hub_visited_by_pointer;
            state.raw_hit = hit_concentric(self.last_pointer, &state.plan, &state.open_rings);
            self.raw_zone = ConcentricState::hit_to_debug_zone(state.raw_hit);
        }

        self.sync_hit_cache();
        out.push(WheelEvent::SubmenuOpened(child_path.clone()));

        if focus_first_child {
            let first_child_path = child_path.child(0);
            self.set_hovered_item_concentric(Some(first_child_path), now, out);
        }
    }

    fn truncate_after_concentric(
        &mut self,
        keep_level: u8,
        now: Millis,
        out: &mut Vec<WheelEvent>,
    ) {
        let target_len = usize::from(keep_level) + 1;
        if self.wheels.len() <= target_len {
            return;
        }

        let retract_dur = if self.cfg.reduced_motion {
            timings::REDUCED_MOTION_FADE_MS
        } else {
            timings::CHILD_RETRACT_MS
        };

        while self.wheels.len() > target_len {
            let Some(mut closed) = self.wheels.pop() else {
                break;
            };
            let closed_path = closed.path.clone();
            let closed_level = closed_path.len();
            closed.scale = Tween::constant(1.0);
            closed
                .opacity
                .retarget(0.0, now, retract_dur, Ease::OutCubic);

            // Keep at most one retracting snapshot per radial level (`INTERACTION_DESIGN.md` §I8)
            self.retracting.retain(|rw| rw.path.len() != closed_level);
            if self.retracting.len() >= 4 {
                self.retracting.remove(0);
            }
            self.retracting.push(closed);
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

        if let LayoutState::Concentric(ref mut state) = self.layout_state {
            state.open_rings.truncate(target_len);
            for lvl in target_len..state.exposure_guards.len() {
                state.exposure_guards[lvl] = None;
            }
            state.geometry_gen = state.geometry_gen.wrapping_add(1);
            state.plan.approach_required = !state.hub_visited_by_pointer;
            state.raw_hit = hit_concentric(self.last_pointer, &state.plan, &state.open_rings);
            self.raw_zone = ConcentricState::hit_to_debug_zone(state.raw_hit);
        }

        if self.wheels.len() <= 1 && self.hovered_item.is_none() {
            let cap_dur = if self.cfg.reduced_motion {
                timings::REDUCED_MOTION_FADE_MS
            } else {
                timings::CAPTION_FADE_MS
            };
            self.caption_opacity
                .retarget(0.0, now, cap_dur, Ease::Linear);
        }

        self.sync_hit_cache();
    }

    fn set_hovered_item_concentric(
        &mut self,
        new_hover: Option<ItemPath>,
        now: Millis,
        out: &mut Vec<WheelEvent>,
    ) {
        let is_pending_target = match (&self.layout_state, &new_hover) {
            (LayoutState::Concentric(state), Some(np)) => state
                .pending_sibling
                .as_ref()
                .is_some_and(|ps| &ps.path == np),
            _ => false,
        };
        let target_hl = if is_pending_target { 0.25 } else { 1.0 };

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

        if self.hovered_item == new_hover {
            // Even if the path is unchanged (e.g. pending sibling adopted in place), ensure highlight
            // reflects the current pending vs active state (`0.25` -> `1.0`).
            if let Some(ref cur_path) = self.hovered_item
                && !cur_path.is_empty()
            {
                let all_enabled = self
                    .menu
                    .as_ref()
                    .is_some_and(|m| menu_path_and_ancestors_enabled(m, cur_path));
                let w_idx = cur_path.len() - 1;
                let s_idx = cur_path.as_slice()[w_idx] as usize;
                if let Some(w) = self.wheels.get_mut(w_idx)
                    && w.path.as_slice() == &cur_path.as_slice()[..w_idx]
                    && let Some(seg) = w.segments.get_mut(s_idx)
                    && all_enabled
                    && (seg.highlight.to - target_hl).abs() > 1e-4
                {
                    seg.highlight
                        .retarget(target_hl, now, in_dur, Ease::OutCubic);
                    seg.pop = Tween::constant(0.0);
                }
            }
            return;
        }

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
                seg.pop = Tween::constant(0.0);
            }
        }

        // Retarget new hovered segment in (only if item and all ancestors are enabled; pop is always 0)
        if let Some(ref new_path) = new_hover
            && !new_path.is_empty()
        {
            self.has_hovered = true;
            let all_enabled = self
                .menu
                .as_ref()
                .is_some_and(|m| menu_path_and_ancestors_enabled(m, new_path));
            let w_idx = new_path.len() - 1;
            let s_idx = new_path.as_slice()[w_idx] as usize;
            if let Some(w) = self.wheels.get_mut(w_idx)
                && w.path.as_slice() == &new_path.as_slice()[..w_idx]
                && let Some(seg) = w.segments.get_mut(s_idx)
            {
                seg.pop = Tween::constant(0.0);
                if all_enabled {
                    seg.highlight
                        .retarget(target_hl, now, in_dur, Ease::OutCubic);
                }
            }
            self.caption_opacity.retarget(
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
            let target_cap_op = if self.wheels.len() > 1 { 1.0 } else { 0.0 };
            self.caption_opacity.retarget(
                target_cap_op,
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

    fn tick_interactive_concentric(&mut self, now: Millis, out: &mut Vec<WheelEvent>) {
        let suspended = match &self.layout_state {
            LayoutState::Concentric(state) => state.interaction_suspended,
            LayoutState::Branching => return,
        };
        if suspended {
            return;
        }

        // 1. Pending sibling adoption grace timer
        let expired_pending = match &mut self.layout_state {
            LayoutState::Concentric(state) => {
                if let Some(ref ps) = state.pending_sibling
                    && now >= ps.deadline
                {
                    state.pending_sibling.take().map(|ps| {
                        let valid = ps.generation == state.geometry_gen
                            && state.raw_hit
                                == NestedHit::Item {
                                    level: ps.level,
                                    index: ps.item,
                                };
                        (ps, valid)
                    })
                } else {
                    None
                }
            }
            LayoutState::Branching => None,
        };

        let mut adopted_in_this_tick = false;
        if let Some((ps, valid)) = expired_pending
            && valid
            && usize::from(ps.level) < self.wheels.len()
            && self.wheels[usize::from(ps.level)].path.child(ps.item) == ps.path
        {
            self.truncate_after_concentric(ps.level, now, out);
            self.set_hovered_item_concentric(Some(ps.path.clone()), now, out);
            adopted_in_this_tick = true;

            if self.is_enabled_submenu_with_ancestors(ps.level, ps.item)
                && matches!(
                    self.cfg.submenu_open,
                    SubmenuOpen::Dwell | SubmenuOpen::DwellAndBand
                )
            {
                let dwell_ready_at = ps.entered_at + u64::from(self.cfg.dwell_ms);
                if now >= dwell_ready_at {
                    self.open_child_wheel_concentric(ps.level, ps.item, false, now, out);
                } else {
                    self.schedule_dwell_concentric(ps.level, ps.item, ps.entered_at, now);
                }
            }
        }

        // 2. Center hub return-to-root timer
        let expired_hub = match &mut self.layout_state {
            LayoutState::Concentric(state) => {
                if let Some(hc) = state.hub_close
                    && now >= hc.deadline
                {
                    state.hub_close = None;
                    Some(hc.generation == state.geometry_gen && state.raw_hit == NestedHit::Hub)
                } else {
                    None
                }
            }
            LayoutState::Branching => None,
        };
        if expired_hub == Some(true) && self.wheels.len() > 1 {
            self.truncate_after_concentric(0, now, out);
        }

        // 3. Deepest-ring submenu dwell timer
        if !adopted_in_this_tick {
            let expired_dwell = match &mut self.layout_state {
                LayoutState::Concentric(state) => {
                    if let Some(ref dwell) = state.dwell
                        && now >= dwell.deadline
                    {
                        state.dwell.take().map(|d| {
                            let valid = d.generation == state.geometry_gen
                                && state.pending_sibling.is_none()
                                && match state.raw_hit {
                                    NestedHit::Item { level, index } => {
                                        !state.is_level_exposure_guarded(level)
                                            && self
                                                .wheels
                                                .get(usize::from(level))
                                                .is_some_and(|w| w.path.child(index) == d.path)
                                    }
                                    _ => false,
                                };
                            (d, valid)
                        })
                    } else {
                        None
                    }
                }
                LayoutState::Branching => None,
            };

            if let Some((dwell, true)) = expired_dwell
                && !dwell.path.is_empty()
            {
                let level = (dwell.path.len() - 1) as u8;
                let item = dwell.path.as_slice()[usize::from(level)];
                if self.is_enabled_submenu_with_ancestors(level, item) {
                    self.open_child_wheel_concentric(level, item, false, now, out);
                }
            }
        }
    }

    fn is_concentric_item_eligible(&self, level: u8, index: u8, path: &ItemPath) -> bool {
        if !self.moved_past_slop
            || !matches!(self.phase, SessionPhase::Appearing | SessionPhase::Open)
        {
            return false;
        }
        let LayoutState::Concentric(ref state) = self.layout_state else {
            return false;
        };
        if state.interaction_suspended
            || !state.hub_visited_by_pointer
            || !matches!(state.input_source, ConcentricInputSource::Pointer)
            || state.pending_sibling.is_some()
            || state.is_level_exposure_guarded(level)
            || state.raw_hit != (NestedHit::Item { level, index })
        {
            return false;
        }
        if self.hovered_item.as_ref() != Some(path) {
            return false;
        }
        let Some(ref menu) = self.menu else {
            return false;
        };
        menu_path_and_ancestors_enabled(menu, path)
    }

    fn drop_target_concentric(&self) -> DropTarget {
        let LayoutState::Concentric(ref state) = self.layout_state else {
            return DropTarget::Reject;
        };
        let NestedHit::Item { level, index } = state.raw_hit else {
            return DropTarget::Reject;
        };
        let Some(w) = self.wheels.get(usize::from(level)) else {
            return DropTarget::Reject;
        };
        let path = w.path.child(index);
        if !self.is_concentric_item_eligible(level, index, &path) {
            return DropTarget::Reject;
        }
        let Some(ref menu) = self.menu else {
            return DropTarget::Reject;
        };
        let Some(node) = menu.node(&path) else {
            return DropTarget::Reject;
        };
        match &node.kind {
            NodeKind::Leaf => DropTarget::Accept(path),
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

    fn non_pending_hover_pair(&self) -> Option<(u8, u8)> {
        let hov = self.hovered_item.as_ref()?;
        if hov.is_empty() {
            return None;
        }
        if let LayoutState::Concentric(ref state) = self.layout_state
            && state
                .pending_sibling
                .as_ref()
                .is_some_and(|ps| &ps.path == hov)
        {
            return None;
        }
        let w_idx = hov.len() - 1;
        let w = self.wheels.get(w_idx)?;
        if w.path.as_slice() != &hov.as_slice()[..w_idx] {
            return None;
        }
        let item_idx = hov.as_slice()[w_idx];
        if w_idx + 1 < self.wheels.len() && self.wheels[w_idx + 1].parent_item != item_idx {
            return None;
        }
        Some((w_idx as u8, item_idx))
    }

    fn rebuild_input_region_concentric(&mut self) {
        self.input_shapes.clear();
        if !matches!(self.phase, SessionPhase::Appearing | SessionPhase::Open) {
            return;
        }
        let LayoutState::Concentric(ref state) = self.layout_state else {
            return;
        };
        let disc_r = state.plan.reserved_radius + self.cfg.hit_margin.max(0.0);
        if self.anchor.distance(state.plan.center) > 0.5 {
            let anchor_r = 24.0;
            self.input_shapes.push(Shape::Circle {
                c: self.anchor,
                r: anchor_r,
            });
            let corr = anchor_corridor_polygon(self.anchor, anchor_r, state.plan.center, disc_r);
            self.input_shapes.push(Shape::Polygon(corr));
        }
        self.input_shapes.push(Shape::Circle {
            c: state.plan.center,
            r: disc_r,
        });
    }

    fn rebuild_scene_concentric(&mut self, now: Millis) {
        self.scene.generation = self.scene.generation.wrapping_add(1);
        self.scene.origin_guide = None;
        self.scene.stems.clear();
        self.scene.wheels.clear();

        let mut any_tween_active = false;
        let num_wheels = self.wheels.len();

        // 0. Displaced-origin guide (`anchor != center`, `0.45 * root_opacity` below rings)
        if let LayoutState::Concentric(ref state) = self.layout_state
            && self.anchor.distance(state.plan.center) > 0.5
        {
            let root_op = self.wheels.first().map_or(0.0, |w| w.opacity.value(now));
            if root_op > 0.0 {
                self.scene.origin_guide = Some(OriginGuideVisual {
                    anchor: self.anchor,
                    center: state.plan.center,
                    opacity: (0.45 * root_op).clamp(0.0, 1.0),
                });
            }
        }

        // 1. Active rings (root -> deepest)
        for (w_idx, w) in self.wheels.iter().enumerate() {
            if !w.scale.done(now) || !w.opacity.done(now) {
                any_tween_active = true;
            }
            let open_child_item = if w_idx + 1 < num_wheels {
                Some(self.wheels[w_idx + 1].parent_item as usize)
            } else {
                None
            };
            let ring_ancestors_enabled = self
                .menu
                .as_ref()
                .is_none_or(|m| menu_path_and_ancestors_enabled(m, &w.path));

            let mut seg_visuals = SmallVec::new();
            for (s_idx, seg) in w.segments.iter().enumerate() {
                if !seg.highlight.done(now) || !seg.pop.done(now) {
                    any_tween_active = true;
                }
                let eff_seg_enabled = seg.enabled && ring_ancestors_enabled;
                let seg_path = w.path.child(s_idx as u8);
                let selection = if self.hovered_item.as_ref() == Some(&seg_path)
                    && matches!(self.phase, SessionPhase::Appearing | SessionPhase::Open)
                {
                    match &self.layout_state {
                        LayoutState::Concentric(state) if !state.interaction_suspended => {
                            if state
                                .pending_sibling
                                .as_ref()
                                .is_some_and(|ps| ps.path == seg_path)
                            {
                                if eff_seg_enabled {
                                    SelectionVisual::Pending
                                } else {
                                    SelectionVisual::None
                                }
                            } else if matches!(
                                state.input_source,
                                ConcentricInputSource::Keyboard { .. }
                            ) {
                                SelectionVisual::KeyboardFocus
                            } else if self.is_concentric_item_eligible(
                                w_idx as u8,
                                s_idx as u8,
                                &seg_path,
                            ) {
                                SelectionVisual::Armed
                            } else {
                                SelectionVisual::None
                            }
                        }
                        _ => SelectionVisual::None,
                    }
                } else {
                    SelectionVisual::None
                };

                seg_visuals.push(SegmentVisual {
                    a0: seg.a0,
                    a1: seg.a1,
                    highlight: seg.highlight.value(now),
                    pop: 0.0,
                    selection,
                    on_path: open_child_item == Some(s_idx),
                    enabled: eff_seg_enabled,
                    has_children: seg.has_children,
                    label: seg.label.clone(),
                    icon: seg.icon.clone(),
                });
            }

            let kind = if w_idx == 0 {
                WheelVisualKind::ConcentricRoot
            } else {
                WheelVisualKind::ConcentricRing
            };

            self.scene.wheels.push(WheelVisual {
                kind,
                center: w.placement.center,
                r_in: w.r_in,
                r_out: w.r_out,
                scale: 1.0,
                origin: w.origin,
                opacity: w.opacity.value(now),
                role: w.role,
                bridge: None,
                separator_half_angle: w.separator_half_angle,
                nested_content: w.nested_content.clone(),
                hub: w.hub.clone(),
                segments: seg_visuals,
            });
        }

        // 2. Retracting rings (at most one per level, total visible ring records <= 6)
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
                        pop: 0.0,
                        selection: SelectionVisual::None,
                        on_path: false,
                        enabled: seg.enabled,
                        has_children: seg.has_children,
                        label: seg.label.clone(),
                        icon: seg.icon.clone(),
                    });
                }
                self.scene.wheels.push(WheelVisual {
                    kind: WheelVisualKind::ConcentricRing,
                    center: rw.placement.center,
                    r_in: rw.r_in,
                    r_out: rw.r_out,
                    scale: 1.0,
                    origin: rw.origin,
                    opacity: rw.opacity.value(now),
                    role: rw.role,
                    bridge: None,
                    separator_half_angle: rw.separator_half_angle,
                    nested_content: rw.nested_content.clone(),
                    hub: rw.hub.clone(),
                    segments: seg_visuals,
                });
            }
        }

        // 3. Reserved caption box below `Rreserve`
        self.scene.caption = None;
        if matches!(self.phase, SessionPhase::Appearing | SessionPhase::Open)
            && let LayoutState::Concentric(ref state) = self.layout_state
            && let Some(ref menu) = self.menu
        {
            if let Some(ref hov_path) = self.hovered_item
                && let Some(node) = menu.node(hov_path)
            {
                if !self.caption_opacity.done(now) {
                    any_tween_active = true;
                }
                let mut crumbs: SmallVec<[Arc<str>; 6]> = SmallVec::new();
                if hov_path.len() > 1 {
                    let slice = hov_path.as_slice();
                    for len in 1..slice.len() {
                        let prefix = ItemPath::from_slice(&slice[..len]);
                        if let Some(anc) = menu.node(&prefix) {
                            crumbs.push(anc.label.clone());
                        }
                    }
                } else if self.wheels.len() > 1 {
                    for w in &self.wheels[1..] {
                        if let Some(anc) = menu.node(&w.path) {
                            crumbs.push(anc.label.clone());
                        }
                    }
                }
                let breadcrumb = if crumbs.is_empty() {
                    None
                } else {
                    Some(crumbs)
                };
                let (eff_enabled, eff_disabled_reason) = effective_node_state(menu, hov_path);
                let detail = if eff_enabled {
                    node.description.clone()
                } else {
                    eff_disabled_reason
                };
                self.scene.caption = Some(CaptionVisual {
                    box_: state.plan.caption_box,
                    align: CaptionAlign::Below,
                    breadcrumb,
                    title: node.label.clone(),
                    detail,
                    disabled: !eff_enabled,
                    opacity: self.caption_opacity.value(now),
                });
            } else if self.hovered_item.is_none()
                && self.wheels.len() > 1
                && !state.interaction_suspended
            {
                if !self.caption_opacity.done(now) {
                    any_tween_active = true;
                }
                let mut crumbs: SmallVec<[Arc<str>; 6]> = SmallVec::new();
                for w in &self.wheels[1..] {
                    if let Some(anc) = menu.node(&w.path) {
                        crumbs.push(anc.label.clone());
                    }
                }
                if let Some(title) = crumbs.last().cloned() {
                    self.scene.caption = Some(CaptionVisual {
                        box_: state.plan.caption_box,
                        align: CaptionAlign::Below,
                        breadcrumb: Some(crumbs),
                        title,
                        detail: Some(self.settled_path_hint.clone()),
                        disabled: false,
                        opacity: self.caption_opacity.value(now),
                    });
                }
            }
        }

        self.animating_flag = any_tween_active;
    }
}

fn menu_path_and_ancestors_enabled(menu: &MenuTree, path: &ItemPath) -> bool {
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

fn effective_node_state(menu: &MenuTree, path: &ItemPath) -> (bool, Option<Arc<str>>) {
    let Some(node) = menu.node(path) else {
        return (false, None);
    };
    if !node.enabled {
        return (false, node.disabled_reason.clone());
    }
    let slice = path.as_slice();
    for len in 1..slice.len() {
        let prefix = ItemPath::from_slice(&slice[..len]);
        if let Some(anc) = menu.node(&prefix)
            && !anc.enabled
        {
            return (
                false,
                node.disabled_reason
                    .clone()
                    .or_else(|| anc.disabled_reason.clone()),
            );
        }
    }
    (true, None)
}

fn select_initial_layout(
    raw_cfg: &WheelConfig,
    validated_cfg: &WheelConfig,
    anchor: Point,
    viewport: Viewport,
    root_item_count: usize,
    now: Millis,
) -> (LayoutSelection, LayoutState) {
    match validated_cfg.layout {
        WheelLayout::Branching => (LayoutSelection::branching(), LayoutState::Branching),
        WheelLayout::Concentric => match plan_concentric(anchor, viewport, raw_cfg) {
            Ok(plan) => (
                LayoutSelection::concentric(),
                LayoutState::Concentric(ConcentricState::new(plan, root_item_count, now)),
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
    is_concentric: bool,
) {
    let spans = root_segment_spans(children.len());
    root_wheel.segments.clear();
    let sep = if is_concentric {
        Some(separator_half_angle_for_ring(
            root_wheel.r_in,
            children.len(),
        ))
    } else {
        None
    };
    let mut nested_content = if is_concentric {
        Some(SmallVec::new())
    } else {
        None
    };
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
        if let (Some(s_half), Some(nc)) = (sep, nested_content.as_mut()) {
            nc.push(compute_sector_content(
                root_wheel.placement.center,
                root_wheel.r_in,
                root_wheel.r_out,
                sp,
                s_half,
                children.len(),
                node.icon.is_some(),
            ));
        }
    }
    root_wheel.separator_half_angle = sep;
    root_wheel.nested_content = nested_content;
    root_wheel.hit_geom.spans = spans;
    root_wheel.hit_geom.hit_radius = root_wheel.r_out + cfg.hit_margin;
}
