//! Milestone N2 pure concentric preflight, annular geometry, and content-box tests
//! (`TEST_MATRIX.md` §T2: `T-GEO1`..`T-GEO5`, `T-GEO7`).

use std::f32::consts::{PI, TAU};

use radivine_wheel::geometry::{Rect, angle_diff, normalize_angle, segment_content_radius};
use radivine_wheel::{
    CONCENTRIC_ANGULAR_EPS, CONCENTRIC_CAPTION_GAP, CONCENTRIC_CAPTION_H, CONCENTRIC_CAPTION_W,
    CONCENTRIC_EDGE_PAD, CONCENTRIC_MIN_TARGET_CORE, CONCENTRIC_RADIAL_EPS, CONCENTRIC_RING_GAP,
    CONCENTRIC_RING_THICKNESS, ContentMode, InputMode, ItemPath, LayoutFallback, LayoutSelection,
    MenuNode, MenuTree, Point, Viewport, WheelConfig, WheelLayout, WheelSession,
    compute_sector_content, concentric_ring_radii, plan_concentric, ring_geometry, sector_box_fits,
    separator_half_angle_for_ring, target_core_fits_ring,
};

fn app_concentric_config() -> WheelConfig {
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
        dwell_ms: 110,
        band: 0.65,
        grace_ms: 180,
        hub_close_ms: 150,
        ..WheelConfig::default()
    }
}

fn sample_menu(count: usize) -> MenuTree {
    let items: Vec<MenuNode> = (0..count)
        .map(|i| {
            MenuNode::leaf(format!("action_{i}"), format!("Action {i}")).with_icon("lucide:copy")
        })
        .collect();
    MenuTree::new(items)
}

fn vp_with_origin(x: f32, y: f32, w: f32, h: f32, scale: f32) -> Viewport {
    Viewport {
        bounds: Rect::new(x, y, w, h),
        scale,
    }
}

/// `T-GEO1`: Counts `1..=12` for every open level; equal spans, clockwise order, item zero at top,
/// full-circle coverage minus separators; `n = 1` has no seam; `n = 0` placeholder has no targets.
#[test]
fn t_geo1_sector_spans_counts_top_start_and_placeholder() {
    let mut cfg = app_concentric_config();
    cfg.max_depth = 6;
    cfg.max_items_per_wheel = 12;
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let plan = plan_concentric(Point::new(960.0, 500.0), vp, &cfg).expect("6-level plan fits");

    let top_angle = 1.5 * PI;

    for level in 0..6u8 {
        // n = 0 placeholder: no spans, no content boxes, zero separator angle
        let g0 = ring_geometry(&plan, level, 0);
        assert_eq!(g0.level, level);
        assert_eq!(g0.separator_half_angle, 0.0);
        assert!(g0.spans.is_empty());
        assert!(g0.content_boxes.is_empty());

        // n = 1: single full-circle annulus without seam
        let g1 = ring_geometry(&plan, level, 1);
        assert_eq!(g1.spans.len(), 1);
        assert_eq!(g1.content_boxes.len(), 1);
        assert_eq!(
            g1.separator_half_angle, 0.0,
            "n=1 ring at level {level} must have no separator seam"
        );
        let sp0 = g1.spans[0];
        assert!(((sp0.a1 - sp0.a0) - TAU).abs() < 1e-5);
        assert!((normalize_angle(sp0.center) - top_angle).abs() < 1e-5);

        // n = 2..=12: equal spans, clockwise order, item 0 centered at 3π/2, full 2π coverage
        for count in 2..=12usize {
            let g = ring_geometry(&plan, level, count);
            assert_eq!(g.spans.len(), count);
            assert_eq!(g.content_boxes.len(), count);

            let expected_step = TAU / (count as f32);
            let expected_delta = separator_half_angle_for_ring(g.r_in, count);
            assert!((g.separator_half_angle - expected_delta).abs() < 1e-6);
            assert!(g.separator_half_angle > 0.0);

            // Item 0 is centered at top (3π/2)
            assert!(
                (normalize_angle(g.spans[0].center) - top_angle).abs() < 1e-5,
                "level {level} count {count}: item 0 center {} != {top_angle}",
                normalize_angle(g.spans[0].center)
            );

            let mut total_span = 0.0f32;
            let mut usable_span = 0.0f32;
            for (idx, span) in g.spans.iter().enumerate() {
                let width = span.a1 - span.a0;
                assert!(
                    (width - expected_step).abs() < 1e-5,
                    "level {level} count {count} item {idx}: span width {width} != {expected_step}"
                );
                let expected_center = normalize_angle(top_angle + (idx as f32) * expected_step);
                assert!(
                    angle_diff(span.center, expected_center).abs() < 1e-5,
                    "level {level} count {count} item {idx}: center {} != {expected_center}",
                    span.center
                );
                if idx > 0 {
                    let prev_end = g.spans[idx - 1].a1;
                    assert!(
                        angle_diff(span.a0, prev_end).abs() < 1e-5,
                        "adjacent nominal spans must touch at shared boundary ray"
                    );
                }
                total_span += width;
                usable_span += width - 2.0 * g.separator_half_angle;
            }
            assert!((total_span - TAU).abs() < 1e-4);
            let expected_usable = TAU - (count as f32) * 2.0 * g.separator_half_angle;
            assert!((usable_span - expected_usable).abs() < 1e-4);
        }
    }
}

/// `T-GEO2`: All centers equal; exact annular radii from I1; gaps 6 lp; no ancestor covered
/// by a child hub; child placement independent of parent angle.
#[test]
fn t_geo2_shared_center_exact_annular_radii_and_parent_angle_independence() {
    let mut cfg = app_concentric_config();
    cfg.max_depth = 6;
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let anchor = Point::new(820.0, 480.0);
    let plan = plan_concentric(anchor, vp, &cfg).expect("plan fits");

    let expected_radii: [(f32, f32); 6] = [
        (40.0, 128.0),
        (134.0, 194.0),
        (200.0, 260.0),
        (266.0, 326.0),
        (332.0, 392.0),
        (398.0, 458.0),
    ];
    assert_eq!(plan.radii.as_slice(), &expected_radii);
    assert_eq!(plan.reserved_radius, 458.0);

    for (level, &(exp_in, exp_out)) in expected_radii.iter().enumerate() {
        let (r_in, r_out) = concentric_ring_radii(level as u8, 40.0, 128.0);
        assert_eq!((r_in, r_out), (exp_in, exp_out));
        let rg = ring_geometry(&plan, level as u8, 6);
        assert_eq!(rg.center, plan.center);
        assert_eq!(rg.r_in, exp_in);
        assert_eq!(rg.r_out, exp_out);

        if level >= 1 {
            let prev_out = expected_radii[level - 1].1;
            assert_eq!(
                r_in - prev_out,
                CONCENTRIC_RING_GAP,
                "gap between level {} and {level} must be 6 lp",
                level - 1
            );
            assert_eq!(
                r_out - r_in,
                CONCENTRIC_RING_THICKNESS,
                "child ring {level} thickness must be 60 lp"
            );
            assert!(
                r_in > prev_out,
                "child ring {level} inner boundary must lie strictly outside ancestor outer boundary"
            );
        }
    }

    // Also verify pure engine default root (38..116)
    let expected_engine_radii: [(f32, f32); 4] = [
        (38.0, 116.0),
        (122.0, 182.0),
        (188.0, 248.0),
        (254.0, 314.0),
    ];
    for (level, &(exp_in, exp_out)) in expected_engine_radii.iter().enumerate() {
        assert_eq!(
            concentric_ring_radii(level as u8, 38.0, 116.0),
            (exp_in, exp_out)
        );
    }
}

/// `T-GEO3`: Viewports at minimum fit, `±0.01 lp`; corners/edges/interior, negative viewport origin,
/// scales `1`, `1.25`, `1.5`, `2`, `4`. Fit or explicit pre-session fallback, never clipped actionable geometry.
#[test]
fn t_geo3_viewport_fit_boundaries_negative_origins_scales_and_placeholder_consistency() {
    let cfg = app_concentric_config(); // depth 4 -> Rreserve = 326, required 668 x 742

    let min_w = 668.0f32;
    let min_h = 742.0f32;

    for scale in [1.0f32, 1.25, 1.5, 2.0, 4.0] {
        for (ox, oy) in [(0.0f32, 0.0f32), (-1920.0, -1080.0), (256.5, -412.25)] {
            // Exact minimum fit succeeds
            let vp_exact = vp_with_origin(ox, oy, min_w, min_h, scale);
            let plan = plan_concentric(Point::new(ox + 334.0, oy + 334.0), vp_exact, &cfg)
                .expect("exact minimum viewport must fit");
            assert!((plan.center.x - (ox + 334.0)).abs() < 1e-4);
            assert!((plan.center.y - (oy + 334.0)).abs() < 1e-4);
            assert!(!plan.approach_required);

            // +0.01 lp fits
            let vp_plus = vp_with_origin(ox, oy, min_w + 0.01, min_h + 0.01, scale);
            assert!(plan_concentric(Point::new(ox + 334.0, oy + 334.0), vp_plus, &cfg).is_ok());

            // -0.01 lp in width or height fails with InsufficientSpace
            let vp_narrow = vp_with_origin(ox, oy, min_w - 0.01, min_h, scale);
            assert_eq!(
                plan_concentric(Point::new(ox + 334.0, oy + 334.0), vp_narrow, &cfg),
                Err(LayoutFallback::InsufficientSpace)
            );
            let vp_short = vp_with_origin(ox, oy, min_w, min_h - 0.01, scale);
            assert_eq!(
                plan_concentric(Point::new(ox + 334.0, oy + 334.0), vp_short, &cfg),
                Err(LayoutFallback::InsufficientSpace)
            );
        }
    }

    // Test corners, edges, and interior clamping inside a 1920x1080 viewport with negative origin
    let vp = vp_with_origin(-1000.0, -500.0, 1920.0, 1080.0, 1.5);
    let anchors = [
        (Point::new(-1000.0, -500.0), true),          // Top-left corner
        (Point::new(920.0, -500.0), true),            // Top-right corner
        (Point::new(-1000.0, 580.0), true),           // Bottom-left corner
        (Point::new(920.0, 580.0), true),             // Bottom-right corner
        (Point::new(-40.0, -500.0), true),            // Top edge
        (Point::new(-40.0, 580.0), true),             // Bottom edge
        (Point::new(-1000.0, 40.0), true),            // Left edge
        (Point::new(920.0, 40.0), true),              // Right edge
        (Point::new(-40.0, 0.0), false),              // Interior (no clamping)
        (Point::new(-1000.0 + 334.3, -166.0), false), // Within 0.5 lp of clamped boundary
    ];

    for (anchor, expect_approach) in anchors {
        let plan = plan_concentric(anchor, vp, &cfg).expect("1920x1080 fits");
        assert_eq!(
            plan.approach_required, expect_approach,
            "anchor {anchor:?} approach_required mismatch"
        );

        // Verify entire reserved cluster + caption stays >= 8 lp inside viewport bounds
        let b = vp.bounds;
        assert!(plan.center.x - plan.reserved_radius >= b.x + CONCENTRIC_EDGE_PAD - 1e-4);
        assert!(plan.center.x + plan.reserved_radius <= b.right() - CONCENTRIC_EDGE_PAD + 1e-4);
        assert!(plan.center.y - plan.reserved_radius >= b.y + CONCENTRIC_EDGE_PAD - 1e-4);
        assert!(plan.caption_box.x >= b.x + CONCENTRIC_EDGE_PAD - 1e-4);
        assert!(plan.caption_box.right() <= b.right() - CONCENTRIC_EDGE_PAD + 1e-4);
        assert!(plan.caption_box.bottom() <= b.bottom() - CONCENTRIC_EDGE_PAD + 1e-4);

        // Verify placeholder (`menu = None`) and `set_menu_at_pointer` keep identical center & layout
        let mut s_placeholder = WheelSession::new(None, anchor, vp, InputMode::Drag, &cfg, 0);
        let s_direct =
            WheelSession::new(Some(sample_menu(6)), anchor, vp, InputMode::Drag, &cfg, 0);
        assert_eq!(
            s_placeholder.layout_selection(),
            LayoutSelection::concentric()
        );
        assert_eq!(
            s_placeholder.scene().wheels[0].center,
            s_direct.scene().wheels[0].center
        );
        s_placeholder.set_menu_at_pointer(sample_menu(6), anchor, 5);
        assert_eq!(
            s_placeholder.scene().wheels[0].center,
            s_direct.scene().wheels[0].center
        );
        assert_eq!(
            s_placeholder
                .wheel_geometry(&ItemPath::root())
                .unwrap()
                .spans,
            s_direct.wheel_geometry(&ItemPath::root()).unwrap().spans
        );
    }
}

/// `T-GEO4`: Use shipped app root `40 / 128`: four rings `R = 326`, required `668 × 742`;
/// six rings `R = 458`, required `932 × 1006`.
/// `1366×768@1` fits; `1280×720@1` falls back; `3840×2160@2` fits; `1920×1080@2` falls back.
#[test]
fn t_geo4_shipped_app_root_standard_resolutions_and_dpi_matrix() {
    let cfg_4 = app_concentric_config();
    let mut cfg_6 = app_concentric_config();
    cfg_6.max_depth = 6;

    // 1. Verify exact reserved radii and required dimensions for 4 and 6 rings
    let plan_4 = plan_concentric(
        Point::new(500.0, 400.0),
        Viewport::new(668.0, 742.0, 1.0),
        &cfg_4,
    )
    .expect("668x742 must fit 4 rings");
    assert_eq!(plan_4.reserved_radius, 326.0);

    let plan_6 = plan_concentric(
        Point::new(500.0, 500.0),
        Viewport::new(932.0, 1006.0, 1.0),
        &cfg_6,
    )
    .expect("932x1006 must fit 6 rings");
    assert_eq!(plan_6.reserved_radius, 458.0);
    assert_eq!(
        plan_concentric(
            Point::new(500.0, 500.0),
            Viewport::new(931.99, 1006.0, 1.0),
            &cfg_6
        ),
        Err(LayoutFallback::InsufficientSpace)
    );
    assert_eq!(
        plan_concentric(
            Point::new(500.0, 500.0),
            Viewport::new(932.0, 1005.99, 1.0),
            &cfg_6
        ),
        Err(LayoutFallback::InsufficientSpace)
    );

    // 2. 1366x768 @ 1x -> fits 4 rings
    let vp_1366 = Viewport::new(1366.0, 768.0, 1.0);
    assert!(plan_concentric(Point::new(683.0, 384.0), vp_1366, &cfg_4).is_ok());
    let s_1366 = WheelSession::new(
        Some(sample_menu(4)),
        Point::new(683.0, 384.0),
        vp_1366,
        InputMode::Drag,
        &cfg_4,
        0,
    );
    assert_eq!(s_1366.layout_selection(), LayoutSelection::concentric());

    // 3. 1280x720 @ 1x -> falls back (720 < 742)
    let vp_720p = Viewport::new(1280.0, 720.0, 1.0);
    assert_eq!(
        plan_concentric(Point::new(640.0, 360.0), vp_720p, &cfg_4),
        Err(LayoutFallback::InsufficientSpace)
    );
    let s_720p = WheelSession::new(
        Some(sample_menu(4)),
        Point::new(640.0, 360.0),
        vp_720p,
        InputMode::Drag,
        &cfg_4,
        0,
    );
    assert_eq!(
        s_720p.layout_selection(),
        LayoutSelection::fallback_to_branching(LayoutFallback::InsufficientSpace)
    );

    // 4. 3840x2160 @ 2x (1920x1080 logical) -> fits both 4 and 6 rings
    let vp_4k_2x = Viewport::new(3840.0 / 2.0, 2160.0 / 2.0, 2.0);
    assert!(plan_concentric(Point::new(960.0, 540.0), vp_4k_2x, &cfg_4).is_ok());
    assert!(plan_concentric(Point::new(960.0, 540.0), vp_4k_2x, &cfg_6).is_ok());

    // 5. 1920x1080 @ 2x (960x540 logical) -> falls back (540 < 742)
    let vp_1080p_2x = Viewport::new(1920.0 / 2.0, 1080.0 / 2.0, 2.0);
    assert_eq!(
        plan_concentric(Point::new(480.0, 270.0), vp_1080p_2x, &cfg_4),
        Err(LayoutFallback::InsufficientSpace)
    );
    let s_1080p_2x = WheelSession::new(
        Some(sample_menu(4)),
        Point::new(480.0, 270.0),
        vp_1080p_2x,
        InputMode::Drag,
        &cfg_4,
        0,
    );
    assert_eq!(
        s_1080p_2x.layout_selection(),
        LayoutSelection::fallback_to_branching(LayoutFallback::InsufficientSpace)
    );
}

/// `T-GEO5`: Minimum target rectangles all lie within sector interiors. Small/nonfinite/invalid radii,
/// extreme gap/hysteresis config and zero-sized viewport fail safely; no NaN/panic/division by zero.
#[test]
fn t_geo5_target_core_containment_and_invalid_config_safety() {
    let mut cfg = app_concentric_config();
    cfg.max_depth = 6;
    cfg.max_items_per_wheel = 12;
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let plan = plan_concentric(Point::new(960.0, 500.0), vp, &cfg).expect("valid config fits");

    // 1. Verify that for every level 0..6 and count 1..=12, the 32x32 radial/tangential target core
    //    centered at `rc = r_in + 0.55*(r_out - r_in)` lies strictly inside the usable sector interior.
    let half = CONCENTRIC_MIN_TARGET_CORE * 0.5;
    for &(r_in, r_out) in &plan.radii {
        for count in 1..=12usize {
            assert!(target_core_fits_ring(r_in, r_out, count));
            let rc = segment_content_radius(r_in, r_out);
            let delta = separator_half_angle_for_ring(r_in, count);
            let half_span = PI / (count as f32);

            for &(u, v) in &[
                (rc - half, -half),
                (rc - half, 0.0),
                (rc - half, half),
                (rc + half, -half),
                (rc + half, 0.0),
                (rc + half, half),
            ] {
                let r = u.hypot(v);
                assert!(r > r_in + CONCENTRIC_RADIAL_EPS);
                assert!(r < r_out - CONCENTRIC_RADIAL_EPS);
                if count > 1 {
                    let theta = v.atan2(u).abs();
                    assert!(theta < half_span - delta - CONCENTRIC_ANGULAR_EPS);
                }
            }
        }
    }

    // 2. Small / thin / invalid root radii fail with `TargetTooSmall` and fall back cleanly
    let invalid_configs = [
        // Root ring thinner than 60 lp
        WheelConfig {
            radius_root: [40.0, 99.9],
            ..app_concentric_config()
        },
        // Root inner radius too small for 12 items' 32x32 tangential core
        WheelConfig {
            radius_root: [16.0, 76.0],
            max_items_per_wheel: 12,
            ..app_concentric_config()
        },
        // Out-of-bounds root radii
        WheelConfig {
            radius_root: [10.0, 128.0],
            ..app_concentric_config()
        },
        WheelConfig {
            radius_root: [40.0, 300.0],
            ..app_concentric_config()
        },
        // Non-finite radii and parameters
        WheelConfig {
            radius_root: [f32::NAN, 128.0],
            ..app_concentric_config()
        },
        WheelConfig {
            radius_root: [40.0, f32::INFINITY],
            ..app_concentric_config()
        },
        WheelConfig {
            hysteresis_deg: f32::NAN,
            ..app_concentric_config()
        },
        WheelConfig {
            segment_gap: f32::NEG_INFINITY,
            ..app_concentric_config()
        },
    ];

    for bad_cfg in invalid_configs {
        assert_eq!(
            plan_concentric(Point::new(960.0, 500.0), vp, &bad_cfg),
            Err(LayoutFallback::TargetTooSmall),
            "bad config {bad_cfg:?} must fail with TargetTooSmall"
        );
        let session = WheelSession::new(
            Some(sample_menu(4)),
            Point::new(960.0, 500.0),
            vp,
            InputMode::Drag,
            &bad_cfg,
            0,
        );
        assert_eq!(
            session.layout_selection(),
            LayoutSelection::fallback_to_branching(LayoutFallback::TargetTooSmall)
        );
        assert!(session.scene().wheels[0].center.x.is_finite());
        assert!(session.scene().wheels[0].center.y.is_finite());
    }

    // 3. Zero-sized, negative-sized, and non-finite viewports / anchors fail safely with InsufficientSpace
    let bad_viewports = [
        (Point::new(100.0, 100.0), Viewport::new(0.0, 0.0, 1.0)),
        (Point::new(100.0, 100.0), Viewport::new(-668.0, 742.0, 1.0)),
        (Point::new(100.0, 100.0), Viewport::new(1920.0, 1080.0, 0.0)),
        (
            Point::new(100.0, 100.0),
            Viewport::new(1920.0, 1080.0, f32::NAN),
        ),
        (
            Point::new(f32::NAN, 100.0),
            Viewport::new(1920.0, 1080.0, 1.0),
        ),
    ];
    for (pt, bad_vp) in bad_viewports {
        assert_eq!(
            plan_concentric(pt, bad_vp, &app_concentric_config()),
            Err(LayoutFallback::InsufficientSpace)
        );
    }
}

/// `T-GEO7`: Content boxes and reservation bounds lie inside their sector material / viewport;
/// long Unicode and missing-icon fixtures select valid `ContentMode` without overflowing sector material.
#[test]
fn t_geo7_sector_content_boxes_and_caption_reservation_containment() {
    let mut cfg = app_concentric_config();
    cfg.max_depth = 6;
    cfg.max_items_per_wheel = 12;
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let plan = plan_concentric(Point::new(960.0, 500.0), vp, &cfg).expect("plan fits");

    // 1. Caption reservation is 280x64 lp, 10 lp below Rreserve, and disjoint from all rings
    assert_eq!(plan.caption_box.w, CONCENTRIC_CAPTION_W);
    assert_eq!(plan.caption_box.h, CONCENTRIC_CAPTION_H);
    assert!(
        (plan.caption_box.y - (plan.center.y + plan.reserved_radius + CONCENTRIC_CAPTION_GAP))
            .abs()
            < 1e-4
    );
    let closest_caption_pt = plan.caption_box.clamp_point(plan.center);
    assert!(
        closest_caption_pt.distance(plan.center)
            >= plan.reserved_radius + CONCENTRIC_CAPTION_GAP - 1e-4
    );

    // 2. Every sector content box across levels 0..6 and counts 1..=12 (with and without icon)
    //    lies inside its annular sector with 4 lp clearance (`sector_box_fits`).
    for level in 0..6u8 {
        for count in 1..=12usize {
            let rg = ring_geometry(&plan, level, count);
            for (idx, &span) in rg.spans.iter().enumerate() {
                for has_icon in [true, false] {
                    let content = compute_sector_content(
                        rg.center,
                        rg.r_in,
                        rg.r_out,
                        span,
                        rg.separator_half_angle,
                        count,
                        has_icon,
                    );
                    let r_c = segment_content_radius(rg.r_in, rg.r_out);
                    let p_c = rg.center.offset_polar(r_c, span.center);
                    assert!(
                        sector_box_fits(
                            rg.center,
                            rg.r_in,
                            rg.r_out,
                            span,
                            rg.separator_half_angle,
                            count,
                            p_c,
                            content.rect.w,
                            content.rect.h,
                        ),
                        "level {level} count {count} sector {idx} has_icon={has_icon}: box {:?} ({:?}) must fit with 4 lp clearance",
                        content.rect,
                        content.mode
                    );
                    match content.mode {
                        ContentMode::IconAndLabel => {
                            assert!(content.rect.w >= 48.0 && content.rect.w <= 128.0);
                            assert_eq!(content.rect.h, 37.0);
                        }
                        ContentMode::IconOnly => {
                            assert!(has_icon);
                            assert_eq!(content.rect.w, 20.0);
                            assert_eq!(content.rect.h, 20.0);
                        }
                        ContentMode::LabelOnly => {
                            assert!(!has_icon);
                            assert!(content.rect.w >= 24.0 && content.rect.w <= 128.0);
                            assert_eq!(content.rect.h, 14.0);
                        }
                    }
                }
            }
        }
    }

    // 3. Long Unicode / Arabic / missing-icon menu fixture preserves full labels in scene/diagnostics
    let unicode_items = vec![
        MenuNode::leaf(
            "arabic_convert",
            "تحويل الصورة إلى صيغة عالية الجودة مع الحفاظ على البيانات الوصفية الكاملة",
        )
        .with_description("وصف طويل جداً لاختبار صندوق التسمية التوضيحية في الوضع المتمركز"),
        MenuNode::leaf(
            "long_latin",
            "Supercalifragilisticexpialidocious_File_Action_With_Extremely_Long_Identifier_0123456789",
        )
        .with_icon("lucide:sparkles"),
    ];
    let unicode_menu = MenuTree::new(unicode_items);
    let session = WheelSession::new(
        Some(unicode_menu),
        Point::new(960.0, 500.0),
        vp,
        InputMode::Drag,
        &cfg,
        0,
    );
    assert_eq!(session.scene().wheels[0].segments.len(), 2);
    assert!(
        session.scene().wheels[0].segments[0]
            .label
            .contains("تحويل الصورة")
    );
    assert!(session.scene().wheels[0].segments[0].icon.is_none());
    assert_eq!(
        Rect::new(
            plan.caption_box.x,
            plan.caption_box.y,
            plan.caption_box.w,
            plan.caption_box.h
        ),
        plan.caption_box
    );
}
