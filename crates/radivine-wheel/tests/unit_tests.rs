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
