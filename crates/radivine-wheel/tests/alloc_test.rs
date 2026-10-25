//! Zero-allocation steady-state test for `radivine-wheel` (`AC-1a.6`, `TESTING_STRATEGY.md` §3).
//!
//! Asserts that `WheelSession::handle`, `WheelSession::tick`, and `WheelSession::scene`
//! perform **zero heap allocations** after session creation.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use radivine_wheel::{
    InputMode, ItemPath, MenuNode, MenuTree, Point, Viewport, WheelConfig, WheelEvent, WheelInput,
    WheelLayout, WheelSession,
};

struct CountingAlloc;

static TRACKING_ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACKING_ENABLED.load(Ordering::Relaxed) {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if TRACKING_ENABLED.load(Ordering::Relaxed) {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn build_menu() -> MenuTree {
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
        .with_icon("lucide:sparkles"),
    ])
}

#[test]
fn test_zero_heap_allocations_in_handle_tick_and_scene() {
    let cfg = WheelConfig::default();
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let mut session = WheelSession::new(
        Some(build_menu()),
        Point::new(600.0, 500.0),
        vp,
        InputMode::Drag,
        &cfg,
        0,
    );
    let mut events: Vec<WheelEvent> = Vec::with_capacity(16);

    // Enable allocation tracking AFTER session creation & event Vec pre-allocation
    ALLOC_COUNT.store(0, Ordering::SeqCst);
    TRACKING_ENABLED.store(true, Ordering::SeqCst);

    // 1. Move into root item 0 ("File Preview"), dwell to open child wheel, tick animations
    events.clear();
    session.handle(
        WheelInput::PointerMoved(Point::new(600.0, 420.0)),
        20,
        &mut events,
    );
    let _ = session.scene();

    for step in 1..=15 {
        events.clear();
        session.tick(20 + step * 16, &mut events);
        let _ = session.scene();
    }

    // 2. Move into child item [0, 1] ("Head")
    if let Some(p_head) = session.item_center(&ItemPath::from_slice(&[0, 1])) {
        events.clear();
        session.handle(WheelInput::PointerMoved(p_head), 280, &mut events);
        let _ = session.scene();
    }

    // 3. Move back to root item 3 ("Custom Actions"), closing [0] and opening [3], then [3, 1]
    if let Some(p_custom) = session.item_center(&ItemPath::from_slice(&[3])) {
        events.clear();
        session.handle(WheelInput::PointerMoved(p_custom), 300, &mut events);
        for step in 1..=12 {
            events.clear();
            session.tick(300 + step * 16, &mut events);
            let _ = session.scene();
        }
    }
    if let Some(p_scripts) = session.item_center(&ItemPath::from_slice(&[3, 1])) {
        events.clear();
        session.handle(WheelInput::PointerMoved(p_scripts), 520, &mut events);
        for step in 1..=12 {
            events.clear();
            session.tick(520 + step * 16, &mut events);
            let _ = session.scene();
        }
    }

    // 4. Commit drop and animate to completion
    let _ = session.commit_drop(750);
    for step in 1..=20 {
        events.clear();
        session.tick(750 + step * 16, &mut events);
        let _ = session.scene();
    }

    TRACKING_ENABLED.store(false, Ordering::SeqCst);
    let total_allocs = ALLOC_COUNT.load(Ordering::SeqCst);

    assert_eq!(
        total_allocs, 0,
        "expected 0 heap allocations during Branching steady-state handle/tick/scene, got {total_allocs}"
    );

    // --- Part 2: Concentric steady-state zero-allocation verification & state-size budget ---
    let session_bytes = std::mem::size_of::<WheelSession>();
    assert!(
        session_bytes <= 65_536,
        "WheelSession size ({session_bytes} bytes) exceeds 64 KiB budget"
    );

    let conc_cfg = WheelConfig {
        layout: WheelLayout::Concentric,
        radius_root: [40.0, 128.0],
        dwell_ms: 110,
        grace_ms: 180,
        hub_close_ms: 150,
        ..WheelConfig::default()
    };
    let mut conc_session = WheelSession::new(
        Some(build_menu()),
        Point::new(960.0, 540.0),
        vp,
        InputMode::Drag,
        &conc_cfg,
        0,
    );
    events.clear();

    ALLOC_COUNT.store(0, Ordering::SeqCst);
    TRACKING_ENABLED.store(true, Ordering::SeqCst);

    // 1. Move into root item 0 ("File Preview"), dwell to open child ring, tick animations
    conc_session.handle(
        WheelInput::PointerMoved(Point::new(960.0, 460.0)),
        10,
        &mut events,
    );
    let _ = conc_session.scene();

    for step in 1..=12 {
        events.clear();
        conc_session.tick(10 + step * 16, &mut events);
        let _ = conc_session.scene();
    }

    // 2. Move into child item [0, 1] ("Head")
    if let Some(p_head) = conc_session.item_center(&ItemPath::from_slice(&[0, 1])) {
        events.clear();
        conc_session.handle(WheelInput::PointerMoved(p_head), 220, &mut events);
        let _ = conc_session.scene();
    }

    // 3. Move to root sibling 3 ("Custom Actions"), wait through grace & dwell to replace child ring,
    //    then open grandchild ring [3, 1] ("Scripts")
    if let Some(p_custom) = conc_session.item_center(&ItemPath::from_slice(&[3])) {
        events.clear();
        conc_session.handle(WheelInput::PointerMoved(p_custom), 240, &mut events);
        for step in 1..=20 {
            events.clear();
            conc_session.tick(240 + step * 16, &mut events);
            let _ = conc_session.scene();
        }
    }
    if let Some(p_scripts) = conc_session.item_center(&ItemPath::from_slice(&[3, 1])) {
        events.clear();
        conc_session.handle(WheelInput::PointerMoved(p_scripts), 580, &mut events);
        for step in 1..=12 {
            events.clear();
            conc_session.tick(580 + step * 16, &mut events);
            let _ = conc_session.scene();
        }
    }
    if let Some(p_sha) = conc_session.item_center(&ItemPath::from_slice(&[3, 1, 0])) {
        events.clear();
        conc_session.handle(WheelInput::PointerMoved(p_sha), 790, &mut events);
        let _ = conc_session.scene();
    }

    // 4. Commit drop and animate to completion
    let _ = conc_session.commit_drop(800);
    for step in 1..=20 {
        events.clear();
        conc_session.tick(800 + step * 16, &mut events);
        let _ = conc_session.scene();
    }

    TRACKING_ENABLED.store(false, Ordering::SeqCst);
    let conc_allocs = ALLOC_COUNT.load(Ordering::SeqCst);

    assert_eq!(
        conc_allocs, 0,
        "expected 0 heap allocations during Concentric steady-state handle/tick/scene, got {conc_allocs}"
    );
}
