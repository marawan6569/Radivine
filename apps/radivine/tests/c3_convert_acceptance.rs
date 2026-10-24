//! Milestone C3 end-to-end acceptance test suite (`AC-C3.1` through `AC-C3.11`, `E1`–`E5`, `E8`).
//!
//! All tests run in strict isolation (`MockPlatform` or `RADIVINE_HEADLESS_DAEMON=1` + isolated
//! temporary XDG directories + `RADIVINE_SUPPRESS_DESKTOP_NOTIFY=1` +
//! `RADIVINE_DISABLE_SYSTEMD_USER_SPAWN=1`) and never touch the owner's live desktop session or
//! running `radivine.service`.

#![forbid(unsafe_code)]

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use image::{ImageFormat, Rgba, RgbaImage};
use radivine::config::AppConfig;
use radivine::session::SessionController;
use radivine_actions::{
    ActionsConfig, ConvertAction, ConvertMenuContext, PreviewSettings, builtin_registry,
    registry_from_options,
};
use radivine_convert::{CapabilityCatalog, FormatId};
use radivine_core::{
    Action, ActionId, BasicContext, Context, Dispatcher, Enablement, InvocationMode, JobEvent,
    MenuLimits, Outcome, SourceInfo, local_path_to_file_uri,
};
use radivine_jobs::{
    ItemState, JobId, JobState, JobsClient, WorkerLaunchMode, WorkerResponse, WorkerRuntimePaths,
};
use radivine_platform::{
    Capabilities, ClipboardContent, FrameRef as PlatFrameRef, Monitor, OverlayInfo, PhysPoint,
    Platform, PlatformError, PlatformEvent, RectPx, ShapePx, SourceInfo as PlatSourceInfo, Support,
};
use radivine_render::{AssetSource, Renderer};
use radivine_wheel::{InputMode, Point, Viewport, WheelConfig, WheelInput, WheelSession};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn radivine_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_radivine"))
}

fn ensure_worker_bin() -> PathBuf {
    static WORKER_BIN: OnceLock<PathBuf> = OnceLock::new();
    WORKER_BIN
        .get_or_init(|| {
            let bin_dir = radivine_bin()
                .parent()
                .expect("radivine bin parent")
                .to_path_buf();
            let candidate = bin_dir.join("radivine-worker");
            let mut cmd = Command::new("cargo");
            cmd.args([
                "build",
                "-q",
                "-p",
                "radivine-worker",
                "--bin",
                "radivine-worker",
            ]);
            if cfg!(not(debug_assertions)) {
                cmd.arg("--release");
            }
            let status = cmd
                .status()
                .expect("build radivine-worker binary for C3 tests");
            assert!(status.success(), "cargo build -p radivine-worker failed");
            assert!(
                candidate.exists(),
                "expected radivine-worker binary at {}",
                candidate.display()
            );
            candidate
        })
        .clone()
}

fn write_test_png(path: &Path, width: u32, height: u32, seed: u8) {
    let mut img = RgbaImage::new(width, height);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = Rgba([
            (x as u8).wrapping_add(seed),
            (y as u8).wrapping_mul(3).wrapping_add(seed),
            seed.wrapping_mul(17),
            255,
        ]);
    }
    img.save_with_format(path, ImageFormat::Png)
        .expect("write test PNG");
}

fn write_test_jpeg(path: &Path, width: u32, height: u32, seed: u8) {
    let mut img = RgbaImage::new(width, height);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = Rgba([
            (x as u8).wrapping_add(seed),
            (y as u8).wrapping_add(seed.wrapping_mul(5)),
            128,
            255,
        ]);
    }
    let rgb = image::DynamicImage::ImageRgba8(img).into_rgb8();
    rgb.save_with_format(path, ImageFormat::Jpeg)
        .expect("write test JPEG");
}

fn write_test_webp(path: &Path, width: u32, height: u32, seed: u8) {
    let mut img = RgbaImage::new(width, height);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = Rgba([
            (x as u8).wrapping_add(seed),
            (y as u8).wrapping_add(seed),
            200,
            255,
        ]);
    }
    img.save_with_format(path, ImageFormat::WebP)
        .expect("write test WebP");
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileFingerprint {
    inode: u64,
    size: u64,
    mtime: SystemTime,
    sha256: Vec<u8>,
}

impl FileFingerprint {
    fn capture(path: &Path) -> Self {
        let meta = fs::symlink_metadata(path).expect("symlink_metadata");
        let bytes = fs::read(path).expect("read file");
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        Self {
            inode: meta.ino(),
            size: meta.len(),
            mtime: meta.modified().expect("mtime"),
            sha256: hasher.finalize().to_vec(),
        }
    }
}

struct IsolatedWorkerEnv {
    _tmp: TempDir,
    root: PathBuf,
    paths: WorkerRuntimePaths,
    children: Vec<Child>,
}

impl IsolatedWorkerEnv {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let root = tmp.path().to_path_buf();
        let runtime_dir = root.join("run");
        let state_dir = root.join("state");
        let config_dir = root.join("config");
        fs::create_dir_all(&runtime_dir).unwrap();
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(&config_dir).unwrap();

        let paths = WorkerRuntimePaths {
            socket_path: runtime_dir.join("worker.sock"),
            lock_path: runtime_dir.join("worker.lock"),
            db_path: state_dir.join("jobs.sqlite3"),
            log_path: state_dir.join("worker.log"),
            worker_bin: Some(ensure_worker_bin()),
            launch_mode: WorkerLaunchMode::Spawn,
        };

        Self {
            _tmp: tmp,
            root,
            paths,
            children: Vec::new(),
        }
    }

    fn spawn_worker(&mut self, extra_env: &[(&str, &str)], idle_ms: u64) -> u32 {
        let mut cmd = Command::new(ensure_worker_bin());
        cmd.env("RADIVINE_WORKER_BIN", ensure_worker_bin())
            .env("RADIVINE_WORKER_SOCKET", &self.paths.socket_path)
            .env("RADIVINE_WORKER_DB", &self.paths.db_path)
            .env("RADIVINE_SUPPRESS_DESKTOP_NOTIFY", "1")
            .env("RADIVINE_DISABLE_SYSTEMD_USER_SPAWN", "1")
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .arg("--socket")
            .arg(&self.paths.socket_path)
            .arg("--db")
            .arg(&self.paths.db_path)
            .arg("--lock")
            .arg(&self.paths.lock_path)
            .arg("serve")
            .arg("--idle-timeout-ms")
            .arg(idle_ms.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().expect("spawn isolated radivine-worker");
        let pid = child.id();
        self.children.push(child);

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(mut client) = JobsClient::connect_with_timeout(
                &self.paths.socket_path,
                Duration::from_millis(100),
            ) && matches!(client.ping(), Ok(WorkerResponse::Pong { .. }))
            {
                return pid;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("isolated worker did not become ready within 5s");
    }

    fn wait_for_latest_job_terminal(
        &self,
        timeout: Duration,
    ) -> (radivine_jobs::JobSummary, Vec<radivine_jobs::JobItemReport>) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(mut client) = JobsClient::connect_with_timeout(
                &self.paths.socket_path,
                Duration::from_millis(200),
            ) && let Ok(jobs) = client.list_jobs(5, true)
                && let Some(latest) = jobs.first()
                && let Ok((job, items, _)) = client.get_job(latest.job_id, None, None)
                && job.state.is_terminal()
            {
                return (job, items);
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("latest job did not reach terminal state within {timeout:?}");
    }

    fn run_worker_cli(&self, args: &[&str]) -> std::process::Output {
        Command::new(ensure_worker_bin())
            .env("RADIVINE_SUPPRESS_DESKTOP_NOTIFY", "1")
            .env("RADIVINE_DISABLE_SYSTEMD_USER_SPAWN", "1")
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .arg("--socket")
            .arg(&self.paths.socket_path)
            .arg("--db")
            .arg(&self.paths.db_path)
            .arg("--lock")
            .arg(&self.paths.lock_path)
            .args(args)
            .output()
            .expect("run radivine-worker CLI")
    }
}

impl Drop for IsolatedWorkerEnv {
    fn drop(&mut self) {
        if let Ok(mut client) =
            JobsClient::connect_with_timeout(&self.paths.socket_path, Duration::from_millis(100))
        {
            let _ = client.shutdown_if_idle();
        }
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Default)]
struct MockPlatformState {
    mapped: bool,
    grabbed: bool,
    dnd_replies: Vec<bool>,
    dnd_finishes: Vec<bool>,
    requested_payloads: Vec<String>,
}

struct MockPlatform {
    caps: Capabilities,
    monitors: Vec<Monitor>,
    state: Arc<Mutex<MockPlatformState>>,
}

impl MockPlatform {
    fn new() -> (Self, Arc<Mutex<MockPlatformState>>) {
        let state = Arc::new(Mutex::new(MockPlatformState::default()));
        let plat = Self {
            caps: Capabilities {
                drag_session: Support::Yes,
                invoke_at_pointer: Support::Yes,
                translucency: Support::Yes,
                input_passthrough: Support::Yes,
                keyboard_in_drag: Support::No("drag source holds keyboard grab"),
                clipboard_files: Support::Yes,
            },
            monitors: vec![Monitor {
                id: 0,
                name: "MOCK-0".into(),
                rect: RectPx::new(0, 0, 1920, 1080),
                refresh_mhz: 60_000,
                primary: true,
            }],
            state: Arc::clone(&state),
        };
        (plat, state)
    }
}

impl Platform for MockPlatform {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    #[cfg(unix)]
    fn poll_fd(&self) -> std::os::fd::RawFd {
        -1
    }

    fn dispatch_pending(&mut self, _out: &mut Vec<PlatformEvent>) -> Result<(), PlatformError> {
        Ok(())
    }

    fn monitors(&self) -> &[Monitor] {
        &self.monitors
    }

    fn scale(&self) -> f32 {
        1.0
    }

    fn reduced_motion(&self) -> Option<bool> {
        Some(true)
    }

    fn pointer_position(&mut self) -> Result<PhysPoint, PlatformError> {
        Ok(PhysPoint::new(960, 540))
    }

    fn overlay_window_id(&self) -> u32 {
        0x0500_0099
    }

    fn prepare_overlay(&mut self, _at: PhysPoint) -> Result<OverlayInfo, PlatformError> {
        Ok(OverlayInfo {
            origin: PhysPoint::new(0, 0),
            size: (1920, 1080),
            scale: 1.0,
            monitor: self.monitors[0].clone(),
            translucent: true,
        })
    }

    fn map_overlay(&mut self) -> Result<(), PlatformError> {
        self.state.lock().unwrap().mapped = true;
        Ok(())
    }

    fn present(
        &mut self,
        _frame: PlatFrameRef<'_>,
        _damage: &[RectPx],
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    fn set_input_region(&mut self, _shapes: Option<&[ShapePx]>) -> Result<(), PlatformError> {
        Ok(())
    }

    fn hide_overlay(&mut self) {
        self.state.lock().unwrap().mapped = false;
    }

    fn release_buffers(&mut self) {}

    fn dnd_reply(&mut self, accept: bool) {
        self.state.lock().unwrap().dnd_replies.push(accept);
    }

    fn request_payload(&mut self, mime: &str) {
        self.state
            .lock()
            .unwrap()
            .requested_payloads
            .push(mime.to_string());
    }

    fn dnd_finish(&mut self, success: bool) {
        self.state.lock().unwrap().dnd_finishes.push(success);
    }

    fn grab_input(&mut self) -> Result<(), PlatformError> {
        self.state.lock().unwrap().grabbed = true;
        Ok(())
    }

    fn ungrab_input(&mut self) {
        self.state.lock().unwrap().grabbed = false;
    }

    fn set_clipboard(&mut self, _content: ClipboardContent) -> Result<(), PlatformError> {
        Ok(())
    }

    fn flush(&mut self) -> Result<(), PlatformError> {
        Ok(())
    }

    fn next_deadline(&self) -> Option<Instant> {
        None
    }

    fn on_deadline(&mut self, _now: Instant, _out: &mut Vec<PlatformEvent>) {}
}

/// Computes the sequence of physical `(x, y)` coordinates along a hierarchical menu path `path`
/// (e.g. `&[0, 0, 1]` for `Convert ▸ Image ▸ WebP` on a PNG selection) so a test can move the
/// pointer through each submenu ring and drop on the leaf.
fn compute_menu_path_points(
    reg: &radivine_core::ActionRegistry,
    uris: &[String],
    anchor: (i32, i32),
    path: &[u8],
) -> Vec<PhysPoint> {
    let basic = BasicContext::from_uris(uris, InvocationMode::Drag, SourceInfo::default());
    let ctx = Context::from_basic(basic);
    let (menu_tree, _) = reg
        .build_menu(&ctx, MenuLimits::default())
        .expect("build_menu");
    let cfg = WheelConfig {
        reduced_motion: true,
        ..AppConfig::default().to_wheel_config(true)
    };
    let viewport = Viewport::new(1920.0, 1080.0, 1.0);
    let mut ws = WheelSession::new(
        Some(menu_tree),
        Point::new(anchor.0 as f32, anchor.1 as f32),
        viewport,
        InputMode::Drag,
        &cfg,
        0,
    );

    let mut out = Vec::new();
    let mut now_ms = 10u64;
    let mut points = Vec::with_capacity(path.len());

    for (depth, &seg_idx) in path.iter().enumerate() {
        let dbg = ws.debug_state();
        let wheel = &dbg.wheels[depth];
        let span = &wheel.spans[seg_idx as usize];
        let mid_r = (wheel.r_in + wheel.r_out) * 0.5;
        let pt = wheel.center.offset_polar(mid_r, span.center);
        points.push(PhysPoint::new(pt.x.round() as i32, pt.y.round() as i32));

        if depth + 1 < path.len() {
            ws.handle(WheelInput::PointerMoved(pt), now_ms, &mut out);
            now_ms += 250;
            ws.tick(now_ms, &mut out);
        }
    }

    points
}

#[test]
fn ac_c3_1_c3_2_c3_3_c3_4_menu_tree_visibility_enablement_captions_and_zero_io() {
    let reg = builtin_registry();

    // 1. AC-C3.1: Text selection produces the exact v0.1.1 MenuTree (Convert pruned)
    let txt_ctx = Context::from_basic(BasicContext::from_uris(
        ["file:///tmp/notes.txt"],
        InvocationMode::Drag,
        SourceInfo::default(),
    ));
    let (txt_tree, _) = reg.build_menu(&txt_ctx, MenuLimits::default()).unwrap();
    let root_ids: Vec<&str> = txt_tree
        .root
        .children()
        .unwrap()
        .iter()
        .map(|n| n.id.as_ref())
        .collect();
    assert_eq!(
        root_ids,
        vec![
            "radivine.preview",
            "radivine.copy",
            "radivine.properties",
            "radivine.custom"
        ]
    );

    // 2. AC-C3.1: Image selection shows Convert -> Image -> {JPEG, WebP} (PNG hidden as same-format)
    let png_ctx = Context::from_basic(BasicContext::from_uris(
        ["file:///tmp/photo.png"],
        InvocationMode::Drag,
        SourceInfo::default(),
    ));
    let (png_tree, _) = reg.build_menu(&png_ctx, MenuLimits::default()).unwrap();
    let conv_node = &png_tree.root.children().unwrap()[0];
    assert_eq!(conv_node.id.as_ref(), "radivine.convert");
    let img_node = &conv_node.children().unwrap()[0];
    assert_eq!(img_node.id.as_ref(), "radivine.convert.image");
    let leaf_ids: Vec<&str> = img_node
        .children()
        .unwrap()
        .iter()
        .map(|n| n.id.as_ref())
        .collect();
    assert_eq!(
        leaf_ids,
        vec!["radivine.convert.image.jpeg", "radivine.convert.image.webp"]
    );
    assert_eq!(
        img_node.children().unwrap()[1].description.as_deref(),
        Some("Convert photo.png to WebP")
    );

    // 3. AC-C3.4: Zero-I/O check inside a 0o000 unreadable directory
    let tmp = tempfile::tempdir().unwrap();
    let locked_dir = tmp.path().join("locked_no_access");
    fs::create_dir_all(&locked_dir).unwrap();
    fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o000)).unwrap();
    let locked_img = locked_dir.join("secret_photo.png");
    let locked_basic = BasicContext::from_uris(
        [local_path_to_file_uri(&locked_img)],
        InvocationMode::Drag,
        SourceInfo::default(),
    );
    let webp_act = ConvertAction::new(FormatId::WebP, ConvertMenuContext::default());
    assert!(webp_act.is_visible(&locked_basic));
    assert_eq!(
        webp_act.description_for(&locked_basic).as_deref(),
        Some("Convert secret_photo.png to WebP")
    );
    fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn ac_c3_5_responsiveness_10k_visibility_and_summon_latency() {
    let webp_act = ConvertAction::new(FormatId::WebP, ConvertMenuContext::default());

    // 1. 10,000-item Convert visibility + caption time
    let uris_10k: Vec<String> = (0..10_000)
        .map(|i| format!("file:///tmp/perf_10k/image_{i:05}.png"))
        .collect();
    let basic_10k = BasicContext::from_uris(uris_10k, InvocationMode::Drag, SourceInfo::default());
    let mut vis_samples_us = Vec::with_capacity(40);
    for _ in 0..40 {
        let t0 = Instant::now();
        let v = webp_act.is_visible(&basic_10k);
        let d = webp_act.description_for(&basic_10k);
        vis_samples_us.push(t0.elapsed().as_micros() as u64);
        assert!(v);
        assert_eq!(d.as_deref(), Some("Convert 10000 images to WebP"));
    }
    vis_samples_us.sort_unstable();
    let p50_10k_us = vis_samples_us[vis_samples_us.len() / 2];
    let p95_10k_us = vis_samples_us[(vis_samples_us.len() * 95) / 100];
    eprintln!(
        "[C3 PERF] 10,000-item Convert visibility+caption: p50 = {p50_10k_us} us, p95 = {p95_10k_us} us"
    );
    assert!(
        p95_10k_us < 10_000,
        "10,000-item visibility + caption p95 = {p95_10k_us} us"
    );

    // 2. Trigger -> first frame summon latency comparison (image selection vs text selection)
    let tmp = tempfile::tempdir().unwrap();
    let sample_png = tmp.path().join("bench.png");
    let sample_txt = tmp.path().join("bench.txt");
    write_test_png(&sample_png, 8, 8, 1);
    fs::write(&sample_txt, b"hello benchmark\n").unwrap();
    let png_uri = local_path_to_file_uri(&sample_png);
    let txt_uri = local_path_to_file_uri(&sample_txt);

    let (mock, _) = MockPlatform::new();
    let cfg = AppConfig::default();
    let renderer = Renderer::new(cfg.resolve_theme(), AssetSource::Embedded).unwrap();
    let registry = Arc::new(builtin_registry());
    let mut ctrl = SessionController::new(Box::new(mock), renderer, registry, cfg);

    let mut txt_summon_us = Vec::with_capacity(20);
    let mut img_summon_us = Vec::with_capacity(20);
    for _ in 0..20 {
        ctrl.summon_invoke(vec![txt_uri.clone()], Some(PhysPoint::new(960, 540)))
            .unwrap();
        txt_summon_us.push(ctrl.status().last_summon_us.unwrap_or(0));
        ctrl.dismiss_immediate();

        ctrl.summon_invoke(vec![png_uri.clone()], Some(PhysPoint::new(960, 540)))
            .unwrap();
        img_summon_us.push(ctrl.status().last_summon_us.unwrap_or(0));
        ctrl.dismiss_immediate();
    }
    txt_summon_us.sort_unstable();
    img_summon_us.sort_unstable();
    let txt_p50 = txt_summon_us[txt_summon_us.len() / 2];
    let txt_p95 = txt_summon_us[(txt_summon_us.len() * 95) / 100];
    let img_p50 = img_summon_us[img_summon_us.len() / 2];
    let img_p95 = img_summon_us[(img_summon_us.len() * 95) / 100];
    let delta_us = img_p95.saturating_sub(txt_p95);
    eprintln!(
        "[C3 PERF] Summon latency: text p50={txt_p50} us / p95={txt_p95} us | image p50={img_p50} us / p95={img_p95} us (delta p95={delta_us} us)"
    );
    assert!(
        delta_us <= 2_000,
        "image summon p95 ({img_p95} us) exceeded text baseline ({txt_p95} us) by {delta_us} us"
    );
}

#[test]
fn ac_c3_6_and_c3_9_drag_and_invoke_wheel_quick_convert_and_mixed_batch_skips() {
    let mut worker_env = IsolatedWorkerEnv::new();
    worker_env.spawn_worker(&[], 60_000);

    let work_dir = worker_env.root.join("wheel_convert");
    fs::create_dir_all(&work_dir).unwrap();

    // 1. Single PNG -> Convert -> Image -> WebP via full Drag session on SessionController (AC-C3.6)
    let src_png = work_dir.join("hero.png");
    write_test_png(&src_png, 24, 16, 42);
    let src_fp_before = FileFingerprint::capture(&src_png);
    let src_uri = local_path_to_file_uri(&src_png);

    let convert_ctx = ConvertMenuContext::with_runtime_paths(
        Arc::new(CapabilityCatalog::default_builtin()),
        worker_env.paths.clone(),
    );
    let reg = Arc::new(
        registry_from_options(
            &ActionsConfig::default(),
            &PreviewSettings::default(),
            &convert_ctx,
        )
        .unwrap(),
    );

    let (mock, plat_state) = MockPlatform::new();
    let mut cfg = AppConfig::default();
    cfg.wheel.hover_dwell_ms = 10;
    let renderer = Renderer::new(cfg.resolve_theme(), AssetSource::Embedded).unwrap();
    let mut ctrl = SessionController::new(Box::new(mock), renderer, Arc::clone(&reg), cfg);
    ctrl.set_convert_menu_context(convert_ctx.clone());

    let anchor = (960, 540);
    // For single PNG, root[0] = Convert, child[0] = Image, grandchild[1] = WebP (since JPEG=0, WebP=1)
    let path_pts =
        compute_menu_path_points(&reg, std::slice::from_ref(&src_uri), anchor, &[0, 0, 1]);
    assert_eq!(path_pts.len(), 3);

    // Arm drag + trigger + deliver URI payload
    ctrl.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: PlatSourceInfo {
                app: Some("nemo".to_string()),
                pid: Some(7777),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(anchor.0, anchor.1),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".to_string()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".to_string(),
            result: Ok(format!("{src_uri}\r\n").into_bytes()),
        },
    ]);
    assert_eq!(ctrl.status().session_state, "Active");

    // Wait briefly for background enrichment of hero.png to complete
    let enrich_dl = Instant::now() + Duration::from_secs(2);
    while Instant::now() < enrich_dl {
        ctrl.drain_async_events(Instant::now());
        let st = ctrl.status();
        if st
            .active_item_states
            .iter()
            .any(|s| s.id == "radivine.convert.image.webp" && s.enabled)
        {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    // Navigate Convert ([0]) -> Image ([0, 0]) -> WebP ([0, 0, 1]) and drop!
    ctrl.handle_platform_events(vec![PlatformEvent::DndPosition { at: path_pts[0] }]);
    thread::sleep(Duration::from_millis(25));
    ctrl.on_timer(Instant::now());

    ctrl.handle_platform_events(vec![PlatformEvent::DndPosition { at: path_pts[1] }]);
    thread::sleep(Duration::from_millis(25));
    ctrl.on_timer(Instant::now());

    ctrl.handle_platform_events(vec![PlatformEvent::DndPosition { at: path_pts[2] }]);
    assert_eq!(
        plat_state.lock().unwrap().dnd_replies.last().copied(),
        Some(true),
        "WebP leaf must accept drop"
    );

    let t_drop = Instant::now();
    ctrl.handle_platform_events(vec![PlatformEvent::DndDrop]);
    let drop_wall_us = t_drop.elapsed().as_micros() as u64;
    let drop_wall_ms = drop_wall_us / 1_000;
    eprintln!(
        "[C3 PERF] Warm worker DndDrop -> job accepted wall time: {drop_wall_us} us ({:.2} ms)",
        drop_wall_us as f64 / 1000.0
    );
    assert!(
        drop_wall_ms < 50,
        "DndDrop + job submission took {drop_wall_ms} ms, must be < 50 ms with warm worker (AC-C3.6)"
    );
    {
        let st = plat_state.lock().unwrap();
        assert!(!st.mapped, "wheel overlay must unmap immediately on drop");
        assert_eq!(st.dnd_finishes, vec![true]);
    }

    let status = ctrl.status();
    assert_eq!(
        status.last_committed_action.as_deref(),
        Some("radivine.convert.image.webp")
    );
    assert_eq!(status.last_committed_source_app.as_deref(), Some("nemo"));
    assert_eq!(status.last_committed_mode.as_deref(), Some("Drag"));

    // Wait for worker job #1 to finish and verify hero.webp + source immutability
    let (job1, items1) = worker_env.wait_for_latest_job_terminal(Duration::from_secs(10));
    assert_eq!(job1.state, JobState::Completed);
    assert_eq!(job1.counts.succeeded_items, 1);
    assert_eq!(items1.len(), 1);
    assert_eq!(items1[0].state, ItemState::Succeeded);

    let out_webp = work_dir.join("hero.webp");
    assert!(
        out_webp.exists(),
        "hero.webp must be created beside hero.png"
    );
    let decoded = image::open(&out_webp).expect("decode hero.webp");
    assert_eq!(decoded.width(), 24);
    assert_eq!(decoded.height(), 16);
    assert_eq!(
        FileFingerprint::capture(&src_png),
        src_fp_before,
        "source hero.png must never be modified"
    );

    // 2. Collision safety (no-overwrite): running Convert -> WebP again creates `hero (1).webp`
    let first_webp_fp = FileFingerprint::capture(&out_webp);
    let webp_action = reg
        .get(&ActionId::from("radivine.convert.image.webp"))
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut dispatcher = Dispatcher::new(
        Box::new(move |ev| {
            let _ = tx.send(ev);
        }),
        2,
    );
    let ctx_single = Arc::new(radivine_core::enrich_context_sync(BasicContext::from_uris(
        [&src_uri],
        InvocationMode::Invoke,
        SourceInfo::default(),
    )));
    dispatcher.dispatch(webp_action.clone(), ctx_single);
    match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
        JobEvent::Finished { result, .. } => assert_eq!(result.unwrap(), Outcome::Done),
    }
    let (job2, _) = worker_env.wait_for_latest_job_terminal(Duration::from_secs(10));
    assert_eq!(job2.job_id, JobId(2));
    assert_eq!(job2.state, JobState::Completed);
    let numbered_webp = work_dir.join("hero (1).webp");
    assert!(
        numbered_webp.exists(),
        "collision must auto-number to hero (1).webp without overwriting hero.webp"
    );
    assert_eq!(
        FileFingerprint::capture(&out_webp),
        first_webp_fp,
        "pre-existing hero.webp must remain 100% untouched"
    );

    // 3. AC-C3.9: Mixed selection [img1.png, img2.jpg, already.webp, notes.txt] -> WebP
    let mix_dir = work_dir.join("mixed_selection");
    fs::create_dir_all(&mix_dir).unwrap();
    let m_png = mix_dir.join("img1.png");
    let m_jpg = mix_dir.join("img2.jpg");
    let m_webp = mix_dir.join("already.webp");
    let m_txt = mix_dir.join("notes.txt");
    write_test_png(&m_png, 16, 16, 10);
    write_test_jpeg(&m_jpg, 16, 16, 20);
    write_test_webp(&m_webp, 16, 16, 30);
    fs::write(&m_txt, b"meeting notes - not an image\n").unwrap();

    let ctx_mixed = Arc::new(radivine_core::enrich_context_sync(BasicContext::from_uris(
        [
            local_path_to_file_uri(&m_png),
            local_path_to_file_uri(&m_jpg),
            local_path_to_file_uri(&m_webp),
            local_path_to_file_uri(&m_txt),
        ],
        InvocationMode::Drag,
        SourceInfo::default(),
    )));
    assert_eq!(webp_action.enablement(&ctx_mixed), Enablement::Enabled);
    assert_eq!(
        webp_action.description_for(&ctx_mixed.basic).as_deref(),
        Some("Convert 2 of 4 files to WebP · 1 already WebP · 1 not supported")
    );

    dispatcher.dispatch(webp_action, ctx_mixed);
    match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
        JobEvent::Finished { result, .. } => assert_eq!(result.unwrap(), Outcome::Done),
    }

    let (job3, items3) = worker_env.wait_for_latest_job_terminal(Duration::from_secs(10));
    assert_eq!(job3.job_id, JobId(3));
    assert_eq!(job3.state, JobState::Completed);
    assert_eq!(job3.counts.total_items, 4);
    assert_eq!(job3.counts.succeeded_items, 2);
    assert_eq!(job3.counts.skipped_items, 2);
    assert_eq!(job3.counts.failed_items, 0);
    assert_eq!(
        job3.summary_text.as_deref(),
        Some("Converted 2 of 4 files (2 skipped)")
    );
    assert!(mix_dir.join("img1.webp").exists());
    assert!(mix_dir.join("img2.webp").exists());
    assert!(!mix_dir.join("already (1).webp").exists());
    assert!(!mix_dir.join("notes.webp").exists());

    assert_eq!(items3[0].state, ItemState::Succeeded);
    assert_eq!(items3[1].state, ItemState::Succeeded);
    assert_eq!(items3[2].state, ItemState::Skipped);
    assert_eq!(
        items3[2].error_code.as_deref(),
        Some("already_target_format")
    );
    assert_eq!(items3[3].state, ItemState::Skipped);
    assert_eq!(items3[3].error_code.as_deref(), Some("unsupported_format"));

    // Verify `radivine-worker show 3` CLI output reports the skipped items
    let show_out = worker_env.run_worker_cli(&["show", "3"]);
    assert!(show_out.status.success());
    let show_stdout = String::from_utf8_lossy(&show_out.stdout);
    assert!(
        show_stdout.contains("2 succeeded, 2 skipped")
            && show_stdout.contains("Source is already in webp format")
            && show_stdout.contains("Not a supported image format"),
        "radivine-worker show 3 must report both skipped items:\n{show_stdout}"
    );
}

#[test]
fn ac_c3_7_worker_survives_daemon_sigkill_and_restart_during_20_item_batch() {
    let mut worker_env = IsolatedWorkerEnv::new();
    worker_env.spawn_worker(&[("RADIVINE_WORKER_TEST_ITEM_SLEEP_MS", "60")], 60_000);

    let batch_dir = worker_env.root.join("batch_20");
    fs::create_dir_all(&batch_dir).unwrap();
    let mut uris = Vec::with_capacity(20);
    for i in 0..20 {
        let p = batch_dir.join(format!("item_{i:02}.png"));
        write_test_png(&p, 12, 12, i as u8);
        uris.push(local_path_to_file_uri(&p));
    }

    let config_path = worker_env.root.join("config.toml");
    fs::write(&config_path, "[appearance]\nreduced_motion = true\n").unwrap();
    let daemon_sock = worker_env.root.join("run/radivine.sock");

    let spawn_headless_daemon = || -> Child {
        let mut child = Command::new(radivine_bin())
            .env("RADIVINE_HEADLESS_DAEMON", "1")
            .env("RADIVINE_SUPPRESS_DESKTOP_NOTIFY", "1")
            .env("RADIVINE_DISABLE_SYSTEMD_USER_SPAWN", "1")
            .env("RADIVINE_WORKER_BIN", ensure_worker_bin())
            .env("RADIVINE_WORKER_SOCKET", &worker_env.paths.socket_path)
            .env("RADIVINE_WORKER_DB", &worker_env.paths.db_path)
            .env("XDG_RUNTIME_DIR", worker_env.root.join("run"))
            .env("XDG_STATE_HOME", worker_env.root.join("state"))
            .env("XDG_CONFIG_HOME", worker_env.root.join("config"))
            .arg("--config")
            .arg(&config_path)
            .arg("--socket")
            .arg(&daemon_sock)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn headless radivine daemon");

        let dl = Instant::now() + Duration::from_secs(3);
        while Instant::now() < dl {
            if daemon_sock.exists()
                && Command::new(radivine_bin())
                    .arg("--socket")
                    .arg(&daemon_sock)
                    .arg("status")
                    .output()
                    .is_ok_and(|o| o.status.success())
            {
                return child;
            }
            thread::sleep(Duration::from_millis(15));
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("headless radivine daemon did not start within 3s");
    };

    let mut daemon1 = spawn_headless_daemon();

    // Submit the 20-item batch via ConvertAction
    let convert_ctx = ConvertMenuContext::with_runtime_paths(
        Arc::new(CapabilityCatalog::default_builtin()),
        worker_env.paths.clone(),
    );
    let webp_act = Arc::new(ConvertAction::new(FormatId::WebP, convert_ctx));
    let ctx20 = Arc::new(Context::from_basic(BasicContext::from_uris(
        uris,
        InvocationMode::Drag,
        SourceInfo::default(),
    )));
    let (tx, rx) = std::sync::mpsc::channel();
    let mut dispatcher = Dispatcher::new(
        Box::new(move |ev| {
            let _ = tx.send(ev);
        }),
        1,
    );
    dispatcher.dispatch(webp_act, ctx20);
    match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
        JobEvent::Finished { result, .. } => assert_eq!(result.unwrap(), Outcome::Done),
    }

    // Wait 100ms so the 20-item batch is actively running in radivine-worker, then SIGKILL daemon1
    // and immediately start daemon2!
    thread::sleep(Duration::from_millis(100));
    let _ = daemon1.kill();
    let _ = daemon1.wait();
    let _ = fs::remove_file(&daemon_sock);

    let mut daemon2 = spawn_headless_daemon();

    // Verify radivine-worker finishes all 20 items cleanly despite daemon1 SIGKILL + daemon2 restart
    let (job, _) = worker_env.wait_for_latest_job_terminal(Duration::from_secs(15));
    assert_eq!(job.state, JobState::Completed);
    assert_eq!(job.counts.total_items, 20);
    assert_eq!(job.counts.succeeded_items, 20);
    for i in 0..20 {
        assert!(
            batch_dir.join(format!("item_{i:02}.webp")).exists(),
            "item_{i:02}.webp must exist after daemon restart"
        );
    }

    let _ = Command::new(radivine_bin())
        .arg("--socket")
        .arg(&daemon_sock)
        .arg("quit")
        .status();
    let _ = daemon2.wait();
}

#[test]
fn ac_c3_8_missing_worker_binary_reports_clear_error_notification_and_daemon_stays_healthy() {
    let tmp = tempfile::tempdir().unwrap();
    let src_png = tmp.path().join("photo.png");
    write_test_png(&src_png, 8, 8, 9);
    let src_uri = local_path_to_file_uri(&src_png);

    let broken_paths = WorkerRuntimePaths {
        socket_path: tmp.path().join("nonexistent_worker.sock"),
        lock_path: tmp.path().join("nonexistent_worker.lock"),
        db_path: tmp.path().join("nonexistent_jobs.sqlite3"),
        log_path: tmp.path().join("nonexistent_worker.log"),
        worker_bin: Some(PathBuf::from("/nonexistent/radivine-worker-missing-c3")),
        launch_mode: WorkerLaunchMode::Spawn,
    };
    let convert_ctx = ConvertMenuContext::with_runtime_paths(
        Arc::new(CapabilityCatalog::default_builtin()),
        broken_paths,
    );
    let reg = Arc::new(
        registry_from_options(
            &ActionsConfig::default(),
            &PreviewSettings::default(),
            &convert_ctx,
        )
        .unwrap(),
    );

    let (mock, _) = MockPlatform::new();
    let mut cfg = AppConfig::default();
    cfg.notifications.enabled = false;
    cfg.wheel.hover_dwell_ms = 10;
    let renderer = Renderer::new(cfg.resolve_theme(), AssetSource::Embedded).unwrap();
    let mut ctrl = SessionController::new(Box::new(mock), renderer, Arc::clone(&reg), cfg);
    ctrl.set_convert_menu_context(convert_ctx);

    let anchor = (960, 540);
    let path_pts =
        compute_menu_path_points(&reg, std::slice::from_ref(&src_uri), anchor, &[0, 0, 1]);

    ctrl.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: PlatSourceInfo::default(),
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(anchor.0, anchor.1),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".to_string()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".to_string(),
            result: Ok(format!("{src_uri}\r\n").into_bytes()),
        },
    ]);

    let enrich_dl = Instant::now() + Duration::from_secs(2);
    while Instant::now() < enrich_dl {
        ctrl.drain_async_events(Instant::now());
        if ctrl
            .status()
            .active_item_states
            .iter()
            .any(|s| s.id == "radivine.convert.image.webp" && s.enabled)
        {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    ctrl.handle_platform_events(vec![PlatformEvent::DndPosition { at: path_pts[0] }]);
    thread::sleep(Duration::from_millis(25));
    ctrl.on_timer(Instant::now());
    ctrl.handle_platform_events(vec![PlatformEvent::DndPosition { at: path_pts[1] }]);
    thread::sleep(Duration::from_millis(25));
    ctrl.on_timer(Instant::now());
    ctrl.handle_platform_events(vec![
        PlatformEvent::DndPosition { at: path_pts[2] },
        PlatformEvent::DndDrop,
    ]);

    let err_dl = Instant::now() + Duration::from_secs(2);
    while Instant::now() < err_dl {
        ctrl.drain_async_events(Instant::now());
        if ctrl.status().last_job_error.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    let st = ctrl.status();
    assert_eq!(
        st.last_job_action.as_deref(),
        Some("radivine.convert.image.webp")
    );
    let job_err = st.last_job_error.expect("expected last_job_error");
    assert!(
        job_err.contains("Couldn't start the conversion service"),
        "unexpected error message: {job_err}"
    );
    let notif = st.last_notification.expect("expected error notification");
    assert!(
        notif.contains("Action failed (radivine.convert.image.webp)")
            && notif.contains("Couldn't start the conversion service"),
        "unexpected notification: {notif}"
    );

    // Verify daemon remains healthy for subsequent sessions (e.g. Invoke + Copy)
    // For photo.png: [0] = Convert, [1] = Copy, [2] = Properties, [3] = Custom (Preview is hidden on image/*)
    let copy_pts = compute_menu_path_points(&reg, std::slice::from_ref(&src_uri), anchor, &[1]);
    ctrl.summon_invoke(vec![src_uri], Some(PhysPoint::new(anchor.0, anchor.1)))
        .unwrap();
    assert_eq!(ctrl.status().session_state, "Active");
    ctrl.handle_platform_events(vec![
        PlatformEvent::PointerMoved { at: copy_pts[0] },
        PlatformEvent::Button {
            button: 1,
            pressed: true,
            at: copy_pts[0],
        },
        PlatformEvent::Button {
            button: 1,
            pressed: false,
            at: copy_pts[0],
        },
    ]);
    assert_eq!(
        ctrl.status().last_committed_action.as_deref(),
        Some("radivine.copy")
    );
}

#[test]
fn ac_c3_10_and_c3_11_and_e8_non_utf8_invoke_cli_and_dependency_and_rss_budget() {
    let mut worker_env = IsolatedWorkerEnv::new();
    worker_env.spawn_worker(&[], 60_000);

    // 1. E8 & AC-C3.11: Non-UTF-8 filename via `radivine invoke` CLI -> headless daemon + worker conversion
    let non_utf8_dir = worker_env.root.join("non_utf8_invoke");
    fs::create_dir_all(&non_utf8_dir).unwrap();
    let raw_src_name = OsStr::from_bytes(b"clip_\x80\xff_c3.png");
    let raw_out_name = OsStr::from_bytes(b"clip_\x80\xff_c3.webp");
    let non_utf8_src = non_utf8_dir.join(raw_src_name);
    let non_utf8_out = non_utf8_dir.join(raw_out_name);
    write_test_png(&non_utf8_src, 14, 14, 88);

    let config_path = worker_env.root.join("config.toml");
    fs::write(&config_path, "[appearance]\nreduced_motion = true\n").unwrap();
    let daemon_sock = worker_env.root.join("run/radivine_e8.sock");

    let mut daemon = Command::new(radivine_bin())
        .env("RADIVINE_HEADLESS_DAEMON", "1")
        .env("RADIVINE_SUPPRESS_DESKTOP_NOTIFY", "1")
        .env("RADIVINE_DISABLE_SYSTEMD_USER_SPAWN", "1")
        .env("RADIVINE_WORKER_BIN", ensure_worker_bin())
        .env("RADIVINE_WORKER_SOCKET", &worker_env.paths.socket_path)
        .env("RADIVINE_WORKER_DB", &worker_env.paths.db_path)
        .env("XDG_RUNTIME_DIR", worker_env.root.join("run"))
        .env("XDG_STATE_HOME", worker_env.root.join("state"))
        .env("XDG_CONFIG_HOME", worker_env.root.join("config"))
        .arg("--config")
        .arg(&config_path)
        .arg("--socket")
        .arg(&daemon_sock)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn headless daemon");

    let dl = Instant::now() + Duration::from_secs(3);
    while Instant::now() < dl {
        if daemon_sock.exists()
            && Command::new(radivine_bin())
                .arg("--socket")
                .arg(&daemon_sock)
                .arg("status")
                .output()
                .is_ok_and(|o| o.status.success())
        {
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }

    // Invoke `radivine invoke` passing the raw non-UTF-8 OsStr path directly in argv!
    let invoke_out = Command::new(radivine_bin())
        .arg("--socket")
        .arg(&daemon_sock)
        .arg("invoke")
        .arg("--at")
        .arg("600,450")
        .arg(&non_utf8_src)
        .output()
        .expect("run radivine invoke with non-UTF-8 path");
    assert!(
        invoke_out.status.success(),
        "radivine invoke failed on non-UTF-8 path: {}",
        String::from_utf8_lossy(&invoke_out.stderr)
    );

    // Poll daemon status until enrichment enables `radivine.convert.image.webp` for the non-UTF-8 file
    let mut webp_enabled = false;
    let mut idle_rss_kib = None;
    let enrich_dl = Instant::now() + Duration::from_secs(3);
    while Instant::now() < enrich_dl {
        let st_out = Command::new(radivine_bin())
            .arg("--socket")
            .arg(&daemon_sock)
            .args(["status", "--json"])
            .output()
            .unwrap();
        let st_json: serde_json::Value = serde_json::from_slice(&st_out.stdout).unwrap();
        idle_rss_kib = st_json["rss_kib"].as_u64();
        if let Some(arr) = st_json["active_item_states"].as_array()
            && arr.iter().any(|item| {
                item["id"].as_str() == Some("radivine.convert.image.webp")
                    && item["enabled"].as_bool() == Some(true)
            })
        {
            webp_enabled = true;
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }
    assert!(
        webp_enabled,
        "non-UTF-8 image path invoked via CLI must be enriched and enable radivine.convert.image.webp"
    );

    let _ = Command::new(radivine_bin())
        .arg("--socket")
        .arg(&daemon_sock)
        .arg("quit")
        .status();
    let _ = daemon.wait();

    // Also run ConvertAction on the non-UTF-8 URI produced by `local_path_to_file_uri` and verify output on disk
    let convert_ctx = ConvertMenuContext::with_runtime_paths(
        Arc::new(CapabilityCatalog::default_builtin()),
        worker_env.paths.clone(),
    );
    let webp_act = Arc::new(ConvertAction::new(FormatId::WebP, convert_ctx));
    let non_utf8_uri = local_path_to_file_uri(&non_utf8_src);
    assert!(non_utf8_uri.contains("%80%FF"));
    let non_utf8_ctx = Arc::new(radivine_core::enrich_context_sync(BasicContext::from_uris(
        [&non_utf8_uri],
        InvocationMode::Invoke,
        SourceInfo::default(),
    )));
    assert_eq!(webp_act.enablement(&non_utf8_ctx), Enablement::Enabled);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut dispatcher = Dispatcher::new(
        Box::new(move |ev| {
            let _ = tx.send(ev);
        }),
        1,
    );
    dispatcher.dispatch(webp_act, non_utf8_ctx);
    match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
        JobEvent::Finished { result, .. } => assert_eq!(result.unwrap(), Outcome::Done),
    }
    let (nu_job, _) = worker_env.wait_for_latest_job_terminal(Duration::from_secs(10));
    assert_eq!(nu_job.state, JobState::Completed);
    assert!(
        non_utf8_out.exists(),
        "converted non-UTF-8 file {:?} must exist on disk",
        non_utf8_out
    );

    // 2. AC-C3.10: Daemon RSS & Dependency Graph Isolation check (`cargo tree`)
    if let Some(rss) = idle_rss_kib {
        eprintln!(
            "[C3 PERF] Headless daemon RSS with Convert actions registered: {rss} KiB ({:.2} MiB)",
            rss as f64 / 1024.0
        );
        assert!(
            rss < 50_000,
            "daemon RSS ({rss} KiB) must stay well within the < 50 MB budget"
        );
    }

    let forbidden_in_daemon = [
        "radivine-convert-image",
        "├── image v",
        "└── image v",
        "├── webp v",
        "└── webp v",
        "libwebp-sys2",
        "rusqlite",
    ];
    for pkg in ["radivine", "radivine-actions"] {
        let out = Command::new("cargo")
            .args(["tree", "-p", pkg, "-e", "normal"])
            .output()
            .expect("run cargo tree");
        assert!(out.status.success());
        let tree_str = String::from_utf8_lossy(&out.stdout);
        for bad in forbidden_in_daemon {
            assert!(
                !tree_str.contains(bad),
                "Package {pkg} must NOT depend on {bad} (AC-C3.10)!\ncargo tree:\n{tree_str}"
            );
        }
    }
}
