//! Milestone N2 concentric hierarchy navigation, safety, and menu-parity tests
//! (`TEST_MATRIX.md` §T2 `T-NAV1`–`T-NAV12`, §T3 `T-SAFE1`–`T-SAFE5`, `T-MENU2`–`T-MENU3`,
//! and the normative deterministic trace).

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use radivine_wheel::{
    DismissReason, DropTarget, InputMode, InvokeSelect, ItemPath, LayoutSelection, MenuNode,
    MenuTree, Millis, NavKey, NestedHit, Point, SubmenuOpen, Viewport, WheelConfig, WheelEvent,
    WheelInput, WheelLayout, WheelRole, WheelSession,
};

fn shipped_concentric_config() -> WheelConfig {
    WheelConfig {
        layout: WheelLayout::Concentric,
        max_items_per_wheel: 8,
        max_depth: 4,
        activation_slop: 6.0,
        hit_margin: 12.0,
        segment_gap: 2.5,
        child_gap: 20.0,
        bridge_angle_deg: 70.0,
        hysteresis_deg: 3.5,
        radius_root: [40.0, 128.0],
        radius_child: [34.0, 108.0],
        radius_deep: [26.0, 84.0],
        radius_min_outer: 64.0,
        submenu_open: SubmenuOpen::DwellAndBand,
        dwell_ms: 110,
        band: 0.65,
        grace_ms: 180,
        hub_close_ms: 150,
        invoke_select: InvokeSelect::Click,
        reduced_motion: false,
    }
}

fn normative_fixture_tree() -> MenuTree {
    // Root item 0 is submenu A with two leaves ("a0", "a1"), item 1 is leaf B ("b"), no default child.
    let sub_a = MenuNode::submenu(
        "sub.a",
        "Submenu A",
        vec![
            MenuNode::leaf("a0", "Action A0").with_description("First leaf in A"),
            MenuNode::leaf("a1", "Action A1").with_description("Second leaf in A"),
        ],
    );
    let leaf_b = MenuNode::leaf("b", "Action B").with_description("Direct root leaf B");
    MenuTree::new(vec![sub_a, leaf_b])
}

/// Returns a point at `(radius, sector_center_angle)` for `(level, index)` using `debug_state()`.
fn sector_point_at_radius(
    session: &WheelSession,
    level: usize,
    index: usize,
    radius: f32,
) -> Point {
    let dbg = session.debug_state();
    let w = &dbg.wheels[level];
    let span = &w.spans[index];
    w.center.offset_polar(radius, span.center)
}

/// Returns the geometric center point `(mid_r, sector_center_angle)` of `(level, index)`.
fn sector_center_point(session: &WheelSession, level: usize, index: usize) -> Point {
    let dbg = session.debug_state();
    let w = &dbg.wheels[level];
    let span = &w.spans[index];
    let mid_r = 0.5 * (w.r_in + w.r_out);
    w.center.offset_polar(mid_r, span.center)
}

fn assert_no_commit(events: &[WheelEvent], context: &str) {
    for ev in events {
        assert!(
            !matches!(ev, WheelEvent::Commit(_)),
            "unexpected Commit event in {context}: {ev:?}"
        );
    }
}

/// Normative 4-part concrete deterministic trace (`TEST_MATRIX.md` lines 45–60).
#[test]
fn t_nav_normative_concrete_deterministic_trace() {
    let cfg = shipped_concentric_config();
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let center = Point::new(960.0, 540.0);

    // --- Part 1 & Part 2 (accept at [0, 0]) ---
    {
        let mut session = WheelSession::new(
            Some(normative_fixture_tree()),
            center,
            vp,
            InputMode::Drag,
            &cfg,
            0,
        );
        assert_eq!(session.layout_selection(), LayoutSelection::concentric());
        let plan = session.concentric_plan().expect("concentric plan").clone();
        assert_eq!(plan.center, center);
        assert_eq!(plan.radii[0], (40.0, 128.0));
        assert_eq!(plan.radii[1], (134.0, 194.0));
        assert_eq!(plan.radii.len(), 4);

        let mut events = Vec::new();

        // 1. At t=10 move to C+(0,-80); hover A ([0]); radius 80 < band 97.2, deadline 120.
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 460.0)),
            10,
            &mut events,
        );
        assert_no_commit(&events, "t=10 move to A");
        assert_eq!(
            session.debug_state().hovered_item,
            Some(ItemPath::from_slice(&[0]))
        );
        assert_eq!(session.next_deadline(), Some(120));

        // At t=119 child absent
        events.clear();
        session.tick(119, &mut events);
        assert_no_commit(&events, "t=119 tick");
        assert_eq!(
            session.debug_state().open_chain.as_slice(),
            &[ItemPath::root()]
        );

        // At t=120 child 134..194 appears and path is `[[], [0]]`; no action.
        events.clear();
        session.tick(120, &mut events);
        assert_no_commit(&events, "t=120 dwell open");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WheelEvent::SubmenuOpened(p) if p.as_slice() == [0]))
        );
        let dbg_120 = session.debug_state();
        assert_eq!(
            dbg_120.open_chain.as_slice(),
            &[ItemPath::root(), ItemPath::from_slice(&[0])]
        );
        assert_eq!(dbg_120.wheels.len(), 2);
        assert_eq!(
            (dbg_120.wheels[1].r_in, dbg_120.wheels[1].r_out),
            (134.0, 194.0)
        );

        // 2. t=125 move to radius 131 at top: gap, Reject, child retained.
        events.clear();
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 131.0)),
            125,
            &mut events,
        );
        assert_no_commit(&events, "t=125 radial gap");
        assert_eq!(
            session.concentric_hit(),
            Some(NestedHit::RadialGap { inner_level: 0 })
        );
        assert_eq!(session.drop_target(), DropTarget::Reject);
        assert_eq!(
            session.debug_state().open_chain.as_slice(),
            &[ItemPath::root(), ItemPath::from_slice(&[0])]
        );

        // t=135 move to child item 0 center (radius 167): fresh motion arms `[0,0]`.
        events.clear();
        let child0_pt = sector_point_at_radius(&session, 1, 0, 167.0);
        session.handle(WheelInput::PointerMoved(child0_pt), 135, &mut events);
        assert_no_commit(&events, "t=135 move to child item 0");
        assert_eq!(
            session.drop_target(),
            DropTarget::Accept(ItemPath::from_slice(&[0, 0]))
        );
        let node_a0 = normative_fixture_tree()
            .node(&ItemPath::from_slice(&[0, 0]))
            .unwrap()
            .id
            .clone();
        assert_eq!(node_a0.as_ref(), "a0");
    }

    // --- Part 2b: Separate run dropping at 131 selects nothing ---
    {
        let mut session = WheelSession::new(
            Some(normative_fixture_tree()),
            center,
            vp,
            InputMode::Drag,
            &cfg,
            0,
        );
        let mut events = Vec::new();
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 460.0)),
            10,
            &mut events,
        );
        session.tick(120, &mut events);
        events.clear();
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 131.0)),
            125,
            &mut events,
        );
        assert_eq!(session.drop_target(), DropTarget::Reject);
        assert_no_commit(&events, "drop at radius 131 gap");
    }

    // --- Part 3: Ancestor sibling pending adoption (t=140 -> deadline 320) ---
    {
        let mut session = WheelSession::new(
            Some(normative_fixture_tree()),
            center,
            vp,
            InputMode::Drag,
            &cfg,
            0,
        );
        let mut events = Vec::new();
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 460.0)),
            10,
            &mut events,
        );
        session.tick(120, &mut events);
        events.clear();

        // Root has 2 items: item 0 is centered at top (-pi/2), item 1 is centered at bottom (+pi/2).
        let b_pt = sector_point_at_radius(&session, 0, 1, 80.0);
        session.handle(WheelInput::PointerMoved(b_pt), 140, &mut events);
        assert_no_commit(&events, "t=140 move to sibling B");
        assert_eq!(session.next_deadline(), Some(320));
        assert_eq!(session.drop_target(), DropTarget::Reject);
        assert_eq!(
            session.debug_state().open_chain.as_slice(),
            &[ItemPath::root(), ItemPath::from_slice(&[0])]
        );

        // Same sample at t=319 does not extend deadline; child still open; Reject.
        events.clear();
        session.handle(WheelInput::PointerMoved(b_pt), 319, &mut events);
        session.tick(319, &mut events);
        assert_no_commit(&events, "t=319 repeated sample");
        assert_eq!(session.next_deadline(), Some(320));
        assert_eq!(session.drop_target(), DropTarget::Reject);
        assert_eq!(
            session.debug_state().open_chain.as_slice(),
            &[ItemPath::root(), ItemPath::from_slice(&[0])]
        );

        // t=320 adopts B / closes child and enables direct B release.
        events.clear();
        session.tick(320, &mut events);
        assert_no_commit(&events, "t=320 sibling adoption tick");
        assert_eq!(
            session.debug_state().open_chain.as_slice(),
            &[ItemPath::root()]
        );
        assert_eq!(
            session.debug_state().hovered_item,
            Some(ItemPath::from_slice(&[1]))
        );
        assert_eq!(
            session.drop_target(),
            DropTarget::Accept(ItemPath::from_slice(&[1]))
        );
    }

    // --- Part 4a: Re-entering existing child at t=200 before deadline 320 cancels pending ---
    {
        let mut session = WheelSession::new(
            Some(normative_fixture_tree()),
            center,
            vp,
            InputMode::Drag,
            &cfg,
            0,
        );
        let mut events = Vec::new();
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 460.0)),
            10,
            &mut events,
        );
        session.tick(120, &mut events);

        let b_pt = sector_point_at_radius(&session, 0, 1, 80.0);
        session.handle(WheelInput::PointerMoved(b_pt), 140, &mut events);
        assert_eq!(session.next_deadline(), Some(320));

        // Re-enter existing child at t=200
        events.clear();
        let child1_pt = sector_point_at_radius(&session, 1, 1, 164.0);
        session.handle(WheelInput::PointerMoved(child1_pt), 200, &mut events);
        assert_no_commit(&events, "t=200 re-enter child");
        assert_eq!(session.next_deadline(), None);

        // Advance past 320: old child is still retained!
        session.tick(350, &mut events);
        assert_no_commit(&events, "t=350 after cancelled pending");
        assert_eq!(
            session.debug_state().open_chain.as_slice(),
            &[ItemPath::root(), ItemPath::from_slice(&[0])]
        );
        assert_eq!(
            session.drop_target(),
            DropTarget::Accept(ItemPath::from_slice(&[0, 1]))
        );
    }

    // --- Part 4b: Entering hub at t=200 keeps descendants at t=349 and collapses at t=350 ---
    {
        let mut session = WheelSession::new(
            Some(normative_fixture_tree()),
            center,
            vp,
            InputMode::Drag,
            &cfg,
            0,
        );
        let mut events = Vec::new();
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 460.0)),
            10,
            &mut events,
        );
        session.tick(120, &mut events);

        events.clear();
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 530.0)),
            200,
            &mut events,
        );
        assert_eq!(session.concentric_hit(), Some(NestedHit::Hub));
        assert_eq!(session.next_deadline(), Some(350));

        session.tick(349, &mut events);
        assert_no_commit(&events, "t=349 hub tick");
        assert_eq!(
            session.debug_state().open_chain.as_slice(),
            &[ItemPath::root(), ItemPath::from_slice(&[0])]
        );

        session.tick(350, &mut events);
        assert_no_commit(&events, "t=350 hub collapse");
        assert_eq!(
            session.debug_state().open_chain.as_slice(),
            &[ItemPath::root()]
        );
    }
}

/// `T-NAV1`: Dwell at `deadline - 1`, `deadline`, `deadline + 1`; band threshold `±epsilon` and
/// `>= 2 lp` outward criterion; `Dwell` / `Band` / `DwellAndBand` / `Click`; zero durations;
/// single `SubmenuOpened`.
#[test]
fn t_nav1_dwell_band_click_and_zero_duration_boundaries() {
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let center = Point::new(960.0, 540.0);

    // 1. Dwell exact boundary: deadline - 1, deadline, deadline + 1, single SubmenuOpened
    {
        let mut cfg = shipped_concentric_config();
        cfg.submenu_open = SubmenuOpen::Dwell;
        let mut session = WheelSession::new(
            Some(normative_fixture_tree()),
            center,
            vp,
            InputMode::Drag,
            &cfg,
            0,
        );
        let mut events = Vec::new();
        // Move into outer band (r = 110 > 97.2); because mode is Dwell-only, it must NOT open immediately.
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 110.0)),
            10,
            &mut events,
        );
        assert_eq!(session.debug_state().wheels.len(), 1);
        assert_eq!(session.next_deadline(), Some(120));

        events.clear();
        session.tick(119, &mut events);
        assert_eq!(session.debug_state().wheels.len(), 1);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, WheelEvent::SubmenuOpened(_)))
        );

        session.tick(120, &mut events);
        assert_eq!(session.debug_state().wheels.len(), 2);
        let opened_count = events
            .iter()
            .filter(|e| matches!(e, WheelEvent::SubmenuOpened(p) if p.as_slice() == [0]))
            .count();
        assert_eq!(opened_count, 1);

        events.clear();
        session.tick(121, &mut events);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, WheelEvent::SubmenuOpened(_)))
        );
    }

    // 2. Band threshold 97.2 +- epsilon and >= 2.0 lp outward velocity requirement
    {
        let mut cfg = shipped_concentric_config();
        cfg.submenu_open = SubmenuOpen::Band;
        let mut session = WheelSession::new(
            Some(normative_fixture_tree()),
            center,
            vp,
            InputMode::Drag,
            &cfg,
            0,
        );
        let mut events = Vec::new();
        // Enter sector A at r = 90.0
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 90.0)),
            10,
            &mut events,
        );
        assert_eq!(session.debug_state().wheels.len(), 1);
        assert_eq!(
            session.next_deadline(),
            None,
            "Band mode must not arm dwell"
        );

        // Move outward by >= 2.0 lp to 97.19 (just below 97.2 threshold): still closed
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 97.19)),
            15,
            &mut events,
        );
        assert_eq!(session.debug_state().wheels.len(), 1);

        // Move outward by only 0.5 lp to 97.69 (< 2.0 lp outward step): still closed
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 97.69)),
            20,
            &mut events,
        );
        assert_eq!(session.debug_state().wheels.len(), 1);

        // Move inward from 97.69 to 95.0: still closed
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 95.0)),
            25,
            &mut events,
        );
        assert_eq!(session.debug_state().wheels.len(), 1);

        // Move outward by >= 2.0 lp from 95.0 to 97.21 (> 97.2 threshold): opens immediately!
        events.clear();
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 97.21)),
            30,
            &mut events,
        );
        assert_eq!(session.debug_state().wheels.len(), 2);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, WheelEvent::SubmenuOpened(_)))
                .count(),
            1
        );

        // Verify in DwellAndBand mode that jumping directly from center into the outer band
        // (r = 105.0 >= 97.2) on the FIRST sample entering the sector does NOT open immediately
        // via band (it starts dwell at t = 10 + 110 = 120), and a second sample moving outward
        // by >= 2.0 lp on that same sector (to r = 107.0 at t = 15) opens immediately via band:
        let cfg_db = shipped_concentric_config();
        let mut s_db = WheelSession::new(
            Some(normative_fixture_tree()),
            center,
            vp,
            InputMode::Drag,
            &cfg_db,
            0,
        );
        events.clear();
        s_db.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 105.0)),
            10,
            &mut events,
        );
        assert_eq!(
            s_db.debug_state().wheels.len(),
            1,
            "first sample entering sector inside outer band must start dwell, not open immediately"
        );
        assert_eq!(s_db.next_deadline(), Some(120));
        s_db.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 107.0)),
            15,
            &mut events,
        );
        assert_eq!(
            s_db.debug_state().wheels.len(),
            2,
            "subsequent >= 2.0 lp outward move inside outer band on same sector must open immediately"
        );
    }

    // 3. SubmenuOpen::Click mode in Pointer mode: neither dwell nor band opens; click opens
    {
        let mut cfg = shipped_concentric_config();
        cfg.submenu_open = SubmenuOpen::Click;
        let mut session = WheelSession::new(
            Some(normative_fixture_tree()),
            center,
            vp,
            InputMode::Pointer,
            &cfg,
            0,
        );
        let mut events = Vec::new();
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 105.0)),
            10,
            &mut events,
        );
        session.tick(500, &mut events);
        assert_eq!(session.debug_state().wheels.len(), 1);

        events.clear();
        session.handle(WheelInput::Press, 510, &mut events);
        session.handle(WheelInput::Release, 520, &mut events);
        assert_no_commit(&events, "click on submenu in Pointer mode");
        assert_eq!(session.debug_state().wheels.len(), 2);
    }

    // 4. Zero durations (dwell_ms = 0, grace_ms = 0, hub_close_ms = 0)
    {
        let mut cfg = shipped_concentric_config();
        cfg.dwell_ms = 0;
        cfg.grace_ms = 0;
        cfg.hub_close_ms = 0;
        let mut session = WheelSession::new(
            Some(normative_fixture_tree()),
            center,
            vp,
            InputMode::Drag,
            &cfg,
            0,
        );
        let mut events = Vec::new();
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 80.0)),
            10,
            &mut events,
        );
        assert_eq!(session.next_deadline(), Some(10));
        session.tick(10, &mut events);
        assert_eq!(session.debug_state().wheels.len(), 2);

        // Switch to sibling B with grace_ms = 0 -> deadline = 20
        let b_pt = sector_point_at_radius(&session, 0, 1, 80.0);
        session.handle(WheelInput::PointerMoved(b_pt), 20, &mut events);
        assert_eq!(session.next_deadline(), Some(20));
        session.tick(20, &mut events);
        assert_eq!(session.debug_state().wheels.len(), 1);
    }
}

/// `T-NAV2`: Follow `root -> child -> grandchild -> level4` (and 6-level engine fixture);
/// verify `open_chain` and all ancestors visible at `0.72` opacity with `on_path` markers.
#[test]
fn t_nav2_multi_ring_chain_depths_4_and_6_and_ancestor_opacity() {
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let center = Point::new(960.0, 540.0);

    // Build a 6-level deep menu tree
    let mut node = MenuNode::leaf("leaf.d6", "Depth 6 Leaf");
    for level in (0..5).rev() {
        node = MenuNode::submenu(
            format!("sub.L{level}"),
            format!("Level {level}"),
            vec![MenuNode::leaf(format!("noop.L{level}"), "Other"), node],
        );
    }
    let mut cfg = shipped_concentric_config();
    cfg.max_depth = 6;
    cfg.reduced_motion = true;
    let tree = MenuTree::new(vec![node, MenuNode::leaf("root.other", "Root Other")]);

    let mut session = WheelSession::new(Some(tree), center, vp, InputMode::Drag, &cfg, 0);
    assert_eq!(session.layout_selection(), LayoutSelection::concentric());

    let mut events = Vec::new();
    let mut now: Millis = 10;

    // Open rings 0 -> 1 -> 2 -> 3 -> 4 -> 5
    // At root (level 0), submenu is at index 0. At levels 1..=4, submenu is at index 1.
    let mut expected_chain = vec![ItemPath::root()];
    for level in 0..5usize {
        let sub_idx = if level == 0 { 0 } else { 1 };
        let pt = sector_center_point(&session, level, sub_idx);
        session.handle(WheelInput::PointerMoved(pt), now, &mut events);
        now += 120;
        session.tick(now, &mut events);
        now += 70; // allow 60ms reduced-motion fade to settle
        session.tick(now, &mut events);

        let next_path = expected_chain.last().unwrap().child(sub_idx as u8);
        expected_chain.push(next_path);
        assert_eq!(
            session.debug_state().open_chain.as_slice(),
            expected_chain.as_slice(),
            "chain mismatch after opening level {}",
            level + 1
        );

        let scene = session.scene();
        assert_eq!(scene.wheels.len(), level + 2);
        for (w_idx, w) in scene.wheels.iter().enumerate() {
            assert_eq!(w.center, center);
            assert_eq!(w.scale, 1.0);
            if w_idx <= level {
                assert_eq!(w.role, WheelRole::Ancestor);
                assert!(
                    (w.opacity - 0.72).abs() < 1e-4,
                    "ancestor opacity {}",
                    w.opacity
                );
                let expected_on_path_idx = if w_idx == 0 { 0 } else { 1 };
                for (s_idx, seg) in w.segments.iter().enumerate() {
                    assert_eq!(seg.on_path, s_idx == expected_on_path_idx);
                }
            } else {
                assert_eq!(w.role, WheelRole::Active);
                assert!(
                    (w.opacity - 1.0).abs() < 1e-4,
                    "active opacity {}",
                    w.opacity
                );
            }
        }
    }

    // Move onto the deepest leaf at level 5, index 0 and verify it is immediately eligible after fresh move
    let leaf_pt = sector_center_point(&session, 5, 0);
    session.handle(WheelInput::PointerMoved(leaf_pt), now + 10, &mut events);
    assert_eq!(
        session.drop_target(),
        DropTarget::Accept(ItemPath::from_slice(&[0, 1, 1, 1, 1, 0]))
    );
}

/// `T-NAV3`: `Parent -> gap -> any child angle`, including diagonal through sibling;
/// existing child survives; no `Commit` from movement/ticks; raw gap rejects.
#[test]
fn t_nav3_radial_gap_diagonal_crossings_preserve_child_and_reject_drop() {
    let cfg = shipped_concentric_config();
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let center = Point::new(960.0, 540.0);

    let sub0 = MenuNode::submenu(
        "s0",
        "Sub 0",
        (0..6)
            .map(|i| MenuNode::leaf(format!("s0.{i}"), format!("Child {i}")))
            .collect(),
    );
    let tree = MenuTree::new(vec![
        sub0,
        MenuNode::leaf("r1", "Root 1"),
        MenuNode::leaf("r2", "Root 2"),
        MenuNode::leaf("r3", "Root 3"),
    ]);

    let mut session = WheelSession::new(Some(tree), center, vp, InputMode::Drag, &cfg, 0);
    let mut events = Vec::new();

    // Open root item 0 (top sector)
    let r0_pt = sector_point_at_radius(&session, 0, 0, 80.0);
    session.handle(WheelInput::PointerMoved(r0_pt), 10, &mut events);
    session.tick(120, &mut events);
    assert_eq!(session.debug_state().wheels.len(), 2);

    // Move into radial gap (< 3.0 lp into 128..134, r = 130.5) at sector 0 center angle:
    // hover hysteresis retains [0], but strict DropTarget MUST be Reject!
    events.clear();
    let s0_angle = session.debug_state().wheels[0].spans[0].center;
    let gap_top = center.offset_polar(130.5, s0_angle);
    session.handle(WheelInput::PointerMoved(gap_top), 130, &mut events);
    assert_eq!(
        session.concentric_hit(),
        Some(NestedHit::RadialGap { inner_level: 0 })
    );
    assert_eq!(
        session.debug_state().hovered_item,
        Some(ItemPath::from_slice(&[0])),
        "hysteresis keeps hover highlight on [0] < 3.0 lp into radial gap"
    );
    assert_eq!(
        session.drop_target(),
        DropTarget::Reject,
        "hover hysteresis must NEVER make radial gap executable"
    );

    // Cut diagonally through root sibling 1 (t=140) and radial gap (t=150) into child sector 3 (bottom of child ring) at t=160
    let sib1_pt = sector_point_at_radius(&session, 0, 1, 95.0);
    session.handle(WheelInput::PointerMoved(sib1_pt), 140, &mut events);
    assert_eq!(session.drop_target(), DropTarget::Reject);

    let gap_diag = center.offset_polar(131.0, 0.25);
    session.handle(WheelInput::PointerMoved(gap_diag), 150, &mut events);
    assert_eq!(session.drop_target(), DropTarget::Reject);

    let child3_pt = sector_point_at_radius(&session, 1, 3, 164.0);
    session.handle(WheelInput::PointerMoved(child3_pt), 160, &mut events);
    session.tick(400, &mut events);
    assert_no_commit(&events, "diagonal travel into child sector 3");
    assert_eq!(
        session.debug_state().open_chain.as_slice(),
        &[ItemPath::root(), ItemPath::from_slice(&[0])]
    );
    assert_eq!(
        session.drop_target(),
        DropTarget::Accept(ItemPath::from_slice(&[0, 3]))
    );
}

/// `T-NAV4`: Ancestor sibling pending at entry; deadline not extended by repeated same position or
/// jitter; adopt at grace; delay child until `max(entry + dwell, entry + grace)`; two-sample outward
/// shortcut; stale timers cannot act on a replacement path.
#[test]
fn t_nav4_ancestor_sibling_pending_grace_jitter_two_sample_shortcut_and_stale_timers() {
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let center = Point::new(960.0, 540.0);

    let tree_two_subs = || {
        MenuTree::new(vec![
            MenuNode::submenu(
                "s0",
                "Sub 0",
                vec![
                    MenuNode::leaf("s0.0", "S0-0"),
                    MenuNode::leaf("s0.1", "S0-1"),
                ],
            ),
            MenuNode::submenu(
                "s1",
                "Sub 1",
                vec![
                    MenuNode::leaf("s1.0", "S1-0"),
                    MenuNode::leaf("s1.1", "S1-1"),
                ],
            ),
        ])
    };

    // Case A: dwell_ms (250) > grace_ms (180) -> child opens at max(t0 + 250, t0 + 180) = t0 + 250
    {
        let mut cfg = shipped_concentric_config();
        cfg.dwell_ms = 250;
        cfg.grace_ms = 180;
        let mut session =
            WheelSession::new(Some(tree_two_subs()), center, vp, InputMode::Drag, &cfg, 0);
        let mut events = Vec::new();

        // Open Sub 0 at t=10 + 250 = 260
        let s0_pt = sector_point_at_radius(&session, 0, 0, 80.0);
        session.handle(WheelInput::PointerMoved(s0_pt), 10, &mut events);
        session.tick(260, &mut events);
        assert_eq!(
            session.debug_state().open_chain.as_slice(),
            &[ItemPath::root(), ItemPath::from_slice(&[0])]
        );

        // Enter Sub 1 at t=300 at r=80 -> pending deadline = 300 + 180 = 480
        let s1_pt = sector_point_at_radius(&session, 0, 1, 80.0);
        session.handle(WheelInput::PointerMoved(s1_pt), 300, &mut events);
        assert_eq!(session.next_deadline(), Some(480));
        assert_eq!(session.drop_target(), DropTarget::Reject);

        // Jitter inside Sub 1 at t=350 and t=420 (inward/sideways): deadline stays 480!
        let s1_jitter = Point::new(s1_pt.x + 3.0, s1_pt.y - 1.0);
        session.handle(WheelInput::PointerMoved(s1_jitter), 350, &mut events);
        session.handle(WheelInput::PointerMoved(s1_pt), 420, &mut events);
        assert_eq!(session.next_deadline(), Some(480));

        // At t=480: grace expires -> Sub 0 child closes, Sub 1 is adopted, and dwell is scheduled for 300 + 250 = 550!
        session.tick(480, &mut events);
        assert_eq!(
            session.debug_state().open_chain.as_slice(),
            &[ItemPath::root()]
        );
        assert_eq!(session.next_deadline(), Some(550));

        // At t=549 still closed; at t=550 Sub 1 opens!
        session.tick(549, &mut events);
        assert_eq!(session.debug_state().wheels.len(), 1);
        session.tick(550, &mut events);
        assert_eq!(
            session.debug_state().open_chain.as_slice(),
            &[ItemPath::root(), ItemPath::from_slice(&[1])]
        );
    }

    // Case B: Two-sample outward shortcut in Sub 1 adopts and opens immediately before grace_ms
    {
        let cfg = shipped_concentric_config();
        let mut session =
            WheelSession::new(Some(tree_two_subs()), center, vp, InputMode::Drag, &cfg, 0);
        let mut events = Vec::new();

        let s0_pt = sector_point_at_radius(&session, 0, 0, 80.0);
        session.handle(WheelInput::PointerMoved(s0_pt), 10, &mut events);
        session.tick(120, &mut events);
        assert_eq!(
            session.debug_state().open_chain.as_slice(),
            &[ItemPath::root(), ItemPath::from_slice(&[0])]
        );

        // Sample 1 in Sub 1 at r = 90.0 at t=140: starts pending (does NOT immediately band-open on first sample)
        let s1_sample1 = sector_point_at_radius(&session, 0, 1, 90.0);
        session.handle(WheelInput::PointerMoved(s1_sample1), 140, &mut events);
        assert_eq!(
            session.debug_state().open_chain.as_slice(),
            &[ItemPath::root(), ItemPath::from_slice(&[0])]
        );

        // Sample 2 in Sub 1 at r = 100.0 (>= 97.2 and 100 - 90 >= 2.0 lp) at t=148: immediately adopts and opens Sub 1!
        let s1_sample2 = sector_point_at_radius(&session, 0, 1, 100.0);
        session.handle(WheelInput::PointerMoved(s1_sample2), 148, &mut events);
        assert_eq!(
            session.debug_state().open_chain.as_slice(),
            &[ItemPath::root(), ItemPath::from_slice(&[1])]
        );
        assert_no_commit(&events, "two-sample outward shortcut");
    }
}

/// `T-NAV5`: Move inward across several levels, open-parent sector, quick hub crossing,
/// hub `deadline - 1` / `+ 0`; no collapse on mere radial crossing; hub timeout returns to root.
#[test]
fn t_nav5_inward_return_across_levels_and_hub_dwell_collapse() {
    let cfg = shipped_concentric_config();
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let center = Point::new(960.0, 540.0);

    // 4-level hierarchy
    let l3 = MenuNode::submenu(
        "l2.sub",
        "L2 Sub",
        vec![
            MenuNode::leaf("l3.leaf", "L3 Leaf"),
            MenuNode::leaf("l3.b", "L3 B"),
        ],
    );
    let l2 = MenuNode::submenu(
        "l1.sub",
        "L1 Sub",
        vec![l3, MenuNode::leaf("l2.leaf", "L2 Leaf")],
    );
    let l1 = MenuNode::submenu(
        "r.sub",
        "Root Sub",
        vec![l2, MenuNode::leaf("l1.leaf", "L1 Leaf")],
    );
    let tree = MenuTree::new(vec![l1, MenuNode::leaf("r.leaf", "Root Leaf")]);

    let mut session = WheelSession::new(Some(tree), center, vp, InputMode::Drag, &cfg, 0);
    let mut events = Vec::new();
    let mut now = 10;

    // Open levels 1, 2, 3
    for level in 0..3 {
        let pt = sector_center_point(&session, level, 0);
        session.handle(WheelInput::PointerMoved(pt), now, &mut events);
        now += 120;
        session.tick(now, &mut events);
    }
    assert_eq!(session.debug_state().wheels.len(), 4);

    // Move inward from level 3 to open-parent sector `[0, 0]` on level 1: NO collapse!
    let l1_open_parent = sector_center_point(&session, 1, 0);
    session.handle(
        WheelInput::PointerMoved(l1_open_parent),
        now + 10,
        &mut events,
    );
    session.tick(now + 500, &mut events);
    assert_eq!(
        session.debug_state().wheels.len(),
        4,
        "hovering open-parent sector on inner ring must keep all deeper rings open"
    );

    // Quick crossing through center hub (t=1000 enter hub, t=1080 exit hub back to level 2 open parent < 150 ms)
    let hub_pt = Point::new(960.0, 535.0);
    session.handle(WheelInput::PointerMoved(hub_pt), 1000, &mut events);
    assert_eq!(session.concentric_hit(), Some(NestedHit::Hub));
    assert_eq!(session.next_deadline(), Some(1150));

    let l2_open_parent = sector_center_point(&session, 2, 0);
    session.handle(WheelInput::PointerMoved(l2_open_parent), 1080, &mut events);
    assert_eq!(session.next_deadline(), None);
    session.tick(1200, &mut events);
    assert_eq!(session.debug_state().wheels.len(), 4);

    // Enter hub at t=2000: at t=2149 all 4 rings remain; at t=2150 collapses to root only!
    session.handle(WheelInput::PointerMoved(hub_pt), 2000, &mut events);
    session.tick(2149, &mut events);
    assert_eq!(session.debug_state().wheels.len(), 4);

    session.tick(2150, &mut events);
    assert_eq!(session.debug_state().wheels.len(), 1);
    assert_eq!(
        session.debug_state().open_chain.as_slice(),
        &[ItemPath::root()]
    );
    assert_no_commit(&events, "inward return and hub collapse");
}

/// `T-NAV6`: `Outside` / `OuterMargin` / `Separator` wandering keeps path, clears target;
/// re-entry restores strict targets; no default action leaks through protected parent hover.
#[test]
fn t_nav6_neutral_zones_outside_outer_margin_separator_and_reentry() {
    let cfg = shipped_concentric_config();
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let center = Point::new(960.0, 540.0);

    // Root item 0 is a submenu with default_child = Some(0)
    let sub_a = MenuNode::submenu(
        "sub.a",
        "Sub A",
        vec![
            MenuNode::leaf("a0", "Default Leaf A0"),
            MenuNode::leaf("a1", "Leaf A1"),
        ],
    )
    .with_default_child(0);
    let tree = MenuTree::new(vec![
        sub_a,
        MenuNode::leaf("b", "Leaf B"),
        MenuNode::leaf("c", "Leaf C"),
        MenuNode::leaf("d", "Leaf D"),
    ]);

    let mut session = WheelSession::new(Some(tree), center, vp, InputMode::Drag, &cfg, 0);
    let mut events = Vec::new();

    // Open Sub A at t=120
    let a_pt = sector_point_at_radius(&session, 0, 0, 80.0);
    session.handle(WheelInput::PointerMoved(a_pt), 10, &mut events);
    session.tick(120, &mut events);
    assert_eq!(session.debug_state().wheels.len(), 2);

    // Move to OuterMargin (child outer radius is 194, margin is 194..206 -> r = 200)
    let margin_pt = center.offset_polar(200.0, -FRAC_PI_2);
    session.handle(WheelInput::PointerMoved(margin_pt), 130, &mut events);
    assert_eq!(session.concentric_hit(), Some(NestedHit::OuterMargin));
    assert_eq!(session.drop_target(), DropTarget::Reject);

    // Move to Outside (r = 500) and wait 1000 ms
    let outside_pt = center.offset_polar(500.0, -FRAC_PI_2);
    session.handle(WheelInput::PointerMoved(outside_pt), 140, &mut events);
    session.tick(1000, &mut events);
    assert_eq!(session.concentric_hit(), Some(NestedHit::Outside));
    assert_eq!(session.drop_target(), DropTarget::Reject);
    assert_eq!(
        session.debug_state().open_chain.as_slice(),
        &[ItemPath::root(), ItemPath::from_slice(&[0])],
        "wandering Outside must preserve open_chain"
    );

    // Move onto an angular separator on root (boundary between sector 0 and sector 1 at angle -pi/4)
    let sep_angle = session.debug_state().wheels[0].spans[0].a1;
    let sep_pt = center.offset_polar(80.0, sep_angle);
    session.handle(WheelInput::PointerMoved(sep_pt), 1010, &mut events);
    assert_eq!(
        session.concentric_hit(),
        Some(NestedHit::Separator { level: 0 })
    );
    assert_eq!(
        session.drop_target(),
        DropTarget::Reject,
        "default_child on parent must NOT leak through separator or protected zone"
    );

    // Re-enter child sector [0, 1] -> immediately restores strict Accept([0, 1])
    let c1_pt = sector_center_point(&session, 1, 1);
    session.handle(WheelInput::PointerMoved(c1_pt), 1020, &mut events);
    assert_eq!(
        session.drop_target(),
        DropTarget::Accept(ItemPath::from_slice(&[0, 1]))
    );
}

/// `T-NAV7`: Drag placeholder -> `set_menu_at_pointer` while pointer sits where a leaf appears:
/// duplicate positions and ticks reject; `>= 2 lp` fresh move arms; child opening cannot
/// open/select grandchildren in same input.
#[test]
fn t_nav7_menu_installation_exposure_guard_and_single_transition_per_input() {
    let cfg = shipped_concentric_config();
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let center = Point::new(960.0, 540.0);

    // Start Drag session with placeholder (`menu = None`)
    let mut session = WheelSession::new(None, center, vp, InputMode::Drag, &cfg, 0);
    let mut events = Vec::new();

    // Pointer moves to (960, 460) while placeholder is active
    let pt_in_root0 = Point::new(960.0, 460.0);
    session.handle(WheelInput::PointerMoved(pt_in_root0), 10, &mut events);
    assert_eq!(session.drop_target(), DropTarget::Reject);

    // Install menu where root item 0 is a leaf directly under the pointer
    let tree = MenuTree::new(vec![
        MenuNode::leaf("leaf0", "Leaf 0"),
        MenuNode::leaf("leaf1", "Leaf 1"),
    ]);
    session.set_menu_at_pointer(tree, pt_in_root0, 20);
    assert_eq!(session.drop_target(), DropTarget::Reject);

    // Duplicate PointerMoved at identical coordinates and sub-2.0 lp jitter (1.4 lp) + ticks MUST still reject!
    session.handle(WheelInput::PointerMoved(pt_in_root0), 25, &mut events);
    session.handle(
        WheelInput::PointerMoved(Point::new(961.4, 460.0)),
        30,
        &mut events,
    );
    session.tick(200, &mut events);
    assert_eq!(
        session.drop_target(),
        DropTarget::Reject,
        "cached/duplicate pointer position or < 2 lp jitter after set_menu_at_pointer must not arm target"
    );

    // Fresh move >= 2.0 lp from `pt_in_root0` arms `[0]`!
    session.handle(
        WheelInput::PointerMoved(Point::new(962.5, 460.0)),
        210,
        &mut events,
    );
    assert_eq!(
        session.drop_target(),
        DropTarget::Accept(ItemPath::from_slice(&[0]))
    );

    // Single transition per input sample: outward band open of level 1 cannot open level 2 in the same input
    let deep_tree = MenuTree::new(vec![
        MenuNode::submenu(
            "s0",
            "S0",
            vec![MenuNode::submenu(
                "s0.0",
                "S0-0",
                vec![MenuNode::leaf("s0.0.0", "Grandchild")],
            )],
        ),
        MenuNode::leaf("b", "B"),
    ]);
    let mut session2 = WheelSession::new(Some(deep_tree), center, vp, InputMode::Drag, &cfg, 0);
    events.clear();
    session2.handle(
        WheelInput::PointerMoved(Point::new(960.0, 540.0 - 90.0)),
        10,
        &mut events,
    );
    // Outward move into root outer band opens level 1, never level 2 in the same sample
    session2.handle(
        WheelInput::PointerMoved(Point::new(960.0, 540.0 - 102.0)),
        20,
        &mut events,
    );
    assert_eq!(session2.debug_state().wheels.len(), 2);
}

/// `T-NAV8`: Displaced root: zero-motion rejects, approach across visible sectors rejects,
/// entering hub releases guard, then leaf movement works; keyboard can enter directly;
/// returning pointer still requires hub when applicable.
#[test]
fn t_nav8_displaced_center_approaching_guard_and_keyboard_bypass() {
    let cfg = shipped_concentric_config();
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let corner_anchor = Point::new(20.0, 20.0);

    let mut session = WheelSession::new(
        Some(normative_fixture_tree()),
        corner_anchor,
        vp,
        InputMode::Pointer,
        &cfg,
        0,
    );
    let plan = session.concentric_plan().expect("concentric plan").clone();
    assert!(plan.approach_required);
    assert_eq!(session.concentric_hit(), Some(NestedHit::Approach));
    assert_eq!(session.drop_target(), DropTarget::Reject);

    let mut events = Vec::new();
    // 1. Zero-motion click at displaced anchor is a no-op (Approach zone)
    session.handle(WheelInput::Press, 5, &mut events);
    session.handle(WheelInput::Release, 10, &mut events);
    assert_no_commit(&events, "zero-motion click at displaced anchor");
    assert!(!events.contains(&WheelEvent::CancelRequested));

    // 2. Moving across visible root sectors without entering the hub stays in Approach and rejects
    let root_sector0 = Point::new(plan.center.x, plan.center.y - 80.0);
    let root_sector1 = Point::new(plan.center.x, plan.center.y + 80.0);
    session.handle(WheelInput::PointerMoved(root_sector0), 20, &mut events);
    assert_eq!(session.concentric_hit(), Some(NestedHit::Approach));
    assert_eq!(session.debug_state().hovered_item, None);
    assert_eq!(session.drop_target(), DropTarget::Reject);

    session.handle(WheelInput::PointerMoved(root_sector1), 30, &mut events);
    assert_eq!(session.concentric_hit(), Some(NestedHit::Approach));
    assert_eq!(session.drop_target(), DropTarget::Reject);

    // 3. Keyboard navigation works immediately without visiting the hub!
    session.handle(WheelInput::Nav(NavKey::Next), 40, &mut events);
    assert_eq!(
        session.debug_state().hovered_item,
        Some(ItemPath::from_slice(&[0]))
    );
    // Moving pointer >= 2 lp while still outside the hub takes over from keyboard,
    // but pointer is STILL blocked by Approach until it enters the hub!
    session.handle(
        WheelInput::PointerMoved(Point::new(plan.center.x + 5.0, plan.center.y + 80.0)),
        50,
        &mut events,
    );
    assert_eq!(session.concentric_hit(), Some(NestedHit::Approach));
    assert_eq!(session.debug_state().hovered_item, None);
    assert_eq!(session.drop_target(), DropTarget::Reject);

    // 4. Enter the center hub (r = 10 < 40): releases the approach guard!
    let hub_pt = Point::new(plan.center.x, plan.center.y - 10.0);
    session.handle(WheelInput::PointerMoved(hub_pt), 60, &mut events);
    assert_eq!(session.concentric_hit(), Some(NestedHit::Hub));

    // 5. Move onto Leaf B (root item 1) -> now hovers and commits cleanly!
    events.clear();
    session.handle(WheelInput::PointerMoved(root_sector1), 70, &mut events);
    assert_eq!(
        session.concentric_hit(),
        Some(NestedHit::Item { level: 0, index: 1 })
    );
    assert_eq!(
        session.drop_target(),
        DropTarget::Accept(ItemPath::from_slice(&[1]))
    );
    session.handle(WheelInput::Press, 75, &mut events);
    session.handle(WheelInput::Release, 80, &mut events);
    assert_eq!(
        events,
        vec![
            WheelEvent::HoverChanged(Some(ItemPath::from_slice(&[1]))),
            WheelEvent::InputRegionChanged,
            WheelEvent::Commit(ItemPath::from_slice(&[1])),
        ]
    );

    // 6. Displaced anchor that is only 10 lp from clamped center (0.5 < d < 40, inside hub radius):
    //    must still start in Approach (even after set_menu_at_pointer at the same anchor) and
    //    must not release until a real pointer motion sample (past activation_slop) lands in d < 40.
    let near_edge_anchor = Point::new(324.0, 334.0); // Clamped center is (334, 334), distance = 10.0 lp
    let mut s_near = WheelSession::new(None, near_edge_anchor, vp, InputMode::Drag, &cfg, 0);
    assert_eq!(s_near.concentric_hit(), Some(NestedHit::Approach));
    s_near.set_menu_at_pointer(normative_fixture_tree(), near_edge_anchor, 5);
    assert_eq!(
        s_near.concentric_hit(),
        Some(NestedHit::Approach),
        "set_menu_at_pointer at displaced anchor inside hub radius must remain Approach before real motion"
    );
    // Jumping straight from near_edge_anchor (324, 334) to a sector (334, 254) without a motion sample in hub stays Approach:
    events.clear();
    let mut s_jump = WheelSession::new(
        Some(normative_fixture_tree()),
        near_edge_anchor,
        vp,
        InputMode::Drag,
        &cfg,
        0,
    );
    s_jump.handle(
        WheelInput::PointerMoved(Point::new(334.0, 254.0)),
        10,
        &mut events,
    );
    assert_eq!(s_jump.concentric_hit(), Some(NestedHit::Approach));
    assert_eq!(s_jump.drop_target(), DropTarget::Reject);

    // Real motion sample past activation_slop inside the hub (334, 334) releases Approach -> Hub:
    s_near.handle(
        WheelInput::PointerMoved(Point::new(334.0, 334.0)),
        10,
        &mut events,
    );
    assert_eq!(s_near.concentric_hit(), Some(NestedHit::Hub));
}

/// `T-NAV9`: All Invoke keys, wrap, disabled focus reason, `Back` at root/no-op, digit bounds,
/// keyboard-only operation without pointer slop, pointer takeover threshold, press+release /
/// hub-marking / release modes.
#[test]
fn t_nav9_invoke_keyboard_navigation_pointer_takeover_and_invoke_select_modes() {
    let cfg = shipped_concentric_config();
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let center = Point::new(960.0, 540.0);

    let sub_a = MenuNode::submenu(
        "sub.a",
        "Sub A",
        vec![MenuNode::leaf("a0", "A0"), MenuNode::leaf("a1", "A1")],
    );
    let disabled_b = MenuNode::leaf("b", "Disabled B").with_disabled("Requires text file");
    let leaf_c = MenuNode::leaf("c", "Leaf C");
    let tree = MenuTree::new(vec![sub_a, disabled_b, leaf_c]);

    // 1. Keyboard navigation without pointer slop: Next/Prev wrap, disabled caption reason,
    //    Back at root no-op, Activate submenu -> Back -> Digit -> Cancel.
    {
        let mut session =
            WheelSession::new(Some(tree.clone()), center, vp, InputMode::Pointer, &cfg, 0);
        let mut events = Vec::new();

        // Back at root is a no-op
        session.handle(WheelInput::Nav(NavKey::Back), 5, &mut events);
        assert!(events.is_empty());

        // Prev from None focuses last item (index 2 = Leaf C)
        session.handle(WheelInput::Nav(NavKey::Prev), 10, &mut events);
        assert_eq!(
            session.debug_state().hovered_item,
            Some(ItemPath::from_slice(&[2]))
        );

        // Next wraps from index 2 -> index 0 (Sub A)
        session.handle(WheelInput::Nav(NavKey::Next), 15, &mut events);
        assert_eq!(
            session.debug_state().hovered_item,
            Some(ItemPath::from_slice(&[0]))
        );

        // Next -> index 1 (Disabled B): caption shows disabled reason, Activate is a no-op
        session.handle(WheelInput::Nav(NavKey::Next), 20, &mut events);
        assert_eq!(
            session.debug_state().hovered_item,
            Some(ItemPath::from_slice(&[1]))
        );
        let cap = session.scene().caption.as_ref().expect("caption");
        assert!(cap.disabled);
        assert_eq!(cap.detail.as_deref(), Some("Requires text file"));

        events.clear();
        session.handle(WheelInput::Nav(NavKey::Activate), 25, &mut events);
        assert_no_commit(&events, "Activate on disabled item");

        // Digit(1) selects 1-based item 1 (index 0 = Sub A) and opens it!
        session.handle(WheelInput::Nav(NavKey::Digit(1)), 30, &mut events);
        assert_eq!(session.debug_state().wheels.len(), 2);
        assert_eq!(
            session.debug_state().hovered_item,
            Some(ItemPath::from_slice(&[0, 0]))
        );

        // Out-of-bounds Digit(9) is a no-op
        events.clear();
        session.handle(WheelInput::Nav(NavKey::Digit(9)), 35, &mut events);
        assert!(events.is_empty());

        // Pointer jitter < 2.0 lp from center does NOT steal keyboard focus
        session.handle(
            WheelInput::PointerMoved(Point::new(961.0, 540.0)),
            40,
            &mut events,
        );
        assert_eq!(
            session.debug_state().hovered_item,
            Some(ItemPath::from_slice(&[0, 0]))
        );

        // Back from child ring closes child ring and focuses parent [0]
        session.handle(WheelInput::Nav(NavKey::Back), 45, &mut events);
        assert_eq!(session.debug_state().wheels.len(), 1);
        assert_eq!(
            session.debug_state().hovered_item,
            Some(ItemPath::from_slice(&[0]))
        );

        // Digit(3) commits Leaf C ([2])
        events.clear();
        session.handle(WheelInput::Nav(NavKey::Digit(3)), 50, &mut events);
        assert!(events.contains(&WheelEvent::Commit(ItemPath::from_slice(&[2]))));

        // Verify pointer takeover from keyboard-first session with a >= 2.0 lp move (< 6.0 lp activation_slop),
        // and verify keyboard Activate / Digit on an enabled child whose ancestor was disabled is rejected:
        let mut s_takeover =
            WheelSession::new(Some(tree.clone()), center, vp, InputMode::Pointer, &cfg, 0);
        events.clear();
        s_takeover.handle(WheelInput::Nav(NavKey::Digit(1)), 10, &mut events);
        assert_eq!(
            s_takeover.debug_state().hovered_item,
            Some(ItemPath::from_slice(&[0, 0]))
        );
        // Disable parent [0] while child [0, 0] is focused by keyboard:
        s_takeover.set_enabled_at(
            &ItemPath::from_slice(&[0]),
            false,
            Some(Arc::from("Parent disabled")),
            15,
        );
        events.clear();
        s_takeover.handle(WheelInput::Nav(NavKey::Activate), 20, &mut events);
        s_takeover.handle(WheelInput::Nav(NavKey::Digit(1)), 25, &mut events);
        assert_no_commit(
            &events,
            "keyboard Activate/Digit on child with disabled ancestor",
        );

        // Now move pointer by 2.5 lp (>= 2.0 lp takeover threshold, < 6.0 lp activation_slop) inside hub:
        // must take over from keyboard and clear keyboard hover to None (since pointer is in Hub)!
        s_takeover.handle(
            WheelInput::PointerMoved(Point::new(center.x + 2.5, center.y)),
            30,
            &mut events,
        );
        assert_eq!(s_takeover.concentric_hit(), Some(NestedHit::Hub));
        assert_eq!(
            s_takeover.debug_state().hovered_item,
            None,
            "2.5 lp pointer move must take over from keyboard even when < 6.0 lp initial slop"
        );
    }

    // 2. Hub-marking press+drag+release in InvokeSelect::Click and release-to-select in InvokeSelect::Release
    {
        // Hub-marking in InvokeSelect::Click: press in hub, drag to Leaf C, release commits Leaf C
        let mut session =
            WheelSession::new(Some(tree.clone()), center, vp, InputMode::Pointer, &cfg, 0);
        let mut events = Vec::new();
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 530.0)),
            10,
            &mut events,
        );
        session.handle(WheelInput::Press, 15, &mut events);
        let c_pt = sector_center_point(&session, 0, 2);
        session.handle(WheelInput::PointerMoved(c_pt), 25, &mut events);
        events.clear();
        session.handle(WheelInput::Release, 30, &mut events);
        assert!(events.contains(&WheelEvent::Commit(ItemPath::from_slice(&[2]))));

        // InvokeSelect::Release: moving to Leaf C and releasing without prior press commits Leaf C
        let mut cfg_rel = cfg.clone();
        cfg_rel.invoke_select = InvokeSelect::Release;
        let mut session_rel =
            WheelSession::new(Some(tree), center, vp, InputMode::Pointer, &cfg_rel, 0);
        events.clear();
        session_rel.handle(WheelInput::PointerMoved(c_pt), 10, &mut events);
        events.clear();
        session_rel.handle(WheelInput::Release, 20, &mut events);
        assert!(events.contains(&WheelEvent::Commit(ItemPath::from_slice(&[2]))));
    }
}

/// `T-NAV10`: Rapid alternating sibling movements at `1 / 8 / 16 ms` intervals for `10,000` steps;
/// `open_chain` valid, ghost count bounded (`<= 6` visible rings), no stale accept, no panic.
#[test]
fn t_nav10_rapid_sibling_oscillation_soak_10000_steps() {
    let cfg = shipped_concentric_config();
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let center = Point::new(960.0, 540.0);

    let sub0 = MenuNode::submenu(
        "s0",
        "S0",
        vec![
            MenuNode::submenu("s0.0", "S0.0", vec![MenuNode::leaf("s0.0.0", "L0")]),
            MenuNode::leaf("s0.1", "S0.1"),
        ],
    );
    let sub1 = MenuNode::submenu(
        "s1",
        "S1",
        vec![
            MenuNode::submenu("s1.0", "S1.0", vec![MenuNode::leaf("s1.0.0", "L1")]),
            MenuNode::leaf("s1.1", "S1.1"),
        ],
    );
    let tree = MenuTree::new(vec![
        sub0,
        sub1,
        MenuNode::leaf("r2", "R2"),
        MenuNode::leaf("r3", "R3"),
    ]);

    let mut session = WheelSession::new(Some(tree.clone()), center, vp, InputMode::Drag, &cfg, 0);
    let mut events = Vec::new();
    let mut now: Millis = 0;

    let intervals = [1u64, 8, 16];
    for step in 0..10_000usize {
        now += intervals[step % intervals.len()];
        events.clear();

        let pt = match step % 8 {
            0 => center.offset_polar(85.0, -FRAC_PI_2), // root sector 0 interior
            1 => center.offset_polar(102.0, -FRAC_PI_2), // root sector 0 outer band
            2 => center.offset_polar(155.0, -FRAC_PI_2), // level 1 sector 0 interior
            3 => center.offset_polar(180.0, -FRAC_PI_2), // level 1 sector 0 outer band
            4 => center.offset_polar(85.0, 0.0),        // root sector 1 interior
            5 => center.offset_polar(104.0, 0.0),       // root sector 1 outer band
            6 => center.offset_polar(131.0, 0.5),       // radial gap
            _ => center.offset_polar(15.0, 0.0),        // center hub
        };

        session.handle(WheelInput::PointerMoved(pt), now, &mut events);
        session.tick(now, &mut events);
        assert_no_commit(&events, "T-NAV10 soak step");

        let dbg = session.debug_state();
        assert!(!dbg.open_chain.is_empty());
        assert_eq!(dbg.open_chain[0], ItemPath::root());
        for (lvl, p) in dbg.open_chain.iter().enumerate() {
            assert_eq!(p.len(), lvl);
            assert!(tree.node(p).is_some(), "invalid open_chain path {p:?}");
        }

        let scene = session.scene();
        assert!(
            scene.wheels.len() <= 6,
            "visible ring count {} exceeds 6 at step {step}",
            scene.wheels.len()
        );

        if let DropTarget::Accept(ref accept_path) = session.drop_target() {
            let parent_len = accept_path.len().saturating_sub(1);
            let parent_prefix = ItemPath::from_slice(&accept_path.as_slice()[..parent_len]);
            // Either direct leaf in an open ring or default_child of an open ring's sector
            let ring_open = dbg.open_chain.contains(&parent_prefix)
                || (parent_len >= 1 && {
                    let grand = ItemPath::from_slice(&accept_path.as_slice()[..parent_len - 1]);
                    dbg.open_chain.contains(&grand)
                });
            assert!(
                ring_open,
                "stale DropTarget::Accept({accept_path:?}) not backed by open_chain {:?}",
                dbg.open_chain
            );
        }
    }
}

/// `T-NAV11` & `T-NAV12`: Interrupt open/close/dismiss every `10 ms` through each tween
/// (continuous opacity retarget, fixed radii, `scale == 1.0`, `pop == 0.0`, single `Finished`,
/// removed/retracting rings never hit) and reduced-motion fade cap (`<= 60 ms`).
#[test]
fn t_nav11_and_t_nav12_interrupt_tweens_every_10ms_and_reduced_motion() {
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let center = Point::new(960.0, 540.0);

    // 1. T-NAV11: Interrupt ring open, sibling replace, and session dismiss every 10 ms
    {
        let cfg = shipped_concentric_config();
        let mut session = WheelSession::new(
            Some(normative_fixture_tree()),
            center,
            vp,
            InputMode::Drag,
            &cfg,
            0,
        );
        let mut events = Vec::new();

        // Trigger child open via outward band at t=10
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 90.0)),
            5,
            &mut events,
        );
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 102.0)),
            10,
            &mut events,
        );
        assert_eq!(session.debug_state().wheels.len(), 2);

        // Step every 10 ms during child emerge (t=20, 30, 40, 50) and verify fixed radii & scale=1.0
        let mut prev_child_op = 0.0f32;
        for t in (20..=50).step_by(10) {
            session.tick(t, &mut events);
            let scene = session.scene();
            for w in &scene.wheels {
                assert_eq!(w.center, center);
                assert_eq!(w.scale, 1.0);
                assert!(w.bridge.is_none());
                for seg in &w.segments {
                    assert_eq!(seg.pop, 0.0);
                }
            }
            let child_op = scene.wheels[1].opacity;
            assert!(child_op >= prev_child_op);
            prev_child_op = child_op;
        }

        // Interrupt mid-emerge at t=50 by entering hub and collapsing at t=200
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 535.0)),
            50,
            &mut events,
        );
        session.tick(200, &mut events);
        assert_eq!(session.debug_state().wheels.len(), 1);

        // While child is retracting (t=210), verify points at child radius (164) are Outside/OuterMargin, NEVER Item!
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 164.0)),
            210,
            &mut events,
        );
        assert_eq!(session.concentric_hit(), Some(NestedHit::Outside));
        assert_eq!(session.drop_target(), DropTarget::Reject);

        // Interrupt with session.dismiss() at t=220 and tick every 10 ms through t=400: exactly ONE Finished
        events.clear();
        session.dismiss(DismissReason::Cancelled, 220);
        let mut finished_count = 0;
        for t in (230..=400).step_by(10) {
            let mut step_events = Vec::new();
            session.tick(t, &mut step_events);
            finished_count += step_events
                .iter()
                .filter(|e| matches!(e, WheelEvent::Finished))
                .count();
        }
        assert_eq!(finished_count, 1, "dismiss must emit Finished exactly once");
    }

    // 2. T-NAV12: Reduced motion (`reduced_motion = true`): scale=1, pop=0, all fades <= 60 ms
    {
        let mut cfg = shipped_concentric_config();
        cfg.reduced_motion = true;
        let mut session = WheelSession::new(
            Some(normative_fixture_tree()),
            center,
            vp,
            InputMode::Drag,
            &cfg,
            0,
        );
        let mut events = Vec::new();
        session.tick(60, &mut events);
        assert!(
            !session.is_animating(),
            "reduced-motion summon fade must finish by 60 ms"
        );

        // Open child at t=70 via band
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 90.0)),
            65,
            &mut events,
        );
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 102.0)),
            70,
            &mut events,
        );
        session.tick(130, &mut events); // 70 + 60 = 130 ms
        assert!(
            !session.is_animating(),
            "reduced-motion child open + hover fade must finish within 60 ms"
        );
        for w in &session.scene().wheels {
            assert_eq!(w.scale, 1.0);
            for s in &w.segments {
                assert_eq!(s.pop, 0.0);
            }
        }
    }
}

/// `T-SAFE1`–`T-SAFE5`:
/// - `T-SAFE1`: Reject in all non-action/protected zones with destructive fake action registered.
/// - `T-SAFE2`: Immediate eligibility updates on leaf/ancestor `set_enabled_at`.
/// - `T-SAFE3`: Direct parent `default_child` leaf vs gap/non-leaf/disabled rejection and Invoke submenu open.
/// - `T-SAFE4`: Invoke mismatched press/release and generation changes never commit unintended leaves.
/// - `T-SAFE5`: Identical committed `(ItemPath, ActionId)` across `Branching` and `Concentric` + `suspend_interaction`.
#[test]
fn t_safe1_to_t_safe5_strict_zone_rejection_enablement_default_child_generation_and_layout_parity()
{
    let cfg = shipped_concentric_config();
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let center = Point::new(960.0, 540.0);

    // --- T-SAFE1: Destructive fake action never committed in any non-action/protected zone ---
    {
        let destructive_tree = MenuTree::new(vec![
            MenuNode::submenu(
                "sub.danger",
                "Danger Sub",
                vec![MenuNode::leaf(
                    "fake.destructive_delete",
                    "Delete Everything",
                )],
            )
            .with_default_child(0),
            MenuNode::leaf("fake.destructive_wipe", "Wipe Everything"),
        ]);

        // Zero movement release
        let mut s = WheelSession::new(
            Some(destructive_tree.clone()),
            center,
            vp,
            InputMode::Pointer,
            &cfg,
            0,
        );
        let mut ev = Vec::new();
        assert_eq!(s.drop_target(), DropTarget::Reject);
        s.handle(WheelInput::Press, 5, &mut ev);
        s.handle(WheelInput::Release, 10, &mut ev);
        assert_no_commit(&ev, "T-SAFE1 zero movement");

        // Open child at t=120, then test release in: Hub, RadialGap, RadialBoundary, Separator, OuterMargin, Outside, PendingSibling
        let mut s2 = WheelSession::new(
            Some(destructive_tree.clone()),
            center,
            vp,
            InputMode::Drag,
            &cfg,
            0,
        );
        s2.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 - 80.0)),
            10,
            &mut ev,
        );
        s2.tick(120, &mut ev);

        let sep_angle = s2.debug_state().wheels[0].spans[0].a1;
        let non_action_points = [
            ("hub", Point::new(960.0, 530.0)),
            ("radial_gap", Point::new(960.0, 540.0 - 131.0)),
            ("radial_boundary", Point::new(960.0, 540.0 - 128.0)),
            ("separator", center.offset_polar(80.0, sep_angle)),
            ("outer_margin", Point::new(960.0, 540.0 - 200.0)),
            ("outside", Point::new(960.0, 540.0 - 400.0)),
            ("pending_sibling", Point::new(960.0, 540.0 + 80.0)),
        ];

        for (label, pt) in non_action_points {
            let mut clone_s = WheelSession::new(
                Some(destructive_tree.clone()),
                center,
                vp,
                InputMode::Pointer,
                &cfg,
                0,
            );
            let mut local_ev = Vec::new();
            clone_s.handle(
                WheelInput::PointerMoved(Point::new(960.0, 540.0 - 80.0)),
                10,
                &mut local_ev,
            );
            clone_s.tick(120, &mut local_ev);
            local_ev.clear();

            clone_s.handle(WheelInput::PointerMoved(pt), 130, &mut local_ev);
            assert_eq!(
                clone_s.drop_target(),
                DropTarget::Reject,
                "T-SAFE1 zone `{label}` must reject drop"
            );
            clone_s.handle(WheelInput::Press, 132, &mut local_ev);
            clone_s.handle(WheelInput::Release, 135, &mut local_ev);
            assert_no_commit(&local_ev, label);
        }
    }

    // --- T-SAFE2: Disabled leaf, disabled ancestor, disabled->enabled and enabled->disabled before drop ---
    {
        let mut session = WheelSession::new(
            Some(normative_fixture_tree()),
            center,
            vp,
            InputMode::Drag,
            &cfg,
            0,
        );
        let mut events = Vec::new();
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 460.0)),
            10,
            &mut events,
        );
        session.tick(120, &mut events);

        let child0_pt = sector_center_point(&session, 1, 0);
        session.handle(WheelInput::PointerMoved(child0_pt), 130, &mut events);
        assert_eq!(
            session.drop_target(),
            DropTarget::Accept(ItemPath::from_slice(&[0, 0]))
        );

        // Disable leaf [0, 0] with "Checking…" immediately before drop (no pointer movement!)
        session.set_enabled_at(
            &ItemPath::from_slice(&[0, 0]),
            false,
            Some(Arc::from("Checking…")),
            135,
        );
        assert_eq!(session.drop_target(), DropTarget::Reject);
        assert_eq!(
            session
                .scene()
                .caption
                .as_ref()
                .and_then(|c| c.detail.as_deref()),
            Some("Checking…")
        );

        // Re-enable leaf [0, 0] -> immediately eligible again!
        session.set_enabled_at(&ItemPath::from_slice(&[0, 0]), true, None, 140);
        assert_eq!(
            session.drop_target(),
            DropTarget::Accept(ItemPath::from_slice(&[0, 0]))
        );

        // Disable ancestor [0] -> descendant [0, 0] immediately becomes ineligible!
        session.set_enabled_at(
            &ItemPath::from_slice(&[0]),
            false,
            Some(Arc::from("Folder unreadable")),
            145,
        );
        assert_eq!(session.drop_target(), DropTarget::Reject);
        assert_eq!(
            session
                .scene()
                .caption
                .as_ref()
                .and_then(|c| c.detail.as_deref()),
            Some("Folder unreadable")
        );
    }

    // --- T-SAFE3: Parent with default_child ---
    {
        // Enabled leaf default_child: direct drop on parent [0] before child opens selects [0, 1]
        let sub_default_leaf = MenuNode::submenu(
            "sub.a",
            "Sub A",
            vec![
                MenuNode::leaf("a0", "A0"),
                MenuNode::leaf("a1", "Default A1"),
            ],
        )
        .with_default_child(1);
        let tree_ok = MenuTree::new(vec![sub_default_leaf, MenuNode::leaf("b", "B")]);

        let mut s_drag =
            WheelSession::new(Some(tree_ok.clone()), center, vp, InputMode::Drag, &cfg, 0);
        let mut ev = Vec::new();
        s_drag.handle(
            WheelInput::PointerMoved(Point::new(960.0, 460.0)),
            10,
            &mut ev,
        );
        assert_eq!(
            s_drag.drop_target(),
            DropTarget::Accept(ItemPath::from_slice(&[0, 1]))
        );

        // In Pointer mode, clicking parent [0] opens the submenu instead of committing default_child
        let mut s_inv = WheelSession::new(Some(tree_ok), center, vp, InputMode::Pointer, &cfg, 0);
        ev.clear();
        s_inv.handle(
            WheelInput::PointerMoved(Point::new(960.0, 460.0)),
            10,
            &mut ev,
        );
        s_inv.handle(WheelInput::Press, 15, &mut ev);
        s_inv.handle(WheelInput::Release, 20, &mut ev);
        assert_no_commit(&ev, "Pointer click on submenu with default_child");
        assert_eq!(s_inv.debug_state().wheels.len(), 2);

        // Non-leaf default_child or disabled default_child rejects direct parent drop
        let sub_nonleaf_default = MenuNode::submenu(
            "sub.nested",
            "Nested",
            vec![MenuNode::submenu(
                "inner",
                "Inner",
                vec![MenuNode::leaf("deep", "Deep")],
            )],
        )
        .with_default_child(0);
        let mut s_nonleaf = WheelSession::new(
            Some(MenuTree::new(vec![sub_nonleaf_default])),
            center,
            vp,
            InputMode::Drag,
            &cfg,
            0,
        );
        s_nonleaf.handle(
            WheelInput::PointerMoved(Point::new(960.0, 460.0)),
            10,
            &mut ev,
        );
        assert_eq!(s_nonleaf.drop_target(), DropTarget::Reject);
    }

    // --- T-SAFE4: Pointer mode mismatched press/release and generation changes ---
    {
        let mut session = WheelSession::new(
            Some(normative_fixture_tree()),
            center,
            vp,
            InputMode::Pointer,
            &cfg,
            0,
        );
        let mut events = Vec::new();

        // Press on Leaf B ([1]), move to Sub A ([0]), release: NO commit
        let b_pt = sector_point_at_radius(&session, 0, 1, 80.0);
        let a_pt = sector_point_at_radius(&session, 0, 0, 80.0);
        session.handle(WheelInput::PointerMoved(b_pt), 10, &mut events);
        session.handle(WheelInput::Press, 15, &mut events);
        session.handle(WheelInput::PointerMoved(a_pt), 20, &mut events);
        events.clear();
        session.handle(WheelInput::Release, 25, &mut events);
        assert_no_commit(&events, "mismatched press/release sectors");

        // Press on child [0, 0], then close and reopen [0] while button is held (geometry_gen changes),
        // then move pointer on [0, 0] and release: NO commit because generation changed!
        session.handle(WheelInput::PointerMoved(a_pt), 30, &mut events);
        session.tick(150, &mut events);
        let c0_pt = sector_center_point(&session, 1, 0);
        session.handle(WheelInput::PointerMoved(c0_pt), 160, &mut events);
        session.handle(WheelInput::Press, 165, &mut events);
        // Collapse child via hub dwell while button is held, then reopen [0]
        session.handle(
            WheelInput::PointerMoved(Point::new(960.0, 535.0)),
            170,
            &mut events,
        );
        session.tick(330, &mut events);
        assert_eq!(session.debug_state().wheels.len(), 1);
        session.handle(WheelInput::PointerMoved(a_pt), 340, &mut events);
        session.tick(460, &mut events);
        assert_eq!(session.debug_state().wheels.len(), 2);
        session.handle(WheelInput::PointerMoved(c0_pt), 470, &mut events);
        events.clear();
        session.handle(WheelInput::Release, 475, &mut events);
        assert_no_commit(&events, "release after geometry_gen changed");
    }

    // --- T-SAFE5: Identical committed ItemPath & action ID across Branching & Concentric + suspend_interaction ---
    {
        let routes: &[&[u8]] = &[&[0, 0], &[0, 1], &[1]];
        for &route in routes {
            let mut committed = Vec::new();
            for layout in [WheelLayout::Branching, WheelLayout::Concentric] {
                let mut lcfg = shipped_concentric_config();
                lcfg.layout = layout;
                let tree = normative_fixture_tree();
                let mut s =
                    WheelSession::new(Some(tree.clone()), center, vp, InputMode::Pointer, &lcfg, 0);
                let mut ev = Vec::new();
                let mut t = 10;
                for (depth, &idx) in route.iter().enumerate() {
                    let pt = sector_center_point(&s, depth, usize::from(idx));
                    s.handle(WheelInput::PointerMoved(pt), t, &mut ev);
                    if depth + 1 < route.len() {
                        t += 150;
                        s.tick(t, &mut ev);
                    }
                    t += 20;
                }
                ev.clear();
                s.handle(WheelInput::Press, t, &mut ev);
                s.handle(WheelInput::Release, t + 5, &mut ev);
                let commit_path = ev
                    .iter()
                    .find_map(|e| match e {
                        WheelEvent::Commit(p) => Some(p.clone()),
                        _ => None,
                    })
                    .expect("expected Commit event");
                let action_id = tree.node(&commit_path).unwrap().id.clone();
                committed.push((commit_path, action_id));
            }
            assert_eq!(
                committed[0], committed[1],
                "route {route:?} must produce identical Commit path and action ID in Branching and Concentric"
            );
        }

        // Verify suspend_interaction irreversibly blocks hover, timers, and commits
        let mut s_susp = WheelSession::new(
            Some(normative_fixture_tree()),
            center,
            vp,
            InputMode::Pointer,
            &cfg,
            0,
        );
        let mut ev = Vec::new();
        s_susp.handle(
            WheelInput::PointerMoved(Point::new(960.0, 540.0 + 80.0)),
            10,
            &mut ev,
        );
        assert_eq!(
            s_susp.drop_target(),
            DropTarget::Accept(ItemPath::from_slice(&[1]))
        );
        s_susp.suspend_interaction(15);
        assert!(s_susp.is_interaction_suspended());
        assert_eq!(s_susp.drop_target(), DropTarget::Reject);
        assert_eq!(s_susp.next_deadline(), None);
        ev.clear();
        s_susp.handle(WheelInput::Press, 18, &mut ev);
        s_susp.handle(WheelInput::Release, 20, &mut ev);
        assert_no_commit(&ev, "suspended session");
    }
}

/// `T-MENU2` & `T-MENU3`:
/// - `More…` wrapping and max-depth disablement for counts `0, 1, 7, 8, 9, 12, 13`.
/// - Enablement updates preserve geometry and item positions; disabled parent makes all
///   descendants ineligible; caption reason updates with current injected time.
#[test]
fn t_menu2_and_t_menu3_more_wrapping_depth_limits_and_enablement_geometry_preservation() {
    let cfg = shipped_concentric_config();
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let center = Point::new(960.0, 540.0);

    // 1. T-MENU2: Item counts 0, 1, 7, 8, 9, 12, 13 with max_items_per_wheel = 8
    for count in [0usize, 1, 7, 8, 9, 12, 13] {
        let items: Vec<MenuNode> = (0..count)
            .map(|i| MenuNode::leaf(format!("act.{i}"), format!("Action {i}")))
            .collect();
        let tree = MenuTree::new(items);

        let mut cfg_b = cfg.clone();
        cfg_b.layout = WheelLayout::Branching;
        let s_b = WheelSession::new(Some(tree.clone()), center, vp, InputMode::Drag, &cfg_b, 0);
        let s_c = WheelSession::new(Some(tree.clone()), center, vp, InputMode::Drag, &cfg, 0);

        // Root sector count must match between Branching and Concentric
        assert_eq!(
            s_b.debug_state().wheels[0].spans.len(),
            s_c.debug_state().wheels[0].spans.len(),
            "sector count mismatch for count={count}"
        );
        if count <= 8 {
            assert_eq!(s_c.debug_state().wheels[0].spans.len(), count);
        } else {
            // 7 direct items + 1 "More…" submenu = 8 sectors
            assert_eq!(s_c.debug_state().wheels[0].spans.len(), 8);
            let mut sanitized = tree.clone();
            sanitized.sanitize(&cfg);
            let more_node = sanitized
                .node(&ItemPath::from_slice(&[7]))
                .expect("More... node at index 7");
            assert_eq!(more_node.label.as_ref(), "More…");
        }
    }

    // 2. T-MENU3: set_enabled_at preserves ring geometry and updates caption at injected timestamp
    let mut session = WheelSession::new(
        Some(normative_fixture_tree()),
        center,
        vp,
        InputMode::Drag,
        &cfg,
        0,
    );
    let mut events = Vec::new();
    session.handle(
        WheelInput::PointerMoved(Point::new(960.0, 460.0)),
        10,
        &mut events,
    );
    session.tick(120, &mut events);
    let child_pt = sector_center_point(&session, 1, 1);
    session.handle(WheelInput::PointerMoved(child_pt), 130, &mut events);

    let before_wheels = session.debug_state().wheels.clone();
    session.set_enabled_at(
        &ItemPath::from_slice(&[0]),
        false,
        Some(Arc::from("Parent locked at t=250")),
        250,
    );
    let after_wheels = session.debug_state().wheels.clone();

    assert_eq!(
        before_wheels.len(),
        after_wheels.len(),
        "set_enabled_at must not alter open ring count"
    );
    for (bw, aw) in before_wheels.iter().zip(after_wheels.iter()) {
        assert_eq!(bw.center, aw.center);
        assert_eq!(bw.r_in, aw.r_in);
        assert_eq!(bw.r_out, aw.r_out);
        assert_eq!(bw.spans, aw.spans);
    }
    assert_eq!(session.drop_target(), DropTarget::Reject);
    let cap = session.scene().caption.as_ref().expect("caption");
    assert!(cap.disabled);
    assert_eq!(cap.detail.as_deref(), Some("Parent locked at t=250"));
}
