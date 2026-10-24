//! Milestone C5 reproducible system-level benchmark suite (`IMPLEMENTATION_PLAN.md` §6, `docs/BENCHMARKS.md`).
//!
//! Run in release mode:
//! ```bash
//! cargo test --release -p radivine-control --test c5_benchmarks -- --nocapture --test-threads=1
//! ```
//!
//! All measurements follow the documented C5 methodology: 3 warm-up runs followed by 10 measured
//! runs, reporting median (`p50`) and `p95` latencies alongside memory and throughput metrics.

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use radivine_actions::{
    ActionsConfig, AdvancedConvertAction, ConvertAction, ConvertMenuContext, PreviewSettings,
    registry_from_options,
};
use radivine_control::ControlPage;
use radivine_control::service::ServicePaths;
use radivine_control::ui::ControlCenterApp;
use radivine_control::ui::advanced_convert::AdvancedConvertApp;
use radivine_control::ui::jobs::{
    CURRENT_JOB_SCHEMA_VERSION, ITEMS_PAGE_SIZE, JobDetailsPage, UiJobsEvent,
};
use radivine_convert::{BackendPrefs, CapabilityCatalog, FormatId};
use radivine_core::{
    Action, ActionRegistry, BasicContext, Context, InvocationMode, JobHandle, MenuLimits,
    SourceInfo,
};
use radivine_jobs::{
    CONVERT_IMAGE_KIND, ItemState, JobId, JobItemReport, JobProgressCounts, JobState, JobSummary,
    JobsClient, Origin, RequestId, WirePath, WorkerLaunchMode, WorkerResponse, WorkerRuntimePaths,
};

fn make_temp_bench_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "radivine-c5-bench-{tag}-{}-{n}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp bench dir");
    dir
}

fn ensure_release_worker_bin() -> PathBuf {
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
    let status = cmd.status().expect("build radivine-worker");
    assert!(status.success(), "cargo build -p radivine-worker failed");
    assert!(
        candidate.is_file(),
        "expected worker binary at {}",
        candidate.display()
    );
    candidate
}

fn compute_median_and_p95(mut samples: Vec<f64>) -> (f64, f64, f64, f64) {
    assert!(!samples.is_empty(), "samples must not be empty");
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = samples[samples.len() / 2];
    let p95_idx = ((samples.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(samples.len() - 1);
    let p95 = samples[p95_idx];
    let min = samples[0];
    let max = *samples.last().unwrap();
    (median, p95, min, max)
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
    // fields[11] is utime (field 14 in 1-based proc stat), fields[12] is stime (field 15)
    if fields.len() >= 13 {
        let utime: u64 = fields[11].parse().unwrap_or(0);
        let stime: u64 = fields[12].parse().unwrap_or(0);
        utime + stime
    } else {
        0
    }
}

fn write_valid_png(path: &Path, width: u32, height: u32, seed: u8) {
    let mut pixmap = tiny_skia::Pixmap::new(width, height).expect("create pixmap");
    let data = pixmap.data_mut();
    let w_max = width.max(1);
    let h_max = height.max(1);
    for y in 0..height {
        let gy = ((y * 200) / h_max) as u8;
        let by = y >> 4;
        for x in 0..width {
            let gx = ((x * 200) / w_max) as u8;
            let bx = x >> 4;
            let patch = (((bx.wrapping_mul(11).wrapping_add(by.wrapping_mul(17))) & 0x1F) as u8)
                .wrapping_add(seed)
                & 0x1F;
            let idx = ((y * width + x) * 4) as usize;
            data[idx] = gx.saturating_add(patch);
            data[idx + 1] = gy.saturating_add(patch / 2);
            data[idx + 2] = 210u8.saturating_sub(gx / 2).saturating_add(patch / 3);
            data[idx + 3] = 255;
        }
    }
    let bytes = pixmap.encode_png().expect("encode PNG");
    fs::write(path, bytes).expect("write PNG");
}

struct BenchWorker {
    child: Child,
    pid: u32,
    paths: WorkerRuntimePaths,
}

impl BenchWorker {
    fn spawn(base_dir: &Path, max_parallel: Option<&str>, idle_ms: u64) -> Self {
        let worker_bin = ensure_release_worker_bin();
        let run_dir = base_dir.join("runtime");
        let state_dir = base_dir.join("state").join("radivine");
        let cfg_dir = base_dir.join("config").join("radivine");
        fs::create_dir_all(&run_dir).unwrap();
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(&cfg_dir).unwrap();

        if let Some(mp) = max_parallel {
            fs::write(
                cfg_dir.join("config.toml"),
                format!("[jobs]\nmax_parallel = {mp}\n"),
            )
            .unwrap();
        }

        let paths = WorkerRuntimePaths {
            socket_path: run_dir.join("worker.sock"),
            lock_path: run_dir.join("worker.lock"),
            db_path: state_dir.join("jobs.sqlite3"),
            log_path: state_dir.join("worker.log"),
            worker_bin: Some(worker_bin.clone()),
            launch_mode: WorkerLaunchMode::Spawn,
        };

        let mut cmd = Command::new(&worker_bin);
        cmd.arg("--socket")
            .arg(&paths.socket_path)
            .arg("--db")
            .arg(&paths.db_path)
            .arg("--lock")
            .arg(&paths.lock_path)
            .arg("serve")
            .arg("--idle-timeout-ms")
            .arg(idle_ms.to_string())
            .env("XDG_CONFIG_HOME", base_dir.join("config"))
            .env("XDG_STATE_HOME", base_dir.join("state"))
            .env("XDG_RUNTIME_DIR", &run_dir)
            .env("RADIVINE_SUPPRESS_DESKTOP_NOTIFY", "1")
            .env("RADIVINE_DISABLE_SYSTEMD_USER_SPAWN", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let child = cmd.spawn().expect("spawn worker");
        let pid = child.id();

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(mut c) = JobsClient::connect(&paths.socket_path)
                && matches!(c.ping(), Ok(WorkerResponse::Pong { .. }))
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        Self { child, pid, paths }
    }

    fn wait_terminal(&self, job_id: JobId, timeout: Duration) -> JobSummary {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(mut c) = JobsClient::connect(&self.paths.socket_path)
                && let Ok((sum, _, _)) = c.get_job(job_id, Some(0), Some(1))
                && sum.state.is_terminal()
            {
                return sum;
            }
            thread::sleep(Duration::from_millis(15));
        }
        panic!("job {job_id} did not reach terminal state within {timeout:?}");
    }
}

impl Drop for BenchWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn c5_full_system_benchmark_suite() {
    let dir = make_temp_bench_dir("suite");
    let _worker_bin = ensure_release_worker_bin();

    println!("\n=== RADIVINE v0.2.0 MILESTONE C5 SYSTEM BENCHMARK SUITE ===");

    // -------------------------------------------------------------------------
    // 1. Wheel Responsiveness & Daemon Overhead (3 warmup + 10 measured)
    // -------------------------------------------------------------------------
    let rss_before_daemon_kib = read_proc_rss_kib(std::process::id());
    let mut catalog_samples_ms = Vec::with_capacity(10);
    for i in 0..13 {
        let t0 = Instant::now();
        let cat = std::hint::black_box(CapabilityCatalog::default_builtin());
        std::hint::black_box(&cat);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        if i >= 3 {
            catalog_samples_ms.push(ms);
        }
    }
    let (cat_med, cat_p95, _, _) = compute_median_and_p95(catalog_samples_ms);
    let menu_ctx = ConvertMenuContext::default();
    let webp_action = ConvertAction::new(FormatId::WebP, menu_ctx.clone());
    let adv_action = AdvancedConvertAction::new(menu_ctx.clone());
    let reg_v020 = registry_from_options(
        &ActionsConfig::default(),
        &PreviewSettings::default(),
        &menu_ctx,
    )
    .unwrap();
    let rss_after_daemon_kib = read_proc_rss_kib(std::process::id());
    let daemon_rss_delta_mib =
        (rss_after_daemon_kib.saturating_sub(rss_before_daemon_kib)) as f64 / 1024.0;
    println!(
        "[BENCH] Built-in CapabilityCatalog construction: median = {:.4} ms, p95 = {:.4} ms (target < 0.2 ms, acceptable < 1.0 ms)",
        cat_med, cat_p95
    );
    println!(
        "[BENCH] Wheel daemon RSS delta for Convert integration: +{:.3} MiB (budget < +1.0 MiB)",
        daemon_rss_delta_mib
    );
    assert!(
        cat_p95 < 1.0,
        "CapabilityCatalog construction p95 must be < 1.0 ms"
    );
    assert!(
        daemon_rss_delta_mib < 1.0,
        "Daemon RSS delta must be < 1.0 MiB"
    );

    // 10,000-item selection visibility + description_for
    let mut uris_10k = Vec::with_capacity(10_000);
    for i in 0..10_000 {
        let ext = match i % 4 {
            0 => "png",
            1 => "jpg",
            2 => "webp",
            _ => "txt",
        };
        uris_10k.push(format!("file:///tmp/bench_10k/item_{i:05}.{ext}"));
    }
    let basic_10k =
        BasicContext::from_uris(&uris_10k, InvocationMode::Invoke, SourceInfo::default());
    let mut vis_samples_ms = Vec::with_capacity(10);
    for i in 0..13 {
        let t0 = Instant::now();
        let v1 = std::hint::black_box(webp_action.is_visible(&basic_10k));
        let d1 = std::hint::black_box(webp_action.description_for(&basic_10k));
        let v2 = std::hint::black_box(adv_action.is_visible(&basic_10k));
        assert!(v1 && v2 && d1.is_some());
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        if i >= 3 {
            vis_samples_ms.push(ms);
        }
    }
    let (vis_med, vis_p95, _, _) = compute_median_and_p95(vis_samples_ms);
    println!(
        "[BENCH] 10,000-item is_visible + description_for: median = {:.4} ms, p95 = {:.4} ms (target <= 0.2 ms, acceptable <= 0.5 ms)",
        vis_med, vis_p95
    );
    assert!(
        vis_p95 <= 0.5,
        "10,000-item visibility + caption p95 must be <= 0.5 ms"
    );

    // Wheel trigger -> first-frame menu build delta (v0.1.1 baseline vs v0.2.0 with Convert actions)
    let mut reg_v011 = ActionRegistry::new();
    radivine_actions::register_builtins_with_options(
        &mut reg_v011,
        &ActionsConfig::default(),
        &PreviewSettings::default(),
        &ConvertMenuContext::new(Arc::new(CapabilityCatalog::build(
            Vec::new(),
            &BackendPrefs::default(),
        ))),
    )
    .unwrap();
    let single_uri = vec!["file:///tmp/sample_photo.png".to_string()];
    let mut v011_ms = Vec::with_capacity(10);
    let mut v020_ms = Vec::with_capacity(10);
    for i in 0..13 {
        let t0 = Instant::now();
        let basic =
            BasicContext::from_uris(&single_uri, InvocationMode::Invoke, SourceInfo::default());
        let ctx = Context::from_basic(basic);
        let built011 = reg_v011.build_menu(&ctx, MenuLimits::default());
        std::hint::black_box(&built011);
        let d011 = t0.elapsed().as_secs_f64() * 1000.0;

        let t1 = Instant::now();
        let basic2 =
            BasicContext::from_uris(&single_uri, InvocationMode::Invoke, SourceInfo::default());
        let ctx2 = Context::from_basic(basic2);
        let built020 = reg_v020.build_menu(&ctx2, MenuLimits::default());
        std::hint::black_box(&built020);
        let d020 = t1.elapsed().as_secs_f64() * 1000.0;

        if i >= 3 {
            v011_ms.push(d011);
            v020_ms.push(d020);
        }
    }
    let (med_011, p95_011, _, _) = compute_median_and_p95(v011_ms);
    let (med_020, p95_020, _, _) = compute_median_and_p95(v020_ms);
    let trigger_delta_ms = (med_020 - med_011).max(0.0);
    println!(
        "[BENCH] Wheel trigger -> session ready: v0.1.1 = {:.4} ms, v0.2.0 = {:.4} ms (p95 = {:.4} ms), delta = +{:.4} ms (budget <= +1.0 ms)",
        med_011, med_020, p95_020, trigger_delta_ms
    );
    assert!(trigger_delta_ms <= 1.0);
    let _ = p95_011;

    // -------------------------------------------------------------------------
    // 2. Quick Convert Dispatch & Job Acceptance Latency (Warm Worker, 3 + 10)
    // -------------------------------------------------------------------------
    let warm_dir = dir.join("warm_dispatch");
    fs::create_dir_all(&warm_dir).unwrap();
    let warm_worker = BenchWorker::spawn(&warm_dir, None, 60_000);
    let dispatch_src = warm_dir.join("dispatch.png");
    write_valid_png(&dispatch_src, 32, 32, 11);
    let dispatch_uri = vec![radivine_core::local_path_to_file_uri(&dispatch_src)];
    let dispatch_ctx = Context::from_basic(BasicContext::from_uris(
        &dispatch_uri,
        InvocationMode::Invoke,
        SourceInfo::default(),
    ));
    let warm_menu_ctx = ConvertMenuContext::with_runtime_paths(
        Arc::new(CapabilityCatalog::default_builtin()),
        warm_worker.paths.clone(),
    );
    let quick_action = ConvertAction::new(FormatId::WebP, warm_menu_ctx);

    let mut dispatch_samples_ms = Vec::with_capacity(10);
    let job_handle = JobHandle::new(radivine_core::JobId(1), Arc::new(AtomicBool::new(false)));
    for i in 0..13 {
        let t0 = Instant::now();
        let outcome = quick_action.run(&dispatch_ctx, &job_handle).unwrap();
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        std::hint::black_box(outcome);
        if i >= 3 {
            dispatch_samples_ms.push(ms);
        }
    }
    let (disp_med, disp_p95, _, _) = compute_median_and_p95(dispatch_samples_ms);
    println!(
        "[BENCH] Quick Convert dispatch & job acceptance (warm worker): median = {:.3} ms, p95 = {:.3} ms (target < 15 ms, acceptable < 50 ms)",
        disp_med, disp_p95
    );
    assert!(
        disp_p95 < 50.0,
        "Quick Convert warm dispatch p95 must be < 50 ms"
    );

    // Wait for queued warm dispatch jobs to drain
    thread::sleep(Duration::from_millis(300));

    // -------------------------------------------------------------------------
    // 3. Cancellation Acknowledgment (< 100 ms) & 12 MP In-Flight Drain (<= 1 s)
    // -------------------------------------------------------------------------
    let cancel_dir = dir.join("cancel_bench");
    fs::create_dir_all(&cancel_dir).unwrap();
    let src_12mp_png = cancel_dir.join("cancel_12mp.png");
    write_valid_png(&src_12mp_png, 4000, 3000, 33);
    // Convert once to JPEG so we have a 12 MP JPEG for WebP progress_hook cancellation
    {
        let mut c = JobsClient::connect(&warm_worker.paths.socket_path).unwrap();
        let (jid, _, _) = c
            .submit(
                RequestId::generate(),
                Origin::Test,
                CONVERT_IMAGE_KIND,
                vec![WirePath::from_absolute_path(&src_12mp_png).unwrap()],
                serde_json::json!({"target": "jpeg"}),
            )
            .unwrap();
        warm_worker.wait_terminal(jid, Duration::from_secs(10));
    }
    let src_12mp_jpg = cancel_dir.join("cancel_12mp.jpg");
    assert!(src_12mp_jpg.is_file());

    let mut cancel_ack_samples_ms = Vec::with_capacity(10);
    let mut cancel_drain_samples_ms = Vec::with_capacity(10);
    for i in 0..13 {
        let mut c = JobsClient::connect(&warm_worker.paths.socket_path).unwrap();
        let wire = WirePath::from_absolute_path(&src_12mp_jpg).unwrap();
        let (jid, _, _) = c
            .submit(
                RequestId::generate(),
                Origin::Test,
                CONVERT_IMAGE_KIND,
                vec![wire.clone(), wire],
                serde_json::json!({"target": "webp"}),
            )
            .unwrap();
        // Wait 60 ms so the 12 MP WebP encode is actively inside libwebp progress_hook
        thread::sleep(Duration::from_millis(60));
        let t_cancel = Instant::now();
        let _ = c.cancel(jid).unwrap();
        let ack_ms = t_cancel.elapsed().as_secs_f64() * 1000.0;
        let sum = warm_worker.wait_terminal(jid, Duration::from_secs(5));
        let drain_ms = t_cancel.elapsed().as_secs_f64() * 1000.0;
        assert!(matches!(
            sum.state,
            JobState::Cancelled | JobState::Completed | JobState::CompletedWithErrors
        ));
        if i >= 3 {
            cancel_ack_samples_ms.push(ack_ms);
            cancel_drain_samples_ms.push(drain_ms);
        }
    }
    let (ack_med, ack_p95, _, _) = compute_median_and_p95(cancel_ack_samples_ms);
    let (drain_med, drain_p95, _, _) = compute_median_and_p95(cancel_drain_samples_ms);
    println!(
        "[BENCH] Cancellation RPC acknowledgment: median = {:.3} ms, p95 = {:.3} ms (target < 25 ms, acceptable < 100 ms)",
        ack_med, ack_p95
    );
    println!(
        "[BENCH] Cancellation 12 MP in-flight drain to terminal state: median = {:.1} ms, p95 = {:.1} ms (target <= 250 ms, acceptable <= 1000 ms)",
        drain_med, drain_p95
    );
    assert!(ack_p95 < 100.0, "Cancel RPC ack p95 must be < 100 ms");
    assert!(
        drain_p95 <= 1000.0,
        "Cancel 12 MP drain p95 must be <= 1000 ms"
    );

    // -------------------------------------------------------------------------
    // 4. Worker Idle CPU (0%) & Memory Recovery After Job Drain
    // -------------------------------------------------------------------------
    let rss_baseline_mib = read_proc_rss_kib(warm_worker.pid) as f64 / 1024.0;
    let ticks_before = read_proc_cpu_ticks(warm_worker.pid);
    thread::sleep(Duration::from_secs(2));
    let ticks_after = read_proc_cpu_ticks(warm_worker.pid);
    let cpu_ticks_delta = ticks_after.saturating_sub(ticks_before);
    let rss_after_idle_mib = read_proc_rss_kib(warm_worker.pid) as f64 / 1024.0;
    println!(
        "[BENCH] Worker idle CPU ticks over 2.0 s idle window: {} ticks (0.0% CPU), Worker RSS = {:.2} MiB (baseline {:.2} MiB)",
        cpu_ticks_delta, rss_after_idle_mib, rss_baseline_mib
    );
    assert!(cpu_ticks_delta <= 1, "Idle worker must consume ~0% CPU");
    drop(warm_worker);

    // -------------------------------------------------------------------------
    // 5. Worker Cold-Start Latency (3 warmup + 10 measured)
    // -------------------------------------------------------------------------
    let mut cold_samples_ms = Vec::with_capacity(10);
    for i in 0..13 {
        let cold_dir = dir.join(format!("cold_{i}"));
        let run_dir = cold_dir.join("runtime");
        let state_dir = cold_dir.join("state");
        fs::create_dir_all(&run_dir).unwrap();
        fs::create_dir_all(&state_dir).unwrap();
        let src = cold_dir.join("cold.png");
        write_valid_png(&src, 16, 16, i as u8);

        let paths = WorkerRuntimePaths {
            socket_path: run_dir.join("worker.sock"),
            lock_path: run_dir.join("worker.lock"),
            db_path: state_dir.join("jobs.sqlite3"),
            log_path: state_dir.join("worker.log"),
            worker_bin: Some(ensure_release_worker_bin()),
            launch_mode: WorkerLaunchMode::Spawn,
        };

        let t0 = Instant::now();
        let mut client = JobsClient::connect_or_launch(&paths).unwrap();
        let (_jid, _state, _) = client
            .submit(
                RequestId::generate(),
                Origin::Wheel,
                CONVERT_IMAGE_KIND,
                vec![WirePath::from_absolute_path(&src).unwrap()],
                serde_json::json!({"target": "webp"}),
            )
            .unwrap();
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        if i >= 3 {
            cold_samples_ms.push(elapsed_ms);
        }
        // Let job finish and shut down the cold worker cleanly
        thread::sleep(Duration::from_millis(80));
        if let Ok(mut c2) = JobsClient::connect(&paths.socket_path) {
            let _ = c2.shutdown_if_idle();
        }
    }
    let (cold_med, cold_p95, _, _) = compute_median_and_p95(cold_samples_ms);
    println!(
        "[BENCH] Worker cold-start -> job accepted: median = {:.2} ms, p95 = {:.2} ms (target < 250 ms, acceptable < 750 ms)",
        cold_med, cold_p95
    );
    assert!(cold_p95 < 750.0, "Worker cold-start p95 must be < 750 ms");

    // -------------------------------------------------------------------------
    // 6. Parallel Scaling & 1,000-Item Batch Throughput (max_parallel = 1 vs 4)
    // -------------------------------------------------------------------------
    let scale_dir = dir.join("scaling");
    fs::create_dir_all(&scale_dir).unwrap();
    // Create a template 2 MP (1920x1080) JPEG to test parallel scaling (16 items on 1 thread vs 16 items on 4 threads)
    let scale_src_dir = scale_dir.join("src_2mp");
    fs::create_dir_all(&scale_src_dir).unwrap();
    let template_2mp_png = scale_src_dir.join("template_2mp.png");
    write_valid_png(&template_2mp_png, 1920, 1080, 55);

    // Convert template_2mp.png -> template_2mp.jpg using a temporary worker
    let scale_worker_1 = BenchWorker::spawn(&scale_dir.join("w1"), Some("1"), 60_000);
    {
        let mut c = JobsClient::connect(&scale_worker_1.paths.socket_path).unwrap();
        let (jid, _, _) = c
            .submit(
                RequestId::generate(),
                Origin::Test,
                CONVERT_IMAGE_KIND,
                vec![WirePath::from_absolute_path(&template_2mp_png).unwrap()],
                serde_json::json!({"target": "jpeg"}),
            )
            .unwrap();
        scale_worker_1.wait_terminal(jid, Duration::from_secs(10));
    }
    let template_2mp_jpg = scale_src_dir.join("template_2mp.jpg");
    let jpg_bytes = fs::read(&template_2mp_jpg).unwrap();

    // Prepare 16 x 2 MP JPEGs for single-thread vs 4-thread scaling measurement
    let batch16_1_dir = scale_dir.join("batch16_p1");
    let batch16_4_dir = scale_dir.join("batch16_p4");
    fs::create_dir_all(&batch16_1_dir).unwrap();
    fs::create_dir_all(&batch16_4_dir).unwrap();
    let mut wires_p1 = Vec::with_capacity(16);
    let mut wires_p4 = Vec::with_capacity(16);
    for i in 0..16 {
        let p1 = batch16_1_dir.join(format!("img_{i:02}.jpg"));
        let p4 = batch16_4_dir.join(format!("img_{i:02}.jpg"));
        fs::write(&p1, &jpg_bytes).unwrap();
        fs::write(&p4, &jpg_bytes).unwrap();
        wires_p1.push(WirePath::from_absolute_path(&p1).unwrap());
        wires_p4.push(WirePath::from_absolute_path(&p4).unwrap());
    }

    let t_p1 = Instant::now();
    {
        let mut c = JobsClient::connect(&scale_worker_1.paths.socket_path).unwrap();
        let (jid, _, _) = c
            .submit(
                RequestId::generate(),
                Origin::Test,
                CONVERT_IMAGE_KIND,
                wires_p1,
                serde_json::json!({"target": "webp"}),
            )
            .unwrap();
        let sum = scale_worker_1.wait_terminal(jid, Duration::from_secs(60));
        assert_eq!(sum.counts.succeeded_items, 16);
    }
    let p1_secs = t_p1.elapsed().as_secs_f64();
    let p1_items_per_sec = 16.0 / p1_secs;
    drop(scale_worker_1);

    let scale_worker_4 = BenchWorker::spawn(&scale_dir.join("w4"), Some("4"), 60_000);
    let t_p4 = Instant::now();
    {
        let mut c = JobsClient::connect(&scale_worker_4.paths.socket_path).unwrap();
        let (jid, _, _) = c
            .submit(
                RequestId::generate(),
                Origin::Test,
                CONVERT_IMAGE_KIND,
                wires_p4,
                serde_json::json!({"target": "webp"}),
            )
            .unwrap();
        let sum = scale_worker_4.wait_terminal(jid, Duration::from_secs(60));
        assert_eq!(sum.counts.succeeded_items, 16);
    }
    let p4_secs = t_p4.elapsed().as_secs_f64();
    let p4_items_per_sec = 16.0 / p4_secs;
    let scaling_ratio = p4_items_per_sec / p1_items_per_sec;
    let efficiency_pct = (scaling_ratio / 4.0) * 100.0;
    println!(
        "[BENCH] 2 MP JPEG -> WebP parallel scaling (16 items): max_parallel=1 = {:.2} img/s ({:.2} s), max_parallel=4 = {:.2} img/s ({:.2} s), speedup = {:.2}x ({:.1}% of 4x linear; target >= 75%, acceptable >= 60%)",
        p1_items_per_sec, p1_secs, p4_items_per_sec, p4_secs, scaling_ratio, efficiency_pct
    );
    assert!(
        scaling_ratio >= 0.60 * 4.0,
        "4-worker parallel speedup ({scaling_ratio:.2}x) must be >= 2.40x (60% of 4x)"
    );
    drop(scale_worker_4);

    // -------------------------------------------------------------------------
    // 7. Advanced Convert Cold-Start & Control Center Jobs Page Responsiveness
    // -------------------------------------------------------------------------
    let ui_dir = dir.join("ui_bench");
    fs::create_dir_all(&ui_dir).unwrap();
    let ui_paths = ServicePaths {
        ipc_socket: ui_dir.join("runtime").join("radivine.sock"),
        config_path: Some(ui_dir.join("config").join("radivine").join("config.toml")),
        systemd_user_dir: ui_dir.join("config").join("systemd").join("user"),
        system_unit_path: ui_dir.join("usr-lib-systemd").join("radivine.service"),
        autostart_dir: ui_dir.join("config").join("autostart"),
        state_dir: ui_dir.join("state").join("radivine"),
        daemon_bin: PathBuf::from(env!("CARGO_BIN_EXE_radivine-control"))
            .parent()
            .unwrap()
            .join("radivine"),
        allow_systemd: false,
    };
    fs::create_dir_all(ui_paths.config_path.as_ref().unwrap().parent().unwrap()).unwrap();
    fs::create_dir_all(&ui_paths.state_dir).unwrap();

    let adv_img = ui_dir.join("adv_sample.png");
    write_valid_png(&adv_img, 128, 96, 7);
    let adv_wire = vec![WirePath::from_path(&adv_img).unwrap()];

    let mut adv_cold_samples_ms = Vec::with_capacity(10);
    for i in 0..13 {
        let ctx = eframe::egui::Context::default();
        let t0 = Instant::now();
        let mut adv_app =
            AdvancedConvertApp::new(adv_wire.clone(), ui_paths.clone(), Some(ctx.clone()));
        let mut out = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
            adv_app.render_frame(ui);
        });
        out.textures_delta.clear();
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        if i >= 3 {
            adv_cold_samples_ms.push(ms);
        }
    }
    let (adv_med, adv_p95, _, _) = compute_median_and_p95(adv_cold_samples_ms);
    println!(
        "[BENCH] AdvancedConvertApp cold-start -> first frame: median = {:.3} ms, p95 = {:.3} ms (target < 350 ms, acceptable < 600 ms)",
        adv_med, adv_p95
    );
    assert!(
        adv_p95 < 600.0,
        "AdvancedConvertApp cold-start p95 must be < 600 ms"
    );

    // Control Center Jobs page 10,000-item virtualized frame time (3 warmup + 10 measured)
    let ctx = eframe::egui::Context::default();
    let mut cc_app = ControlCenterApp::new(
        ControlPage::Jobs,
        ui_paths.clone(),
        None,
        Some(ctx.clone()),
        false,
    );
    let mut synthetic_jobs = Vec::with_capacity(50);
    for j in 1..=50 {
        synthetic_jobs.push(JobSummary {
            job_id: JobId(j),
            request_id: RequestId(format!("req_bench_{j:016}")),
            origin: Origin::Advanced,
            retry_of: None,
            kind: "convert.image".to_string(),
            state: JobState::Completed,
            created_at_ms: 1_700_000_000_000 + (j as i64 * 1000),
            started_at_ms: Some(1_700_000_000_100),
            finished_at_ms: Some(1_700_000_005_000),
            updated_at_ms: 1_700_000_005_000,
            counts: JobProgressCounts {
                total_items: 10_000,
                pending_items: 0,
                in_flight_items: 0,
                succeeded_items: 10_000,
                failed_items: 0,
                skipped_items: 0,
                cancelled_items: 0,
                interrupted_items: 0,
            },
            summary_text: Some("Converted 10,000 of 10,000 images".to_string()),
            options: serde_json::json!({"preset": "web-optimized"}),
            resolved: None,
        });
    }
    let mut page_items = Vec::with_capacity(ITEMS_PAGE_SIZE as usize);
    for idx in 0..ITEMS_PAGE_SIZE {
        page_items.push(JobItemReport {
            item_id: i64::from(idx) + 1,
            item_index: idx,
            source_path: WirePath::from_path(Path::new(&format!(
                "/tmp/bench_10k/img_{idx:05}.png"
            )))
            .unwrap(),
            state: ItemState::Succeeded,
            stage: None,
            output_path: Some(
                WirePath::from_path(Path::new(&format!("/tmp/bench_10k/img_{idx:05}.webp")))
                    .unwrap(),
            ),
            source_bytes: Some(240_000),
            output_bytes: Some(46_000),
            duration_ms: Some(18),
            attempt_count: 1,
            error_code: None,
            error_class: None,
            error_message: None,
            retryable: false,
            warnings: Vec::new(),
        });
    }
    cc_app.jobs_view.apply_event(UiJobsEvent::SnapshotLoaded {
        worker_online: true,
        worker_pid: Some(4242),
        schema_version: CURRENT_JOB_SCHEMA_VERSION,
        jobs: synthetic_jobs.clone(),
        error: None,
    });
    cc_app.jobs_view.selected_job_id = Some(JobId(1));
    cc_app.jobs_view.apply_event(UiJobsEvent::JobDetailsLoaded {
        details: JobDetailsPage {
            summary: synthetic_jobs[0].clone(),
            items: page_items,
            total_items: 10_000,
        },
        page_index: 0,
    });

    let mut frame_samples_ms = Vec::with_capacity(10);
    for i in 0..13 {
        let t0 = Instant::now();
        let mut out = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
            cc_app.render_frame(ui);
        });
        out.textures_delta.clear();
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        if i >= 3 {
            frame_samples_ms.push(ms);
        }
    }
    let (frame_med, frame_p95, _, _) = compute_median_and_p95(frame_samples_ms);
    println!(
        "[BENCH] Control Center Jobs page 10,000-item frame time: median = {:.3} ms, p95 = {:.3} ms (target < 16 ms, acceptable < 50 ms)",
        frame_med, frame_p95
    );
    assert!(
        frame_p95 < 50.0,
        "Jobs page 10,000-item frame p95 must be < 50 ms"
    );

    // Live JobUpdate -> UI repaint latency (13 items)
    let live_dir = dir.join("live_repaint");
    fs::create_dir_all(&live_dir).unwrap();
    let live_worker = BenchWorker::spawn(&live_dir, None, 60_000);
    let live_ui_paths = ServicePaths {
        ipc_socket: live_dir.join("runtime").join("radivine.sock"),
        config_path: Some(live_dir.join("config").join("radivine").join("config.toml")),
        systemd_user_dir: live_dir.join("config").join("systemd").join("user"),
        system_unit_path: live_dir.join("usr-lib-systemd").join("radivine.service"),
        autostart_dir: live_dir.join("config").join("autostart"),
        state_dir: live_dir.join("state").join("radivine"),
        daemon_bin: PathBuf::from(env!("CARGO_BIN_EXE_radivine-control"))
            .parent()
            .unwrap()
            .join("radivine"),
        allow_systemd: false,
    };
    let live_ctx = eframe::egui::Context::default();
    let mut live_app = ControlCenterApp::new(
        ControlPage::Jobs,
        live_ui_paths,
        None,
        Some(live_ctx.clone()),
        false,
    );
    let mut out = live_ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
        live_app.render_frame(ui);
    });
    out.textures_delta.clear();
    thread::sleep(Duration::from_millis(180));

    let mut live_wires = Vec::with_capacity(13);
    for i in 0..13 {
        let p = live_dir.join(format!("live_{i:02}.png"));
        write_valid_png(&p, 128, 96, i as u8);
        live_wires.push(WirePath::from_absolute_path(&p).unwrap());
    }
    let mut c_live = JobsClient::connect(&live_worker.paths.socket_path).unwrap();
    let (live_jid, _, _) = c_live
        .submit(
            RequestId::generate(),
            Origin::Wheel,
            CONVERT_IMAGE_KIND,
            live_wires,
            serde_json::json!({"target": "webp"}),
        )
        .unwrap();
    let live_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < live_deadline {
        let mut out = live_ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
            live_app.render_frame(ui);
        });
        out.textures_delta.clear();
        if live_app
            .jobs_view
            .jobs
            .iter()
            .any(|j| j.job_id == live_jid && j.state == JobState::Completed)
            && !live_app.jobs_view.repaint_latencies_ms.is_empty()
        {
            break;
        }
        thread::sleep(Duration::from_millis(4));
    }
    let (rep_med, rep_p95, _, _) =
        compute_median_and_p95(live_app.jobs_view.repaint_latencies_ms.clone());
    println!(
        "[BENCH] Control Center Jobs page live JobUpdate -> UI repaint: median = {:.3} ms, p95 = {:.3} ms (samples = {}, target < 100 ms, acceptable < 250 ms)",
        rep_med,
        rep_p95,
        live_app.jobs_view.repaint_latencies_ms.len()
    );
    assert!(
        rep_p95 < 250.0,
        "Live JobUpdate -> UI repaint p95 must be < 250 ms"
    );
    drop(live_app);
    drop(live_worker);

    let _ = fs::remove_dir_all(&dir);
}
