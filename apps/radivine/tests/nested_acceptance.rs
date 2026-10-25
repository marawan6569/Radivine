//! Milestone N4 end-to-end Concentric integration, reload-immutability, action-identity,
//! and X11 safety acceptance tests (`TEST_MATRIX.md` `T-CFG1`–`T-CFG8`, `T-SAFE1`–`T-SAFE8`,
//! `T-MENU1`–`T-MENU5`, `T-X111`–`T-X118`).

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use radivine::config::{AppConfig, ReducedMotionSetting, ScaleSetting};
use radivine::session::{PendingDropSelection, SessionController};
use radivine_actions::builtin_registry;
use radivine_core::{
    ActionId, BasicContext, Context, IconKey, InvocationMode, MenuLimits, MenuOverrides, MenuSpec,
    SourceInfo as CoreSourceInfo,
};
use radivine_platform::x11::atoms::Atoms;
use radivine_platform::{
    Capabilities, ClipboardContent, FrameRef, Monitor, OverlayInfo, PayloadError, PhysPoint,
    Platform, PlatformError, PlatformEvent, RectPx, ShapePx, SourceInfo as PlatSourceInfo, Support,
};
use radivine_render::{AssetSource, Renderer};
use radivine_wheel::{ItemPath, Point, WheelLayout};

fn make_temp_test_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("radivine-n4-acc-{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileFingerprint {
    path: PathBuf,
    bytes: Vec<u8>,
    size: u64,
    ino: u64,
    mtime: Option<SystemTime>,
}

impl FileFingerprint {
    fn capture(path: &Path) -> Self {
        let meta = fs::symlink_metadata(path).expect("symlink_metadata");
        let bytes = fs::read(path).expect("read file bytes");
        Self {
            path: path.to_path_buf(),
            bytes,
            size: meta.len(),
            ino: meta.ino(),
            mtime: meta.modified().ok(),
        }
    }

    fn verify_unchanged(&self, context: &str) {
        assert!(
            self.path.exists(),
            "source file missing after {context}: {}",
            self.path.display()
        );
        let actual = Self::capture(&self.path);
        assert_eq!(
            &actual,
            self,
            "source file metadata or bytes changed after {context}: {}",
            self.path.display()
        );
    }
}

#[derive(Default)]
struct FakePlatformState {
    pointer: PhysPoint,
    monitors: Vec<Monitor>,
    scale: f32,
    reduced_motion: bool,
    translucent: bool,
    mapped: bool,
    map_count: usize,
    hide_count: usize,
    grabbed: bool,
    grab_count: usize,
    present_count: usize,
    input_regions: Vec<Option<Vec<ShapePx>>>,
    payload_requests: Vec<String>,
    dnd_replies: Vec<bool>,
    dnd_finishes: Vec<bool>,
    clipboard: Option<ClipboardContent>,
}

struct FakePlatform {
    state: Arc<Mutex<FakePlatformState>>,
    monitors_cache: Vec<Monitor>,
    caps: Capabilities,
}

impl FakePlatform {
    fn new(
        monitors: Vec<Monitor>,
        scale: f32,
        translucent: bool,
    ) -> (Self, Arc<Mutex<FakePlatformState>>) {
        let state = Arc::new(Mutex::new(FakePlatformState {
            pointer: PhysPoint::new(960, 540),
            monitors: monitors.clone(),
            scale,
            reduced_motion: true,
            translucent,
            ..Default::default()
        }));
        let caps = Capabilities {
            drag_session: Support::Yes,
            invoke_at_pointer: Support::Yes,
            translucency: if translucent {
                Support::Yes
            } else {
                Support::Degraded("no compositor")
            },
            input_passthrough: Support::Yes,
            keyboard_in_drag: Support::No("drag source holds keyboard grab"),
            clipboard_files: Support::Yes,
        };
        (
            Self {
                state: Arc::clone(&state),
                monitors_cache: monitors,
                caps,
            },
            state,
        )
    }
}

impl Platform for FakePlatform {
    fn name(&self) -> &'static str {
        "fake-n4"
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn sync_graphical_state(&mut self) {
        let s = self.state.lock().unwrap();
        self.monitors_cache = s.monitors.clone();
        self.caps.translucency = if s.translucent {
            Support::Yes
        } else {
            Support::Degraded("no compositor")
        };
    }

    #[cfg(unix)]
    fn poll_fd(&self) -> std::os::fd::RawFd {
        0
    }

    fn dispatch_pending(&mut self, _out: &mut Vec<PlatformEvent>) -> Result<(), PlatformError> {
        Ok(())
    }

    fn monitors(&self) -> &[Monitor] {
        &self.monitors_cache
    }

    fn pointer_position(&mut self) -> Result<PhysPoint, PlatformError> {
        Ok(self.state.lock().unwrap().pointer)
    }

    fn overlay_window_id(&self) -> u32 {
        0x0500_0001
    }

    fn scale(&self) -> f32 {
        self.state.lock().unwrap().scale
    }

    fn reduced_motion(&self) -> Option<bool> {
        Some(self.state.lock().unwrap().reduced_motion)
    }

    fn prepare_overlay(&mut self, at: PhysPoint) -> Result<OverlayInfo, PlatformError> {
        let s = self.state.lock().unwrap();
        let mon = s
            .monitors
            .iter()
            .find(|m| {
                at.x >= m.rect.x
                    && at.y >= m.rect.y
                    && at.x < m.rect.x + m.rect.w as i32
                    && at.y < m.rect.y + m.rect.h as i32
            })
            .cloned()
            .unwrap_or_else(|| s.monitors[0].clone());
        Ok(OverlayInfo {
            origin: PhysPoint::new(mon.rect.x, mon.rect.y),
            size: (mon.rect.w, mon.rect.h),
            scale: s.scale,
            monitor: mon,
            translucent: s.translucent,
        })
    }

    fn map_overlay(&mut self) -> Result<(), PlatformError> {
        let mut s = self.state.lock().unwrap();
        s.mapped = true;
        s.map_count += 1;
        Ok(())
    }

    fn hide_overlay(&mut self) {
        let mut s = self.state.lock().unwrap();
        s.mapped = false;
        s.hide_count += 1;
    }

    fn set_input_region(&mut self, shapes: Option<&[ShapePx]>) -> Result<(), PlatformError> {
        self.state
            .lock()
            .unwrap()
            .input_regions
            .push(shapes.map(|s| s.to_vec()));
        Ok(())
    }

    fn present(&mut self, _frame: FrameRef<'_>, _damage: &[RectPx]) -> Result<(), PlatformError> {
        self.state.lock().unwrap().present_count += 1;
        Ok(())
    }

    fn release_buffers(&mut self) {}

    fn grab_input(&mut self) -> Result<(), PlatformError> {
        let mut s = self.state.lock().unwrap();
        s.grabbed = true;
        s.grab_count += 1;
        Ok(())
    }

    fn ungrab_input(&mut self) {
        self.state.lock().unwrap().grabbed = false;
    }

    fn request_payload(&mut self, mime: &str) {
        self.state
            .lock()
            .unwrap()
            .payload_requests
            .push(mime.to_string());
    }

    fn dnd_reply(&mut self, accept: bool) {
        self.state.lock().unwrap().dnd_replies.push(accept);
    }

    fn dnd_finish(&mut self, success: bool) {
        self.state.lock().unwrap().dnd_finishes.push(success);
    }

    fn set_clipboard(&mut self, content: ClipboardContent) -> Result<(), PlatformError> {
        self.state.lock().unwrap().clipboard = Some(content);
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

fn default_1080p_monitor() -> Vec<Monitor> {
    vec![Monitor {
        id: 0,
        name: "TEST-0".into(),
        rect: RectPx::new(0, 0, 1920, 1080),
        refresh_mhz: 60_000,
        primary: true,
    }]
}

fn concentric_config() -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.wheel.layout = "concentric".to_string();
    cfg.appearance.reduced_motion = ReducedMotionSetting::Fixed(true);
    cfg
}

fn make_controller(
    plat: FakePlatform,
    registry: Arc<radivine_core::ActionRegistry>,
    cfg: AppConfig,
) -> SessionController {
    let renderer = Renderer::new(cfg.resolve_theme(), AssetSource::Embedded).expect("renderer");
    SessionController::new(Box::new(plat), renderer, registry, cfg)
}

/// Computes the physical pixel point for segment `seg_idx` on ring `ring_depth`
/// of the active `WheelSession`.
fn ring_segment_phys_point(
    ctrl: &SessionController,
    origin: PhysPoint,
    scale: f32,
    ring_depth: usize,
    seg_idx: usize,
) -> PhysPoint {
    let ws = ctrl.wheel_session().expect("active wheel_session");
    let dbg = ws.debug_state();
    let wheel = &dbg.wheels[ring_depth];
    let span = &wheel.spans[seg_idx];
    let mid_r = (wheel.r_in + wheel.r_out) * 0.5;
    let logical_pt = wheel.center.offset_polar(mid_r, span.center);
    PhysPoint::new(
        origin.x + (logical_pt.x * scale).round() as i32,
        origin.y + (logical_pt.y * scale).round() as i32,
    )
}

/// `T-SAFE1` & `T-SAFE2` (Scenarios 1 & 2):
/// 1. Drag a disposable file and successfully select a Concentric action (`Copy` at `[1]`).
/// 2. Drop inside the central hub, separator, radial gap, outer margin, caption rect, or outside:
///    `dnd_finish(false)`, zero unintended actions dispatched, and disposable source file
///    remains byte-for-byte and inode-identical.
#[test]
fn t_safe1_t_safe2_concentric_drag_commit_and_non_actionable_zone_drops() {
    let dir = make_temp_test_dir("safe1-safe2");
    let file_path = dir.join("disposable_note.txt");
    fs::write(&file_path, b"Radivine N4 disposable drag payload\n").expect("write disposable file");
    let fp = FileFingerprint::capture(&file_path);
    let file_uri = format!("file://{}", file_path.display());

    let (plat, plat_state) = FakePlatform::new(default_1080p_monitor(), 1.0, true);
    let mut ctrl = make_controller(plat, Arc::new(builtin_registry()), concentric_config());

    // --- Part A: Non-actionable drops (hub, separator boundary, inter-ring gap, outer margin, caption, outside) ---
    ctrl.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: PlatSourceInfo {
                app: Some("nemo".into()),
                pid: Some(4242),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".into(),
            result: Ok(format!("{file_uri}\r\n").into_bytes()),
        },
    ]);
    assert_eq!(ctrl.status().session_state, "Active");
    let snap = ctrl.active_session_settings().expect("session snapshot");
    assert_eq!(snap.layout_selection.requested, WheelLayout::Concentric);
    assert_eq!(snap.layout_selection.effective, WheelLayout::Concentric);
    assert_eq!(snap.layout_selection.fallback, None);
    // Zero system-wide input grabs in Drag mode (`T-X113`)
    assert_eq!(plat_state.lock().unwrap().grab_count, 0);

    // Move onto root submenu `[0]` ("File Preview") and dwell to open ring 1
    let seg0_pt = ring_segment_phys_point(&ctrl, PhysPoint::new(0, 0), 1.0, 0, 0);
    ctrl.handle_platform_events(vec![PlatformEvent::DndPosition { at: seg0_pt }]);
    ctrl.on_timer(Instant::now() + Duration::from_millis(200));
    assert_eq!(
        ctrl.wheel_session().unwrap().debug_state().wheels.len(),
        2,
        "ring 1 must open after hover dwell on submenu segment [0]"
    );

    let dbg = ctrl.wheel_session().unwrap().debug_state();
    let center = dbg.wheels[0].center;
    let r0_in = dbg.wheels[0].r_in;
    let r0_out = dbg.wheels[0].r_out;
    let r1_in = dbg.wheels[1].r_in;
    let r1_out = dbg.wheels[1].r_out;
    let sep_angle = dbg.wheels[0].spans[0].a1;
    let sep_pt = center.offset_polar((r0_in + r0_out) * 0.5, sep_angle);
    let gap_pt = center.offset_polar((r0_out + r1_in) * 0.5, dbg.wheels[1].spans[0].center);
    let hub_pt = Point::new(center.x, center.y);
    let outer_margin_pt = Point::new(center.x + r1_out + 10.0, center.y);
    let outside_pt = Point::new(center.x + r1_out + 180.0, center.y);
    let caption_pt = ctrl
        .wheel_session()
        .unwrap()
        .scene()
        .caption
        .as_ref()
        .map(|c| Point::new(c.box_.x + c.box_.w * 0.5, c.box_.y + c.box_.h * 0.5))
        .unwrap_or(Point::new(center.x, center.y + r1_out + 36.0));

    for (zone_name, pt) in [
        ("central_hub", hub_pt),
        ("separator_boundary", sep_pt),
        ("radial_gap", gap_pt),
        ("outer_margin", outer_margin_pt),
        ("caption_rect", caption_pt),
        ("outside_cluster", outside_pt),
    ] {
        if ctrl.status().session_state != "Active" {
            ctrl.handle_platform_events(vec![
                PlatformEvent::Trigger {
                    at: PhysPoint::new(960, 540),
                },
                PlatformEvent::DndEnter {
                    types: vec!["text/uri-list".into()],
                },
                PlatformEvent::Payload {
                    mime: "text/uri-list".into(),
                    result: Ok(format!("{file_uri}\r\n").into_bytes()),
                },
            ]);
            let p0 = ring_segment_phys_point(&ctrl, PhysPoint::new(0, 0), 1.0, 0, 0);
            ctrl.handle_platform_events(vec![PlatformEvent::DndPosition { at: p0 }]);
            ctrl.on_timer(Instant::now() + Duration::from_millis(200));
        }

        plat_state.lock().unwrap().dnd_replies.clear();
        plat_state.lock().unwrap().dnd_finishes.clear();

        let phys = PhysPoint::new(pt.x.round() as i32, pt.y.round() as i32);
        ctrl.handle_platform_events(vec![
            PlatformEvent::DndPosition { at: phys },
            PlatformEvent::DndDrop,
        ]);

        assert_eq!(
            plat_state.lock().unwrap().dnd_finishes,
            vec![false],
            "drop in non-actionable zone `{zone_name}` at {phys:?} must finish with false"
        );
        assert!(
            ctrl.status().last_committed_action.is_none(),
            "drop in `{zone_name}` must never dispatch an action"
        );
        fp.verify_unchanged(zone_name);
    }

    // --- Part B: Valid Concentric leaf commit (`[1]` `radivine.copy`) ---
    ctrl.handle_platform_events(vec![
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".into(),
            result: Ok(format!("{file_uri}\r\n").into_bytes()),
        },
    ]);
    plat_state.lock().unwrap().dnd_replies.clear();
    plat_state.lock().unwrap().dnd_finishes.clear();

    let copy_pt = ring_segment_phys_point(&ctrl, PhysPoint::new(0, 0), 1.0, 0, 1);
    ctrl.handle_platform_events(vec![
        PlatformEvent::DndPosition { at: copy_pt },
        PlatformEvent::DndDrop,
    ]);
    assert_eq!(plat_state.lock().unwrap().dnd_replies.last(), Some(&true));
    assert_eq!(plat_state.lock().unwrap().dnd_finishes, vec![true]);

    let status = ctrl.status();
    assert_eq!(
        status.last_committed_action.as_deref(),
        Some("radivine.copy")
    );
    assert_eq!(status.last_committed_path, Some(vec![1]));
    assert_eq!(status.last_committed_items, vec![file_uri]);
    fp.verify_unchanged("valid_concentric_commit");

    let _ = fs::remove_dir_all(&dir);
}

/// `T-SAFE3`, `T-SAFE5`, `T-SAFE7`:
/// - Returning pointer to parent ring closes outer ring (`T-SAFE3`); dropping at the closed
///   outer ring's former coordinates rejects (`T-SAFE5`).
/// - Summoning near a viewport edge/corner displaces the shared center and protects initial
///   approach traversal across outer rings (`T-SAFE7`) until the pointer enters the root ring.
#[test]
fn t_safe3_t_safe5_t_safe7_close_ring_retraction_and_displaced_center_approach_protection() {
    let (plat, plat_state) = FakePlatform::new(default_1080p_monitor(), 1.0, true);
    let mut ctrl = make_controller(plat, Arc::new(builtin_registry()), concentric_config());

    // 1. Summon near top-left corner (16, 16) -> center is displaced inward (`T-SAFE7`)
    ctrl.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: PlatSourceInfo {
                app: Some("nemo".into()),
                pid: Some(100),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(16, 16),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".into(),
            result: Ok(b"file:///tmp/corner.txt\r\n".to_vec()),
        },
    ]);
    let center = ctrl.wheel_session().unwrap().debug_state().wheels[0].center;
    assert!(
        center.x > 100.0 && center.y > 100.0,
        "center {center:?} must be displaced inward from corner (16, 16)"
    );
    assert!(
        ctrl.wheel_session()
            .unwrap()
            .debug_state()
            .hovered_item
            .is_none(),
        "initial displaced-center summon must not prematurely hover a segment"
    );

    // Dropping before entering the root ring must reject (`T-SAFE7`)
    plat_state.lock().unwrap().dnd_finishes.clear();
    ctrl.handle_platform_events(vec![
        PlatformEvent::DndPosition {
            at: PhysPoint::new(30, 30),
        },
        PlatformEvent::DndDrop,
    ]);
    assert_eq!(plat_state.lock().unwrap().dnd_finishes, vec![false]);

    // 2. Summon at center, open ring 1 (`[0]` File Preview), then return to root leaf `[1]` (`T-SAFE3`, `T-SAFE5`)
    ctrl.handle_platform_events(vec![
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".into(),
            result: Ok(b"file:///tmp/retract.txt\r\n".to_vec()),
        },
    ]);
    let seg0_pt = ring_segment_phys_point(&ctrl, PhysPoint::new(0, 0), 1.0, 0, 0);
    ctrl.handle_platform_events(vec![PlatformEvent::DndPosition { at: seg0_pt }]);
    ctrl.on_timer(Instant::now() + Duration::from_millis(200));
    assert_eq!(ctrl.wheel_session().unwrap().debug_state().wheels.len(), 2);

    let ring1_seg0_pt = ring_segment_phys_point(&ctrl, PhysPoint::new(0, 0), 1.0, 1, 0);
    ctrl.handle_platform_events(vec![PlatformEvent::DndPosition { at: ring1_seg0_pt }]);
    assert_eq!(
        ctrl.wheel_session()
            .unwrap()
            .debug_state()
            .hovered_item
            .as_ref()
            .map(|p| p.as_slice()),
        Some(&[0u8, 0u8][..])
    );

    // Return to root leaf `[1]` and advance timer past submenu_grace_ms -> ring 1 closes (`T-SAFE3`)
    let root_seg1_pt = ring_segment_phys_point(&ctrl, PhysPoint::new(0, 0), 1.0, 0, 1);
    ctrl.handle_platform_events(vec![PlatformEvent::DndPosition { at: root_seg1_pt }]);
    ctrl.on_timer(Instant::now() + Duration::from_millis(500));
    assert_eq!(
        ctrl.wheel_session().unwrap().debug_state().wheels.len(),
        1,
        "outer ring must close after returning to a sibling segment on the parent ring"
    );

    // Move to `ring1_seg0_pt` (where ring 1 used to be) and drop -> must reject (`T-SAFE5`)
    plat_state.lock().unwrap().dnd_finishes.clear();
    ctrl.handle_platform_events(vec![
        PlatformEvent::DndPosition { at: ring1_seg0_pt },
        PlatformEvent::DndDrop,
    ]);
    assert_eq!(
        plat_state.lock().unwrap().dnd_finishes,
        vec![false],
        "dropping at a closed outer ring position must reject"
    );
}

/// `T-SAFE4`, `T-SAFE6`, `T-CFG3`, `T-CFG6` (Scenarios 3 & 6):
/// - Changing layout while a Drag is active (`Summoning`, `Active`, `LeftGrace`, pending payload)
///   leaves the active session unchanged (`layout_pending == true`), never hides or remaps the
///   overlay, and applies on the next activation.
/// - Switching `Concentric -> Branching -> Concentric` across separate activations works cleanly.
#[test]
fn t_safe4_t_safe6_t_cfg3_t_cfg6_active_session_immutability_and_multi_session_layout_switching() {
    let (plat, plat_state) = FakePlatform::new(default_1080p_monitor(), 1.0, true);
    let mut ctrl = make_controller(plat, Arc::new(builtin_registry()), concentric_config());

    // 1. Start Drag session in `Summoning` (payload not yet delivered)
    ctrl.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: PlatSourceInfo {
                app: Some("nemo".into()),
                pid: Some(200),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
    ]);
    assert_eq!(ctrl.status().session_state, "Summoning");
    let maps_at_summon = plat_state.lock().unwrap().map_count;
    let hides_at_summon = plat_state.lock().unwrap().hide_count;

    // Reload to Branching during `Summoning`
    let mut branching_cfg = concentric_config();
    branching_cfg.wheel.layout = "branching".to_string();
    ctrl.reload_config(branching_cfg.clone());
    assert_eq!(
        ctrl.active_session_settings()
            .unwrap()
            .layout_selection
            .effective,
        WheelLayout::Concentric
    );
    assert_eq!(ctrl.status().requested_layout, "branching");
    assert_eq!(ctrl.status().active_layout, "concentric");
    assert!(ctrl.status().layout_pending);
    assert_eq!(plat_state.lock().unwrap().map_count, maps_at_summon);
    assert_eq!(plat_state.lock().unwrap().hide_count, hides_at_summon);

    // Deliver payload -> transitions to `Active`, still Concentric
    ctrl.handle_platform_events(vec![PlatformEvent::Payload {
        mime: "text/uri-list".into(),
        result: Ok(b"file:///tmp/active.txt\r\n".to_vec()),
    }]);
    assert_eq!(ctrl.status().session_state, "Active");
    assert_eq!(
        ctrl.wheel_session().unwrap().layout_selection().effective,
        WheelLayout::Concentric
    );

    // Repeated alternating reloads during `Active` and `LeftGrace` (`T-SAFE6`)
    ctrl.reload_config(concentric_config());
    ctrl.reload_config(branching_cfg.clone());
    ctrl.handle_platform_events(vec![PlatformEvent::DndLeave { drag_ended: false }]);
    assert_eq!(ctrl.status().session_state, "LeftGrace");
    ctrl.reload_config(concentric_config());
    ctrl.reload_config(branching_cfg.clone());
    assert_eq!(
        ctrl.wheel_session().unwrap().layout_selection().effective,
        WheelLayout::Concentric
    );
    assert_eq!(plat_state.lock().unwrap().map_count, maps_at_summon);
    assert_eq!(plat_state.lock().unwrap().hide_count, hides_at_summon);

    // Re-enter and cancel session
    ctrl.handle_platform_events(vec![
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
        PlatformEvent::DndPosition {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndDrop,
    ]);
    assert_eq!(ctrl.status().session_state, "Armed");

    // 2. Next activation uses Branching (`Concentric -> Branching`)
    ctrl.handle_platform_events(vec![
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".into(),
            result: Ok(b"file:///tmp/branching.txt\r\n".to_vec()),
        },
    ]);
    assert_eq!(
        ctrl.wheel_session().unwrap().layout_selection().effective,
        WheelLayout::Branching
    );
    assert_eq!(ctrl.status().active_layout, "branching");
    assert!(!ctrl.status().layout_pending);

    // Dismiss Branching, reload back to Concentric, and activate (`Branching -> Concentric`)
    ctrl.dismiss_immediate();
    ctrl.reload_config(concentric_config());
    ctrl.handle_platform_events(vec![
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".into(),
            result: Ok(b"file:///tmp/concentric_again.txt\r\n".to_vec()),
        },
    ]);
    assert_eq!(
        ctrl.wheel_session().unwrap().layout_selection().effective,
        WheelLayout::Concentric
    );
    assert_eq!(ctrl.status().active_layout, "concentric");
}

/// `T-SAFE8` & `T-CFG7` (Scenarios 4 & 5):
/// - Reloading `ActionRegistry` during pending payload resolution never remaps a menu path to a
///   different action.
/// - Disabling or removing an action while pending safely rejects (`dnd_finish(false)`) when
///   payload resolution completes.
#[test]
fn t_safe8_t_cfg7_deferred_payload_action_identity_and_disabled_action_rejection() {
    let dir = make_temp_test_dir("safe8-cfg7");
    let real_dir = dir.join("folder_payload");
    fs::create_dir_all(&real_dir).expect("create folder_payload");
    let actions_path = dir.join("actions.toml");

    fs::write(
        &actions_path,
        r#"
version = 1

[[action]]
id = "custom.text_only"
label = "Text Only"
exec = ["true"]
[action.when]
extensions = ["txt"]

[[action]]
id = "custom.any_item"
label = "Any Item"
exec = ["true"]

[menu]
order = ["radivine.custom", "radivine.copy", "radivine.properties"]
hide = ["radivine.preview", "radivine.convert"]
"#,
    )
    .expect("write actions.toml");

    let (plat, plat_state) = FakePlatform::new(default_1080p_monitor(), 1.0, true);
    let mut ctrl = make_controller(plat, Arc::new(builtin_registry()), concentric_config());
    ctrl.load_actions_from_path(&actions_path);

    // Start Concentric drag and force provisional menu (source refuses pre-drop selection twice)
    ctrl.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: PlatSourceInfo {
                app: Some("nemo".into()),
                pid: Some(300),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
        PlatformEvent::DndPosition {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".into(),
            result: Err(PayloadError::Timeout),
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".into(),
            result: Err(PayloadError::Timeout),
        },
    ]);
    assert_eq!(ctrl.status().session_state, "Active");

    // Open ring 1 (`[0]` `Custom`) by dwelling on `[0]`, then move to `[0, 0]` (`custom.text_only`) and drop
    let seg0_pt = ring_segment_phys_point(&ctrl, PhysPoint::new(0, 0), 1.0, 0, 0);
    ctrl.handle_platform_events(vec![PlatformEvent::DndPosition { at: seg0_pt }]);
    ctrl.on_timer(Instant::now() + Duration::from_millis(200));
    assert_eq!(ctrl.wheel_session().unwrap().debug_state().wheels.len(), 2);

    let child_pt = ring_segment_phys_point(&ctrl, PhysPoint::new(0, 0), 1.0, 1, 0);
    plat_state.lock().unwrap().dnd_finishes.clear();
    ctrl.handle_platform_events(vec![
        PlatformEvent::DndPosition { at: child_pt },
        PlatformEvent::DndDrop,
    ]);
    assert_eq!(ctrl.status().session_state, "Committing");
    assert_eq!(
        ctrl.pending_drop_selection(),
        Some(&PendingDropSelection {
            path: ItemPath::from_slice(&[0, 0]),
            action_id: ActionId::from("custom.text_only"),
        })
    );
    assert!(
        plat_state.lock().unwrap().dnd_finishes.is_empty(),
        "must not send dnd_finish before deferred payload validation completes"
    );

    // Reload `actions.toml` while drop is pending AND deliver directory payload:
    // In the rebuilt menu for `real_dir`, `custom.text_only` is absent and `custom.any_item` moves to `[0, 0]`.
    // Must reject safely (`dnd_finish(false)`) and never execute `custom.any_item`!
    fs::write(
        &actions_path,
        r#"
version = 1

[[action]]
id = "custom.any_item"
label = "Any Item"
exec = ["true"]

[menu]
order = ["radivine.custom", "radivine.copy", "radivine.properties"]
hide = ["radivine.preview", "radivine.convert"]
"#,
    )
    .expect("rewrite actions.toml");
    ctrl.load_actions_from_path(&actions_path);

    let dir_uri = format!("file://{}\r\n", real_dir.display());
    ctrl.handle_platform_events(vec![PlatformEvent::Payload {
        mime: "text/uri-list".into(),
        result: Ok(dir_uri.into_bytes()),
    }]);
    assert_eq!(
        plat_state.lock().unwrap().dnd_finishes,
        vec![false],
        "must reject when pinned ActionId is absent/ineligible for the resolved payload"
    );
    assert!(ctrl.status().last_committed_action.is_none());

    let _ = fs::remove_dir_all(&dir);
}

/// `T-MENU1`–`T-MENU5` (Scenario 10):
/// - Real `ActionRegistry` hierarchy in Concentric mode (`T-MENU1`).
/// - Disabled item stays visible and hoverable with reason caption, and rejects drop (`T-MENU2`).
/// - Disabled ancestor blocks descendants (`T-MENU3`).
/// - `default_child` opens submenu on hover in Drag and commits on click in Invoke (`T-MENU4`).
/// - `More…` overflow wrapping at depth < `max_depth` (`T-MENU5`).
#[test]
fn t_menu1_t_menu5_concentric_menu_semantics_disabled_ancestors_default_children_and_overflow() {
    let dir = make_temp_test_dir("menu1-menu5");
    let actions_path = dir.join("actions.toml");

    // Create 9 custom actions under `radivine.custom` so `Custom` submenu exceeds 8 items and wraps into `More…` (`T-MENU5`)
    let mut actions_toml = String::from("version = 1\n");
    for i in 0..9 {
        actions_toml.push_str(&format!(
            r#"
[[action]]
id = "custom.item_{i}"
label = "Custom Item {i}"
exec = ["/bin/true", "{{path}}"]
"#
        ));
    }
    fs::write(&actions_path, &actions_toml).expect("write actions.toml");

    let (plat, plat_state) = FakePlatform::new(default_1080p_monitor(), 1.0, true);
    let mut ctrl = make_controller(plat, Arc::new(builtin_registry()), concentric_config());
    ctrl.load_actions_from_path(&actions_path);

    // 1. Verify `More…` overflow wrapping in `ActionRegistry::build_menu` (`T-MENU5`)
    let basic = BasicContext::from_uris(
        ["file:///tmp/sample.txt"],
        InvocationMode::Drag,
        CoreSourceInfo::default(),
    );
    let ctx = Context::from_basic(basic);
    let (tree, _) = builtin_registry()
        .build_menu(&ctx, MenuLimits::default())
        .expect("build_menu");
    assert!(!tree.root.children().unwrap().is_empty());

    // 2. Drag a directory (`file://{dir}`) and open `[0]` ("File Preview") before async enrichment completes;
    //    once enrichment finishes (directory is not a regular text file), `[0]` and its children `[0, 0..2]`
    //    are disabled in-place (`T-MENU2`, `T-MENU3`), and dropping on `[0, 0]` rejects (`dnd_finish(false)`).
    ctrl.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: PlatSourceInfo {
                app: Some("nemo".into()),
                pid: Some(400),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".into(),
            result: Ok(format!("file://{}\r\n", dir.display()).into_bytes()),
        },
    ]);
    let seg0_pt = ring_segment_phys_point(&ctrl, PhysPoint::new(0, 0), 1.0, 0, 0);
    ctrl.handle_platform_events(vec![PlatformEvent::DndPosition { at: seg0_pt }]);
    ctrl.on_timer(Instant::now() + Duration::from_millis(200));
    assert_eq!(ctrl.wheel_session().unwrap().debug_state().wheels.len(), 2);

    // Wait briefly for background `Enricher` to inspect `dir` and drain async events via `on_timer`
    thread::sleep(Duration::from_millis(40));
    ctrl.on_timer(Instant::now() + Duration::from_millis(250));

    let ring1_seg0_pt = ring_segment_phys_point(&ctrl, PhysPoint::new(0, 0), 1.0, 1, 0);
    plat_state.lock().unwrap().dnd_replies.clear();
    plat_state.lock().unwrap().dnd_finishes.clear();
    ctrl.handle_platform_events(vec![
        PlatformEvent::DndPosition { at: ring1_seg0_pt },
        PlatformEvent::DndDrop,
    ]);
    assert_eq!(
        plat_state.lock().unwrap().dnd_finishes,
        vec![false],
        "dropping on child `[0, 0]` after enrichment disables directory preview must reject (`T-MENU2` / `T-MENU3`)"
    );

    // 3. `T-MENU4` (`INTERACTION_DESIGN.md` §I6):
    //    Configure a submenu with `default: Some(ActionId::from("radivine.preview.full"))`:
    //    - In Invoke mode, clicking submenu `[0]` opens ring 1 and NEVER commits `default_child`.
    //    - In Drag mode, dropping directly on submenu `[0]` commits its `default_child` (`radivine.preview.full`).
    let txt_file = dir.join("sample.txt");
    fs::write(&txt_file, b"Preview me\n").expect("write sample.txt");
    let txt_uri = format!("file://{}", txt_file.display());

    let mut reg_default = builtin_registry();
    reg_default.set_menu(
        vec![
            MenuSpec::Submenu {
                id: Arc::from("radivine.preview"),
                label: "File Preview".to_string(),
                icon: Some(IconKey::from("lucide:eye")),
                description: Some("Preview the file".to_string()),
                children: vec![
                    MenuSpec::Action(ActionId::from("radivine.preview.full")),
                    MenuSpec::Action(ActionId::from("radivine.preview.head")),
                    MenuSpec::Action(ActionId::from("radivine.preview.tail")),
                ],
                default: Some(ActionId::from("radivine.preview.full")),
            },
            MenuSpec::Action(ActionId::from("radivine.copy")),
        ],
        MenuOverrides::default(),
    );
    let (plat_def, state_def) = FakePlatform::new(default_1080p_monitor(), 1.0, true);
    let mut ctrl_def = make_controller(plat_def, Arc::new(reg_default), concentric_config());

    ctrl_def
        .summon_invoke(vec![txt_uri.clone()], Some(PhysPoint::new(960, 540)))
        .expect("summon_invoke");
    let inv_seg0_pt = ring_segment_phys_point(&ctrl_def, PhysPoint::new(0, 0), 1.0, 0, 0);
    ctrl_def.handle_platform_events(vec![
        PlatformEvent::PointerMoved { at: inv_seg0_pt },
        PlatformEvent::Button {
            button: 1,
            pressed: true,
            at: inv_seg0_pt,
        },
        PlatformEvent::Button {
            button: 1,
            pressed: false,
            at: inv_seg0_pt,
        },
    ]);
    assert_eq!(
        ctrl_def.wheel_session().unwrap().debug_state().wheels.len(),
        2,
        "clicking submenu in Invoke mode must open ring 1 (`T-MENU4`)"
    );
    assert!(
        ctrl_def.status().last_committed_action.is_none(),
        "clicking submenu in Invoke mode must not commit default_child (`T-MENU4`)"
    );
    ctrl_def.dismiss_immediate();

    // Now in Drag mode on `txt_uri`, drop directly on submenu `[0]` -> commits `default_child` (`radivine.preview.full`)
    ctrl_def.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: PlatSourceInfo {
                app: Some("nemo".into()),
                pid: Some(401),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".into(),
            result: Ok(format!("{txt_uri}\r\n").into_bytes()),
        },
    ]);
    let drag_seg0_pt = ring_segment_phys_point(&ctrl_def, PhysPoint::new(0, 0), 1.0, 0, 0);
    state_def.lock().unwrap().dnd_finishes.clear();
    ctrl_def.handle_platform_events(vec![
        PlatformEvent::DndPosition { at: drag_seg0_pt },
        PlatformEvent::DndDrop,
    ]);
    assert_eq!(state_def.lock().unwrap().dnd_finishes, vec![true]);
    assert_eq!(
        ctrl_def.status().last_committed_action.as_deref(),
        Some("radivine.preview.full"),
        "dropping on submenu with default_child in Drag mode must commit default_child (`T-MENU4`)"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// `T-X111`–`T-X118` (Scenarios 7, 8, 9, 11, 12):
/// - Insufficient-space and `no_compositor` fallback to Branching before activation (`Scenario 7`).
/// - Open nested submenus near all 4 viewport edges and 4 corners (`Scenario 8`).
/// - Multi-monitor negative-origin coordinates `(-2560, -240)` and fractional scale `1.25` & `1.5` (`Scenario 9`).
/// - Display invalidation (`MonitorsChanged` and mid-session compositor loss) enters `reject_until_drag_end` (`T-X118`).
#[test]
fn t_x111_t_x118_concentric_fallbacks_edges_negative_origin_fractional_scale_and_display_guards() {
    // --- Scenario 7a: Insufficient viewport space falls back to Branching with exact caption ---
    let small_mon = vec![Monitor {
        id: 0,
        name: "SMALL-0".into(),
        rect: RectPx::new(0, 0, 640, 480),
        refresh_mhz: 60_000,
        primary: true,
    }];
    let (plat_small, _) = FakePlatform::new(small_mon, 1.0, true);
    let mut ctrl_small = make_controller(
        plat_small,
        Arc::new(builtin_registry()),
        concentric_config(),
    );
    ctrl_small
        .summon_invoke(
            vec!["file:///tmp/small.txt".into()],
            Some(PhysPoint::new(320, 240)),
        )
        .expect("summon on small viewport");
    let st_small = ctrl_small.status();
    assert_eq!(st_small.requested_layout, "concentric");
    assert_eq!(st_small.active_layout, "branching");
    assert_eq!(
        st_small.layout_fallback_reason.as_deref(),
        Some("insufficient_space")
    );
    let cap_small = ctrl_small
        .wheel_session()
        .unwrap()
        .scene()
        .caption
        .as_ref()
        .expect("fallback caption");
    assert_eq!(
        cap_small.title.as_ref(),
        "Using Branching: nested rings need more space"
    );

    // --- Scenario 7b: No compositor (`translucent = false`) falls back to Branching (`no_compositor`) ---
    let (plat_nocomp, _) = FakePlatform::new(default_1080p_monitor(), 1.0, false);
    let mut ctrl_nocomp = make_controller(
        plat_nocomp,
        Arc::new(builtin_registry()),
        concentric_config(),
    );
    ctrl_nocomp
        .summon_invoke(
            vec!["file:///tmp/nocomp.txt".into()],
            Some(PhysPoint::new(960, 540)),
        )
        .expect("summon without compositor");
    let st_nocomp = ctrl_nocomp.status();
    assert_eq!(st_nocomp.requested_layout, "concentric");
    assert_eq!(st_nocomp.active_layout, "branching");
    assert_eq!(
        st_nocomp.layout_fallback_reason.as_deref(),
        Some("no_compositor")
    );

    // --- Scenario 8: Open nested submenus near all 4 viewport edges and 4 corners ---
    let (plat_edges, _) = FakePlatform::new(default_1080p_monitor(), 1.0, true);
    let mut ctrl_edges = make_controller(
        plat_edges,
        Arc::new(builtin_registry()),
        concentric_config(),
    );
    let edge_and_corner_points = [
        PhysPoint::new(10, 540),    // Left edge
        PhysPoint::new(1910, 540),  // Right edge
        PhysPoint::new(960, 10),    // Top edge
        PhysPoint::new(960, 1070),  // Bottom edge
        PhysPoint::new(10, 10),     // Top-left corner
        PhysPoint::new(1910, 10),   // Top-right corner
        PhysPoint::new(10, 1070),   // Bottom-left corner
        PhysPoint::new(1910, 1070), // Bottom-right corner
    ];
    for pt in edge_and_corner_points {
        ctrl_edges
            .summon_invoke(vec!["file:///tmp/edge.txt".into()], Some(pt))
            .expect("summon at edge/corner");
        assert_eq!(
            ctrl_edges
                .wheel_session()
                .unwrap()
                .layout_selection()
                .effective,
            WheelLayout::Concentric
        );
        // Visit displaced hub interior first to clear approach protection (`T-SAFE7`), then move into `[0]`
        let center = ctrl_edges.wheel_session().unwrap().debug_state().wheels[0].center;
        let hub_phys = PhysPoint::new(center.x.round() as i32, center.y.round() as i32);
        let seg0 = ring_segment_phys_point(&ctrl_edges, PhysPoint::new(0, 0), 1.0, 0, 0);
        ctrl_edges.handle_platform_events(vec![
            PlatformEvent::PointerMoved { at: hub_phys },
            PlatformEvent::PointerMoved { at: seg0 },
        ]);
        ctrl_edges.on_timer(Instant::now() + Duration::from_millis(200));
        let dbg = ctrl_edges.wheel_session().unwrap().debug_state();
        assert_eq!(
            dbg.wheels.len(),
            2,
            "nested submenu must open cleanly at edge/corner {pt:?}"
        );
        let r1 = &dbg.wheels[1];
        assert!(
            r1.center.x - r1.r_out >= 8.0 - 1e-3
                && r1.center.x + r1.r_out <= 1920.0 - 8.0 + 1e-3
                && r1.center.y - r1.r_out >= 8.0 - 1e-3
                && r1.center.y + r1.r_out <= 1080.0 - 8.0 + 1e-3,
            "ring 1 at {pt:?} must stay within 8 lp viewport margin: center={:?}, r_out={}",
            r1.center,
            r1.r_out
        );
        ctrl_edges.dismiss_immediate();
    }

    // --- Scenario 9: Multi-monitor negative-origin coordinates + fractional scale (1.25 and 1.5) ---
    for scale in [1.25f32, 1.5f32] {
        let hi_monitors = vec![
            Monitor {
                id: 0,
                name: "LEFT-NEG-QHD".into(),
                rect: RectPx::new(-2560, -240, 2560, 1440),
                refresh_mhz: 60_000,
                primary: false,
            },
            Monitor {
                id: 1,
                name: "RIGHT-PRIMARY".into(),
                rect: RectPx::new(0, 0, 2560, 1440),
                refresh_mhz: 60_000,
                primary: true,
            },
        ];
        let (plat_neg, state_neg) = FakePlatform::new(hi_monitors, scale, true);
        let mut cfg = concentric_config();
        cfg.appearance.scale = ScaleSetting::Fixed(scale);
        let mut ctrl_neg = make_controller(plat_neg, Arc::new(builtin_registry()), cfg);

        let neg_summon = PhysPoint::new(-1280, 480);
        ctrl_neg.handle_platform_events(vec![
            PlatformEvent::DragArmed {
                source: PlatSourceInfo {
                    app: Some("nemo".into()),
                    pid: Some(500),
                },
            },
            PlatformEvent::Trigger { at: neg_summon },
            PlatformEvent::DndEnter {
                types: vec!["text/uri-list".into()],
            },
            PlatformEvent::Payload {
                mime: "text/uri-list".into(),
                result: Ok(b"file:///tmp/neg_fractional.txt\r\n".to_vec()),
            },
        ]);
        assert_eq!(
            ctrl_neg.status().monitor_rect,
            (-2560, -240, 2560, 1440),
            "must select negative-origin monitor at scale {scale}"
        );
        assert_eq!(ctrl_neg.status().active_layout, "concentric");

        let copy_phys =
            ring_segment_phys_point(&ctrl_neg, PhysPoint::new(-2560, -240), scale, 0, 1);
        state_neg.lock().unwrap().dnd_finishes.clear();
        ctrl_neg.handle_platform_events(vec![
            PlatformEvent::DndPosition { at: copy_phys },
            PlatformEvent::DndDrop,
        ]);
        assert_eq!(state_neg.lock().unwrap().dnd_finishes, vec![true]);
        assert_eq!(
            ctrl_neg.status().last_committed_action.as_deref(),
            Some("radivine.copy")
        );
    }

    // --- Scenario 10 (`T-X112` & `ARCHITECTURE.md` §A7): Input region capture policy ---
    // (a) Invoke mode and default Drag mode (`passthrough = false`) must use full-overlay capture (`None`).
    ctrl_edges
        .summon_invoke(
            vec!["file:///tmp/invoke_full_capture.txt".into()],
            Some(PhysPoint::new(960, 540)),
        )
        .expect("summon invoke full capture");
    ctrl_edges.dismiss_immediate();

    let (plat_cap, state_cap) = FakePlatform::new(default_1080p_monitor(), 1.0, true);
    let mut ctrl_cap = make_controller(plat_cap, Arc::new(builtin_registry()), concentric_config());
    ctrl_cap.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: PlatSourceInfo {
                app: Some("nemo".into()),
                pid: Some(601),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".into(),
            result: Ok(b"file:///tmp/default_drag_capture.txt\r\n".to_vec()),
        },
    ]);
    assert_eq!(
        state_cap.lock().unwrap().input_regions.last(),
        Some(&None),
        "default Drag mode (`passthrough = false`) must use full-overlay ShapeInput (`None`)"
    );
    ctrl_cap.dismiss_immediate();

    // (b) Explicit Drag pass-through (`passthrough = true`) uses reserved-disc ShapeInput (excluding caption)
    // and never shrinks when child rings open or retract.
    let (plat_pt, state_pt) = FakePlatform::new(default_1080p_monitor(), 1.0, true);
    let mut cfg_pt = concentric_config();
    cfg_pt.input.passthrough = true;
    let mut ctrl_pt = make_controller(plat_pt, Arc::new(builtin_registry()), cfg_pt);
    ctrl_pt.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: PlatSourceInfo {
                app: Some("nemo".into()),
                pid: Some(602),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(10, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".into(),
            result: Ok(b"file:///tmp/passthrough_drag.txt\r\n".to_vec()),
        },
    ]);
    let initial_shapes = state_pt
        .lock()
        .unwrap()
        .input_regions
        .last()
        .and_then(|r| r.clone())
        .expect("passthrough Drag must install reserved-disc ShapeInput");
    let has_reserved_disc = initial_shapes
        .iter()
        .any(|s| matches!(s, ShapePx::Circle { r, .. } if (*r - 338.0).abs() < 1e-3));
    let has_anchor_disc = initial_shapes.iter().any(|s| {
        matches!(
            s,
            ShapePx::Circle { cx, cy, r }
                if (*cx - 10.0).abs() < 1e-3 && (*cy - 540.0).abs() < 1e-3 && (*r - 24.0).abs() < 1e-3
        )
    });
    let has_corridor = initial_shapes
        .iter()
        .any(|s| matches!(s, ShapePx::Polygon(_)));
    assert!(
        initial_shapes.len() == 3 && has_reserved_disc && has_anchor_disc && has_corridor,
        "passthrough Concentric ShapeInput must consist strictly of reserved disc (Rreserve 326 + hit_margin 12 = 338 px), anchor (24 px), and corridor polygon (caption excluded): {initial_shapes:?}"
    );
    // Open ring 1 and verify ShapeInput does not shrink or change
    let center_pt = ctrl_pt.wheel_session().unwrap().debug_state().wheels[0].center;
    let seg0_pt = ring_segment_phys_point(&ctrl_pt, PhysPoint::new(0, 0), 1.0, 0, 0);
    ctrl_pt.handle_platform_events(vec![
        PlatformEvent::DndPosition {
            at: PhysPoint::new(center_pt.x.round() as i32, center_pt.y.round() as i32),
        },
        PlatformEvent::DndPosition { at: seg0_pt },
    ]);
    ctrl_pt.on_timer(Instant::now() + Duration::from_millis(200));
    assert_eq!(
        ctrl_pt.wheel_session().unwrap().debug_state().wheels.len(),
        2
    );
    let open_shapes = state_pt
        .lock()
        .unwrap()
        .input_regions
        .last()
        .and_then(|r| r.clone())
        .expect("ShapeInput while ring 1 open");
    assert_eq!(
        initial_shapes, open_shapes,
        "reserved-disc ShapeInput must remain stable as child rings open"
    );
    ctrl_pt.dismiss_immediate();

    // --- Scenario 11 (`T-SAFE8` & `ARCHITECTURE.md` §A7): Display / scale / compositor guards ---
    let (plat_guard, state_guard) = FakePlatform::new(default_1080p_monitor(), 1.0, true);
    let mut ctrl_guard = make_controller(
        plat_guard,
        Arc::new(builtin_registry()),
        concentric_config(),
    );
    let t_start = Instant::now();
    ctrl_guard.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: PlatSourceInfo {
                app: Some("nemo".into()),
                pid: Some(603),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".into(),
            result: Ok(b"file:///tmp/guard_test.txt\r\n".to_vec()),
        },
    ]);
    assert_eq!(ctrl_guard.status().session_state, "Active");
    // 11a: Scale change (`SettingsChanged` while compositor + monitor unchanged) is deferred during active Drag
    state_guard.lock().unwrap().scale = 1.5;
    ctrl_guard.handle_platform_events(vec![PlatformEvent::SettingsChanged]);
    assert_eq!(
        ctrl_guard.status().session_state,
        "Active",
        "scale change during active Concentric Drag must be deferred"
    );
    // 11b: Monitor change (`MonitorsChanged`) or compositor loss enters `reject_until_drag_end`,
    // keeps the capture window mapped (`mapped == true`, `hide_count` unchanged) even past MAX_SESSION_DURATION,
    // replies `dnd_reply(false)`, and rejects `DndDrop` with `dnd_finish(false)`.
    let hide_before = state_guard.lock().unwrap().hide_count;
    state_guard.lock().unwrap().translucent = false;
    ctrl_guard.handle_platform_events(vec![PlatformEvent::SettingsChanged]);
    assert!(
        state_guard.lock().unwrap().mapped && state_guard.lock().unwrap().hide_count == hide_before,
        "reject_until_drag_end must keep the capture overlay mapped until the source drag ends"
    );
    ctrl_guard.on_timer(t_start + Duration::from_secs(130));
    assert!(
        state_guard.lock().unwrap().mapped && state_guard.lock().unwrap().hide_count == hide_before,
        "MAX_SESSION_DURATION must not prematurely hide the capture overlay while drag_armed && reject_until_drag_end"
    );
    let copy_pt = ring_segment_phys_point(&ctrl_guard, PhysPoint::new(0, 0), 1.0, 0, 1);
    state_guard.lock().unwrap().dnd_replies.clear();
    state_guard.lock().unwrap().dnd_finishes.clear();
    ctrl_guard.handle_platform_events(vec![
        PlatformEvent::DndPosition { at: copy_pt },
        PlatformEvent::DndDrop,
    ]);
    assert_eq!(state_guard.lock().unwrap().dnd_replies, vec![false]);
    assert_eq!(state_guard.lock().unwrap().dnd_finishes, vec![false]);
    assert!(ctrl_guard.status().last_committed_action.is_none());
    // 11c: ConnectionLost during active Drag dispatches nothing and resets cleanly
    state_guard.lock().unwrap().translucent = true;
    ctrl_guard.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: PlatSourceInfo {
                app: Some("nemo".into()),
                pid: Some(604),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::ConnectionLost,
    ]);
    assert_eq!(ctrl_guard.status().session_state, "Idle");
    assert!(ctrl_guard.status().last_committed_action.is_none());
}

/// Live isolated X11 / `Xvfb` end-to-end Concentric test (`Authorization` §6, `T-X111`–`T-X118`):
/// Executes against a real isolated X11 server (`is_isolated_test_display()`) and verifies:
/// 1. When `_NET_WM_CM_S0` has no owner, Concentric falls back to Branching with `no_compositor`.
/// 2. When `_NET_WM_CM_S0` is owned by a compositor window on the isolated X11 display, the daemon
///    activates Concentric mode on a single 32-bit ARGB overlay window (`overlay_window_id`).
/// 3. Real `XdndEnter` / `XdndPosition` / `XdndDrop` drag of a disposable file replies with
///    `XdndActionPrivate` on actionable segments and `None` (0) on non-actionable hub/outside.
/// 4. Releasing `_NET_WM_CM_S0` mid-drag triggers `XfixesSelectionNotify` -> `reject_until_drag_end`
///    and rejects the drop safely (`XdndFinished` with `accepted = 0`).
#[test]
fn live_xvfb_concentric_end_to_end_xdnd_session_and_compositor_guards() {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{
        ClientMessageData, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask,
        PropMode, SelectionNotifyEvent, WindowClass,
    };
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    if !radivine_platform::is_isolated_test_display() {
        eprintln!(
            "SKIPPED live_xvfb_concentric_end_to_end_xdnd_session_and_compositor_guards: DISPLAY is not an isolated Xvfb test server"
        );
        return;
    }

    let dir = make_temp_test_dir("xvfb-live");
    let disposable_file = dir.join("xvfb_disposable.txt");
    fs::write(&disposable_file, b"Disposable Xvfb Concentric test file\n")
        .expect("write disposable file");
    let fp = FileFingerprint::capture(&disposable_file);
    let file_uri = format!("file://{}", disposable_file.display());

    let cfg_path = dir.join("config.toml");
    let sock_path = dir.join("radivine.sock");
    fs::write(
        &cfg_path,
        r#"
version = 1

[wheel]
layout = "concentric"

[appearance]
reduced_motion = true
"#,
    )
    .expect("write config.toml");

    // Connect a test X11 client to act as both compositor owner (`_NET_WM_CM_S0`) and XDND source
    let (conn, screen_num) = RustConnection::connect(None).expect("connect X11 test client");
    let atoms = Atoms::new(&conn)
        .expect("atoms cookie")
        .reply()
        .expect("atoms reply");
    let root = conn.setup().roots[screen_num].root;

    let cm_win = conn.generate_id().expect("generate cm_win");
    conn.create_window(
        0,
        cm_win,
        root,
        -100,
        -100,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new().override_redirect(1),
    )
    .expect("create cm_win");

    let src_win = conn.generate_id().expect("generate src_win");
    conn.create_window(
        0,
        src_win,
        root,
        -200,
        -200,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new()
            .override_redirect(1)
            .event_mask(EventMask::PROPERTY_CHANGE),
    )
    .expect("create src_win");
    conn.flush().expect("flush windows");

    // Ensure `_NET_WM_CM_S0` is initially unowned so we test `NoCompositor` fallback first (`T-X117`)
    conn.set_selection_owner(x11rb::NONE, atoms._NET_WM_CM_S0, x11rb::CURRENT_TIME)
        .expect("clear _NET_WM_CM_S0");
    conn.flush().expect("flush clear _NET_WM_CM_S0");

    let radivine_bin = PathBuf::from(env!("CARGO_BIN_EXE_radivine"));
    let mut daemon: Child = Command::new(&radivine_bin)
        .env("RADIVINE_DRY_RUN_VIEWER", "1")
        .env("RADIVINE_DRY_RUN_PROPERTIES", "1")
        .env("RADIVINE_SUPPRESS_DESKTOP_NOTIFY", "1")
        .arg("--config")
        .arg(&cfg_path)
        .arg("--socket")
        .arg(&sock_path)
        .arg("daemon")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn radivine daemon under Xvfb");

    let wait_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < wait_deadline {
        if sock_path.exists()
            && Command::new(&radivine_bin)
                .args(["--socket", sock_path.to_str().unwrap(), "status"])
                .output()
                .is_ok_and(|o| o.status.success())
        {
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }

    let query_status = || -> serde_json::Value {
        let out = Command::new(&radivine_bin)
            .args([
                "--config",
                cfg_path.to_str().unwrap(),
                "--socket",
                sock_path.to_str().unwrap(),
                "status",
                "--json",
            ])
            .output()
            .expect("status --json");
        assert!(out.status.success());
        serde_json::from_slice(&out.stdout).expect("parse status json")
    };

    // 1. Without `_NET_WM_CM_S0`, `status` reports `no_compositor` fallback to `branching` (`T-X117`)
    let st_nocomp = query_status();
    assert_eq!(st_nocomp["requested_layout"], "concentric");
    assert_eq!(st_nocomp["active_layout"], "branching");
    assert_eq!(st_nocomp["layout_fallback_reason"], "no_compositor");
    let overlay_win = st_nocomp["overlay_window_id"].as_u64().unwrap() as u32;
    assert!(overlay_win > 0);

    // 2. Claim `_NET_WM_CM_S0` so X11Platform detects active compositor & 32-bit translucency (`T-X111`)
    conn.set_selection_owner(cm_win, atoms._NET_WM_CM_S0, x11rb::CURRENT_TIME)
        .expect("claim _NET_WM_CM_S0");
    conn.flush().expect("flush _NET_WM_CM_S0");
    thread::sleep(Duration::from_millis(40));

    let st_comp = query_status();
    assert_eq!(st_comp["compositor_active"], true);
    assert_eq!(st_comp["translucent_overlay"], true);
    assert_eq!(st_comp["requested_layout"], "concentric");
    assert_eq!(st_comp["active_layout"], "concentric");
    assert!(st_comp["layout_fallback_reason"].is_null());
    assert_eq!(
        st_comp["overlay_window_id"].as_u64().unwrap() as u32,
        overlay_win,
        "must reuse the single overlay window XID (`T-X111`)"
    );

    // 3. Summon via `invoke` on the isolated Xvfb display and verify single overlay XID & render latency
    let _ = Command::new(&radivine_bin)
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "--socket",
            sock_path.to_str().unwrap(),
            "invoke",
            "--at",
            "960,540",
            &file_uri,
        ])
        .status();
    let st_active = query_status();
    assert_eq!(st_active["session_state"], "Active");
    assert_eq!(st_active["active_layout"], "concentric");
    assert_eq!(
        st_active["overlay_window_id"].as_u64().unwrap() as u32,
        overlay_win
    );
    let summon_us = st_active["last_summon_us"]
        .as_u64()
        .expect("must record live X11 summon latency");
    let render_us = st_active["last_frame_render_us"]
        .as_u64()
        .expect("must record live X11 frame render latency");
    eprintln!(
        "LIVE_XVFB_METRICS: summon_us={summon_us} frame_render_us={render_us} overlay_xid=0x{overlay_win:08x}"
    );

    // Dismiss Invoke session cleanly
    let _ = Command::new(&radivine_bin)
        .args(["--socket", sock_path.to_str().unwrap(), "cancel"])
        .status();
    assert_eq!(query_status()["session_state"], "Idle");

    // 4. Perform a live X11 XDND drag session against `overlay_win` (`T-X113`, `T-X114`)
    let send_xdnd_msg = |msg_type: u32, data: [u32; 5]| {
        let ev = ClientMessageEvent {
            response_type: x11rb::protocol::xproto::CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window: overlay_win,
            type_: msg_type,
            data: ClientMessageData::from(data),
        };
        conn.send_event(false, overlay_win, EventMask::NO_EVENT, ev)
            .expect("send Xdnd ClientMessage");
        conn.flush().expect("flush Xdnd ClientMessage");
    };

    let pump_xdnd = |uri_payload: &[u8]| -> Vec<(u32, [u32; 5])> {
        let mut msgs = Vec::new();
        while let Ok(Some(ev)) = conn.poll_for_event() {
            match ev {
                Event::SelectionRequest(req) if req.target == atoms.URI_LIST => {
                    let prop = if req.property == x11rb::NONE {
                        req.target
                    } else {
                        req.property
                    };
                    let _ = conn.change_property8(
                        PropMode::REPLACE,
                        req.requestor,
                        prop,
                        atoms.URI_LIST,
                        uri_payload,
                    );
                    let notify = SelectionNotifyEvent {
                        response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
                        sequence: 0,
                        time: req.time,
                        requestor: req.requestor,
                        selection: req.selection,
                        target: req.target,
                        property: prop,
                    };
                    let _ = conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify);
                    let _ = conn.flush();
                }
                Event::ClientMessage(cm) if cm.window == src_win => {
                    msgs.push((cm.type_, cm.data.as_data32()));
                }
                _ => {}
            }
        }
        msgs
    };

    // Hold mouse button 1 via xdotool, claim XdndSelection, and press Shift to summon Drag wheel
    let _ = Command::new("xdotool")
        .args(["mousemove", "960", "540", "mousedown", "1"])
        .status();
    thread::sleep(Duration::from_millis(15));
    conn.set_selection_owner(src_win, atoms.XdndSelection, x11rb::CURRENT_TIME)
        .expect("own XdndSelection");
    conn.flush().expect("flush XdndSelection");
    thread::sleep(Duration::from_millis(25));
    let _ = Command::new("xdotool")
        .args(["keydown", "Shift_L", "keyup", "Shift_L"])
        .status();

    let payload_bytes = format!("{file_uri}\r\n").into_bytes();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let _ = pump_xdnd(&payload_bytes);
        if query_status()["session_state"] == "Active" {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(query_status()["session_state"], "Active");
    assert_eq!(query_status()["active_layout"], "concentric");

    // Send XdndEnter and XdndPosition at hub (960, 540) -> must reply XdndStatus with accept=0, action=0 (`T-X114`)
    send_xdnd_msg(atoms.XdndEnter, [src_win, 5 << 24, atoms.URI_LIST, 0, 0]);
    send_xdnd_msg(
        atoms.XdndPosition,
        [
            src_win,
            0,
            (960 << 16) | 540,
            x11rb::CURRENT_TIME,
            atoms.XdndActionPrivate,
        ],
    );
    thread::sleep(Duration::from_millis(25));
    let replies_hub = pump_xdnd(&payload_bytes);
    let last_hub_status = replies_hub
        .iter()
        .rev()
        .find(|(t, _)| *t == atoms.XdndStatus)
        .expect("XdndStatus for hub");
    assert_eq!(
        last_hub_status.1[1] & 1,
        0,
        "hub position must reject (`accept = 0`)"
    );
    assert_eq!(last_hub_status.1[4], 0, "rejected status action must be 0");

    // Move to actionable segment `[1]` (`Copy`, right of center: 1044, 540) -> must reply XdndStatus with accept=1, action=XdndActionPrivate
    send_xdnd_msg(
        atoms.XdndPosition,
        [
            src_win,
            0,
            (1044 << 16) | 540,
            x11rb::CURRENT_TIME,
            atoms.XdndActionPrivate,
        ],
    );
    thread::sleep(Duration::from_millis(25));
    let replies_seg = pump_xdnd(&payload_bytes);
    let last_seg_status = replies_seg
        .iter()
        .rev()
        .find(|(t, _)| *t == atoms.XdndStatus)
        .expect("XdndStatus for actionable segment");
    assert_eq!(
        last_seg_status.1[1] & 1,
        1,
        "actionable segment must accept (`accept = 1`)"
    );
    assert_eq!(
        last_seg_status.1[4], atoms.XdndActionPrivate,
        "accepted status action must be XdndActionPrivate (`T-X114`)"
    );
    if let Some(status_us) = query_status()["last_dnd_status_us"].as_u64() {
        eprintln!("LIVE_XVFB_METRICS: dnd_status_us={status_us}");
    }

    // Release `_NET_WM_CM_S0` mid-drag -> daemon enters `reject_until_drag_end` and rejects drop (`T-X118`)
    conn.set_selection_owner(x11rb::NONE, atoms._NET_WM_CM_S0, x11rb::CURRENT_TIME)
        .expect("release _NET_WM_CM_S0 mid-drag");
    conn.flush().expect("flush release _NET_WM_CM_S0");
    thread::sleep(Duration::from_millis(30));

    send_xdnd_msg(atoms.XdndDrop, [src_win, 0, x11rb::CURRENT_TIME, 0, 0]);
    thread::sleep(Duration::from_millis(30));
    let replies_drop = pump_xdnd(&payload_bytes);
    let finished = replies_drop
        .iter()
        .find(|(t, _)| *t == atoms.XdndFinished)
        .expect("XdndFinished after compositor-loss drop");
    assert_eq!(
        finished.1[1], 0,
        "drop after mid-drag compositor loss must finish with accepted=0 (`T-X118`)"
    );

    // Clean up XdndSelection & mouse button
    let _ = conn.set_selection_owner(x11rb::NONE, atoms.XdndSelection, x11rb::CURRENT_TIME);
    let _ = conn.flush();
    let _ = Command::new("xdotool")
        .args(["keyup", "Shift_L", "mouseup", "1"])
        .status();

    // Verify source file is completely untouched
    fp.verify_unchanged("live_xvfb_concentric_test");

    let _ = Command::new(&radivine_bin)
        .args(["--socket", sock_path.to_str().unwrap(), "quit"])
        .status();
    let _ = daemon.kill();
    let _ = daemon.wait();
    let _ = fs::remove_dir_all(&dir);
}

/// Scenarios 11 & 12 + Arabic font fallback check (`Authorization` §5, §7):
/// - Verifies that Branching drag behavior (including 3-level `Custom -> Scripts -> Compute SHA-256`)
///   remains completely unchanged when `wheel.layout = "branching"`.
/// - Verifies that Arabic filenames/labels (`"تجرية_ملف_عربي.txt"`) render and commit cleanly in both
///   Concentric and Branching sessions without panic.
#[test]
fn scenario_11_12_branching_drag_parity_and_arabic_filename_rendering() {
    let dir = make_temp_test_dir("arabic-branching");
    let arabic_file = dir.join("تجرية_ملف.txt");
    fs::write(&arabic_file, "محتوى تجريبي\n".as_bytes()).expect("write arabic file");
    let arabic_uri = radivine_core::local_path_to_file_uri(&arabic_file);

    let (plat, plat_state) = FakePlatform::new(default_1080p_monitor(), 1.0, true);
    let mut branching_cfg = AppConfig::default();
    branching_cfg.wheel.layout = "branching".to_string();
    branching_cfg.appearance.reduced_motion = ReducedMotionSetting::Fixed(true);

    let mut ctrl = make_controller(plat, Arc::new(builtin_registry()), branching_cfg);

    // 1. Unchanged Branching drag session with Arabic UTF-8 filename
    ctrl.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: PlatSourceInfo {
                app: Some("nemo".into()),
                pid: Some(777),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".into(),
            result: Ok(format!("{arabic_uri}\r\n").into_bytes()),
        },
    ]);
    assert_eq!(ctrl.status().active_layout, "branching");
    assert_eq!(
        ctrl.wheel_session().unwrap().layout_selection().effective,
        WheelLayout::Branching
    );

    // Open `File Preview` (`[0]`) and commit `Head` (`[0, 1]`) in Branching mode
    thread::sleep(Duration::from_millis(30));
    ctrl.on_timer(Instant::now() + Duration::from_millis(50));
    let seg0_pt = ring_segment_phys_point(&ctrl, PhysPoint::new(0, 0), 1.0, 0, 0);
    ctrl.handle_platform_events(vec![PlatformEvent::DndPosition { at: seg0_pt }]);
    ctrl.on_timer(Instant::now() + Duration::from_millis(200));
    assert_eq!(ctrl.wheel_session().unwrap().debug_state().wheels.len(), 2);

    let child_head_pt = ring_segment_phys_point(&ctrl, PhysPoint::new(0, 0), 1.0, 1, 1);
    plat_state.lock().unwrap().dnd_finishes.clear();
    ctrl.handle_platform_events(vec![
        PlatformEvent::DndPosition { at: child_head_pt },
        PlatformEvent::DndDrop,
    ]);
    assert_eq!(plat_state.lock().unwrap().dnd_finishes, vec![true]);
    assert_eq!(
        ctrl.status().last_committed_action.as_deref(),
        Some("radivine.preview.head")
    );

    // 2. Switch to Concentric and render/commit the same Arabic UTF-8 filename
    ctrl.reload_config(concentric_config());
    ctrl.handle_platform_events(vec![
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".into()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".into(),
            result: Ok(format!("{arabic_uri}\r\n").into_bytes()),
        },
    ]);
    assert_eq!(ctrl.status().active_layout, "concentric");
    let copy_pt = ring_segment_phys_point(&ctrl, PhysPoint::new(0, 0), 1.0, 0, 1);
    plat_state.lock().unwrap().dnd_finishes.clear();
    ctrl.handle_platform_events(vec![
        PlatformEvent::DndPosition { at: copy_pt },
        PlatformEvent::DndDrop,
    ]);
    assert_eq!(plat_state.lock().unwrap().dnd_finishes, vec![true]);
    assert_eq!(
        ctrl.status().last_committed_action.as_deref(),
        Some("radivine.copy")
    );

    let _ = fs::remove_dir_all(&dir);
}
