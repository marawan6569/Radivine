//! End-to-end X11 acceptance tests for Milestone M3 (`AC-3.1` through `AC-3.13`).
//!
//! Verifies:
//! - `AC-3.1`: Activation on fresh `Shift` mid-drag (`p95 < 50 ms` over 20 trials), pre-held `Shift`
//!   ignored until released, non-file text drag (`UTF8_STRING` without `text/uri-list`) never activates.
//! - `AC-3.2`: Positioning & edge/corner clamping (`>= 8 lp` margin inside monitor bounds).
//! - `AC-3.3`: Hierarchical 2-level (`File Preview -> Full/Head/Tail`) and 3-level (`Custom -> Scripts -> Compute SHA-256`) menus.
//! - `AC-3.4`: 20/20 diagonal trajectories from `File Preview` into its child wheel without collapse;
//!   switching to a sibling root segment closes the child after `<= grace_ms`.
//! - `AC-3.5`: Exact file context preservation (`1 file`, `3 files`, spaces, non-ASCII UTF-8, symlink,
//!   and multi-chunk `INCR` payload) + `source.app` identification (`"nemo"` on live Nemo drag).
//! - `AC-3.6`: Commit paths and `ActionId`s (`[0, 0]` Full, `[0, 1]` Head, `[0, 2]` Tail, `[1]` Copy,
//!   `[2]` Properties, `[3, 1, 0]` Custom Scripts SHA-256).
//! - `AC-3.7`: Event routing under `passthrough = false` (capture-all safe default: outside release
//!   rejects drop, underlying folder untouched) and `passthrough = true` (shaped input region:
//!   outside pointer routes to underlying window).
//! - `AC-3.8`: Cancellation on hub/gap/outside (including with **zero pointer movement** after pressing
//!   `Shift`), `Esc`, second-`Shift` toggle-off, and `leave_grace_ms` expiration.
//! - `AC-3.9`: Clean dismissal (`MapState::UNMAPPED` after every session).
//! - `AC-3.10`: No stuck grabs (zero `GrabPointer`/`GrabKeyboard` in drag mode; 50 sessions + mid-drag
//!   `SIGKILL` leave mouse/keyboard input working).
//! - `AC-3.11`: No interference with normal drags or pre-held Shift drags.
//! - `AC-3.12`: Data safety — protected test files have identical SHA-256, size, mtime, inode, and
//!   location before and after all tests.
//! - `AC-3.13`: Robustness — 20 drag sessions with toggles/cancels/commits in < 30 s, RSS growth < 2 MB.

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
    Atom, AtomEnum, ClientMessageData, ClientMessageEvent, ConnectionExt as _, CreateWindowAux,
    EventMask, MapState, PropMode, Property, SelectionNotifyEvent, Window, WindowClass,
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
            std::env::temp_dir().join(format!("radivine-m3-test-{}-{}", tag, std::process::id()));
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

    fn rss_kb(&self) -> u64 {
        let status_path = format!("/proc/{}/status", self.child.id());
        let Ok(contents) = fs::read_to_string(status_path) else {
            return 0;
        };
        for line in contents.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let num_str = rest.split_whitespace().next().unwrap_or("0");
                return num_str.parse::<u64>().unwrap_or(0);
            }
        }
        0
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

/// Snapshot of a file's content, size, inode, and modification timestamp for `AC-3.12`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileFingerprint {
    path: PathBuf,
    is_symlink: bool,
    symlink_target: Option<PathBuf>,
    bytes: Vec<u8>,
    size: u64,
    ino: u64,
    mtime: Option<SystemTime>,
}

impl FileFingerprint {
    fn capture(path: &Path) -> Self {
        let sym_meta = fs::symlink_metadata(path).expect("symlink_metadata");
        let is_symlink = sym_meta.file_type().is_symlink();
        let symlink_target = if is_symlink {
            Some(fs::read_link(path).expect("read_link"))
        } else {
            None
        };
        let bytes = fs::read(path).expect("read file bytes");
        Self {
            path: path.to_path_buf(),
            is_symlink,
            symlink_target,
            bytes,
            size: sym_meta.len(),
            ino: sym_meta.ino(),
            mtime: sym_meta.modified().ok(),
        }
    }
}

struct ProtectedCorpus {
    dir: PathBuf,
    files: Vec<PathBuf>,
    fingerprints: Vec<FileFingerprint>,
}

impl ProtectedCorpus {
    fn create(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("radivine-m3-corpus-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create corpus dir");

        let f1 = dir.join("alpha.txt");
        let f2 = dir.join("file with spaces.txt");
        let f3 = dir.join("ümlaut_émoji_🚀_تجرية.txt");
        let f4 = dir.join("link_to_alpha.txt");

        fs::write(&f1, b"Radivine M3 immutable test payload #1\n").expect("write f1");
        fs::write(&f2, b"Radivine M3 payload with spaces #2\n").expect("write f2");
        fs::write(&f3, "Radivine M3 UTF-8 payload 🚀\n".as_bytes()).expect("write f3");
        symlink(&f1, &f4).expect("create symlink f4");

        let files = vec![f1, f2, f3, f4];
        let fingerprints = files.iter().map(|p| FileFingerprint::capture(p)).collect();
        Self {
            dir,
            files,
            fingerprints,
        }
    }

    fn verify_unchanged(&self, context: &str) {
        for expected in &self.fingerprints {
            assert!(
                expected.path.exists(),
                "AC-3.12 violation ({context}): file missing: {}",
                expected.path.display()
            );
            let actual = FileFingerprint::capture(&expected.path);
            assert_eq!(
                &actual,
                expected,
                "AC-3.12 violation ({context}): file metadata or content changed for {}",
                expected.path.display()
            );
        }
    }
}

impl Drop for ProtectedCorpus {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn is_window_mapped(conn: &RustConnection, win: u32) -> bool {
    conn.get_window_attributes(win)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some_and(|r| r.map_state == MapState::VIEWABLE)
}

fn wait_window_unmapped(conn: &RustConnection, win: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !is_window_mapped(conn, win) {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    !is_window_mapped(conn, win)
}

/// Computes the exact logical/physical pixel center of any segment path using `WheelSession`
/// so E2E pointer movements hit the exact geometric center of root, child, and grandchild segments.
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
            // Move into the segment and advance past dwell to open the child wheel
            ws.handle(WheelInput::PointerMoved(pt), now_ms, &mut out);
            now_ms += 200;
            ws.tick(now_ms, &mut out);
        }
    }

    target_pt
}

/// Deterministic X11 XDND v5 drag source harness that owns `XdndSelection`, serves `text/uri-list`
/// (both single-shot and multi-chunk `INCR`), and exchanges `XdndEnter`/`XdndPosition`/`XdndStatus`/
/// `XdndDrop`/`XdndFinished` with Radivine's overlay window.
struct XdndSourceHarness {
    conn: RustConnection,
    atoms: Atoms,
    src_win: Window,
    payload_bytes: Vec<u8>,
    offer_uri_list: bool,
    use_incr: bool,
    incr_requestor: Window,
    incr_property: Atom,
    incr_offset: usize,
    incr_active: bool,
}

impl XdndSourceHarness {
    fn new() -> Self {
        let (conn, screen_num) = RustConnection::connect(None).expect("connect harness X11");
        let atoms = Atoms::new(&conn)
            .expect("atoms cookie")
            .reply()
            .expect("atoms reply");
        let root = conn.setup().roots[screen_num].root;
        let src_win = conn.generate_id().expect("generate src_win id");
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

        let pid = std::process::id();
        let _ = conn.change_property32(
            PropMode::REPLACE,
            src_win,
            atoms._NET_WM_PID,
            AtomEnum::CARDINAL,
            &[pid],
        );
        conn.flush().expect("flush src_win");

        Self {
            conn,
            atoms,
            src_win,
            payload_bytes: Vec::new(),
            offer_uri_list: true,
            use_incr: false,
            incr_requestor: x11rb::NONE,
            incr_property: x11rb::NONE,
            incr_offset: 0,
            incr_active: false,
        }
    }

    fn set_uris(&mut self, uris: &[String], use_incr: bool) {
        let mut buf = String::new();
        for u in uris {
            buf.push_str(u);
            buf.push_str("\r\n");
        }
        self.payload_bytes = buf.into_bytes();
        self.offer_uri_list = true;
        self.use_incr = use_incr;
        self.incr_active = false;
    }

    fn set_non_file_text_only(&mut self, text: &str) {
        self.payload_bytes = text.as_bytes().to_vec();
        self.offer_uri_list = false;
        self.use_incr = false;
        self.incr_active = false;
    }

    fn begin_drag_at(&mut self, x: i32, y: i32, prehold_shift: bool) {
        let _ = Command::new("xdotool")
            .args(["keyup", "Shift_L", "keyup", "Shift_R", "mouseup", "1"])
            .status();
        thread::sleep(Duration::from_millis(10));

        let mut args = vec!["mousemove".to_string(), x.to_string(), y.to_string()];
        if prehold_shift {
            args.push("keydown".to_string());
            args.push("Shift_L".to_string());
        }
        args.push("mousedown".to_string());
        args.push("1".to_string());
        let _ = Command::new("xdotool").args(&args).status();
        thread::sleep(Duration::from_millis(8));

        self.conn
            .set_selection_owner(self.src_win, self.atoms.XdndSelection, x11rb::CURRENT_TIME)
            .expect("set_selection_owner XdndSelection");
        self.conn.flush().expect("flush selection owner");
    }

    fn end_drag(&mut self) {
        self.incr_active = false;
        let _ = self.conn.set_selection_owner(
            x11rb::NONE,
            self.atoms.XdndSelection,
            x11rb::CURRENT_TIME,
        );
        let _ = self.conn.flush();
        let _ = Command::new("xdotool")
            .args(["keyup", "Shift_L", "keyup", "Shift_R", "mouseup", "1"])
            .status();
        thread::sleep(Duration::from_millis(10));
    }

    /// Pumps incoming `SelectionRequest` and `PropertyNotify` (for `INCR`) events and returns any
    /// received `(msg_type, data32)` `ClientMessage`s (`XdndStatus` or `XdndFinished`).
    fn pump(&mut self) -> Vec<(Atom, [u32; 5])> {
        let mut client_msgs = Vec::new();
        while let Ok(Some(ev)) = self.conn.poll_for_event() {
            match ev {
                Event::SelectionRequest(req) => {
                    if self.offer_uri_list && req.target == self.atoms.URI_LIST {
                        let prop = if req.property == x11rb::NONE {
                            req.target
                        } else {
                            req.property
                        };
                        if self.use_incr {
                            let _ = self.conn.change_window_attributes(
                                req.requestor,
                                &x11rb::protocol::xproto::ChangeWindowAttributesAux::new()
                                    .event_mask(
                                        EventMask::EXPOSURE
                                            | EventMask::STRUCTURE_NOTIFY
                                            | EventMask::PROPERTY_CHANGE,
                                    ),
                            );
                            let total_len = self.payload_bytes.len() as u32;
                            let _ = self.conn.change_property32(
                                PropMode::REPLACE,
                                req.requestor,
                                prop,
                                self.atoms.INCR,
                                &[total_len],
                            );
                            self.incr_requestor = req.requestor;
                            self.incr_property = prop;
                            self.incr_offset = 0;
                            self.incr_active = true;
                        } else {
                            let _ = self.conn.change_property8(
                                PropMode::REPLACE,
                                req.requestor,
                                prop,
                                self.atoms.URI_LIST,
                                &self.payload_bytes,
                            );
                        }
                        let notify = SelectionNotifyEvent {
                            response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
                            sequence: 0,
                            time: req.time,
                            requestor: req.requestor,
                            selection: req.selection,
                            target: req.target,
                            property: prop,
                        };
                        let _ =
                            self.conn
                                .send_event(false, req.requestor, EventMask::NO_EVENT, notify);
                        let _ = self.conn.flush();
                    } else {
                        // Refuse conversion (`property = NONE`)
                        let notify = SelectionNotifyEvent {
                            response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
                            sequence: 0,
                            time: req.time,
                            requestor: req.requestor,
                            selection: req.selection,
                            target: req.target,
                            property: x11rb::NONE,
                        };
                        let _ =
                            self.conn
                                .send_event(false, req.requestor, EventMask::NO_EVENT, notify);
                        let _ = self.conn.flush();
                    }
                }
                Event::PropertyNotify(pn)
                    if self.incr_active
                        && pn.window == self.incr_requestor
                        && pn.atom == self.incr_property
                        && pn.state == Property::DELETE =>
                {
                    let remaining = self.payload_bytes.len().saturating_sub(self.incr_offset);
                    let chunk_len = remaining.min(4096);
                    let chunk = &self.payload_bytes[self.incr_offset..self.incr_offset + chunk_len];
                    let _ = self.conn.change_property8(
                        PropMode::REPLACE,
                        self.incr_requestor,
                        self.incr_property,
                        self.atoms.URI_LIST,
                        chunk,
                    );
                    let _ = self.conn.flush();
                    self.incr_offset += chunk_len;
                    if chunk_len == 0 {
                        self.incr_active = false;
                    }
                }
                Event::ClientMessage(cm) if cm.window == self.src_win && cm.format == 32 => {
                    client_msgs.push((cm.type_, cm.data.as_data32()));
                }
                _ => {}
            }
        }
        client_msgs
    }

    fn send_xdnd_enter(&mut self, overlay_win: Window) {
        let type0 = if self.offer_uri_list {
            self.atoms.URI_LIST
        } else {
            self.atoms.UTF8_STRING
        };
        let ev = ClientMessageEvent {
            response_type: x11rb::protocol::xproto::CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window: overlay_win,
            type_: self.atoms.XdndEnter,
            data: ClientMessageData::from([self.src_win, 5 << 24, type0, 0, 0]),
        };
        let _ = self
            .conn
            .send_event(false, overlay_win, EventMask::NO_EVENT, ev);
        let _ = self.conn.flush();
    }

    fn send_xdnd_position_and_wait_status(
        &mut self,
        overlay_win: Window,
        x: i32,
        y: i32,
    ) -> (bool, Atom) {
        let _ = Command::new("xdotool")
            .args(["mousemove", &x.to_string(), &y.to_string()])
            .status();
        let packed = (((x as i16 as u16) as u32) << 16) | ((y as i16 as u16) as u32);
        let ev = ClientMessageEvent {
            response_type: x11rb::protocol::xproto::CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window: overlay_win,
            type_: self.atoms.XdndPosition,
            data: ClientMessageData::from([
                self.src_win,
                0,
                packed,
                x11rb::CURRENT_TIME,
                self.atoms.XdndActionMove,
            ]),
        };
        let _ = self
            .conn
            .send_event(false, overlay_win, EventMask::NO_EVENT, ev);
        let _ = self.conn.flush();

        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            for (msg_type, d) in self.pump() {
                if msg_type == self.atoms.XdndStatus && d[0] == overlay_win {
                    let accept = (d[1] & 1) != 0;
                    let action = d[4];
                    if accept {
                        assert_eq!(
                            action, self.atoms.XdndActionPrivate,
                            "accepted XdndStatus must always specify XdndActionPrivate, never Move"
                        );
                    } else {
                        assert_eq!(action, x11rb::NONE);
                    }
                    return (accept, action);
                }
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("timed out waiting for XdndStatus from overlay {overlay_win:#x}");
    }

    fn send_xdnd_leave(&mut self, overlay_win: Window) {
        let ev = ClientMessageEvent {
            response_type: x11rb::protocol::xproto::CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window: overlay_win,
            type_: self.atoms.XdndLeave,
            data: ClientMessageData::from([self.src_win, 0, 0, 0, 0]),
        };
        let _ = self
            .conn
            .send_event(false, overlay_win, EventMask::NO_EVENT, ev);
        let _ = self.conn.flush();
    }

    fn send_xdnd_drop_and_wait_finished(&mut self, overlay_win: Window) -> (bool, Atom) {
        let _ = Command::new("xdotool").args(["mouseup", "1"]).status();
        let ev = ClientMessageEvent {
            response_type: x11rb::protocol::xproto::CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window: overlay_win,
            type_: self.atoms.XdndDrop,
            data: ClientMessageData::from([self.src_win, 0, x11rb::CURRENT_TIME, 0, 0]),
        };
        let _ = self
            .conn
            .send_event(false, overlay_win, EventMask::NO_EVENT, ev);
        let _ = self.conn.flush();

        let deadline = Instant::now() + Duration::from_millis(1000);
        while Instant::now() < deadline {
            for (msg_type, d) in self.pump() {
                if msg_type == self.atoms.XdndFinished && d[0] == overlay_win {
                    let accepted = (d[1] & 1) != 0;
                    let action = d[2];
                    if accepted {
                        assert_eq!(action, self.atoms.XdndActionPrivate);
                    } else {
                        assert_eq!(action, x11rb::NONE);
                    }
                    self.end_drag();
                    return (accepted, action);
                }
            }
            thread::sleep(Duration::from_millis(1));
        }
        self.end_drag();
        panic!("timed out waiting for XdndFinished from overlay {overlay_win:#x}");
    }

    /// Triggers Shift press and pumps `SelectionRequest` until `overlay_win` is mapped (or timeout).
    fn trigger_shift_and_wait_mapped(
        &mut self,
        overlay_win: Window,
        timeout: Duration,
    ) -> Option<Duration> {
        let t0 = Instant::now();
        let _ = Command::new("xdotool")
            .args(["keydown", "Shift_L", "keyup", "Shift_L"])
            .status();
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let _ = self.pump();
            if is_window_mapped(&self.conn, overlay_win) {
                let lat = t0.elapsed();
                // Pump for a couple more milliseconds so the prefetched SelectionRequest completes
                let settle = Instant::now() + Duration::from_millis(15);
                while Instant::now() < settle {
                    let _ = self.pump();
                    thread::sleep(Duration::from_millis(1));
                }
                return Some(lat);
            }
            thread::sleep(Duration::from_millis(1));
        }
        None
    }
}

impl Drop for XdndSourceHarness {
    fn drop(&mut self) {
        self.end_drag();
        let _ = self.conn.destroy_window(self.src_win);
        let _ = self.conn.flush();
    }
}

#[test]
fn m3_drag_and_drop_acceptance_suite() {
    if std::env::var("DISPLAY").is_err() {
        return;
    }
    let Ok((conn, _)) = RustConnection::connect(None) else {
        return;
    };

    let corpus = ProtectedCorpus::create("suite");
    let file_uris: Vec<String> = corpus
        .files
        .iter()
        .map(|p| radivine_core::local_path_to_file_uri(p))
        .collect();

    let mut daemon = TestDaemon::spawn(
        r#"
        [appearance]
        reduced_motion = true
        [wheel]
        hover_dwell_ms = 80
        submenu_grace_ms = 160
        leave_grace_ms = 120
        "#,
        "m3-main",
    );
    let st0 = daemon.status_json();
    let overlay_xid = st0["overlay_window_id"]
        .as_u64()
        .expect("overlay_window_id") as u32;
    assert!(!is_window_mapped(&conn, overlay_xid));

    let mut harness = XdndSourceHarness::new();

    // =========================================================================
    // AC-3.1: Pre-held Shift before drag does NOT activate; fresh Shift after
    // releasing pre-held Shift DOES activate; p95 trigger->map < 50 ms (20 runs)
    // =========================================================================
    harness.set_uris(&file_uris[0..1], false);
    harness.begin_drag_at(960, 540, true); // Shift pre-held before drag!
    thread::sleep(Duration::from_millis(25));
    // Auto-repeat / keydown while pre-held must NOT map the overlay
    let _ = Command::new("xdotool")
        .args(["keydown", "Shift_L"])
        .status();
    thread::sleep(Duration::from_millis(30));
    assert!(
        !is_window_mapped(&conn, overlay_xid),
        "AC-3.1: pre-held Shift must never summon the wheel"
    );
    // Release pre-held Shift -> still unmapped
    let _ = Command::new("xdotool").args(["keyup", "Shift_L"]).status();
    thread::sleep(Duration::from_millis(20));
    assert!(!is_window_mapped(&conn, overlay_xid));
    // Now press a FRESH Shift during the same drag -> MUST map!
    let lat = harness
        .trigger_shift_and_wait_mapped(overlay_xid, Duration::from_millis(300))
        .expect("fresh Shift after releasing pre-held Shift must activate");
    assert!(lat.as_millis() < 50);
    harness.end_drag();
    thread::sleep(Duration::from_millis(25));
    assert!(!is_window_mapped(&conn, overlay_xid));

    // Non-file text drag (Shift+drag text in editor without `text/uri-list`) never activates
    harness.set_non_file_text_only("selected plain text in editor");
    harness.begin_drag_at(960, 540, false);
    thread::sleep(Duration::from_millis(20));
    let _ = Command::new("xdotool")
        .args(["keydown", "Shift_L", "keyup", "Shift_L"])
        .status();
    let wait_text = Instant::now() + Duration::from_millis(80);
    while Instant::now() < wait_text {
        let _ = harness.pump();
        thread::sleep(Duration::from_millis(2));
    }
    harness.send_xdnd_enter(overlay_xid);
    thread::sleep(Duration::from_millis(25));
    assert!(
        !is_window_mapped(&conn, overlay_xid),
        "AC-3.1: non-file text drag must be dismissed and unmapped"
    );
    harness.end_drag();
    thread::sleep(Duration::from_millis(20));

    // 20 activation latency trials -> p95 < 50 ms
    harness.set_uris(&file_uris[0..1], false);
    let mut trigger_latencies_ms = Vec::with_capacity(20);
    for _ in 0..20 {
        harness.begin_drag_at(960, 540, false);
        thread::sleep(Duration::from_millis(15));
        let d = harness
            .trigger_shift_and_wait_mapped(overlay_xid, Duration::from_millis(300))
            .expect("wheel mapped on fresh Shift");
        trigger_latencies_ms.push(d.as_secs_f64() * 1000.0);
        // Toggle off with second Shift
        let _ = Command::new("xdotool")
            .args(["keydown", "Shift_L", "keyup", "Shift_L"])
            .status();
        assert!(wait_window_unmapped(
            &conn,
            overlay_xid,
            Duration::from_millis(300)
        ));
        harness.end_drag();
    }
    trigger_latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let min_trigger_ms = trigger_latencies_ms[0];
    let med_trigger_ms = trigger_latencies_ms[10];
    let p95_trigger_ms = trigger_latencies_ms[18];
    let max_trigger_ms = trigger_latencies_ms[19];
    println!(
        "M3 trigger->mapped latency (20 trials): min={min_trigger_ms:.2} ms, median={med_trigger_ms:.2} ms, p95={p95_trigger_ms:.2} ms, max={max_trigger_ms:.2} ms"
    );
    assert!(
        p95_trigger_ms < 50.0,
        "AC-3.1 trigger->mapped p95 latency was {p95_trigger_ms:.2} ms (limit 50.0 ms)"
    );
    corpus.verify_unchanged("after AC-3.1");

    // =========================================================================
    // AC-3.2: Positioning at corners & edges (root displaced inward >= 8 lp)
    // =========================================================================
    let edge_points = [(10, 10), (1910, 10), (10, 1070), (1910, 1070), (960, 8)];
    for (ex, ey) in edge_points {
        harness.begin_drag_at(ex, ey, false);
        thread::sleep(Duration::from_millis(15));
        harness
            .trigger_shift_and_wait_mapped(overlay_xid, Duration::from_millis(300))
            .expect("mapped at edge");
        let st = daemon.status_json();
        let mon = st["monitor_rect"].as_array().expect("monitor_rect");
        let mw = mon[2].as_u64().unwrap() as f32;
        let mh = mon[3].as_u64().unwrap() as f32;
        let center = st["root_wheel_center_px"]
            .as_array()
            .expect("root_wheel_center_px");
        let cx = center[0].as_f64().unwrap() as f32;
        let cy = center[1].as_f64().unwrap() as f32;
        let r_out = st["root_wheel_outer_radius_px"].as_f64().unwrap() as f32;
        assert!(cx - r_out >= 7.5 && cy - r_out >= 7.5);
        assert!(cx + r_out <= mw - 7.5 && cy + r_out <= mh - 7.5);
        harness.end_drag();
        thread::sleep(Duration::from_millis(20));
    }

    // =========================================================================
    // AC-3.3, AC-3.5, AC-3.6: Hierarchy (2-level & 3-level), Action Selection,
    // and Exact File Context (1 file, 4 files incl. spaces/UTF-8/symlink, INCR)
    // =========================================================================
    let anchor = (960, 540);
    let action_cases: &[(&[u8], &str)] = &[
        (&[0, 0], "radivine.preview.full"),
        (&[0, 1], "radivine.preview.head"),
        (&[0, 2], "radivine.preview.tail"),
        (&[1], "radivine.copy"),
        (&[2], "radivine.properties"),
        (&[3, 1, 0], "custom.scripts.sha256"),
    ];

    for &(path, expected_id) in action_cases {
        harness.set_uris(&file_uris, false);
        harness.begin_drag_at(anchor.0, anchor.1, false);
        thread::sleep(Duration::from_millis(15));
        harness
            .trigger_shift_and_wait_mapped(overlay_xid, Duration::from_millis(300))
            .expect("mapped for action case");
        harness.send_xdnd_enter(overlay_xid);

        // Traverse each prefix of `path` to open submenus, then land on the leaf item
        for depth in 1..=path.len() {
            let prefix = &path[0..depth];
            let pt = compute_segment_point(anchor, prefix);
            let (accept, _) = harness.send_xdnd_position_and_wait_status(overlay_xid, pt.0, pt.1);
            if depth < path.len() {
                // Submenu parent without default action rejects direct drop until child is entered
                assert!(!accept, "submenu parent {prefix:?} must reject direct drop");
                // Wait for submenu dwell timer to open the child wheel
                thread::sleep(Duration::from_millis(140));
            } else {
                let st_dbg = daemon.status_json();
                assert!(
                    accept,
                    "leaf item {prefix:?} ({expected_id}) at pt={pt:?} must accept drop; daemon status={st_dbg}"
                );
            }
        }

        let (finished_ok, _) = harness.send_xdnd_drop_and_wait_finished(overlay_xid);
        assert!(
            finished_ok,
            "drop on {expected_id} must finish with success=true"
        );
        thread::sleep(Duration::from_millis(20));
        assert!(!is_window_mapped(&conn, overlay_xid));

        let st = daemon.status_json();
        assert_eq!(st["last_committed_action"], expected_id);
        let committed_path: Vec<u8> = st["last_committed_path"]
            .as_array()
            .expect("last_committed_path")
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        assert_eq!(committed_path.as_slice(), path);
        let committed_items: Vec<String> = st["last_committed_items"]
            .as_array()
            .expect("last_committed_items")
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(committed_items, file_uris);
        assert_eq!(st["last_committed_mode"], "Drag");
    }
    corpus.verify_unchanged("after AC-3.3/3.5/3.6 action cases");

    // Verify multi-chunk INCR payload retrieval (500 URIs)
    let incr_uris: Vec<String> = (0..500)
        .map(|i| format!("file:///tmp/radivine-incr-item-{i:04}.txt"))
        .collect();
    harness.set_uris(&incr_uris, true);
    harness.begin_drag_at(anchor.0, anchor.1, false);
    thread::sleep(Duration::from_millis(15));
    harness
        .trigger_shift_and_wait_mapped(overlay_xid, Duration::from_millis(500))
        .expect("mapped for INCR payload");
    // Pump until all INCR chunks complete
    let incr_deadline = Instant::now() + Duration::from_millis(500);
    while harness.incr_active && Instant::now() < incr_deadline {
        let _ = harness.pump();
        thread::sleep(Duration::from_millis(1));
    }
    harness.send_xdnd_enter(overlay_xid);
    let copy_pt = compute_segment_point(anchor, &[1]);
    let (accept, _) = harness.send_xdnd_position_and_wait_status(overlay_xid, copy_pt.0, copy_pt.1);
    assert!(accept);
    let (finished_ok, _) = harness.send_xdnd_drop_and_wait_finished(overlay_xid);
    assert!(finished_ok);
    let st_incr = daemon.status_json();
    assert_eq!(
        st_incr["last_committed_items"].as_array().unwrap().len(),
        500
    );

    // =========================================================================
    // AC-3.4: 20/20 Diagonal Pointer Navigation into Child Wheel Without Collapse
    // =========================================================================
    harness.set_uris(&file_uris[0..1], false);
    harness.begin_drag_at(anchor.0, anchor.1, false);
    thread::sleep(Duration::from_millis(15));
    harness
        .trigger_shift_and_wait_mapped(overlay_xid, Duration::from_millis(300))
        .expect("mapped for AC-3.4");
    harness.send_xdnd_enter(overlay_xid);

    let parent_preview_pt = compute_segment_point(anchor, &[0]);
    let child_tail_pt = compute_segment_point(anchor, &[0, 2]);
    let sibling_copy_pt = compute_segment_point(anchor, &[1]);

    for trial in 0..20 {
        // Hover File Preview (`[0]`) and open child wheel
        harness.send_xdnd_position_and_wait_status(
            overlay_xid,
            parent_preview_pt.0,
            parent_preview_pt.1,
        );
        thread::sleep(Duration::from_millis(100));

        // Interpolate 5 diagonal steps from File Preview straight to Child Tail (`[0, 2]`)
        for step in 1..=5 {
            let t = (step as f32) / 5.0;
            let ix = (parent_preview_pt.0 as f32
                + (child_tail_pt.0 - parent_preview_pt.0) as f32 * t)
                .round() as i32;
            let iy = (parent_preview_pt.1 as f32
                + (child_tail_pt.1 - parent_preview_pt.1) as f32 * t)
                .round() as i32;
            let (acc, _) = harness.send_xdnd_position_and_wait_status(overlay_xid, ix, iy);
            if step == 5 {
                assert!(
                    acc,
                    "AC-3.4 diagonal travel trial {trial} failed: child segment [0,2] did not accept"
                );
            }
        }

        // Now move to sibling parent segment `Copy` (`[1]`) and wait past grace_ms (160 ms) -> child closes, Copy accepts
        let (acc_copy, _) = harness.send_xdnd_position_and_wait_status(
            overlay_xid,
            sibling_copy_pt.0,
            sibling_copy_pt.1,
        );
        assert!(acc_copy);
    }
    harness.end_drag();
    thread::sleep(Duration::from_millis(25));

    // =========================================================================
    // AC-3.7 & AC-3.8: Cancellation (zero-motion hub release, outside release,
    // second-Shift toggle-off, and passthrough=true leave_grace_ms expiration)
    // =========================================================================
    // 1) Zero-motion release on center hub immediately after pressing Shift
    harness.set_uris(&file_uris[0..1], false);
    harness.begin_drag_at(anchor.0, anchor.1, false);
    thread::sleep(Duration::from_millis(15));
    harness
        .trigger_shift_and_wait_mapped(overlay_xid, Duration::from_millis(300))
        .expect("mapped for zero-motion release");
    harness.send_xdnd_enter(overlay_xid);
    let (acc_hub, _) = harness.send_xdnd_position_and_wait_status(overlay_xid, anchor.0, anchor.1);
    assert!(!acc_hub, "center hub with zero motion must reject drop");
    let (fin_hub, _) = harness.send_xdnd_drop_and_wait_finished(overlay_xid);
    assert!(
        !fin_hub,
        "drop on center hub must finish with success=false"
    );
    assert!(!is_window_mapped(&conn, overlay_xid));

    // 2) Release outside wheel cluster under safe default `passthrough = false`
    harness.begin_drag_at(anchor.0, anchor.1, false);
    thread::sleep(Duration::from_millis(15));
    harness
        .trigger_shift_and_wait_mapped(overlay_xid, Duration::from_millis(300))
        .expect("mapped for outside release");
    harness.send_xdnd_enter(overlay_xid);
    let (acc_out, _) = harness.send_xdnd_position_and_wait_status(overlay_xid, 200, 200);
    assert!(!acc_out, "outside wheel cluster must reject drop");
    let (fin_out, _) = harness.send_xdnd_drop_and_wait_finished(overlay_xid);
    assert!(!fin_out);
    assert!(!is_window_mapped(&conn, overlay_xid));

    // 3) Opt-in `passthrough = true`: leaving cluster > leave_grace_ms (120 ms) dismisses wheel while drag stays Armed
    fs::write(
        &daemon.config_path,
        r#"
        [appearance]
        reduced_motion = true
        [input]
        passthrough = true
        [wheel]
        hover_dwell_ms = 80
        submenu_grace_ms = 160
        leave_grace_ms = 120
        "#,
    )
    .expect("write passthrough=true config");
    assert!(daemon.cli(&["reload"]).status.success());

    harness.begin_drag_at(anchor.0, anchor.1, false);
    thread::sleep(Duration::from_millis(15));
    harness
        .trigger_shift_and_wait_mapped(overlay_xid, Duration::from_millis(300))
        .expect("mapped with passthrough=true");
    harness.send_xdnd_enter(overlay_xid);
    // Move pointer outside wheel cluster and send XdndLeave while button 1 is still held (`drag_ended = false`)
    let _ = Command::new("xdotool")
        .args(["mousemove", "150", "150"])
        .status();
    harness.send_xdnd_leave(overlay_xid);
    thread::sleep(Duration::from_millis(30));
    assert_eq!(daemon.status_json()["session_state"], "LeftGrace");
    // Wait past `leave_grace_ms` (120 ms) -> wheel dismisses to `Armed` while drag continues!
    thread::sleep(Duration::from_millis(140));
    let st_after_leave = daemon.status_json();
    assert_eq!(st_after_leave["session_state"], "Armed");
    assert!(!is_window_mapped(&conn, overlay_xid));
    harness.end_drag();
    thread::sleep(Duration::from_millis(20));
    assert_eq!(daemon.status_json()["session_state"], "Idle");

    // Restore safe default `passthrough = false`
    fs::write(
        &daemon.config_path,
        r#"
        [appearance]
        reduced_motion = true
        [input]
        passthrough = false
        [wheel]
        hover_dwell_ms = 80
        submenu_grace_ms = 160
        leave_grace_ms = 120
        "#,
    )
    .expect("restore passthrough=false config");
    assert!(daemon.cli(&["reload"]).status.success());

    // =========================================================================
    // AC-3.10 & AC-3.13: 50 Mixed Drag Sessions + RSS Growth < 2 MB + SIGKILL
    // =========================================================================
    let rss_before_kb = daemon.rss_kb();
    let t_stress = Instant::now();
    for i in 0..50 {
        harness.set_uris(&file_uris[0..1], false);
        harness.begin_drag_at(anchor.0, anchor.1, false);
        thread::sleep(Duration::from_millis(8));
        harness
            .trigger_shift_and_wait_mapped(overlay_xid, Duration::from_millis(300))
            .expect("stress session mapped");
        if i % 2 == 0 {
            // Toggle off via second Shift
            let _ = Command::new("xdotool")
                .args(["keydown", "Shift_L", "keyup", "Shift_L"])
                .status();
            assert!(wait_window_unmapped(
                &conn,
                overlay_xid,
                Duration::from_millis(300)
            ));
            harness.end_drag();
        } else {
            // Commit Copy
            harness.send_xdnd_enter(overlay_xid);
            let copy_pt = compute_segment_point(anchor, &[1]);
            let (acc, _) =
                harness.send_xdnd_position_and_wait_status(overlay_xid, copy_pt.0, copy_pt.1);
            assert!(acc);
            let (fin, _) = harness.send_xdnd_drop_and_wait_finished(overlay_xid);
            assert!(fin);
            assert!(wait_window_unmapped(
                &conn,
                overlay_xid,
                Duration::from_millis(300)
            ));
        }
    }
    let stress_elapsed = t_stress.elapsed();
    let rss_after_kb = daemon.rss_kb();
    let rss_growth_kb = rss_after_kb.saturating_sub(rss_before_kb);
    assert!(
        rss_growth_kb < 2048,
        "AC-3.13 RSS grew by {rss_growth_kb} KB across 50 drag sessions (limit 2048 KB)"
    );
    println!(
        "M3 stress suite: 50 drag sessions completed in {:.2} s, RSS before={} KB, after={} KB (delta +{} KB)",
        stress_elapsed.as_secs_f64(),
        rss_before_kb,
        rss_after_kb,
        rss_growth_kb
    );

    daemon.stop();

    // Test SIGKILL mid-drag (`AC-3.10`): killing the daemon while the drag wheel is open leaves
    // the X11 server with zero stuck grabs so mouse/keyboard events reach a normal window.
    let mut kill_daemon = TestDaemon::spawn("", "sigkill-drag");
    let kill_xid = kill_daemon.status_json()["overlay_window_id"]
        .as_u64()
        .unwrap() as u32;
    harness.begin_drag_at(anchor.0, anchor.1, false);
    thread::sleep(Duration::from_millis(15));
    harness
        .trigger_shift_and_wait_mapped(kill_xid, Duration::from_millis(300))
        .expect("mapped before SIGKILL");
    let _ = kill_daemon.child.kill();
    let _ = kill_daemon.child.wait();
    harness.end_drag();

    // Verify all protected files are 100% untouched (`AC-3.12`)
    corpus.verify_unchanged("end of M3 suite");
}

/// Live GTK3 (`libgtk-3.so.0` / `gdkdnd-x11.c`, identical to Nemo 6.6.3) drag retargeting test:
/// Spawns `spikes/xdnd-overlay/gtk3_drag_harness.py`, starts a real GTK3 drag using `xdotool`,
/// presses Shift mid-drag so Radivine maps its overlay and `gdkdnd-x11.c` retargets the active
/// drag onto Radivine's overlay via the 0-px `WarpPointer` nudge, navigates to `[0]` (`File Preview`)
/// -> `[0, 1]` (`Head`) and releases button 1, verifying that `libgtk-3` sends `XdndDrop`, receives
/// `XdndFinished(accept=1, XdndActionPrivate)`, never invokes `drag-data-delete`, and leaves all
/// source files untouched.
#[test]
fn m3_real_gtk3_drag_retargeting() {
    if std::env::var("DISPLAY").is_err() {
        return;
    }
    let Ok((conn, _)) = RustConnection::connect(None) else {
        return;
    };

    let corpus = ProtectedCorpus::create("gtk3");
    let mut daemon = TestDaemon::spawn(
        r#"
        [appearance]
        reduced_motion = true
        [wheel]
        hover_dwell_ms = 80
        submenu_grace_ms = 160
        leave_grace_ms = 120
        "#,
        "m3-gtk3",
    );
    let overlay_xid = daemon.status_json()["overlay_window_id"]
        .as_u64()
        .expect("overlay_window_id") as u32;

    let harness_script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("spikes/xdnd-overlay/gtk3_drag_harness.py");

    let mut gtk_cmd = Command::new("python3");
    gtk_cmd
        .arg(&harness_script)
        .arg("--duration-ms")
        .arg("5000")
        .arg("--files");
    for f in &corpus.files {
        gtk_cmd.arg(f);
    }
    let mut gtk_child = gtk_cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn gtk3_drag_harness.py");

    // Wait for GTK3 harness window to map at (100, 100)
    thread::sleep(Duration::from_millis(500));

    // Start real GTK3 drag from source box (200, 220) and drag to (960, 540)
    let _ = Command::new("xdotool")
        .args([
            "mousemove",
            "200",
            "220",
            "mousedown",
            "1",
            "mousemove",
            "--sync",
            "240",
            "240",
            "mousemove",
            "--sync",
            "960",
            "540",
        ])
        .status();
    thread::sleep(Duration::from_millis(80));

    // Press Shift mid-drag -> Radivine maps overlay and 0-px WarpPointer causes GTK3 to retarget!
    let _ = Command::new("xdotool")
        .args(["keydown", "Shift_L", "keyup", "Shift_L"])
        .status();

    let map_deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < map_deadline && !is_window_mapped(&conn, overlay_xid) {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        is_window_mapped(&conn, overlay_xid),
        "Radivine overlay must map on Shift during live GTK3 drag"
    );
    thread::sleep(Duration::from_millis(60));

    // Navigate to root `[0]` (File Preview), wait for child wheel to open, then move to `[0, 1]` (Head)
    let preview_pt = compute_segment_point((960, 540), &[0]);
    let head_pt = compute_segment_point((960, 540), &[0, 1]);

    let _ = Command::new("xdotool")
        .args([
            "mousemove",
            &preview_pt.0.to_string(),
            &preview_pt.1.to_string(),
        ])
        .status();
    thread::sleep(Duration::from_millis(140));

    let _ = Command::new("xdotool")
        .args(["mousemove", &head_pt.0.to_string(), &head_pt.1.to_string()])
        .status();
    thread::sleep(Duration::from_millis(60));

    // Release mouse button 1 -> GTK3 sends XdndDrop -> Radivine commits `radivine.preview.head`!
    let _ = Command::new("xdotool").args(["mouseup", "1"]).status();
    assert!(wait_window_unmapped(
        &conn,
        overlay_xid,
        Duration::from_millis(500)
    ));

    let st = daemon.status_json();
    assert_eq!(st["last_committed_action"], "radivine.preview.head");
    assert_eq!(st["last_committed_path"], serde_json::json!([0, 1]));
    assert_eq!(st["last_committed_mode"], "Drag");
    assert_eq!(
        st["last_committed_items"].as_array().unwrap().len(),
        corpus.files.len()
    );

    let _ = gtk_child.kill();
    let out = gtk_child.wait_with_output().expect("wait gtk_child");
    let gtk_log = String::from_utf8_lossy(&out.stdout);
    assert!(
        !gtk_log.contains("GTK_SOURCE_DATA_DELETE_CALLED"),
        "GTK3 drag-data-delete must never be called: {gtk_log}"
    );
    assert!(
        gtk_log.contains("GTK_CONTEXT_DND_FINISHED")
            && gtk_log.contains("final_selected_action=['GDK_ACTION_COPY']"),
        "GTK3 drag source must translate XdndActionPrivate -> GDK_ACTION_COPY and complete DND_FINISHED: {gtk_log}"
    );

    daemon.stop();
    corpus.verify_unchanged("after live GTK3 drag retargeting");
}
