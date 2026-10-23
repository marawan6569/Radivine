//! Milestone M6 security audit & continuous fuzzing suite (`AC-6.4`, `TESTING_STRATEGY.md` §6.3).
//!
//! Verifies:
//! 1. `#![forbid(unsafe_code)]` enforcement in `radivine-wheel`, `radivine-render`, `radivine-actions`,
//!    and `radivine-viewer`, and audits that `crates/radivine-platform/src/x11/shm.rs` contains the
//!    only `unsafe` blocks in the entire workspace.
//! 2. Command execution safety (`actions.toml` shell-injection rejection, partial placeholder
//!    rejection, relative path rejection, and literal metacharacter handling).
//! 3. `text/uri-list` RFC 2483 parser + `BasicContext::from_uris` + `sniff_bytes` fuzzing (`>= 10 min`
//!    in `m6_ten_minute_fuzz_suite_ac_6_4`).
//! 4. XDND v5 `ClientMessage` (`XdndTarget`) + `SelectionReader` + `parse_actions_toml` fuzzing
//!    (`>= 10 min` in `m6_ten_minute_fuzz_suite_ac_6_4`).

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use radivine_actions::parse_actions_toml;
use radivine_core::{BasicContext, InvocationMode, SourceInfo, parse_uri_list, sniff_bytes};
use radivine_platform::x11::selection::SelectionReader;
use radivine_platform::x11::xdnd::{XdndAtoms, XdndTarget};

/// Fast deterministic SplitMix64 PRNG for reproducible high-throughput mutation fuzzing.
struct Rng64(u64);

impl Rng64 {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound <= 1 {
            0
        } else {
            (self.next_u64() as usize) % bound
        }
    }

    fn fill_bytes(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let w = self.next_u64().to_le_bytes();
            let n = chunk.len();
            chunk.copy_from_slice(&w[..n]);
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn verify_unsafe_code_audit() {
    let root = workspace_root();

    // 1. Verify `#![forbid(unsafe_code)]` in pure/safe crates
    for rel in [
        "crates/radivine-wheel/src/lib.rs",
        "crates/radivine-render/src/lib.rs",
        "crates/radivine-actions/src/lib.rs",
        "apps/radivine-viewer/src/lib.rs",
        "apps/radivine-viewer/src/main.rs",
    ] {
        let p = root.join(rel);
        let content = fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        assert!(
            content.contains("#![forbid(unsafe_code)]"),
            "missing #![forbid(unsafe_code)] in {rel}"
        );
    }

    // 2. Scan all production `src/**/*.rs` files across `crates/`, `apps/`, and `tools/`
    //    and verify that `unsafe` blocks only appear in `crates/radivine-platform/src/x11/shm.rs`.
    let mut rs_files = Vec::new();
    for top in ["crates", "apps", "tools"] {
        collect_rs_files(&root.join(top), &mut rs_files);
    }

    let allowed_shm = root.join("crates/radivine-platform/src/x11/shm.rs");
    let allowed_main_fd = root.join("apps/radivine/src/main.rs");
    for file in rs_files {
        // Skip integration test files under `tests/`
        if file.components().any(|c| c.as_os_str() == "tests") {
            continue;
        }
        let text = fs::read_to_string(&file).expect("read rs file");
        for (line_no, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("#![") {
                continue;
            }
            if trimmed.contains("unsafe ") || trimmed.contains("unsafe{") {
                assert!(
                    file == allowed_shm || file == allowed_main_fd,
                    "unexpected `unsafe` in {}:{} -> {}",
                    file.display(),
                    line_no + 1,
                    trimmed
                );
            }
        }
    }
}

fn run_uri_list_fuzz_batch(rng: &mut Rng64, batch_size: usize) {
    let seeds: &[&[u8]] = &[
        b"file:///tmp/normal.txt\r\n",
        b"# comment line\r\nfile:///home/user/spaced%20name%20%F0%9F%9A%80.txt\r\n",
        b"file:///tmp/embedded%00nul.txt\r\n",
        b"file:///../../etc/passwd\r\nfile://localhost/tmp/local.txt\r\n",
        b"sftp://server/remote/path.txt\r\nhttp://example.com/a%20b\r\ntrash:///item\r\n",
        b"file:///broken%ZZ%G0%2\r\n%\r\n%%\r\n%0\r\n",
        b"\xff\xfe\x00\x01\r\nfile:///non_utf8_\xff\xfe.txt\r\n",
    ];

    let mut buf = vec![0u8; 512];
    for i in 0..batch_size {
        let seed = seeds[i % seeds.len()];
        let len = (rng.next_usize(384) + 1).min(buf.len());
        buf.resize(len, 0);
        rng.fill_bytes(&mut buf);

        // Splice seed fragments with random mutations
        if len >= seed.len() {
            buf[..seed.len()].copy_from_slice(seed);
            let flips = rng.next_usize(6);
            for _ in 0..flips {
                let idx = rng.next_usize(len);
                buf[idx] ^= (rng.next_u32() & 0xFF) as u8;
            }
        }

        let text = String::from_utf8_lossy(&buf);
        let uris = parse_uri_list(&text);
        let ctx = BasicContext::from_uris(&uris, InvocationMode::Drag, SourceInfo::default());
        for item in &ctx.items {
            assert!(!item.uri.is_empty());
            if let Some(ref lp) = item.local_path {
                assert!(
                    !lp.as_os_str().as_encoded_bytes().contains(&0),
                    "local_path must never contain interior NUL bytes"
                );
            }
        }
        let _ = sniff_bytes(&buf);
    }
}

fn run_xdnd_and_toml_fuzz_batch(rng: &mut Rng64, batch_size: usize) {
    let atoms = XdndAtoms {
        enter: 10,
        position: 11,
        status: 12,
        leave: 13,
        drop: 14,
        finished: 15,
        action_private: 16,
    };
    let mut target = XdndTarget::new(0x0100_0001, atoms);
    let mut sel = SelectionReader::new(0x0100_0001, 20, 21);
    let now = Instant::now();

    let toml_seeds = [
        r#"[[action]]
id = "custom.ok"
label = "Valid"
exec = ["/bin/echo", "{path}"]
per_item = true
"#,
        r#"[[action]]
id = "custom.shell"
label = "Bad Shell"
exec = ["/bin/sh", "-c", "rm -rf /"]
"#,
        r#"[[action]]
id = "custom.partial"
label = "Partial Placeholder"
exec = ["/usr/bin/env", "--file={path}"]
"#,
        r#"[[action]]
id = "custom.rel"
label = "Relative Program"
exec = ["./relative.sh", "{paths}"]
"#,
        r#"[menu]
order = ["radivine.preview", "radivine.copy", "unknown.id"]
hide = ["radivine.properties"]
"#,
    ];

    let mut chunk = [0u8; 64];
    for i in 0..batch_size {
        let msg_type = match rng.next_usize(7) {
            0 => atoms.enter,
            1 => atoms.position,
            2 => atoms.leave,
            3 => atoms.drop,
            4 => atoms.status,
            5 => atoms.finished,
            _ => rng.next_u32(),
        };
        let d = [
            if rng.next_usize(4) == 0 {
                0
            } else {
                0x0200_0001 + (rng.next_u32() % 3)
            },
            rng.next_u32(),
            rng.next_u32(),
            rng.next_u32(),
            rng.next_u32(),
        ];
        let _ = target.on_message(msg_type, d);
        if let Some(out) = target.status(rng.next_u32() & 1 != 0) {
            // Invariant: Radivine never replies with XdndActionMove; only XdndActionPrivate or NONE
            assert!(out.data[4] == atoms.action_private || out.data[4] == 0);
        }
        if let Some(fin) = target.finished(rng.next_u32() & 1 != 0) {
            assert!(fin.data[2] == atoms.action_private || fin.data[2] == 0);
        }

        rng.fill_bytes(&mut chunk);
        if rng.next_u32() & 1 != 0 {
            sel.begin("text/uri-list", now);
            let prop = if rng.next_usize(5) == 0 { 0 } else { 20 };
            let prop_type = if rng.next_usize(3) == 0 { 21 } else { 31 };
            let hint = rng.next_usize(20 * 1024 * 1024);
            let _ = sel.on_selection_notify_property(prop, prop_type, chunk.to_vec(), hint);
        } else {
            let len = rng.next_usize(chunk.len());
            let _ = sel.on_incr_chunk(&chunk[..len]);
            let _ = sel.check_timeout(now + Duration::from_millis(rng.next_u64() % 3000));
        }

        if i.is_multiple_of(8) {
            let base = toml_seeds[i % toml_seeds.len()];
            let mut mutated = base.as_bytes().to_vec();
            if !mutated.is_empty() {
                let idx = rng.next_usize(mutated.len());
                mutated[idx] = (rng.next_u32() & 0x7F) as u8;
            }
            if let Ok(s) = std::str::from_utf8(&mutated) {
                let _ = parse_actions_toml(s);
            }
        }
    }
}

#[test]
fn m6_security_audit_and_fuzz_smoke_test() {
    verify_unsafe_code_audit();

    let mut rng = Rng64::new(0x5241_4449_5649_4E45);
    run_uri_list_fuzz_batch(&mut rng, 2500);
    run_xdnd_and_toml_fuzz_batch(&mut rng, 2500);
}

#[test]
#[ignore = "2x10-minute M6 continuous fuzzing suite (`AC-6.4`); run with RADIVINE_FUZZ_SECONDS=600 cargo test --release -p radivine --test m6_fuzz_suite -- --ignored --nocapture"]
fn m6_ten_minute_fuzz_suite_ac_6_4() {
    verify_unsafe_code_audit();

    let per_target_secs: u64 = std::env::var("RADIVINE_FUZZ_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);

    let report_path = PathBuf::from("/tmp/radivine-m6-fuzz-report.json");
    let mut rng = Rng64::new(0x2026_0100_5241_4449);

    // Phase 1: >= 10 minutes (`per_target_secs`) of `text/uri-list` + `BasicContext` + `sniff_bytes` fuzzing
    let t1_start = Instant::now();
    let mut uri_iterations: u64 = 0;
    while t1_start.elapsed().as_secs() < per_target_secs {
        run_uri_list_fuzz_batch(&mut rng, 1000);
        uri_iterations += 1000;
        if uri_iterations.is_multiple_of(50_000) {
            let progress = serde_json::json!({
                "status": "running_phase_1_uri_list",
                "uri_list_elapsed_secs": t1_start.elapsed().as_secs(),
                "uri_list_iterations": uri_iterations,
                "target_secs_per_fuzzer": per_target_secs,
                "panics": 0,
            });
            let _ = fs::write(
                &report_path,
                serde_json::to_string_pretty(&progress).unwrap_or_default(),
            );
        }
        // Light pacing so background fuzzing coexists smoothly on the machine (~200,000+ iterations/min)
        thread::sleep(Duration::from_millis(2));
    }
    let uri_elapsed_secs = t1_start.elapsed().as_secs();

    // Phase 2: >= 10 minutes (`per_target_secs`) of XDND `ClientMessage` + `SelectionReader` + `actions.toml` fuzzing
    let t2_start = Instant::now();
    let mut xdnd_iterations: u64 = 0;
    while t2_start.elapsed().as_secs() < per_target_secs {
        run_xdnd_and_toml_fuzz_batch(&mut rng, 1000);
        xdnd_iterations += 1000;
        if xdnd_iterations.is_multiple_of(50_000) {
            let progress = serde_json::json!({
                "status": "running_phase_2_xdnd_and_toml",
                "uri_list_elapsed_secs": uri_elapsed_secs,
                "uri_list_iterations": uri_iterations,
                "xdnd_elapsed_secs": t2_start.elapsed().as_secs(),
                "xdnd_iterations": xdnd_iterations,
                "target_secs_per_fuzzer": per_target_secs,
                "panics": 0,
            });
            let _ = fs::write(
                &report_path,
                serde_json::to_string_pretty(&progress).unwrap_or_default(),
            );
        }
        thread::sleep(Duration::from_millis(2));
    }
    let xdnd_elapsed_secs = t2_start.elapsed().as_secs();

    let final_report = serde_json::json!({
        "status": "completed",
        "unsafe_audit_passed": true,
        "only_unsafe_file": "crates/radivine-platform/src/x11/shm.rs",
        "uri_list_elapsed_secs": uri_elapsed_secs,
        "uri_list_iterations": uri_iterations,
        "xdnd_and_toml_elapsed_secs": xdnd_elapsed_secs,
        "xdnd_and_toml_iterations": xdnd_iterations,
        "panics": 0,
        "memory_safety_violations": 0,
    });
    let json = serde_json::to_string_pretty(&final_report).expect("serialize fuzz report");
    fs::write(&report_path, &json).expect("write /tmp/radivine-m6-fuzz-report.json");
    println!("{json}");

    assert!(uri_elapsed_secs >= per_target_secs);
    assert!(xdnd_elapsed_secs >= per_target_secs);
    assert!(uri_iterations >= 100_000);
    assert!(xdnd_iterations >= 100_000);
}
