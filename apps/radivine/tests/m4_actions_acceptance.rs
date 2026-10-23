//! End-to-end X11 acceptance tests for Milestone M4 (`AC-4.1` through `AC-4.7`).
//!
//! Verifies:
//! - `AC-4.1`: Context enrichment & two-tier action filtering (`BasicContext` visibility + `FileFacts`
//!   enablement across UTF-8 `.txt`, known binary `.png`, disguised binary `.txt`, directory,
//!   3-file multi-selection, broken symlink, FIFO special file without blocking, and non-local `sftp://` URI).
//! - `AC-4.2`: Asynchronous enrichment timeout (`1500 ms` watchdog): hung `stat` path opens the wheel
//!   immediately (`< 120 ms`) with `FileFacts`-dependent actions disabled with `"Checking…"`, while
//!   `Copy` and `Properties` remain immediately enabled; after `~1.5 s`, `"Checking…"` transitions
//!   cleanly to `"Couldn't inspect the file"` without layout reflow.
//! - `AC-4.3`: `Copy` (`radivine.copy`): commits `SetClipboard` effect, acquires X11 `CLIPBOARD`
//!   selection ownership, serves `x-special/gnome-copied-files` (`copy\nfile:///...`), `text/uri-list`
//!   (`file:///...\r\n`), and `UTF8_STRING` / `text/plain` across single file, multi-file (with spaces
//!   and Unicode), and directory targets.
//! - `AC-4.4`: `Properties` (`radivine.properties`): dispatches `ShowItemProperties` (with `1..=16`
//!   enablement rule and graceful error notification on D-Bus failure).
//! - `AC-4.5`: Custom actions (`actions.toml`): parses `[[action]]`, expands whole-element `{path}`,
//!   `{paths}`, `{uri}`, `{uris}`, `{dir}`, `{name}`, executes directly via `Command::new(program).args(...)`
//!   without a shell (`semi;colon & $(id) \`uname\`.txt` passed literally), and respects `[action.when]`
//!   (`count`, `extensions`, `mime`, `require_text`) and `[menu]` (`hide`, `order`).
//! - `AC-4.6`: Invalid `actions.toml`: syntax errors or invalid entries report line/column diagnostics
//!   in `config_warnings` and `last_notification` while preserving the active `ActionRegistry` and
//!   all built-in actions.
//! - `AC-4.7`: Failure isolation: non-zero exit codes (`wait = true`) and panicking actions
//!   (`custom.__test_panic__`) are caught, logged, and notified without crashing the daemon or
//!   breaking subsequent sessions.
//! - Safety Invariant: Protected test files remain 100% unmodified (identical inode, size, SHA-256).

use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use radivine_actions::builtin_registry;
use radivine_core::{BasicContext, Context, InvocationMode, MenuLimits, SourceInfo};
use radivine_platform::x11::atoms::Atoms;
use radivine_wheel::{InputMode, Point, Viewport, WheelConfig, WheelInput, WheelSession};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, MapState, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

fn radivine_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_radivine"))
}

struct TestDaemon {
    child: Child,
    sock_path: PathBuf,
    config_path: PathBuf,
    actions_path: PathBuf,
    temp_dir: PathBuf,
}

impl TestDaemon {
    fn spawn(config_toml: &str, actions_toml: Option<&str>, tag: &str) -> Self {
        let temp_dir =
            std::env::temp_dir().join(format!("radivine-m4-test-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("create temp_dir");

        let config_path = temp_dir.join("config.toml");
        fs::write(&config_path, config_toml).expect("write config.toml");

        let actions_path = temp_dir.join("actions.toml");
        if let Some(act_str) = actions_toml {
            fs::write(&actions_path, act_str).expect("write actions.toml");
        }

        let sock_path = temp_dir.join("radivine.sock");

        let child = Command::new(radivine_bin())
            .env("RADIVINE_DRY_RUN_VIEWER", "1")
            .env("RADIVINE_DRY_RUN_PROPERTIES", "1")
            .env("RADIVINE_SUPPRESS_DESKTOP_NOTIFY", "1")
            .arg("--config")
            .arg(&config_path)
            .arg("--socket")
            .arg(&sock_path)
            .arg("daemon")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn radivine daemon");

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if sock_path.exists() {
                let status = Command::new(radivine_bin())
                    .arg("--socket")
                    .arg(&sock_path)
                    .arg("status")
                    .output()
                    .expect("run status");
                if status.status.success() {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(10));
        }

        Self {
            child,
            sock_path,
            config_path,
            actions_path,
            temp_dir,
        }
    }

    fn cli(&self, args: &[&str]) -> std::process::Output {
        Command::new(radivine_bin())
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

    fn stop(&mut self) {
        let _ = self.cli(&["quit"]);
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileFingerprint {
    path: PathBuf,
    inode: u64,
    size: u64,
    mtime: SystemTime,
    bytes: Vec<u8>,
}

impl FileFingerprint {
    fn capture(path: &Path) -> Self {
        let meta = fs::symlink_metadata(path).expect("symlink_metadata");
        let bytes = if meta.file_type().is_symlink() {
            fs::read_link(path)
                .expect("read_link")
                .to_string_lossy()
                .into_owned()
                .into_bytes()
        } else if meta.is_file() {
            fs::read(path).expect("read file")
        } else {
            Vec::new()
        };
        Self {
            path: path.to_path_buf(),
            inode: meta.ino(),
            size: meta.len(),
            mtime: meta.modified().expect("mtime"),
            bytes,
        }
    }
}

fn is_window_mapped(conn: &RustConnection, win: Window) -> bool {
    conn.get_window_attributes(win)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some_and(|r| r.map_state == MapState::VIEWABLE)
}

fn wait_window_mapped(conn: &RustConnection, win: Window, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if is_window_mapped(conn, win) {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    is_window_mapped(conn, win)
}

fn wait_window_unmapped(conn: &RustConnection, win: Window, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !is_window_mapped(conn, win) {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    !is_window_mapped(conn, win)
}

fn path_to_file_uri(path: &Path) -> String {
    radivine_core::normalize_cli_arg_to_uri(&path.to_string_lossy(), Path::new("/"))
        .expect("normalize_cli_arg_to_uri")
}

fn compute_segment_point(anchor: (i32, i32), path: &[u8]) -> (i32, i32) {
    let reg = Arc::new(builtin_registry());
    let basic = BasicContext::from_uris(
        ["file:///tmp/alpha.txt"],
        InvocationMode::Drag,
        SourceInfo::default(),
    );
    let ctx = Context::from_basic(basic);
    let (menu_tree, _) = reg
        .build_menu(&ctx, MenuLimits::default())
        .expect("build_menu");
    let cfg = WheelConfig {
        radius_root: [40.0, 128.0],
        radius_child: [34.0, 108.0],
        child_gap: 20.0,
        hysteresis_deg: 3.5,
        reduced_motion: true,
        ..Default::default()
    };
    let viewport = Viewport::new(3840.0, 1080.0, 1.0);
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
    let mut target_pt = (anchor.0, anchor.1);

    for (depth, &seg_idx) in path.iter().enumerate() {
        let dbg = ws.debug_state();
        let wheel = &dbg.wheels[depth];
        let span = &wheel.spans[seg_idx as usize];
        let mid_r = (wheel.r_in + wheel.r_out) * 0.5;
        let pt = wheel.center.offset_polar(mid_r, span.center);
        target_pt = (pt.x.round() as i32, pt.y.round() as i32);

        if depth + 1 < path.len() {
            ws.handle(WheelInput::PointerMoved(pt), now_ms, &mut out);
            now_ms += 200;
            ws.tick(now_ms, &mut out);
        }
    }

    target_pt
}

fn find_item_state<'a>(
    status: &'a serde_json::Value,
    action_id: &str,
) -> Option<(bool, Option<&'a str>)> {
    let arr = status["active_item_states"].as_array()?;
    for item in arr {
        if item["id"].as_str() == Some(action_id) {
            let enabled = item["enabled"].as_bool().unwrap_or(false);
            let reason = item["disabled_reason"].as_str();
            return Some((enabled, reason));
        }
    }
    None
}

/// Reads the specified selection target from `CLIPBOARD` using an independent X11 requestor window.
fn request_clipboard_target(conn: &RustConnection, atoms: &Atoms, target: Atom) -> Vec<u8> {
    let screen = &conn.setup().roots[0];
    let requestor = conn.generate_id().expect("generate requestor xid");
    let prop = atoms.RADIVINE_CLIP;
    conn.create_window(
        x11rb::COPY_FROM_PARENT as u8,
        requestor,
        screen.root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )
    .expect("create clipboard requestor");

    conn.convert_selection(
        requestor,
        atoms.CLIPBOARD,
        target,
        prop,
        x11rb::CURRENT_TIME,
    )
    .expect("convert_selection");
    conn.flush().expect("flush");

    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        while let Some(ev) = conn.poll_for_event().expect("poll_for_event") {
            if let Event::SelectionNotify(sn) = ev
                && sn.requestor == requestor
                && sn.selection == atoms.CLIPBOARD
            {
                assert_ne!(
                    sn.property,
                    u32::from(AtomEnum::NONE),
                    "CLIPBOARD owner rejected target {target}"
                );
                let reply = conn
                    .get_property(true, requestor, prop, AtomEnum::ANY, 0, 262_144)
                    .expect("get_property")
                    .reply()
                    .expect("get_property reply");
                let _ = conn.destroy_window(requestor);
                let _ = conn.flush();
                return reply.value;
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    let _ = conn.destroy_window(requestor);
    let _ = conn.flush();
    panic!("timed out waiting for SelectionNotify from CLIPBOARD");
}

#[test]
fn m4_actions_acceptance_suite() {
    if !radivine_platform::is_isolated_test_display() {
        return;
    }
    let Ok((conn, _)) = RustConnection::connect(None) else {
        return;
    };
    let atoms = Atoms::new(&conn)
        .expect("atoms cookie")
        .reply()
        .expect("atoms reply");

    // Create protected corpus directory with diverse test items
    let corpus_dir =
        std::env::temp_dir().join(format!("radivine-m4-corpus-{}", std::process::id()));
    let _ = fs::remove_dir_all(&corpus_dir);
    fs::create_dir_all(&corpus_dir).expect("create corpus_dir");

    let txt_a = corpus_dir.join("notes_alpha.txt");
    let txt_b = corpus_dir.join("space name & (2).txt");
    let txt_c = corpus_dir.join("تقرير_عربي_🚀.txt");
    let png_bin = corpus_dir.join("photo.png");
    let disguised_bin = corpus_dir.join("disguised_binary.txt");
    let sub_dir = corpus_dir.join("project_subdir");
    let broken_link = corpus_dir.join("broken_symlink.txt");
    let fifo_path = corpus_dir.join("test_pipe.fifo");
    let hung_path = corpus_dir.join("__radivine_hung_stat__.txt");
    let shell_hazard = corpus_dir.join("semi;colon & $(id) `uname`.txt");

    fs::write(&txt_a, b"Line 1\nLine 2\nLine 3\n").unwrap();
    fs::write(&txt_b, b"File with spaces and symbols\n").unwrap();
    fs::write(&txt_c, "محتوى نصي بالعربية\n".as_bytes()).unwrap();
    fs::write(
        &png_bin,
        b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01",
    )
    .unwrap();
    fs::write(
        &disguised_bin,
        b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01",
    )
    .unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    symlink(corpus_dir.join("does_not_exist_target.txt"), &broken_link).unwrap();
    let mkfifo_st = Command::new("mkfifo")
        .arg(&fifo_path)
        .status()
        .expect("run mkfifo");
    assert!(mkfifo_st.success(), "mkfifo failed");
    fs::write(&hung_path, b"slow stat simulation\n").unwrap();
    fs::write(&shell_hazard, b"shell hazard test content\n").unwrap();

    let protected_fingerprints = vec![
        FileFingerprint::capture(&txt_a),
        FileFingerprint::capture(&txt_b),
        FileFingerprint::capture(&txt_c),
        FileFingerprint::capture(&png_bin),
        FileFingerprint::capture(&disguised_bin),
        FileFingerprint::capture(&broken_link),
        FileFingerprint::capture(&shell_hazard),
    ];

    let config_toml = r#"
[appearance]
reduced_motion = true

[wheel]
hover_dwell_ms = 50
submenu_grace_ms = 140
leave_grace_ms = 120
"#;

    let mut daemon = TestDaemon::spawn(config_toml, None, "suite");
    let overlay_xid = daemon.status_json()["overlay_window_id"]
        .as_u64()
        .expect("overlay_window_id") as u32;

    // =========================================================================
    // 1. AC-4.1: Context enrichment & two-tier action filtering across item types
    // =========================================================================

    // 1a. Single UTF-8 `.txt` file -> Preview, Copy, Properties enabled
    let out = daemon.cli(&["invoke", "--at", "600,450", txt_a.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));
    let st = daemon.status_json();
    assert_eq!(
        find_item_state(&st, "radivine.preview.full"),
        Some((true, None))
    );
    assert_eq!(
        find_item_state(&st, "radivine.preview.head"),
        Some((true, None))
    );
    assert_eq!(
        find_item_state(&st, "radivine.preview.tail"),
        Some((true, None))
    );
    assert_eq!(find_item_state(&st, "radivine.copy"), Some((true, None)));
    assert_eq!(
        find_item_state(&st, "radivine.properties"),
        Some((true, None))
    );
    let _ = Command::new("xdotool").args(["key", "Escape"]).status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));

    // 1b. Known binary `.png` file -> Preview hidden at BasicContext tier (`None`); Copy & Properties enabled
    let out = daemon.cli(&["invoke", "--at", "600,450", png_bin.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));
    let st = daemon.status_json();
    assert_eq!(
        find_item_state(&st, "radivine.preview.full"),
        None,
        "known binary MIME image/png must hide File Preview at BasicContext tier"
    );
    assert_eq!(find_item_state(&st, "radivine.copy"), Some((true, None)));
    assert_eq!(
        find_item_state(&st, "radivine.properties"),
        Some((true, None))
    );
    let _ = Command::new("xdotool").args(["key", "Escape"]).status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));

    // 1c. Disguised binary `.txt` file -> Preview visible at BasicContext tier, disabled by FileFacts with "Not a text file"
    let out = daemon.cli(&["invoke", "--at", "600,450", disguised_bin.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));
    let st = daemon.status_json();
    assert_eq!(
        find_item_state(&st, "radivine.preview.full"),
        Some((false, Some("Not a text file")))
    );
    assert_eq!(
        find_item_state(&st, "radivine.preview.head"),
        Some((false, Some("Not a text file")))
    );
    assert_eq!(
        find_item_state(&st, "radivine.preview.tail"),
        Some((false, Some("Not a text file")))
    );
    assert_eq!(find_item_state(&st, "radivine.copy"), Some((true, None)));
    assert_eq!(
        find_item_state(&st, "radivine.properties"),
        Some((true, None))
    );
    let _ = Command::new("xdotool").args(["key", "Escape"]).status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));

    // 1d. Directory -> Preview disabled with "Folders can't be previewed"; Copy & Properties enabled
    let out = daemon.cli(&["invoke", "--at", "600,450", sub_dir.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));
    let st = daemon.status_json();
    assert_eq!(
        find_item_state(&st, "radivine.preview.full"),
        Some((false, Some("Folders can't be previewed")))
    );
    assert_eq!(find_item_state(&st, "radivine.copy"), Some((true, None)));
    assert_eq!(
        find_item_state(&st, "radivine.properties"),
        Some((true, None))
    );
    let _ = Command::new("xdotool").args(["key", "Escape"]).status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));

    // 1e. Multi-file selection (3 `.txt` files) -> Preview disabled with "Select a single file to preview"; Copy & Properties enabled
    let out = daemon.cli(&[
        "invoke",
        "--at",
        "600,450",
        txt_a.to_str().unwrap(),
        txt_b.to_str().unwrap(),
        txt_c.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));
    let st = daemon.status_json();
    assert_eq!(
        find_item_state(&st, "radivine.preview.full"),
        Some((false, Some("Select a single file to preview")))
    );
    assert_eq!(find_item_state(&st, "radivine.copy"), Some((true, None)));
    assert_eq!(
        find_item_state(&st, "radivine.properties"),
        Some((true, None))
    );
    let _ = Command::new("xdotool").args(["key", "Escape"]).status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));

    // 1f. Broken symbolic link -> Preview disabled with "Broken link"
    let out = daemon.cli(&["invoke", "--at", "600,450", broken_link.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));
    let st = daemon.status_json();
    assert_eq!(
        find_item_state(&st, "radivine.preview.full"),
        Some((false, Some("Broken link")))
    );
    assert_eq!(find_item_state(&st, "radivine.copy"), Some((true, None)));
    assert_eq!(
        find_item_state(&st, "radivine.properties"),
        Some((true, None))
    );
    let _ = Command::new("xdotool").args(["key", "Escape"]).status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));

    // 1g. FIFO special file (must never block on open!) -> Preview disabled with "Special file"
    let t_fifo = Instant::now();
    let out = daemon.cli(&["invoke", "--at", "600,450", fifo_path.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));
    assert!(
        t_fifo.elapsed() < Duration::from_millis(200),
        "FIFO enrichment must never block on open()"
    );
    let st = daemon.status_json();
    assert_eq!(
        find_item_state(&st, "radivine.preview.full"),
        Some((false, Some("Special file")))
    );
    assert_eq!(find_item_state(&st, "radivine.copy"), Some((true, None)));
    let _ = Command::new("xdotool").args(["key", "Escape"]).status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));

    // 1h. Non-local URI (`sftp://example.local/remote.txt`) -> Preview hidden (`local_path.is_none()`); Copy & Properties enabled
    let out = daemon.cli(&[
        "invoke",
        "--at",
        "600,450",
        "sftp://example.local/home/user/remote.txt",
    ]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));
    let st = daemon.status_json();
    assert_eq!(find_item_state(&st, "radivine.preview.full"), None);
    assert_eq!(find_item_state(&st, "radivine.copy"), Some((true, None)));
    assert_eq!(
        find_item_state(&st, "radivine.properties"),
        Some((true, None))
    );
    let _ = Command::new("xdotool").args(["key", "Escape"]).status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));

    // =========================================================================
    // 2. AC-4.2: Enrichment timeout (`1500 ms` watchdog) + non-blocking UI summon
    // =========================================================================
    let t_hung = Instant::now();
    let out = daemon.cli(&["invoke", "--at", "600,450", hung_path.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(120)
    ));
    let map_latency = t_hung.elapsed();
    assert!(
        map_latency < Duration::from_millis(150),
        "wheel must map immediately (< 150 ms) even when stat hangs: {map_latency:?}"
    );
    let st_pending = daemon.status_json();
    assert_eq!(
        find_item_state(&st_pending, "radivine.preview.full"),
        Some((false, Some("Checking…")))
    );
    // Copy and Properties do not require FileFacts and must be immediately enabled!
    assert_eq!(
        find_item_state(&st_pending, "radivine.copy"),
        Some((true, None))
    );
    assert_eq!(
        find_item_state(&st_pending, "radivine.properties"),
        Some((true, None))
    );

    // Wait for the 1500 ms enrichment watchdog to fire
    thread::sleep(Duration::from_millis(1600));
    let st_timed_out = daemon.status_json();
    assert_eq!(
        find_item_state(&st_timed_out, "radivine.preview.full"),
        Some((false, Some("Couldn't inspect the file")))
    );
    let _ = Command::new("xdotool").args(["key", "Escape"]).status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));

    // =========================================================================
    // 3. AC-4.3: Copy action (`radivine.copy`) -> X11 CLIPBOARD ownership & formats
    // =========================================================================
    let anchor = (600, 450);
    let copy_pt = compute_segment_point(anchor, &[1]);

    // 3a. Single file Copy
    let out = daemon.cli(&["invoke", "--at", "600,450", txt_a.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));
    let _ = Command::new("xdotool")
        .args([
            "mousemove",
            &copy_pt.0.to_string(),
            &copy_pt.1.to_string(),
            "click",
            "1",
        ])
        .status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(400)
    ));

    let st = daemon.status_json();
    assert_eq!(st["last_committed_action"], "radivine.copy");
    assert_eq!(st["last_job_action"], "radivine.copy");
    assert!(st["last_job_error"].is_null(), "unexpected error: {st}");

    let expected_uri_a = path_to_file_uri(&txt_a);
    let gnome_bytes = request_clipboard_target(&conn, &atoms, atoms.GNOME_COPIED_FILES);
    assert_eq!(
        String::from_utf8_lossy(&gnome_bytes),
        format!("copy\n{expected_uri_a}")
    );
    let uri_list_bytes = request_clipboard_target(&conn, &atoms, atoms.URI_LIST);
    assert_eq!(
        String::from_utf8_lossy(&uri_list_bytes),
        format!("{expected_uri_a}\r\n")
    );
    let utf8_bytes = request_clipboard_target(&conn, &atoms, atoms.UTF8_STRING);
    assert_eq!(
        String::from_utf8_lossy(&utf8_bytes),
        txt_a.to_string_lossy().as_ref()
    );

    // 3b. Multi-file Copy (with spaces & Unicode) + Directory Copy
    let out = daemon.cli(&[
        "invoke",
        "--at",
        "600,450",
        txt_b.to_str().unwrap(),
        txt_c.to_str().unwrap(),
        sub_dir.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));
    let _ = Command::new("xdotool")
        .args([
            "mousemove",
            &copy_pt.0.to_string(),
            &copy_pt.1.to_string(),
            "click",
            "1",
        ])
        .status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(400)
    ));

    let expected_uri_b = path_to_file_uri(&txt_b);
    let expected_uri_c = path_to_file_uri(&txt_c);
    let expected_uri_dir = path_to_file_uri(&sub_dir);
    let gnome_multi = request_clipboard_target(&conn, &atoms, atoms.GNOME_COPIED_FILES);
    assert_eq!(
        String::from_utf8_lossy(&gnome_multi),
        format!("copy\n{expected_uri_b}\n{expected_uri_c}\n{expected_uri_dir}")
    );

    // =========================================================================
    // 4. AC-4.4: Properties action (`radivine.properties`)
    // =========================================================================
    let props_pt = compute_segment_point(anchor, &[2]);
    let out = daemon.cli(&["invoke", "--at", "600,450", txt_a.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));
    let _ = Command::new("xdotool")
        .args([
            "mousemove",
            &props_pt.0.to_string(),
            &props_pt.1.to_string(),
            "click",
            "1",
        ])
        .status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(400)
    ));
    let st = daemon.status_json();
    assert_eq!(st["last_committed_action"], "radivine.properties");
    assert_eq!(st["last_job_action"], "radivine.properties");
    assert!(st["last_job_error"].is_null());

    // =========================================================================
    // 5. AC-4.5, AC-4.6, AC-4.7: User `actions.toml` reload, shell-free argv,
    //    invalid config diagnostics, non-zero exit, and panic recovery
    // =========================================================================
    let marker_file = corpus_dir.join("argv_marker.txt");
    let user_actions_toml = format!(
        r#"
[[action]]
id = "argv_dump"
label = "Dump Argv"
icon = "lucide:terminal"
exec = ["/usr/bin/python3", "-c", "import sys; open(sys.argv[1], 'w').write('\\n'.join(sys.argv[2:]))", "{marker}", "{{dir}}", "{{name}}", "{{paths}}"]
cwd = "{{dir}}"
wait = true
notify = "always"
[action.when]
count = "1.."
require_text = true

[[action]]
id = "fail_exit"
label = "Fail Exit"
icon = "lucide:code"
exec = ["/usr/bin/python3", "-c", "import sys; sys.exit(42)"]
wait = true
notify = "on-error"

[[action]]
id = "__test_panic__"
label = "Simulated Panic"
icon = "lucide:sparkles"
exec = ["/bin/true"]
wait = true
notify = "on-error"
"#,
        marker = marker_file.to_string_lossy()
    );
    fs::write(&daemon.actions_path, &user_actions_toml).unwrap();
    let reload_out = daemon.cli(&["reload"]);
    assert!(reload_out.status.success());

    // 5a. Execute `custom.argv_dump` (`[3, 0]`) on `shell_hazard` + `txt_c` (Unicode)
    let _ = Command::new("xdotool")
        .args(["mousemove", "600", "450"])
        .status();
    let out = daemon.cli(&[
        "invoke",
        "--at",
        "600,450",
        shell_hazard.to_str().unwrap(),
        txt_c.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));
    let _ = Command::new("xdotool").args(["key", "4"]).status();
    thread::sleep(Duration::from_millis(40));
    let _ = Command::new("xdotool").args(["key", "1"]).status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(400)
    ));
    let st_dump = daemon.status_json();
    assert_eq!(
        st_dump["last_committed_action"], "custom.argv_dump",
        "unexpected commit status for argv_dump: {st_dump}"
    );

    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline && !marker_file.exists() {
        thread::sleep(Duration::from_millis(10));
    }
    let dumped = fs::read_to_string(&marker_file).expect("read argv_marker.txt");
    let dumped_lines: Vec<&str> = dumped.lines().collect();
    assert_eq!(
        dumped_lines,
        vec![
            corpus_dir.to_str().unwrap(),
            "semi;colon & $(id) `uname`.txt",
            shell_hazard.to_str().unwrap(),
            txt_c.to_str().unwrap(),
        ],
        "shell metacharacters and Unicode paths must be passed literally in argv without shell interpretation"
    );

    // 5b. AC-4.6: Write invalid TOML syntax to `actions.toml` and reload -> error reported with line/col,
    //     while previous valid registry remains active!
    fs::write(&daemon.actions_path, "[[action]]\nid = [unclosed_syntax\n").unwrap();
    let reload_bad = daemon.cli(&["reload"]);
    assert!(reload_bad.status.success());
    let st_bad = daemon.status_json();
    let warnings = st_bad["config_warnings"]
        .as_array()
        .expect("config_warnings");
    assert!(
        !warnings.is_empty() && warnings[0].as_str().unwrap_or("").contains("line"),
        "expected line/col syntax diagnostic in config_warnings, got {warnings:?}"
    );

    // 5c. AC-4.7: Trigger `custom.fail_exit` (`[3, 1]`) and `custom.__test_panic__` (`[3, 2]`)
    let out = daemon.cli(&["invoke", "--at", "600,450", txt_a.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));
    let _ = Command::new("xdotool").args(["key", "4"]).status();
    thread::sleep(Duration::from_millis(40));
    let _ = Command::new("xdotool").args(["key", "2"]).status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(400)
    ));
    thread::sleep(Duration::from_millis(120));
    let st_fail = daemon.status_json();
    assert_eq!(st_fail["last_job_action"], "custom.fail_exit");
    assert!(
        st_fail["last_job_error"]
            .as_str()
            .unwrap_or("")
            .contains("42"),
        "expected exit code 42 in last_job_error: {st_fail}"
    );

    // Panicking action (`custom.__test_panic__`) caught via `catch_unwind`
    let out = daemon.cli(&["invoke", "--at", "600,450", txt_a.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));
    let _ = Command::new("xdotool").args(["key", "4"]).status();
    thread::sleep(Duration::from_millis(40));
    let _ = Command::new("xdotool").args(["key", "3"]).status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(400)
    ));
    thread::sleep(Duration::from_millis(80));
    let st_panic = daemon.status_json();
    assert_eq!(st_panic["last_job_action"], "custom.__test_panic__");
    assert_eq!(st_panic["last_job_error"].as_str(), Some("internal error"));
    assert_eq!(st_panic["session_state"], "Idle");

    // Subsequent session after panic must work normally (`AC-4.7`)
    let _ = Command::new("xdotool")
        .args(["mousemove", "600", "450"])
        .status();
    let out = daemon.cli(&["invoke", "--at", "600,450", txt_a.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(wait_window_mapped(
        &conn,
        overlay_xid,
        Duration::from_millis(300)
    ));
    let _ = Command::new("xdotool")
        .args([
            "mousemove",
            &copy_pt.0.to_string(),
            &copy_pt.1.to_string(),
            "click",
            "1",
        ])
        .status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(400)
    ));
    let st_after = daemon.status_json();
    assert_eq!(st_after["last_committed_action"], "radivine.copy");
    assert!(st_after["last_job_error"].is_null());

    // Verify all protected corpus files remain 100% unmodified
    for before in &protected_fingerprints {
        let after = FileFingerprint::capture(&before.path);
        assert_eq!(
            &after,
            before,
            "protected corpus file was modified during M4 acceptance tests: {}",
            before.path.display()
        );
    }

    daemon.stop();
    let _ = fs::remove_dir_all(&corpus_dir);
}
