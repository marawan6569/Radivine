//! Automated tests for `radivine-render` (`AC-1b.1`, `AC-1b.2`, `AC-1b.3`).

use std::path::PathBuf;
use std::sync::Arc;

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
fn test_ac_1b_1_renderer_init_embedded_and_dir_dark_and_light() {
    let mut r_dark = Renderer::new(Theme::dark(), AssetSource::Embedded)
        .expect("Embedded dark renderer must initialize");
    r_dark.begin_session(SizePx::new(800, 600), 1.0);

    let mut r_light = Renderer::new(Theme::light(), AssetSource::Embedded)
        .expect("Embedded light renderer must initialize");
    r_light.begin_session(SizePx::new(800, 600), 2.0);

    let assets_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let _r_dir = Renderer::new(Theme::dark(), AssetSource::Dir(assets_dir))
        .expect("Dir asset source must initialize");
}

#[test]
fn test_ac_1b_2_snapshot_renders_1_2_3_levels_and_disabled_caption() {
    let cfg = WheelConfig::default();
    let vp = Viewport::new(1280.0, 800.0, 1.0);
    let mut events: Vec<WheelEvent> = Vec::new();

    let mut renderer = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
    renderer.begin_session(SizePx::new(1280, 800), 1.0);

    // 1. Root wheel settled
    let mut session = WheelSession::new(
        Some(build_demo_menu()),
        Point::new(640.0, 420.0),
        vp,
        InputMode::Drag,
        &cfg,
        0,
    );
    session.set_root_badge(Some(3), 0);
    session.tick(250, &mut events);
    let dmg1 = renderer.render(session.scene());
    assert!(!dmg1.is_empty());
    let f1 = renderer.frame();
    let non_zero_1 = f1.data.chunks_exact(4).filter(|px| px[3] > 0).count();
    assert!(
        non_zero_1 > 10_000,
        "root wheel should render >10k opaque/semi-opaque pixels, got {non_zero_1}"
    );

    // Determinism check: rendering identical scene into a fresh renderer produces bit-exact output
    let mut renderer2 = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
    renderer2.begin_session(SizePx::new(1280, 800), 1.0);
    renderer2.render(session.scene());
    assert_eq!(renderer.frame().data, renderer2.frame().data);

    // 2. Root + Child + Stem (2 levels)
    let p_custom = session.item_center(&ItemPath::from_slice(&[3])).unwrap();
    session.handle(WheelInput::PointerMoved(p_custom), 260, &mut events);
    session.tick(500, &mut events);
    assert_eq!(session.scene().wheels.len(), 2);
    assert_eq!(session.scene().stems.len(), 1);
    let dmg2 = renderer.render(session.scene());
    assert!(!dmg2.is_empty() && dmg2.len() <= 4);
    let non_zero_2 = renderer
        .frame()
        .data
        .chunks_exact(4)
        .filter(|px| px[3] > 0)
        .count();
    assert!(
        non_zero_2 > non_zero_1 + 5_000,
        "2-level cluster should have significantly more drawn pixels than root alone"
    );

    // 3. Root + Child + Grandchild (3 levels)
    let p_scripts = session.item_center(&ItemPath::from_slice(&[3, 1])).unwrap();
    session.handle(WheelInput::PointerMoved(p_scripts), 520, &mut events);
    session.tick(760, &mut events);
    assert_eq!(session.scene().wheels.len(), 3);
    assert_eq!(session.scene().stems.len(), 2);
    let dmg3 = renderer.render(session.scene());
    assert!(!dmg3.is_empty() && dmg3.len() <= 4);

    // 4. Disabled item + caption
    session.set_enabled(
        &ItemPath::from_slice(&[3, 1]),
        false,
        Some(Arc::from("Script execution disabled in sandbox")),
    );
    let dmg4 = renderer.render(session.scene());
    assert!(!dmg4.is_empty() && dmg4.len() <= 4);
}

#[test]
fn test_ac_1b_3_damage_single_segment_highlight_under_30_percent() {
    let cfg = WheelConfig::default();
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let frame_area = 1920u64 * 1080u64;
    let mut events: Vec<WheelEvent> = Vec::new();

    let mut session = WheelSession::new(
        Some(build_demo_menu()),
        Point::new(960.0, 540.0),
        vp,
        InputMode::Drag,
        &cfg,
        0,
    );
    // Settle root appear animation first
    session.tick(250, &mut events);

    let mut renderer = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
    renderer.begin_session(SizePx::new(1920, 1080), 1.0);
    let initial_dmg = renderer.render(session.scene());
    assert!(!initial_dmg.is_empty());

    // Rendering an unchanged scene produces 0 damage rects
    let zero_dmg = renderer.render(session.scene());
    assert!(zero_dmg.is_empty());

    // Now hover a single leaf segment ("Copy", item 1)
    let p_copy = session.item_center(&ItemPath::from_slice(&[1])).unwrap();
    session.handle(WheelInput::PointerMoved(p_copy), 260, &mut events);
    let hover_dmg = renderer.render(session.scene());

    assert!(
        !hover_dmg.is_empty() && hover_dmg.len() <= 4,
        "expected 1..=4 damage rects, got {}",
        hover_dmg.len()
    );

    let damaged_area: u64 = hover_dmg.iter().map(|r| r.area()).sum();
    let ratio = (damaged_area as f64) / (frame_area as f64);
    assert!(
        ratio < 0.30,
        "expected damaged area < 30% of frame area, got {:.2}% ({damaged_area} / {frame_area})",
        ratio * 100.0
    );
}
