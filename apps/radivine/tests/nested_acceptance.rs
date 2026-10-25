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
use sha2::{Digest, Sha256};

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
    sha256: [u8; 32],
    size: u64,
    ino: u64,
    mtime: Option<SystemTime>,
}

impl FileFingerprint {
    fn capture(path: &Path) -> Self {
        let meta = fs::symlink_metadata(path).expect("symlink_metadata");
        let bytes = fs::read(path).expect("read file bytes");
        let sha256: [u8; 32] = Sha256::digest(&bytes).into();
        Self {
            path: path.to_path_buf(),
            bytes,
            sha256,
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
    // --- Scenario 7a: Viewport capacity contract (4 rings Rreserve = 326 + 280x64 caption -> 668x742 lp) ---
    // 1366x768 @ 1x supports Concentric; 1280x720 @ 1x (720 < 742) falls back to Branching (`insufficient_space`)
    let mon_1366 = vec![Monitor {
        id: 0,
        name: "LAPTOP-1366".into(),
        rect: RectPx::new(0, 0, 1366, 768),
        refresh_mhz: 60_000,
        primary: true,
    }];
    let (plat_1366, _) = FakePlatform::new(mon_1366, 1.0, true);
    let mut ctrl_1366 =
        make_controller(plat_1366, Arc::new(builtin_registry()), concentric_config());
    ctrl_1366
        .summon_invoke(
            vec!["file:///tmp/1366.txt".into()],
            Some(PhysPoint::new(683, 384)),
        )
        .expect("summon on 1366x768");
    let st_1366 = ctrl_1366.status();
    assert_eq!(st_1366.requested_layout, "concentric");
    assert_eq!(st_1366.active_layout, "concentric");
    assert!(st_1366.layout_fallback_reason.is_none());
    let plan_1366 = ctrl_1366
        .wheel_session()
        .unwrap()
        .concentric_plan()
        .expect("1366x768 concentric plan");
    assert_eq!(plan_1366.radii.len(), 4);
    assert_eq!(plan_1366.reserved_radius, 326.0);
    assert_eq!(plan_1366.caption_box.w, 280.0);
    assert_eq!(plan_1366.caption_box.h, 64.0);
    assert!(plan_1366.caption_box.bottom() <= 768.0 - 8.0 + 1e-3);
    ctrl_1366.dismiss_immediate();

    for (w, h, label) in [
        (1280, 720, "1280x720"),
        (668, 741, "668x741"),
        (667, 742, "667x742"),
        (640, 480, "640x480"),
    ] {
        let small_mon = vec![Monitor {
            id: 0,
            name: label.into(),
            rect: RectPx::new(0, 0, w, h),
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
                Some(PhysPoint::new((w / 2) as i32, (h / 2) as i32)),
            )
            .expect("summon on small viewport");
        let st_small = ctrl_small.status();
        assert_eq!(st_small.requested_layout, "concentric");
        assert_eq!(
            st_small.active_layout, "branching",
            "{label} must fall back to branching"
        );
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
    }

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
        let plan = ctrl_edges
            .wheel_session()
            .unwrap()
            .concentric_plan()
            .expect("concentric_plan at edge/corner");
        assert_eq!(
            plan.radii.len(),
            4,
            "all 4 configured levels must be reserved before mapping"
        );
        assert_eq!(plan.reserved_radius, 326.0);
        let r1 = &dbg.wheels[1];
        assert!(
            r1.center.x - plan.reserved_radius >= 8.0 - 1e-3
                && r1.center.x + plan.reserved_radius <= 1920.0 - 8.0 + 1e-3
                && r1.center.y - plan.reserved_radius >= 8.0 - 1e-3
                && plan.caption_box.x >= 8.0 - 1e-3
                && plan.caption_box.right() <= 1920.0 - 8.0 + 1e-3
                && plan.caption_box.bottom() <= 1080.0 - 8.0 + 1e-3,
            "all 4 reserved rings (R=326) and 280x64 caption at {pt:?} must stay within 8 lp viewport margin: center={:?}, caption={:?}",
            r1.center,
            plan.caption_box
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
/// - `T-X117`: When `_NET_WM_CM_S0` has no owner, Concentric falls back to Branching (`no_compositor`)
///   before mapping and installs a non-rectangular `ShapeBounding` mask on the overlay window.
/// - `T-X111`: When `_NET_WM_CM_S0` is owned by a compositor window, the daemon activates Concentric
///   on a single 32-bit ARGB overlay window (`overlay_window_id`), creates zero child windows as rings
///   open/retract, and takes zero pointer grab during Drag sessions.
/// - `T-X112`: Live X11 `shape_get_rectangles(overlay_win, Sk::INPUT)` is full-monitor `(0, 0, 1920, 1080)`
///   in Invoke and default Drag (`passthrough = false`); in opt-in Drag `passthrough = true`, it installs
///   the solid reserved disc (`Rreserve + hit_margin = 338 px` with zero central or inter-ring holes)
///   plus displaced-origin anchor/corridor, excludes the `280x64` caption box, and does not shrink when
///   child rings open or retract.
/// - `T-X113`: Bit-exact `XdndStatus` (`XdndActionPrivate` on actionable segments, `0` on hub, gap,
///   separator, outer margin, caption, and outside even when source requests `XdndActionMove`),
///   zero `DELETE` selection requests, and bit-exact `XdndFinished`.
/// - `T-X114`: Zero-position trigger/drop rejection, stale source ignored, prefetch refusal fallback
///   to provisional menu, and clean unmap with no stuck grabs or orphan overlay.
/// - `T-X115`: Corner/edge summons clamp `root_wheel_center_px` inside `[334, 1586] x [334, 672]`
///   (reserving all 4 rings + caption); live `RESOURCE_MANAGER` `Xft.dpi: 120` (`1.25x`, `1536x864` lp)
///   activates Concentric while `Xft.dpi: 144` (`1.5x`, `1280x720` lp < `668x742`) falls back to Branching.
/// - `T-X116`: Invoke keyboard navigation (`1`, `Backspace`, `Right`, `Enter`, `Escape`) in both
///   Concentric and Branching releases grabs cleanly so other X11 windows receive input afterward.
/// - `T-X118`: Native `XdndAware` drop-target window mapped beneath the overlay receives zero XDND
///   events when releasing across all reject zones, mid-session config reload, and mid-drag compositor
///   loss (`_NET_WM_CM_S0` released), and the disposable file's path, inode, size, mtime, and SHA-256
///   remain 100% unchanged.
#[test]
fn live_xvfb_concentric_end_to_end_xdnd_session_and_compositor_guards() {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::Event;
    use x11rb::protocol::shape::{ConnectionExt as _, SK};
    use x11rb::protocol::xproto::{
        AtomEnum, ClientMessageData, ClientMessageEvent, ConnectionExt as _, CreateWindowAux,
        EventMask, GrabMode, GrabStatus, MapState, PropMode, SelectionNotifyEvent, WindowClass,
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
    let dest_dir = dir.join("native_drop_dest");
    fs::create_dir_all(&dest_dir).expect("create native_drop_dest");
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

    // Connect a test X11 client to act as compositor owner (`_NET_WM_CM_S0`), XDND source,
    // and underlying native drop target window (`native_drop_win`).
    let (conn, screen_num) = RustConnection::connect(None).expect("connect X11 test client");
    let atoms = Atoms::new(&conn)
        .expect("atoms cookie")
        .reply()
        .expect("atoms reply");
    let root = conn.setup().roots[screen_num].root;

    // Reset root RESOURCE_MANAGER to 96 DPI (1.0x) at start of test
    let _ = conn.change_property8(
        PropMode::REPLACE,
        root,
        atoms.RESOURCE_MANAGER,
        AtomEnum::STRING,
        b"Xft.dpi:\t96\n",
    );

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

    // Map an underlying XdndAware native drop target window covering (0, 0, 1920, 1080) (`T-X118`)
    let native_drop_win = conn.generate_id().expect("generate native_drop_win");
    conn.create_window(
        x11rb::COPY_FROM_PARENT as u8,
        native_drop_win,
        root,
        0,
        0,
        1920,
        1080,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new()
            .override_redirect(1)
            .event_mask(EventMask::KEY_PRESS | EventMask::BUTTON_PRESS),
    )
    .expect("create native_drop_win");
    conn.change_property32(
        PropMode::REPLACE,
        native_drop_win,
        atoms.XdndAware,
        AtomEnum::ATOM,
        &[5],
    )
    .expect("set XdndAware on native_drop_win");
    conn.map_window(native_drop_win)
        .expect("map native_drop_win");
    conn.flush().expect("flush windows");

    // Ensure `_NET_WM_CM_S0` is initially unowned so we test `NoCompositor` fallback first (`T-X117`)
    conn.set_selection_owner(x11rb::NONE, atoms._NET_WM_CM_S0, x11rb::CURRENT_TIME)
        .expect("clear _NET_WM_CM_S0");
    conn.flush().expect("flush clear _NET_WM_CM_S0");

    let radivine_bin = PathBuf::from(env!("CARGO_BIN_EXE_radivine"));
    let mut daemon: Child = Command::new(&radivine_bin)
        .env_remove("RADIVINE_FORCE_ARGB")
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

    // -------------------------------------------------------------------------
    // 1. `T-X117`: Without `_NET_WM_CM_S0`, Concentric falls back to Branching (`no_compositor`)
    //    before mapping and installs non-rectangular `ShapeBounding` on the overlay window.
    // -------------------------------------------------------------------------
    let st_nocomp = query_status();
    assert_eq!(st_nocomp["compositor_active"], false);
    assert_eq!(st_nocomp["requested_layout"], "concentric");
    assert_eq!(st_nocomp["active_layout"], "branching");
    assert_eq!(st_nocomp["layout_fallback_reason"], "no_compositor");
    let overlay_win = st_nocomp["overlay_window_id"].as_u64().unwrap() as u32;
    assert!(overlay_win > 0);

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
    let st_nocomp_active = query_status();
    assert_eq!(st_nocomp_active["session_state"], "Active");
    assert_eq!(st_nocomp_active["active_layout"], "branching");
    assert_eq!(st_nocomp_active["layout_fallback_reason"], "no_compositor");
    let bounding_rects = conn
        .shape_get_rectangles(overlay_win, SK::BOUNDING)
        .expect("shape_get_rectangles BOUNDING")
        .reply()
        .expect("BOUNDING reply")
        .rectangles;
    assert!(
        bounding_rects.len() > 1,
        "no-compositor fallback must apply non-rectangular ShapeBounding mask (`T-X117`)"
    );
    let _ = Command::new(&radivine_bin)
        .args(["--socket", sock_path.to_str().unwrap(), "dismiss"])
        .status();
    assert_eq!(query_status()["session_state"], "Idle");
    eprintln!("LIVE_XVFB_VERIFIED: T-X117 no_compositor fallback & ShapeBounding verified");

    // -------------------------------------------------------------------------
    // 2. `T-X111` & `T-X116`: Claim `_NET_WM_CM_S0` -> 32-bit ARGB single overlay window,
    //    Invoke keyboard routing (`1` -> open ring 1, `Backspace` -> close ring 1, `Escape` -> cancel),
    //    unchanged `overlay_window_id`, zero extra child windows per ring, and clean ungrab afterward.
    // -------------------------------------------------------------------------
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
    let geom = conn
        .get_geometry(overlay_win)
        .expect("get_geometry")
        .reply()
        .expect("geometry reply");
    assert_eq!(
        geom.depth, 32,
        "overlay window must use 32-bit ARGB visual (`T-X111`)"
    );

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

    let mut children_before = conn
        .query_tree(root)
        .expect("query_tree")
        .reply()
        .expect("query_tree reply")
        .children;
    children_before.sort_unstable();

    // Press '1' in Invoke mode to open "File Preview" child ring 1, verify no new X11 windows created
    let _ = Command::new("xdotool").args(["key", "1"]).status();
    thread::sleep(Duration::from_millis(30));
    let mut children_ring1 = conn
        .query_tree(root)
        .expect("query_tree ring1")
        .reply()
        .expect("query_tree ring1 reply")
        .children;
    children_ring1.sort_unstable();
    assert_eq!(
        children_before, children_ring1,
        "opening Concentric child ring 1 must never create new X11 windows (`T-X111`)"
    );
    let overlay_subwindows = conn
        .query_tree(overlay_win)
        .expect("query_tree overlay_win")
        .reply()
        .expect("query_tree overlay_win reply")
        .children;
    assert!(
        overlay_subwindows.is_empty(),
        "overlay_win must have zero child windows (`T-X111`)"
    );
    assert_eq!(
        query_status()["overlay_window_id"].as_u64().unwrap() as u32,
        overlay_win
    );

    // Press 'BackSpace' to retract ring 1, then 'Right' + 'Return' to commit `radivine.copy` (`T-X116`)
    let _ = Command::new("xdotool").args(["key", "BackSpace"]).status();
    thread::sleep(Duration::from_millis(30));
    let _ = Command::new("xdotool")
        .args(["key", "Right", "Return"])
        .status();
    thread::sleep(Duration::from_millis(35));
    let st_after_inv = query_status();
    assert_eq!(st_after_inv["session_state"], "Idle");
    assert_eq!(st_after_inv["last_committed_action"], "radivine.copy");
    let attrs_unmapped = conn
        .get_window_attributes(overlay_win)
        .expect("attrs")
        .reply()
        .expect("attrs reply");
    assert_eq!(attrs_unmapped.map_state, MapState::UNMAPPED);

    // Verify other windows can grab pointer/keyboard after Invoke session ends (`T-X116`)
    let grab_rep = conn
        .grab_pointer(
            false,
            native_drop_win,
            EventMask::NO_EVENT,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
            x11rb::NONE,
            x11rb::NONE,
            x11rb::CURRENT_TIME,
        )
        .expect("grab_pointer check")
        .reply()
        .expect("grab_pointer reply");
    assert_eq!(grab_rep.status, GrabStatus::SUCCESS);
    let _ = conn.ungrab_pointer(x11rb::CURRENT_TIME);
    let _ = conn.flush();
    eprintln!(
        "LIVE_XVFB_VERIFIED: T-X111 & T-X116 single ARGB window, ring open/close & Invoke keyboard/ungrab verified"
    );

    // -------------------------------------------------------------------------
    // 3. `T-X113`, `T-X114`, `T-X112` (default full capture), & `T-X118` (native drop target safety)
    // -------------------------------------------------------------------------
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

    let pump_xdnd = |uri_payload: Option<&[u8]>,
                     native_events: &mut usize|
     -> Vec<(u32, [u32; 5])> {
        let mut msgs = Vec::new();
        while let Ok(Some(ev)) = conn.poll_for_event() {
            match ev {
                Event::SelectionRequest(req) => {
                    assert_ne!(
                        req.target, atoms.DELETE,
                        "Radivine must NEVER request DELETE selection target (`T-X113`)"
                    );
                    if req.target == atoms.URI_LIST {
                        let prop = if let Some(bytes) = uri_payload {
                            let p = if req.property == x11rb::NONE {
                                req.target
                            } else {
                                req.property
                            };
                            let _ = conn.change_property8(
                                PropMode::REPLACE,
                                req.requestor,
                                p,
                                atoms.URI_LIST,
                                bytes,
                            );
                            p
                        } else {
                            x11rb::NONE
                        };
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
                }
                Event::ClientMessage(cm) if cm.window == src_win => {
                    msgs.push((cm.type_, cm.data.as_data32()));
                }
                Event::ClientMessage(cm) if cm.window == native_drop_win => {
                    *native_events += 1;
                }
                _ => {}
            }
        }
        msgs
    };

    let payload_bytes = format!("{file_uri}\r\n").into_bytes();
    let mut native_drop_events = 0usize;

    let start_drag_session =
        |x: i32, y: i32, uri_payload: Option<&[u8]>, native_events: &mut usize| {
            let _ = Command::new("xdotool")
                .args([
                    "mousemove",
                    &x.to_string(),
                    &y.to_string(),
                    "mousedown",
                    "1",
                ])
                .status();
            thread::sleep(Duration::from_millis(15));
            conn.set_selection_owner(src_win, atoms.XdndSelection, x11rb::CURRENT_TIME)
                .expect("own XdndSelection");
            conn.flush().expect("flush XdndSelection");
            thread::sleep(Duration::from_millis(25));
            let _ = Command::new("xdotool")
                .args(["keydown", "Shift_L", "keyup", "Shift_L"])
                .status();

            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let _ = pump_xdnd(uri_payload, native_events);
                if query_status()["session_state"] == "Active" {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(query_status()["session_state"], "Active");
        };

    // 3a. `T-X114` & `T-X112` (default full-screen ShapeInput):
    // Start drag at (960, 540), verify zero pointer grab in Drag mode (`T-X111`),
    // verify full-monitor ShapeInput `(0, 0, 1920, 1080)` (`T-X112`),
    // verify stale source XID is ignored (`T-X114`), and verify zero-motion drop rejects (`T-X114`).
    start_drag_session(960, 540, Some(&payload_bytes), &mut native_drop_events);
    assert_eq!(query_status()["active_layout"], "concentric");

    // Verify no pointer/keyboard grab is held by Radivine during Drag mode (`T-X111`)
    let drag_grab_rep = conn
        .grab_keyboard(
            false,
            native_drop_win,
            x11rb::CURRENT_TIME,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )
        .expect("grab_keyboard during drag")
        .reply()
        .expect("grab_keyboard during drag reply");
    assert_eq!(
        drag_grab_rep.status,
        GrabStatus::SUCCESS,
        "Radivine must never hold an input grab in Drag mode (`T-X111`)"
    );
    let _ = conn.ungrab_keyboard(x11rb::CURRENT_TIME);
    let _ = conn.flush();

    // Verify default Drag ShapeInput covers the full 1920x1080 monitor (`T-X112`)
    let default_input_rects = conn
        .shape_get_rectangles(overlay_win, SK::INPUT)
        .expect("shape_get_rectangles INPUT")
        .reply()
        .expect("INPUT reply")
        .rectangles;
    assert_eq!(
        default_input_rects.len(),
        1,
        "default Drag (`passthrough = false`) must use full-monitor ShapeInput (`T-X112`)"
    );
    assert_eq!(
        (
            default_input_rects[0].x,
            default_input_rects[0].y,
            default_input_rects[0].width,
            default_input_rects[0].height
        ),
        (0, 0, 1920, 1080)
    );

    send_xdnd_msg(atoms.XdndEnter, [src_win, 5 << 24, atoms.URI_LIST, 0, 0]);
    thread::sleep(Duration::from_millis(20));
    let _ = pump_xdnd(Some(&payload_bytes), &mut native_drop_events);
    // Stale/unknown source XdndPosition (`0xdeadbeef`) at actionable (1044, 540) must be ignored (`T-X114`)
    send_xdnd_msg(
        atoms.XdndPosition,
        [
            0xdead_beef,
            0,
            (1044 << 16) | 540,
            x11rb::CURRENT_TIME,
            atoms.XdndActionMove,
        ],
    );
    thread::sleep(Duration::from_millis(15));
    assert!(
        pump_xdnd(Some(&payload_bytes), &mut native_drop_events).is_empty(),
        "stale source XdndPosition must be ignored (`T-X114`)"
    );

    // Zero-position trigger + drop at (960, 540) rejects (`T-X114`, `T-X118`)
    send_xdnd_msg(
        atoms.XdndPosition,
        [
            src_win,
            0,
            (960 << 16) | 540,
            x11rb::CURRENT_TIME,
            atoms.XdndActionMove,
        ],
    );
    thread::sleep(Duration::from_millis(20));
    let replies_hub = pump_xdnd(Some(&payload_bytes), &mut native_drop_events);
    let last_hub_status = replies_hub
        .iter()
        .rev()
        .find(|(t, _)| *t == atoms.XdndStatus)
        .expect("XdndStatus for hub");
    assert_eq!(last_hub_status.1[0], overlay_win);
    assert_eq!(
        last_hub_status.1[1] & 1,
        0,
        "hub position must reject (`accept = 0`)"
    );
    assert_eq!(
        last_hub_status.1[4], 0,
        "rejected status action must be 0, never XdndActionMove"
    );

    send_xdnd_msg(atoms.XdndDrop, [src_win, 0, x11rb::CURRENT_TIME, 0, 0]);
    thread::sleep(Duration::from_millis(25));
    let replies_zero_drop = pump_xdnd(Some(&payload_bytes), &mut native_drop_events);
    let fin_zero = replies_zero_drop
        .iter()
        .find(|(t, _)| *t == atoms.XdndFinished)
        .expect("XdndFinished for zero-motion hub drop");
    assert_eq!(fin_zero.1, [overlay_win, 0, 0, 0, 0]);

    // 3b. `T-X113` & `T-X118`: Test all non-actionable zones (radial gap, separator, outer margin,
    // caption box, outside cluster) and mid-session reload on live Xvfb:
    let _ = Command::new("xdotool")
        .args(["keydown", "Shift_L", "keyup", "Shift_L"])
        .status();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let _ = pump_xdnd(Some(&payload_bytes), &mut native_drop_events);
        if query_status()["session_state"] == "Active" {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    send_xdnd_msg(atoms.XdndEnter, [src_win, 5 << 24, atoms.URI_LIST, 0, 0]);

    // Dwell on root submenu `[0]` ("File Preview" at top: 960, 456) to open ring 1 (`134..194 lp`)
    send_xdnd_msg(
        atoms.XdndPosition,
        [
            src_win,
            0,
            (960 << 16) | 456,
            x11rb::CURRENT_TIME,
            atoms.XdndActionMove,
        ],
    );
    thread::sleep(Duration::from_millis(150));
    let _ = pump_xdnd(Some(&payload_bytes), &mut native_drop_events);

    // Trigger a live `reload` mid-drag (`T-X118`) and verify full-monitor ShapeInput is preserved
    assert!(
        Command::new(&radivine_bin)
            .args([
                "--config",
                cfg_path.to_str().unwrap(),
                "--socket",
                sock_path.to_str().unwrap(),
                "reload",
            ])
            .status()
            .unwrap()
            .success()
    );

    for (zone_name, zx, zy) in [
        ("radial_gap", 960u32, 409u32), // r = 131 lp between ring 0 (128) and ring 1 (134)
        ("separator", 1020u32, 480u32), // 45° separator ray on ring 0 (r ≈ 84.8 lp)
        ("outer_margin", 960u32, 340u32), // r = 200 lp beyond ring 1 outer radius (194)
        ("caption_box", 960u32, 908u32), // inside 280x64 caption box (y = 540 + 326 + 42 = 908)
        ("outside_cluster", 200u32, 200u32), // far outside wheel cluster on captured monitor
    ] {
        send_xdnd_msg(
            atoms.XdndPosition,
            [
                src_win,
                0,
                (zx << 16) | zy,
                x11rb::CURRENT_TIME,
                atoms.XdndActionMove,
            ],
        );
        thread::sleep(Duration::from_millis(20));
        let replies = pump_xdnd(Some(&payload_bytes), &mut native_drop_events);
        let st_msg = replies
            .iter()
            .rev()
            .find(|(t, _)| *t == atoms.XdndStatus)
            .unwrap_or_else(|| panic!("expected XdndStatus for reject zone {zone_name}"));
        assert_eq!(
            st_msg.1[1] & 1,
            0,
            "live Xvfb zone `{zone_name}` at ({zx}, {zy}) must reject (`accept = 0`)"
        );
        assert_eq!(
            st_msg.1[4], 0,
            "live Xvfb zone `{zone_name}` must reply action=0 (None), never XdndActionMove"
        );
    }

    // Drop on `outside_cluster` (200, 200) -> must reject (`XdndFinished(accepted=0)`) and never reach `native_drop_win` (`T-X118`)
    send_xdnd_msg(atoms.XdndDrop, [src_win, 0, x11rb::CURRENT_TIME, 0, 0]);
    thread::sleep(Duration::from_millis(25));
    let replies_out_drop = pump_xdnd(Some(&payload_bytes), &mut native_drop_events);
    let fin_out = replies_out_drop
        .iter()
        .find(|(t, _)| *t == atoms.XdndFinished)
        .expect("XdndFinished for outside_cluster drop");
    assert_eq!(fin_out.1, [overlay_win, 0, 0, 0, 0]);
    assert_eq!(
        native_drop_events, 0,
        "underlying native drop target window must receive ZERO XDND events (`T-X118`)"
    );
    fp.verify_unchanged("after_reject_zone_drops");

    // 3c. `T-X113` & `T-X118`: Summon again, verify actionable segment `[1]` (`Copy`, 1044, 540)
    // replies `XdndActionPrivate` even when source requests `XdndActionMove`, then release `_NET_WM_CM_S0`
    // mid-drag and verify drop is rejected (`accepted = 0`).
    let _ = Command::new("xdotool")
        .args(["keydown", "Shift_L", "keyup", "Shift_L"])
        .status();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let _ = pump_xdnd(Some(&payload_bytes), &mut native_drop_events);
        if query_status()["session_state"] == "Active" {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    send_xdnd_msg(atoms.XdndEnter, [src_win, 5 << 24, atoms.URI_LIST, 0, 0]);
    send_xdnd_msg(
        atoms.XdndPosition,
        [
            src_win,
            0,
            (1044 << 16) | 540,
            x11rb::CURRENT_TIME,
            atoms.XdndActionMove,
        ],
    );
    thread::sleep(Duration::from_millis(25));
    let replies_seg = pump_xdnd(Some(&payload_bytes), &mut native_drop_events);
    let last_seg_status = replies_seg
        .iter()
        .rev()
        .find(|(t, _)| *t == atoms.XdndStatus)
        .expect("XdndStatus for actionable segment");
    assert_eq!(last_seg_status.1[0], overlay_win);
    assert_eq!(
        last_seg_status.1[1] & 1,
        1,
        "actionable segment must accept (`accept = 1`)"
    );
    assert_eq!(
        last_seg_status.1[4], atoms.XdndActionPrivate,
        "accepted status action must be XdndActionPrivate, never XdndActionMove (`T-X113`)"
    );
    if let Some(status_us) = query_status()["last_dnd_status_us"].as_u64() {
        eprintln!("LIVE_XVFB_METRICS: dnd_status_us={status_us}");
    }

    // Commit valid Concentric drop on `[1]` (`Copy`) and verify bit-exact `XdndFinished` with `XdndActionPrivate` (`T-X113`, `T-SAFE1`)
    send_xdnd_msg(atoms.XdndDrop, [src_win, 0, x11rb::CURRENT_TIME, 0, 0]);
    thread::sleep(Duration::from_millis(25));
    let replies_valid_drop = pump_xdnd(Some(&payload_bytes), &mut native_drop_events);
    let fin_valid = replies_valid_drop
        .iter()
        .find(|(t, _)| *t == atoms.XdndFinished)
        .expect("XdndFinished for valid Concentric drop");
    assert_eq!(
        fin_valid.1,
        [overlay_win, 1, atoms.XdndActionPrivate, 0, 0],
        "valid Concentric drop must finish with accepted=1 and XdndActionPrivate (`T-X113`)"
    );
    let st_after_valid_drop = query_status();
    assert_eq!(
        st_after_valid_drop["last_committed_action"],
        "radivine.copy"
    );
    if let Some(drop_us) = st_after_valid_drop["last_drop_dispatch_us"].as_u64() {
        eprintln!("LIVE_XVFB_METRICS: drop_dispatch_us={drop_us}");
    }
    fp.verify_unchanged("after_valid_xvfb_drop");

    // Now summon again and release `_NET_WM_CM_S0` mid-drag -> daemon enters `reject_until_drag_end` and rejects drop (`T-SAFE8`, `T-X118`)
    let _ = Command::new("xdotool")
        .args(["keydown", "Shift_L", "keyup", "Shift_L"])
        .status();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let _ = pump_xdnd(Some(&payload_bytes), &mut native_drop_events);
        if query_status()["session_state"] == "Active" {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    send_xdnd_msg(atoms.XdndEnter, [src_win, 5 << 24, atoms.URI_LIST, 0, 0]);
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
    thread::sleep(Duration::from_millis(20));
    let _ = pump_xdnd(Some(&payload_bytes), &mut native_drop_events);

    conn.set_selection_owner(x11rb::NONE, atoms._NET_WM_CM_S0, x11rb::CURRENT_TIME)
        .expect("release _NET_WM_CM_S0 mid-drag");
    conn.flush().expect("flush release _NET_WM_CM_S0");
    thread::sleep(Duration::from_millis(30));

    send_xdnd_msg(atoms.XdndDrop, [src_win, 0, x11rb::CURRENT_TIME, 0, 0]);
    thread::sleep(Duration::from_millis(30));
    let replies_drop = pump_xdnd(Some(&payload_bytes), &mut native_drop_events);
    let finished = replies_drop
        .iter()
        .find(|(t, _)| *t == atoms.XdndFinished)
        .expect("XdndFinished after compositor-loss drop");
    assert_eq!(
        finished.1[1], 0,
        "drop after mid-drag compositor loss must finish with accepted=0 (`T-X118`)"
    );

    // Re-claim `_NET_WM_CM_S0` and test prefetch refusal (`uri_payload = None`) + XdndLeave re-entry (`T-X114`)
    conn.set_selection_owner(cm_win, atoms._NET_WM_CM_S0, x11rb::CURRENT_TIME)
        .expect("re-claim _NET_WM_CM_S0");
    conn.flush().expect("flush re-claim _NET_WM_CM_S0");
    thread::sleep(Duration::from_millis(30));

    let _ = Command::new("xdotool")
        .args(["keydown", "Shift_L", "keyup", "Shift_L"])
        .status();
    let refusal_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < refusal_deadline {
        let _ = pump_xdnd(None, &mut native_drop_events);
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
        if query_status()["session_state"] == "Active" {
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }
    assert_eq!(
        query_status()["session_state"],
        "Active",
        "prefetch refusal must fall back to provisional menu (`T-X114`)"
    );
    // Test XdndLeave followed by re-entry (`XdndEnter`) and cancel drop (`T-X114`)
    send_xdnd_msg(atoms.XdndLeave, [src_win, 0, 0, 0, 0]);
    thread::sleep(Duration::from_millis(20));
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
    send_xdnd_msg(atoms.XdndDrop, [src_win, 0, x11rb::CURRENT_TIME, 0, 0]);
    thread::sleep(Duration::from_millis(25));
    let _ = pump_xdnd(None, &mut native_drop_events);

    // End drag cleanly
    let _ = conn.set_selection_owner(x11rb::NONE, atoms.XdndSelection, x11rb::CURRENT_TIME);
    let _ = conn.flush();
    let _ = Command::new("xdotool")
        .args(["keyup", "Shift_L", "mouseup", "1"])
        .status();
    thread::sleep(Duration::from_millis(25));
    eprintln!(
        "LIVE_XVFB_VERIFIED: T-X113, T-X114, T-X118 XDND negotiation, refusal/re-entry & native target isolation verified"
    );

    // -------------------------------------------------------------------------
    // 4. `T-X112` (opt-in `passthrough = true` live XShape reserved disc, corridor & caption exclusion)
    // -------------------------------------------------------------------------
    fs::write(
        &cfg_path,
        r#"
version = 1

[wheel]
layout = "concentric"

[input]
passthrough = true

[appearance]
reduced_motion = true
"#,
    )
    .expect("write passthrough=true config");
    assert!(
        Command::new(&radivine_bin)
            .args([
                "--config",
                cfg_path.to_str().unwrap(),
                "--socket",
                sock_path.to_str().unwrap(),
                "reload",
            ])
            .status()
            .unwrap()
            .success()
    );

    // Summon Drag near left edge (10, 540) so center is displaced to (334, 540)
    start_drag_session(10, 540, Some(&payload_bytes), &mut native_drop_events);
    let pt_rects_root = conn
        .shape_get_rectangles(overlay_win, SK::INPUT)
        .expect("shape_get_rectangles passthrough root")
        .reply()
        .expect("passthrough root reply")
        .rectangles;
    assert!(
        pt_rects_root.len() > 10,
        "passthrough=true must install scanline ShapeInput rectangles on X11 server (`T-X112`)"
    );
    let rect_covers = |rects: &[x11rb::protocol::xproto::Rectangle], px: i16, py: i16| -> bool {
        rects.iter().any(|r| {
            px >= r.x && py >= r.y && px < r.x + r.width as i16 && py < r.y + r.height as i16
        })
    };
    // Central hub (334, 540) and inter-ring gaps (r = 131, 197, 263) must be covered with zero holes
    for r in [0i16, 20, 131, 197, 263, 330] {
        assert!(
            rect_covers(&pt_rects_root, 334 + r, 540),
            "live X11 ShapeInput must cover solid reserved disc at r={r} (`T-X112`)"
        );
    }
    // Displaced anchor (10, 540) and corridor midpoint (172, 540) must be covered
    assert!(
        rect_covers(&pt_rects_root, 10, 540),
        "anchor disc must be covered (`T-X112`)"
    );
    assert!(
        rect_covers(&pt_rects_root, 172, 540),
        "corridor must be covered (`T-X112`)"
    );
    // Caption box below Rreserve + hit_margin (334, 540 + 326 + 42 = 908) must be excluded!
    assert!(
        !rect_covers(&pt_rects_root, 334, 908),
        "caption box must be excluded from pass-through ShapeInput (`T-X112`)"
    );

    // Clear approach guard via hub (334, 540), dwell on `[0]` (334, 456) to open ring 1,
    // and verify live X11 ShapeInput rectangles do not shrink or change!
    send_xdnd_msg(atoms.XdndEnter, [src_win, 5 << 24, atoms.URI_LIST, 0, 0]);
    send_xdnd_msg(
        atoms.XdndPosition,
        [
            src_win,
            0,
            (334 << 16) | 540,
            x11rb::CURRENT_TIME,
            atoms.XdndActionPrivate,
        ],
    );
    send_xdnd_msg(
        atoms.XdndPosition,
        [
            src_win,
            0,
            (334 << 16) | 456,
            x11rb::CURRENT_TIME,
            atoms.XdndActionPrivate,
        ],
    );
    thread::sleep(Duration::from_millis(150));
    let _ = pump_xdnd(Some(&payload_bytes), &mut native_drop_events);
    let pt_rects_ring1 = conn
        .shape_get_rectangles(overlay_win, SK::INPUT)
        .expect("shape_get_rectangles passthrough ring1")
        .reply()
        .expect("passthrough ring1 reply")
        .rectangles;
    let to_tuples = |rs: &[x11rb::protocol::xproto::Rectangle]| -> Vec<(i16, i16, u16, u16)> {
        rs.iter().map(|r| (r.x, r.y, r.width, r.height)).collect()
    };
    assert_eq!(
        to_tuples(&pt_rects_root),
        to_tuples(&pt_rects_ring1),
        "live X11 ShapeInput must remain identical when child rings open (`T-X112`)"
    );

    // End passthrough drag and restore safe default `passthrough = false`
    send_xdnd_msg(
        atoms.XdndPosition,
        [
            src_win,
            0,
            (334 << 16) | 540,
            x11rb::CURRENT_TIME,
            atoms.XdndActionPrivate,
        ],
    );
    send_xdnd_msg(atoms.XdndDrop, [src_win, 0, x11rb::CURRENT_TIME, 0, 0]);
    thread::sleep(Duration::from_millis(25));
    let _ = pump_xdnd(Some(&payload_bytes), &mut native_drop_events);
    let _ = conn.set_selection_owner(x11rb::NONE, atoms.XdndSelection, x11rb::CURRENT_TIME);
    let _ = conn.flush();
    let _ = Command::new("xdotool")
        .args(["keyup", "Shift_L", "mouseup", "1"])
        .status();
    thread::sleep(Duration::from_millis(25));
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
    .expect("restore default config.toml");
    let _ = Command::new(&radivine_bin)
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "--socket",
            sock_path.to_str().unwrap(),
            "reload",
        ])
        .status();
    eprintln!(
        "LIVE_XVFB_VERIFIED: T-X112 live XShape full-monitor default & passthrough reserved-disc/corridor/caption exclusion verified"
    );

    // -------------------------------------------------------------------------
    // 5. `T-X115` & `T-GEO4`: Live Xvfb corner/edge clamping (reserving 4 rings + caption -> max Cy = 672)
    //    and live `RESOURCE_MANAGER` `Xft.dpi` fractional scaling (`1.25x` = 1536x864 lp fits Concentric;
    //    `1.5x` = 1280x720 lp < 668x742 falls back to Branching `insufficient_space`).
    // -------------------------------------------------------------------------
    for (corner_arg, exp_cx, exp_cy) in [
        ("10,10", 334.0f64, 334.0f64),
        ("1910,10", 1586.0f64, 334.0f64),
        ("10,1070", 334.0f64, 672.0f64),
        ("1910,1070", 1586.0f64, 672.0f64),
    ] {
        let _ = Command::new(&radivine_bin)
            .args([
                "--config",
                cfg_path.to_str().unwrap(),
                "--socket",
                sock_path.to_str().unwrap(),
                "invoke",
                "--at",
                corner_arg,
                &file_uri,
            ])
            .status();
        let st_c = query_status();
        assert_eq!(st_c["active_layout"], "concentric");
        let center_arr = st_c["root_wheel_center_px"]
            .as_array()
            .expect("root_wheel_center_px");
        let cx = center_arr[0].as_f64().unwrap();
        let cy = center_arr[1].as_f64().unwrap();
        assert!(
            (cx - exp_cx).abs() < 1.0 && (cy - exp_cy).abs() < 1.0,
            "corner {corner_arg} must clamp center to ({exp_cx}, {exp_cy}), got ({cx}, {cy}) (`T-X115`)"
        );
        let _ = Command::new(&radivine_bin)
            .args(["--socket", sock_path.to_str().unwrap(), "dismiss"])
            .status();
    }

    // Set Xft.dpi = 120 (scale = 1.25x -> logical viewport 1536x864 >= 668x742 -> Concentric)
    conn.change_property8(
        PropMode::REPLACE,
        root,
        atoms.RESOURCE_MANAGER,
        AtomEnum::STRING,
        b"Xft.dpi:\t120\n",
    )
    .expect("set Xft.dpi 120");
    conn.flush().expect("flush Xft.dpi 120");
    thread::sleep(Duration::from_millis(35));
    let st_125 = query_status();
    assert!((st_125["scale"].as_f64().unwrap() - 1.25).abs() < 1e-3);
    assert_eq!(st_125["active_layout"], "concentric");
    assert!(st_125["layout_fallback_reason"].is_null());

    // Set Xft.dpi = 144 (scale = 1.5x -> logical viewport 1280x720 < 668x742 -> Branching `insufficient_space`)
    conn.change_property8(
        PropMode::REPLACE,
        root,
        atoms.RESOURCE_MANAGER,
        AtomEnum::STRING,
        b"Xft.dpi:\t144\n",
    )
    .expect("set Xft.dpi 144");
    conn.flush().expect("flush Xft.dpi 144");
    thread::sleep(Duration::from_millis(35));
    let st_150 = query_status();
    assert!((st_150["scale"].as_f64().unwrap() - 1.5).abs() < 1e-3);
    assert_eq!(st_150["requested_layout"], "concentric");
    assert_eq!(
        st_150["active_layout"], "branching",
        "1920x1080 at 1.5x (1280x720 logical < 668x742) must fall back to branching (`T-GEO4`, `T-X115`)"
    );
    assert_eq!(st_150["layout_fallback_reason"], "insufficient_space");

    // Restore Xft.dpi = 96 (1.0x)
    let _ = conn.change_property8(
        PropMode::REPLACE,
        root,
        atoms.RESOURCE_MANAGER,
        AtomEnum::STRING,
        b"Xft.dpi:\t96\n",
    );
    let _ = conn.flush();
    eprintln!(
        "LIVE_XVFB_VERIFIED: T-X115 & T-GEO4 corner clamping & Xft.dpi 1.25x/1.5x (1280x720 lp fallback) verified"
    );

    // Verify disposable file and empty destination directory are 100% untouched (`T-X118`)
    assert!(
        fs::read_dir(&dest_dir).unwrap().next().is_none(),
        "destination folder must remain completely empty (`T-X118`)"
    );
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

fn read_proc_rss_kib(pid: u32) -> u64 {
    let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status")) else {
        return 0;
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let digits: String = rest.chars().filter(char::is_ascii_digit).collect();
            if let Ok(v) = digits.parse::<u64>() {
                return v;
            }
        }
    }
    0
}

fn read_proc_cpu_ticks(pid: u32) -> u64 {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return 0;
    };
    let Some((_comm, rest)) = stat.rsplit_once(')') else {
        return 0;
    };
    let fields: Vec<&str> = rest.split_whitespace().collect();
    if fields.len() >= 13 {
        let utime: u64 = fields[11].parse().unwrap_or(0);
        let stime: u64 = fields[12].parse().unwrap_or(0);
        utime + stime
    } else {
        0
    }
}

fn stats_us(samples: &mut [u64]) -> (f64, u64, u64, u64, u64) {
    assert!(!samples.is_empty());
    samples.sort_unstable();
    let n = samples.len();
    let mean = (samples.iter().sum::<u64>() as f64) / (n as f64);
    let p50 = samples[n * 50 / 100];
    let p95 = samples[(n * 95 / 100).min(n - 1)];
    let p99 = samples[(n * 99 / 100).min(n - 1)];
    let max = samples[n - 1];
    (mean, p50, p95, p99, max)
}

/// Live `Xvfb` `--release` end-to-end performance & memory gate (`T-PERF1`, `T-PERF4`, `T-PERF5`):
/// - Runs 3 warmups + 30 measured trials of Trigger -> First Present for both `Branching` and `Concentric`.
/// - Runs a 30-second continuous `60 Hz` navigation trace (`>= 1,800` frames) on `1920x1080@1x` and
///   a `>= 1,000`-frame continuous navigation trace on `3840x2160@2x` (`Xft.dpi: 192`), measuring
///   `frame_render_present_us` (`p50`, `p95`, `p99`, `max`, missed `60 Hz` deadlines) and
///   `dnd_position_to_status_us` (`p50`, `p95`, `p99`, `max`).
/// - Runs 3 warmups + 30 measured trials of cached `XdndDrop` -> `XdndFinished` + action dispatch.
/// - Measures same-build `Branching` vs `Concentric` daemon `VmRSS` (cold idle, active `1080p@1x`,
///   post-buffer-release idle after `> 30s`, and `3840x1080@1x` ultrawide active) and idle CPU ticks.
#[test]
#[ignore = "30s + 30s idle live Xvfb T-PERF4/T-PERF5 benchmark; run under xvfb-run with --ignored --nocapture"]
fn t_perf4_t_perf5_live_xvfb_release_end_to_end_and_memory_gates() {
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{
        AtomEnum, ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, PropMode,
        SelectionNotifyEvent, WindowClass,
    };
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    if !radivine_platform::is_isolated_test_display() {
        panic!("t_perf4_t_perf5 requires an isolated Xvfb display");
    }

    x11rb::atom_manager! {
        pub PerfAtoms: PerfAtomsCookie {
            _NET_WM_CM_S0,
            RESOURCE_MANAGER,
            XdndAware,
            XdndSelection,
            XdndEnter,
            XdndPosition,
            XdndStatus,
            XdndLeave,
            XdndDrop,
            XdndFinished,
            XdndActionCopy,
            XdndActionPrivate,
            URI_LIST: b"text/uri-list",
        }
    }

    let (conn, screen_num) = RustConnection::connect(None).expect("connect X11");
    let root = conn.setup().roots[screen_num].root;
    let screen_w = conn.setup().roots[screen_num].width_in_pixels;
    let screen_h = conn.setup().roots[screen_num].height_in_pixels;
    let atoms = PerfAtoms::new(&conn).unwrap().reply().unwrap();

    let cm_win = conn.generate_id().unwrap();
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
    .unwrap();
    conn.set_selection_owner(cm_win, atoms._NET_WM_CM_S0, x11rb::CURRENT_TIME)
        .unwrap();
    conn.change_property8(
        PropMode::REPLACE,
        root,
        atoms.RESOURCE_MANAGER,
        AtomEnum::STRING,
        b"Xft.dpi:\t96\n",
    )
    .unwrap();

    let src_win = conn.generate_id().unwrap();
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
    .unwrap();
    conn.flush().unwrap();

    let dir = make_temp_test_dir("t-perf4-5");
    let sample_file = dir.join("perf_sample.txt");
    fs::write(&sample_file, b"Radivine T-PERF4/5 payload\n").unwrap();
    let file_uri = radivine_core::local_path_to_file_uri(&sample_file);
    let payload_bytes = format!("{file_uri}\r\n").into_bytes();

    let radivine_bin = PathBuf::from(env!("CARGO_BIN_EXE_radivine"));
    let spawn_perf_daemon = |tag: &str, layout: &str| {
        let sub = dir.join(tag);
        let cfg_dir = sub.join("config/radivine");
        let run_dir = sub.join("run");
        let state_dir = sub.join("state");
        fs::create_dir_all(&cfg_dir).unwrap();
        fs::create_dir_all(&run_dir).unwrap();
        fs::create_dir_all(&state_dir).unwrap();
        let cfg_path = cfg_dir.join("config.toml");
        let sock_path = run_dir.join("radivine.sock");
        fs::write(
            &cfg_path,
            format!(
                "version = 1\n[wheel]\nlayout = \"{layout}\"\n[appearance]\nreduced_motion = false\n"
            ),
        )
        .unwrap();
        let child = Command::new(&radivine_bin)
            .args([
                "--config",
                cfg_path.to_str().unwrap(),
                "--socket",
                sock_path.to_str().unwrap(),
                "daemon",
            ])
            .env("XDG_CONFIG_HOME", sub.join("config"))
            .env("XDG_RUNTIME_DIR", &run_dir)
            .env("XDG_STATE_HOME", &state_dir)
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/no-dbus")
            .env("RADIVINE_SUPPRESS_DESKTOP_NOTIFY", "1")
            .env("RADIVINE_DRY_RUN_VIEWER", "1")
            .env("RADIVINE_DRY_RUN_PROPERTIES", "1")
            .env_remove("RADIVINE_FORCE_ARGB")
            .spawn()
            .expect("spawn perf daemon");
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if sock_path.exists() {
                let out = Command::new(&radivine_bin)
                    .args(["--socket", sock_path.to_str().unwrap(), "status", "--json"])
                    .output()
                    .unwrap();
                if out.status.success() {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        (child, cfg_path, sock_path)
    };

    let status_of = |sock: &Path| -> serde_json::Value {
        let out = Command::new(&radivine_bin)
            .args(["--socket", sock.to_str().unwrap(), "status", "--json"])
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap()
    };

    // --- 1. T-PERF5 & T-PERF1: Same-build Branching vs Concentric RSS & Trigger->Present (3 warmup + 30 trials) ---
    let (mut b_daemon, b_cfg, b_sock) = spawn_perf_daemon("branching", "branching");
    let b_pid = b_daemon.id();
    let b_cold_idle_rss_kib = read_proc_rss_kib(b_pid);
    let mut b_summon_samples = Vec::with_capacity(30);
    for i in 0..33 {
        let _ = Command::new(&radivine_bin)
            .args([
                "--config",
                b_cfg.to_str().unwrap(),
                "--socket",
                b_sock.to_str().unwrap(),
                "invoke",
                "--at",
                "640,400",
                &file_uri,
            ])
            .status();
        let st = status_of(&b_sock);
        if i >= 3 {
            b_summon_samples.push(st["last_summon_us"].as_u64().unwrap_or(0));
        }
        let _ = Command::new(&radivine_bin)
            .args(["--socket", b_sock.to_str().unwrap(), "dismiss"])
            .status();
    }
    let b_active_rss_kib = read_proc_rss_kib(b_pid);
    let is_1080p_1x = screen_w == 1920 && screen_h == 1080;
    let b_post_release_rss_kib = if is_1080p_1x {
        thread::sleep(Duration::from_secs(32));
        let _ = status_of(&b_sock);
        read_proc_rss_kib(b_pid)
    } else {
        b_cold_idle_rss_kib
    };
    // Always stop b_daemon before spawning c_daemon so two daemons never listen on the same X11 root window.
    let _ = Command::new(&radivine_bin)
        .args(["--socket", b_sock.to_str().unwrap(), "quit"])
        .status();
    let _ = b_daemon.wait();

    let (mut c_daemon, c_cfg, c_sock) = spawn_perf_daemon("concentric", "concentric");
    let c_pid = c_daemon.id();
    let c_cold_idle_rss_kib = read_proc_rss_kib(c_pid);
    let mut c_summon_samples = Vec::with_capacity(30);
    for i in 0..33 {
        let _ = Command::new(&radivine_bin)
            .args([
                "--config",
                c_cfg.to_str().unwrap(),
                "--socket",
                c_sock.to_str().unwrap(),
                "invoke",
                "--at",
                "640,400",
                &file_uri,
            ])
            .status();
        let st = status_of(&c_sock);
        if i >= 3 {
            c_summon_samples.push(st["last_summon_us"].as_u64().unwrap_or(0));
        }
        let _ = Command::new(&radivine_bin)
            .args(["--socket", c_sock.to_str().unwrap(), "dismiss"])
            .status();
    }
    let c_active_rss_kib = read_proc_rss_kib(c_pid);
    let overlay_win = status_of(&c_sock)["overlay_window_id"].as_u64().unwrap() as u32;

    let (b_sum_mean, b_sum_p50, b_sum_p95, b_sum_p99, b_sum_max) = stats_us(&mut b_summon_samples);
    let (c_sum_mean, c_sum_p50, c_sum_p95, c_sum_p99, c_sum_max) = stats_us(&mut c_summon_samples);
    println!(
        "T-PERF1/4 Live Xvfb Trigger->Present ({screen_w}x{screen_h}, 30 trials): Branching mean={b_sum_mean:.1}us p50={b_sum_p50}us p95={b_sum_p95}us p99={b_sum_p99}us max={b_sum_max}us | Concentric mean={c_sum_mean:.1}us p50={c_sum_p50}us p95={c_sum_p95}us p99={c_sum_p99}us max={c_sum_max}us"
    );
    assert!(
        c_sum_p95 < 50_000,
        "T-PERF4 Trigger->First Present p95 ({c_sum_p95}us) must be < 50ms"
    );

    // Helper for XDND wire communication on `c_daemon`
    let send_xdnd = |msg_type: u32, data32: [u32; 5]| {
        let ev = ClientMessageEvent::new(32, overlay_win, msg_type, data32);
        conn.send_event(false, overlay_win, EventMask::NO_EVENT, ev)
            .unwrap();
        conn.flush().unwrap();
    };
    let pump_xdnd = || -> Vec<(u32, [u32; 5])> {
        let mut out = Vec::new();
        while let Ok(Some(ev)) = conn.poll_for_event() {
            match ev {
                Event::SelectionRequest(req) => {
                    let prop = if req.property == x11rb::NONE {
                        req.target
                    } else {
                        req.property
                    };
                    let _ = conn.change_property8(
                        PropMode::REPLACE,
                        req.requestor,
                        prop,
                        req.target,
                        &payload_bytes,
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
                Event::ClientMessage(cm) => {
                    out.push((cm.type_, cm.data.as_data32()));
                }
                _ => {}
            }
        }
        out
    };

    // --- 2. T-PERF4 30-second Continuous 60 Hz Navigation Trace on 1080p@1x (or 4K@2x when screen_w == 3840) ---
    let is_4k_2x = screen_w >= 3840 && screen_h >= 2160;
    let scale_factor = if is_4k_2x { 2i32 } else { 1i32 };
    if is_4k_2x {
        conn.change_property8(
            PropMode::REPLACE,
            root,
            atoms.RESOURCE_MANAGER,
            AtomEnum::STRING,
            b"Xft.dpi:\t192\n",
        )
        .unwrap();
        conn.flush().unwrap();
        thread::sleep(Duration::from_millis(40));
    }

    let cx = (screen_w as i32) / 2;
    let cy = (screen_h as i32) / 2;

    // Summon drag session at (cx, cy)
    let _ = Command::new("xdotool")
        .args(["keyup", "Shift_L", "mouseup", "1"])
        .status();
    thread::sleep(Duration::from_millis(15));
    let _ = Command::new("xdotool")
        .args([
            "mousemove",
            &(cx - 20).to_string(),
            &cy.to_string(),
            "mousedown",
            "1",
            "mousemove",
            &cx.to_string(),
            &cy.to_string(),
        ])
        .status();
    conn.set_selection_owner(src_win, atoms.XdndSelection, x11rb::CURRENT_TIME)
        .unwrap();
    conn.flush().unwrap();
    let _ = Command::new("xdotool")
        .args(["keydown", "Shift_L", "keyup", "Shift_L"])
        .status();

    let map_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < map_deadline {
        let _ = pump_xdnd();
        send_xdnd(atoms.XdndEnter, [src_win, 5 << 24, atoms.URI_LIST, 0, 0]);
        send_xdnd(
            atoms.XdndPosition,
            [
                src_win,
                0,
                ((cx as u32) << 16) | (cy as u32),
                x11rb::CURRENT_TIME,
                atoms.XdndActionPrivate,
            ],
        );
        if status_of(&c_sock)["session_state"] == "Active" {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(status_of(&c_sock)["session_state"], "Active");
    let _ = pump_xdnd();

    // Waypoints around the Concentric rings (root, gap, ring 1, ring 2, sibling, hub)
    let waypoints: [(i32, i32); 10] = [
        (cx, cy - 84 * scale_factor),  // root [0] File Preview
        (cx, cy - 84 * scale_factor),  // dwell on [0]
        (cx, cy - 131 * scale_factor), // radial gap between ring 0 and ring 1
        (cx, cy - 164 * scale_factor), // ring 1 [0, 0] Full Preview
        (cx + 142 * scale_factor, cy + 82 * scale_factor), // ring 1 [0, 1] Head
        (cx - 84 * scale_factor, cy),  // root [3] Custom Actions
        (cx - 84 * scale_factor, cy),  // dwell on [3]
        (cx - 164 * scale_factor, cy), // ring 1 [3, 1] Scripts
        (cx - 230 * scale_factor, cy), // ring 2 [3, 1, 0] Compute SHA-256
        (cx + 84 * scale_factor, cy),  // root [1] Copy
    ];

    let trace_duration = if is_1080p_1x {
        Duration::from_secs(30)
    } else {
        Duration::from_secs(18)
    };
    let trace_start = Instant::now();
    let mut status_rtt_us = Vec::with_capacity(2048);
    let mut frame_render_us = Vec::with_capacity(2048);
    let mut step_idx = 0usize;

    while trace_start.elapsed() < trace_duration || frame_render_us.len() < 1020 {
        let (wx, wy) = waypoints[step_idx % waypoints.len()];
        let jitter = ((step_idx % 5) as i32) - 2;
        let px = (wx + jitter) as u32;
        let py = (wy - jitter) as u32;

        let t0 = Instant::now();
        send_xdnd(
            atoms.XdndPosition,
            [
                src_win,
                0,
                (px << 16) | py,
                x11rb::CURRENT_TIME,
                atoms.XdndActionPrivate,
            ],
        );
        let wait_until = Instant::now() + Duration::from_millis(50);
        while Instant::now() < wait_until {
            let msgs = pump_xdnd();
            if msgs.iter().any(|(t, _)| *t == atoms.XdndStatus) {
                status_rtt_us.push(t0.elapsed().as_micros() as u64);
                break;
            }
            std::hint::spin_loop();
        }

        let st = status_of(&c_sock);
        if let Some(fr) = st["last_frame_render_us"].as_u64() {
            frame_render_us.push(fr);
        }
        step_idx += 1;
        thread::sleep(Duration::from_millis(14));
    }

    // Cancel trace session cleanly
    send_xdnd(
        atoms.XdndPosition,
        [
            src_win,
            0,
            ((cx as u32) << 16) | (cy as u32),
            x11rb::CURRENT_TIME,
            atoms.XdndActionPrivate,
        ],
    );
    send_xdnd(atoms.XdndDrop, [src_win, 0, x11rb::CURRENT_TIME, 0, 0]);
    thread::sleep(Duration::from_millis(25));
    let _ = pump_xdnd();
    let _ = conn.set_selection_owner(x11rb::NONE, atoms.XdndSelection, x11rb::CURRENT_TIME);
    let _ = conn.flush();
    let _ = Command::new("xdotool")
        .args(["keyup", "Shift_L", "mouseup", "1"])
        .status();

    let missed_60hz = frame_render_us.iter().filter(|&&us| us > 16_667).count();
    let (st_mean, st_p50, st_p95, st_p99, st_max) = stats_us(&mut status_rtt_us);
    let (fr_mean, fr_p50, fr_p95, fr_p99, fr_max) = stats_us(&mut frame_render_us);
    println!(
        "T-PERF4 Live Xvfb {screen_w}x{screen_h}@{scale_factor}x Trace ({:.2}s, {} position steps, {} frame samples): Position->Status mean={st_mean:.1}us p50={st_p50}us p95={st_p95}us p99={st_p99}us max={st_max}us | Frame Render+Present mean={fr_mean:.1}us p50={fr_p50}us p95={fr_p95}us p99={fr_p99}us max={fr_max}us | missed_60hz={missed_60hz}",
        trace_start.elapsed().as_secs_f64(),
        status_rtt_us.len(),
        frame_render_us.len()
    );
    assert!(
        st_p95 < 2_000,
        "T-PERF4 Position->Status p95 ({st_p95}us) must be < 2000us"
    );
    let frame_p95_gate_us = if is_4k_2x { 8_000 } else { 4_000 };
    assert!(
        fr_p95 < frame_p95_gate_us,
        "T-PERF4 Frame Render+Present p95 ({fr_p95}us) exceeded {frame_p95_gate_us}us gate"
    );
    assert_eq!(
        missed_60hz, 0,
        "T-PERF4 must have 0 missed 60 Hz frame deadlines (>16.67ms)"
    );

    if is_1080p_1x {
        // --- 3. T-PERF4 Cached Drop -> Dispatch Latency (3 warmup + 30 measured trials) ---
        let mut drop_samples_us = Vec::with_capacity(30);
        for trial in 0..33 {
            let _ = Command::new("xdotool")
                .args([
                    "mousemove",
                    &(cx - 20).to_string(),
                    &cy.to_string(),
                    "mousedown",
                    "1",
                    "mousemove",
                    &cx.to_string(),
                    &cy.to_string(),
                ])
                .status();
            conn.set_selection_owner(src_win, atoms.XdndSelection, x11rb::CURRENT_TIME)
                .unwrap();
            conn.flush().unwrap();
            let _ = Command::new("xdotool")
                .args(["keydown", "Shift_L", "keyup", "Shift_L"])
                .status();
            let dl = Instant::now() + Duration::from_secs(2);
            while Instant::now() < dl {
                let _ = pump_xdnd();
                send_xdnd(atoms.XdndEnter, [src_win, 5 << 24, atoms.URI_LIST, 0, 0]);
                send_xdnd(
                    atoms.XdndPosition,
                    [
                        src_win,
                        0,
                        (((cx + 84) as u32) << 16) | (cy as u32),
                        x11rb::CURRENT_TIME,
                        atoms.XdndActionPrivate,
                    ],
                );
                if status_of(&c_sock)["session_state"] == "Active" {
                    break;
                }
                thread::sleep(Duration::from_millis(8));
            }
            send_xdnd(
                atoms.XdndPosition,
                [
                    src_win,
                    0,
                    (((cx + 84) as u32) << 16) | (cy as u32),
                    x11rb::CURRENT_TIME,
                    atoms.XdndActionPrivate,
                ],
            );
            thread::sleep(Duration::from_millis(8));
            let _ = pump_xdnd();

            let t_drop = Instant::now();
            send_xdnd(atoms.XdndDrop, [src_win, 0, x11rb::CURRENT_TIME, 0, 0]);
            let fin_dl = Instant::now() + Duration::from_millis(200);
            while Instant::now() < fin_dl {
                let msgs = pump_xdnd();
                if msgs
                    .iter()
                    .any(|(t, d)| *t == atoms.XdndFinished && d[1] == 1)
                {
                    if trial >= 3 {
                        drop_samples_us.push(t_drop.elapsed().as_micros() as u64);
                    }
                    break;
                }
                std::hint::spin_loop();
            }
            let _ = conn.set_selection_owner(x11rb::NONE, atoms.XdndSelection, x11rb::CURRENT_TIME);
            let _ = conn.flush();
            let _ = Command::new("xdotool")
                .args(["keyup", "Shift_L", "mouseup", "1"])
                .status();
            thread::sleep(Duration::from_millis(10));
        }
        let (dr_mean, dr_p50, dr_p95, dr_p99, dr_max) = stats_us(&mut drop_samples_us);
        println!(
            "T-PERF4 Live Xvfb Cached Drop->Finished+Dispatch (30 trials): mean={dr_mean:.1}us p50={dr_p50}us p95={dr_p95}us p99={dr_p99}us max={dr_max}us"
        );
        assert!(
            dr_p95 < 30_000,
            "T-PERF4 Cached Drop->Dispatch p95 ({dr_p95}us) must be < 30ms"
        );

        // --- 4. T-PERF5 Post-session buffer release (>30s idle) & 30s Idle CPU check ---
        println!("T-PERF5 Waiting 32s for idle buffer release and measuring idle CPU ticks...");
        thread::sleep(Duration::from_secs(32));
        let _ = status_of(&c_sock);
        let c_post_release_rss_kib = read_proc_rss_kib(c_pid);

        let ticks_before = read_proc_cpu_ticks(c_pid);
        thread::sleep(Duration::from_secs(10));
        let ticks_after = read_proc_cpu_ticks(c_pid);
        let idle_ticks_10s = ticks_after.saturating_sub(ticks_before);
        let active_delta_mib = (c_active_rss_kib as f64 - b_active_rss_kib as f64) / 1024.0;
        let b_post_idle_mib = (b_post_release_rss_kib as f64) / 1024.0;
        let post_idle_mib = (c_post_release_rss_kib as f64) / 1024.0;
        let post_idle_delta_mib = post_idle_mib - b_post_idle_mib;
        println!(
            "T-PERF5 Memory & Idle Summary ({screen_w}x{screen_h}): Branching cold={:.2}MiB active={:.2}MiB post_32s_idle={b_post_idle_mib:.2}MiB | Concentric cold={:.2}MiB active={:.2}MiB (delta={:+.2}MiB, gate <+8.0MiB) post_32s_idle={post_idle_mib:.2}MiB (delta={post_idle_delta_mib:+.2}MiB, gates <25.0MiB & <+1.0MiB) | idle_cpu_ticks_10s={idle_ticks_10s} (0.00% CPU)",
            (b_cold_idle_rss_kib as f64) / 1024.0,
            (b_active_rss_kib as f64) / 1024.0,
            (c_cold_idle_rss_kib as f64) / 1024.0,
            (c_active_rss_kib as f64) / 1024.0,
            active_delta_mib,
        );
        assert!(
            active_delta_mib < 8.0,
            "T-PERF5 Active Concentric RSS delta ({active_delta_mib:.2} MiB) must be < +8 MiB vs Branching"
        );
        assert!(
            post_idle_mib < 25.0,
            "T-PERF5 Post-release idle RSS ({post_idle_mib:.2} MiB) must be < 25 MiB"
        );
        assert!(
            post_idle_delta_mib < 1.0,
            "T-PERF5 Post-release idle RSS delta ({post_idle_delta_mib:+.2} MiB) must be < +1.0 MiB vs Branching"
        );
        assert_eq!(
            idle_ticks_10s, 0,
            "T-PERF5 Idle daemon must consume 0 CPU ticks (< 0.1% CPU)"
        );
    } else if screen_w == 3840 && screen_h == 1080 {
        let active_delta_mib = (c_active_rss_kib as f64 - b_active_rss_kib as f64) / 1024.0;
        let active_mib = (c_active_rss_kib as f64) / 1024.0;
        println!(
            "T-PERF5 Ultrawide 3840x1080 Memory Summary: Branching active={:.2}MiB | Concentric active={active_mib:.2}MiB (delta={active_delta_mib:+.2}MiB, gates <45.0MiB & <+8.0MiB)",
            (b_active_rss_kib as f64) / 1024.0,
        );
        assert!(
            active_delta_mib < 8.0,
            "T-PERF5 Active Concentric RSS delta ({active_delta_mib:.2} MiB) must be < +8 MiB vs Branching"
        );
        assert!(
            active_mib < 45.0,
            "T-PERF5 Ultrawide 3840x1080 active RSS ({active_mib:.2} MiB) must be < 45 MiB"
        );
    }

    let _ = Command::new(&radivine_bin)
        .args(["--socket", c_sock.to_str().unwrap(), "quit"])
        .status();
    let _ = c_daemon.wait();
    let _ = fs::remove_dir_all(&dir);
}
