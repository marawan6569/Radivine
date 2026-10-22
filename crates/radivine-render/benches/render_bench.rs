use criterion::{Criterion, black_box, criterion_group, criterion_main};
use radivine_render::{AssetSource, Renderer, SizePx, Theme};
use radivine_wheel::{
    InputMode, ItemPath, MenuNode, MenuTree, Point, Viewport, WheelConfig, WheelEvent, WheelInput,
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

fn build_3level_pair(vp: Viewport) -> (WheelSession, WheelSession) {
    let cfg = WheelConfig {
        reduced_motion: true,
        ..WheelConfig::default()
    };
    let mut events = Vec::<WheelEvent>::new();

    let mut s1 = WheelSession::new(
        Some(build_demo_menu()),
        Point::new(960.0, 540.0),
        vp,
        InputMode::Drag,
        &cfg,
        0,
    );
    let p_custom = s1.item_center(&ItemPath::from_slice(&[3])).unwrap();
    s1.handle(WheelInput::PointerMoved(p_custom), 10, &mut events);
    s1.tick(200, &mut events);

    let p_scripts = s1.item_center(&ItemPath::from_slice(&[3, 1])).unwrap();
    s1.handle(WheelInput::PointerMoved(p_scripts), 210, &mut events);
    s1.tick(400, &mut events);

    let mut s2 = WheelSession::new(
        Some(build_demo_menu()),
        Point::new(960.0, 540.0),
        vp,
        InputMode::Drag,
        &cfg,
        0,
    );
    s2.handle(WheelInput::PointerMoved(p_custom), 10, &mut events);
    s2.tick(200, &mut events);
    s2.handle(WheelInput::PointerMoved(p_scripts), 210, &mut events);
    s2.tick(400, &mut events);

    let p_sha = s1.item_center(&ItemPath::from_slice(&[3, 1, 0])).unwrap();
    let p_chmod = s1.item_center(&ItemPath::from_slice(&[3, 1, 1])).unwrap();
    s1.handle(WheelInput::PointerMoved(p_sha), 410, &mut events);
    s2.handle(WheelInput::PointerMoved(p_chmod), 410, &mut events);

    (s1, s2)
}

fn bench_render(c: &mut Criterion) {
    // 1x (1920x1080 @ scale 1.0) warm single-segment-highlight update
    {
        let (s1, s2) = build_3level_pair(Viewport::new(1920.0, 1080.0, 1.0));
        let mut renderer = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
        renderer.begin_session(SizePx::new(1920, 1080), 1.0);
        let _ = renderer.render(s1.scene());
        let _ = renderer.render(s2.scene());
        let mut toggle = false;
        c.bench_function("render_warm_3level_1080p_1x", |b| {
            b.iter(|| {
                toggle = !toggle;
                let scene = if toggle { s1.scene() } else { s2.scene() };
                let dmg = renderer.render(black_box(scene));
                black_box(dmg.len())
            })
        });
    }

    // 2x (3840x2160 @ scale 2.0) warm single-segment-highlight update
    {
        let (s1, s2) = build_3level_pair(Viewport::new(1920.0, 1080.0, 2.0));
        let mut renderer = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
        renderer.begin_session(SizePx::new(3840, 2160), 2.0);
        let _ = renderer.render(s1.scene());
        let _ = renderer.render(s2.scene());
        let mut toggle = false;
        c.bench_function("render_warm_3level_4k_2x", |b| {
            b.iter(|| {
                toggle = !toggle;
                let scene = if toggle { s1.scene() } else { s2.scene() };
                let dmg = renderer.render(black_box(scene));
                black_box(dmg.len())
            })
        });
    }
}

criterion_group!(benches, bench_render);
criterion_main!(benches);
