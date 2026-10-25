//! Milestone N3 acceptance and regression tests for `radivine-render`
//! (`T-REN1`..`T-REN7`, `T-NAV10`..`T-NAV12`, `T-REG1`, `T-REG2`).

use std::sync::Arc;

use radivine_render::{AssetSource, RectPx, Renderer, SizePx, Theme};
use radivine_wheel::{
    ContentMode, InputMode, ItemPath, MenuNode, MenuTree, NavKey, Point, Scene, SelectionVisual,
    Viewport, WheelConfig, WheelInput, WheelLayout, WheelSession, WheelVisualKind,
};

fn build_deep_menu(depth: usize, breadth: usize) -> MenuTree {
    fn build_level(level: usize, max_depth: usize, breadth: usize) -> Vec<MenuNode> {
        (0..breadth)
            .map(|i| {
                let id = format!("node.d{level}.i{i}");
                let label = format!("Level {level} Item {i}");
                let icon = match i % 4 {
                    0 => "lucide:eye",
                    1 => "lucide:copy",
                    2 => "lucide:sparkles",
                    _ => "lucide:terminal",
                };
                if level + 1 < max_depth && i < 2 {
                    MenuNode::submenu(id, label, build_level(level + 1, max_depth, breadth))
                        .with_icon(icon)
                        .with_description(format!("Submenu at depth {level} index {i}"))
                } else {
                    MenuNode::leaf(id, label)
                        .with_icon(icon)
                        .with_description(format!("Action at depth {level} index {i}"))
                }
            })
            .collect()
    }
    MenuTree::new(build_level(0, depth, breadth))
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn point_in_any_rect(x: u32, y: u32, rects: &[RectPx]) -> bool {
    let xi = x as i32;
    let yi = y as i32;
    rects
        .iter()
        .any(|r| xi >= r.x && xi < r.x + (r.w as i32) && yi >= r.y && yi < r.y + (r.h as i32))
}

/// Verifies `T-REN3` and `T-REN4`:
/// 1. Incremental `inc.render(scene)` produces the exact same framebuffer bytes as a fresh
///    `Renderer` rendering `scene` from scratch.
/// 2. Returned damage rect count is `<= 4` and within framebuffer bounds.
/// 3. Every pixel that changed between the previous incremental frame and the new frame lies
///    inside at least one returned damage rectangle.
/// 4. Re-rendering the identical `scene` immediately produces `0` damage rectangles.
fn assert_incremental_matches_fresh(
    inc: &mut Renderer,
    scene: &Scene,
    size: SizePx,
    scale: f32,
    theme: Theme,
    step_label: &str,
) {
    let prev_bytes = inc.frame().data.to_vec();
    let rects = inc.render(scene).to_vec();

    assert!(
        rects.len() <= 4,
        "[{step_label}] expected <= 4 damage rects, got {}",
        rects.len()
    );
    for r in &rects {
        assert!(
            r.x >= 0 && r.y >= 0 && (r.x as u32) + r.w <= size.w && (r.y as u32) + r.h <= size.h,
            "[{step_label}] damage rect {r:?} out of bounds {size:?}"
        );
    }

    // Check that every changed pixel is inside `rects`
    let cur_bytes = inc.frame().data;
    for y in 0..size.h {
        for x in 0..size.w {
            let idx = ((y * size.w + x) * 4) as usize;
            if prev_bytes[idx..idx + 4] != cur_bytes[idx..idx + 4] {
                assert!(
                    point_in_any_rect(x, y, &rects),
                    "[{step_label}] changed pixel at ({x}, {y}) outside damage rects {rects:?}"
                );
            }
        }
    }

    // Fresh full render comparison
    let mut fresh = Renderer::new(theme, AssetSource::Embedded).unwrap();
    fresh.begin_session(size, scale);
    fresh.render(scene);
    if inc.frame().data != fresh.frame().data {
        let mut first_diff = None;
        let mut diff_count = 0usize;
        for y in 0..size.h {
            for x in 0..size.w {
                let idx = ((y * size.w + x) * 4) as usize;
                if inc.frame().data[idx..idx + 4] != fresh.frame().data[idx..idx + 4] {
                    diff_count += 1;
                    if first_diff.is_none() {
                        first_diff = Some((
                            x,
                            y,
                            [
                                inc.frame().data[idx],
                                inc.frame().data[idx + 1],
                                inc.frame().data[idx + 2],
                                inc.frame().data[idx + 3],
                            ],
                            [
                                fresh.frame().data[idx],
                                fresh.frame().data[idx + 1],
                                fresh.frame().data[idx + 2],
                                fresh.frame().data[idx + 3],
                            ],
                        ));
                    }
                }
            }
        }
        panic!(
            "[{step_label}] incremental render != fresh render: {diff_count} differing pixels, first={first_diff:?}, rects={rects:?}"
        );
    }

    // Re-rendering identical scene must return 0 damage rects
    let repeat_rects = inc.render(scene);
    assert!(
        repeat_rects.is_empty(),
        "[{step_label}] identical scene produced non-empty damage: {repeat_rects:?}"
    );
}

#[test]
fn t_ren1_concentric_1_to_6_levels_single_hub_no_disc_shadow_no_stems_path_markers_breadcrumb() {
    let cfg = WheelConfig {
        layout: WheelLayout::Concentric,
        max_depth: 6,
        ..WheelConfig::default()
    };
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let mut events = Vec::new();

    let mut session = WheelSession::new(
        Some(build_deep_menu(6, 6)),
        Point::new(960.0, 540.0),
        vp,
        InputMode::Drag,
        &cfg,
        0,
    );
    session.tick(250, &mut events);

    let mut renderer = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
    renderer.begin_session(SizePx::new(1920, 1080), 1.0);

    // Depth 1 (root only)
    let sc1 = session.scene();
    assert_eq!(sc1.wheels.len(), 1);
    assert_eq!(sc1.wheels[0].kind, WheelVisualKind::ConcentricRoot);
    assert!(sc1.stems.is_empty());
    renderer.render(sc1);

    // Record root pixel at an ancestor sector center when only level 0 is open
    let root_item2_pt = session.item_center(&ItemPath::from_slice(&[2])).unwrap();
    let root_center = sc1.wheels[0].center;

    // Open levels 2, 3, 4, 5, 6 sequentially
    let mut current_path: Vec<u8> = Vec::new();
    let mut time_ms = 260u64;
    for target_depth in 2..=6 {
        current_path.push(0);
        let p = session
            .item_center(&ItemPath::from_slice(&current_path))
            .unwrap();
        session.handle(WheelInput::PointerMoved(p), time_ms, &mut events);
        session.handle(
            WheelInput::PointerMoved(Point::new(p.x + 3.0, p.y)),
            time_ms + 5,
            &mut events,
        );
        time_ms += 150;
        session.tick(time_ms, &mut events);
        time_ms += 250;
        session.tick(time_ms, &mut events);

        let sc = session.scene();
        assert_eq!(
            sc.wheels.len(),
            target_depth,
            "expected {target_depth} concentric rings open"
        );
        assert!(sc.stems.is_empty(), "concentric mode must never emit stems");

        for (idx, w) in sc.wheels.iter().enumerate() {
            assert_eq!(w.center, root_center, "all rings must share one center");
            if idx + 1 < target_depth {
                assert!(
                    (w.opacity - 0.72).abs() < 1e-4,
                    "all open ancestor rings must have settled opacity 0.72, got {} at level {idx}",
                    w.opacity
                );
            } else {
                assert!(
                    (w.opacity - 1.0).abs() < 1e-4,
                    "active ring must have settled opacity 1.0, got {} at level {idx}",
                    w.opacity
                );
            }
            if idx == 0 {
                assert_eq!(w.kind, WheelVisualKind::ConcentricRoot);
                // Item 0 is open submenu (on_path = true), item 1 is closed submenu (on_path = false)
                assert!(w.segments[0].has_children && w.segments[0].on_path);
                assert!(w.segments[1].has_children && !w.segments[1].on_path);
            } else {
                assert_eq!(w.kind, WheelVisualKind::ConcentricRing);
                assert!(
                    w.hub.badge.is_none(),
                    "outer rings must not carry a hub badge"
                );
            }
        }

        renderer.render(sc);
    }

    // Hover a leaf on level 6 (depth index 5, item 2) and verify caption breadcrumb has 5 ancestors
    let leaf_path = [0u8, 0, 0, 0, 0, 2];
    let p_leaf = session
        .item_center(&ItemPath::from_slice(&leaf_path))
        .unwrap();
    session.handle(WheelInput::PointerMoved(p_leaf), time_ms + 20, &mut events);
    session.handle(
        WheelInput::PointerMoved(Point::new(p_leaf.x + 3.0, p_leaf.y)),
        time_ms + 25,
        &mut events,
    );
    let sc6 = session.scene();
    let cap = sc6
        .caption
        .as_ref()
        .expect("caption should be present on hovered leaf");
    assert_eq!(
        (cap.box_.w, cap.box_.h),
        (280.0, 64.0),
        "Concentric caption box must be fixed 280x64 lp"
    );
    let crumbs = cap
        .breadcrumb
        .as_ref()
        .expect("concentric caption must populate breadcrumb");
    assert_eq!(crumbs.len(), 5);
    assert_eq!(crumbs[0].as_ref(), "Level 0 Item 0");
    assert_eq!(crumbs[4].as_ref(), "Level 4 Item 0");
    renderer.render(sc6);

    // Verify the drawn Concentric caption pill covers the fixed 280x64 lp box (testing left/right/top/bottom inside edges)
    let left_edge_x = (cap.box_.x + 8.0).round() as u32;
    let right_edge_x = (cap.box_.x + cap.box_.w - 8.0).round() as u32;
    let mid_y = (cap.box_.y + cap.box_.h * 0.5).round() as u32;
    let top_edge_y = (cap.box_.y + 6.0).round() as u32;
    let bot_edge_y = (cap.box_.y + cap.box_.h - 6.0).round() as u32;
    let mid_x = (cap.box_.x + cap.box_.w * 0.5).round() as u32;
    for &(px, py) in &[
        (left_edge_x, mid_y),
        (right_edge_x, mid_y),
        (mid_x, top_edge_y),
        (mid_x, bot_edge_y),
    ] {
        let idx = ((py * 1920 + px) * 4) as usize;
        assert!(
            renderer.frame().data[idx + 3] > 180,
            "fixed 280x64 caption pill must cover ({px}, {py}), got alpha={}",
            renderer.frame().data[idx + 3]
        );
    }

    // Verify ancestor sector pixel is visible and not covered by any opaque child hub
    let px_idx =
        ((root_item2_pt.y.round() as u32 * 1920 + root_item2_pt.x.round() as u32) * 4) as usize;
    let alpha = renderer.frame().data[px_idx + 3];
    assert!(
        alpha > 60,
        "ancestor ring pixel at {root_item2_pt:?} should remain visible, got alpha={alpha}"
    );
}

#[test]
fn t_ren2_deterministic_animation_checkpoints_themes_scales_counts_labels_icons() {
    let cfg = WheelConfig {
        layout: WheelLayout::Concentric,
        ..WheelConfig::default()
    };
    let mut events = Vec::new();

    // 1. Animation checkpoints t = 0, 30, 60, 130, 250 ms & interruption
    let vp = Viewport::new(1280.0, 800.0, 1.0);
    let mut session = WheelSession::new(
        Some(build_deep_menu(3, 6)),
        Point::new(640.0, 400.0),
        vp,
        InputMode::Drag,
        &cfg,
        0,
    );
    let mut renderer = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
    renderer.begin_session(SizePx::new(1280, 800), 1.0);

    for &t in &[0u64, 30, 60, 130, 250] {
        session.tick(t, &mut events);
        let sc = session.scene();
        assert_eq!(sc.wheels[0].scale, 1.0);
        assert!(sc.wheels[0].segments.iter().all(|s| s.pop == 0.0));
        if t == 0 {
            assert!(
                (sc.wheels[0].opacity - 0.35).abs() < 1e-4,
                "ring expansion must start at opacity 0.35, got {}",
                sc.wheels[0].opacity
            );
        } else if t == 30 || t == 60 {
            let u = (t as f32) / 130.0;
            let expected = 0.35 + (1.0 - 0.35) * (1.0 - (1.0 - u).powi(3));
            assert!(
                (sc.wheels[0].opacity - expected).abs() < 1e-3,
                "ring expansion at t={t}ms must follow 130ms OutCubic ({expected}), got {}",
                sc.wheels[0].opacity
            );
        } else if t >= 130 {
            assert!(
                (sc.wheels[0].opacity - 1.0).abs() < 1e-4,
                "ring expansion must reach 1.0 at 130ms, got {}",
                sc.wheels[0].opacity
            );
        }
        renderer.render(sc);
    }

    // Verify I8 hover in/out (90/120 ms), caption transition (80 ms), and ancestor dim (120 ms)
    let p_item2 = session.item_center(&ItemPath::from_slice(&[2])).unwrap();
    session.handle(WheelInput::PointerMoved(p_item2), 300, &mut events);
    session.tick(345, &mut events); // 45 ms into 90 ms hover-in and 80 ms caption fade
    assert!(
        session.scene().wheels[0].segments[2].highlight > 0.5
            && session.scene().wheels[0].segments[2].highlight < 1.0,
        "hover-in must still be interpolating at 45 ms (< 90 ms)"
    );
    assert!(
        (session.scene().caption.as_ref().unwrap().opacity - (45.0 / 80.0)).abs() < 1e-3,
        "caption fade must follow 80 ms duration"
    );
    session.tick(380, &mut events); // 80 ms: caption settled at 1.0, hover-in still < 90 ms
    assert!((session.scene().caption.as_ref().unwrap().opacity - 1.0).abs() < 1e-4);
    assert!(session.scene().wheels[0].segments[2].highlight < 1.0);
    session.tick(390, &mut events); // 90 ms: hover-in settled at 1.0
    assert!((session.scene().wheels[0].segments[2].highlight - 1.0).abs() < 1e-4);

    // Move to submenu [0]: item [2] hovers out over 120 ms; submenu [0] opens at 400 + 140 = 540 ms,
    // dimming ancestor root from 1.0 -> 0.72 over 120 ms.
    let p_item0 = session.item_center(&ItemPath::from_slice(&[0])).unwrap();
    session.handle(WheelInput::PointerMoved(p_item0), 400, &mut events);
    session.tick(460, &mut events); // 60 ms into 120 ms hover-out of item [2]
    assert!(
        session.scene().wheels[0].segments[2].highlight > 0.05
            && session.scene().wheels[0].segments[2].highlight < 0.5
    );
    session.tick(520, &mut events); // 120 ms: item [2] hover-out settled at 0.0
    assert!(session.scene().wheels[0].segments[2].highlight.abs() < 1e-4);
    session.tick(540, &mut events); // submenu [0] opens at 540 ms; ancestor dim (120 ms) starts
    session.tick(600, &mut events); // 60 ms into 120 ms ancestor dim
    assert!(
        session.scene().wheels[0].opacity > 0.72 && session.scene().wheels[0].opacity < 1.0,
        "ancestor dim must still be interpolating at 60 ms (< 120 ms)"
    );
    session.tick(660, &mut events); // 120 ms: ancestor dim settled at 0.72
    assert!((session.scene().wheels[0].opacity - 0.72).abs() < 1e-4);
    session.tick(700, &mut events); // child ring emergence settled at 1.0
    assert!((session.scene().wheels[1].opacity - 1.0).abs() < 1e-4);

    // 2. Dark and Light themes across scales 1.0, 1.25, 1.5, 2.0
    for theme in [Theme::dark(), Theme::light()] {
        for &scale in &[1.0f32, 1.25, 1.5, 2.0] {
            let pw = (1280.0 * scale).round() as u32;
            let ph = (800.0 * scale).round() as u32;
            let mut r = Renderer::new(theme.clone(), AssetSource::Embedded).unwrap();
            r.begin_session(SizePx::new(pw, ph), scale);
            let dmg = r.render(session.scene());
            assert!(!dmg.is_empty());
        }
    }

    // 3. Menu item counts n = 1, 2, 3, 8, 12 + long Latin/Arabic + missing/unknown icons
    for &count in &[1usize, 2, 3, 8, 12] {
        let cfg_count = WheelConfig {
            layout: WheelLayout::Concentric,
            max_items_per_wheel: count as u8,
            radius_root: if count > 8 {
                [56.0, 144.0]
            } else {
                [40.0, 128.0]
            },
            ..WheelConfig::default()
        };
        let items: Vec<MenuNode> = (0..count)
            .map(|i| {
                let mut node = match i % 4 {
                    0 => MenuNode::leaf(
                        format!("item.{i}"),
                        "Export High-Resolution Lossless Sequence Archive",
                    )
                    .with_icon("lucide:eye"),
                    1 => MenuNode::leaf(
                        format!("item.{i}"),
                        "تحويل الصور المحددة إلى صيغة عالية الجودة بدون فقدان",
                    )
                    .with_icon("lucide:nonexistent-icon-name"),
                    2 => MenuNode::leaf(format!("item.{i}"), "Label Only No Icon"),
                    _ => MenuNode::leaf(format!("item.{i}"), format!("Action {i}"))
                        .with_icon("lucide:copy"),
                };
                if i == 0 {
                    node = node.with_description(
                        "Very long detailed description that wraps across two lines inside the 280x64 caption box without overflowing",
                    );
                }
                node
            })
            .collect();

        let mut s = WheelSession::new(
            Some(MenuTree::new(items)),
            Point::new(640.0, 400.0),
            vp,
            InputMode::Drag,
            &cfg_count,
            0,
        );
        s.tick(250, &mut events);
        let p0 = s.item_center(&ItemPath::from_slice(&[0])).unwrap();
        s.handle(WheelInput::PointerMoved(p0), 260, &mut events);

        let sc = s.scene();
        let w = &sc.wheels[0];
        let nc = w
            .nested_content
            .as_ref()
            .expect("concentric ring must populate nested_content");
        assert_eq!(w.segments.len(), count);
        assert_eq!(nc.len(), count);

        if count >= 3 {
            assert_eq!(
                nc[2].mode,
                ContentMode::LabelOnly,
                "item without icon must use ContentMode::LabelOnly"
            );
        }

        // Verify no pair of sector content boxes overlap
        for i in 0..nc.len() {
            for j in (i + 1)..nc.len() {
                let a = nc[i].rect;
                let b = nc[j].rect;
                let overlaps =
                    a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y;
                assert!(
                    !overlaps,
                    "content boxes {i} ({a:?}) and {j} ({b:?}) overlap for count={count}"
                );
            }
        }

        assert_incremental_matches_fresh(
            &mut renderer,
            sc,
            SizePx::new(1280, 800),
            1.0,
            Theme::dark(),
            &format!("count_{count}"),
        );

        // For n = 1, verify top of annulus (12 o'clock, angle -PI/2) has rendered ring material
        // with no separator gap seam.
        if count == 1 {
            let top_y = (400.0_f32 - 84.0_f32).round() as u32;
            let top_x = 640u32;
            let idx = ((top_y * 1280 + top_x) * 4) as usize;
            assert!(
                renderer.frame().data[idx + 3] > 100,
                "n=1 full annulus must have no separator gap at 12 o'clock"
            );
        }
    }
}

#[test]
fn t_ren3_t_ren4_t_ren6_full_vs_incremental_byte_identity_and_damage_invariants() {
    let cfg = WheelConfig {
        layout: WheelLayout::Concentric,
        ..WheelConfig::default()
    };
    let size = SizePx::new(1440, 900);
    let vp = Viewport::new(1440.0, 900.0, 1.0);
    let theme = Theme::dark();
    let mut events = Vec::new();

    let mut inc = Renderer::new(theme.clone(), AssetSource::Embedded).unwrap();
    inc.begin_session(size, 1.0);

    let mut session = WheelSession::new(
        Some(build_deep_menu(4, 6)),
        Point::new(720.0, 450.0),
        vp,
        InputMode::Drag,
        &cfg,
        0,
    );
    session.set_root_badge(Some(4), 0);

    // 1. Root appear animation steps
    for &t in &[0u64, 30, 60, 130, 250] {
        session.tick(t, &mut events);
        assert_incremental_matches_fresh(
            &mut inc,
            session.scene(),
            size,
            1.0,
            theme.clone(),
            &format!("root_appear_t{t}"),
        );
    }

    // 2. Hover root leaf item 2 -> Armed outline + caption
    let p_root2 = session.item_center(&ItemPath::from_slice(&[2])).unwrap();
    session.handle(WheelInput::PointerMoved(p_root2), 260, &mut events);
    assert_eq!(
        session.scene().wheels[0].segments[2].selection,
        SelectionVisual::Armed
    );
    assert_incremental_matches_fresh(
        &mut inc,
        session.scene(),
        size,
        1.0,
        theme.clone(),
        "hover_root_2",
    );

    // 3. Hover root submenu 0 and dwell to open ring 1
    let p_root0 = session.item_center(&ItemPath::from_slice(&[0])).unwrap();
    session.handle(WheelInput::PointerMoved(p_root0), 270, &mut events);
    assert_incremental_matches_fresh(
        &mut inc,
        session.scene(),
        size,
        1.0,
        theme.clone(),
        "hover_root_0",
    );

    for &t in &[380u64, 410, 460, 550] {
        session.tick(t, &mut events);
        assert_incremental_matches_fresh(
            &mut inc,
            session.scene(),
            size,
            1.0,
            theme.clone(),
            &format!("open_ring1_t{t}"),
        );
    }
    assert_eq!(session.scene().wheels.len(), 2);

    // 4. Move into outer neutral margin (r = 205 lp, beyond ring 1 r_out=194 + 3 lp hysteresis)
    // -> hovered_item cleared, settled path caption "Move inward to go back."
    session.handle(
        WheelInput::PointerMoved(Point::new(720.0, 450.0 - 205.0)),
        560,
        &mut events,
    );
    assert_eq!(
        session
            .scene()
            .caption
            .as_ref()
            .and_then(|c| c.detail.as_deref()),
        Some("Move inward to go back.")
    );
    assert_incremental_matches_fresh(
        &mut inc,
        session.scene(),
        size,
        1.0,
        theme.clone(),
        "gap_caption",
    );

    // 5. Move into ring 1 submenu 0 and open ring 2
    let p_r1_0 = session.item_center(&ItemPath::from_slice(&[0, 0])).unwrap();
    session.handle(WheelInput::PointerMoved(p_r1_0), 570, &mut events);
    session.handle(
        WheelInput::PointerMoved(Point::new(p_r1_0.x + 3.0, p_r1_0.y)),
        575,
        &mut events,
    );
    for &t in &[690u64, 730, 850] {
        session.tick(t, &mut events);
        assert_incremental_matches_fresh(
            &mut inc,
            session.scene(),
            size,
            1.0,
            theme.clone(),
            &format!("open_ring2_t{t}"),
        );
    }
    assert_eq!(session.scene().wheels.len(), 3);

    // 6. Hover ancestor sibling submenu [1] -> Pending visual (0.25 highlight, no outline)
    let p_root1 = session.item_center(&ItemPath::from_slice(&[1])).unwrap();
    session.handle(WheelInput::PointerMoved(p_root1), 860, &mut events);
    assert_eq!(
        session.scene().wheels[0].segments[1].selection,
        SelectionVisual::Pending
    );
    assert_incremental_matches_fresh(
        &mut inc,
        session.scene(),
        size,
        1.0,
        theme.clone(),
        "ancestor_pending_root1",
    );

    // 7. Advance past 180 ms sibling switch grace + dwell -> old branch retracts, new branch opens
    for &t in &[1040u64, 1080, 1150, 1280] {
        session.tick(t, &mut events);
        assert_incremental_matches_fresh(
            &mut inc,
            session.scene(),
            size,
            1.0,
            theme.clone(),
            &format!("sibling_switch_t{t}"),
        );
    }

    // 8. Disable an item and verify disabled appearance + reason caption
    session.set_enabled(
        &ItemPath::from_slice(&[1, 2]),
        false,
        Some(Arc::from("Action unavailable for read-only archive")),
    );
    let p_r1_2 = session.item_center(&ItemPath::from_slice(&[1, 2])).unwrap();
    session.handle(WheelInput::PointerMoved(p_r1_2), 1300, &mut events);
    session.handle(
        WheelInput::PointerMoved(Point::new(p_r1_2.x + 3.0, p_r1_2.y)),
        1305,
        &mut events,
    );
    assert_incremental_matches_fresh(
        &mut inc,
        session.scene(),
        size,
        1.0,
        theme.clone(),
        "disabled_item_caption",
    );

    // 9. Enter hub and dwell to collapse back to root (caption removed, outer rings fade and erase)
    session.handle(
        WheelInput::PointerMoved(Point::new(720.0, 450.0)),
        1320,
        &mut events,
    );
    for &t in &[1470u64, 1510, 1620] {
        session.tick(t, &mut events);
        assert_incremental_matches_fresh(
            &mut inc,
            session.scene(),
            size,
            1.0,
            theme.clone(),
            &format!("hub_collapse_t{t}"),
        );
    }
    assert_eq!(session.scene().wheels.len(), 1);
    assert!(session.scene().caption.is_none());

    // 10. Displaced origin near top-left corner -> origin_guide rendered and tracked in damage
    let mut corner_session = WheelSession::new(
        Some(build_deep_menu(2, 4)),
        Point::new(32.0, 32.0),
        vp,
        InputMode::Pointer,
        &cfg,
        2000,
    );
    // At t = 2000 (root opacity = 0.35), origin_guide opacity is 0.45 * 0.35
    let guide_t0 = corner_session
        .scene()
        .origin_guide
        .expect("origin_guide present when distance(anchor, C) > 0.5");
    assert!((guide_t0.opacity - 0.45 * 0.35).abs() < 1e-4);
    corner_session.tick(2250, &mut events);
    let guide_settled = corner_session.scene().origin_guide.unwrap();
    assert!(
        (guide_settled.opacity - 0.45).abs() < 1e-4,
        "settled origin_guide opacity must be 0.45 * 1.0 = 0.45, got {}",
        guide_settled.opacity
    );
    assert_incremental_matches_fresh(
        &mut inc,
        corner_session.scene(),
        size,
        1.0,
        theme.clone(),
        "displaced_origin_invoke",
    );

    // Keyboard navigation on Pointer/Invoke session -> KeyboardFocus selection visual
    corner_session.handle(WheelInput::Nav(NavKey::Next), 2260, &mut events);
    assert_eq!(
        corner_session.scene().wheels[0].segments[0].selection,
        SelectionVisual::KeyboardFocus
    );
    assert_incremental_matches_fresh(
        &mut inc,
        corner_session.scene(),
        size,
        1.0,
        theme,
        "keyboard_focus_root0",
    );
}

#[test]
fn t_nav10_t_nav11_t_nav12_rendering_invariants() {
    let mut events = Vec::new();
    let size = SizePx::new(1280, 800);
    let vp = Viewport::new(1280.0, 800.0, 1.0);
    let cfg = WheelConfig {
        layout: WheelLayout::Concentric,
        ..WheelConfig::default()
    };

    let mut inc = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
    inc.begin_session(size, 1.0);

    // T-NAV10: Rapid alternating sibling movements at 1/8/16 ms intervals + 6-level sibling replacement
    let mut session = WheelSession::new(
        Some(build_deep_menu(3, 4)),
        Point::new(640.0, 400.0),
        vp,
        InputMode::Drag,
        &cfg,
        0,
    );
    session.tick(250, &mut events);
    let p0 = session.item_center(&ItemPath::from_slice(&[0])).unwrap();
    let p1 = session.item_center(&ItemPath::from_slice(&[1])).unwrap();
    let mut t = 260u64;
    for step in 0..120usize {
        let dt = match step % 3 {
            0 => 1,
            1 => 8,
            _ => 16,
        };
        t += dt;
        let pt = if step % 2 == 0 { p0 } else { p1 };
        session.handle(WheelInput::PointerMoved(pt), t, &mut events);
        session.tick(t, &mut events);
        let sc = session.scene();
        assert!(
            sc.wheels.len() <= 6,
            "scene.wheels.len() must never exceed 6 visible ring records (got {})",
            sc.wheels.len()
        );
        // Verify no two visible rings ever occupy the same radial slot `(r_in, r_out)`
        for i in 0..sc.wheels.len() {
            for j in (i + 1)..sc.wheels.len() {
                let same_slot = (sc.wheels[i].r_in - sc.wheels[j].r_in).abs() < 0.5
                    && (sc.wheels[i].r_out - sc.wheels[j].r_out).abs() < 0.5;
                assert!(
                    !same_slot,
                    "two ring records painted over the same radial slot: {:?} and {:?}",
                    (sc.wheels[i].r_in, sc.wheels[i].r_out),
                    (sc.wheels[j].r_in, sc.wheels[j].r_out)
                );
            }
        }
    }
    // Open all 6 levels in a clean 1080p Concentric session, then switch root sibling [0] -> [1] and verify:
    // - Replaced level-1 ring is removed before new level-1 ring fades in from 0.35 -> 1.0
    // - Outer retracting rings fade over 110 ms and total visible ring records <= 6
    let cfg6 = WheelConfig {
        layout: WheelLayout::Concentric,
        max_depth: 6,
        ..WheelConfig::default()
    };
    let mut session6 = WheelSession::new(
        Some(build_deep_menu(6, 4)),
        Point::new(960.0, 540.0),
        Viewport::new(1920.0, 1080.0, 1.0),
        InputMode::Drag,
        &cfg6,
        0,
    );
    session6.tick(250, &mut events);
    let p6_1 = session6.item_center(&ItemPath::from_slice(&[1])).unwrap();
    let mut t6 = 260u64;
    for depth_len in 1..=5 {
        let cur_path = vec![0u8; depth_len];
        let pt = session6
            .item_center(&ItemPath::from_slice(&cur_path))
            .unwrap();
        t6 += 20;
        session6.handle(WheelInput::PointerMoved(pt), t6, &mut events);
        session6.handle(
            WheelInput::PointerMoved(Point::new(pt.x + 3.0, pt.y)),
            t6 + 5,
            &mut events,
        );
        t6 += 300;
        session6.tick(t6, &mut events);
    }
    assert_eq!(session6.scene().wheels.len(), 6);
    // Hover root sibling [1] and advance 300 ms (`cfg.grace_ms = 300 >= cfg.dwell_ms = 140`)
    t6 += 10;
    session6.handle(WheelInput::PointerMoved(p6_1), t6, &mut events);
    let switch_open_t = t6 + 300;
    session6.tick(switch_open_t, &mut events);
    let sc_replace = session6.scene();
    assert_eq!(
        sc_replace.wheels.len(),
        6,
        "2 open rings + 4 outer retracting ghosts must equal 6 visible ring records"
    );
    for i in 0..sc_replace.wheels.len() {
        for j in (i + 1)..sc_replace.wheels.len() {
            let same_slot = (sc_replace.wheels[i].r_in - sc_replace.wheels[j].r_in).abs() < 0.5
                && (sc_replace.wheels[i].r_out - sc_replace.wheels[j].r_out).abs() < 0.5;
            assert!(
                !same_slot,
                "sibling replacement painted two action sets over the same radial slot at {i} and {j}"
            );
        }
    }
    // New level-1 ring starts at opacity 0.35 on the frame it opens; outer retracting rings finish after 110 ms
    assert!((sc_replace.wheels[1].opacity - 0.35).abs() < 1e-4);
    session6.tick(switch_open_t + 55, &mut events);
    assert!(
        session6.scene().wheels.len() > 2,
        "outer retracting rings still fading at 55ms (< 110ms)"
    );
    session6.tick(switch_open_t + 110, &mut events);
    assert_eq!(
        session6.scene().wheels.len(),
        2,
        "all retracting rings must complete their 110ms fade at +110ms"
    );

    // Settle on p1 and verify incremental frame matches fresh render with zero ghost pixels
    t += 500;
    session.tick(t, &mut events);
    assert_incremental_matches_fresh(
        &mut inc,
        session.scene(),
        size,
        1.0,
        Theme::dark(),
        "t_nav10_settled_after_rapid_alternation",
    );

    // T-NAV11: Interrupt open/close every 10 ms and verify fixed radii + continuous opacity
    let mut session11 = WheelSession::new(
        Some(build_deep_menu(3, 4)),
        Point::new(640.0, 400.0),
        vp,
        InputMode::Drag,
        &cfg,
        0,
    );
    for step_t in (0..=300).step_by(10) {
        session11.tick(step_t, &mut events);
        if step_t == 40 {
            session11.handle(WheelInput::PointerMoved(p0), step_t, &mut events);
        } else if step_t == 170 {
            // Enter hub mid-fade to trigger collapse
            session11.handle(
                WheelInput::PointerMoved(Point::new(640.0, 400.0)),
                step_t,
                &mut events,
            );
        }
        let sc = session11.scene();
        for w in &sc.wheels {
            assert_eq!(
                w.scale, 1.0,
                "Concentric ring scale must remain fixed at 1.0"
            );
            assert!(
                w.segments.iter().all(|s| s.pop == 0.0),
                "Concentric ring segment pop must remain fixed at 0.0"
            );
        }
        assert_incremental_matches_fresh(
            &mut inc,
            sc,
            size,
            1.0,
            Theme::dark(),
            &format!("t_nav11_step_{step_t}"),
        );
    }

    // T-NAV12: Reduced motion finishes within <= 60 ms
    let cfg_rm = WheelConfig {
        layout: WheelLayout::Concentric,
        reduced_motion: true,
        ..WheelConfig::default()
    };
    let mut session_rm = WheelSession::new(
        Some(build_deep_menu(2, 4)),
        Point::new(640.0, 400.0),
        vp,
        InputMode::Drag,
        &cfg_rm,
        0,
    );
    session_rm.tick(60, &mut events);
    let sc_rm = session_rm.scene();
    assert_eq!(
        sc_rm.wheels[0].opacity, 1.0,
        "reduced motion fade must complete within <= 60 ms"
    );
    assert_eq!(sc_rm.wheels[0].scale, 1.0);
    assert!(sc_rm.wheels[0].segments.iter().all(|s| s.pop == 0.0));
    assert_incremental_matches_fresh(
        &mut inc,
        sc_rm,
        size,
        1.0,
        Theme::dark(),
        "t_nav12_reduced_motion",
    );
}

#[test]
fn t_ren5_and_t_reg2_deterministic_golden_hashes_branching_and_concentric() {
    let size = SizePx::new(1280, 800);
    let vp = Viewport::new(1280.0, 800.0, 1.0);
    let mut events = Vec::new();
    let cfg_branch = WheelConfig {
        layout: WheelLayout::Branching,
        ..WheelConfig::default()
    };

    // --- Historical v0.2.0 (`e6a342e`) Branching golden baselines (5 fixtures) ---

    // Fixture B1: 1-level root hover + badge + caption (1280x800@1x, Dark)
    let mut sb1 = WheelSession::new(
        Some(build_deep_menu(3, 4)),
        Point::new(640.0, 400.0),
        vp,
        InputMode::Drag,
        &cfg_branch,
        0,
    );
    sb1.set_root_badge(Some(3), 0);
    sb1.tick(250, &mut events);
    let p_r2 = sb1.item_center(&ItemPath::from_slice(&[2])).unwrap();
    sb1.handle(WheelInput::PointerMoved(p_r2), 260, &mut events);
    sb1.tick(400, &mut events);
    let mut rb1 = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
    rb1.begin_session(size, 1.0);
    let dmg_b1 = rb1.render(sb1.scene()).to_vec();
    let hash_b1 = fnv1a_64(rb1.frame().data);
    assert_eq!(
        hash_b1, 0xe40f_0f49_14d0_bff2,
        "v0.2.0 Branching F1 (1-level dark 1x) hash mismatch: got {hash_b1:#018x}"
    );
    assert_eq!(dmg_b1, vec![RectPx::new(496, 256, 288, 320)]);

    // Fixture B2: 2-level child open + hovered submenu (1280x800@1x, Dark)
    let mut s_branch = WheelSession::new(
        Some(build_deep_menu(3, 4)),
        Point::new(640.0, 400.0),
        vp,
        InputMode::Drag,
        &cfg_branch,
        0,
    );
    s_branch.set_root_badge(Some(3), 0);
    s_branch.tick(250, &mut events);
    let p_b0 = s_branch.item_center(&ItemPath::from_slice(&[0])).unwrap();
    s_branch.handle(WheelInput::PointerMoved(p_b0), 260, &mut events);
    s_branch.tick(500, &mut events);

    let mut r_branch_1 = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
    r_branch_1.begin_session(size, 1.0);
    let dmg_b2 = r_branch_1.render(s_branch.scene()).to_vec();
    let hash_branch_1 = fnv1a_64(r_branch_1.frame().data);

    let mut r_branch_2 = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
    r_branch_2.begin_session(size, 1.0);
    r_branch_2.render(s_branch.scene());
    let hash_branch_2 = fnv1a_64(r_branch_2.frame().data);
    assert_eq!(hash_branch_1, hash_branch_2);
    assert_eq!(
        hash_branch_1, 0x778b_132f_51fb_61cd,
        "v0.2.0 Branching F2 (2-level dark 1x) hash mismatch: got {hash_branch_1:#018x}"
    );
    assert_eq!(dmg_b2, vec![RectPx::new(496, 22, 288, 527)]);

    // Fixture B3 & B4: 3-level deep scene + hovered child leaf + caption (Dark & Light @ 1x)
    let p_b00 = s_branch
        .item_center(&ItemPath::from_slice(&[0, 0]))
        .unwrap();
    s_branch.handle(WheelInput::PointerMoved(p_b00), 510, &mut events);
    s_branch.tick(800, &mut events);
    let p_b002 = s_branch
        .item_center(&ItemPath::from_slice(&[0, 0, 2]))
        .unwrap();
    s_branch.handle(WheelInput::PointerMoved(p_b002), 810, &mut events);
    s_branch.tick(950, &mut events);

    let mut rb3 = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
    rb3.begin_session(size, 1.0);
    let dmg_b3 = rb3.render(s_branch.scene()).to_vec();
    let hash_b3 = fnv1a_64(rb3.frame().data);
    assert_eq!(
        hash_b3, 0xba43_4238_7605_73e5,
        "v0.2.0 Branching F3 (3-level dark 1x) hash mismatch: got {hash_b3:#018x}"
    );
    assert_eq!(dmg_b3, vec![RectPx::new(119, 54, 665, 495)]);

    let mut rb4 = Renderer::new(Theme::light(), AssetSource::Embedded).unwrap();
    rb4.begin_session(size, 1.0);
    let dmg_b4 = rb4.render(s_branch.scene()).to_vec();
    let hash_b4 = fnv1a_64(rb4.frame().data);
    assert_eq!(
        hash_b4, 0xc4dc_293b_973d_93af,
        "v0.2.0 Branching F4 (3-level light 1x) hash mismatch: got {hash_b4:#018x}"
    );
    assert_eq!(dmg_b4, vec![RectPx::new(119, 54, 665, 495)]);

    // Fixture B5: 2-level scene at 2.0x HiDPI scale (2560x1600, Dark)
    let mut sb5 = WheelSession::new(
        Some(build_deep_menu(3, 4)),
        Point::new(640.0, 400.0),
        Viewport::new(1280.0, 800.0, 2.0),
        InputMode::Drag,
        &cfg_branch,
        0,
    );
    sb5.set_root_badge(Some(3), 0);
    sb5.tick(250, &mut events);
    let p5_0 = sb5.item_center(&ItemPath::from_slice(&[0])).unwrap();
    sb5.handle(WheelInput::PointerMoved(p5_0), 260, &mut events);
    sb5.tick(500, &mut events);
    let mut rb5 = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
    rb5.begin_session(SizePx::new(2560, 1600), 2.0);
    let dmg_b5 = rb5.render(sb5.scene()).to_vec();
    let hash_b5 = fnv1a_64(rb5.frame().data);
    assert_eq!(
        hash_b5, 0xd7c4_2c7e_c26b_64cd,
        "v0.2.0 Branching F5 (2-level dark 2x) hash mismatch: got {hash_b5:#018x}"
    );
    assert_eq!(dmg_b5, vec![RectPx::new(992, 44, 576, 1054)]);

    // --- Concentric canonical scene (2 levels open + hovered child leaf + fixed 280x64 caption) ---
    let cfg_conc = WheelConfig {
        layout: WheelLayout::Concentric,
        ..WheelConfig::default()
    };
    let mut s_conc = WheelSession::new(
        Some(build_deep_menu(3, 4)),
        Point::new(640.0, 400.0),
        vp,
        InputMode::Drag,
        &cfg_conc,
        0,
    );
    s_conc.set_root_badge(Some(3), 0);
    s_conc.tick(250, &mut events);
    let p_c0 = s_conc.item_center(&ItemPath::from_slice(&[0])).unwrap();
    s_conc.handle(WheelInput::PointerMoved(p_c0), 260, &mut events);
    s_conc.tick(500, &mut events);
    let p_c02 = s_conc.item_center(&ItemPath::from_slice(&[0, 2])).unwrap();
    s_conc.handle(WheelInput::PointerMoved(p_c02), 510, &mut events);
    s_conc.handle(
        WheelInput::PointerMoved(Point::new(p_c02.x + 3.0, p_c02.y)),
        515,
        &mut events,
    );

    let mut r_conc_1 = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
    r_conc_1.begin_session(size, 1.0);
    r_conc_1.render(s_conc.scene());
    let hash_conc_1 = fnv1a_64(r_conc_1.frame().data);

    let mut r_conc_2 = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
    r_conc_2.begin_session(size, 1.0);
    r_conc_2.render(s_conc.scene());
    let hash_conc_2 = fnv1a_64(r_conc_2.frame().data);
    assert_eq!(hash_conc_1, hash_conc_2);
    assert_ne!(hash_conc_1, hash_branch_1);
    println!("GOLDEN_HASHES: branch={hash_branch_1:#018x}, conc={hash_conc_1:#018x}");
    assert_eq!(
        hash_conc_1, 0x3aac_1a44_aec2_5ec0,
        "Concentric 2-level golden hash changed unexpectedly: got {hash_conc_1:#018x}"
    );
}
