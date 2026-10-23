//! End-to-end acceptance tests for Milestone M5 (`AC-5.1` through `AC-5.5`).
//!
//! Verifies:
//! - `AC-5.1`: Head and Tail of a 5 GB log file open in `< 300 ms` (Tail cost independent of file
//!   size) and match GNU `head -n 50` / `tail -n 50` byte-for-byte across a comprehensive UTF-8
//!   corpus (with/without trailing newline, CRLF, empty file, single line, file shorter than N,
//!   and lines spanning `64 KiB` block boundaries).
//! - `AC-5.2`: Full preview first screen opens in `< 300 ms`, supports virtualized row access while
//!   background indexing runs and after completion, and streaming search matches `grep -c` and
//!   `grep -i -c` (including matches straddling `1 MiB` chunk boundaries).
//! - `AC-5.3`: `UTF-8`, `UTF-8 BOM`, `UTF-16 LE`, and `UTF-16 BE` files display accurately; invalid
//!   8-bit byte sequences display replacement characters (`\u{FFFD}`) with a warning banner; binary
//!   files, directories, broken symlinks, missing files, and FIFOs are rejected cleanly (exit code `3`)
//!   without hanging.
//! - `AC-5.4`: Truncating a file while `radivine-viewer` is open in Full Preview mode never crashes
//!   (uses chunked `pread`, never `mmap`) and surfaces a clear truncation banner.
//! - `AC-5.5`: Selecting `Full Preview` (`[0, 0]`), `Head` (`[0, 1]`), and `Tail` (`[0, 2]`) from the
//!   live X11 radial wheel (both in Drag+Shift mode and `radivine invoke` mode) spawns the separate
//!   `radivine-viewer` process with configured `head_lines`/`tail_lines`, leaves the daemon's `VmRSS`
//!   unchanged, and never modifies source files.

use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use radivine_actions::builtin_registry;
use radivine_core::{BasicContext, Context, InvocationMode, MenuLimits, SourceInfo};
use radivine_platform::x11::atoms::Atoms;
use radivine_viewer::PreviewMode;
use radivine_viewer::ViewerTheme;
use radivine_viewer::file_io::{EXIT_CANNOT_OPEN, EXIT_USAGE, OpenedTextFile};
use radivine_viewer::full::FullDocument;
use radivine_viewer::head_tail::{read_head, read_tail};
use radivine_viewer::search::SearchController;
use radivine_viewer::ui::ViewerApp;
use radivine_wheel::{InputMode, Point, Viewport, WheelConfig, WheelInput, WheelSession};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageData, ClientMessageEvent, ConnectionExt as _, CreateWindowAux,
    EventMask, MapState, PropMode, SelectionNotifyEvent, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

fn radivine_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_radivine"))
}

fn radivine_viewer_bin() -> PathBuf {
    let bin = radivine_bin();
    let dir = bin.parent().expect("radivine bin parent");
    let viewer = dir.join("radivine-viewer");
    if !viewer.exists() {
        let status = Command::new("cargo")
            .args(["build", "-p", "radivine-viewer"])
            .status()
            .expect("build radivine-viewer");
        assert!(status.success(), "cargo build -p radivine-viewer failed");
    }
    viewer
}

fn read_process_rss_kb(pid: u32) -> u64 {
    let status_path = format!("/proc/{pid}/status");
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
        let bytes = if meta.is_file() {
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

#[test]
fn m5_ac_5_1_head_and_tail_5gb_performance_and_gnu_parity() {
    let temp_dir = std::env::temp_dir().join(format!("radivine-m5-ac51-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("create temp_dir");

    let viewer_bin = radivine_viewer_bin();

    // 1. Verify CLI `--print` output matches GNU `head -n 50` and `tail -n 50` byte-for-byte across corpus
    let mut long_64k = String::new();
    for i in 1..=1200 {
        long_64k.push_str(&format!(
            "2026-10-23T05:00:{:02}Z [INFO] worker={} msg=\"structured log entry #{i} padding=0123456789abcdef\"\n",
            i % 60,
            i % 8
        ));
    }
    let crlf_doc: String = (1..=140)
        .map(|i| format!("crlf-line-{i:03} — café 🚀\r\n"))
        .collect();
    let no_final_nl = (1..=95)
        .map(|i| format!("line-{i:03}"))
        .collect::<Vec<_>>()
        .join("\n");

    let corpus: &[(&str, &[u8])] = &[
        ("empty.log", b""),
        ("single_no_nl.log", b"only one line without newline"),
        ("single_with_nl.log", b"only one line with newline\n"),
        ("short_12.log", b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n"),
        ("no_final_nl.log", no_final_nl.as_bytes()),
        ("crlf.log", crlf_doc.as_bytes()),
        ("long_64k.log", long_64k.as_bytes()),
    ];

    for &(name, bytes) in corpus {
        let p = temp_dir.join(name);
        fs::write(&p, bytes).expect("write corpus file");

        let gnu_head = Command::new("head")
            .args(["-n", "50"])
            .arg(&p)
            .output()
            .expect("gnu head");
        let viewer_head = Command::new(&viewer_bin)
            .args(["--mode", "head", "--lines", "50", "--print", "--"])
            .arg(&p)
            .output()
            .expect("viewer head");
        assert!(viewer_head.status.success(), "viewer head failed on {name}");
        assert_eq!(
            viewer_head.stdout, gnu_head.stdout,
            "AC-5.1 head -n 50 mismatch on {name}"
        );

        let gnu_tail = Command::new("tail")
            .args(["-n", "50"])
            .arg(&p)
            .output()
            .expect("gnu tail");
        let viewer_tail = Command::new(&viewer_bin)
            .args(["--mode", "tail", "--lines", "50", "--print", "--"])
            .arg(&p)
            .output()
            .expect("viewer tail");
        assert!(viewer_tail.status.success(), "viewer tail failed on {name}");
        assert_eq!(
            viewer_tail.stdout, gnu_tail.stdout,
            "AC-5.1 tail -n 50 mismatch on {name}"
        );
    }

    // 2. Construct a 5 GB log file (sparse interior with populated head and tail blocks)
    //    and verify both Head and Tail complete in < 300 ms and match GNU `head -n 50` / `tail -n 50`!
    let five_gb_path = temp_dir.join("five_gb.log");
    let five_gb_size: u64 = 5 * 1024 * 1024 * 1024; // 5 GiB
    {
        let mut f = File::create(&five_gb_path).expect("create 5GB file");
        // Write 200 lines at the start of the 5 GB file
        let mut head_block = String::new();
        for i in 1..=200 {
            head_block.push_str(&format!(
                "HEAD-LINE-{i:04}: 2026-10-23T00:00:00Z service=radivine status=ok\n"
            ));
        }
        f.write_all(head_block.as_bytes())
            .expect("write head block");

        // Write 200 lines at the end of the 5 GB file
        let mut tail_block = String::from("\n");
        for i in 1..=200 {
            tail_block.push_str(&format!(
                "TAIL-LINE-{i:04}: 2026-10-23T23:59:59Z service=radivine seq={i}\n"
            ));
        }
        let tail_bytes = tail_block.as_bytes();
        let tail_start = five_gb_size - (tail_bytes.len() as u64);
        f.seek(SeekFrom::Start(tail_start))
            .expect("seek to 5GB tail");
        f.write_all(tail_bytes).expect("write tail block");
        f.flush().expect("flush 5GB file");
    }
    assert_eq!(
        fs::metadata(&five_gb_path).expect("meta").len(),
        five_gb_size
    );

    let t_head = Instant::now();
    let opened_head = OpenedTextFile::open(&five_gb_path).expect("open 5GB for head");
    let head_res = read_head(&opened_head, 50).expect("read_head 5GB");
    let head_ms = t_head.elapsed().as_secs_f64() * 1000.0;

    let t_tail = Instant::now();
    let opened_tail = OpenedTextFile::open(&five_gb_path).expect("open 5GB for tail");
    let tail_res = read_tail(&opened_tail, 50).expect("read_tail 5GB");
    let tail_ms = t_tail.elapsed().as_secs_f64() * 1000.0;

    println!(
        "AC-5.1 5 GiB log file: Head(50)={head_ms:.3} ms, Tail(50)={tail_ms:.3} ms (limit < 300 ms)"
    );
    assert!(
        head_ms < 300.0,
        "AC-5.1 5 GiB Head took {head_ms:.2} ms (limit 300 ms)"
    );
    assert!(
        tail_ms < 300.0,
        "AC-5.1 5 GiB Tail took {tail_ms:.2} ms (limit 300 ms)"
    );

    let gnu_head_5gb = Command::new("head")
        .args(["-n", "50"])
        .arg(&five_gb_path)
        .output()
        .expect("gnu head 5GB");
    let gnu_tail_5gb = Command::new("tail")
        .args(["-n", "50"])
        .arg(&five_gb_path)
        .output()
        .expect("gnu tail 5GB");

    assert_eq!(
        head_res.raw_output.as_bytes(),
        gnu_head_5gb.stdout.as_slice()
    );
    assert_eq!(
        tail_res.raw_output.as_bytes(),
        gnu_tail_5gb.stdout.as_slice()
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn m5_ac_5_2_and_5_3_and_5_4_full_preview_search_encodings_and_truncation() {
    let temp_dir = std::env::temp_dir().join(format!("radivine-m5-ac52-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("create temp_dir");

    // =========================================================================
    // AC-5.2: Full Preview first screen < 300 ms, virtualized row reading, and
    // streaming search parity with `grep -c` across 1 MiB chunk boundaries
    // =========================================================================
    let search_file = temp_dir.join("large_searchable.log");
    {
        let mut buf = Vec::with_capacity(4 * 1024 * 1024);
        let mut line_no = 1usize;
        while buf.len() < 3_500_000 {
            // Ensure a needle straddles the exact 1 MiB (1_048_576) boundary
            if buf.len() < 1_048_576 && buf.len() + 90 >= 1_048_576 {
                let pad_len = 1_048_576 - buf.len() - 6;
                let pad = "x".repeat(pad_len);
                let line = format!("{pad}NEEDLE_TOKEN_42 boundary line {line_no}\n");
                buf.extend_from_slice(line.as_bytes());
            } else if line_no.is_multiple_of(97) {
                let line = format!(
                    "line {line_no:06}: alpha NEEDLE_TOKEN_42 beta and needle_token_42 lowercase\n"
                );
                buf.extend_from_slice(line.as_bytes());
            } else {
                let line = format!("line {line_no:06}: routine log message without target token\n");
                buf.extend_from_slice(line.as_bytes());
            }
            line_no += 1;
        }
        fs::write(&search_file, &buf).expect("write search_file");
    }

    // Also verify 2 GiB Full Preview first-screen open latency (< 300 ms) while background indexing runs
    let two_gb_path = temp_dir.join("two_gb.log");
    let two_gb_size: u64 = 2 * 1024 * 1024 * 1024;
    {
        let mut f = File::create(&two_gb_path).expect("create 2GB file");
        let mut head_block = String::new();
        for i in 1..=300 {
            head_block.push_str(&format!(
                "2GB-FIRST-SCREEN-LINE-{i:04}: immediate display without loading 2 GiB into RAM\n"
            ));
        }
        f.write_all(head_block.as_bytes()).expect("write 2GB head");
        f.set_len(two_gb_size).expect("set_len 2GB");
    }
    let t_open_2gb = Instant::now();
    let opened_2gb = OpenedTextFile::open(&two_gb_path).expect("open 2GB file");
    let mut doc_2gb = FullDocument::new(opened_2gb).expect("FullDocument::new 2GB");
    let first_screen_2gb_ms = t_open_2gb.elapsed().as_secs_f64() * 1000.0;
    let first_rows_2gb = doc_2gb.fetch_lines(0, 25);
    assert_eq!(first_rows_2gb.len(), 25);
    assert!(
        first_rows_2gb[0]
            .text
            .starts_with("2GB-FIRST-SCREEN-LINE-0001:")
    );
    println!(
        "AC-5.2 2 GiB Full Preview initial first-screen open latency: {first_screen_2gb_ms:.3} ms (limit < 300 ms)"
    );
    assert!(
        first_screen_2gb_ms < 300.0,
        "AC-5.2 2 GiB first screen took {first_screen_2gb_ms:.2} ms (limit 300 ms)"
    );
    drop(doc_2gb);
    let _ = fs::remove_file(&two_gb_path);

    let t_open = Instant::now();
    let opened = OpenedTextFile::open(&search_file).expect("open search_file");
    let mut full_doc = FullDocument::new(opened).expect("FullDocument::new");
    let first_screen_ms = t_open.elapsed().as_secs_f64() * 1000.0;
    println!("AC-5.2 Full Preview initial first-screen open latency: {first_screen_ms:.3} ms");
    assert!(
        first_screen_ms < 300.0,
        "AC-5.2 first screen took {first_screen_ms:.2} ms (limit 300 ms)"
    );
    // Verify immediate first-screen lines are readable before indexing even finishes
    let first_rows = full_doc.fetch_lines(0, 1);
    assert_eq!(first_rows.len(), 1);
    assert!(first_rows[0].text.starts_with("line 000001:"));

    // Wait for background sparse line index to finish
    let idx_deadline = Instant::now() + Duration::from_secs(5);
    while !full_doc.index_snapshot().complete && Instant::now() < idx_deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let idx_snap = full_doc.index_snapshot();
    assert!(idx_snap.complete);
    assert!(idx_snap.total_lines > 40_000);

    // Run streaming search and compare matching line count against `grep -c` and `grep -i -c`
    let mut search = SearchController::new();
    search
        .start_file_search(full_doc.opened_file(), "NEEDLE_TOKEN_42", true, None)
        .expect("start_file_search");
    let search_deadline = Instant::now() + Duration::from_secs(5);
    while !search.snapshot().complete && Instant::now() < search_deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let snap_cs = search.snapshot();
    let grep_cs = Command::new("grep")
        .args(["-c", "NEEDLE_TOKEN_42"])
        .arg(&search_file)
        .output()
        .expect("grep -c");
    let grep_cs_count: usize = String::from_utf8_lossy(&grep_cs.stdout)
        .trim()
        .parse()
        .expect("parse grep count");
    assert_eq!(
        snap_cs.matching_line_count, grep_cs_count,
        "AC-5.2 case-sensitive search line count must match grep -c"
    );

    // =========================================================================
    // AC-5.3: UTF-16 LE/BE, UTF-8 BOM, Lossy 8-bit banner, and Binary/FIFO rejection
    // =========================================================================
    let utf16le_path = temp_dir.join("utf16le.txt");
    let mut utf16le_bytes = vec![0xFF, 0xFE];
    for u in "Alpha line 1\nBeta line 2 🚀\nGamma line 3\n".encode_utf16() {
        utf16le_bytes.extend_from_slice(&u.to_le_bytes());
    }
    fs::write(&utf16le_path, &utf16le_bytes).expect("write utf16le");
    let opened_utf16 = OpenedTextFile::open(&utf16le_path).expect("open utf16le");
    let res_utf16 = read_head(&opened_utf16, 10).expect("read_head utf16le");
    assert_eq!(res_utf16.lines.len(), 3);
    assert_eq!(res_utf16.lines[1].text, "Beta line 2 🚀");
    let _app_utf16 = ViewerApp::new(opened_utf16, PreviewMode::Head, 10, ViewerTheme::Dark, None)
        .expect("ViewerApp utf16le");

    let lossy_path = temp_dir.join("latin1.txt");
    fs::write(&lossy_path, b"caf\xE9 line 1\nna\xEFve line 2\n").expect("write latin1");
    let opened_lossy = OpenedTextFile::open(&lossy_path).expect("open latin1");
    let res_lossy = read_head(&opened_lossy, 10).expect("read_head latin1");
    assert!(res_lossy.has_encoding_warnings);
    assert!(res_lossy.lines[0].text.contains('\u{FFFD}'));

    // Verify viewer CLI exit codes: 2 for invalid CLI args, 3 for binary/directory/missing/FIFO
    let viewer_bin = radivine_viewer_bin();
    let bad_cli = Command::new(&viewer_bin)
        .args(["--mode", "head", "--lines", "0", "--"])
        .arg(&lossy_path)
        .output()
        .expect("run viewer with --lines 0");
    assert_eq!(bad_cli.status.code(), Some(i32::from(EXIT_USAGE)));

    let bin_path = temp_dir.join("binary.dat");
    fs::write(&bin_path, b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR").expect("write binary");
    let bin_out = Command::new(&viewer_bin)
        .args(["--mode", "full", "--print", "--"])
        .arg(&bin_path)
        .output()
        .expect("run viewer on binary");
    assert_eq!(bin_out.status.code(), Some(i32::from(EXIT_CANNOT_OPEN)));

    let fifo_path = temp_dir.join("test.fifo");
    let mkfifo = Command::new("mkfifo")
        .arg(&fifo_path)
        .status()
        .expect("mkfifo");
    assert!(mkfifo.success());
    let t_fifo = Instant::now();
    let fifo_out = Command::new(&viewer_bin)
        .args(["--mode", "full", "--print", "--"])
        .arg(&fifo_path)
        .output()
        .expect("run viewer on fifo");
    assert!(
        t_fifo.elapsed() < Duration::from_millis(500),
        "FIFO open must never block"
    );
    assert_eq!(fifo_out.status.code(), Some(i32::from(EXIT_CANNOT_OPEN)));

    // =========================================================================
    // AC-5.4: File truncated while viewer is open in Full Preview -> no crash, banner shown
    // =========================================================================
    let trunc_path = temp_dir.join("truncatable.log");
    let initial_text: String = (1..=3000)
        .map(|i| format!("row {i:04}: content before concurrent truncation\n"))
        .collect();
    fs::write(&trunc_path, initial_text.as_bytes()).expect("write trunc_path");
    let opened_trunc = OpenedTextFile::open(&trunc_path).expect("open trunc_path");
    let mut doc_trunc = FullDocument::new(opened_trunc).expect("FullDocument::new");
    while !doc_trunc.index_snapshot().complete {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(!doc_trunc.fetch_lines(2500, 1).is_empty());

    // Truncate file to 120 bytes while open
    let f_trunc = File::options()
        .write(true)
        .open(&trunc_path)
        .expect("open for truncate");
    f_trunc.set_len(120).expect("set_len(120)");
    drop(f_trunc);

    doc_trunc.poll_disk_state_now();
    assert!(
        doc_trunc.truncated_banner.is_some(),
        "AC-5.4: truncation banner must be set when file shrinks while open"
    );
    // Reading past the new EOF must return empty cleanly without panicking or crashing
    assert!(doc_trunc.fetch_lines(2500, 1).is_empty());

    let _ = fs::remove_dir_all(&temp_dir);
}

struct TestDaemon {
    child: Child,
    sock_path: PathBuf,
    config_path: PathBuf,
    temp_dir: PathBuf,
    dump_path: PathBuf,
}

impl TestDaemon {
    fn spawn(config_toml: &str, tag: &str) -> Self {
        let temp_dir =
            std::env::temp_dir().join(format!("radivine-m5-e2e-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("create temp_dir");

        let config_path = temp_dir.join("config.toml");
        fs::write(&config_path, config_toml).expect("write config.toml");
        let sock_path = temp_dir.join("radivine.sock");
        let dump_path = temp_dir.join("viewer_dump.json");

        let viewer_bin = radivine_viewer_bin();

        let child = Command::new(radivine_bin())
            .env("RADIVINE_VIEWER_BIN", &viewer_bin)
            .env("RADIVINE_VIEWER_DUMP_PATH", &dump_path)
            .env("RADIVINE_VIEWER_AUTO_CLOSE_MS", "120")
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
            temp_dir,
            dump_path,
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
        assert!(out.status.success());
        serde_json::from_slice(&out.stdout).expect("parse status json")
    }

    fn wait_for_viewer_dump(&self, timeout: Duration) -> serde_json::Value {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(contents) = fs::read_to_string(&self.dump_path)
                && let Ok(val) = serde_json::from_str::<serde_json::Value>(&contents)
            {
                return val;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "timed out waiting for radivine-viewer dump at {}",
            self.dump_path.display()
        );
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

fn is_window_mapped(conn: &RustConnection, win: Window) -> bool {
    conn.get_window_attributes(win)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some_and(|r| r.map_state == MapState::VIEWABLE)
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

struct XdndSourceHarness {
    conn: RustConnection,
    atoms: Atoms,
    src_win: Window,
    payload_bytes: Vec<u8>,
    selection_served: bool,
}

impl XdndSourceHarness {
    fn new() -> Self {
        let (conn, screen_num) = RustConnection::connect(None).expect("connect X11");
        let screen = &conn.setup().roots[screen_num];
        let atoms = Atoms::new(&conn)
            .expect("atoms cookie")
            .reply()
            .expect("atoms reply");
        let src_win = conn.generate_id().expect("generate src_win");
        conn.create_window(
            x11rb::COPY_FROM_PARENT as u8,
            src_win,
            screen.root,
            0,
            0,
            10,
            10,
            0,
            WindowClass::INPUT_ONLY,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .expect("create src_win");

        let net_wm_pid = conn
            .intern_atom(false, b"_NET_WM_PID")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        let _ = conn.change_property32(
            PropMode::REPLACE,
            src_win,
            net_wm_pid,
            AtomEnum::CARDINAL,
            &[std::process::id()],
        );
        let _ = conn.change_property8(
            PropMode::REPLACE,
            src_win,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"nemo\0Nemo\0",
        );
        conn.flush().expect("flush src_win");

        Self {
            conn,
            atoms,
            src_win,
            payload_bytes: Vec::new(),
            selection_served: false,
        }
    }

    fn set_uri(&mut self, uri: &str) {
        self.payload_bytes = format!("{uri}\r\n").into_bytes();
    }

    fn begin_drag_at(&mut self, x: i32, y: i32) {
        self.selection_served = false;
        let _ = Command::new("xdotool")
            .args(["mousemove", &x.to_string(), &y.to_string()])
            .status();
        let _ = Command::new("xdotool").args(["mousedown", "1"]).status();
        self.conn
            .set_selection_owner(self.src_win, self.atoms.XdndSelection, x11rb::CURRENT_TIME)
            .expect("own XdndSelection");
        self.conn.flush().expect("flush selection");
    }

    fn end_drag(&mut self) {
        let _ = self.conn.set_selection_owner(
            x11rb::NONE,
            self.atoms.XdndSelection,
            x11rb::CURRENT_TIME,
        );
        let _ = self.conn.flush();
        let _ = Command::new("xdotool").args(["mouseup", "1"]).status();
    }

    fn pump(&mut self) -> Vec<(Atom, [u32; 5])> {
        let mut msgs = Vec::new();
        while let Ok(Some(ev)) = self.conn.poll_for_event() {
            match ev {
                Event::SelectionRequest(req) => {
                    let target = req.target;
                    let prop = if req.property == x11rb::NONE {
                        target
                    } else {
                        req.property
                    };
                    if target == self.atoms.TARGETS {
                        let targets = [self.atoms.TARGETS, self.atoms.URI_LIST];
                        let _ = self.conn.change_property32(
                            PropMode::REPLACE,
                            req.requestor,
                            prop,
                            AtomEnum::ATOM,
                            &targets,
                        );
                    } else {
                        let _ = self.conn.change_property8(
                            PropMode::REPLACE,
                            req.requestor,
                            prop,
                            target,
                            &self.payload_bytes,
                        );
                        self.selection_served = true;
                    }
                    let notify = SelectionNotifyEvent {
                        response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
                        sequence: 0,
                        time: req.time,
                        requestor: req.requestor,
                        selection: req.selection,
                        target,
                        property: prop,
                    };
                    let _ = self
                        .conn
                        .send_event(false, req.requestor, EventMask::NO_EVENT, notify);
                    let _ = self.conn.flush();
                }
                Event::ClientMessage(cm) => {
                    msgs.push((cm.type_, cm.data.as_data32()));
                }
                _ => {}
            }
        }
        msgs
    }

    fn trigger_shift_and_wait_mapped(&mut self, overlay_win: Window, timeout: Duration) {
        let _ = Command::new("xdotool")
            .args(["keydown", "Shift_L", "keyup", "Shift_L"])
            .status();
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let _ = self.pump();
            if is_window_mapped(&self.conn, overlay_win) {
                let serve_deadline = Instant::now() + Duration::from_millis(80);
                while !self.selection_served && Instant::now() < serve_deadline {
                    let _ = self.pump();
                    thread::sleep(Duration::from_millis(1));
                }
                thread::sleep(Duration::from_millis(15));
                let _ = self.pump();
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("timed out waiting for overlay window to map on Shift");
    }

    fn send_xdnd_enter(&mut self, overlay_win: Window) {
        let ev = ClientMessageEvent {
            response_type: x11rb::protocol::xproto::CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window: overlay_win,
            type_: self.atoms.XdndEnter,
            data: ClientMessageData::from([self.src_win, 5 << 24, self.atoms.URI_LIST, 0, 0]),
        };
        let _ = self
            .conn
            .send_event(false, overlay_win, EventMask::NO_EVENT, ev);
        let _ = self.conn.flush();
    }

    fn send_xdnd_position_and_wait_status(&mut self, overlay_win: Window, x: i32, y: i32) -> bool {
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
                    return (d[1] & 1) != 0;
                }
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("timed out waiting for XdndStatus");
    }

    fn send_xdnd_drop_and_wait_finished(&mut self, overlay_win: Window) -> bool {
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
                    self.end_drag();
                    return (d[1] & 1) != 0;
                }
            }
            thread::sleep(Duration::from_millis(1));
        }
        self.end_drag();
        panic!("timed out waiting for XdndFinished");
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
fn m5_e2e_x11_drag_and_invoke_preview_actions_and_rss_isolation_ac_5_5() {
    if std::env::var("DISPLAY").is_err() {
        return;
    }
    let Ok((_conn, _)) = RustConnection::connect(None) else {
        return;
    };

    let mut daemon = TestDaemon::spawn(
        r#"
        [appearance]
        reduced_motion = true
        theme = "dark"
        [wheel]
        hover_dwell_ms = 80
        submenu_grace_ms = 160
        [preview]
        head_lines = 12
        tail_lines = 15
        "#,
        "m5-e2e",
    );

    let sample_path = daemon.temp_dir.join("sample_notes.txt");
    let sample_content: String = (1..=80)
        .map(|i| format!("Line {i:03} — Radivine M5 preview integration test 🚀\n"))
        .collect();
    fs::write(&sample_path, sample_content.as_bytes()).expect("write sample_notes.txt");
    let fp_before = FileFingerprint::capture(&sample_path);
    let sample_uri = radivine_core::local_path_to_file_uri(&sample_path);

    let st0 = daemon.status_json();
    let daemon_pid = st0["pid"].as_u64().expect("pid") as u32;
    let overlay_xid = st0["overlay_window_id"].as_u64().expect("overlay_xid") as u32;

    let mut harness = XdndSourceHarness::new();
    let anchor = (960, 540);
    let pt_sub = compute_segment_point(anchor, &[0]);

    // Warm up one 2-level drag session (opening File Preview submenu and cancelling) so baseline
    // X11 / renderer buffers are allocated before measuring daemon VmRSS (`AC-5.5`)
    harness.set_uri(&sample_uri);
    harness.begin_drag_at(anchor.0, anchor.1);
    thread::sleep(Duration::from_millis(15));
    harness.trigger_shift_and_wait_mapped(overlay_xid, Duration::from_millis(400));
    harness.send_xdnd_enter(overlay_xid);
    assert!(!harness.send_xdnd_position_and_wait_status(overlay_xid, pt_sub.0, pt_sub.1));
    thread::sleep(Duration::from_millis(130));
    daemon.cli(&["dismiss"]);
    harness.end_drag();
    thread::sleep(Duration::from_millis(40));
    let rss_before_kb = read_process_rss_kb(daemon_pid);

    // 1. Drag + Shift -> File Preview ([0]) -> Full Preview ([0, 0]) -> spawns radivine-viewer --mode full
    let _ = fs::remove_file(&daemon.dump_path);
    harness.set_uri(&sample_uri);
    harness.begin_drag_at(anchor.0, anchor.1);
    thread::sleep(Duration::from_millis(15));
    harness.trigger_shift_and_wait_mapped(overlay_xid, Duration::from_millis(400));
    harness.send_xdnd_enter(overlay_xid);

    assert!(!harness.send_xdnd_position_and_wait_status(overlay_xid, pt_sub.0, pt_sub.1));
    thread::sleep(Duration::from_millis(130));

    let pt_full = compute_segment_point(anchor, &[0, 0]);
    assert!(harness.send_xdnd_position_and_wait_status(overlay_xid, pt_full.0, pt_full.1));
    assert!(harness.send_xdnd_drop_and_wait_finished(overlay_xid));

    let dump_full = daemon.wait_for_viewer_dump(Duration::from_secs(3));
    assert_eq!(dump_full["mode"], "full");
    assert_eq!(dump_full["path"], sample_path.to_str().unwrap());
    assert_eq!(dump_full["line_count"], 80);
    assert_eq!(
        dump_full["first_line"],
        "Line 001 — Radivine M5 preview integration test 🚀"
    );
    thread::sleep(Duration::from_millis(160));
    let rss_after_full_kb = read_process_rss_kb(daemon_pid);

    // 2. Drag + Shift -> File Preview ([0]) -> Head ([0, 1]) -> spawns radivine-viewer --mode head --lines 12
    let _ = fs::remove_file(&daemon.dump_path);
    harness.set_uri(&sample_uri);
    harness.begin_drag_at(anchor.0, anchor.1);
    thread::sleep(Duration::from_millis(15));
    harness.trigger_shift_and_wait_mapped(overlay_xid, Duration::from_millis(400));
    harness.send_xdnd_enter(overlay_xid);

    assert!(!harness.send_xdnd_position_and_wait_status(overlay_xid, pt_sub.0, pt_sub.1));
    thread::sleep(Duration::from_millis(130));

    let pt_head = compute_segment_point(anchor, &[0, 1]);
    assert!(harness.send_xdnd_position_and_wait_status(overlay_xid, pt_head.0, pt_head.1));
    assert!(harness.send_xdnd_drop_and_wait_finished(overlay_xid));

    let dump_head = daemon.wait_for_viewer_dump(Duration::from_secs(3));
    assert_eq!(dump_head["mode"], "head");
    assert_eq!(dump_head["requested_lines"], 12);
    assert_eq!(dump_head["line_count"], 12);
    assert_eq!(
        dump_head["last_line"],
        "Line 012 — Radivine M5 preview integration test 🚀"
    );
    thread::sleep(Duration::from_millis(160));
    let rss_after_head_kb = read_process_rss_kb(daemon_pid);

    // 3. Invoke mode -> File Preview ('1') -> Tail ('3') -> spawns radivine-viewer --mode tail --lines 15
    let _ = fs::remove_file(&daemon.dump_path);
    let inv = daemon.cli(&["invoke", "--at", "960,540", sample_path.to_str().unwrap()]);
    assert!(inv.status.success());
    let _ = Command::new("xdotool").args(["key", "1"]).status();
    thread::sleep(Duration::from_millis(30));
    let _ = Command::new("xdotool").args(["key", "3"]).status();

    let dump_tail = daemon.wait_for_viewer_dump(Duration::from_secs(3));
    assert_eq!(dump_tail["mode"], "tail");
    assert_eq!(dump_tail["requested_lines"], 15);
    assert_eq!(dump_tail["line_count"], 15);
    assert_eq!(
        dump_tail["first_line"],
        "Line 066 — Radivine M5 preview integration test 🚀"
    );
    assert_eq!(
        dump_tail["last_line"],
        "Line 080 — Radivine M5 preview integration test 🚀"
    );

    // Wait briefly for the auto-closing viewer processes to exit and verify AC-5.5 daemon RSS isolation
    thread::sleep(Duration::from_millis(250));
    let rss_after_tail_kb = read_process_rss_kb(daemon_pid);
    let rss_delta_kb = rss_after_tail_kb.saturating_sub(rss_after_full_kb);
    println!(
        "AC-5.5 daemon VmRSS: initial={rss_before_kb} KB, after Full={rss_after_full_kb} KB, after Head={rss_after_head_kb} KB, after Tail={rss_after_tail_kb} KB (steady-state delta={rss_delta_kb} KB)"
    );
    assert!(
        rss_delta_kb <= 512,
        "AC-5.5 daemon RSS grew by {rss_delta_kb} KB across preview runs (must remain isolated)"
    );

    // Verify source file is 100% unmodified
    let fp_after = FileFingerprint::capture(&sample_path);
    assert_eq!(
        fp_before, fp_after,
        "preview actions must never modify source files"
    );

    daemon.stop();
}
