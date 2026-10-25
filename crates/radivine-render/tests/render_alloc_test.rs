//! Warm zero-allocation test for `radivine-render` (`T-PERF1`, `T-PERF3`, `ARCHITECTURE.md` §A5).
//!
//! Asserts that:
//! 1. Warm `WheelLayout::Concentric` rendering performs **zero heap allocations** across a full
//!    multi-ring navigation, hover, breadcrumb, origin-guide, and collapse trace.
//! 2. Unchanged-scene renders perform **zero heap allocations** in both layouts.
//! 3. Cold vs warm allocation counts for both `Concentric` and `Branching` are measured and
//!    reported separately.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use radivine_render::{AssetSource, Renderer, SizePx, Theme};
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

static ALLOC_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn build_menu() -> MenuTree {
    MenuTree::new(vec![
        MenuNode::submenu(
            "radivine.preview",
            "File Preview",
            vec![
                MenuNode::leaf("radivine.preview.full", "Full Preview")
                    .with_icon("lucide:eye")
                    .with_description("Open full file preview"),
                MenuNode::leaf("radivine.preview.head", "Head")
                    .with_icon("lucide:arrow-up-to-line")
                    .with_description("Show first 50 lines"),
                MenuNode::leaf("radivine.preview.tail", "Tail")
                    .with_icon("lucide:arrow-down-to-line")
                    .with_description("Show last 50 lines"),
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
                MenuNode::leaf("custom.terminal", "Open in Terminal")
                    .with_icon("lucide:terminal")
                    .with_description("Launch terminal in folder"),
                MenuNode::submenu(
                    "custom.scripts",
                    "Scripts",
                    vec![
                        MenuNode::leaf("custom.scripts.sha256", "Compute SHA-256")
                            .with_icon("lucide:code")
                            .with_description("Calculate SHA-256 digest"),
                        MenuNode::leaf("custom.scripts.chmod", "Make Executable")
                            .with_icon("lucide:sparkles")
                            .with_description("Set executable permission bit"),
                    ],
                )
                .with_icon("lucide:code")
                .with_description("Run helper scripts"),
            ],
        )
        .with_icon("lucide:sparkles")
        .with_description("Run custom tools"),
    ])
}

fn run_navigation_trace(
    session: &mut WheelSession,
    renderer: &mut Renderer,
    events: &mut Vec<WheelEvent>,
) -> usize {
    let mut frames = 0usize;

    // 1. Root appear animation
    for step in 0..=8 {
        events.clear();
        session.tick(step * 30, events);
        let _ = renderer.render(session.scene());
        frames += 1;
    }

    // 2. Hover root item 1 ("Copy")
    if let Some(p_copy) = session.item_center(&ItemPath::from_slice(&[1])) {
        events.clear();
        session.handle(WheelInput::PointerMoved(p_copy), 260, events);
        let _ = renderer.render(session.scene());
        frames += 1;
    }

    // 3. Hover root submenu 3 ("Custom Actions") and dwell to open ring 1
    if let Some(p_custom) = session.item_center(&ItemPath::from_slice(&[3])) {
        events.clear();
        session.handle(WheelInput::PointerMoved(p_custom), 280, events);
        let _ = renderer.render(session.scene());
        frames += 1;
        for step in 1..=12 {
            events.clear();
            session.tick(280 + step * 20, events);
            let _ = renderer.render(session.scene());
            frames += 1;
        }
    }

    // 4. Move into child submenu [3, 1] ("Scripts") and dwell to open ring 2
    if let Some(p_scripts) = session.item_center(&ItemPath::from_slice(&[3, 1])) {
        events.clear();
        session.handle(WheelInput::PointerMoved(p_scripts), 540, events);
        session.handle(
            WheelInput::PointerMoved(Point::new(p_scripts.x + 3.0, p_scripts.y)),
            545,
            events,
        );
        let _ = renderer.render(session.scene());
        frames += 1;
        for step in 1..=12 {
            events.clear();
            session.tick(545 + step * 20, events);
            let _ = renderer.render(session.scene());
            frames += 1;
        }
    }

    // 5. Hover grandchild leaf [3, 1, 0] ("Compute SHA-256")
    if let Some(p_sha) = session.item_center(&ItemPath::from_slice(&[3, 1, 0])) {
        events.clear();
        session.handle(WheelInput::PointerMoved(p_sha), 800, events);
        session.handle(
            WheelInput::PointerMoved(Point::new(p_sha.x + 3.0, p_sha.y)),
            805,
            events,
        );
        let _ = renderer.render(session.scene());
        frames += 1;
    }

    // 6. Collapse via hub
    let center = session.scene().wheels[0].center;
    events.clear();
    session.handle(WheelInput::PointerMoved(center), 820, events);
    for step in 1..=15 {
        events.clear();
        session.tick(820 + step * 20, events);
        let _ = renderer.render(session.scene());
        frames += 1;
    }

    frames
}

#[test]
fn test_warm_renderer_zero_heap_allocations_concentric_and_baseline_comparison() {
    let _guard = ALLOC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let size = SizePx::new(1280, 800);
    let vp = Viewport::new(1280.0, 800.0, 1.0);
    let mut events = Vec::with_capacity(16);

    // --- 1. Concentric Cold vs Warm ---
    let mut conc_renderer = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
    conc_renderer.begin_session(size, 1.0);

    let conc_cfg = WheelConfig {
        layout: WheelLayout::Concentric,
        ..WheelConfig::default()
    };

    let mut conc_cold_session = WheelSession::new(
        Some(build_menu()),
        Point::new(48.0, 48.0), // displaced anchor so origin_guide is also exercised
        vp,
        InputMode::Drag,
        &conc_cfg,
        0,
    );
    conc_cold_session.set_root_badge(Some(3), 0);
    let c_center = conc_cold_session.scene().wheels[0].center;
    conc_cold_session.handle(WheelInput::PointerMoved(c_center), 5, &mut events);

    ALLOC_COUNT.store(0, Ordering::SeqCst);
    TRACKING_ENABLED.store(true, Ordering::SeqCst);
    let conc_frames = run_navigation_trace(&mut conc_cold_session, &mut conc_renderer, &mut events);
    TRACKING_ENABLED.store(false, Ordering::SeqCst);
    let conc_cold_allocs = ALLOC_COUNT.load(Ordering::SeqCst);

    let mut conc_warm_session = WheelSession::new(
        Some(build_menu()),
        Point::new(48.0, 48.0),
        vp,
        InputMode::Drag,
        &conc_cfg,
        0,
    );
    conc_warm_session.set_root_badge(Some(3), 0);
    conc_warm_session.handle(WheelInput::PointerMoved(c_center), 5, &mut events);
    events.clear();

    ALLOC_COUNT.store(0, Ordering::SeqCst);
    TRACKING_ENABLED.store(true, Ordering::SeqCst);
    let _ = run_navigation_trace(&mut conc_warm_session, &mut conc_renderer, &mut events);
    TRACKING_ENABLED.store(false, Ordering::SeqCst);
    let conc_warm_allocs = ALLOC_COUNT.load(Ordering::SeqCst);

    assert_eq!(
        conc_warm_allocs, 0,
        "expected 0 heap allocations during warm Concentric navigation + rendering ({conc_frames} frames), got {conc_warm_allocs}"
    );

    // --- 2. Branching Cold vs Warm (baseline comparison) ---
    let mut branch_renderer = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
    branch_renderer.begin_session(size, 1.0);

    let branch_cfg = WheelConfig {
        layout: WheelLayout::Branching,
        ..WheelConfig::default()
    };

    let mut branch_cold_session = WheelSession::new(
        Some(build_menu()),
        Point::new(640.0, 400.0),
        vp,
        InputMode::Drag,
        &branch_cfg,
        0,
    );
    branch_cold_session.set_root_badge(Some(3), 0);

    ALLOC_COUNT.store(0, Ordering::SeqCst);
    TRACKING_ENABLED.store(true, Ordering::SeqCst);
    let branch_frames =
        run_navigation_trace(&mut branch_cold_session, &mut branch_renderer, &mut events);
    TRACKING_ENABLED.store(false, Ordering::SeqCst);
    let branch_cold_allocs = ALLOC_COUNT.load(Ordering::SeqCst);

    let mut branch_warm_session = WheelSession::new(
        Some(build_menu()),
        Point::new(640.0, 400.0),
        vp,
        InputMode::Drag,
        &branch_cfg,
        0,
    );
    branch_warm_session.set_root_badge(Some(3), 0);
    events.clear();

    ALLOC_COUNT.store(0, Ordering::SeqCst);
    TRACKING_ENABLED.store(true, Ordering::SeqCst);
    let _ = run_navigation_trace(&mut branch_warm_session, &mut branch_renderer, &mut events);
    TRACKING_ENABLED.store(false, Ordering::SeqCst);
    let branch_warm_allocs = ALLOC_COUNT.load(Ordering::SeqCst);

    // Re-rendering an unchanged Branching scene must also allocate 0
    ALLOC_COUNT.store(0, Ordering::SeqCst);
    TRACKING_ENABLED.store(true, Ordering::SeqCst);
    let zero_dmg = branch_renderer.render(branch_warm_session.scene());
    TRACKING_ENABLED.store(false, Ordering::SeqCst);
    let branch_noop_allocs = ALLOC_COUNT.load(Ordering::SeqCst);
    assert!(zero_dmg.is_empty());
    assert_eq!(branch_noop_allocs, 0);

    println!(
        "RENDER_ALLOC_SUMMARY: Concentric cold={conc_cold_allocs} ({:.1}/frame), warm={conc_warm_allocs} (0.0/frame) over {conc_frames} frames | Branching cold={branch_cold_allocs} ({:.1}/frame), warm={branch_warm_allocs} ({:.1}/frame) over {branch_frames} frames",
        (conc_cold_allocs as f64) / (conc_frames as f64),
        (branch_cold_allocs as f64) / (branch_frames as f64),
        (branch_warm_allocs as f64) / (branch_frames as f64),
    );
    assert!(
        branch_warm_allocs < branch_cold_allocs,
        "warm Branching should allocate strictly less than cold Branching"
    );
}

fn build_deep_6x8_menu(depth: usize) -> MenuTree {
    fn make_level(cur: usize, max_depth: usize) -> Vec<MenuNode> {
        let icons = [
            "lucide:folder",
            "lucide:eye",
            "lucide:copy",
            "lucide:terminal",
            "lucide:code",
            "lucide:file-text",
            "lucide:info",
            "lucide:sparkles",
        ];
        let mut nodes = Vec::with_capacity(8);
        if cur < max_depth {
            nodes.push(
                MenuNode::submenu(
                    format!("perf.L{cur}.sub0"),
                    format!("Submenu L{cur}A"),
                    make_level(cur + 1, max_depth),
                )
                .with_icon(icons[cur % icons.len()])
                .with_description(format!("Open ring {}", cur + 1)),
            );
            nodes.push(
                MenuNode::submenu(
                    format!("perf.L{cur}.sub1"),
                    format!("Submenu L{cur}B"),
                    vec![
                        MenuNode::leaf(format!("perf.L{cur}.b0"), "Branch Action 1")
                            .with_icon("lucide:sparkles"),
                        MenuNode::leaf(format!("perf.L{cur}.b1"), "Branch Action 2")
                            .with_icon("lucide:copy"),
                    ],
                )
                .with_icon(icons[(cur + 1) % icons.len()])
                .with_description(format!("Alternate branch at ring {cur}")),
            );
        }
        while nodes.len() < 8 {
            let idx = nodes.len();
            nodes.push(
                MenuNode::leaf(
                    format!("perf.L{cur}.leaf{idx}"),
                    format!("Action {cur}-{idx}"),
                )
                .with_icon(icons[(cur + idx) % icons.len()])
                .with_description(format!("Deterministic leaf action {cur}-{idx}")),
            );
        }
        nodes
    }
    MenuTree::new(make_level(1, depth))
}

fn compute_percentiles_us(samples_ns: &mut [u64]) -> (f64, f64, f64, f64) {
    samples_ns.sort_unstable();
    let n = samples_ns.len();
    let p50 = (samples_ns[n * 50 / 100] as f64) / 1_000.0;
    let p95 = (samples_ns[n * 95 / 100] as f64) / 1_000.0;
    let p99 = (samples_ns[n * 99 / 100] as f64) / 1_000.0;
    let mean = (samples_ns.iter().sum::<u64>() as f64) / (n as f64) / 1_000.0;
    (mean, p50, p95, p99)
}

#[test]
fn t_perf1_t_perf2_t_perf3_sample_percentiles_and_damage_metrics() {
    use std::time::Instant;
    let _guard = ALLOC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let session_bytes = std::mem::size_of::<WheelSession>();
    println!("T-PERF1/5 WheelSession stack size: {session_bytes} B (budget <= 65536 B)");
    assert!(
        session_bytes <= 65_536,
        "WheelSession size {session_bytes} exceeds 64 KiB budget"
    );

    // --- T-PERF2: Concentric engine handle + tick + scene across >= 1,000 samples (30 trials x 40 steps) ---
    {
        let vp = Viewport::new(1920.0, 1200.0, 1.0);
        let cfg = WheelConfig {
            layout: WheelLayout::Concentric,
            radius_root: [40.0, 128.0],
            max_depth: 6,
            ..WheelConfig::default()
        };
        let mut session = WheelSession::new(
            Some(build_deep_6x8_menu(6)),
            Point::new(960.0, 560.0),
            vp,
            InputMode::Drag,
            &cfg,
            0,
        );
        let mut events = Vec::with_capacity(16);
        let mut t = 10u64;
        let zeros = [0u8; 6];
        for lvl in 1..=5 {
            let p = session
                .item_center(&ItemPath::from_slice(&zeros[..lvl]))
                .unwrap();
            session.handle(WheelInput::PointerMoved(p), t, &mut events);
            t += 200;
            session.tick(t, &mut events);
            t += 200;
            session.tick(t, &mut events);
        }
        let p_a = session
            .item_center(&ItemPath::from_slice(&[0, 0, 0, 0, 0, 2]))
            .unwrap();
        let p_b = session
            .item_center(&ItemPath::from_slice(&[0, 0, 0, 0, 0, 3]))
            .unwrap();

        // 3 warmups
        for w in 0..3 {
            t += 16;
            let pt = if w % 2 == 0 { p_a } else { p_b };
            events.clear();
            session.handle(WheelInput::PointerMoved(pt), t, &mut events);
            session.tick(t, &mut events);
            let _ = std::hint::black_box(session.scene());
        }

        let mut samples_ns = Vec::with_capacity(1200);
        ALLOC_COUNT.store(0, Ordering::SeqCst);
        TRACKING_ENABLED.store(true, Ordering::SeqCst);
        for trial in 0..30 {
            for step in 0..40 {
                t += 16;
                let pt = if (trial + step) % 2 == 0 { p_a } else { p_b };
                let t0 = Instant::now();
                events.clear();
                session.handle(
                    std::hint::black_box(WheelInput::PointerMoved(pt)),
                    t,
                    &mut events,
                );
                session.tick(t, &mut events);
                let _ = std::hint::black_box(session.scene());
                samples_ns.push(t0.elapsed().as_nanos() as u64);
            }
        }
        TRACKING_ENABLED.store(false, Ordering::SeqCst);
        let engine_allocs = ALLOC_COUNT.load(Ordering::SeqCst);
        let (mean_us, p50_us, p95_us, p99_us) = compute_percentiles_us(&mut samples_ns);
        println!(
            "T-PERF2 Concentric Engine (6-level, 1200 samples, 30 trials): mean={mean_us:.2}us, p50={p50_us:.2}us, p95={p95_us:.2}us, p99={p99_us:.2}us, allocs={engine_allocs}"
        );
        assert_eq!(
            engine_allocs, 0,
            "T-PERF2 warm engine handle+tick+scene must allocate 0"
        );
        if !cfg!(debug_assertions) {
            assert!(
                p95_us < 50.0,
                "T-PERF2 engine p95 {p95_us:.2}us exceeded 50us gate"
            );
        }
    }

    // Helper to build a pair of alternating scenes at a given depth & layout and measure 1,020 frame samples (34 per trial x 30 trials)
    let measure_pair = |label: &str,
                        layout: WheelLayout,
                        depth: usize,
                        w_px: u32,
                        h_px: u32,
                        scale: f32|
     -> (f64, f64, f64, usize, f64) {
        let vp = Viewport::new((w_px as f32) / scale, (h_px as f32) / scale, scale);
        let center = Point::new(vp.bounds.w * 0.5, vp.bounds.h * 0.48);
        let cfg = WheelConfig {
            layout,
            radius_root: if layout == WheelLayout::Concentric {
                [40.0, 128.0]
            } else {
                [38.0, 116.0]
            },
            max_depth: (depth as u8).max(3),
            reduced_motion: true,
            ..WheelConfig::default()
        };
        let tree = build_deep_6x8_menu(depth);
        let mut events = Vec::with_capacity(16);

        let mut setup_session = |leaf_idx: u8| -> WheelSession {
            let mut s = WheelSession::new(Some(tree.clone()), center, vp, InputMode::Drag, &cfg, 0);
            let mut t = 10u64;
            let mut p_arr = [0u8; 6];
            for lvl in 1..depth {
                let pt = s.item_center(&ItemPath::from_slice(&p_arr[..lvl])).unwrap();
                s.handle(WheelInput::PointerMoved(pt), t, &mut events);
                t += 200;
                s.tick(t, &mut events);
                t += 200;
                s.tick(t, &mut events);
            }
            p_arr[depth - 1] = leaf_idx;
            let pt_leaf = s
                .item_center(&ItemPath::from_slice(&p_arr[..depth]))
                .unwrap();
            s.handle(WheelInput::PointerMoved(pt_leaf), t + 10, &mut events);
            s.tick(t + 50, &mut events);
            s
        };

        let s_a = setup_session(2);
        let s_b = setup_session(3);

        let mut renderer = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
        renderer.begin_session(SizePx::new(w_px, h_px), scale);

        // Cold render timing
        let t_cold0 = Instant::now();
        let _ = renderer.render(s_a.scene());
        let cold_ms = t_cold0.elapsed().as_secs_f64() * 1_000.0;
        let _ = renderer.render(s_b.scene());

        // 3 warmups
        for w in 0..6 {
            let sc = if w % 2 == 0 { s_a.scene() } else { s_b.scene() };
            let _ = renderer.render(sc);
        }

        let mut samples_ns = Vec::with_capacity(1020);
        let mut last_rect_count = 0usize;
        let mut last_dmg_px = 0u64;
        for trial in 0..30 {
            for step in 0..34 {
                let sc = if (trial * 34 + step) % 2 == 0 {
                    s_a.scene()
                } else {
                    s_b.scene()
                };
                let t0 = Instant::now();
                let dmg = renderer.render(std::hint::black_box(sc));
                samples_ns.push(t0.elapsed().as_nanos() as u64);
                last_rect_count = dmg.len();
                last_dmg_px = dmg.iter().map(|r| u64::from(r.w) * u64::from(r.h)).sum();
            }
        }
        let (mean_us, p50_us, p95_us, p99_us) = compute_percentiles_us(&mut samples_ns);
        let mean_ms = mean_us / 1_000.0;
        let p50_ms = p50_us / 1_000.0;
        let p95_ms = p95_us / 1_000.0;
        let p99_ms = p99_us / 1_000.0;
        let total_px = f64::from(w_px) * f64::from(h_px);
        let dmg_pct = (last_dmg_px as f64) * 100.0 / total_px;
        println!(
            "{label}: cold={cold_ms:.2}ms | warm (1020 samples, 30 trials) mean={mean_ms:.3}ms, p50={p50_ms:.3}ms, p95={p95_ms:.3}ms, p99={p99_ms:.3}ms | dmg_rects={last_rect_count}, dmg_area={last_dmg_px}px ({dmg_pct:.2}%)"
        );
        (p50_ms, p95_ms, cold_ms, last_rect_count, dmg_pct)
    };

    // T-PERF1 Branching baselines
    let (_, branch_1080_p95, _, _, branch_1080_pct) = measure_pair(
        "T-PERF1 Branching 3-level 1920x1080@1x",
        WheelLayout::Branching,
        3,
        1920,
        1080,
        1.0,
    );
    let (_, branch_4k_p95, _, _, branch_4k_pct) = measure_pair(
        "T-PERF1 Branching 3-level 3840x2160@2x",
        WheelLayout::Branching,
        3,
        3840,
        2160,
        2.0,
    );
    assert!(branch_1080_pct < 30.0);
    assert!(branch_4k_pct < 30.0);

    // T-PERF3 Concentric 1920x1080@1x (1, 3, 4, 6 levels)
    let (c1_1080_p50, c1_1080_p95, _, _, _) = measure_pair(
        "T-PERF3 Concentric 1-level 1920x1080@1x",
        WheelLayout::Concentric,
        1,
        1920,
        1080,
        1.0,
    );
    let (c3_1080_p50, c3_1080_p95, _, _, _) = measure_pair(
        "T-PERF3 Concentric 3-level 1920x1080@1x",
        WheelLayout::Concentric,
        3,
        1920,
        1080,
        1.0,
    );
    let (c4_1080_p50, c4_1080_p95, _, _, _) = measure_pair(
        "T-PERF3 Concentric 4-level 1920x1080@1x",
        WheelLayout::Concentric,
        4,
        1920,
        1080,
        1.0,
    );
    let (c6_1080_p50, c6_1080_p95, _, _, _) = measure_pair(
        "T-PERF3 Concentric 6-level 1920x1200@1x",
        WheelLayout::Concentric,
        6,
        1920,
        1200,
        1.0,
    );

    // T-PERF3 Concentric 3840x2160@2x (1, 3, 4, 6 levels)
    let (c1_4k_p50, c1_4k_p95, _, _, _) = measure_pair(
        "T-PERF3 Concentric 1-level 3840x2160@2x",
        WheelLayout::Concentric,
        1,
        3840,
        2160,
        2.0,
    );
    let (c3_4k_p50, c3_4k_p95, _, _, _) = measure_pair(
        "T-PERF3 Concentric 3-level 3840x2160@2x",
        WheelLayout::Concentric,
        3,
        3840,
        2160,
        2.0,
    );
    let (c4_4k_p50, c4_4k_p95, _, _, _) = measure_pair(
        "T-PERF3 Concentric 4-level 3840x2160@2x",
        WheelLayout::Concentric,
        4,
        3840,
        2160,
        2.0,
    );
    let (c6_4k_p50, c6_4k_p95, _, _, _) = measure_pair(
        "T-PERF3 Concentric 6-level 3840x2400@2x",
        WheelLayout::Concentric,
        6,
        3840,
        2400,
        2.0,
    );

    // T-PERF3 4-level Ring Opening / Sibling Replacement transition (branch 0 <-> branch 1)
    let measure_ring_replace = |label: &str, w_px: u32, h_px: u32, scale: f32| -> (f64, f64) {
        let vp = Viewport::new((w_px as f32) / scale, (h_px as f32) / scale, scale);
        let center = Point::new(vp.bounds.w * 0.5, vp.bounds.h * 0.48);
        let cfg = WheelConfig {
            layout: WheelLayout::Concentric,
            radius_root: [40.0, 128.0],
            max_depth: 4,
            reduced_motion: true,
            ..WheelConfig::default()
        };
        let tree = build_deep_6x8_menu(4);
        let mut events = Vec::with_capacity(16);

        // s_branch0: 4 rings open ([0, 0, 0, 2]); s_branch1: 2 rings open via sibling [1, 0]
        let mut s_branch0 =
            WheelSession::new(Some(tree.clone()), center, vp, InputMode::Drag, &cfg, 0);
        let mut t = 10u64;
        for path in [&[0u8][..], &[0, 0][..], &[0, 0, 0][..]] {
            let pt = s_branch0.item_center(&ItemPath::from_slice(path)).unwrap();
            s_branch0.handle(WheelInput::PointerMoved(pt), t, &mut events);
            t += 200;
            s_branch0.tick(t, &mut events);
            t += 200;
            s_branch0.tick(t, &mut events);
        }
        let pt_leaf0 = s_branch0
            .item_center(&ItemPath::from_slice(&[0, 0, 0, 2]))
            .unwrap();
        s_branch0.handle(WheelInput::PointerMoved(pt_leaf0), t + 10, &mut events);
        s_branch0.tick(t + 50, &mut events);

        let mut s_branch1 = WheelSession::new(Some(tree), center, vp, InputMode::Drag, &cfg, 0);
        let pt_sub1 = s_branch1.item_center(&ItemPath::from_slice(&[1])).unwrap();
        s_branch1.handle(WheelInput::PointerMoved(pt_sub1), 10, &mut events);
        s_branch1.tick(200, &mut events);
        s_branch1.tick(400, &mut events);
        let pt_b0 = s_branch1
            .item_center(&ItemPath::from_slice(&[1, 0]))
            .unwrap();
        s_branch1.handle(WheelInput::PointerMoved(pt_b0), 410, &mut events);
        s_branch1.tick(450, &mut events);

        let mut renderer = Renderer::new(Theme::dark(), AssetSource::Embedded).unwrap();
        renderer.begin_session(SizePx::new(w_px, h_px), scale);
        for w in 0..6 {
            let sc = if w % 2 == 0 {
                s_branch0.scene()
            } else {
                s_branch1.scene()
            };
            let _ = renderer.render(sc);
        }

        let mut samples_ns = Vec::with_capacity(1020);
        let mut last_rect_count = 0usize;
        let mut last_dmg_px = 0u64;
        for trial in 0..30 {
            for step in 0..34 {
                let sc = if (trial * 34 + step) % 2 == 0 {
                    s_branch0.scene()
                } else {
                    s_branch1.scene()
                };
                let t0 = Instant::now();
                let dmg = renderer.render(std::hint::black_box(sc));
                samples_ns.push(t0.elapsed().as_nanos() as u64);
                last_rect_count = dmg.len();
                last_dmg_px = dmg.iter().map(|r| u64::from(r.w) * u64::from(r.h)).sum();
            }
        }
        let (mean_us, p50_us, p95_us, p99_us) = compute_percentiles_us(&mut samples_ns);
        let total_px = f64::from(w_px) * f64::from(h_px);
        let dmg_pct = (last_dmg_px as f64) * 100.0 / total_px;
        println!(
            "{label}: warm (1020 samples, 30 trials) mean={:.3}ms, p50={:.3}ms, p95={:.3}ms, p99={:.3}ms | dmg_rects={last_rect_count}, dmg_area={last_dmg_px}px ({dmg_pct:.2}%)",
            mean_us / 1_000.0,
            p50_us / 1_000.0,
            p95_us / 1_000.0,
            p99_us / 1_000.0,
        );
        (p50_us / 1_000.0, p95_us / 1_000.0)
    };

    let (rep_1080_p50, rep_1080_p95) = measure_ring_replace(
        "T-PERF3 Concentric 4-level Ring-Replace 1920x1080@1x",
        1920,
        1080,
        1.0,
    );
    let (rep_4k_p50, rep_4k_p95) = measure_ring_replace(
        "T-PERF3 Concentric 4-level Ring-Replace 3840x2160@2x",
        3840,
        2160,
        2.0,
    );

    if !cfg!(debug_assertions) {
        assert!(branch_1080_p95 < 4.0);
        assert!(branch_4k_p95 < 6.0);
        for (p50, p95) in [
            (c1_1080_p50, c1_1080_p95),
            (c3_1080_p50, c3_1080_p95),
            (c4_1080_p50, c4_1080_p95),
            (c6_1080_p50, c6_1080_p95),
            (rep_1080_p50, rep_1080_p95),
        ] {
            assert!(
                p50 < 2.5 && p95 < 4.0,
                "1080p@1x gate failed: p50={p50:.3}ms, p95={p95:.3}ms"
            );
        }
        for (p50, p95) in [
            (c1_4k_p50, c1_4k_p95),
            (c3_4k_p50, c3_4k_p95),
            (c4_4k_p50, c4_4k_p95),
            (c6_4k_p50, c6_4k_p95),
            (rep_4k_p50, rep_4k_p95),
        ] {
            assert!(
                p50 < 4.0 && p95 < 6.0,
                "4K@2x gate failed: p50={p50:.3}ms, p95={p95:.3}ms"
            );
        }
    }
}
