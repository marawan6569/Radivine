//! Criterion micro-benchmarks for `radivine-wheel` (`TESTING_STRATEGY.md` §7).

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use radivine_wheel::{
    InputMode, MenuNode, MenuTree, Point, Viewport, WheelConfig, WheelInput, WheelLayout,
    WheelSession,
};

fn build_demo_menu() -> MenuTree {
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
        .with_icon("lucide:eye"),
        MenuNode::leaf("radivine.copy", "Copy").with_icon("lucide:copy"),
        MenuNode::leaf("radivine.properties", "Properties").with_icon("lucide:info"),
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
        .with_icon("lucide:sparkles"),
    ])
}

fn bench_wheel_engine(c: &mut Criterion) {
    let cfg = WheelConfig::default();
    let vp = Viewport::new(1920.0, 1080.0, 1.0);

    c.bench_function("wheel_handle_pointer_move_3_levels", |b| {
        let mut session = WheelSession::new(
            Some(build_demo_menu()),
            Point::new(500.0, 500.0),
            vp,
            InputMode::Drag,
            &cfg,
            0,
        );
        let mut events = Vec::with_capacity(8);
        // Open child and grandchild
        session.handle(
            WheelInput::PointerMoved(Point::new(400.0, 500.0)),
            10,
            &mut events,
        );
        session.tick(200, &mut events);
        if let Some(p) = session.item_center(&radivine_wheel::ItemPath::from_slice(&[3, 1])) {
            session.handle(WheelInput::PointerMoved(p), 220, &mut events);
            session.tick(400, &mut events);
        }

        let mut t = 500u64;
        b.iter(|| {
            events.clear();
            t += 16;
            let pt = Point::new(450.0 + ((t % 32) as f32), 480.0);
            session.handle(black_box(WheelInput::PointerMoved(pt)), t, &mut events);
            black_box(session.scene());
        });
    });

    c.bench_function("wheel_placement_3_levels_corner", |b| {
        let menu = build_demo_menu();
        let mut events = Vec::with_capacity(8);
        b.iter(|| {
            events.clear();
            let mut session = WheelSession::new(
                Some(black_box(menu.clone())),
                Point::new(30.0, 30.0),
                vp,
                InputMode::Drag,
                &cfg,
                0,
            );
            if let Some(p0) = session.item_center(&radivine_wheel::ItemPath::from_slice(&[3])) {
                session.handle(WheelInput::PointerMoved(p0), 10, &mut events);
                session.tick(200, &mut events);
            }
            if let Some(p1) = session.item_center(&radivine_wheel::ItemPath::from_slice(&[3, 1])) {
                session.handle(WheelInput::PointerMoved(p1), 210, &mut events);
                session.tick(400, &mut events);
            }
            black_box(session.scene().generation);
        });
    });

    let conc_cfg = WheelConfig {
        layout: WheelLayout::Concentric,
        radius_root: [40.0, 128.0],
        dwell_ms: 110,
        grace_ms: 180,
        hub_close_ms: 150,
        ..WheelConfig::default()
    };

    c.bench_function("concentric_handle_pointer_move_4_levels", |b| {
        let deep_menu = MenuTree::new(vec![
            MenuNode::submenu(
                "l0",
                "Level 0",
                vec![MenuNode::submenu(
                    "l1",
                    "Level 1",
                    vec![MenuNode::submenu(
                        "l2",
                        "Level 2",
                        vec![
                            MenuNode::leaf("l3.0", "Leaf 3-0"),
                            MenuNode::leaf("l3.1", "Leaf 3-1"),
                        ],
                    )],
                )],
            ),
            MenuNode::leaf("root.1", "Root 1"),
        ]);
        let mut session = WheelSession::new(
            Some(deep_menu),
            Point::new(960.0, 540.0),
            vp,
            InputMode::Drag,
            &conc_cfg,
            0,
        );
        let mut events = Vec::with_capacity(8);
        let mut now = 10u64;
        for path in [&[0u8][..], &[0, 0][..], &[0, 0, 0][..]] {
            if let Some(p) = session.item_center(&radivine_wheel::ItemPath::from_slice(path)) {
                session.handle(WheelInput::PointerMoved(p), now, &mut events);
                now += 150;
                session.tick(now, &mut events);
            }
        }

        let mut t = 600u64;
        b.iter(|| {
            events.clear();
            t += 16;
            let pt = Point::new(960.0 + ((t % 32) as f32), 540.0 - 296.0);
            session.handle(black_box(WheelInput::PointerMoved(pt)), t, &mut events);
            session.tick(t, &mut events);
            black_box(session.scene());
        });
    });

    c.bench_function("concentric_placement_4_levels_corner", |b| {
        let menu = build_demo_menu();
        let mut events = Vec::with_capacity(8);
        b.iter(|| {
            events.clear();
            let mut session = WheelSession::new(
                Some(black_box(menu.clone())),
                Point::new(30.0, 30.0),
                vp,
                InputMode::Drag,
                &conc_cfg,
                0,
            );
            // Visit hub first to clear displaced-center approach guard
            let c = session.concentric_plan().unwrap().center;
            session.handle(WheelInput::PointerMoved(c), 5, &mut events);
            if let Some(p0) = session.item_center(&radivine_wheel::ItemPath::from_slice(&[3])) {
                session.handle(WheelInput::PointerMoved(p0), 10, &mut events);
                session.tick(200, &mut events);
            }
            if let Some(p1) = session.item_center(&radivine_wheel::ItemPath::from_slice(&[3, 1])) {
                session.handle(WheelInput::PointerMoved(p1), 210, &mut events);
                session.tick(400, &mut events);
            }
            black_box(session.scene().generation);
        });
    });
}

criterion_group!(benches, bench_wheel_engine);
criterion_main!(benches);
