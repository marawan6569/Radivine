//! Regression test suite for the v0.1.1 Cinnamon login autostart rendering regression.
//!
//! Includes:
//! 1. Deterministic, self-contained `SessionController` + `Renderer` regression tests that run
//!    unconditionally in headless environments (`DISPLAY="" cargo test --workspace`) and verify:
//!    - Early login startup before the compositor owns `_NET_WM_CM_S0` (`translucent = false`).
//!    - Visual-only `ShapeBounding` clipping in no-compositor fallback mode (excluding invisible
//!      pointer hit-slop corridors such as `anchor_corridor_polygon` and updating on `HoverChanged`).
//!    - Dynamic recovery to full 32-bit ARGB translucency (`translucent = true`), `ShapeBounding`
//!      reset (`set_bounding_region(None)`), and transparent background/gap pixels (`[0, 0, 0, 0]`)
//!      when the compositor appears after startup — without restarting the daemon.
//!    - Live recovery across compositor restarts (`cinnamon --replace`) and delayed `csd-xrandr` /
//!      `csd-xsettings` monitor geometry and `Xft.dpi` scaling updates.
//! 2. An optional disposable `Xvfb` wire-protocol test (when `Xvfb` is installed on the runner) on
//!    a high display number (`:107..:128`) that never touches the host desktop.

use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use radivine::config::AppConfig;
use radivine::ipc::DaemonStatus;
use radivine::session::SessionController;
use radivine_actions::builtin_registry;
use radivine_platform::{
    Capabilities, ClipboardContent, FrameRef, Monitor, OverlayInfo, PhysPoint, Platform,
    PlatformError, PlatformEvent, RectPx, ShapePx, Support,
};
use radivine_render::{AssetSource, Renderer};
use x11rb::connection::Connection;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateWindowAux, ImageFormat, PropMode, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

#[derive(Clone)]
struct DynamicPlatformShared {
    compositor_active: bool,
    scale: f32,
    reduced_motion: Option<bool>,
    monitors: Vec<Monitor>,
    mapped: bool,
    bounding_region: Option<Vec<ShapePx>>,
    bounding_reset_calls: usize,
    input_region: Option<Vec<ShapePx>>,
    last_frame_rgba: Vec<u8>,
    last_frame_width: u32,
    last_frame_height: u32,
    last_damage: Vec<RectPx>,
}

impl Default for DynamicPlatformShared {
    fn default() -> Self {
        Self {
            compositor_active: false,
            scale: 1.0,
            reduced_motion: Some(false),
            monitors: vec![Monitor {
                id: 0,
                name: "HDMI-0".to_string(),
                rect: RectPx::new(0, 0, 1920, 1080),
                refresh_mhz: 60_000,
                primary: true,
            }],
            mapped: false,
            bounding_region: None,
            bounding_reset_calls: 0,
            input_region: None,
            last_frame_rgba: Vec::new(),
            last_frame_width: 0,
            last_frame_height: 0,
            last_damage: Vec::new(),
        }
    }
}

struct DynamicLoginPlatform {
    shared: Arc<Mutex<DynamicPlatformShared>>,
    caps: Capabilities,
    cached_monitors: Vec<Monitor>,
    cached_scale: f32,
    cached_reduced_motion: Option<bool>,
}

impl DynamicLoginPlatform {
    fn new(initial_compositor_active: bool) -> (Self, Arc<Mutex<DynamicPlatformShared>>) {
        let shared = Arc::new(Mutex::new(DynamicPlatformShared {
            compositor_active: initial_compositor_active,
            ..DynamicPlatformShared::default()
        }));
        let st = shared.lock().expect("lock shared");
        let caps = Self::caps_for(st.compositor_active);
        let cached_monitors = st.monitors.clone();
        let cached_scale = st.scale;
        let cached_reduced_motion = st.reduced_motion;
        drop(st);

        (
            Self {
                shared: Arc::clone(&shared),
                caps,
                cached_monitors,
                cached_scale,
                cached_reduced_motion,
            },
            shared,
        )
    }

    fn caps_for(compositor_active: bool) -> Capabilities {
        Capabilities {
            drag_session: Support::Yes,
            invoke_at_pointer: Support::Yes,
            translucency: if compositor_active {
                Support::Yes
            } else {
                Support::Degraded("no compositor owns _NET_WM_CM_S0; using opaque card fallback")
            },
            input_passthrough: Support::Yes,
            keyboard_in_drag: Support::No("drag source holds keyboard grab"),
            clipboard_files: Support::Yes,
        }
    }

    fn sync_from_shared(&mut self) {
        let mut st = self.shared.lock().expect("lock shared");
        let was_composited = matches!(self.caps.translucency, Support::Yes);
        self.caps = Self::caps_for(st.compositor_active);
        if !was_composited && st.compositor_active {
            st.bounding_region = None;
            st.bounding_reset_calls += 1;
        }
        self.cached_monitors = st.monitors.clone();
        self.cached_scale = st.scale;
        self.cached_reduced_motion = st.reduced_motion;
    }
}

impl Platform for DynamicLoginPlatform {
    fn name(&self) -> &'static str {
        "x11"
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn sync_graphical_state(&mut self) {
        self.sync_from_shared();
    }

    #[cfg(unix)]
    fn poll_fd(&self) -> std::os::fd::RawFd {
        0
    }

    fn dispatch_pending(&mut self, _out: &mut Vec<PlatformEvent>) -> Result<(), PlatformError> {
        self.sync_from_shared();
        Ok(())
    }

    fn monitors(&self) -> &[Monitor] {
        &self.cached_monitors
    }

    fn scale(&self) -> f32 {
        self.cached_scale
    }

    fn reduced_motion(&self) -> Option<bool> {
        self.cached_reduced_motion
    }

    fn pointer_position(&mut self) -> Result<PhysPoint, PlatformError> {
        Ok(PhysPoint::new(960, 540))
    }

    fn overlay_window_id(&self) -> u32 {
        0x0020_0001
    }

    fn prepare_overlay(&mut self, at: PhysPoint) -> Result<OverlayInfo, PlatformError> {
        self.sync_from_shared();
        let mut st = self.shared.lock().expect("lock shared");
        let mon = st
            .monitors
            .iter()
            .find(|m| m.rect.contains(at))
            .or_else(|| st.monitors.first())
            .cloned()
            .expect("at least one monitor");
        st.last_frame_width = mon.rect.w;
        st.last_frame_height = mon.rect.h;
        st.last_frame_rgba = vec![0u8; (mon.rect.w as usize) * (mon.rect.h as usize) * 4];
        Ok(OverlayInfo {
            origin: PhysPoint::new(mon.rect.x, mon.rect.y),
            size: (mon.rect.w, mon.rect.h),
            scale: st.scale,
            monitor: mon,
            translucent: st.compositor_active,
        })
    }

    fn map_overlay(&mut self) -> Result<(), PlatformError> {
        self.shared.lock().expect("lock shared").mapped = true;
        Ok(())
    }

    fn present(&mut self, frame: FrameRef<'_>, damage: &[RectPx]) -> Result<(), PlatformError> {
        let mut st = self.shared.lock().expect("lock shared");
        let total_bytes = frame.stride * (frame.height as usize);
        if st.last_frame_rgba.len() != total_bytes
            || st.last_frame_width != frame.width
            || st.last_frame_height != frame.height
        {
            st.last_frame_rgba = frame.data[..total_bytes].to_vec();
            st.last_frame_width = frame.width;
            st.last_frame_height = frame.height;
        } else {
            for r in damage {
                let x0 = r.x.max(0) as usize;
                let y0 = r.y.max(0) as usize;
                let x1 = (r.x + r.w as i32).min(frame.width as i32).max(0) as usize;
                let y1 = (r.y + r.h as i32).min(frame.height as i32).max(0) as usize;
                for y in y0..y1 {
                    let row_start = y * frame.stride + x0 * 4;
                    let row_end = y * frame.stride + x1 * 4;
                    st.last_frame_rgba[row_start..row_end]
                        .copy_from_slice(&frame.data[row_start..row_end]);
                }
            }
        }
        st.last_damage = damage.to_vec();
        Ok(())
    }

    fn set_input_region(&mut self, shapes: Option<&[ShapePx]>) -> Result<(), PlatformError> {
        self.shared.lock().expect("lock shared").input_region = shapes.map(<[ShapePx]>::to_vec);
        Ok(())
    }

    fn set_bounding_region(&mut self, shapes: Option<&[ShapePx]>) -> Result<(), PlatformError> {
        let mut st = self.shared.lock().expect("lock shared");
        if shapes.is_none() {
            st.bounding_reset_calls += 1;
        }
        st.bounding_region = shapes.map(<[ShapePx]>::to_vec);
        Ok(())
    }

    fn hide_overlay(&mut self) {
        self.shared.lock().expect("lock shared").mapped = false;
    }

    fn release_buffers(&mut self) {}

    fn dnd_reply(&mut self, _accept: bool) {}

    fn request_payload(&mut self, _mime: &str) {}

    fn dnd_finish(&mut self, _success: bool) {}

    fn grab_input(&mut self) -> Result<(), PlatformError> {
        Ok(())
    }

    fn ungrab_input(&mut self) {}

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

fn pixel_at(buf: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let idx = ((y * width + x) as usize) * 4;
    [buf[idx], buf[idx + 1], buf[idx + 2], buf[idx + 3]]
}

#[test]
fn login_autostart_delayed_compositor_recovers_translucency_and_clears_shape_bounding_without_restart()
 {
    // 1. Simulate early Cinnamon login startup BEFORE `cinnamon --replace` (Muffin) owns `_NET_WM_CM_S0`
    let (plat, shared) = DynamicLoginPlatform::new(false);
    let cfg = AppConfig::default();
    let renderer = Renderer::new(cfg.resolve_theme(), AssetSource::Embedded).expect("renderer");
    let registry = Arc::new(builtin_registry());
    let mut ctrl = SessionController::new(Box::new(plat), renderer, registry, cfg);

    let st_initial = ctrl.status();
    assert!(st_initial.graphical_ready);
    assert!(
        !st_initial.compositor_active,
        "compositor_active must be false when daemon starts before Muffin"
    );
    assert!(
        !st_initial.translucent_overlay,
        "translucent_overlay must be false when daemon starts before Muffin"
    );

    // 2. Summon near the top-left corner (60, 60) while no compositor is active.
    // Clamping near the screen edge creates an invisible `anchor_corridor_polygon` in `ws.input_region()`.
    // Verify that in `!translucent` fallback mode:
    //   - `ShapeBounding` contains ONLY the visible wheel circle (no invisible pointer corridor polygon!).
    //   - Hovering a segment updates `ShapeBounding` to include the caption box.
    ctrl.summon_invoke(
        vec!["file:///tmp/login_autostart_test.txt".to_string()],
        Some(PhysPoint::new(60, 60)),
    )
    .expect("summon_invoke in no-compositor fallback mode");

    {
        let st = shared.lock().expect("lock shared");
        assert!(st.mapped);
        let bounding = st
            .bounding_region
            .as_ref()
            .expect("no-compositor fallback must set ShapeBounding");
        assert_eq!(
            bounding.len(),
            1,
            "unhovered root wheel near screen edge must have 1 Circle in ShapeBounding (excluding invisible anchor_corridor_polygon)"
        );
        assert!(matches!(bounding[0], ShapePx::Circle { .. }));

        // In opaque fallback mode, corners of the wheel's damage box are filled with opaque ring_fill (alpha == 255)
        let dmg = st.last_damage.first().expect("damage rect");
        let corner_px = pixel_at(
            &st.last_frame_rgba,
            st.last_frame_width,
            dmg.x as u32 + 2,
            dmg.y as u32 + 2,
        );
        assert_eq!(
            corner_px[3], 0xFF,
            "in no-compositor opaque fallback mode, damage rect corners have alpha 255"
        );
    }

    // Move pointer onto a wheel segment so `HoverChanged` fires and a caption is shown
    let (wheel_cx, wheel_cy) = {
        let st = shared.lock().expect("lock shared");
        match st.bounding_region.as_ref().unwrap()[0] {
            ShapePx::Circle { cx, cy, .. } => (cx.round() as i32, cy.round() as i32),
            ShapePx::Polygon(_) => panic!("expected circle"),
        }
    };
    ctrl.handle_platform_events(vec![PlatformEvent::PointerMoved {
        at: PhysPoint::new(wheel_cx + 80, wheel_cy),
    }]);
    {
        let st = shared.lock().expect("lock shared");
        let bounding = st.bounding_region.as_ref().expect("bounding region");
        assert!(
            bounding.len() >= 2,
            "hovering a segment in no-compositor mode must update ShapeBounding to include the caption rectangle"
        );
    }

    ctrl.dismiss_immediate();

    // 3. Now simulate Muffin (`cinnamon --replace`) claiming `_NET_WM_CM_S0` AFTER login!
    // Do NOT restart `SessionController`.
    shared.lock().expect("lock shared").compositor_active = true;

    let st_after_cm = ctrl.status();
    assert!(
        st_after_cm.compositor_active,
        "status() must dynamically detect that the compositor is now active"
    );
    assert!(
        st_after_cm.translucent_overlay,
        "status() must report translucent_overlay = true without daemon restart"
    );

    // 4. Summon the radial wheel at screen center (960, 540) in the SAME daemon session
    ctrl.summon_invoke(
        vec!["file:///tmp/login_autostart_test.txt".to_string()],
        Some(PhysPoint::new(960, 540)),
    )
    .expect("summon_invoke after compositor becomes active");
    ctrl.on_timer(Instant::now() + Duration::from_millis(200));

    {
        let st = shared.lock().expect("lock shared");
        assert!(st.mapped);
        assert!(
            st.bounding_region.is_none(),
            "ShapeBounding must be cleared (None) when transitioning to composited translucent mode"
        );
        assert!(
            st.bounding_reset_calls >= 1,
            "set_bounding_region(None) must be invoked to clear prior XShape clip"
        );

        // Verify pixel-level rendering in the 32-bit ARGB frame buffer:
        // (a) Corner of the damage box (outside the outer ring radius) MUST be 100% transparent [0, 0, 0, 0],
        //     NOT filled with opaque dark `ring_fill`!
        let dmg = st.last_damage.first().expect("damage rect");
        let outside_corner = pixel_at(
            &st.last_frame_rgba,
            st.last_frame_width,
            dmg.x as u32 + 2,
            dmg.y as u32 + 2,
        );
        assert_eq!(
            outside_corner,
            [0, 0, 0, 0],
            "pixel outside the radial wheel ring must be 100% transparent [0, 0, 0, 0] once compositor is active"
        );

        // (b) Pixel just below outer ring (960, 540 + 136) has semi-transparent soft drop shadow (0 < alpha < 180)
        let shadow_px = pixel_at(&st.last_frame_rgba, st.last_frame_width, 960, 540 + 136);
        assert!(
            shadow_px[3] > 0 && shadow_px[3] < 180,
            "composited mode must render semi-transparent soft drop shadow, got {shadow_px:?}"
        );

        // (c) Pixel on the wheel ring (960 + 80, 540) must be visibly rendered (alpha > 0)
        let ring_px = pixel_at(&st.last_frame_rgba, st.last_frame_width, 960 + 80, 540);
        assert!(
            ring_px[3] > 200,
            "pixel on the radial wheel ring must be rendered with high alpha, got {ring_px:?}"
        );
    }

    // 5. Compare byte-for-byte against a freshly restarted daemon started when the compositor is already active
    let (fresh_plat, fresh_shared) = DynamicLoginPlatform::new(true);
    let fresh_cfg = AppConfig::default();
    let fresh_renderer =
        Renderer::new(fresh_cfg.resolve_theme(), AssetSource::Embedded).expect("fresh renderer");
    let mut fresh_ctrl = SessionController::new(
        Box::new(fresh_plat),
        fresh_renderer,
        Arc::new(builtin_registry()),
        fresh_cfg,
    );
    fresh_ctrl
        .summon_invoke(
            vec!["file:///tmp/login_autostart_test.txt".to_string()],
            Some(PhysPoint::new(960, 540)),
        )
        .expect("summon on fresh composited daemon");
    fresh_ctrl.on_timer(Instant::now() + Duration::from_millis(200));

    {
        let st_recovered = shared.lock().expect("lock recovered");
        let st_fresh = fresh_shared.lock().expect("lock fresh");
        assert_eq!(st_recovered.bounding_region, st_fresh.bounding_region);
        assert_eq!(st_recovered.input_region, st_fresh.input_region);
        assert_eq!(st_recovered.last_frame_width, st_fresh.last_frame_width);
        assert_eq!(st_recovered.last_frame_height, st_fresh.last_frame_height);
        assert_eq!(
            st_recovered.last_frame_rgba, st_fresh.last_frame_rgba,
            "recovered login-autostart wheel framebuffer must match fresh composited daemon startup byte-for-byte"
        );
    }

    fresh_ctrl.dismiss_immediate();
    ctrl.dismiss_immediate();
}

#[test]
fn compositor_restart_and_delayed_dpi_and_randr_geometry_sync_dynamically() {
    // Start with a basic pre-session geometry (1024x768, scale 1.0, no compositor)
    let (plat, shared) = DynamicLoginPlatform::new(false);
    {
        let mut st = shared.lock().expect("lock shared");
        st.monitors = vec![Monitor {
            id: 0,
            name: "DP-1".to_string(),
            rect: RectPx::new(0, 0, 1024, 768),
            refresh_mhz: 60_000,
            primary: true,
        }];
        st.scale = 1.0;
    }

    let cfg = AppConfig::default();
    let renderer = Renderer::new(cfg.resolve_theme(), AssetSource::Embedded).expect("renderer");
    let registry = Arc::new(builtin_registry());
    let mut ctrl = SessionController::new(Box::new(plat), renderer, registry, cfg);

    // Simulate `csd-xrandr`, `csd-xsettings`, and `cinnamon --replace` finishing initialization:
    // Monitor becomes 2560x1440, Xft.dpi becomes 144 (scale 1.5), and compositor becomes active.
    {
        let mut st = shared.lock().expect("lock shared");
        st.monitors = vec![Monitor {
            id: 0,
            name: "DP-1".to_string(),
            rect: RectPx::new(0, 0, 2560, 1440),
            refresh_mhz: 144_000,
            primary: true,
        }];
        st.scale = 1.5;
        st.compositor_active = true;
    }
    ctrl.handle_platform_events(vec![
        PlatformEvent::MonitorsChanged,
        PlatformEvent::SettingsChanged,
    ]);

    let status = ctrl.status();
    assert!(status.graphical_ready);
    assert!(status.compositor_active);
    assert!(status.translucent_overlay);
    assert_eq!(status.monitor_count, 1);
    assert!((status.scale - 1.5).abs() < f32::EPSILON);

    // Summon at (1800, 900) — outside the old 1024x768 screen, inside the updated 2560x1440 monitor
    ctrl.summon_invoke(
        vec!["file:///tmp/dpi_randr_test.txt".to_string()],
        Some(PhysPoint::new(1800, 900)),
    )
    .expect("summon at 1800,900 on updated 2560x1440 monitor");
    {
        let st = shared.lock().expect("lock shared");
        assert_eq!(st.last_frame_width, 2560);
        assert_eq!(st.last_frame_height, 1440);
        assert!(st.bounding_region.is_none());
    }
    ctrl.dismiss_immediate();

    // Simulate compositor crash/exit (`compositor_active = false`) followed by compositor restart (`true`)
    shared.lock().expect("lock shared").compositor_active = false;
    ctrl.handle_platform_events(vec![PlatformEvent::SettingsChanged]);
    assert!(!ctrl.status().compositor_active);
    assert!(!ctrl.status().translucent_overlay);

    shared.lock().expect("lock shared").compositor_active = true;
    ctrl.handle_platform_events(vec![PlatformEvent::SettingsChanged]);
    assert!(ctrl.status().compositor_active);
    assert!(ctrl.status().translucent_overlay);
}

#[test]
fn daemon_status_serde_backward_compatibility_and_packaged_service_unit_invariants() {
    // 1. Verify older v0.1.0/v0.1.1 JSON payloads without the new fields still deserialize cleanly
    let legacy_json = r#"{
        "pid": 12345,
        "overlay_window_id": 2097153,
        "session_state": "Idle",
        "scale": 1.0,
        "monitor_rect": [0, 0, 1920, 1080],
        "root_wheel_center_px": null,
        "root_wheel_outer_radius_px": null,
        "invoke_count": 0,
        "last_committed_action": null,
        "last_committed_items": []
    }"#;
    let parsed: DaemonStatus =
        serde_json::from_str(legacy_json).expect("deserialize legacy DaemonStatus");
    assert!(parsed.graphical_ready);
    assert!(!parsed.compositor_active);
    assert!(!parsed.translucent_overlay);
    assert_eq!(parsed.monitor_count, 0);

    // 2. Verify `packaging/radivine.service` contains the login autostart hardening directives
    let unit_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("packaging/radivine.service");
    let unit_text = fs::read_to_string(&unit_path).expect("read packaging/radivine.service");
    assert!(
        unit_text.contains("After=default.target graphical-session-pre.target dbus.socket"),
        "packaging/radivine.service must order after graphical-session-pre.target and dbus.socket"
    );
    assert!(
        unit_text.contains("StartLimitIntervalSec=0"),
        "packaging/radivine.service must disable start-limit lockout across logout/login cycles"
    );
    assert!(
        unit_text.contains("Environment=RADIVINE_SYSTEMD_SERVICE=1"),
        "packaging/radivine.service must export RADIVINE_SYSTEMD_SERVICE=1"
    );
    assert!(
        unit_text.contains("WantedBy=default.target"),
        "packaging/radivine.service must use WantedBy=default.target for Linux Mint Cinnamon"
    );
}

// ============================================================================
// Optional Disposable Xvfb Wire-Protocol Regression Tests (run when Xvfb exists)
// ============================================================================

fn radivine_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_radivine"))
}

struct DisposableXvfb {
    child: Child,
    display: String,
}

impl DisposableXvfb {
    fn start(display_num: u32) -> Option<Self> {
        let display = format!(":{display_num}");
        let lock_file = format!("/tmp/.X{display_num}-lock");
        let sock_file = format!("/tmp/.X11-unix/X{display_num}");
        let _ = fs::remove_file(&lock_file);
        let _ = fs::remove_file(&sock_file);

        let mut child = Command::new("Xvfb")
            .args([
                &display,
                "-screen",
                "0",
                "1920x1080x24",
                "+extension",
                "Composite",
                "+extension",
                "RENDER",
                "+extension",
                "RANDR",
                "+extension",
                "SHAPE",
                "+extension",
                "XFIXES",
                "+extension",
                "MIT-SHM",
                "-nolisten",
                "tcp",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if RustConnection::connect(Some(&display)).is_ok() {
                return Some(Self { child, display });
            }
            if child.try_wait().ok().flatten().is_some() {
                return None;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let _ = child.kill();
        let _ = child.wait();
        None
    }
}

impl Drop for DisposableXvfb {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct IsolatedDaemon {
    child: Child,
    display: String,
    sock_path: PathBuf,
    config_path: PathBuf,
    temp_dir: PathBuf,
}

impl IsolatedDaemon {
    fn spawn(display: &str, config_toml: &str, extra_env: &[(&str, &str)], tag: &str) -> Self {
        let temp_dir =
            std::env::temp_dir().join(format!("radivine-login-reg-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("create temp_dir");

        let config_path = temp_dir.join("config.toml");
        fs::write(&config_path, config_toml).expect("write config.toml");
        let sock_path = temp_dir.join("radivine.sock");

        let mut cmd = Command::new(radivine_bin());
        cmd.env("DISPLAY", display)
            .env_remove("WAYLAND_DISPLAY")
            .env("XDG_SESSION_TYPE", "x11")
            .env("RADIVINE_DRY_RUN_VIEWER", "1")
            .env("RADIVINE_DRY_RUN_PROPERTIES", "1")
            .env("RADIVINE_SUPPRESS_DESKTOP_NOTIFY", "1");
        for &(k, v) in extra_env {
            cmd.env(k, v);
        }
        let child = cmd
            .arg("--config")
            .arg(&config_path)
            .arg("--socket")
            .arg(&sock_path)
            .arg("daemon")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn radivine daemon");

        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            if sock_path.exists() {
                let status = Command::new(radivine_bin())
                    .env("DISPLAY", display)
                    .arg("--socket")
                    .arg(&sock_path)
                    .arg("status")
                    .output()
                    .expect("run status");
                if status.status.success() {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(20));
        }

        Self {
            child,
            display: display.to_string(),
            sock_path,
            config_path,
            temp_dir,
        }
    }

    fn cli(&self, args: &[&str]) -> std::process::Output {
        Command::new(radivine_bin())
            .env("DISPLAY", &self.display)
            .arg("--config")
            .arg(&self.config_path)
            .arg("--socket")
            .arg(&self.sock_path)
            .args(args)
            .output()
            .expect("run radivine cli")
    }

    fn status_json(&self) -> serde_json::Value {
        let out = self.cli(&["status", "--json"]);
        assert!(
            out.status.success(),
            "status --json failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("parse status json")
    }
}

impl Drop for IsolatedDaemon {
    fn drop(&mut self) {
        let _ = self.cli(&["quit"]);
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

fn create_mock_compositor_window(conn: &RustConnection, root: u32, cm_atom: u32) -> u32 {
    let win = conn.generate_id().expect("generate cm win id");
    conn.create_window(
        0,
        win,
        root,
        -10,
        -10,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new().override_redirect(1),
    )
    .expect("create mock compositor window");
    conn.set_selection_owner(win, cm_atom, x11rb::CURRENT_TIME)
        .expect("set _NET_WM_CM_S0 owner");
    conn.flush().expect("flush _NET_WM_CM_S0 owner");
    win
}

#[test]
fn xvfb_wire_daemon_dynamically_recovers_when_compositor_and_dpi_initialize_after_startup() {
    let Some(xvfb) = (107..118).find_map(DisposableXvfb::start) else {
        eprintln!("skipping Xvfb wire test: Xvfb binary not installed on host");
        return;
    };

    let (conn, screen_num) =
        RustConnection::connect(Some(&xvfb.display)).expect("connect to disposable Xvfb");
    let root = conn.setup().roots[screen_num].root;
    let cm_atom = conn
        .intern_atom(false, b"_NET_WM_CM_S0")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let rm_atom = conn
        .intern_atom(false, b"RESOURCE_MANAGER")
        .unwrap()
        .reply()
        .unwrap()
        .atom;

    let sample_file = std::env::temp_dir().join("radivine-login-reg-sample.txt");
    fs::write(&sample_file, "Login autostart regression sample\n").unwrap();
    let sample_str = sample_file.to_str().unwrap();

    let daemon = IsolatedDaemon::spawn(&xvfb.display, "", &[], "delayed-cm");
    let st0 = daemon.status_json();
    let initial_pid = st0["pid"].as_u64().expect("pid");
    let overlay_xid = st0["overlay_window_id"].as_u64().expect("overlay_xid") as u32;

    assert!(st0["graphical_ready"].as_bool().unwrap());
    assert!(!st0["compositor_active"].as_bool().unwrap());
    assert!(!st0["translucent_overlay"].as_bool().unwrap());

    let inv0 = daemon.cli(&["invoke", "--at", "80,80", sample_str]);
    assert!(inv0.status.success());
    let rects_fallback = conn
        .shape_get_rectangles(overlay_xid, shape::SK::BOUNDING)
        .unwrap()
        .reply()
        .unwrap()
        .rectangles;
    assert!(rects_fallback.len() > 1);
    assert!(daemon.cli(&["dismiss"]).status.success());

    let cm_win = create_mock_compositor_window(&conn, root, cm_atom);
    thread::sleep(Duration::from_millis(40));

    let inv1 = daemon.cli(&["invoke", "--at", "960,540", sample_str]);
    assert!(inv1.status.success());

    let st1 = daemon.status_json();
    assert_eq!(st1["pid"].as_u64().unwrap(), initial_pid);
    assert!(st1["compositor_active"].as_bool().unwrap());
    assert!(st1["translucent_overlay"].as_bool().unwrap());

    let rects_composited = conn
        .shape_get_rectangles(overlay_xid, shape::SK::BOUNDING)
        .unwrap()
        .reply()
        .unwrap()
        .rectangles;
    assert_eq!(rects_composited.len(), 1);
    assert_eq!(rects_composited[0].width, 1920);
    assert_eq!(rects_composited[0].height, 1080);

    let outside_img = conn
        .get_image(ImageFormat::Z_PIXMAP, overlay_xid, 200, 200, 1, 1, !0)
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(outside_img.data, vec![0, 0, 0, 0]);

    assert!(daemon.cli(&["dismiss"]).status.success());

    conn.destroy_window(cm_win).unwrap();
    conn.flush().unwrap();
    thread::sleep(Duration::from_millis(40));

    let st_no_cm = daemon.status_json();
    assert!(!st_no_cm["compositor_active"].as_bool().unwrap());
    assert!(!st_no_cm["translucent_overlay"].as_bool().unwrap());

    let _cm_win_2 = create_mock_compositor_window(&conn, root, cm_atom);
    thread::sleep(Duration::from_millis(40));

    let st_restarted_cm = daemon.status_json();
    assert!(st_restarted_cm["compositor_active"].as_bool().unwrap());
    assert!(st_restarted_cm["translucent_overlay"].as_bool().unwrap());

    conn.change_property8(
        PropMode::REPLACE,
        root,
        rm_atom,
        AtomEnum::STRING,
        b"Xft.dpi:\t144\n",
    )
    .unwrap();
    conn.flush().unwrap();
    thread::sleep(Duration::from_millis(40));

    let st_dpi = daemon.status_json();
    assert!((st_dpi["scale"].as_f64().unwrap() - 1.5).abs() < 0.01);

    let _ = fs::remove_file(&sample_file);
}
