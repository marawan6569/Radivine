//! Property-based tests (`proptest`) for `radivine-wheel` (`TESTING_STRATEGY.md` §3).

use proptest::prelude::*;
use radivine_wheel::geometry::{child_segment_spans, root_segment_spans};
use radivine_wheel::layout::{
    EDGE_MARGIN, MIN_ANCESTOR_SEP, nominal_radii, place_child, place_root,
};
use radivine_wheel::{
    InputMode, ItemPath, MenuNode, MenuTree, NavKey, Point, Viewport, WheelConfig, WheelEvent,
    WheelHitGeometry, WheelInput, WheelSession, Zone, hit_test,
};
use smallvec::SmallVec;

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
