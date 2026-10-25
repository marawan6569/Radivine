//! Comprehensive unit tests for `radivine-wheel` geometry, hit-testing, layout, animation, and menu validation.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use radivine_wheel::anim::{Ease, Tween, timings};
use radivine_wheel::config::WheelConfig;
use radivine_wheel::geometry::{
    Point, Viewport, anchor_corridor_polygon, angle_diff, angle_in_span, child_corridor_polygon,
    child_segment_spans, circle_tangent_points, convex_hull_4, gap_inset_angle, normalize_angle,
    point_in_convex_polygon, point_in_triangle, root_segment_spans, segment_content_radius,
    segment_label_max_width, widen_span,
};
use radivine_wheel::hit::{WheelHitGeometry, Zone, hit_test};
use radivine_wheel::layout::{
    CAPTION_BOX_H, CAPTION_BOX_W, EDGE_MARGIN, compute_stem_points, nominal_radii, place_caption,
    place_child, place_root, scaled_inner_radius,
};
use radivine_wheel::menu::{ItemPath, MenuNode, MenuTree};
use radivine_wheel::scene::CaptionAlign;

#[test]
fn test_angle_normalization_and_diff() {
    assert!((normalize_angle(0.0) - 0.0).abs() < 1e-6);
    assert!((normalize_angle(TAU) - 0.0).abs() < 1e-6);
    assert!((normalize_angle(-FRAC_PI_2) - 3.0 * FRAC_PI_2).abs() < 1e-6);
    assert!((normalize_angle(5.0 * PI) - PI).abs() < 1e-6);
    assert_eq!(normalize_angle(f32::NAN), 0.0);
    assert_eq!(normalize_angle(f32::INFINITY), 0.0);

    // Shortest signed angular difference
    assert!((angle_diff(0.1, TAU - 0.1) - 0.2).abs() < 1e-5);
    assert!((angle_diff(TAU - 0.1, 0.1) + 0.2).abs() < 1e-5);
    assert!((angle_diff(PI, 0.0).abs() - PI).abs() < 1e-5);
}

#[test]
fn test_angle_in_span_wrap_around() {
    // Span crossing 0/2π: from 350° to 10° (in unwrapped form: 350° to 370°)
    let a0 = 350.0_f32.to_radians();
    let a1 = 370.0_f32.to_radians();
    assert!(angle_in_span(355.0_f32.to_radians(), a0, a1));
    assert!(angle_in_span(0.0, a0, a1));
    assert!(angle_in_span(5.0_f32.to_radians(), a0, a1));
    assert!(!angle_in_span(15.0_f32.to_radians(), a0, a1));
    assert!(!angle_in_span(345.0_f32.to_radians(), a0, a1));

    // Widening span
    let (w0, w1) = widen_span(a0, a1, 4.0_f32.to_radians());
    assert!(angle_in_span(347.0_f32.to_radians(), w0, w1));
    assert!(angle_in_span(13.0_f32.to_radians(), w0, w1));
}

#[test]
fn test_root_segment_distribution_1_to_12() {
    let top = 3.0 * FRAC_PI_2;
    for n in 1..=12 {
        let spans = root_segment_spans(n);
        assert_eq!(spans.len(), n);
        // Item 0 is always centered at top (3π/2)
        assert!(angle_diff(spans[0].center, top).abs() < 1e-5);

        // Every angle in [0, 2π) belongs to exactly one root segment
        for step in 0..360 {
            let theta = (step as f32).to_radians();
            let matches: usize = spans.iter().filter(|s| s.contains(theta)).count();
            assert_eq!(
                matches, 1,
                "root n={n} angle={step} deg matched {matches} segments"
            );
        }
    }
}

#[test]
fn test_child_segment_distribution_all_quadrants() {
    let bridge_rad = 70.0_f32.to_radians();
    let max_delta = 72.0_f32.to_radians();

    for &beta_deg in &[0.0_f32, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0] {
        let beta = beta_deg.to_radians();
        for n in 1..=12 {
            let (bridge, spans) = child_segment_spans(n, beta, bridge_rad);
            assert_eq!(spans.len(), n);
            assert!((bridge.width() - bridge_rad).abs() < 1e-5);
            assert!(bridge.contains(beta));

            for sp in &spans {
                assert!(sp.width() <= max_delta + 1e-5);
                // Bridge center must never be inside an item span
                assert!(!sp.contains(beta));
            }

            // No two item spans overlap, and none overlap the bridge
            for step in 0..720 {
                let theta = ((step as f32) * 0.5).to_radians();
                let in_bridge = usize::from(bridge.contains(theta));
                let item_hits: usize = spans.iter().filter(|s| s.contains(theta)).count();
                assert!(
                    in_bridge + item_hits <= 1,
                    "overlap at beta={beta_deg}, n={n}, theta={theta}"
                );
            }
        }
    }
}

#[test]
fn test_gap_insets_and_content_geometry() {
    // Constant linear gap = 2.5 lp means angular inset at r_in=38 is larger than at r_out=116
    let d_in = gap_inset_angle(2.5, 38.0);
    let d_out = gap_inset_angle(2.5, 116.0);
    assert!(d_in > d_out);
    assert!((d_in * 38.0 - 1.25).abs() < 1e-5);
    assert!((d_out * 116.0 - 1.25).abs() < 1e-5);

    let r_c = segment_content_radius(38.0, 116.0);
    assert!((r_c - (38.0 + 0.55 * 78.0)).abs() < 1e-5);
    let max_w = segment_label_max_width(38.0, 116.0, FRAC_PI_2, 2.5);
    assert!(max_w > 60.0);
}

#[test]
fn test_corridor_and_tangent_geometry() {
    let c_p = Point::new(500.0, 400.0);
    let r_p = 116.0;
    let spans = root_segment_spans(4);
    let c_c = Point::new(500.0, 178.0);
    let r_c_hit = 96.0 + 12.0;

    let poly = child_corridor_polygon(c_p, r_p, spans[0], 2.5, c_c, r_c_hit);
    assert!(poly.len() >= 3);
    // Midpoint between parent top rim (500, 284) and child center (500, 178) must be inside corridor
    assert!(point_in_convex_polygon(Point::new(500.0, 260.0), &poly));
    // Point far to the side is outside corridor
    assert!(!point_in_convex_polygon(Point::new(300.0, 260.0), &poly));

    let anchor_corr = anchor_corridor_polygon(Point::new(10.0, 10.0), 24.0, c_p, 128.0);
    assert!(point_in_convex_polygon(
        Point::new(250.0, 200.0),
        &anchor_corr
    ));

    let (t1, t2) = circle_tangent_points(Point::new(500.0, 320.0), c_c, r_c_hit);
    assert!(point_in_triangle(
        Point::new(500.0, 280.0),
        Point::new(500.0, 320.0),
        t1,
        t2
    ));

    let hull = convex_hull_4([
        Point::new(0.0, 0.0),
        Point::new(10.0, 0.0),
        Point::new(10.0, 10.0),
        Point::new(0.0, 10.0),
    ]);
    assert_eq!(hull.len(), 4);
}

#[test]
fn test_hit_testing_hysteresis_and_submenu_overlap_guard() {
    let c_root = Point::new(500.0, 400.0);
    let root_spans = root_segment_spans(4);
    let root_geom = WheelHitGeometry {
        index: 0,
        parent_index: None,
        center: c_root,
        r_in: 38.0,
        r_out: 116.0,
        hit_radius: 128.0,
        bridge: None,
        spans: root_spans.clone(),
        corridor: None,
    };

    // Child wheel above item 0 (top): center at (500, 178), r_out = 96, hit_radius = 108
    // Notice bottom of child hit_radius is y = 178 + 108 = 286, which overlaps parent top r_out y = 400 - 116 = 284!
    let c_child = Point::new(500.0, 178.0);
    let beta = c_child.angle_to(c_root); // 90 deg (down)
    let (bridge, child_spans) = child_segment_spans(3, beta, 70.0_f32.to_radians());
    let corridor = child_corridor_polygon(c_root, 116.0, root_spans[0], 2.5, c_child, 108.0);
    let child_geom = WheelHitGeometry {
        index: 1,
        parent_index: Some(0),
        center: c_child,
        r_in: 30.0,
        r_out: 96.0,
        hit_radius: 108.0,
        bridge: Some(bridge),
        spans: child_spans,
        corridor: Some(corridor),
    };

    let wheels = [root_geom, child_geom];
    let hyst = 4.0_f32.to_radians();

    // 1. M0 S-1 Submenu Hit-Margin Overlap Guard:
    // At (500, 285), distance to root is 115 <= 116 (inside parent's drawn ring),
    // and distance to child is 107 <= 108 (inside child's hit_margin, outside child's r_out=96).
    // Hit test MUST return parent's Item(0, 0), NOT child's Bridge(1)!
    let p_overlap = Point::new(500.0, 285.0);
    let zone = hit_test(p_overlap, &wheels, Zone::Item { wheel: 0, index: 0 }, hyst);
    assert_eq!(zone, Zone::Item { wheel: 0, index: 0 });

    // Once pointer crosses into child's drawn ring (e.g. y = 270, d_child = 92 <= 96), it hits Bridge(1)
    let p_bridge = Point::new(500.0, 270.0);
    assert_eq!(
        hit_test(p_bridge, &wheels, Zone::Item { wheel: 0, index: 0 }, hyst),
        Zone::Bridge { wheel: 1 }
    );

    // 2. Radial hub hysteresis (+3 lp when already in Hub(0))
    let p_hub_edge = Point::new(500.0, 400.0 - 39.5); // d = 39.5, between 38 and 41
    assert_eq!(
        hit_test(p_hub_edge, &wheels, Zone::Item { wheel: 0, index: 0 }, hyst),
        Zone::Item { wheel: 0, index: 0 }
    );
    assert_eq!(
        hit_test(p_hub_edge, &wheels, Zone::Hub { wheel: 0 }, hyst),
        Zone::Hub { wheel: 0 }
    );

    // 3. Angular segment hysteresis (±4 deg)
    // Boundary between item 0 (225°..315°) and item 1 (315°..45°) is at 315° (-45°).
    // Test at 317° (2° past boundary into item 1):
    let theta_317 = 317.0_f32.to_radians();
    let p_boundary = c_root.offset_polar(75.0, theta_317);
    // When item 0 is currently hovered, 317° stays in item 0 (within 4° hysteresis)
    assert_eq!(
        hit_test(p_boundary, &wheels, Zone::Item { wheel: 0, index: 0 }, hyst),
        Zone::Item { wheel: 0, index: 0 }
    );
    // When item 1 is currently hovered (or Outside), 317° is in item 1
    assert_eq!(
        hit_test(p_boundary, &wheels, Zone::Outside, hyst),
        Zone::Item { wheel: 0, index: 1 }
    );
}

#[test]
fn test_layout_root_and_child_edge_adaptation_and_caption() {
    let cfg = WheelConfig::default();
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let (_, r_root) = nominal_radii(0, &cfg);

    // Root clamped at all 4 corners
    let c_tl = place_root(Point::new(5.0, 5.0), r_root, vp);
    assert_eq!(c_tl, Point::new(r_root + EDGE_MARGIN, r_root + EDGE_MARGIN));

    let c_br = place_root(Point::new(1915.0, 1075.0), r_root, vp);
    assert_eq!(
        c_br,
        Point::new(1920.0 - r_root - EDGE_MARGIN, 1080.0 - r_root - EDGE_MARGIN)
    );

    // Child of top segment when root is near top edge (500, 140) rotates toward viewport center
    let c_top = Point::new(500.0, 140.0);
    let (_, r_child) = nominal_radii(1, &cfg);
    let placement = place_child(c_top, r_root, 3.0 * FRAC_PI_2, r_child, &[], vp, &cfg);
    assert!(!placement.fallback);
    assert!(
        vp.bounds
            .contains_disc(placement.center, placement.r_out + EDGE_MARGIN)
    );
    // Should have rotated clockwise (toward +x / interior) from 270 deg
    assert!(angle_diff(placement.attach_angle, 3.0 * FRAC_PI_2) > 0.0);

    // Scaled inner radius proportional to outer radius shrink
    assert!((scaled_inner_radius(64.0, 30.0, 96.0) - 20.0).abs() < 1e-5);

    // Stem points
    let (p0, ctrl, p1) = compute_stem_points(
        c_top,
        r_root,
        3.0 * FRAC_PI_2,
        placement.center,
        placement.r_out,
        placement.attach_angle,
        cfg.child_gap,
    );
    assert!((p0.distance(c_top) - r_root).abs() < 1e-4);
    assert!((p1.distance(placement.center) - placement.r_out).abs() < 1e-4);
    assert!(ctrl.distance(c_top) > r_root);

    // Caption placement: Below normally, Above when near bottom edge
    let (cap_rect, cap_align) = place_caption(
        Point::new(500.0, 500.0),
        116.0,
        &[(Point::new(500.0, 500.0), 128.0)],
        &[],
        vp,
    );
    assert_eq!(cap_align, CaptionAlign::Below);
    assert_eq!(cap_rect.w, CAPTION_BOX_W);
    assert_eq!(cap_rect.h, CAPTION_BOX_H);

    let (_, cap_align_bottom) = place_caption(
        Point::new(500.0, 1080.0 - 130.0),
        116.0,
        &[(Point::new(500.0, 1080.0 - 130.0), 128.0)],
        &[],
        vp,
    );
    assert_eq!(cap_align_bottom, CaptionAlign::Above);
}

#[test]
fn test_tween_interpolation_and_retarget_continuity() {
    let mut tw = Tween::new(0.0, 100.0, 10, 100, Ease::OutCubic);
    assert_eq!(tw.value(5), 0.0);
    assert_eq!(tw.value(10), 0.0);
    assert!(!tw.done(50));
    let mid = tw.value(60);
    // OutCubic at t=0.5 is 1 - 0.5^3 = 0.875 -> 87.5
    assert!((mid - 87.5).abs() < 1e-4);
    assert_eq!(tw.value(110), 100.0);
    assert!(tw.done(110));

    // Retarget mid-flight at t=60 has no discontinuity jump
    tw.retarget(20.0, 60, 80, Ease::InOutCubic);
    assert!((tw.value(60) - mid).abs() < 1e-5);
    assert!(!tw.done(100));
    assert_eq!(tw.value(140), 20.0);
    assert!(tw.done(140));
    assert_eq!(timings::ROOT_APPEAR_MS, 140);
}

#[test]
fn test_menu_sanitization_overflow_depth_and_empty_pruning() {
    let cfg = WheelConfig::default(); // max_items_per_wheel = 8, max_depth = 4

    // 1. 10 items on root -> first 7 kept + 8th is "More…" submenu containing 3 overflow items
    let mut items = Vec::new();
    for i in 0..10 {
        items.push(MenuNode::leaf(format!("id.{i}"), format!("Item {i}")));
    }
    // Add an empty submenu that should be pruned
    items.push(MenuNode::submenu("empty.sub", "Empty", vec![]));

    let mut tree = MenuTree::new(items);
    assert!(tree.sanitize(&cfg));
    let root_children = tree.root.children().unwrap();
    assert_eq!(root_children.len(), 8);
    assert_eq!(&*root_children[7].id, "radivine.more");
    assert_eq!(&*root_children[7].label, "More…");
    let more_children = root_children[7].children().unwrap();
    assert_eq!(more_children.len(), 3);
    assert_eq!(&*more_children[0].id, "id.7");

    // 2. Menu exceeding max_depth (4) marks submenu at depth 4 as disabled with "Too deeply nested"
    let d5 = MenuNode::leaf("d5", "Level 5 Leaf");
    let d4 = MenuNode::submenu("d4", "Level 4 Sub", vec![d5]);
    let d3 = MenuNode::submenu("d3", "Level 3 Sub", vec![d4]);
    let d2 = MenuNode::submenu("d2", "Level 2 Sub", vec![d3]);
    let d1 = MenuNode::submenu("d1", "Level 1 Sub", vec![d2]);
    let mut deep_tree = MenuTree::new(vec![d1]);
    assert!(deep_tree.sanitize(&cfg));

    // Path [0] = d1 (depth 1 -> opens wheel at depth 2)
    // Path [0, 0] = d2 (depth 2 -> opens wheel at depth 3)
    // Path [0, 0, 0] = d3 (depth 3 -> opens wheel at depth 4)
    // Path [0, 0, 0, 0] = d4 ( sits in wheel at depth 4; cannot open wheel at depth 5!)
    let node_d3 = deep_tree.node(&ItemPath::from_slice(&[0, 0, 0])).unwrap();
    assert!(node_d3.enabled);
    let node_d4 = deep_tree
        .node(&ItemPath::from_slice(&[0, 0, 0, 0]))
        .unwrap();
    assert!(!node_d4.enabled);
    assert_eq!(
        node_d4.disabled_reason.as_deref(),
        Some("Too deeply nested")
    );

    // 3. Root with only empty submenus returns false on sanitize
    let mut empty_tree = MenuTree::new(vec![MenuNode::submenu(
        "empty",
        "Empty",
        vec![MenuNode::submenu("nested.empty", "Nested Empty", vec![])],
    )]);
    assert!(!empty_tree.sanitize(&cfg));

    // 4. Hard limits (>12 items and depth >6) clamped gracefully without panic (AC-1a.5)
    let mut hard_cfg = WheelConfig {
        max_items_per_wheel: 20, // will be clamped to hard cap 12
        max_depth: 10,           // will be clamped to hard cap 6
        ..WheelConfig::default()
    };
    hard_cfg.validate();
    assert_eq!(hard_cfg.max_items_per_wheel, 12);
    assert_eq!(hard_cfg.max_depth, 6);

    let mut sixteen_items = Vec::new();
    for i in 0..16 {
        sixteen_items.push(MenuNode::leaf(format!("item.{i}"), format!("Item {i}")));
    }
    let mut big_tree = MenuTree::new(sixteen_items);
    assert!(big_tree.sanitize(&hard_cfg));
    assert_eq!(big_tree.root.children().unwrap().len(), 12);
    assert_eq!(
        big_tree.root.children().unwrap()[11]
            .children()
            .unwrap()
            .len(),
        5
    );

    let mut cur = MenuNode::leaf("d8", "Depth 8 Leaf");
    for d in (1..=7).rev() {
        cur = MenuNode::submenu(format!("d{d}"), format!("Depth {d}"), vec![cur]);
    }
    let mut eight_deep = MenuTree::new(vec![cur]);
    assert!(eight_deep.sanitize(&hard_cfg));
    // At depth 6 (path length 6: [0, 0, 0, 0, 0, 0]), submenu is disabled with "Too deeply nested"
    let d6_node = eight_deep
        .node(&ItemPath::from_slice(&[0, 0, 0, 0, 0, 0]))
        .unwrap();
    assert!(!d6_node.enabled);
    assert_eq!(
        d6_node.disabled_reason.as_deref(),
        Some("Too deeply nested")
    );
}

fn branching_regression_menu() -> MenuTree {
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
        .with_default_child(0)
        .with_icon("lucide:eye")
        .with_description("Preview file contents"),
        MenuNode::leaf("radivine.copy", "Copy")
            .with_icon("lucide:copy")
            .with_description("Copy file to clipboard"),
        MenuNode::leaf("radivine.delete", "Delete")
            .with_disabled("Read-only filesystem")
            .with_icon("lucide:trash"),
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
fn test_n1_layout_selection_state_and_fallback_queries() {
    use radivine_wheel::{InputMode, LayoutFallback, LayoutSelection, WheelLayout, WheelSession};

    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let anchor = Point::new(500.0, 400.0);

    // 1. Default config selects Branching with no fallback
    let def_cfg = WheelConfig::default();
    assert_eq!(def_cfg.layout, WheelLayout::Branching);
    let session_def = WheelSession::new(
        Some(branching_regression_menu()),
        anchor,
        vp,
        InputMode::Drag,
        &def_cfg,
        0,
    );
    assert_eq!(
        session_def.layout_selection(),
        LayoutSelection {
            requested: WheelLayout::Branching,
            effective: WheelLayout::Branching,
            fallback: None,
        }
    );
    assert_eq!(
        session_def.debug_state().layout_selection,
        LayoutSelection::branching()
    );

    // 2. Explicit Concentric config in a fitting viewport selects Concentric;
    //    platform-level or preflight fallback records the exact reason and switches to Branching.
    let conc_cfg = WheelConfig {
        layout: WheelLayout::Concentric,
        ..WheelConfig::default()
    };
    assert_eq!(conc_cfg.validated().layout, WheelLayout::Concentric);
    let mut session_conc = WheelSession::new(
        Some(branching_regression_menu()),
        anchor,
        vp,
        InputMode::Drag,
        &conc_cfg,
        0,
    );
    assert_eq!(
        session_conc.layout_selection(),
        LayoutSelection::concentric()
    );

    // 3. Platform-level fallback recording (`NoCompositor` / `InsufficientSpace` / `TargetTooSmall`)
    session_conc.record_layout_fallback(LayoutFallback::NoCompositor);
    assert_eq!(
        session_conc.layout_selection(),
        LayoutSelection::fallback_to_branching(LayoutFallback::NoCompositor)
    );
    assert_eq!(
        LayoutSelection::concentric(),
        LayoutSelection {
            requested: WheelLayout::Concentric,
            effective: WheelLayout::Concentric,
            fallback: None,
        }
    );
    assert_eq!(
        LayoutSelection::fallback_to_branching(LayoutFallback::InsufficientSpace).fallback,
        Some(LayoutFallback::InsufficientSpace)
    );
    assert_eq!(
        LayoutSelection::fallback_to_branching(LayoutFallback::TargetTooSmall).fallback,
        Some(LayoutFallback::TargetTooSmall)
    );
}

#[test]
fn test_t_reg1_branching_deterministic_baseline_traces() {
    use radivine_wheel::{
        DismissReason, DropTarget, InputMode, NavKey, SessionPhase, WheelEvent, WheelInput,
        WheelLayout, WheelSession,
    };
    use std::sync::Arc;

    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let anchor = Point::new(500.0, 400.0);
    let cfg_default = WheelConfig::default();
    let cfg_explicit_branching = WheelConfig {
        layout: WheelLayout::Branching,
        ..WheelConfig::default()
    };

    // --- 1. Slop, defaults, hysteresis, child overlap, bridge, enablement, and interrupted animation ---
    // Run identically on both implicit default and explicit Branching configs and assert step-by-step parity.
    let mut s_def = WheelSession::new(
        Some(branching_regression_menu()),
        anchor,
        vp,
        InputMode::Drag,
        &cfg_default,
        0,
    );
    let mut s_exp = WheelSession::new(
        Some(branching_regression_menu()),
        anchor,
        vp,
        InputMode::Drag,
        &cfg_explicit_branching,
        0,
    );

    let assert_sessions_identical = |a: &WheelSession, b: &WheelSession| {
        assert_eq!(a.layout_selection(), b.layout_selection());
        assert_eq!(a.drop_target(), b.drop_target());
        assert_eq!(a.next_deadline(), b.next_deadline());
        assert_eq!(a.is_animating(), b.is_animating());
        assert_eq!(a.is_finished(), b.is_finished());
        assert_eq!(a.input_region(), b.input_region());
        assert_eq!(a.scene(), b.scene());
        let da = a.debug_state();
        let db = b.debug_state();
        assert_eq!(da.phase, db.phase);
        assert_eq!(da.moved_past_slop, db.moved_past_slop);
        assert_eq!(da.raw_zone, db.raw_zone);
        assert_eq!(da.hovered_item, db.hovered_item);
        assert_eq!(da.open_chain, db.open_chain);
        assert_eq!(da.wheels.len(), db.wheels.len());
        for (wa, wb) in da.wheels.iter().zip(db.wheels.iter()) {
            assert_eq!(wa.index, wb.index);
            assert_eq!(wa.parent_index, wb.parent_index);
            assert_eq!(wa.center, wb.center);
            assert_eq!(wa.r_in, wb.r_in);
            assert_eq!(wa.r_out, wb.r_out);
            assert_eq!(wa.hit_radius, wb.hit_radius);
            assert_eq!(wa.bridge, wb.bridge);
            assert_eq!(wa.spans, wb.spans);
            assert_eq!(wa.corridor, wb.corridor);
        }
        assert_eq!(da.intent_triangle, db.intent_triangle);
        assert_eq!(da.dwell_deadline, db.dwell_deadline);
        assert_eq!(da.hub_close_deadline, db.hub_close_deadline);
    };

    assert_sessions_identical(&s_def, &s_exp);
    assert_eq!(s_def.scene().wheels.len(), 1);
    assert_eq!(s_def.scene().wheels[0].center, anchor);
    assert_eq!(s_def.scene().wheels[0].r_in, 38.0);
    assert_eq!(s_def.scene().wheels[0].r_out, 116.0);

    let mut ev_def = Vec::new();
    let mut ev_exp = Vec::new();

    // Step A: Slop guard (< 6.0 lp from anchor (500, 400))
    s_def.handle(
        WheelInput::PointerMoved(Point::new(500.0, 394.5)),
        10,
        &mut ev_def,
    );
    s_exp.handle(
        WheelInput::PointerMoved(Point::new(500.0, 394.5)),
        10,
        &mut ev_exp,
    );
    assert_eq!(ev_def, ev_exp);
    assert!(ev_def.is_empty());
    assert!(!s_def.debug_state().moved_past_slop);
    assert_eq!(s_def.drop_target(), DropTarget::Reject);
    assert_sessions_identical(&s_def, &s_exp);

    // Cross slop at 6.0 lp into Hub(0)
    s_def.handle(
        WheelInput::PointerMoved(Point::new(500.0, 394.0)),
        15,
        &mut ev_def,
    );
    s_exp.handle(
        WheelInput::PointerMoved(Point::new(500.0, 394.0)),
        15,
        &mut ev_exp,
    );
    assert_eq!(ev_def, ev_exp);
    assert!(s_def.debug_state().moved_past_slop);
    assert_eq!(s_def.debug_state().raw_zone, Zone::Hub { wheel: 0 });
    assert_sessions_identical(&s_def, &s_exp);

    // Step B: Radial hub hysteresis (+3 lp: d = 39.5 <= 41.0 stays in Hub(0))
    s_def.handle(
        WheelInput::PointerMoved(Point::new(500.0, 400.0 - 39.5)),
        18,
        &mut ev_def,
    );
    s_exp.handle(
        WheelInput::PointerMoved(Point::new(500.0, 400.0 - 39.5)),
        18,
        &mut ev_exp,
    );
    assert_eq!(s_def.debug_state().raw_zone, Zone::Hub { wheel: 0 });
    assert_sessions_identical(&s_def, &s_exp);

    // Step C: Enter submenu [0] ("File Preview", default_child = Some(0)) below outer-band threshold
    // Verify default_child acceptance and dwell timer scheduling (20 + 140 = 160 ms)
    ev_def.clear();
    ev_exp.clear();
    s_def.handle(
        WheelInput::PointerMoved(Point::new(500.0, 330.0)),
        20,
        &mut ev_def,
    );
    s_exp.handle(
        WheelInput::PointerMoved(Point::new(500.0, 330.0)),
        20,
        &mut ev_exp,
    );
    assert_eq!(
        ev_def,
        vec![WheelEvent::HoverChanged(Some(ItemPath::from_slice(&[0])))]
    );
    assert_eq!(ev_def, ev_exp);
    assert_eq!(
        s_def.drop_target(),
        DropTarget::Accept(ItemPath::from_slice(&[0, 0]))
    );
    assert_eq!(s_def.next_deadline(), Some(160));
    assert_sessions_identical(&s_def, &s_exp);

    // Step D: Angular segment hysteresis (±4° across 315° boundary between item 0 and item 1)
    let p_317_deg = anchor.offset_polar(75.0, 317.0_f32.to_radians());
    ev_def.clear();
    ev_exp.clear();
    s_def.handle(WheelInput::PointerMoved(p_317_deg), 30, &mut ev_def);
    s_exp.handle(WheelInput::PointerMoved(p_317_deg), 30, &mut ev_exp);
    assert!(ev_def.is_empty());
    assert_eq!(
        s_def.debug_state().raw_zone,
        Zone::Item { wheel: 0, index: 0 }
    );
    assert_sessions_identical(&s_def, &s_exp);

    // Step E: Advance to dwell deadline (t = 160) -> child wheel [0] opens at (500, 178)
    ev_def.clear();
    ev_exp.clear();
    s_def.tick(159, &mut ev_def);
    s_exp.tick(159, &mut ev_exp);
    assert!(ev_def.is_empty());
    assert_eq!(s_def.debug_state().open_chain.len(), 1);

    s_def.tick(160, &mut ev_def);
    s_exp.tick(160, &mut ev_exp);
    assert_eq!(
        ev_def,
        vec![
            WheelEvent::SubmenuOpened(ItemPath::from_slice(&[0])),
            WheelEvent::InputRegionChanged,
        ]
    );
    assert_eq!(ev_def, ev_exp);
    assert_eq!(s_def.debug_state().open_chain.len(), 2);
    let child_geom = s_def.wheel_geometry(&ItemPath::from_slice(&[0])).unwrap();
    assert!((child_geom.center.x - 500.0).abs() < 1e-4);
    assert!((child_geom.center.y - 178.0).abs() < 1e-4);
    assert_eq!(child_geom.r_in, 30.0);
    assert_eq!(child_geom.r_out, 96.0);
    assert_sessions_identical(&s_def, &s_exp);

    // Step F: M0 S-1 Submenu Hit-Margin Overlap Guard at (500, 285)
    // Inside parent drawn ring (d_root = 115 <= 116) and child hit_margin (d_child = 107 <= 108)
    ev_def.clear();
    ev_exp.clear();
    s_def.handle(
        WheelInput::PointerMoved(Point::new(500.0, 285.0)),
        175,
        &mut ev_def,
    );
    s_exp.handle(
        WheelInput::PointerMoved(Point::new(500.0, 285.0)),
        175,
        &mut ev_exp,
    );
    assert_eq!(
        s_def.debug_state().raw_zone,
        Zone::Item { wheel: 0, index: 0 }
    );
    assert_sessions_identical(&s_def, &s_exp);

    // Step G: Child Bridge zone at (500, 270) (d_child = 92 <= 96, facing parent)
    ev_def.clear();
    ev_exp.clear();
    s_def.handle(
        WheelInput::PointerMoved(Point::new(500.0, 270.0)),
        190,
        &mut ev_def,
    );
    s_exp.handle(
        WheelInput::PointerMoved(Point::new(500.0, 270.0)),
        190,
        &mut ev_exp,
    );
    assert_eq!(s_def.debug_state().raw_zone, Zone::Bridge { wheel: 1 });
    assert_eq!(
        s_def.debug_state().hovered_item,
        Some(ItemPath::from_slice(&[0]))
    );
    assert_eq!(s_def.debug_state().open_chain.len(), 2);
    assert_sessions_identical(&s_def, &s_exp);

    // Step H: Enablement update while hovering child leaf [0, 1] ("Head")
    let p_head = s_def.item_center(&ItemPath::from_slice(&[0, 1])).unwrap();
    ev_def.clear();
    ev_exp.clear();
    s_def.handle(WheelInput::PointerMoved(p_head), 210, &mut ev_def);
    s_exp.handle(WheelInput::PointerMoved(p_head), 210, &mut ev_exp);
    assert_eq!(
        s_def.drop_target(),
        DropTarget::Accept(ItemPath::from_slice(&[0, 1]))
    );

    s_def.set_enabled(
        &ItemPath::from_slice(&[0, 1]),
        false,
        Some(Arc::from("Binary file")),
    );
    s_exp.set_enabled(
        &ItemPath::from_slice(&[0, 1]),
        false,
        Some(Arc::from("Binary file")),
    );
    assert_eq!(s_def.drop_target(), DropTarget::Reject);
    assert_eq!(
        s_def.scene().caption.as_ref().unwrap().detail.as_deref(),
        Some("Binary file")
    );
    assert!(s_def.scene().caption.as_ref().unwrap().disabled);
    assert_sessions_identical(&s_def, &s_exp);

    s_def.set_enabled(&ItemPath::from_slice(&[0, 1]), true, None);
    s_exp.set_enabled(&ItemPath::from_slice(&[0, 1]), true, None);
    assert_eq!(
        s_def.drop_target(),
        DropTarget::Accept(ItemPath::from_slice(&[0, 1]))
    );
    assert_sessions_identical(&s_def, &s_exp);

    // Step I: Interrupted animation — dismiss mid-flight and verify single Finished event
    s_def.dismiss(DismissReason::Cancelled, 220);
    s_exp.dismiss(DismissReason::Cancelled, 220);
    assert_eq!(s_def.debug_state().phase, SessionPhase::Dismissing);
    assert!(s_def.input_region().is_empty());
    assert_sessions_identical(&s_def, &s_exp);

    ev_def.clear();
    ev_exp.clear();
    s_def.tick(270, &mut ev_def);
    s_exp.tick(270, &mut ev_exp);
    assert!(ev_def.is_empty());
    assert!(!s_def.is_finished());

    s_def.tick(320, &mut ev_def);
    s_exp.tick(320, &mut ev_exp);
    assert_eq!(ev_def, vec![WheelEvent::Finished]);
    assert_eq!(ev_def, ev_exp);
    assert!(s_def.is_finished());

    ev_def.clear();
    s_def.tick(400, &mut ev_def);
    assert!(ev_def.is_empty());

    // --- 2. Hover-intent tangent triangle protection & grace expiry near top edge ---
    let mut s_intent = WheelSession::new(
        Some(branching_regression_menu()),
        Point::new(500.0, 140.0),
        vp,
        InputMode::Drag,
        &cfg_explicit_branching,
        0,
    );
    let mut ev = Vec::new();
    s_intent.handle(
        WheelInput::PointerMoved(Point::new(500.0, 70.0)),
        10,
        &mut ev,
    );
    s_intent.tick(150, &mut ev);
    assert_eq!(s_intent.debug_state().open_chain.len(), 2);

    let child_c = s_intent
        .wheel_geometry(&ItemPath::from_slice(&[0]))
        .unwrap()
        .center;
    let root_g = s_intent.wheel_geometry(&ItemPath::root()).unwrap().clone();
    let sp1 = root_g.spans[1];
    let dir = root_g.center.angle_to(child_c);
    let mid = (sp1.a0 + sp1.a1) * 0.5;
    let span_w = sp1.a1 - sp1.a0;
    let theta = if angle_diff(dir, mid) < 0.0 {
        sp1.a0 + 0.22 * span_w
    } else {
        sp1.a1 - 0.22 * span_w
    };
    let diag_pt = root_g
        .center
        .offset_polar(root_g.r_in + 0.60 * (root_g.r_out - root_g.r_in), theta);

    ev.clear();
    s_intent.handle(WheelInput::PointerMoved(diag_pt), 180, &mut ev);
    assert!(s_intent.debug_state().intent_triangle.is_some());
    assert_eq!(s_intent.debug_state().open_chain.len(), 2);
    assert_eq!(
        s_intent.debug_state().hovered_item,
        Some(ItemPath::from_slice(&[0]))
    );

    // 1 ms before grace deadline (180 + 300 = 480): child [0] still protected
    s_intent.tick(479, &mut ev);
    assert_eq!(s_intent.debug_state().open_chain.len(), 2);

    // At deadline (480): child [0] closes and hover adopts sibling [1] ("Copy")
    ev.clear();
    s_intent.tick(480, &mut ev);
    assert_eq!(s_intent.debug_state().open_chain.len(), 1);
    assert_eq!(
        s_intent.debug_state().hovered_item,
        Some(ItemPath::from_slice(&[1]))
    );
    assert_eq!(
        s_intent.drop_target(),
        DropTarget::Accept(ItemPath::from_slice(&[1]))
    );

    // --- 3. Keyboard navigation trace in Pointer mode ---
    let mut s_key = WheelSession::new(
        Some(branching_regression_menu()),
        anchor,
        vp,
        InputMode::Pointer,
        &cfg_explicit_branching,
        0,
    );
    ev.clear();
    s_key.handle(WheelInput::Nav(NavKey::Next), 10, &mut ev);
    assert_eq!(
        s_key.debug_state().hovered_item,
        Some(ItemPath::from_slice(&[0]))
    );

    s_key.handle(WheelInput::Nav(NavKey::Prev), 20, &mut ev);
    assert_eq!(
        s_key.debug_state().hovered_item,
        Some(ItemPath::from_slice(&[3]))
    );

    s_key.handle(WheelInput::Nav(NavKey::Next), 30, &mut ev);
    s_key.handle(WheelInput::Nav(NavKey::Activate), 40, &mut ev);
    assert_eq!(
        s_key.debug_state().open_chain.last(),
        Some(&ItemPath::from_slice(&[0]))
    );
    assert_eq!(
        s_key.debug_state().hovered_item,
        Some(ItemPath::from_slice(&[0, 0]))
    );

    s_key.handle(WheelInput::Nav(NavKey::Back), 50, &mut ev);
    assert_eq!(s_key.debug_state().open_chain.len(), 1);
    assert_eq!(
        s_key.debug_state().hovered_item,
        Some(ItemPath::from_slice(&[0]))
    );

    ev.clear();
    s_key.handle(WheelInput::Nav(NavKey::Digit(2)), 60, &mut ev);
    assert!(ev.contains(&WheelEvent::Commit(ItemPath::from_slice(&[1]))));
}
