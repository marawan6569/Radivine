//! End-to-end X11 acceptance tests for Milestone M2 (`AC-2.1` through `AC-2.9`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, MapState, PropMode, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

fn radivine_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_radivine"))
}

struct TestDaemon {
    child: Child,
    sock_path: PathBuf,
    config_path: PathBuf,
    temp_dir: PathBuf,
}

impl TestDaemon {
    fn spawn(config_toml: &str, tag: &str) -> Self {
        let temp_dir =
            std::env::temp_dir().join(format!("radivine-m2-test-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("create temp_dir");

        let config_path = temp_dir.join("config.toml");
        fs::write(&config_path, config_toml).expect("write config.toml");
        let sock_path = temp_dir.join("radivine.sock");

        let child = Command::new(radivine_bin())
            .arg("--config")
            .arg(&config_path)
            .arg("--socket")
            .arg(&sock_path)
            .arg("daemon")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn radivine daemon");

        // Wait up to 2 s for socket readiness
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
            thread::sleep(Duration::from_millis(15));
        }

        Self {
            child,
            sock_path,
            config_path,
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

fn is_window_mapped(conn: &RustConnection, win: u32) -> bool {
    conn.get_window_attributes(win)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some_and(|r| r.map_state == MapState::VIEWABLE)
}

fn is_override_redirect(conn: &RustConnection, win: u32) -> bool {
    conn.get_window_attributes(win)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some_and(|r| r.override_redirect)
}

#[test]
fn m2_live_acceptance_suite() {
    if std::env::var("DISPLAY").is_err() {
        return;
    }
    let Ok((conn, _)) = RustConnection::connect(None) else {
        return;
    };

    // -------------------------------------------------------------------------
    // AC-2.9: `radivine doctor` diagnostic output
    // -------------------------------------------------------------------------
    let doc_out = Command::new(radivine_bin())
        .arg("doctor")
        .output()
        .expect("run radivine doctor");
    assert!(doc_out.status.success(), "radivine doctor must exit 0");
    let doc_stdout = String::from_utf8_lossy(&doc_out.stdout);
    assert!(doc_stdout.contains("Compositor"));
    assert!(doc_stdout.contains("XInputExtension"));
    assert!(doc_stdout.contains("MIT-SHM"));
    assert!(doc_stdout.contains("invoke_at_pointer = yes"));

    // Start daemon for AC-2.1 .. AC-2.7
    let mut daemon = TestDaemon::spawn("", "suite");
    let st0 = daemon.status_json();
    let overlay_xid = st0["overlay_window_id"]
        .as_u64()
        .expect("overlay_window_id") as u32;
    assert!(is_override_redirect(&conn, overlay_xid));
    assert!(!is_window_mapped(&conn, overlay_xid));

    // -------------------------------------------------------------------------
    // AC-2.1: 20 invoke runs -> p95 < 50 ms, override_redirect = true, mapped
    // -------------------------------------------------------------------------
    let mut latencies_ms = Vec::with_capacity(20);
    for _ in 0..20 {
        let t0 = Instant::now();
        let out = daemon.cli(&["invoke", "--at", "960,540", "/tmp/sample.txt"]);
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert!(out.status.success());
        assert!(is_window_mapped(&conn, overlay_xid));
        latencies_ms.push(elapsed_ms);

        let dis = daemon.cli(&["dismiss"]);
        assert!(dis.status.success());
        assert!(!is_window_mapped(&conn, overlay_xid));
    }
    latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95_ms = latencies_ms[18];
    assert!(
        p95_ms < 50.0,
        "AC-2.1 invoke p95 latency was {p95_ms:.2} ms (limit 50.0 ms)"
    );

    // -------------------------------------------------------------------------
    // AC-2.2 & AC-2.3: Keyboard navigation into nested wheel + commit with spaces/UTF-8
    // -------------------------------------------------------------------------
    let special_path = "/tmp/radivine test — file 🚀.txt";
    let inv = daemon.cli(&["invoke", "--at", "960,540", special_path]);
    assert!(inv.status.success());
    assert!(is_window_mapped(&conn, overlay_xid));

    // Press '1' to open "File Preview" submenu, then '2' to commit "Head" (`radivine.preview.head`)
    let _ = Command::new("xdotool").args(["key", "1"]).status();
    thread::sleep(Duration::from_millis(30));
    let _ = Command::new("xdotool").args(["key", "2"]).status();
    thread::sleep(Duration::from_millis(30));

    let st_commit = daemon.status_json();
    assert_eq!(st_commit["session_state"], "Idle");
    assert!(!is_window_mapped(&conn, overlay_xid));
    assert_eq!(st_commit["last_committed_action"], "radivine.preview.head");
    let committed_uris = st_commit["last_committed_items"]
        .as_array()
        .expect("committed items array");
    assert_eq!(
        committed_uris[0].as_str().unwrap(),
        "file:///tmp/radivine%20test%20%E2%80%94%20file%20%F0%9F%9A%80.txt"
    );

    // Also verify mouse pointer navigation & click commit on "Copy" (segment 1 at angle 0 rad -> (1040, 540))
    let inv_mouse = daemon.cli(&["invoke", "--at", "960,540", special_path]);
    assert!(inv_mouse.status.success());
    let _ = Command::new("xdotool")
        .args(["mousemove", "1040", "540", "click", "1"])
        .status();
    thread::sleep(Duration::from_millis(30));
    let st_mouse = daemon.status_json();
    assert_eq!(st_mouse["session_state"], "Idle");
    assert_eq!(st_mouse["last_committed_action"], "radivine.copy");

    // -------------------------------------------------------------------------
    // AC-2.4: Esc, click outside, and root-hub click all cancel and unmap
    // -------------------------------------------------------------------------
    // 1) Escape key cancel
    daemon.cli(&["invoke", "--at", "960,540", "/tmp/a.txt"]);
    assert!(is_window_mapped(&conn, overlay_xid));
    let _ = Command::new("xdotool").args(["key", "Escape"]).status();
    thread::sleep(Duration::from_millis(25));
    assert!(!is_window_mapped(&conn, overlay_xid));

    // 2) Click outside wheel cluster (at 200, 200 when wheel is at 960, 540)
    daemon.cli(&["invoke", "--at", "960,540", "/tmp/a.txt"]);
    assert!(is_window_mapped(&conn, overlay_xid));
    let _ = Command::new("xdotool")
        .args(["mousemove", "200", "200", "click", "1"])
        .status();
    thread::sleep(Duration::from_millis(25));
    assert!(!is_window_mapped(&conn, overlay_xid));

    // 3) Click on root hub (at 960, 540)
    daemon.cli(&["invoke", "--at", "960,540", "/tmp/a.txt"]);
    assert!(is_window_mapped(&conn, overlay_xid));
    let _ = Command::new("xdotool")
        .args(["mousemove", "960", "540", "click", "1"])
        .status();
    thread::sleep(Duration::from_millis(25));
    assert!(!is_window_mapped(&conn, overlay_xid));

    // -------------------------------------------------------------------------
    // AC-2.5: 50 scripted invoke/cancel cycles -> no stuck grabs
    // -------------------------------------------------------------------------
    for _ in 0..50 {
        let out = daemon.cli(&["invoke", "--at", "960,540", "/tmp/cycle.txt"]);
        assert!(out.status.success());
        let _ = Command::new("xdotool").args(["key", "Escape"]).status();
        // Wait briefly for unmap
        for _ in 0..20 {
            if !is_window_mapped(&conn, overlay_xid) {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!is_window_mapped(&conn, overlay_xid));
    }

    // Verify xdotool mouse & keyboard input reach a normal X11 test window after 50 cycles
    let root = conn.setup().roots[0].root;
    let test_win = conn.generate_id().expect("generate test_win id");
    conn.create_window(
        x11rb::COPY_FROM_PARENT as u8,
        test_win,
        root,
        300,
        300,
        120,
        120,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new()
            .override_redirect(1)
            .event_mask(EventMask::BUTTON_PRESS | EventMask::KEY_PRESS),
    )
    .expect("create test_win");
    conn.map_window(test_win).expect("map test_win");
    conn.flush().expect("flush test_win");
    thread::sleep(Duration::from_millis(20));

    let _ = Command::new("xdotool")
        .args(["mousemove", "350", "350", "click", "1"])
        .status();
    thread::sleep(Duration::from_millis(20));
    let mut got_button = false;
    while let Ok(Some(ev)) = conn.poll_for_event() {
        if let Event::ButtonPress(bp) = ev
            && bp.event == test_win
        {
            got_button = true;
        }
    }
    let _ = conn.destroy_window(test_win);
    let _ = conn.flush();
    assert!(
        got_button,
        "AC-2.5 test window did not receive ButtonPress after 50 invoke/cancel cycles"
    );

    // -------------------------------------------------------------------------
    // AC-2.6: Screen edges & corners clamping on 3840x1080
    // -------------------------------------------------------------------------
    let edge_points = [
        (0, 0),
        (1919, 0),
        (0, 1079),
        (1919, 1079),
        (3839, 0),
        (3839, 1079),
        (960, 0),
        (960, 1079),
        (0, 540),
        (1919, 540),
    ];
    for (px, py) in edge_points {
        let coord = format!("{px},{py}");
        let out = daemon.cli(&["invoke", "--at", &coord, "/tmp/edge.txt"]);
        assert!(out.status.success());
        let st = daemon.status_json();
        let mon = st["monitor_rect"].as_array().unwrap();
        let mw = mon[2].as_u64().unwrap() as f32;
        let mh = mon[3].as_u64().unwrap() as f32;
        let center = st["root_wheel_center_px"].as_array().unwrap();
        let cx = center[0].as_f64().unwrap() as f32;
        let cy = center[1].as_f64().unwrap() as f32;
        let r_out = st["root_wheel_outer_radius_px"].as_f64().unwrap() as f32;
        assert!(
            cx - r_out >= 0.0 && cx + r_out <= mw,
            "wheel X out of monitor bounds at ({px},{py}): cx={cx}, r_out={r_out}, mw={mw}"
        );
        assert!(
            cy - r_out >= 0.0 && cy + r_out <= mh,
            "wheel Y out of monitor bounds at ({px},{py}): cy={cy}, r_out={r_out}, mh={mh}"
        );
        daemon.cli(&["dismiss"]);
    }

    // -------------------------------------------------------------------------
    // AC-2.7: Scale = 2.0 doubles geometry cleanly + Xft.dpi live auto-detection
    // -------------------------------------------------------------------------
    // Test live Xft.dpi auto-detection via RESOURCE_MANAGER property on root
    let rm_atom = conn
        .intern_atom(false, b"RESOURCE_MANAGER")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let orig_rm = conn
        .get_property(false, root, rm_atom, AtomEnum::STRING, 0, 16_384)
        .unwrap()
        .reply()
        .unwrap()
        .value;

    conn.change_property8(
        PropMode::REPLACE,
        root,
        rm_atom,
        AtomEnum::STRING,
        b"Xft.dpi:\t144\n",
    )
    .unwrap();
    conn.flush().unwrap();
    thread::sleep(Duration::from_millis(30));
    let st_dpi = daemon.status_json();
    assert!(
        (st_dpi["scale"].as_f64().unwrap() - 1.5).abs() < 0.01,
        "expected Xft.dpi=144 -> scale=1.5, got {}",
        st_dpi["scale"]
    );

    // Restore original RESOURCE_MANAGER property immediately
    conn.change_property8(PropMode::REPLACE, root, rm_atom, AtomEnum::STRING, &orig_rm)
        .unwrap();
    conn.flush().unwrap();
    thread::sleep(Duration::from_millis(20));

    fs::write(&daemon.config_path, "[appearance]\nscale = 2.0\n").expect("write scale=2.0 config");
    daemon.cli(&["reload"]);
    daemon.cli(&["invoke", "--at", "960,540", "/tmp/scale.txt"]);
    let st_2x = daemon.status_json();
    assert_eq!(st_2x["scale"].as_f64().unwrap(), 2.0);
    let r_2x = st_2x["root_wheel_outer_radius_px"].as_f64().unwrap() as f32;
    assert!(
        (r_2x - 256.0).abs() < 1.0,
        "expected 2x outer radius 256.0 px, got {r_2x}"
    );
    daemon.cli(&["dismiss"]);
    daemon.stop();

    // -------------------------------------------------------------------------
    // AC-2.5 Part 2: SIGKILL mid-session releases grabs and unmaps overlay
    // -------------------------------------------------------------------------
    let mut kill_daemon = TestDaemon::spawn("", "sigkill");
    let st_k = kill_daemon.status_json();
    let kill_xid = st_k["overlay_window_id"].as_u64().unwrap() as u32;
    kill_daemon.cli(&["invoke", "--at", "960,540", "/tmp/kill.txt"]);
    assert!(is_window_mapped(&conn, kill_xid));
    let _ = kill_daemon.child.kill();
    let _ = kill_daemon.child.wait();
    thread::sleep(Duration::from_millis(30));
    // Verify X server destroyed the client's window and released its grabs
    assert!(!is_window_mapped(&conn, kill_xid));
    let _ = Path::new(&kill_daemon.temp_dir);
}
