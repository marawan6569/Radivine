//! Milestone M6 isolated stability & soak test suite (`AC-6.2`, `TESTING_STRATEGY.md` §6.2).
//!
//! Exercises the real `SessionController` + `Renderer` (`tiny_skia` RGBA8 + damage tracking +
//! `dirty_union` clearing) + `WheelSession` (2-level and 3-level menus) + `Enricher` (worker
//! threads) + `Dispatcher` (worker threads + real `radivine-viewer` headless child processes) +
//! `ClipboardService` formatting + `actions.toml` reloads + no-compositor fallback with zero
//! connection to any host X11 display (`DISPLAY=""`).

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use radivine::config::AppConfig;
use radivine::session::SessionController;
use radivine_actions::builtin_registry;
use radivine_core::normalize_cli_arg_to_uri;
use radivine_platform::x11::clipboard::{
    format_gnome_copied_files, format_plain_paths, format_uri_list,
};
use radivine_platform::{
    Capabilities, ClipboardContent, FrameRef, Key, Monitor, OverlayInfo, PhysPoint, Platform,
    PlatformError, PlatformEvent, RectPx, ShapePx, SourceInfo, Support,
};
use radivine_render::{AssetSource, Renderer};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
struct SoakFileFingerprint {
    path: PathBuf,
    ino: u64,
    size: u64,
    mtime: Option<SystemTime>,
    sha256: [u8; 32],
}

impl SoakFileFingerprint {
    fn capture(path: &Path) -> Self {
        let meta = fs::symlink_metadata(path).expect("symlink_metadata");
        let bytes = fs::read(path).expect("read soak file");
        let sha256: [u8; 32] = Sha256::digest(&bytes).into();
        Self {
            path: path.to_path_buf(),
            ino: meta.ino(),
            size: meta.len(),
            mtime: meta.modified().ok(),
            sha256,
        }
    }
}

#[derive(Default)]
struct HeadlessPlatformState {
    mapped: bool,
    grabbed: bool,
    translucent_mode: bool,
    input_shaped: bool,
    bounding_shaped: bool,
    buffers_released_count: u64,
    presented_frames: u64,
    presented_bytes: u64,
    dnd_replies: Vec<bool>,
    dnd_finishes: Vec<bool>,
    requested_payloads: Vec<String>,
    clipboard_sets: u64,
}

struct HeadlessSoakPlatform {
    caps: Capabilities,
    monitors: Vec<Monitor>,
    state: Arc<Mutex<HeadlessPlatformState>>,
}

impl HeadlessSoakPlatform {
    fn new() -> (Self, Arc<Mutex<HeadlessPlatformState>>) {
        let state = Arc::new(Mutex::new(HeadlessPlatformState {
            translucent_mode: true,
            ..Default::default()
        }));
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
                name: "Virtual-Soak-0".into(),
                rect: RectPx::new(0, 0, 1920, 1080),
                refresh_mhz: 60_000,
                primary: true,
            }],
            state: Arc::clone(&state),
        };
        (plat, state)
    }
}

impl Platform for HeadlessSoakPlatform {
    fn name(&self) -> &'static str {
        "headless-soak"
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    #[cfg(unix)]
    fn poll_fd(&self) -> std::os::fd::RawFd {
        0
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
        0x0600_0001
    }

    fn prepare_overlay(&mut self, _at: PhysPoint) -> Result<OverlayInfo, PlatformError> {
        let translucent = self.state.lock().expect("lock").translucent_mode;
        Ok(OverlayInfo {
            origin: PhysPoint::new(0, 0),
            size: (1920, 1080),
            scale: 1.0,
            monitor: self.monitors[0].clone(),
            translucent,
        })
    }

    fn map_overlay(&mut self) -> Result<(), PlatformError> {
        self.state.lock().expect("lock").mapped = true;
        Ok(())
    }

    fn present(&mut self, frame: FrameRef<'_>, damage: &[RectPx]) -> Result<(), PlatformError> {
        let mut st = self.state.lock().expect("lock");
        st.presented_frames += 1;
        let damaged_bytes: u64 = damage.iter().map(|r| (r.w as u64) * (r.h as u64) * 4).sum();
        st.presented_bytes += damaged_bytes.min(frame.data.len() as u64);
        Ok(())
    }

    fn set_input_region(&mut self, shapes: Option<&[ShapePx]>) -> Result<(), PlatformError> {
        self.state.lock().expect("lock").input_shaped = shapes.is_some();
        Ok(())
    }

    fn set_bounding_region(&mut self, shapes: Option<&[ShapePx]>) -> Result<(), PlatformError> {
        self.state.lock().expect("lock").bounding_shaped = shapes.is_some();
        Ok(())
    }

    fn hide_overlay(&mut self) {
        self.state.lock().expect("lock").mapped = false;
    }

    fn release_buffers(&mut self) {
        self.state.lock().expect("lock").buffers_released_count += 1;
    }

    fn dnd_reply(&mut self, accept: bool) {
        let mut st = self.state.lock().expect("lock");
        if st.dnd_replies.len() >= 64 {
            st.dnd_replies.remove(0);
        }
        st.dnd_replies.push(accept);
    }

    fn request_payload(&mut self, mime: &str) {
        let mut st = self.state.lock().expect("lock");
        if st.requested_payloads.len() >= 16 {
            st.requested_payloads.remove(0);
        }
        st.requested_payloads.push(mime.to_string());
    }

    fn dnd_finish(&mut self, success: bool) {
        let mut st = self.state.lock().expect("lock");
        if st.dnd_finishes.len() >= 64 {
            st.dnd_finishes.remove(0);
        }
        st.dnd_finishes.push(success);
    }

    fn grab_input(&mut self) -> Result<(), PlatformError> {
        self.state.lock().expect("lock").grabbed = true;
        Ok(())
    }

    fn ungrab_input(&mut self) {
        self.state.lock().expect("lock").grabbed = false;
    }

    fn set_clipboard(&mut self, content: ClipboardContent) -> Result<(), PlatformError> {
        let gnome_bytes = format_gnome_copied_files(&content);
        let uri_bytes = format_uri_list(&content);
        let plain_bytes = format_plain_paths(&content);
        assert!(!gnome_bytes.is_empty());
        assert!(!uri_bytes.is_empty());
        assert!(!plain_bytes.is_empty());
        self.state.lock().expect("lock").clipboard_sets += 1;
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

fn read_vm_rss_kb() -> u64 {
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let num_str = rest.split_whitespace().next().unwrap_or("0");
            if let Ok(kb) = num_str.parse::<u64>() {
                return kb;
            }
        }
    }
    0
}

fn count_open_fds() -> usize {
    fs::read_dir("/proc/self/fd").map_or(0, Iterator::count)
}

fn count_zombie_children() -> usize {
    let mut zombies = 0usize;
    let Ok(tasks) = fs::read_dir("/proc/self/task") else {
        return 0;
    };
    for task in tasks.flatten() {
        let children_path = task.path().join("children");
        let Ok(children_str) = fs::read_to_string(children_path) else {
            continue;
        };
        for pid_str in children_str.split_whitespace() {
            let status_path = format!("/proc/{pid_str}/status");
            let Ok(content) = fs::read_to_string(status_path) else {
                continue;
            };
            for line in content.lines() {
                if let Some(state) = line.strip_prefix("State:")
                    && state.trim_start().starts_with('Z')
                {
                    zombies += 1;
                }
            }
        }
    }
    zombies
}

struct SoakCorpus {
    temp_dir: PathBuf,
    text_uri: String,
    dir_uri: String,
    bin_uri: String,
    symlink_uri: String,
    broken_symlink_uri: String,
    missing_uri: String,
    unreadable_uri: String,
    unreadable_path: PathBuf,
    fifo_uri: String,
    unicode_uri: String,
    valid_actions_path: PathBuf,
    invalid_actions_path: PathBuf,
    fingerprints: Vec<SoakFileFingerprint>,
}

impl SoakCorpus {
    fn create(tag: &str) -> Self {
        let temp_dir =
            std::env::temp_dir().join(format!("radivine-m6-soak-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("create soak temp_dir");

        let text_path = temp_dir.join("notes.txt");
        let mut text_body = String::new();
        for i in 1..=120 {
            text_body.push_str(&format!("Radivine soak test line {i:03} — UTF-8 café 🚀\n"));
        }
        fs::write(&text_path, &text_body).expect("write notes.txt");

        let sub_dir = temp_dir.join("folder_item");
        fs::create_dir_all(&sub_dir).expect("create folder_item");

        let bin_path = temp_dir.join("archive.bin");
        fs::write(
            &bin_path,
            b"\x7fELF\x02\x01\x01\x00\x00\x00\xff\xfe\x00\x01",
        )
        .expect("write bin");

        let symlink_path = temp_dir.join("valid_link.txt");
        symlink(&text_path, &symlink_path).expect("create valid symlink");

        let broken_symlink_path = temp_dir.join("broken_link.txt");
        symlink(
            temp_dir.join("nonexistent_target.txt"),
            &broken_symlink_path,
        )
        .expect("create broken symlink");

        let missing_path = temp_dir.join("missing_file.txt");

        let unreadable_path = temp_dir.join("unreadable.txt");
        fs::write(&unreadable_path, "secret content\n").expect("write unreadable.txt");
        let _ = fs::set_permissions(&unreadable_path, fs::Permissions::from_mode(0o000));

        let fifo_path = temp_dir.join("named_pipe.fifo");
        let _ = Command::new("mkfifo").arg(&fifo_path).status();

        let unicode_path = temp_dir.join("spaced & unicode файл.txt");
        fs::write(&unicode_path, "Unicode file body\n").expect("write unicode file");

        let valid_actions_path = temp_dir.join("actions_valid.toml");
        fs::write(
            &valid_actions_path,
            r#"
[[action]]
id = "custom.echo_path"
label = "Echo Path"
icon = "lucide:terminal"
description = "Echo file path safely"
exec = ["/bin/echo", "{path}"]
per_item = true
wait = true
notify = "never"
"#,
        )
        .expect("write valid actions.toml");

        let invalid_actions_path = temp_dir.join("actions_invalid.toml");
        fs::write(
            &invalid_actions_path,
            r#"
[[action]]
id = "custom.bad_shell"
label = "Forbidden Shell"
program = "/bin/sh"
args = ["-c", "echo pwn"]
"#,
        )
        .expect("write invalid actions.toml");

        let to_uri = |p: &std::path::Path| {
            normalize_cli_arg_to_uri(p.to_str().expect("utf8"), &temp_dir).expect("uri")
        };

        let fingerprints = vec![
            SoakFileFingerprint::capture(&text_path),
            SoakFileFingerprint::capture(&bin_path),
            SoakFileFingerprint::capture(&unicode_path),
        ];

        Self {
            text_uri: to_uri(&text_path),
            dir_uri: to_uri(&sub_dir),
            bin_uri: to_uri(&bin_path),
            symlink_uri: to_uri(&symlink_path),
            broken_symlink_uri: to_uri(&broken_symlink_path),
            missing_uri: to_uri(&missing_path),
            unreadable_uri: to_uri(&unreadable_path),
            unreadable_path,
            fifo_uri: to_uri(&fifo_path),
            unicode_uri: to_uri(&unicode_path),
            valid_actions_path,
            invalid_actions_path,
            fingerprints,
            temp_dir,
        }
    }

    fn verify_fingerprints_unchanged(&self) {
        for expected in &self.fingerprints {
            let actual = SoakFileFingerprint::capture(&expected.path);
            assert_eq!(
                &actual,
                expected,
                "soak source file mutated: {}",
                expected.path.display()
            );
        }
    }
}

impl Drop for SoakCorpus {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.unreadable_path, fs::Permissions::from_mode(0o644));
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

fn percentile_us(samples: &[u64], pct: usize) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let idx = ((sorted.len() * pct) / 100).min(sorted.len() - 1);
    sorted[idx]
}

/// Executes one complete 12-scenario cycle covering all 12 M6 Section 4 scenarios.
fn run_twelve_scenario_cycle(
    ctrl: &mut SessionController,
    plat_state: &Arc<Mutex<HeadlessPlatformState>>,
    corpus: &SoakCorpus,
    summon_us: &mut Vec<u64>,
    dnd_status_us: &mut Vec<u64>,
    drop_dispatch_us: &mut Vec<u64>,
    frame_render_us: &mut Vec<u64>,
) {
    let record_metrics = |ctrl: &mut SessionController,
                          summon_us: &mut Vec<u64>,
                          dnd_status_us: &mut Vec<u64>,
                          drop_dispatch_us: &mut Vec<u64>,
                          frame_render_us: &mut Vec<u64>| {
        let st = ctrl.status();
        if let Some(v) = st.last_summon_us {
            summon_us.push(v);
        }
        if let Some(v) = st.last_dnd_status_us {
            dnd_status_us.push(v);
        }
        if let Some(v) = st.last_drop_dispatch_us {
            drop_dispatch_us.push(v);
        }
        if let Some(v) = st.last_frame_render_us {
            frame_render_us.push(v);
        }
    };

    // Scenario 1: Drag + Shift activation -> leaf commit (`Copy`, index 1 at +(80, 0))
    ctrl.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: SourceInfo {
                app: Some("nemo".to_string()),
                pid: Some(4242),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".to_string()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".to_string(),
            result: Ok(format!("{}\r\n", corpus.text_uri).into_bytes()),
        },
        PlatformEvent::DndPosition {
            at: PhysPoint::new(1040, 540),
        },
        PlatformEvent::DndDrop,
        PlatformEvent::DragDisarmed,
    ]);
    assert_eq!(ctrl.status().session_state, "Idle");
    record_metrics(
        ctrl,
        summon_us,
        dnd_status_us,
        drop_dispatch_us,
        frame_render_us,
    );

    // Scenario 2: Invoke mode activation -> Keyboard navigation -> Commit (`Copy`)
    ctrl.summon_invoke(
        vec![corpus.text_uri.clone()],
        Some(PhysPoint::new(960, 540)),
    )
    .expect("summon_invoke scenario 2");
    ctrl.handle_platform_events(vec![
        PlatformEvent::Key {
            key: Key::Digit(2),
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Enter,
            pressed: true,
        },
    ]);
    assert_eq!(ctrl.status().session_state, "Idle");
    record_metrics(
        ctrl,
        summon_us,
        dnd_status_us,
        drop_dispatch_us,
        frame_render_us,
    );

    // Scenario 3: 2-level submenu navigation (`File Preview` -> `Head`) -> spawns real headless `radivine-viewer`
    ctrl.summon_invoke(
        vec![corpus.text_uri.clone()],
        Some(PhysPoint::new(960, 540)),
    )
    .expect("summon_invoke scenario 3");
    ctrl.handle_platform_events(vec![
        PlatformEvent::Key {
            key: Key::Digit(1),
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Enter,
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Digit(2),
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Enter,
            pressed: true,
        },
    ]);
    assert_eq!(ctrl.status().session_state, "Idle");
    record_metrics(
        ctrl,
        summon_us,
        dnd_status_us,
        drop_dispatch_us,
        frame_render_us,
    );

    // Scenario 4: 3-level radial menu navigation (`Custom` -> `Scripts` -> `Compute SHA-256`) + pointer move
    ctrl.summon_invoke(
        vec![corpus.text_uri.clone()],
        Some(PhysPoint::new(960, 540)),
    )
    .expect("summon_invoke scenario 4");
    ctrl.handle_platform_events(vec![
        PlatformEvent::PointerMoved {
            at: PhysPoint::new(880, 540),
        },
        PlatformEvent::Key {
            key: Key::Enter,
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Digit(2),
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Enter,
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Digit(1),
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Enter,
            pressed: true,
        },
    ]);
    assert_eq!(ctrl.status().session_state, "Idle");
    record_metrics(
        ctrl,
        summon_us,
        dnd_status_us,
        drop_dispatch_us,
        frame_render_us,
    );

    // Scenario 5: Cancellation via center hub click in Invoke mode
    ctrl.summon_invoke(
        vec![corpus.text_uri.clone()],
        Some(PhysPoint::new(960, 540)),
    )
    .expect("summon_invoke scenario 5");
    ctrl.handle_platform_events(vec![
        PlatformEvent::PointerMoved {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::Button {
            button: 1,
            pressed: false,
            at: PhysPoint::new(960, 540),
        },
    ]);
    assert_eq!(ctrl.status().session_state, "Idle");

    // Scenario 6: Cancellation via `Escape` key and second `Shift` toggle-off in Drag mode
    ctrl.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: SourceInfo {
                app: Some("nemo".to_string()),
                pid: Some(4242),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".to_string(),
            result: Ok(format!("{}\r\n", corpus.text_uri).into_bytes()),
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DragDisarmed,
    ]);
    assert_eq!(ctrl.status().session_state, "Idle");

    // Scenario 7: Source window death / drag abort mid-drag (`DndLeave { drag_ended: true }`)
    ctrl.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: SourceInfo {
                app: Some("nemo".to_string()),
                pid: Some(4242),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".to_string()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".to_string(),
            result: Ok(format!("{}\r\n", corpus.text_uri).into_bytes()),
        },
        PlatformEvent::DndLeave { drag_ended: true },
        PlatformEvent::DragDisarmed,
    ]);
    assert_eq!(ctrl.status().session_state, "Idle");
    assert!(!plat_state.lock().expect("lock").mapped);

    // Scenario 8: Rapid multi-file drag cycle (5 mixed items: text, dir, bin, symlink, unicode)
    let multi_payload = format!(
        "{}\r\n{}\r\n{}\r\n{}\r\n{}\r\n",
        corpus.text_uri, corpus.dir_uri, corpus.bin_uri, corpus.symlink_uri, corpus.unicode_uri
    );
    ctrl.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: SourceInfo {
                app: Some("nemo".to_string()),
                pid: Some(4242),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".to_string()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".to_string(),
            result: Ok(multi_payload.into_bytes()),
        },
        PlatformEvent::DndPosition {
            at: PhysPoint::new(1040, 540),
        },
        PlatformEvent::DndDrop,
        PlatformEvent::DragDisarmed,
    ]);
    assert_eq!(ctrl.status().session_state, "Idle");
    record_metrics(
        ctrl,
        summon_us,
        dnd_status_us,
        drop_dispatch_us,
        frame_render_us,
    );

    // Scenario 9: Context enrichment failure matrix (missing, broken symlink, unreadable, FIFO)
    for bad_uri in [
        &corpus.missing_uri,
        &corpus.broken_symlink_uri,
        &corpus.unreadable_uri,
        &corpus.fifo_uri,
    ] {
        ctrl.summon_invoke(vec![bad_uri.clone()], Some(PhysPoint::new(960, 540)))
            .expect("summon_invoke enrichment failure case");
        let st = ctrl.status();
        let preview_head = st
            .active_item_states
            .iter()
            .find(|i| i.id == "radivine.preview.head")
            .expect("radivine.preview.head present");
        assert!(!preview_head.enabled);
        assert!(preview_head.disabled_reason.is_some());
        ctrl.handle_platform_events(vec![PlatformEvent::Key {
            key: Key::Escape,
            pressed: true,
        }]);
        assert_eq!(ctrl.status().session_state, "Idle");
    }

    // Scenario 10: Preview Full launch (`radivine-viewer` headless child process + reap)
    ctrl.summon_invoke(
        vec![corpus.text_uri.clone()],
        Some(PhysPoint::new(960, 540)),
    )
    .expect("summon_invoke scenario 10");
    ctrl.handle_platform_events(vec![
        PlatformEvent::Key {
            key: Key::Digit(1),
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Enter,
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Digit(1),
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Enter,
            pressed: true,
        },
    ]);
    assert_eq!(ctrl.status().session_state, "Idle");

    // Scenario 11: Invalid `actions.toml` reload followed by valid `actions.toml` reload & execution
    ctrl.load_actions_from_path(&corpus.invalid_actions_path);
    assert!(!ctrl.status().config_warnings.is_empty());
    ctrl.load_actions_from_path(&corpus.valid_actions_path);
    assert!(ctrl.status().config_warnings.is_empty());
    ctrl.summon_invoke(
        vec![corpus.text_uri.clone()],
        Some(PhysPoint::new(960, 540)),
    )
    .expect("summon_invoke scenario 11");
    ctrl.handle_platform_events(vec![
        PlatformEvent::Key {
            key: Key::Digit(4),
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Enter,
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Digit(1),
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Enter,
            pressed: true,
        },
    ]);
    assert_eq!(ctrl.status().session_state, "Idle");

    // Scenario 12: No-compositor opaque fallback mode (`translucent = false` + `set_bounding_region`)
    // plus 31s idle timer tick to verify `release_buffers()` reclamation
    plat_state.lock().expect("lock").translucent_mode = false;
    ctrl.summon_invoke(
        vec![corpus.text_uri.clone()],
        Some(PhysPoint::new(100, 100)),
    )
    .expect("summon_invoke scenario 12 no-compositor");
    assert!(plat_state.lock().expect("lock").bounding_shaped);
    ctrl.handle_platform_events(vec![PlatformEvent::Key {
        key: Key::Escape,
        pressed: true,
    }]);
    plat_state.lock().expect("lock").translucent_mode = true;
    ctrl.on_timer(Instant::now() + Duration::from_secs(31));
    assert_eq!(ctrl.status().session_state, "Idle");
    record_metrics(
        ctrl,
        summon_us,
        dnd_status_us,
        drop_dispatch_us,
        frame_render_us,
    );

    // Scenario 13: Switch layout Branching -> Concentric via `reload_config`, then run
    // Concentric Drag + Shift multi-ring navigation (root `[0]` -> child ring 1 -> gap -> root `[1]` Copy commit)
    let mut concentric_cfg = AppConfig::default();
    concentric_cfg.wheel.layout = "concentric".to_string();
    ctrl.reload_config(concentric_cfg.clone());
    ctrl.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: SourceInfo {
                app: Some("nemo".to_string()),
                pid: Some(4242),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".to_string()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".to_string(),
            result: Ok(format!("{}\r\n", corpus.text_uri).into_bytes()),
        },
        PlatformEvent::DndPosition {
            at: PhysPoint::new(960, 456),
        },
    ]);
    assert_eq!(ctrl.status().active_layout, "concentric");
    ctrl.on_timer(Instant::now() + Duration::from_millis(200));
    ctrl.handle_platform_events(vec![
        // Inter-ring radial gap (`r = 131`) must reject while keeping child ring open
        PlatformEvent::DndPosition {
            at: PhysPoint::new(960, 409),
        },
        // Child ring 1 leaf (`r = 164` top)
        PlatformEvent::DndPosition {
            at: PhysPoint::new(960, 374),
        },
        PlatformEvent::DndPosition {
            at: PhysPoint::new(962, 374),
        },
        // Adopt sibling `[1]` (`Copy` at `+(84, 0)`) after grace and commit
        PlatformEvent::DndPosition {
            at: PhysPoint::new(1044, 540),
        },
    ]);
    ctrl.on_timer(Instant::now() + Duration::from_millis(450));
    ctrl.handle_platform_events(vec![
        PlatformEvent::DndPosition {
            at: PhysPoint::new(1046, 540),
        },
        PlatformEvent::DndDrop,
        PlatformEvent::DragDisarmed,
    ]);
    assert_eq!(ctrl.status().session_state, "Idle");
    record_metrics(
        ctrl,
        summon_us,
        dnd_status_us,
        drop_dispatch_us,
        frame_render_us,
    );

    // Scenario 14: Concentric Invoke sustained 2-level navigation + mid-session `reload_config(Branching)`
    // + `load_actions_from_path` menu reload (verifying active session snapshot immutability)
    ctrl.summon_invoke(
        vec![corpus.text_uri.clone()],
        Some(PhysPoint::new(960, 540)),
    )
    .expect("summon_invoke scenario 14 concentric");
    assert_eq!(ctrl.status().active_layout, "concentric");
    let branching_cfg = AppConfig::default();
    ctrl.reload_config(branching_cfg);
    ctrl.load_actions_from_path(&corpus.valid_actions_path);
    // Active session must remain Concentric with layout_pending = true until session ends
    let mid_status = ctrl.status();
    assert_eq!(mid_status.active_layout, "concentric");
    assert!(mid_status.layout_pending);
    ctrl.handle_platform_events(vec![
        PlatformEvent::Key {
            key: Key::Digit(1),
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Backspace,
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Digit(2),
            pressed: true,
        },
        PlatformEvent::Key {
            key: Key::Enter,
            pressed: true,
        },
    ]);
    assert_eq!(ctrl.status().session_state, "Idle");
    record_metrics(
        ctrl,
        summon_us,
        dnd_status_us,
        drop_dispatch_us,
        frame_render_us,
    );

    // Scenario 15: Concentric Drag non-actionable reject-zone cancellations (hub, radial gap,
    // caption box, outside cluster drop) + corner Invoke Escape cancellation
    ctrl.reload_config(concentric_cfg);
    ctrl.handle_platform_events(vec![
        PlatformEvent::DragArmed {
            source: SourceInfo {
                app: Some("nemo".to_string()),
                pid: Some(4242),
            },
        },
        PlatformEvent::Trigger {
            at: PhysPoint::new(960, 540),
        },
        PlatformEvent::DndEnter {
            types: vec!["text/uri-list".to_string()],
        },
        PlatformEvent::Payload {
            mime: "text/uri-list".to_string(),
            result: Ok(format!("{}\r\n", corpus.text_uri).into_bytes()),
        },
        // Hub (`r = 0`)
        PlatformEvent::DndPosition {
            at: PhysPoint::new(960, 540),
        },
        // Caption box (`(960, 908)`)
        PlatformEvent::DndPosition {
            at: PhysPoint::new(960, 908),
        },
        // Outside cluster (`(200, 200)`) + drop -> must reject (`dnd_finish(false)`)
        PlatformEvent::DndPosition {
            at: PhysPoint::new(200, 200),
        },
        PlatformEvent::DndDrop,
        PlatformEvent::DragDisarmed,
    ]);
    assert_eq!(ctrl.status().session_state, "Idle");
    assert_eq!(
        plat_state
            .lock()
            .expect("lock")
            .dnd_finishes
            .last()
            .copied(),
        Some(false)
    );

    // Corner summon (`(20, 20)` -> clamped to `(334, 334)`) + Escape cancel
    ctrl.summon_invoke(vec![corpus.text_uri.clone()], Some(PhysPoint::new(20, 20)))
        .expect("summon_invoke corner concentric");
    assert_eq!(ctrl.status().root_wheel_center_px, Some((334.0, 334.0)));
    ctrl.handle_platform_events(vec![PlatformEvent::Key {
        key: Key::Escape,
        pressed: true,
    }]);
    assert_eq!(ctrl.status().session_state, "Idle");

    // Scenario 16: Restore Branching layout for the next cycle and verify zero source file modifications
    ctrl.reload_config(AppConfig::default());
    corpus.verify_fingerprints_unchanged();
}

#[test]
fn m6_isolated_multi_scenario_smoke_test() {
    let corpus = SoakCorpus::create("smoke");
    let (plat, plat_state) = HeadlessSoakPlatform::new();
    let cfg = AppConfig::default();
    let renderer = Renderer::new(cfg.resolve_theme(), AssetSource::Embedded).expect("renderer");
    let registry = Arc::new(builtin_registry());
    let mut ctrl = SessionController::new(Box::new(plat), renderer, registry, cfg);

    let mut summon_us = Vec::new();
    let mut dnd_status_us = Vec::new();
    let mut drop_dispatch_us = Vec::new();
    let mut frame_render_us = Vec::new();

    for _ in 0..2 {
        run_twelve_scenario_cycle(
            &mut ctrl,
            &plat_state,
            &corpus,
            &mut summon_us,
            &mut dnd_status_us,
            &mut drop_dispatch_us,
            &mut frame_render_us,
        );
    }

    // Wait briefly for detached `radivine-viewer-reap` threads to reap headless viewer children
    thread::sleep(Duration::from_millis(80));
    assert_eq!(
        count_zombie_children(),
        0,
        "expected 0 zombie child processes"
    );
    let st = plat_state.lock().expect("lock");
    assert!(st.presented_frames >= 24);
    assert!(st.clipboard_sets >= 4);
    assert!(st.buffers_released_count >= 2);
}

#[test]
#[ignore = "1-hour M6 soak test (`AC-6.2`); run with RADIVINE_SOAK_SECONDS=3605 cargo test --release -p radivine --test m6_soak_test -- --ignored --nocapture"]
fn m6_one_hour_isolated_soak_test_ac_6_2() {
    let target_secs: u64 = std::env::var("RADIVINE_SOAK_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3605);
    let min_sessions: u64 = 2400;

    let corpus = SoakCorpus::create("soak-1h");
    let (plat, plat_state) = HeadlessSoakPlatform::new();
    let cfg = AppConfig::default();
    let renderer = Renderer::new(cfg.resolve_theme(), AssetSource::Embedded).expect("renderer");
    let registry = Arc::new(builtin_registry());
    let mut ctrl = SessionController::new(Box::new(plat), renderer, registry, cfg);

    let start = Instant::now();
    let initial_rss_kb = read_vm_rss_kb();
    let initial_fds = count_open_fds();
    let mut warmup_rss_kb: Option<u64> = None;
    let mut warmup_fds: Option<usize> = None;
    let mut peak_rss_kb = initial_rss_kb;
    let mut total_sessions = 0u64;
    let mut cycles_completed = 0u64;

    let mut summon_us = Vec::with_capacity(8192);
    let mut dnd_status_us = Vec::with_capacity(8192);
    let mut drop_dispatch_us = Vec::with_capacity(8192);
    let mut frame_render_us = Vec::with_capacity(8192);
    let mut rss_timeline_kb: Vec<(u64, u64, u64)> = Vec::new();

    let report_path = PathBuf::from("/tmp/radivine-m6-soak-report.json");

    while start.elapsed().as_secs() < target_secs || total_sessions < min_sessions {
        run_twelve_scenario_cycle(
            &mut ctrl,
            &plat_state,
            &corpus,
            &mut summon_us,
            &mut dnd_status_us,
            &mut drop_dispatch_us,
            &mut frame_render_us,
        );
        // Each cycle executes 15 Branching sessions + 4 Concentric sessions = 19 distinct sessions
        cycles_completed += 1;
        total_sessions += 19;

        let rss_now = read_vm_rss_kb();
        peak_rss_kb = peak_rss_kb.max(rss_now);

        if warmup_rss_kb.is_none() && total_sessions >= 760 {
            let settle_deadline = Instant::now() + Duration::from_millis(500);
            while Instant::now() < settle_deadline {
                let _ = ctrl.status();
                thread::sleep(Duration::from_millis(25));
            }
            let settled_rss = read_vm_rss_kb();
            warmup_rss_kb = Some(settled_rss);
            warmup_fds = Some(count_open_fds());
            rss_timeline_kb.push((start.elapsed().as_secs(), total_sessions, settled_rss));
        } else if warmup_rss_kb.is_some() && cycles_completed.is_multiple_of(10) {
            rss_timeline_kb.push((start.elapsed().as_secs(), total_sessions, rss_now));
            let warmup = warmup_rss_kb.unwrap_or(rss_now);
            let growth_kb = rss_now as i64 - warmup as i64;
            let zombies = count_zombie_children();
            let fds_now = count_open_fds();
            let snapshot = serde_json::json!({
                "status": "running",
                "elapsed_secs": start.elapsed().as_secs(),
                "target_secs": target_secs,
                "cycles_completed": cycles_completed,
                "total_sessions": total_sessions,
                "branching_sessions": cycles_completed * 15,
                "concentric_sessions": cycles_completed * 4,
                "layout_switches": cycles_completed * 4,
                "menu_reloads": cycles_completed * 3,
                "cancellations": cycles_completed * 10,
                "initial_rss_kb": initial_rss_kb,
                "warmup_rss_kb": warmup,
                "current_rss_kb": rss_now,
                "peak_rss_kb": peak_rss_kb,
                "rss_growth_after_warmup_kb": growth_kb,
                "initial_fds": initial_fds,
                "warmup_fds": warmup_fds.unwrap_or(initial_fds),
                "current_fds": fds_now,
                "zombie_children": zombies,
                "summon_p50_us": percentile_us(&summon_us, 50),
                "summon_p95_us": percentile_us(&summon_us, 95),
                "summon_p99_us": percentile_us(&summon_us, 99),
                "dnd_status_p50_us": percentile_us(&dnd_status_us, 50),
                "dnd_status_p95_us": percentile_us(&dnd_status_us, 95),
                "dnd_status_p99_us": percentile_us(&dnd_status_us, 99),
                "drop_dispatch_p50_us": percentile_us(&drop_dispatch_us, 50),
                "drop_dispatch_p95_us": percentile_us(&drop_dispatch_us, 95),
                "drop_dispatch_p99_us": percentile_us(&drop_dispatch_us, 99),
                "frame_render_p50_us": percentile_us(&frame_render_us, 50),
                "frame_render_p95_us": percentile_us(&frame_render_us, 95),
                "frame_render_p99_us": percentile_us(&frame_render_us, 99),
            });
            let _ = fs::write(
                &report_path,
                serde_json::to_string_pretty(&snapshot).unwrap_or_default(),
            );
        }

        // Bound telemetry vectors so the test harness itself does not grow memory over 1 hour
        if summon_us.len() > 8192 {
            summon_us.drain(0..4096);
        }
        if dnd_status_us.len() > 8192 {
            dnd_status_us.drain(0..4096);
        }
        if drop_dispatch_us.len() > 8192 {
            drop_dispatch_us.drain(0..4096);
        }
        if frame_render_us.len() > 8192 {
            frame_render_us.drain(0..4096);
        }

        // Pace the soak loop during 1-hour runs after warm-up (~800 ms per 19-session cycle -> ~8,500+ sessions/hour)
        if target_secs >= 60 && warmup_rss_kb.is_some() {
            thread::sleep(Duration::from_millis(800));
        }
    }

    // Allow background reaper/worker threads to finish in-flight child pipes before final measurement
    ctrl.on_timer(Instant::now() + Duration::from_secs(31));
    let warmup_fd_count = warmup_fds.unwrap_or(initial_fds);
    let drain_deadline = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < drain_deadline {
        let _ = ctrl.status();
        if count_open_fds() <= warmup_fd_count && count_zombie_children() == 0 {
            break;
        }
        thread::sleep(Duration::from_millis(30));
    }

    let final_rss_kb = read_vm_rss_kb();
    let warmup_rss = warmup_rss_kb.unwrap_or(initial_rss_kb);
    let rss_growth_kb = final_rss_kb as i64 - warmup_rss as i64;
    let final_fds = count_open_fds();
    let zombies = count_zombie_children();
    let elapsed_secs = start.elapsed().as_secs();
    let st = plat_state.lock().expect("lock");
    corpus.verify_fingerprints_unchanged();

    let final_report = serde_json::json!({
        "status": "completed",
        "elapsed_secs": elapsed_secs,
        "cycles_completed": cycles_completed,
        "total_sessions": total_sessions,
        "branching_sessions": cycles_completed * 15,
        "concentric_sessions": cycles_completed * 4,
        "layout_switches": cycles_completed * 4,
        "menu_reloads": cycles_completed * 3,
        "cancellations": cycles_completed * 10,
        "source_modifications_detected": 0,
        "initial_rss_kb": initial_rss_kb,
        "warmup_rss_kb": warmup_rss,
        "final_rss_kb": final_rss_kb,
        "peak_rss_kb": peak_rss_kb,
        "rss_growth_after_warmup_kb": rss_growth_kb,
        "initial_fds": initial_fds,
        "warmup_fds": warmup_fd_count,
        "final_fds": final_fds,
        "zombie_children": zombies,
        "presented_frames": st.presented_frames,
        "presented_bytes": st.presented_bytes,
        "clipboard_sets": st.clipboard_sets,
        "buffers_released_count": st.buffers_released_count,
        "summon_p50_us": percentile_us(&summon_us, 50),
        "summon_p95_us": percentile_us(&summon_us, 95),
        "summon_p99_us": percentile_us(&summon_us, 99),
        "dnd_status_p50_us": percentile_us(&dnd_status_us, 50),
        "dnd_status_p95_us": percentile_us(&dnd_status_us, 95),
        "dnd_status_p99_us": percentile_us(&dnd_status_us, 99),
        "drop_dispatch_p50_us": percentile_us(&drop_dispatch_us, 50),
        "drop_dispatch_p95_us": percentile_us(&drop_dispatch_us, 95),
        "drop_dispatch_p99_us": percentile_us(&drop_dispatch_us, 99),
        "frame_render_p50_us": percentile_us(&frame_render_us, 50),
        "frame_render_p95_us": percentile_us(&frame_render_us, 95),
        "frame_render_p99_us": percentile_us(&frame_render_us, 99),
        "rss_timeline_kb": rss_timeline_kb,
    });
    let report_json = serde_json::to_string_pretty(&final_report).expect("serialize soak report");
    fs::write(&report_path, &report_json).expect("write /tmp/radivine-m6-soak-report.json");
    println!("{report_json}");

    assert!(
        total_sessions >= 2000,
        "expected >= 2,000 sessions, got {total_sessions}"
    );
    assert!(
        rss_growth_kb <= 2048,
        "AC-6.2 failed: RSS grew by {rss_growth_kb} KB after warm-up (limit <= 2048 KB)"
    );
    assert!(
        final_fds <= warmup_fd_count + 2,
        "AC-6.2 failed: open FDs grew from {warmup_fd_count} to {final_fds}"
    );
    assert_eq!(
        zombies, 0,
        "AC-6.2 failed: found {zombies} zombie child processes"
    );
}
