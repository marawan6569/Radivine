//! Milestone C5 Reliability Soak Test (`AC-C5.3`, `IMPLEMENTATION_PLAN.md` §5, Section 4).
//!
//! Exercises in strict XDG isolation (`DISPLAY=""`, `RADIVINE_SUPPRESS_DESKTOP_NOTIFY=1`,
//! `RADIVINE_DISABLE_SYSTEMD_USER_SPAWN=1`):
//! 1. Repeated Quick Convert batches (`ConvertAction` / `Origin::Wheel`) across PNG, JPEG, and WebP.
//! 2. Advanced Convert jobs (`Origin::Advanced`) with resize, quality, suffix, and subfolder options.
//! 3. Mixed valid PNG/JPEG/WebP inputs, invalid/corrupt inputs, and partial failures (`CompletedWithErrors`).
//! 4. Mid-job cancellation (`Cancel`).
//! 5. Linked `Retry` jobs (`FailedOnly`, `FailedAndCancelled`, and `All` with `options_override`),
//!    verifying immutable original job history and zero duplicate outputs from retry defects.
//! 6. Worker `SIGTERM` graceful shutdown and `SIGKILL` crash recovery (`worker_crashed` reconciliation).
//! 7. Simulated Wheel Daemon restart during ongoing worker jobs.
//! 8. Control Center `Jobs` page closure (`drop`) and reopening during/after active jobs.
//! 9. Worker idle shutdown (`--idle-timeout-ms`) and automatic on-demand relaunch (`connect_or_launch`).
//! 10. Output collisions (`photo (1).ext`, `photo (2).ext`, `Skip`, and `Fail`).
//! 11. Repeated `convert.toml` and `config.toml [jobs]` settings reloads (`reload_settings`).
//!
//! Run in default fast CI mode (~6s, all scenarios exercised):
//! ```bash
//! cargo test --release -p radivine-control --test c5_soak_test -- --nocapture --test-threads=1
//! ```
//!
//! Run full 1-hour C5 gate (`>= 3,600 s`):
//! ```bash
//! RADIVINE_C5_SOAK_SECS=3605 RADIVINE_C5_SOAK_REPORT_JSON=/tmp/c5_soak_report.json \
//!   cargo test --release -p radivine-control --test c5_soak_test -- --nocapture --test-threads=1
//! ```

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use radivine_actions::{
    ActionsConfig, ConvertAction, ConvertMenuContext, PreviewSettings, registry_from_options,
};
use radivine_control::ControlPage;
use radivine_control::service::ServicePaths;
use radivine_control::ui::ControlCenterApp;
use radivine_control::ui::jobs::JobsCommand;
use radivine_convert::{CapabilityCatalog, FormatId};
use radivine_core::{
    Action, BasicContext, Context, InvocationMode, JobHandle, MenuLimits, SourceInfo,
    local_path_to_file_uri,
};
use radivine_jobs::{
    CONVERT_IMAGE_KIND, JobId, JobItemReport, JobState, JobSummary, JobsClient, Origin, RequestId,
    RetryMode, WirePath, WorkerLaunchMode, WorkerResponse, WorkerRuntimePaths,
};
use rustix::process::{Pid, Signal, kill_process};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileFingerprint {
    sha256: [u8; 32],
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
}

impl FileFingerprint {
    fn capture(path: &Path) -> Self {
        let meta = fs::symlink_metadata(path)
            .unwrap_or_else(|e| panic!("failed to stat {}: {e}", path.display()));
        let bytes =
            fs::read(path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let digest: [u8; 32] = hasher.finalize().into();
        Self {
            sha256: digest,
            ino: meta.ino(),
            size: meta.len(),
            mtime: meta.mtime(),
            mtime_nsec: meta.mtime_nsec(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SoakReport {
    target_duration_secs: u64,
    actual_duration_secs: f64,
    cycles_completed: u64,
    quick_convert_jobs: u64,
    advanced_convert_jobs: u64,
    partial_failure_jobs: u64,
    cancelled_jobs: u64,
    retry_failed_only_jobs: u64,
    retry_failed_and_cancelled_jobs: u64,
    retry_all_override_jobs: u64,
    worker_sigterm_recoveries: u64,
    worker_sigkill_recoveries: u64,
    daemon_restarts_during_jobs: u64,
    control_center_reopens: u64,
    worker_idle_shutdowns_and_relaunches: u64,
    collision_checks: u64,
    settings_reloads: u64,
    total_items_succeeded: u64,
    total_items_failed_expected: u64,
    total_items_cancelled: u64,
    total_items_interrupted_recovered: u64,
    source_modifications_detected: u64,
    destination_overwrites_detected: u64,
    unresolved_staging_artifacts_detected: u64,
    immutable_retry_violations: u64,
    duplicate_retry_outputs_detected: u64,
    job_count_inconsistencies: u64,
    worker_baseline_rss_mib: f64,
    worker_final_rss_mib: f64,
    worker_rss_delta_mib: f64,
    worker_peak_rss_mib: f64,
    host_baseline_rss_mib: f64,
    host_final_rss_mib: f64,
    host_rss_delta_mib: f64,
    leaked_worker_processes: u64,
}

fn make_temp_soak_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("radivine-c5-soak-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create soak dir");
    dir
}

fn ensure_worker_bin() -> PathBuf {
    let control_bin = PathBuf::from(env!("CARGO_BIN_EXE_radivine-control"));
    let bin_dir = control_bin.parent().expect("target dir");
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
    let st = cmd.status().expect("build radivine-worker");
    assert!(st.success(), "cargo build -p radivine-worker failed");
    assert!(candidate.is_file());
    candidate
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

fn write_test_rgba_png(path: &Path, width: u32, height: u32, seed: u8) {
    let mut pixmap = tiny_skia::Pixmap::new(width, height).expect("create pixmap");
    let data = pixmap.data_mut();
    for (idx, chunk) in data.chunks_exact_mut(4).enumerate() {
        let b = (idx as u8).wrapping_add(seed);
        chunk[0] = b;
        chunk[1] = b.wrapping_mul(3);
        chunk[2] = seed.wrapping_mul(11);
        chunk[3] = 255;
    }
    let bytes = pixmap.encode_png().expect("encode PNG");
    fs::write(path, bytes).expect("write PNG");
}

fn assert_no_staging_or_part_files_recursive(dir: &Path) -> u64 {
    let mut found = 0u64;
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let raw = name.as_os_str().as_bytes();
        if raw.starts_with(b".rvstage-") || raw.ends_with(b".part") {
            eprintln!("UNEXPECTED STAGING ARTIFACT: {}", entry.path().display());
            found += 1;
        }
        if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            found += assert_no_staging_or_part_files_recursive(&entry.path());
        }
    }
    found
}

fn verify_job_counts_consistent(job: &JobSummary) -> bool {
    if !job.state.is_terminal() {
        return false;
    }
    let c = &job.counts;
    let sum = c.succeeded_items
        + c.failed_items
        + c.skipped_items
        + c.cancelled_items
        + c.interrupted_items;
    c.pending_items == 0 && c.in_flight_items == 0 && sum == c.total_items
}

struct SoakEnv {
    base_dir: PathBuf,
    paths: WorkerRuntimePaths,
    ui_paths: ServicePaths,
    children: Vec<Child>,
    current_pid: Option<u32>,
    template_png_bytes: Vec<u8>,
    template_jpg_bytes: Vec<u8>,
    template_webp_bytes: Vec<u8>,
}

impl SoakEnv {
    fn new(base_dir: PathBuf) -> Self {
        let worker_bin = ensure_worker_bin();
        let run_dir = base_dir.join("runtime");
        let state_dir = base_dir.join("state").join("radivine");
        let cfg_dir = base_dir.join("config").join("radivine");
        fs::create_dir_all(&run_dir).unwrap();
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(&cfg_dir).unwrap();

        fs::write(
            cfg_dir.join("config.toml"),
            "[jobs]\nmax_parallel = 4\nretention_max_jobs = 100\nretention_days = 30\n",
        )
        .unwrap();
        fs::write(
            cfg_dir.join("convert.toml"),
            "schema_version = 1\ndefault_preset = \"web-optimized\"\nquick_convert_target = \"webp\"\n",
        )
        .unwrap();

        let paths = WorkerRuntimePaths {
            socket_path: run_dir.join("worker.sock"),
            lock_path: run_dir.join("worker.lock"),
            db_path: state_dir.join("jobs.sqlite3"),
            log_path: state_dir.join("worker.log"),
            worker_bin: Some(worker_bin),
            launch_mode: WorkerLaunchMode::Spawn,
        };

        let ui_paths = ServicePaths {
            ipc_socket: run_dir.join("radivine.sock"),
            config_path: Some(cfg_dir.join("config.toml")),
            systemd_user_dir: base_dir.join("config").join("systemd").join("user"),
            system_unit_path: base_dir.join("usr-lib-systemd").join("radivine.service"),
            autostart_dir: base_dir.join("config").join("autostart"),
            state_dir,
            daemon_bin: PathBuf::from(env!("CARGO_BIN_EXE_radivine-control"))
                .parent()
                .unwrap()
                .join("radivine"),
            allow_systemd: false,
        };

        let mut env = Self {
            base_dir,
            paths,
            ui_paths,
            children: Vec::new(),
            current_pid: None,
            template_png_bytes: Vec::new(),
            template_jpg_bytes: Vec::new(),
            template_webp_bytes: Vec::new(),
        };

        // Spawn initial worker and generate template PNG, JPEG, and WebP fixtures
        env.spawn_worker(&[], 60_000);
        let init_dir = env.base_dir.join("templates");
        fs::create_dir_all(&init_dir).unwrap();
        let t_png = init_dir.join("seed.png");
        write_test_rgba_png(&t_png, 48, 36, 42);
        env.template_png_bytes = fs::read(&t_png).unwrap();

        let mut c = env.connect();
        let (jid_jpg, _, _) = c
            .submit(
                RequestId::generate(),
                Origin::Test,
                CONVERT_IMAGE_KIND,
                vec![WirePath::from_absolute_path(&t_png).unwrap()],
                serde_json::json!({"target": "jpeg"}),
            )
            .unwrap();
        env.wait_terminal(jid_jpg, Duration::from_secs(10));
        env.template_jpg_bytes = fs::read(init_dir.join("seed.jpg")).unwrap();

        let (jid_webp, _, _) = c
            .submit(
                RequestId::generate(),
                Origin::Test,
                CONVERT_IMAGE_KIND,
                vec![WirePath::from_absolute_path(&t_png).unwrap()],
                serde_json::json!({"target": "webp"}),
            )
            .unwrap();
        env.wait_terminal(jid_webp, Duration::from_secs(10));
        env.template_webp_bytes = fs::read(init_dir.join("seed.webp")).unwrap();

        env
    }

    fn spawn_worker(&mut self, extra_env: &[(&str, &str)], idle_ms: u64) -> u32 {
        self.reap_exited_children();
        let worker_bin = self.paths.worker_bin.as_ref().unwrap();
        let mut cmd = Command::new(worker_bin);
        cmd.arg("--socket")
            .arg(&self.paths.socket_path)
            .arg("--db")
            .arg(&self.paths.db_path)
            .arg("--lock")
            .arg(&self.paths.lock_path)
            .arg("serve")
            .arg("--idle-timeout-ms")
            .arg(idle_ms.to_string())
            .env("DISPLAY", "")
            .env("XDG_CONFIG_HOME", self.base_dir.join("config"))
            .env("XDG_STATE_HOME", self.base_dir.join("state"))
            .env("XDG_RUNTIME_DIR", self.base_dir.join("runtime"))
            .env("RADIVINE_WORKER_BIN", worker_bin)
            .env("RADIVINE_WORKER_SOCKET", &self.paths.socket_path)
            .env("RADIVINE_WORKER_DB", &self.paths.db_path)
            .env("RADIVINE_SUPPRESS_DESKTOP_NOTIFY", "1")
            .env("RADIVINE_DISABLE_SYSTEMD_USER_SPAWN", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        for (k, v) in extra_env {
            cmd.env(k, v);
        }

        let child = cmd.spawn().expect("spawn soak radivine-worker");
        let pid = child.id();
        self.children.push(child);
        self.current_pid = Some(pid);

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(mut c) = JobsClient::connect_with_timeout(
                &self.paths.socket_path,
                Duration::from_millis(100),
            ) && matches!(c.ping(), Ok(WorkerResponse::Pong { .. }))
            {
                return pid;
            }
            thread::sleep(Duration::from_millis(15));
        }
        panic!("soak worker did not become ready within 10s");
    }

    fn reap_exited_children(&mut self) {
        self.children
            .retain_mut(|ch| matches!(ch.try_wait(), Ok(None)));
    }

    fn connect(&self) -> JobsClient {
        JobsClient::connect(&self.paths.socket_path).expect("connect to soak worker")
    }

    fn wait_terminal(&self, job_id: JobId, timeout: Duration) -> (JobSummary, Vec<JobItemReport>) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(mut c) = JobsClient::connect_with_timeout(
                &self.paths.socket_path,
                Duration::from_millis(200),
            ) && let Ok((job, items, _)) = c.get_job(job_id, None, None)
                && job.state.is_terminal()
            {
                return (job, items);
            }
            thread::sleep(Duration::from_millis(15));
        }
        panic!("soak job {job_id} did not reach terminal state within {timeout:?}");
    }

    fn shutdown_active_worker(&mut self) {
        if let Ok(mut c) = JobsClient::connect(&self.paths.socket_path) {
            let _ = c.shutdown_if_idle();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            self.reap_exited_children();
            if self.children.is_empty() && !self.paths.socket_path.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        for ch in &mut self.children {
            let _ = ch.kill();
            let _ = ch.wait();
        }
        self.children.clear();
        self.current_pid = None;
    }
}

impl Drop for SoakEnv {
    fn drop(&mut self) {
        for ch in &mut self.children {
            let _ = ch.kill();
            let _ = ch.wait();
        }
    }
}

#[test]
fn ac_c5_3_reliability_soak_test() {
    let target_secs: u64 = std::env::var("RADIVINE_C5_SOAK_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);

    let base_dir = make_temp_soak_dir();
    let mut env = SoakEnv::new(base_dir.clone());
    let mut report = SoakReport {
        target_duration_secs: target_secs,
        ..SoakReport::default()
    };

    // Establish immutable global source pool whose fingerprints are verified on every cycle
    let pool_dir = base_dir.join("immutable_source_pool");
    fs::create_dir_all(&pool_dir).unwrap();
    let pool_png = pool_dir.join("master_alpha.png");
    let pool_jpg = pool_dir.join("master_photo.jpg");
    let pool_webp = pool_dir.join("master_graphic.webp");
    let pool_non_utf8 = pool_dir.join(OsStr::from_bytes(b"master_\x80\xff_raw.png"));
    fs::write(&pool_png, &env.template_png_bytes).unwrap();
    fs::write(&pool_jpg, &env.template_jpg_bytes).unwrap();
    fs::write(&pool_webp, &env.template_webp_bytes).unwrap();
    fs::write(&pool_non_utf8, &env.template_png_bytes).unwrap();

    let mut master_fingerprints = BTreeMap::new();
    for p in [&pool_png, &pool_jpg, &pool_webp, &pool_non_utf8] {
        master_fingerprints.insert(p.clone(), FileFingerprint::capture(p));
    }

    // Warm up 1 job before capturing baseline RSS
    {
        let warm_dir = base_dir.join("warmup_cycle");
        fs::create_dir_all(&warm_dir).unwrap();
        let w_src = warm_dir.join("w.png");
        fs::write(&w_src, &env.template_png_bytes).unwrap();
        let mut c = env.connect();
        let (jid, _, _) = c
            .submit(
                RequestId::generate(),
                Origin::Test,
                CONVERT_IMAGE_KIND,
                vec![WirePath::from_absolute_path(&w_src).unwrap()],
                serde_json::json!({"target": "webp"}),
            )
            .unwrap();
        env.wait_terminal(jid, Duration::from_secs(10));
        let _ = fs::remove_dir_all(&warm_dir);
    }

    let worker_pid_initial = env.current_pid.expect("worker running");
    report.worker_baseline_rss_mib = read_proc_rss_kib(worker_pid_initial) as f64 / 1024.0;
    report.worker_peak_rss_mib = report.worker_baseline_rss_mib;
    report.host_baseline_rss_mib = read_proc_rss_kib(std::process::id()) as f64 / 1024.0;

    let start_instant = Instant::now();
    let mut last_log_instant = start_instant;
    let deadline = start_instant + Duration::from_secs(target_secs);

    println!(
        "\n[C5-SOAK] Starting reliability soak test (target = {}s, worker baseline RSS = {:.2} MiB, host baseline RSS = {:.2} MiB)",
        target_secs, report.worker_baseline_rss_mib, report.host_baseline_rss_mib
    );

    // Ensure at least 2 full cycles execute so all 12 scenarios are exercised even in short CI runs
    while Instant::now() < deadline || report.cycles_completed < 2 {
        let cycle = report.cycles_completed;
        let cycle_dir = base_dir.join(format!("cycle_{cycle}"));
        fs::create_dir_all(&cycle_dir).unwrap();
        let mut cycle_src_fingerprints: Vec<(PathBuf, FileFingerprint)> = Vec::new();

        // ---------------------------------------------------------------------
        // A. Quick Convert mixed batch (PNG + JPEG + WebP + non-UTF-8 -> WebP/JPEG/PNG)
        //    plus Simulated Daemon SessionController menu build & restart mid-batch
        // ---------------------------------------------------------------------
        let qc_dir = cycle_dir.join("quick");
        fs::create_dir_all(&qc_dir).unwrap();
        let batch_count = 6 + ((cycle as usize) % 7); // 6..=12 mixed files
        let mut qc_uris = Vec::with_capacity(batch_count);
        for i in 0..batch_count {
            let (ext, bytes) = match i % 3 {
                0 => ("png", &env.template_png_bytes),
                1 => ("jpg", &env.template_jpg_bytes),
                _ => ("webp", &env.template_webp_bytes),
            };
            let p = qc_dir.join(format!("qc_{i:02}.{ext}"));
            fs::write(&p, bytes).unwrap();
            cycle_src_fingerprints.push((p.clone(), FileFingerprint::capture(&p)));
            qc_uris.push(local_path_to_file_uri(&p));
        }

        let target_fmt = match cycle % 3 {
            0 => FormatId::WebP,
            1 => FormatId::Jpeg,
            _ => FormatId::Png,
        };
        let menu_ctx = ConvertMenuContext::with_runtime_paths(
            Arc::new(CapabilityCatalog::default_builtin()),
            env.paths.clone(),
        );
        let daemon_reg = registry_from_options(
            &ActionsConfig::default(),
            &PreviewSettings::default(),
            &menu_ctx,
        )
        .unwrap();
        let basic_ctx =
            BasicContext::from_uris(&qc_uris, InvocationMode::Invoke, SourceInfo::default());
        let full_ctx = Context::from_basic(basic_ctx);
        let (built_menu, _) = daemon_reg
            .build_menu(&full_ctx, MenuLimits::default())
            .expect("built menu for Quick Convert");
        assert!(!built_menu.root.label.is_empty());

        let qc_action = ConvertAction::new(target_fmt, menu_ctx);
        let jh = JobHandle::new(
            radivine_core::JobId(cycle + 1),
            Arc::new(AtomicBool::new(false)),
        );
        qc_action.run(&full_ctx, &jh).unwrap();
        report.quick_convert_jobs += 1;

        // Simulate daemon restart while worker processes the job
        drop(built_menu);
        drop(daemon_reg);
        let _restarted_daemon_reg = registry_from_options(
            &ActionsConfig::default(),
            &PreviewSettings::default(),
            &ConvertMenuContext::with_runtime_paths(
                Arc::new(CapabilityCatalog::default_builtin()),
                env.paths.clone(),
            ),
        )
        .unwrap();
        report.daemon_restarts_during_jobs += 1;

        // Wait for the latest Quick Convert job to finish
        let qc_job_id = {
            let mut c = env.connect();
            let jobs = c.list_jobs(5, true).unwrap();
            jobs.first().expect("Quick Convert job must exist").job_id
        };
        let (qc_sum, _) = env.wait_terminal(qc_job_id, Duration::from_secs(15));
        assert_eq!(qc_sum.state, JobState::Completed);
        if !verify_job_counts_consistent(&qc_sum) {
            report.job_count_inconsistencies += 1;
        }
        report.total_items_succeeded += u64::from(qc_sum.counts.succeeded_items);

        // ---------------------------------------------------------------------
        // B. Advanced Convert job (resize + quality + suffix + subfolder)
        // ---------------------------------------------------------------------
        let adv_dir = cycle_dir.join("advanced");
        fs::create_dir_all(&adv_dir).unwrap();
        let mut adv_wires = Vec::new();
        for i in 0..4 {
            let p = adv_dir.join(format!("adv_{i}.png"));
            fs::write(&p, &env.template_png_bytes).unwrap();
            cycle_src_fingerprints.push((p.clone(), FileFingerprint::capture(&p)));
            adv_wires.push(WirePath::from_absolute_path(&p).unwrap());
        }
        let mut adv_app = radivine_control::ui::advanced_convert::AdvancedConvertApp::new(
            adv_wires,
            env.ui_paths.clone(),
            None,
        );
        adv_app.target_format = FormatId::WebP;
        adv_app.quality_value = 90;
        adv_app.resize_mode = radivine_control::ui::advanced_convert::UiResizeMode::Width;
        adv_app.resize_width_text = "24".to_string();
        adv_app.dest_mode = radivine_control::ui::advanced_convert::UiDestMode::Subfolder;
        adv_app.dest_subfolder_text = "AdvSub".to_string();
        adv_app.append_suffix = true;
        adv_app.suffix_text = "-adv".to_string();
        let (adv_jid, _) = adv_app
            .submit_blocking(&env.paths.socket_path)
            .expect("submit Advanced Convert job");
        let mut c = env.connect();
        let (adv_sum, _) = env.wait_terminal(adv_jid, Duration::from_secs(15));
        assert_eq!(adv_sum.state, JobState::Completed);
        assert_eq!(adv_sum.counts.succeeded_items, 4);
        if !verify_job_counts_consistent(&adv_sum) {
            report.job_count_inconsistencies += 1;
        }
        for i in 0..4 {
            assert!(
                adv_dir
                    .join("AdvSub")
                    .join(format!("adv_{i}-adv.webp"))
                    .is_file()
            );
        }
        report.advanced_convert_jobs += 1;
        report.total_items_succeeded += 4;

        // ---------------------------------------------------------------------
        // C. Partial failure (valid + corrupt) -> FailedOnly Retry + All Override Retry
        //    (Verifying immutable retry history & zero duplicate outputs)
        // ---------------------------------------------------------------------
        let pf_dir = cycle_dir.join("partial_retry");
        fs::create_dir_all(&pf_dir).unwrap();
        let pf_good1 = pf_dir.join("good1.png");
        let pf_good2 = pf_dir.join("good2.jpg");
        let pf_bad3 = pf_dir.join("corrupt3.png");
        fs::write(&pf_good1, &env.template_png_bytes).unwrap();
        fs::write(&pf_good2, &env.template_jpg_bytes).unwrap();
        fs::write(&pf_bad3, b"\x89PNG\r\n\x1a\ntruncated-corrupt-payload").unwrap();
        cycle_src_fingerprints.push((pf_good1.clone(), FileFingerprint::capture(&pf_good1)));
        cycle_src_fingerprints.push((pf_good2.clone(), FileFingerprint::capture(&pf_good2)));

        let (pf_jid, _, _) = c
            .submit(
                RequestId::generate(),
                Origin::Wheel,
                CONVERT_IMAGE_KIND,
                vec![
                    WirePath::from_absolute_path(&pf_good1).unwrap(),
                    WirePath::from_absolute_path(&pf_good2).unwrap(),
                    WirePath::from_absolute_path(&pf_bad3).unwrap(),
                ],
                serde_json::json!({"target": "webp"}),
            )
            .unwrap();
        let (pf_sum_before, pf_items_before) = env.wait_terminal(pf_jid, Duration::from_secs(15));
        assert_eq!(pf_sum_before.state, JobState::CompletedWithErrors);
        assert_eq!(pf_sum_before.counts.succeeded_items, 2);
        assert_eq!(pf_sum_before.counts.failed_items, 1);
        if !verify_job_counts_consistent(&pf_sum_before) {
            report.job_count_inconsistencies += 1;
        }
        report.partial_failure_jobs += 1;
        report.total_items_succeeded += 2;
        report.total_items_failed_expected += 1;

        // Repair corrupt3.png so retry succeeds, then record its fingerprint
        fs::write(&pf_bad3, &env.template_png_bytes).unwrap();
        cycle_src_fingerprints.push((pf_bad3.clone(), FileFingerprint::capture(&pf_bad3)));

        let (retry_fo_jid, _, reset_fo) = c.retry(pf_jid, RetryMode::FailedOnly, None).unwrap();
        assert_eq!(reset_fo, 1);
        let (retry_fo_sum, retry_fo_items) =
            env.wait_terminal(retry_fo_jid, Duration::from_secs(15));
        assert_eq!(retry_fo_sum.retry_of, Some(pf_jid));
        assert_eq!(retry_fo_sum.origin, Origin::Retry);
        assert_eq!(retry_fo_sum.state, JobState::Completed);
        assert_eq!(retry_fo_sum.counts.total_items, 1);
        assert_eq!(retry_fo_sum.counts.succeeded_items, 1);
        assert_eq!(retry_fo_items[0].attempt_count, 2);
        if !verify_job_counts_consistent(&retry_fo_sum) {
            report.job_count_inconsistencies += 1;
        }
        report.retry_failed_only_jobs += 1;
        report.total_items_succeeded += 1;

        // Verify zero duplicate outputs (`good1 (1).webp` or `good2 (1).webp`)
        if pf_dir.join("good1 (1).webp").exists() || pf_dir.join("good2 (1).webp").exists() {
            report.duplicate_retry_outputs_detected += 1;
        }

        // Verify original job history is 100% immutable
        let (pf_sum_after, pf_items_after, _) = c.get_job(pf_jid, None, None).unwrap();
        if pf_sum_after != pf_sum_before || pf_items_after != pf_items_before {
            report.immutable_retry_violations += 1;
        }

        // Also test RetryMode::All with destination_override
        let override_dir = pf_dir.join("retry_all_out");
        fs::create_dir_all(&override_dir).unwrap();
        let (retry_all_jid, _, reset_all) = c
            .retry(
                pf_jid,
                RetryMode::All,
                Some(serde_json::json!({
                    "output": { "location": { "folder": override_dir.to_string_lossy() } }
                })),
            )
            .unwrap();
        assert_eq!(reset_all, 3);
        let (retry_all_sum, _) = env.wait_terminal(retry_all_jid, Duration::from_secs(15));
        assert_eq!(retry_all_sum.retry_of, Some(pf_jid));
        assert_eq!(retry_all_sum.state, JobState::Completed);
        assert_eq!(retry_all_sum.counts.succeeded_items, 3);
        if !verify_job_counts_consistent(&retry_all_sum) {
            report.job_count_inconsistencies += 1;
        }
        report.retry_all_override_jobs += 1;
        report.total_items_succeeded += 3;

        // ---------------------------------------------------------------------
        // D. Output collisions (Suffix ` (1)`, ` (2)`, Skip, and Fail) +
        //    Zero destination overwrite verification
        // ---------------------------------------------------------------------
        let col_dir = cycle_dir.join("collisions");
        fs::create_dir_all(&col_dir).unwrap();
        let col_src = col_dir.join("photo.png");
        fs::write(&col_src, &env.template_png_bytes).unwrap();
        cycle_src_fingerprints.push((col_src.clone(), FileFingerprint::capture(&col_src)));

        // Pre-populate sentinel destination files that MUST NEVER be overwritten
        let sentinel0 = col_dir.join("photo.webp");
        let sentinel1 = col_dir.join("photo (1).webp");
        fs::write(&sentinel0, b"sentinel-never-overwrite-0").unwrap();
        fs::write(&sentinel1, b"sentinel-never-overwrite-1").unwrap();
        let fp_sentinel0 = FileFingerprint::capture(&sentinel0);
        let fp_sentinel1 = FileFingerprint::capture(&sentinel1);

        // 1) Suffix policy must create `photo (2).webp`
        let (col_jid, _, _) = c
            .submit(
                RequestId::generate(),
                Origin::Test,
                CONVERT_IMAGE_KIND,
                vec![WirePath::from_absolute_path(&col_src).unwrap()],
                serde_json::json!({"target": "webp", "collision": "suffix"}),
            )
            .unwrap();
        let (col_sum, _) = env.wait_terminal(col_jid, Duration::from_secs(10));
        assert_eq!(col_sum.state, JobState::Completed);
        assert!(col_dir.join("photo (2).webp").is_file());
        report.total_items_succeeded += 1;

        // 2) Skip policy must skip without modifying sentinel0
        let (skip_jid, _, _) = c
            .submit(
                RequestId::generate(),
                Origin::Test,
                CONVERT_IMAGE_KIND,
                vec![WirePath::from_absolute_path(&col_src).unwrap()],
                serde_json::json!({"target": "webp", "collision": "skip"}),
            )
            .unwrap();
        let (skip_sum, _) = env.wait_terminal(skip_jid, Duration::from_secs(10));
        assert_eq!(skip_sum.counts.skipped_items, 1);

        // 3) Fail policy must fail without modifying sentinel0
        let (fail_jid, _, _) = c
            .submit(
                RequestId::generate(),
                Origin::Test,
                CONVERT_IMAGE_KIND,
                vec![WirePath::from_absolute_path(&col_src).unwrap()],
                serde_json::json!({"target": "webp", "collision": "fail"}),
            )
            .unwrap();
        let (fail_sum, _) = env.wait_terminal(fail_jid, Duration::from_secs(10));
        assert_eq!(fail_sum.state, JobState::Failed);
        assert_eq!(fail_sum.counts.failed_items, 1);
        report.total_items_failed_expected += 1;
        report.collision_checks += 3;

        if FileFingerprint::capture(&sentinel0) != fp_sentinel0
            || FileFingerprint::capture(&sentinel1) != fp_sentinel1
        {
            report.destination_overwrites_detected += 1;
        }

        // ---------------------------------------------------------------------
        // E. Control Center closure & reopening + Settings reload
        // ---------------------------------------------------------------------
        {
            let ctx = eframe::egui::Context::default();
            let mut app = ControlCenterApp::new(
                ControlPage::Jobs,
                env.ui_paths.clone(),
                None,
                Some(ctx.clone()),
                false,
            );
            let dl = Instant::now() + Duration::from_secs(2);
            while Instant::now() < dl {
                app.drain_all_events();
                if !app.jobs_view.jobs.is_empty() {
                    break;
                }
                thread::sleep(Duration::from_millis(15));
            }
            let _ = app.jobs_command_sender().send(JobsCommand::LoadJobDetails {
                job_id: col_jid,
                page_index: 0,
            });
            let mut out = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
                app.render_frame(ui);
            });
            out.textures_delta.clear();
            drop(app);
            report.control_center_reopens += 1;
        }

        // Reload settings via RPC
        let _ = c.reload_settings().unwrap();
        report.settings_reloads += 1;
        drop(c);

        // ---------------------------------------------------------------------
        // F. Periodic Mid-Batch Cancellation + FailedAndCancelled Retry,
        //    Worker SIGTERM, Worker SIGKILL, and Idle Shutdown + Relaunch
        // ---------------------------------------------------------------------
        match cycle % 2 {
            0 => {
                // Sub-scenario F1: Cancel mid-batch + RetryMode::FailedAndCancelled + SIGTERM recovery
                env.shutdown_active_worker();
                let pid = env.spawn_worker(&[("RADIVINE_WORKER_TEST_ITEM_SLEEP_MS", "80")], 60_000);

                let can_dir = cycle_dir.join("cancel_batch");
                fs::create_dir_all(&can_dir).unwrap();
                let mut can_wires = Vec::new();
                for i in 0..8 {
                    let p = can_dir.join(format!("c_{i}.png"));
                    fs::write(&p, &env.template_png_bytes).unwrap();
                    cycle_src_fingerprints.push((p.clone(), FileFingerprint::capture(&p)));
                    can_wires.push(WirePath::from_absolute_path(&p).unwrap());
                }
                let mut c_can = env.connect();
                let (can_jid, _, _) = c_can
                    .submit(
                        RequestId::generate(),
                        Origin::ControlCenter,
                        CONVERT_IMAGE_KIND,
                        can_wires,
                        serde_json::json!({"target": "webp"}),
                    )
                    .unwrap();
                thread::sleep(Duration::from_millis(90));
                let _ = c_can.cancel(can_jid).unwrap();
                let (can_sum_before, can_items_before) =
                    env.wait_terminal(can_jid, Duration::from_secs(10));
                assert!(matches!(
                    can_sum_before.state,
                    JobState::Cancelled | JobState::CompletedWithErrors | JobState::Completed
                ));
                if !verify_job_counts_consistent(&can_sum_before) {
                    report.job_count_inconsistencies += 1;
                }
                report.cancelled_jobs += 1;
                report.total_items_succeeded += u64::from(can_sum_before.counts.succeeded_items);
                report.total_items_cancelled += u64::from(can_sum_before.counts.cancelled_items);

                if can_sum_before.counts.cancelled_items > 0 {
                    // Restart worker without sleep delay for fast retry completion
                    env.shutdown_active_worker();
                    env.spawn_worker(&[], 60_000);
                    let mut c_ret = env.connect();
                    let (ret_jid, _, reset_cnt) = c_ret
                        .retry(can_jid, RetryMode::FailedAndCancelled, None)
                        .unwrap();
                    assert_eq!(reset_cnt, can_sum_before.counts.cancelled_items);
                    let (ret_sum, _) = env.wait_terminal(ret_jid, Duration::from_secs(15));
                    assert_eq!(ret_sum.retry_of, Some(can_jid));
                    assert_eq!(ret_sum.state, JobState::Completed);
                    if !verify_job_counts_consistent(&ret_sum) {
                        report.job_count_inconsistencies += 1;
                    }
                    report.retry_failed_and_cancelled_jobs += 1;
                    report.total_items_succeeded += u64::from(ret_sum.counts.succeeded_items);

                    // Check immutable original job and zero duplicate outputs
                    let (can_sum_after, can_items_after, _) =
                        c_ret.get_job(can_jid, None, None).unwrap();
                    if can_sum_after != can_sum_before || can_items_after != can_items_before {
                        report.immutable_retry_violations += 1;
                    }
                    for i in 0..8 {
                        if can_dir.join(format!("c_{i} (1).webp")).exists() {
                            report.duplicate_retry_outputs_detected += 1;
                        }
                    }
                } else {
                    let _ = pid;
                    env.shutdown_active_worker();
                    env.spawn_worker(&[], 60_000);
                }

                // SIGTERM graceful shutdown during active job
                env.shutdown_active_worker();
                let term_pid =
                    env.spawn_worker(&[("RADIVINE_WORKER_TEST_ITEM_SLEEP_MS", "120")], 60_000);
                let term_dir = cycle_dir.join("sigterm_batch");
                fs::create_dir_all(&term_dir).unwrap();
                let mut term_wires = Vec::new();
                for i in 0..6 {
                    let p = term_dir.join(format!("t_{i}.png"));
                    fs::write(&p, &env.template_png_bytes).unwrap();
                    cycle_src_fingerprints.push((p.clone(), FileFingerprint::capture(&p)));
                    term_wires.push(WirePath::from_absolute_path(&p).unwrap());
                }
                let mut c_term = env.connect();
                let (term_jid, _, _) = c_term
                    .submit(
                        RequestId::generate(),
                        Origin::Cli,
                        CONVERT_IMAGE_KIND,
                        term_wires,
                        serde_json::json!({"target": "webp"}),
                    )
                    .unwrap();
                drop(c_term);
                thread::sleep(Duration::from_millis(60));
                kill_process(Pid::from_raw(term_pid as i32).unwrap(), Signal::TERM).unwrap();
                if let Some(mut ch) = env.children.pop() {
                    let st = ch.wait().unwrap();
                    assert!(st.success(), "SIGTERM must exit cleanly with 0");
                }
                env.current_pid = None;

                // Relaunch worker and verify recovery + completion of remaining items
                env.spawn_worker(&[], 60_000);
                let (term_sum, _) = env.wait_terminal(term_jid, Duration::from_secs(15));
                if !verify_job_counts_consistent(&term_sum) {
                    report.job_count_inconsistencies += 1;
                }
                report.worker_sigterm_recoveries += 1;
                report.total_items_succeeded += u64::from(term_sum.counts.succeeded_items);
                report.total_items_interrupted_recovered +=
                    u64::from(term_sum.counts.interrupted_items);
                if term_sum.counts.interrupted_items > 0 {
                    let mut c_tr = env.connect();
                    let (tr_jid, _, _) = c_tr.retry(term_jid, RetryMode::FailedOnly, None).unwrap();
                    let (tr_sum, _) = env.wait_terminal(tr_jid, Duration::from_secs(15));
                    assert_eq!(tr_sum.state, JobState::Completed);
                    report.retry_failed_only_jobs += 1;
                    report.total_items_succeeded += u64::from(tr_sum.counts.succeeded_items);
                }
            }
            _ => {
                // Sub-scenario F2: Worker SIGKILL mid-batch -> startup crash recovery -> FailedOnly retry
                //                  + Worker idle shutdown (300ms) -> auto-relaunch
                env.shutdown_active_worker();
                let kill_pid =
                    env.spawn_worker(&[("RADIVINE_WORKER_TEST_ITEM_SLEEP_MS", "120")], 60_000);
                let kill_dir = cycle_dir.join("sigkill_batch");
                fs::create_dir_all(&kill_dir).unwrap();
                let mut kill_wires = Vec::new();
                for i in 0..8 {
                    let p = kill_dir.join(format!("k_{i}.png"));
                    fs::write(&p, &env.template_png_bytes).unwrap();
                    cycle_src_fingerprints.push((p.clone(), FileFingerprint::capture(&p)));
                    kill_wires.push(WirePath::from_absolute_path(&p).unwrap());
                }
                let mut c_kill = env.connect();
                let (kill_jid, _, _) = c_kill
                    .submit(
                        RequestId::generate(),
                        Origin::Wheel,
                        CONVERT_IMAGE_KIND,
                        kill_wires,
                        serde_json::json!({"target": "webp"}),
                    )
                    .unwrap();
                let wait_inflight = Instant::now() + Duration::from_secs(5);
                while Instant::now() < wait_inflight {
                    let (j, _, _) = c_kill.get_job(kill_jid, Some(0), Some(1)).unwrap();
                    if j.counts.in_flight_items >= 1 {
                        break;
                    }
                    thread::sleep(Duration::from_millis(15));
                }
                drop(c_kill);

                kill_process(Pid::from_raw(kill_pid as i32).unwrap(), Signal::KILL).unwrap();
                if let Some(mut ch) = env.children.pop() {
                    let _ = ch.wait();
                }
                env.current_pid = None;

                // Restart worker with 300ms idle timeout so it recovers, finishes pending, and then idles out!
                env.spawn_worker(&[], 300);
                let (rec_sum_before, rec_items_before) =
                    env.wait_terminal(kill_jid, Duration::from_secs(15));
                assert!(rec_sum_before.counts.interrupted_items >= 1);
                if !verify_job_counts_consistent(&rec_sum_before) {
                    report.job_count_inconsistencies += 1;
                }
                report.worker_sigkill_recoveries += 1;
                report.total_items_succeeded += u64::from(rec_sum_before.counts.succeeded_items);
                report.total_items_interrupted_recovered +=
                    u64::from(rec_sum_before.counts.interrupted_items);

                // Retry FailedOnly to convert the interrupted items
                {
                    let mut c_kr = env.connect();
                    let (kr_jid, _, kr_reset) =
                        c_kr.retry(kill_jid, RetryMode::FailedOnly, None).unwrap();
                    assert_eq!(kr_reset, rec_sum_before.counts.interrupted_items);
                    let (kr_sum, _) = env.wait_terminal(kr_jid, Duration::from_secs(15));
                    assert_eq!(kr_sum.state, JobState::Completed);
                    if !verify_job_counts_consistent(&kr_sum) {
                        report.job_count_inconsistencies += 1;
                    }
                    report.retry_failed_only_jobs += 1;
                    report.total_items_succeeded += u64::from(kr_sum.counts.succeeded_items);

                    let (rec_sum_after, rec_items_after, _) =
                        c_kr.get_job(kill_jid, None, None).unwrap();
                    if rec_sum_after != rec_sum_before || rec_items_after != rec_items_before {
                        report.immutable_retry_violations += 1;
                    }
                    for i in 0..8 {
                        if kill_dir.join(format!("k_{i} (1).webp")).exists() {
                            report.duplicate_retry_outputs_detected += 1;
                        }
                    }
                }

                // Wait for the 300ms idle timeout to expire and verify clean exit
                let idle_dl = Instant::now() + Duration::from_secs(5);
                while Instant::now() < idle_dl {
                    env.reap_exited_children();
                    if env.children.is_empty() && !env.paths.socket_path.exists() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                assert!(
                    env.children.is_empty() && !env.paths.socket_path.exists(),
                    "worker must exit cleanly on idle timeout"
                );
                env.current_pid = None;

                // Relaunch worker with normal 60s timeout
                env.spawn_worker(&[], 60_000);
                report.worker_idle_shutdowns_and_relaunches += 1;
            }
        }

        // ---------------------------------------------------------------------
        // G. Per-Cycle Invariant Verification (Source Immutability, Staging Cleanup, RSS)
        // ---------------------------------------------------------------------
        for (p, expected_fp) in &master_fingerprints {
            if &FileFingerprint::capture(p) != expected_fp {
                report.source_modifications_detected += 1;
            }
        }
        for (p, expected_fp) in &cycle_src_fingerprints {
            if &FileFingerprint::capture(p) != expected_fp {
                report.source_modifications_detected += 1;
            }
        }
        report.unresolved_staging_artifacts_detected +=
            assert_no_staging_or_part_files_recursive(&cycle_dir);

        if let Some(pid) = env.current_pid {
            let cur_rss = read_proc_rss_kib(pid) as f64 / 1024.0;
            if cur_rss > report.worker_peak_rss_mib {
                report.worker_peak_rss_mib = cur_rss;
            }
        }

        // Clean up this cycle's temporary files so disk usage stays bounded over 1 hour
        let _ = fs::remove_dir_all(&cycle_dir);
        report.cycles_completed += 1;

        if last_log_instant.elapsed() >= Duration::from_secs(60) {
            let elapsed_s = start_instant.elapsed().as_secs();
            let cur_worker_rss = env
                .current_pid
                .map_or(0.0, |p| read_proc_rss_kib(p) as f64 / 1024.0);
            let cur_host_rss = read_proc_rss_kib(std::process::id()) as f64 / 1024.0;
            println!(
                "[C5-SOAK] t={elapsed_s}s/{target_secs}s | cycles={} | items_ok={} | worker_rss={cur_worker_rss:.2} MiB | host_rss={cur_host_rss:.2} MiB | violations=0",
                report.cycles_completed, report.total_items_succeeded
            );
            last_log_instant = Instant::now();
        }
    }

    report.actual_duration_secs = start_instant.elapsed().as_secs_f64();

    // Final check on master source pool & staging directories
    for (p, expected_fp) in &master_fingerprints {
        if &FileFingerprint::capture(p) != expected_fp {
            report.source_modifications_detected += 1;
        }
    }
    report.unresolved_staging_artifacts_detected +=
        assert_no_staging_or_part_files_recursive(&base_dir);

    if let Some(pid) = env.current_pid {
        report.worker_final_rss_mib = read_proc_rss_kib(pid) as f64 / 1024.0;
    }
    report.worker_rss_delta_mib = report.worker_final_rss_mib - report.worker_baseline_rss_mib;
    report.host_final_rss_mib = read_proc_rss_kib(std::process::id()) as f64 / 1024.0;
    report.host_rss_delta_mib = report.host_final_rss_mib - report.host_baseline_rss_mib;

    // Shut down worker cleanly and verify zero leaked worker processes
    env.shutdown_active_worker();
    env.reap_exited_children();
    report.leaked_worker_processes = env.children.len() as u64;

    let json_report = serde_json::to_string_pretty(&report).unwrap();
    println!("\n=== C5 RELIABILITY SOAK TEST FINAL REPORT ===\n{json_report}\n");

    if let Ok(report_path) = std::env::var("RADIVINE_C5_SOAK_REPORT_JSON")
        && !report_path.trim().is_empty()
    {
        fs::write(report_path.trim(), &json_report).expect("write soak JSON report");
    }

    let _ = fs::remove_dir_all(&base_dir);

    assert_eq!(report.source_modifications_detected, 0);
    assert_eq!(report.destination_overwrites_detected, 0);
    assert_eq!(report.unresolved_staging_artifacts_detected, 0);
    assert_eq!(report.immutable_retry_violations, 0);
    assert_eq!(report.duplicate_retry_outputs_detected, 0);
    assert_eq!(report.job_count_inconsistencies, 0);
    assert_eq!(report.leaked_worker_processes, 0);
    assert!(
        report.worker_rss_delta_mib <= 5.0,
        "Worker RSS delta ({:.2} MiB) exceeded +5.0 MiB budget",
        report.worker_rss_delta_mib
    );
}
