//! Property-based tests (`proptest`) for `radivine-wheel` (`TESTING_STRATEGY.md` §3).

use proptest::prelude::*;
use radivine_wheel::concentric::{
    CONCENTRIC_EDGE_PAD, NestedHit, hit_concentric, plan_concentric, ring_geometry,
    target_core_fits_ring,
};
use radivine_wheel::geometry::{child_segment_spans, root_segment_spans};
use radivine_wheel::layout::{
    EDGE_MARGIN, MIN_ANCESTOR_SEP, nominal_radii, place_child, place_root,
};
use radivine_wheel::{
    DropTarget, InputMode, ItemPath, LayoutSelection, MenuNode, MenuTree, NavKey, Point, Rect,
    Viewport, WheelConfig, WheelEvent, WheelHitGeometry, WheelInput, WheelLayout, WheelSession,
    Zone, hit_test,
};
use smallvec::SmallVec;
use std::sync::Arc;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10_000))]

    /// Property 1 (AC-1a.2): For 10,000 random `(n, r_in, r_out, p_in_annulus)`, `hit_test` returns
    /// exactly one of `{Item, Bridge, Empty}` (never `Hub`, `Corridor`, or `Outside`).
    #[test]
    fn prop_annulus_partition(
        n in 1usize..=12,
        is_child in any::<bool>(),
        r_in in 20.0f32..50.0,
        ring_w in 30.0f32..90.0,
        beta in 0.0f32..std::f32::consts::TAU,
        theta in 0.0f32..std::f32::consts::TAU,
        t_rad in 0.05f32..0.95,
    ) {
        let r_out = r_in + ring_w;
        let center = Point::new(500.0, 500.0);
        let (bridge, spans) = if is_child {
            let (b, s) = child_segment_spans(n, beta, 70.0_f32.to_radians());
            (Some(b), s)
        } else {
            (None, root_segment_spans(n))
        };

        let geom = WheelHitGeometry {
            index: u8::from(is_child),
            parent_index: None,
            center,
            r_in,
            r_out,
            hit_radius: r_out + 12.0,
            bridge,
            spans,
            corridor: None,
        };

        let r = r_in + t_rad * (r_out - r_in);
        let p = center.offset_polar(r, theta);
        let z = hit_test(p, &[geom], Zone::Outside, 4.0_f32.to_radians());

        let is_valid_annulus_zone = matches!(
            z,
            Zone::Item { .. } | Zone::Bridge { .. } | Zone::Empty { .. }
        );
        prop_assert!(is_valid_annulus_zone, "unexpected zone {z:?} in annulus");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5_000))]

    /// Property 2 (AC-1a.3): For 5,000 random anchors in a viewport >= 800x600 and random menus
    /// (<= 8 items, depth <= 3): every placed wheel is inside the viewport or flagged as `fallback`;
    /// no two non-parent wheels overlap unless flagged as `fallback`.
    #[test]
    fn prop_placement_within_viewport_and_non_overlapping(
        vp_w in 800.0f32..2560.0,
        vp_h in 600.0f32..1440.0,
        anchor_x in 0.0f32..2560.0,
        anchor_y in 0.0f32..1440.0,
        n_root in 2usize..=8,
        n_child in 2usize..=8,
        idx_root in 0usize..8,
        idx_child in 0usize..8,
    ) {
        let cfg = WheelConfig::default();
        let vp = Viewport::new(vp_w, vp_h, 1.0);
        let anchor = Point::new(anchor_x.min(vp_w), anchor_y.min(vp_h));

        let (_, r_root) = nominal_radii(0, &cfg);
        let c0 = place_root(anchor, r_root, vp);
        prop_assert!(vp.bounds.contains_disc(c0, r_root + EDGE_MARGIN - 1e-2));

        let root_spans = root_segment_spans(n_root);
        let phi0 = root_spans[idx_root % n_root].center;
        let (_, r_child_nom) = nominal_radii(1, &cfg);
        let p1 = place_child(c0, r_root, phi0, r_child_nom, &[], vp, &cfg);
        if !p1.fallback {
            prop_assert!(vp.bounds.contains_disc(p1.center, p1.r_out + EDGE_MARGIN - 1e-2));
        }

        let beta1 = p1.center.angle_to(c0);
        let (_, child_spans) = child_segment_spans(n_child, beta1, cfg.bridge_angle_deg.to_radians());
        let phi1 = child_spans[idx_child % n_child].center;
        let (_, r_deep_nom) = nominal_radii(2, &cfg);
        let p2 = place_child(p1.center, p1.r_out, phi1, r_deep_nom, &[(c0, r_root)], vp, &cfg);
        if !p2.fallback {
            prop_assert!(vp.bounds.contains_disc(p2.center, p2.r_out + EDGE_MARGIN - 1e-2));
            let dist_0_2 = c0.distance(p2.center);
            prop_assert!(
                dist_0_2 >= r_root + p2.r_out + MIN_ANCESTOR_SEP - 1e-2,
                "grandchild overlapped root without fallback: dist={dist_0_2}"
            );
        }
    }

    /// Property 3 (AC-1a.4): For 5,000 random input sequences of length <= 50, never panics,
    /// never produces `Commit` for disabled items, and `next_deadline() >= now`.
    #[test]
    fn prop_random_input_sequences_invariants(
        steps in proptest::collection::vec((0u8..8, 0.0f32..1920.0, 0.0f32..1080.0, 1u64..80), 1..=50),
        pointer_mode in any::<bool>(),
    ) {
        let menu = MenuTree::new(vec![
            MenuNode::submenu(
                "sub.a",
                "Sub A",
                vec![
                    MenuNode::leaf("a.1", "A1"),
                    MenuNode::leaf("a.2", "A2").with_disabled("Disabled leaf A2"),
                ],
            ),
            MenuNode::leaf("leaf.enabled", "Enabled Leaf"),
            MenuNode::leaf("leaf.disabled", "Disabled Leaf").with_disabled("Disabled root leaf"),
            MenuNode::submenu(
                "sub.disabled",
                "Disabled Sub",
                vec![MenuNode::leaf("d.1", "D1")],
            )
            .with_disabled("Disabled submenu"),
        ]);

        let disabled_paths = [
            ItemPath::from_slice(&[0, 1]),
            ItemPath::from_slice(&[2]),
            ItemPath::from_slice(&[3]),
            ItemPath::from_slice(&[3, 0]),
        ];

        let cfg = WheelConfig::default();
        let vp = Viewport::new(1920.0, 1080.0, 1.0);
        let mode = if pointer_mode { InputMode::Pointer } else { InputMode::Drag };
        let mut session = WheelSession::new(Some(menu), Point::new(500.0, 400.0), vp, mode, &cfg, 0);
        let mut now = 0u64;
        let mut events = Vec::new();

        for (kind, x, y, dt) in steps {
            now += dt;
            events.clear();
            // Advance any due deadlines up to `now` before checking `next_deadline() >= now`
            session.tick(now, &mut events);

            let input = match kind {
                0..=2 => WheelInput::PointerMoved(Point::new(x, y)),
                3 => WheelInput::Press,
                4 => WheelInput::Release,
                5 => WheelInput::Nav(NavKey::Next),
                6 => WheelInput::Nav(NavKey::Activate),
                _ => WheelInput::Nav(NavKey::Back),
            };
            session.handle(input, now, &mut events);

            if let Some(deadline) = session.next_deadline() {
                prop_assert!(
                    deadline >= now,
                    "next_deadline {deadline} < now {now}"
                );
            }

            for ev in &events {
                if let WheelEvent::Commit(path) = ev {
                    prop_assert!(
                        !disabled_paths.contains(path),
                        "emitted Commit for disabled path {path:?}"
                    );
                }
            }
        }
        let _: SmallVec<[u8; 6]> = SmallVec::new();
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5_000))]

    /// Concentric Property 1: For random ring item counts `1..=12`, depth `0..4`, and points inside
    /// `(r_in + 1e-3, r_out - 1e-3)`, `hit_concentric` partitions the annulus strictly into
    /// `NestedHit::Item` or `NestedHit::Separator` (and for `n == 1`, always `NestedHit::Item`).
    #[test]
    fn prop_concentric_annulus_partition(
        n in 1usize..=12,
        level in 0u8..4,
        theta in 0.0f32..std::f32::consts::TAU,
        t_rad in 0.02f32..0.98,
    ) {
        let cfg = WheelConfig {
            layout: WheelLayout::Concentric,
            radius_root: [40.0, 128.0],
            max_depth: 4,
            ..WheelConfig::default()
        };
        let vp = Viewport::new(1920.0, 1080.0, 1.0);
        let plan = plan_concentric(Point::new(960.0, 540.0), vp, &cfg).expect("plan_concentric");

        let mut open = SmallVec::<[_; 6]>::new();
        for l in 0..=level {
            open.push(ring_geometry(&plan, l, if l == level { n } else { 4 }));
        }
        let rg = &open[usize::from(level)];
        let r = rg.r_in + t_rad * (rg.r_out - rg.r_in);
        let p = plan.center.offset_polar(r, theta);
        let hit = hit_concentric(p, &plan, &open);

        if n == 1 {
            prop_assert_eq!(hit, NestedHit::Item { level, index: 0 });
        } else {
            let valid = matches!(
                hit,
                NestedHit::Item { level: l, index } if l == level && usize::from(index) < n
            ) || hit == NestedHit::Separator { level };
            prop_assert!(valid, "unexpected concentric hit {hit:?} in ring {level} (n={n})");
        }
    }

    /// Concentric Property 2: Whenever `plan_concentric` succeeds for random viewports, anchors,
    /// root radii, item counts, and depths `1..=6`, all reserved rings and the caption box fit
    /// within the viewport with `8.0 lp` clearance, and every ring passes the `32x32` target core check.
    #[test]
    fn prop_concentric_preflight_fit_and_fallback_invariants(
        vp_x in -1920.0f32..1920.0,
        vp_y in -1080.0f32..1080.0,
        vp_w in 240.0f32..3840.0,
        vp_h in 240.0f32..2160.0,
        anchor_dx in -200.0f32..4000.0,
        anchor_dy in -200.0f32..2400.0,
        r_in in 20.0f32..80.0,
        root_thickness in 40.0f32..140.0,
        max_items in 2u8..=12,
        max_depth in 1u8..=6,
    ) {
        let mut cfg = WheelConfig {
            layout: WheelLayout::Concentric,
            radius_root: [r_in, r_in + root_thickness],
            max_items_per_wheel: max_items,
            max_depth,
            ..WheelConfig::default()
        };
        cfg.validate();
        let vp = Viewport {
            bounds: Rect::new(vp_x, vp_y, vp_w, vp_h),
            scale: 1.0,
        };
        let anchor = Point::new(vp_x + anchor_dx, vp_y + anchor_dy);

        if let Ok(plan) = plan_concentric(anchor, vp, &cfg) {
            prop_assert!(plan.center.x.is_finite() && plan.center.y.is_finite());
            prop_assert_eq!(plan.radii.len(), usize::from(cfg.max_depth));
            prop_assert!(
                vp.bounds.contains_disc(plan.center, plan.reserved_radius + CONCENTRIC_EDGE_PAD - 1e-2),
                "reserved disc exceeds viewport bounds: center={:?}, R={}",
                plan.center,
                plan.reserved_radius
            );
            prop_assert!(
                plan.caption_box.x >= vp.bounds.x + CONCENTRIC_EDGE_PAD - 1e-2
                    && plan.caption_box.right() <= vp.bounds.right() - CONCENTRIC_EDGE_PAD + 1e-2
                    && plan.caption_box.bottom() <= vp.bounds.bottom() - CONCENTRIC_EDGE_PAD + 1e-2,
                "caption_box {:?} exceeds viewport {:?}",
                plan.caption_box,
                vp.bounds
            );
            for &(ri, ro) in &plan.radii {
                prop_assert!(target_core_fits_ring(ri, ro, usize::from(cfg.max_items_per_wheel)));
            }
        }
    }

    /// Concentric Property 3: For 5,000 random monotonic event streams (with dynamic leaf and
    /// ancestor enablement changes), assert bounded storage, valid path prefixes, no commits to
    /// disabled nodes/ancestors, no commits from timers, targets inside visible material, no
    /// unexpected fallback after map, finite geometry, and future/absent deadlines.
    #[test]
    fn prop_concentric_random_input_and_enablement_stream_invariants(
        steps in proptest::collection::vec((0u8..12, 600.0f32..1320.0, 180.0f32..900.0, 1u64..90), 1..=60),
        pointer_mode in any::<bool>(),
        displaced_anchor in any::<bool>(),
    ) {
        let menu = MenuTree::new(vec![
            MenuNode::submenu(
                "sub.a",
                "Sub A",
                vec![
                    MenuNode::leaf("a.0", "A0"),
                    MenuNode::leaf("a.1", "A1").with_disabled("Initially disabled A1"),
                    MenuNode::submenu(
                        "sub.a2",
                        "Sub A2",
                        vec![
                            MenuNode::leaf("a.2.0", "A2-0"),
                            MenuNode::leaf("a.2.1", "A2-1"),
                        ],
                    ),
                ],
            ),
            MenuNode::leaf("leaf.b", "Leaf B"),
            MenuNode::leaf("leaf.c", "Leaf C").with_disabled("Always disabled C"),
            MenuNode::submenu(
                "sub.d",
                "Disabled Sub D",
                vec![MenuNode::leaf("d.0", "D0")],
            )
            .with_disabled("Disabled Sub D"),
        ]);

        let cfg = WheelConfig {
            layout: WheelLayout::Concentric,
            radius_root: [40.0, 128.0],
            dwell_ms: 110,
            grace_ms: 180,
            hub_close_ms: 150,
            ..WheelConfig::default()
        };
        let vp = Viewport::new(1920.0, 1080.0, 1.0);
        let anchor = if displaced_anchor {
            Point::new(20.0, 20.0)
        } else {
            Point::new(960.0, 540.0)
        };
        let mode = if pointer_mode { InputMode::Pointer } else { InputMode::Drag };
        let mut session = WheelSession::new(Some(menu.clone()), anchor, vp, mode, &cfg, 0);
        prop_assert_eq!(session.layout_selection(), LayoutSelection::concentric());

        let mut a0_enabled = true;
        let mut sub_a_enabled = true;
        let mut now = 0u64;
        let mut events = Vec::new();

        for (kind, x, y, dt) in steps {
            now += dt;

            // 1. Advance timers and verify timers NEVER emit Commit
            events.clear();
            session.tick(now, &mut events);
            for ev in &events {
                prop_assert!(
                    !matches!(ev, WheelEvent::Commit(_)),
                    "tick() emitted Commit: {ev:?}"
                );
            }

            // 2. Apply input or dynamic enablement change
            events.clear();
            match kind {
                0..=4 => session.handle(WheelInput::PointerMoved(Point::new(x, y)), now, &mut events),
                5 => session.handle(WheelInput::Press, now, &mut events),
                6 => session.handle(WheelInput::Release, now, &mut events),
                7 => session.handle(WheelInput::Nav(NavKey::Next), now, &mut events),
                8 => session.handle(WheelInput::Nav(NavKey::Activate), now, &mut events),
                9 => session.handle(WheelInput::Nav(NavKey::Back), now, &mut events),
                10 => {
                    a0_enabled = !a0_enabled;
                    session.set_enabled_at(
                        &ItemPath::from_slice(&[0, 0]),
                        a0_enabled,
                        if a0_enabled { None } else { Some(Arc::from("Toggled off A0")) },
                        now,
                    );
                }
                _ => {
                    sub_a_enabled = !sub_a_enabled;
                    session.set_enabled_at(
                        &ItemPath::from_slice(&[0]),
                        sub_a_enabled,
                        if sub_a_enabled { None } else { Some(Arc::from("Toggled off Sub A")) },
                        now,
                    );
                }
            }

            // 3. Layout never falls back after map
            prop_assert_eq!(session.layout_selection(), LayoutSelection::concentric());

            // 4. Bounded storage & valid open_chain prefixes
            let dbg = session.debug_state();
            prop_assert!(dbg.open_chain.len() <= 6);
            prop_assert_eq!(&dbg.open_chain[0], &ItemPath::root());
            for (lvl, p) in dbg.open_chain.iter().enumerate() {
                prop_assert_eq!(p.len(), lvl);
                prop_assert!(menu.node(p).is_some());
            }

            // 5. Finite scene geometry & bounded visible wheels (<= 6)
            let scene = session.scene();
            prop_assert!(scene.wheels.len() <= 6);
            prop_assert!(scene.stems.is_empty());
            for w in &scene.wheels {
                prop_assert!(w.center.x.is_finite() && w.center.y.is_finite());
                prop_assert!(w.r_in.is_finite() && w.r_out.is_finite() && w.r_out > w.r_in);
                prop_assert_eq!(w.scale, 1.0);
                prop_assert!(w.bridge.is_none());
                for seg in &w.segments {
                    prop_assert_eq!(seg.pop, 0.0);
                }
            }

            // 6. Future or absent deadlines
            if let Some(dl) = session.next_deadline() {
                prop_assert!(dl >= now, "next_deadline {dl} < now {now}");
            }

            // 7. Helper to check if a path is currently enabled including all ancestors
            let is_path_enabled = |p: &ItemPath| -> bool {
                let s = p.as_slice();
                if s.is_empty() {
                    return false;
                }
                if s[0] == 2 || s[0] == 3 {
                    return false;
                }
                if s[0] == 0 && !sub_a_enabled {
                    return false;
                }
                if s == [0, 1] {
                    return false;
                }
                if s == [0, 0] && !a0_enabled {
                    return false;
                }
                true
            };

            // 8. DropTarget::Accept must lie strictly inside an open ring sector and be enabled
            if let DropTarget::Accept(ref path) = session.drop_target() {
                prop_assert!(
                    is_path_enabled(path),
                    "DropTarget::Accept({path:?}) on disabled node/ancestor (a0={a0_enabled}, sub_a={sub_a_enabled})"
                );
                prop_assert!(
                    matches!(session.concentric_hit(), Some(NestedHit::Item { .. })),
                    "DropTarget::Accept({path:?}) outside strict NestedHit::Item: {:?}",
                    session.concentric_hit()
                );
            }

            // 9. No Commit ever targets a disabled node or disabled ancestor
            for ev in &events {
                if let WheelEvent::Commit(path) = ev {
                    prop_assert!(
                        is_path_enabled(path),
                        "emitted Commit({path:?}) for disabled node/ancestor (a0={a0_enabled}, sub_a={sub_a_enabled})"
                    );
                }
            }
        }
    }
}
