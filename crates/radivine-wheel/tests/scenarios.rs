//! Scenario tests for `radivine-wheel` using the deterministic script DSL (`TESTING_STRATEGY.md` §3).

use std::sync::Arc;

use radivine_wheel::geometry::angle_diff;
use radivine_wheel::layout::{EDGE_MARGIN, MIN_ANCESTOR_SEP};
use radivine_wheel::{
    DismissReason, DropTarget, InputMode, InvokeSelect, ItemPath, MenuNode, MenuTree, Millis,
    NavKey, Point, SubmenuOpen, Viewport, WheelConfig, WheelEvent, WheelHitGeometry, WheelInput,
    WheelSession, Zone,
};

#[inline]
const fn t(ms: Millis) -> Millis {
    ms
}

/// Helper for asserting drop targets in the DSL.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ExpectedDrop<'a> {
    Accept(&'a [u8]),
    Reject,
}

use ExpectedDrop::{Accept, Reject};

/// Geometry query helper over a live [`WheelSession`] (`TESTING_STRATEGY.md` §3).
struct GeomHelper<'a> {
    session: &'a WheelSession,
}

struct PointBuilder {
    origin: Point,
    r_in: f32,
    r_out: f32,
    a0: f32,
    a1: f32,
}

impl PointBuilder {
    /// Computes a point inside the segment that lies toward `target` (for diagonal intent-triangle travel).
    fn toward(&self, target: Point) -> Point {
        let dir = self.origin.angle_to(target);
        let mid = (self.a0 + self.a1) * 0.5;
        let span = self.a1 - self.a0;
        let theta = if angle_diff(dir, mid) < 0.0 {
            self.a0 + 0.22 * span
        } else {
            self.a1 - 0.22 * span
        };
        let r = self.r_in + 0.60 * (self.r_out - self.r_in);
        self.origin.offset_polar(r, theta)
    }
}

impl<'a> GeomHelper<'a> {
    fn item_center(&self, path: &[u8]) -> Point {
        self.session
            .item_center(&ItemPath::from_slice(path))
            .expect("expected item_center for open wheel path")
    }

    fn wheel(&self, path: &[u8]) -> &WheelHitGeometry {
        self.session
            .wheel_geometry(&ItemPath::from_slice(path))
            .expect("expected wheel geometry for open wheel path")
    }

    fn point_in(&self, zone: Zone) -> PointBuilder {
        match zone {
            Zone::Item { wheel, index } => {
                let st = self.session.debug_state();
                let w = &st.wheels[wheel as usize];
                let sp = w.spans[index as usize];
                PointBuilder {
                    origin: w.center,
                    r_in: w.r_in,
                    r_out: w.r_out,
                    a0: sp.a0,
                    a1: sp.a1,
                }
            }
            _ => panic!("point_in only supports Zone::Item"),
        }
    }
}

/// Deterministic test script runner (`TESTING_STRATEGY.md` §3).
struct Script {
    menu: Option<MenuTree>,
    cfg: WheelConfig,
    mode: InputMode,
    viewport: Viewport,
    session: Option<WheelSession>,
    now: Millis,
    events: Vec<WheelEvent>,
}

fn script(menu: MenuTree) -> Script {
    Script {
        menu: Some(menu),
        cfg: WheelConfig::default(),
        mode: InputMode::Drag,
        viewport: Viewport::new(1920.0, 1080.0, 1.0),
        session: None,
        now: 0,
        events: Vec::new(),
    }
}

impl Script {
    fn with_config(mut self, cfg: WheelConfig) -> Self {
        self.cfg = cfg;
        self
    }

    fn with_mode(mut self, mode: InputMode) -> Self {
        self.mode = mode;
        self
    }

    fn summon_at(mut self, x: f32, y: f32, at: Millis) -> Self {
        self.now = at;
        self.session = Some(WheelSession::new(
            self.menu.take(),
            Point::new(x, y),
            self.viewport,
            self.mode,
            &self.cfg,
            at,
        ));
        self
    }

    fn session(&self) -> &WheelSession {
        self.session.as_ref().expect("session must be summoned")
    }

    fn session_mut(&mut self) -> &mut WheelSession {
        self.session.as_mut().expect("session must be summoned")
    }

    fn geom(&self) -> GeomHelper<'_> {
        GeomHelper {
            session: self.session(),
        }
    }

    fn move_to(mut self, x: f32, y: f32, at: Millis) -> Self {
        self.now = at;
        let mut ev = Vec::new();
        self.session_mut()
            .handle(WheelInput::PointerMoved(Point::new(x, y)), at, &mut ev);
        self.events.extend(ev);
        self
    }

    fn move_to_pt(self, p: Point, at: Millis) -> Self {
        self.move_to(p.x, p.y, at)
    }

    fn wait_until(mut self, at: Millis) -> Self {
        self.now = at;
        let mut ev = Vec::new();
        self.session_mut().tick(at, &mut ev);
        self.events.extend(ev);
        self
    }

    fn input(mut self, inp: WheelInput, at: Millis) -> Self {
        self.now = at;
        let mut ev = Vec::new();
        self.session_mut().handle(inp, at, &mut ev);
        self.events.extend(ev);
        self
    }

    fn expect_hover(self, expected: Option<&[u8]>) -> Self {
        let actual = self.session().debug_state().hovered_item;
        let exp_path = expected.map(ItemPath::from_slice);
        assert_eq!(
            actual, exp_path,
            "at t={}: expected hover {:?}, got {:?}",
            self.now, exp_path, actual
        );
        self
    }

    fn expect_open(self, deepest_submenu_path: &[u8]) -> Self {
        let chain = self.session().debug_state().open_chain;
        let deepest = chain.last().expect("at least root wheel must be open");
        assert_eq!(
            deepest.as_slice(),
            deepest_submenu_path,
            "at t={}: expected deepest open wheel {:?}, got chain {:?}",
            self.now,
            deepest_submenu_path,
            chain
        );
        self
    }

    fn expect_drop_target(self, expected: ExpectedDrop<'_>) -> Self {
        let actual = self.session().drop_target();
        match expected {
            Accept(slice) => assert_eq!(
                actual,
                DropTarget::Accept(ItemPath::from_slice(slice)),
                "at t={}: expected Accept({:?}), got {:?}",
                self.now,
                slice,
                actual
            ),
            Reject => assert_eq!(
                actual,
                DropTarget::Reject,
                "at t={}: expected Reject, got {:?}",
                self.now,
                actual
            ),
        }
        self
    }
}

/// Standard demonstration menu from `TESTING_STRATEGY.md` §3 and `IMPLEMENTATION_PLAN.md` §3.3.
fn demo_menu() -> MenuTree {
    MenuTree::new(vec![
        MenuNode::submenu(
            "radivine.preview",
            "File Preview",
            vec![
                MenuNode::leaf("radivine.preview.full", "Full Preview").with_icon("lucide:eye"),
                MenuNode::leaf("radivine.preview.head", "Head")
                    .with_icon("lucide:arrow-up-to-line"),
                MenuNode::leaf("radivine.preview.tail", "Tail")
                    .with_icon("lucide:arrow-down-to-line"),
            ],
        )
        .with_icon("lucide:eye")
        .with_description("Preview file contents"),
        MenuNode::leaf("radivine.copy", "Copy")
            .with_icon("lucide:copy")
            .with_description("Copy file to clipboard"),
        MenuNode::leaf("radivine.properties", "Properties")
            .with_icon("lucide:info")
            .with_description("Show file properties"),
        MenuNode::submenu(
            "radivine.custom",
            "Custom Actions",
            vec![
                MenuNode::leaf("custom.terminal", "Open in Terminal").with_icon("lucide:terminal"),
                MenuNode::submenu(
                    "custom.scripts",
                    "Scripts",
                    vec![
                        MenuNode::leaf("custom.scripts.sha256", "Compute SHA-256")
                            .with_icon("lucide:code"),
                        MenuNode::leaf("custom.scripts.chmod", "Make Executable")
                            .with_icon("lucide:sparkles"),
                    ],
                )
                .with_icon("lucide:code"),
            ],
        )
        .with_icon("lucide:sparkles")
        .with_description("Run custom tools"),
    ])
}

#[test]
fn test_normative_dsl_walkthrough_from_testing_strategy() {
    // Exact script from TESTING_STRATEGY.md §3 lines 56-66:
    script(demo_menu())
        .summon_at(500.0, 400.0, t(0))
        .move_to(500.0, 330.0, t(20)) // d = 70 < 88.7 -> hover item 0, dwell needed
        .expect_hover(Some(&[0]))
        .wait_until(t(170))
        .expect_open(&[0]) // dwell 140 ms -> child open
        .move_to(500.0, 270.0, t(190)) // outside root hit disc; child d = 92, angle 90° -> Bridge(child)
        .expect_open(&[0])
        .expect_hover(Some(&[0]))
        .move_to(500.0, 200.0, t(210)) // child hub (d = 22 < 30) -> child stays open
        .move_to(500.0, 122.0, t(230)) // child d = 56, angle 270° -> "Head"
        .expect_hover(Some(&[0, 1]))
        .expect_drop_target(Accept(&[0, 1]));
}

// Scenario 1: Dwell opens; band opens immediately; `submenu_open = "click"` never auto-opens.
#[test]
fn scenario_01_dwell_band_and_click_modes() {
    // 1a. Dwell opens after 140 ms (inner band d = 70 < 88.7)
    script(demo_menu())
        .summon_at(500.0, 400.0, t(0))
        .move_to(500.0, 330.0, t(10))
        .expect_hover(Some(&[0]))
        .expect_open(&[])
        .wait_until(t(149))
        .expect_open(&[])
        .wait_until(t(150))
        .expect_open(&[0]);

    // 1b. Outer band (d >= 88.7 with outward motion) opens immediately without waiting for dwell
    script(demo_menu())
        .summon_at(500.0, 400.0, t(0))
        .move_to(500.0, 300.0, t(10)) // d = 100 >= 88.7, outward from (500, 400)
        .expect_hover(Some(&[0]))
        .expect_open(&[0]);

    // 1c. `submenu_open = Click` never auto-opens on dwell or outer band, only on click
    let click_cfg = WheelConfig {
        submenu_open: SubmenuOpen::Click,
        ..WheelConfig::default()
    };
    script(demo_menu())
        .with_config(click_cfg)
        .with_mode(InputMode::Pointer)
        .summon_at(500.0, 400.0, t(0))
        .move_to(500.0, 300.0, t(10)) // outer band
        .expect_open(&[])
        .wait_until(t(500)) // well past dwell
        .expect_open(&[])
        .input(WheelInput::Press, t(510))
        .input(WheelInput::Release, t(520))
        .expect_open(&[0]);
}

// Scenario 2: Diagonal travel through a neighbouring segment keeps the child (intent triangle);
// dwelling on the neighbour closes it after `grace_ms`.
#[test]
fn scenario_02_intent_triangle_diagonal_travel_and_grace_expiry() {
    // Summon near the top edge (500, 140) so child of item 0 ("File Preview", top) rotates clockwise toward item 1 ("Copy", right)
    let mut s = script(demo_menu())
        .summon_at(500.0, 140.0, t(0))
        .move_to(500.0, 70.0, t(10)) // inside item 0
        .wait_until(t(160))
        .expect_open(&[0]);

    let child_center = s.geom().wheel(&[0]).center;
    // Compute a point inside neighbour segment Item(0, 1) that is inside the intent triangle toward child_center
    let diag_pt = s
        .geom()
        .point_in(Zone::Item { wheel: 0, index: 1 })
        .toward(child_center);

    // Move diagonally into Item(0, 1) toward the child at t=180 (within 300 ms grace):
    // Child [0] MUST stay open and item [0] MUST stay hovered!
    s = s
        .move_to_pt(diag_pt, t(180))
        .expect_open(&[0])
        .expect_hover(Some(&[0]));

    // Now if the user stops and dwells on that neighbour past grace_ms (180 + 300 = 480 ms),
    // the child closes and hover switches to the neighbour Item(0, 1) ("Copy")!
    s.wait_until(t(485))
        .expect_open(&[])
        .expect_hover(Some(&[1]))
        .expect_drop_target(Accept(&[1]));
}

// Scenario 3: Hub entry closes children after `hub_close_ms`; quick pass-through hub does not.
#[test]
fn scenario_03_hub_entry_closes_children_after_timeout() {
    // Quick pass-through root hub (< 150 ms) keeps child open
    let s = script(demo_menu())
        .summon_at(500.0, 400.0, t(0))
        .move_to(500.0, 300.0, t(10)) // open [0] via outer band
        .expect_open(&[0])
        .move_to(500.0, 400.0, t(30)) // enter root hub
        .wait_until(t(120)) // 90 ms < 150 ms
        .expect_open(&[0])
        .move_to(500.0, 320.0, t(130)) // back into item 0 before hub_close_ms
        .wait_until(t(250))
        .expect_open(&[0]);

    // Dwelling in root hub >= 150 ms closes child [0]
    s.move_to(500.0, 400.0, t(260))
        .wait_until(t(409))
        .expect_open(&[0])
        .wait_until(t(415))
        .expect_open(&[]);
}

// Scenario 4: Outside keeps the chain; re-entry restores hover.
#[test]
fn scenario_04_outside_keeps_chain_and_reentry_restores_hover() {
    let s = script(demo_menu())
        .summon_at(500.0, 400.0, t(0))
        .move_to(500.0, 300.0, t(10)) // open [0]
        .expect_open(&[0]);

    let head_pt = s.geom().item_center(&[0, 1]);
    s.move_to_pt(head_pt, t(30))
        .expect_hover(Some(&[0, 1]))
        .move_to(50.0, 50.0, t(50)) // far outside all wheels
        .expect_open(&[0]) // chain stays open!
        .expect_hover(None)
        .expect_drop_target(Reject)
        .wait_until(t(1000)) // even after a long time, engine keeps chain open
        .expect_open(&[0])
        .move_to_pt(head_pt, t(1020)) // re-entry restores hover immediately
        .expect_hover(Some(&[0, 1]))
        .expect_drop_target(Accept(&[0, 1]));
}

// Scenario 5: Ancestor item switch truncates deeper wheels.
#[test]
fn scenario_05_ancestor_item_switch_truncates_deeper_wheels() {
    // Open 3-level chain: Root [3] ("Custom Actions") -> Child [3, 1] ("Scripts") -> Grandchild
    let mut s = script(demo_menu()).summon_at(600.0, 500.0, t(0));
    let p_custom = s.geom().item_center(&[3]);
    s = s
        .move_to_pt(p_custom, t(10))
        .wait_until(t(160))
        .expect_open(&[3]);

    let p_scripts = s.geom().item_center(&[3, 1]);
    s = s
        .move_to_pt(p_scripts, t(180))
        .wait_until(t(330))
        .expect_open(&[3, 1]);

    // Now move back to root item 1 ("Copy"): both [3, 1] and [3] must close!
    let p_copy = s.geom().item_center(&[1]);
    s.move_to_pt(p_copy, t(350))
        .expect_open(&[])
        .expect_hover(Some(&[1]))
        .expect_drop_target(Accept(&[1]));
}

// Scenario 6: Activation slop: no hover/drop before 6 lp of motion.
#[test]
fn scenario_06_activation_slop_blocks_hover_and_drop() {
    // Summon near top-left corner (10, 10) so root center is clamped to (124, 124).
    // Even though (12, 12) is far from root center, it is only 2.83 lp (< 6 lp) from summon point (10, 10).
    let mut s = script(demo_menu())
        .summon_at(500.0, 400.0, t(0))
        .move_to(500.0, 395.0, t(10)) // 5.0 lp < 6.0 lp slop
        .expect_hover(None)
        .expect_drop_target(Reject);

    assert!(!s.session().debug_state().moved_past_slop);

    // Move to 6.0 lp from summon point -> crosses slop, enters root hub (still no item hover)
    s = s.move_to(500.0, 394.0, t(20)).expect_hover(None);
    assert!(s.session().debug_state().moved_past_slop);

    // Move into item 1 ("Copy") -> hovered and droppable
    let p_copy = s.geom().item_center(&[1]);
    s.move_to_pt(p_copy, t(30))
        .expect_hover(Some(&[1]))
        .expect_drop_target(Accept(&[1]));
}

// Scenario 7: Drop target always equals hover; disabled -> Reject; submenu with `default_child` -> Accept(default).
#[test]
fn scenario_07_drop_target_matches_hover_disabled_and_default_child() {
    let menu = MenuTree::new(vec![
        MenuNode::submenu(
            "sub.with.default",
            "Preview",
            vec![
                MenuNode::leaf("preview.full", "Full Preview"),
                MenuNode::leaf("preview.head", "Head"),
            ],
        )
        .with_default_child(0),
        MenuNode::leaf("leaf.enabled", "Copy"),
        MenuNode::leaf("leaf.disabled", "Delete").with_disabled("Read-only filesystem"),
        MenuNode::submenu(
            "sub.no.default",
            "More",
            vec![MenuNode::leaf("more.a", "Action A")],
        ),
    ]);

    let mut s = script(menu).summon_at(500.0, 400.0, t(0));
    let p0 = s.geom().item_center(&[0]);
    let p1 = s.geom().item_center(&[1]);
    let p2 = s.geom().item_center(&[2]);
    let p3 = s.geom().item_center(&[3]);

    // Item 0: submenu with default_child = Some(0) -> Accept(&[0, 0]) even before child wheel opens!
    s = s
        .move_to_pt(p0, t(10))
        .expect_hover(Some(&[0]))
        .expect_drop_target(Accept(&[0, 0]));

    // Item 1: enabled leaf -> Accept(&[1])
    s = s
        .move_to_pt(p1, t(20))
        .expect_hover(Some(&[1]))
        .expect_drop_target(Accept(&[1]));

    // Item 2: disabled leaf -> hovered (for caption reason), but drop_target is Reject
    s = s
        .move_to_pt(p2, t(30))
        .expect_hover(Some(&[2]))
        .expect_drop_target(Reject);
    assert!(s.session().scene().caption.as_ref().unwrap().disabled);

    // Item 3: submenu without default_child -> Reject
    s.move_to_pt(p3, t(40))
        .expect_hover(Some(&[3]))
        .expect_drop_target(Reject);
}

// Scenario 8: `set_enabled` during hover updates drop target without layout change.
#[test]
fn scenario_08_set_enabled_updates_drop_target_without_layout_change() {
    let mut s = script(demo_menu()).summon_at(500.0, 400.0, t(0));
    let p_copy = s.geom().item_center(&[1]);
    s = s
        .move_to_pt(p_copy, t(10))
        .expect_hover(Some(&[1]))
        .expect_drop_target(Accept(&[1]));

    let before_center = s.session().scene().wheels[0].center;
    let before_seg_a0 = s.session().scene().wheels[0].segments[1].a0;

    // Disable item [1] while it is hovered (without any pointer motion)
    s.session_mut().set_enabled(
        &ItemPath::from_slice(&[1]),
        false,
        Some(Arc::from("Permission denied")),
    );
    s = s.expect_drop_target(Reject);
    assert_eq!(s.session().scene().wheels[0].center, before_center);
    assert_eq!(s.session().scene().wheels[0].segments[1].a0, before_seg_a0);
    assert_eq!(
        s.session()
            .scene()
            .caption
            .as_ref()
            .unwrap()
            .detail
            .as_deref(),
        Some("Permission denied")
    );

    // Re-enable item [1] -> drop_target returns to Accept(&[1]) without pointer motion
    s.session_mut()
        .set_enabled(&ItemPath::from_slice(&[1]), true, None);
    s.expect_drop_target(Accept(&[1]));
}

// Scenario 9: Keyboard: next/prev wrap, activate leaf -> `Commit`, activate submenu -> focus first child, back, digit, cancel.
#[test]
fn scenario_09_keyboard_navigation_complete() {
    let s = script(demo_menu())
        .with_mode(InputMode::Pointer)
        .summon_at(500.0, 400.0, t(0))
        // Next focuses item 0
        .input(WheelInput::Nav(NavKey::Next), t(10))
        .expect_hover(Some(&[0]))
        // Prev wraps around to item 3
        .input(WheelInput::Nav(NavKey::Prev), t(20))
        .expect_hover(Some(&[3]))
        // Next wraps back to item 0 ("File Preview" submenu)
        .input(WheelInput::Nav(NavKey::Next), t(30))
        .expect_hover(Some(&[0]))
        // Activate on submenu opens child wheel [0] AND focuses its first item [0, 0]
        .input(WheelInput::Nav(NavKey::Activate), t(40))
        .expect_open(&[0])
        .expect_hover(Some(&[0, 0]))
        // Back closes child [0] and returns focus to parent item [0]
        .input(WheelInput::Nav(NavKey::Back), t(50))
        .expect_open(&[])
        .expect_hover(Some(&[0]))
        // Cancel emits CancelRequested
        .input(WheelInput::Nav(NavKey::Cancel), t(60));

    assert!(s.events.contains(&WheelEvent::CancelRequested));

    // Digit(2) on root wheel activates 2nd item (index 1 = "Copy" leaf) -> emits Commit([1])
    let s2 = script(demo_menu())
        .with_mode(InputMode::Pointer)
        .summon_at(500.0, 400.0, t(0))
        .input(WheelInput::Nav(NavKey::Digit(2)), t(10));

    assert!(
        s2.events
            .contains(&WheelEvent::Commit(ItemPath::from_slice(&[1])))
    );
    let _ = InvokeSelect::Click;
}

// Scenario 10: Dismiss during child emerge animation finishes cleanly (`Finished` emitted once).
#[test]
fn scenario_10_dismiss_during_child_emerge_emits_finished_once() {
    let mut s = script(demo_menu())
        .summon_at(500.0, 400.0, t(0))
        .move_to(500.0, 300.0, t(10)) // opens child [0] via outer band at t=10
        .expect_open(&[0])
        .wait_until(t(60)); // mid child-emerge animation (emerge runs t=50..180)

    assert!(s.session().is_animating());
    s.session_mut().dismiss(DismissReason::Cancelled, t(60));
    assert!(s.session().input_region().is_empty());

    // Advance past dismiss duration (100 ms -> t=160) and tick multiple times
    s = s.wait_until(t(120)).wait_until(t(170)).wait_until(t(300));
    assert!(s.session().is_finished());

    let finished_count = s
        .events
        .iter()
        .filter(|e| matches!(e, WheelEvent::Finished))
        .count();
    assert_eq!(finished_count, 1, "Finished must be emitted exactly once");
}

// Scenario 11: Placement near all corners for 2-level and 3-level menus.
#[test]
fn scenario_11_placement_near_all_corners_2_and_3_levels() {
    let corners = [
        (20.0, 20.0),
        (1900.0, 20.0),
        (20.0, 1060.0),
        (1900.0, 1060.0),
    ];

    for (cx, cy) in corners {
        let mut s = script(demo_menu()).summon_at(cx, cy, t(0));
        let root_center_initial = s.session().scene().wheels[0].center;

        // Open level 1 child [3] ("Custom Actions")
        let p_custom = s.geom().item_center(&[3]);
        s = s
            .move_to_pt(p_custom, t(10))
            .wait_until(t(160))
            .expect_open(&[3]);

        // Open level 2 grandchild [3, 1] ("Scripts")
        let p_scripts = s.geom().item_center(&[3, 1]);
        s = s
            .move_to_pt(p_scripts, t(180))
            .wait_until(t(340))
            .expect_open(&[3, 1]);

        let scene = s.session().scene();
        assert_eq!(scene.wheels.len(), 3);
        // Root wheel must never move after summon
        assert_eq!(scene.wheels[0].center, root_center_initial);

        // Every wheel must lie within viewport bounds with EDGE_MARGIN
        let vp_bounds = s.viewport.bounds;
        for (i, w) in scene.wheels.iter().enumerate() {
            assert!(
                vp_bounds.contains_disc(w.center, w.r_out + EDGE_MARGIN - 1e-2),
                "corner ({cx},{cy}) wheel {i} at {:?} r={} outside viewport",
                w.center,
                w.r_out
            );
        }

        // Grandchild (wheel 2) must not overlap grandparent (wheel 0)
        let d_0_2 = scene.wheels[0].center.distance(scene.wheels[2].center);
        let min_dist = scene.wheels[0].r_out + scene.wheels[2].r_out + MIN_ANCESTOR_SEP - 1e-2;
        assert!(
            d_0_2 >= min_dist,
            "corner ({cx},{cy}): grandchild overlaps root: d={d_0_2} < {min_dist}"
        );
    }
}
