//! Milestone C4 end-to-end acceptance and performance test suite (`AC-C4.1` through `AC-C4.7`).
//!
//! All tests run in strict isolation using temporary XDG directories, dedicated Unix sockets,
//! and headless software rasterization (`EguiSoftRasterizer`), never touching the owner's live
//! desktop session, `~/.config/radivine`, `~/.local/state/radivine`, or `radivine.service`.

#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::fs;
use std::io::Cursor;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use radivine_control::ControlPage;
use radivine_control::persistence::convert_path_for;
use radivine_control::screenshot::EguiSoftRasterizer;
use radivine_control::service::ServicePaths;
use radivine_control::single_instance::ControlInstanceLock;
use radivine_control::ui::advanced_convert::{
    AdvancedConvertApp, UiDestMode, UiResizeMode, inspect_selection_headers, pixel_decode_count,
    read_bounded_stdin_paths,
};
use radivine_control::ui::jobs::{
    CURRENT_JOB_SCHEMA_VERSION, ITEMS_PAGE_SIZE, JobDetailsPage, JobsCommand, UiJobsEvent,
    path_to_file_uri,
};
use radivine_control::ui::{BrandPalette, ControlCenterApp, export_all_page_screenshots};
use radivine_convert::{
    ConvertSettings, ExifPolicy, FormatId, JpegExt, NamingPolicy, OutputLocation,
    ResourceLimitsConfig,
};
use radivine_jobs::{
    ItemState, JobId, JobItemReport, JobProgressCounts, JobState, JobSummary, JobsClient,
    MaxParallelSetting, Origin, RequestId, RetryMode, WirePath, WorkerResponse,
};

fn make_temp_test_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("radivine-c4-{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}

fn ensure_worker_bin() -> PathBuf {
    static WORKER_BIN: OnceLock<PathBuf> = OnceLock::new();
    WORKER_BIN
        .get_or_init(|| {
            let control_bin = PathBuf::from(env!("CARGO_BIN_EXE_radivine-control"));
            let bin_dir = control_bin.parent().expect("target debug dir");
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
            let status = cmd
                .status()
                .expect("build radivine-worker binary for C4 acceptance tests");
            assert!(status.success(), "cargo build -p radivine-worker failed");
            assert!(
                candidate.is_file(),
                "expected radivine-worker binary at {}",
                candidate.display()
            );
            candidate
        })
        .clone()
}

fn isolated_service_paths(base_dir: &Path) -> ServicePaths {
    let _ = ensure_worker_bin();
    let daemon_bin = PathBuf::from(env!("CARGO_BIN_EXE_radivine-control"))
        .parent()
        .expect("target debug dir")
        .join("radivine");

    let cfg_dir = base_dir.join("config").join("radivine");
    let runtime_dir = base_dir.join("runtime");
    let state_dir = base_dir.join("state").join("radivine");
    fs::create_dir_all(&cfg_dir).expect("create config dir");
    fs::create_dir_all(&runtime_dir).expect("create runtime dir");
    fs::create_dir_all(&state_dir).expect("create state dir");

    ServicePaths {
        ipc_socket: runtime_dir.join("radivine.sock"),
        config_path: Some(cfg_dir.join("config.toml")),
        systemd_user_dir: base_dir.join("config").join("systemd").join("user"),
        system_unit_path: base_dir.join("usr-lib-systemd").join("radivine.service"),
        autostart_dir: base_dir.join("config").join("autostart"),
        state_dir,
        daemon_bin,
        allow_systemd: false,
    }
}

/// Writes a valid RGBA PNG using `tiny-skia` (which has an alpha channel, `color_type = 6`).
fn write_valid_rgba_png(path: &Path, width: u32, height: u32, seed: u8) {
    let mut pixmap = tiny_skia::Pixmap::new(width, height).expect("create tiny-skia pixmap");
    let data = pixmap.data_mut();
    for (idx, chunk) in data.chunks_exact_mut(4).enumerate() {
        let b = (idx as u8).wrapping_add(seed);
        chunk[0] = b;
        chunk[1] = b.wrapping_mul(3);
        chunk[2] = seed.wrapping_mul(11);
        chunk[3] = 255;
    }
    let png_bytes = pixmap.encode_png().expect("encode valid PNG");
    fs::write(path, png_bytes).expect("write PNG file");
}

fn step_ui<F: FnMut(&mut eframe::egui::Ui)>(ctx: &eframe::egui::Context, f: F) {
    let mut out = ctx.run_ui(eframe::egui::RawInput::default(), f);
    out.textures_delta.clear();
}

struct TestWorkerGuard {
    child: Child,
    pub socket_path: PathBuf,
    pub db_path: PathBuf,
}

impl TestWorkerGuard {
    fn spawn(base_dir: &Path, paths: &ServicePaths, item_sleep_ms: Option<u64>) -> Self {
        let worker_bin = ensure_worker_bin();
        let socket_path = paths.worker_socket_path();
        let db_path = paths.state_dir.join("jobs.sqlite3");
        let lock_path = socket_path.parent().unwrap().join("worker.lock");

        let mut cmd = Command::new(&worker_bin);
        cmd.arg("serve")
            .arg("--socket")
            .arg(&socket_path)
            .arg("--db")
            .arg(&db_path)
            .arg("--lock")
            .arg(&lock_path)
            .env("XDG_CONFIG_HOME", base_dir.join("config"))
            .env("XDG_STATE_HOME", base_dir.join("state"))
            .env("XDG_RUNTIME_DIR", base_dir.join("runtime"))
            .env("RADIVINE_SUPPRESS_DESKTOP_NOTIFY", "1")
            .env("RADIVINE_DISABLE_SYSTEMD_USER_SPAWN", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        if let Some(ms) = item_sleep_ms {
            cmd.env("RADIVINE_WORKER_TEST_ITEM_SLEEP_MS", ms.to_string());
        }

        let child = cmd.spawn().expect("spawn isolated radivine-worker");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while Instant::now() < deadline {
            if let Ok(mut client) = JobsClient::connect(&socket_path)
                && matches!(client.ping(), Ok(WorkerResponse::Pong { .. }))
            {
                ready = true;
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            ready,
            "isolated radivine-worker did not become ready at {}",
            socket_path.display()
        );

        Self {
            child,
            socket_path,
            db_path,
        }
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.socket_path);
    }
}

impl Drop for TestWorkerGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

#[test]
fn ac_c4_1_persistent_job_visibility_and_lifecycle_independence() {
    let dir = make_temp_test_dir("ac-c4-1");
    let paths = isolated_service_paths(&dir);
    let mut worker = TestWorkerGuard::spawn(&dir, &paths, Some(70));

    let src_dir = dir.join("src_images");
    fs::create_dir_all(&src_dir).expect("create src_images");
    let mut wire_items = Vec::new();
    for i in 0..6 {
        let p = src_dir.join(format!("img_{i}.png"));
        write_valid_rgba_png(&p, 32, 32, i as u8);
        wire_items.push(WirePath::from_path(&p).expect("WirePath"));
    }

    // 1. Submit a 6-item conversion job to the worker
    let mut client = JobsClient::connect(&worker.socket_path).expect("connect worker");
    let opts = serde_json::json!({ "target": "webp" });
    let (job_id, _, _) = client
        .submit(
            RequestId::generate(),
            Origin::Wheel,
            "convert.image",
            wire_items,
            opts,
        )
        .expect("submit batch job");

    // 2. Open ControlCenterApp on ControlPage::Jobs while job is running
    let ctx = eframe::egui::Context::default();
    let mut app1 = ControlCenterApp::new(
        ControlPage::Jobs,
        paths.clone(),
        None,
        Some(ctx.clone()),
        false,
    );

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        app1.drain_all_events();
        if app1.jobs_view.jobs.iter().any(|j| j.job_id == job_id) {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        app1.jobs_view.jobs.iter().any(|j| j.job_id == job_id),
        "active job must be visible in ControlCenterApp::jobs_view"
    );

    // 3. Close (drop) ControlCenterApp while the job is still running
    drop(app1);

    // 4. Verify closing ControlCenterApp does NOT interrupt the worker job
    let wait_deadline = Instant::now() + Duration::from_secs(10);
    let mut final_summary = None;
    while Instant::now() < wait_deadline {
        let mut c = JobsClient::connect(&worker.socket_path).expect("connect worker");
        let (summary, _items, _total) = c.get_job(job_id, Some(0), Some(10)).expect("get_job");
        if summary.state.is_terminal() {
            final_summary = Some(summary);
            break;
        }
        thread::sleep(Duration::from_millis(30));
    }
    let summary = final_summary.expect("job must complete after ControlCenterApp closes");
    assert_eq!(summary.state, JobState::Completed);
    assert_eq!(summary.counts.succeeded_items, 6);
    for i in 0..6 {
        assert!(
            src_dir.join(format!("img_{i}.webp")).is_file(),
            "converted img_{i}.webp must exist on disk"
        );
    }

    // 5. Reopen a fresh ControlCenterApp and verify the completed job & items are visible
    let mut app2 = ControlCenterApp::new(
        ControlPage::Jobs,
        paths.clone(),
        None,
        Some(ctx.clone()),
        false,
    );
    let _ = app2
        .jobs_command_sender()
        .send(JobsCommand::LoadJobDetails {
            job_id,
            page_index: 0,
        });

    let reopen_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < reopen_deadline {
        app2.drain_all_events();
        if app2
            .jobs_view
            .jobs
            .iter()
            .any(|j| j.job_id == job_id && j.state == JobState::Completed)
            && app2
                .jobs_view
                .selected_job_details
                .as_ref()
                .is_some_and(|d| d.summary.job_id == job_id && d.items.len() == 6)
        {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        app2.jobs_view
            .jobs
            .iter()
            .any(|j| j.job_id == job_id && j.state == JobState::Completed),
        "completed job must remain visible after reopening Control Center"
    );
    let details = app2
        .jobs_view
        .selected_job_details
        .as_ref()
        .expect("job details loaded");
    assert_eq!(details.items.len(), 6);
    assert!(
        details
            .items
            .iter()
            .all(|it| it.state == ItemState::Succeeded)
    );
    drop(app2);

    // 6. Stop worker (simulating 120s idle exit) and reopen ControlCenterApp on Jobs:
    // because jobs.sqlite3 exists, ControlCenterApp auto-launches the worker and shows history.
    assert!(
        worker.db_path.is_file(),
        "jobs.sqlite3 must exist on disk at {}",
        worker.db_path.display()
    );
    worker.stop();
    let mut app3 = ControlCenterApp::new(ControlPage::Jobs, paths.clone(), None, Some(ctx), false);
    let auto_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < auto_deadline {
        app3.drain_all_events();
        if app3.jobs_view.jobs.iter().any(|j| j.job_id == job_id) {
            break;
        }
        thread::sleep(Duration::from_millis(30));
    }
    assert!(
        app3.jobs_view.jobs.iter().any(|j| j.job_id == job_id),
        "persistent job history must be available after worker idle exit and Control Center reopen"
    );
    drop(app3);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ac_c4_2_live_progress_latency_and_10000_item_virtualized_frame_budget() {
    let dir = make_temp_test_dir("ac-c4-2");
    let paths = isolated_service_paths(&dir);
    let _worker = TestWorkerGuard::spawn(&dir, &paths, Some(25));

    let src_dir = dir.join("perf_src");
    fs::create_dir_all(&src_dir).expect("create perf_src");
    let mut wire_items = Vec::new();
    for i in 0..8 {
        let p = src_dir.join(format!("frame_{i}.png"));
        write_valid_rgba_png(&p, 24, 24, i as u8);
        wire_items.push(WirePath::from_path(&p).expect("WirePath"));
    }

    let ctx = eframe::egui::Context::default();
    let mut app = ControlCenterApp::new(
        ControlPage::Jobs,
        paths.clone(),
        None,
        Some(ctx.clone()),
        false,
    );
    // Warm up initial window frame (one-time font & logo texture upload) and allow subscription thread to connect
    step_ui(&ctx, |ui| {
        app.render_frame(ui);
    });
    thread::sleep(Duration::from_millis(180));

    let mut client = JobsClient::connect(&paths.worker_socket_path()).expect("connect worker");
    let (job_id, _, _) = client
        .submit(
            RequestId::generate(),
            Origin::Wheel,
            "convert.image",
            wire_items,
            serde_json::json!({ "target": "webp" }),
        )
        .expect("submit perf job");

    let mut rasterizer = EguiSoftRasterizer::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        step_ui(&ctx, |ui| {
            app.render_frame(ui);
        });
        if app
            .jobs_view
            .jobs
            .iter()
            .any(|j| j.job_id == job_id && j.state == JobState::Completed)
            && !app.jobs_view.repaint_latencies_ms.is_empty()
        {
            break;
        }
        thread::sleep(Duration::from_millis(8));
    }

    let p95_ms = app
        .jobs_view
        .p95_repaint_latency_ms()
        .expect("must record live event -> repaint latencies");
    println!(
        "[C4-PERF] Live JobUpdate -> UI repaint p95 latency: {:.3} ms (samples: {})",
        p95_ms,
        app.jobs_view.repaint_latencies_ms.len()
    );
    assert!(
        p95_ms < 250.0,
        "AC-C4.2: JobUpdate -> UI repaint p95 ({p95_ms:.2} ms) must be < 250 ms"
    );

    // Part B: 10,000-item stress history rendering & pagination (`< 50 ms` per frame budget)
    let mut synthetic_jobs = Vec::with_capacity(50);
    for j in 1..=50 {
        synthetic_jobs.push(JobSummary {
            job_id: JobId(j),
            request_id: RequestId(format!("req_stress_{j:016}")),
            origin: Origin::Advanced,
            retry_of: if j == 2 { Some(JobId(1)) } else { None },
            kind: "convert.image".to_string(),
            state: if j == 1 {
                JobState::CompletedWithErrors
            } else {
                JobState::Completed
            },
            created_at_ms: 1_700_000_000_000 + (j as i64 * 1000),
            started_at_ms: Some(1_700_000_000_100),
            finished_at_ms: Some(1_700_000_005_000),
            updated_at_ms: 1_700_000_005_000,
            counts: JobProgressCounts {
                total_items: if j == 1 { 10_000 } else { 250 },
                pending_items: 0,
                in_flight_items: 0,
                succeeded_items: if j == 1 { 9_990 } else { 250 },
                failed_items: if j == 1 { 5 } else { 0 },
                skipped_items: if j == 1 { 3 } else { 0 },
                cancelled_items: 0,
                interrupted_items: if j == 1 { 2 } else { 0 },
            },
            summary_text: Some("Converted 9,990 of 10,000 images".to_string()),
            options: serde_json::json!({"preset": "webp-balanced"}),
            resolved: None,
        });
    }

    let mut page_items = Vec::with_capacity(ITEMS_PAGE_SIZE as usize);
    for idx in 0..ITEMS_PAGE_SIZE {
        let state = match idx % 5 {
            0..=2 => ItemState::Succeeded,
            3 => ItemState::Failed,
            _ => ItemState::Skipped,
        };
        page_items.push(JobItemReport {
            item_id: i64::from(idx) + 1,
            item_index: idx,
            source_path: WirePath::from_path(Path::new(&format!(
                "/tmp/stress_10k/photo_{idx:05}.png"
            )))
            .expect("WirePath"),
            state,
            stage: None,
            output_path: (state == ItemState::Succeeded).then(|| {
                WirePath::from_path(Path::new(&format!("/tmp/stress_10k/photo_{idx:05}.webp")))
                    .expect("WirePath")
            }),
            source_bytes: Some(240_000),
            output_bytes: (state == ItemState::Succeeded).then_some(64_000),
            duration_ms: Some(18),
            attempt_count: 1,
            error_code: (state == ItemState::Failed).then(|| "E_DECODE_CORRUPT".to_string()),
            error_class: (state == ItemState::Failed).then(|| "CorruptInput".to_string()),
            error_message: (state == ItemState::Failed)
                .then(|| "Invalid PNG IHDR CRC in source file".to_string()),
            retryable: state == ItemState::Failed,
            warnings: Vec::new(),
        });
    }

    app.jobs_view.apply_event(UiJobsEvent::SnapshotLoaded {
        worker_online: true,
        worker_pid: Some(4242),
        schema_version: CURRENT_JOB_SCHEMA_VERSION,
        jobs: synthetic_jobs.clone(),
        error: None,
    });
    app.jobs_view.selected_job_id = Some(JobId(1));
    app.jobs_view.apply_event(UiJobsEvent::JobDetailsLoaded {
        details: JobDetailsPage {
            summary: synthetic_jobs[0].clone(),
            items: page_items,
            total_items: 10_000,
        },
        page_index: 0,
    });

    assert_eq!(
        app.jobs_view
            .selected_job_details
            .as_ref()
            .unwrap()
            .total_items
            .div_ceil(ITEMS_PAGE_SIZE),
        50
    );

    let mut max_frame_ms = 0.0_f64;
    let mut max_jobs_page_ms = 0.0_f64;
    for frame_idx in 0..10 {
        let t0 = Instant::now();
        step_ui(&ctx, |ui| {
            app.render_frame(ui);
        });
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        if frame_idx > 0 {
            // Frame 0 uploads embedded font/logo textures once; measure steady-state UI frames 1..10
            max_frame_ms = max_frame_ms.max(elapsed_ms);
            max_jobs_page_ms = max_jobs_page_ms.max(app.jobs_view.last_frame_render_ms);
        }
    }

    // Also render one full PNG screenshot of the 10,000-item stress view
    let stress_png = dir.join("jobs-10k-stress.png");
    rasterizer
        .render_to_png(
            &ctx,
            eframe::egui::Vec2::new(1040.0, 680.0),
            1.0,
            &stress_png,
            |ui| {
                app.render_frame(ui);
            },
        )
        .expect("render 10k stress PNG");
    assert!(stress_png.is_file());

    println!(
        "[C4-PERF] 10,000-item stress job steady-state max frame time: {:.3} ms ( render_jobs_page internal max: {:.3} ms )",
        max_frame_ms, max_jobs_page_ms
    );
    assert!(
        max_frame_ms < 50.0,
        "AC-C4.2: steady-state frame time ({max_frame_ms:.2} ms) on 10,000-item job must be < 50 ms"
    );
    assert!(
        max_jobs_page_ms < 50.0,
        "AC-C4.2: render_jobs_page internal duration ({max_jobs_page_ms:.2} ms) must be < 50 ms"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ac_c4_3_cancel_retry_modes_retry_to_folder_open_folder_clear_history_and_staging() {
    let dir = make_temp_test_dir("ac-c4-3");
    let paths = isolated_service_paths(&dir);
    let worker = TestWorkerGuard::spawn(&dir, &paths, Some(80));

    let ctx = eframe::egui::Context::default();
    let mut app = ControlCenterApp::new(
        ControlPage::Jobs,
        paths.clone(),
        None,
        Some(ctx.clone()),
        false,
    );
    let cmd_tx = app.jobs_command_sender();

    // --- 1. Cancel running job ---
    let cancel_dir = dir.join("cancel_batch");
    fs::create_dir_all(&cancel_dir).expect("create cancel_batch");
    let mut cancel_items = Vec::new();
    for i in 0..8 {
        let p = cancel_dir.join(format!("c_{i}.png"));
        write_valid_rgba_png(&p, 32, 32, i as u8);
        cancel_items.push(WirePath::from_path(&p).expect("WirePath"));
    }

    let mut client = JobsClient::connect(&worker.socket_path).expect("connect worker");
    let (cancel_job_id, _, _) = client
        .submit(
            RequestId::generate(),
            Origin::Wheel,
            "convert.image",
            cancel_items,
            serde_json::json!({ "target": "webp" }),
        )
        .expect("submit cancel batch");

    thread::sleep(Duration::from_millis(50));
    cmd_tx
        .send(JobsCommand::CancelJob {
            job_id: cancel_job_id,
        })
        .expect("send CancelJob");

    let cancel_deadline = Instant::now() + Duration::from_secs(5);
    let mut cancelled_summary = None;
    while Instant::now() < cancel_deadline {
        let mut c = JobsClient::connect(&worker.socket_path).expect("connect worker");
        let (sum, _, _) = c.get_job(cancel_job_id, Some(0), Some(10)).expect("get");
        if sum.state.is_terminal() {
            cancelled_summary = Some(sum);
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    let cancelled_sum = cancelled_summary.expect("cancelled job must reach terminal state");
    assert!(
        matches!(
            cancelled_sum.state,
            JobState::Cancelled | JobState::CompletedWithErrors
        ),
        "expected Cancelled or CompletedWithErrors, got {:?}",
        cancelled_sum.state
    );
    assert!(
        cancelled_sum.counts.cancelled_items > 0,
        "at least one queued item must be marked Cancelled"
    );

    // --- 2. Retry Failed (RetryMode::FailedOnly) & Immutable History Contract ---
    let retry_dir = dir.join("retry_batch");
    fs::create_dir_all(&retry_dir).expect("create retry_batch");
    let good1 = retry_dir.join("good1.png");
    let good2 = retry_dir.join("good2.png");
    let bad3 = retry_dir.join("bad3.png");
    write_valid_rgba_png(&good1, 28, 28, 1);
    write_valid_rgba_png(&good2, 28, 28, 2);
    fs::write(&bad3, b"corrupt-not-a-valid-png-payload").expect("write corrupt bad3.png");

    let retry_items = vec![
        WirePath::from_path(&good1).expect("WirePath"),
        WirePath::from_path(&good2).expect("WirePath"),
        WirePath::from_path(&bad3).expect("WirePath"),
    ];
    let mut client = JobsClient::connect(&worker.socket_path).expect("connect worker");
    let (orig_job_id, _, _) = client
        .submit(
            RequestId::generate(),
            Origin::Wheel,
            "convert.image",
            retry_items,
            serde_json::json!({ "target": "webp" }),
        )
        .expect("submit partial job");

    let orig_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < orig_deadline {
        let mut c = JobsClient::connect(&worker.socket_path).expect("connect");
        let (sum, _, _) = c.get_job(orig_job_id, Some(0), Some(10)).expect("get");
        if sum.state.is_terminal() {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }

    let mut c = JobsClient::connect(&worker.socket_path).expect("connect");
    let (orig_summary_before, orig_items_before, orig_total_before) = c
        .get_job(orig_job_id, Some(0), Some(10))
        .expect("snapshot original job before retry");
    assert_eq!(orig_summary_before.state, JobState::CompletedWithErrors);
    assert_eq!(orig_summary_before.counts.succeeded_items, 2);
    assert_eq!(orig_summary_before.counts.failed_items, 1);
    assert_eq!(orig_total_before, 3);

    // Replace bad3.png with a valid PNG so the retry will succeed
    write_valid_rgba_png(&bad3, 28, 28, 3);

    cmd_tx
        .send(JobsCommand::RetryJob {
            job_id: orig_job_id,
            mode: RetryMode::FailedOnly,
            destination_override: None,
        })
        .expect("send RetryJob FailedOnly");

    let mut retry_job_id = None;
    let retry_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < retry_deadline {
        app.drain_all_events();
        if let Some(sel_id) = app.jobs_view.selected_job_id
            && sel_id != orig_job_id
        {
            retry_job_id = Some(sel_id);
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let new_job_id = retry_job_id.expect("RetryCreated must select new linked retry job");

    // Wait for the retry job to finish
    let finish_deadline = Instant::now() + Duration::from_secs(5);
    let mut new_job_summary = None;
    while Instant::now() < finish_deadline {
        let mut c = JobsClient::connect(&worker.socket_path).expect("connect");
        let (sum, _, _) = c.get_job(new_job_id, Some(0), Some(10)).expect("get");
        if sum.state.is_terminal() {
            new_job_summary = Some(sum);
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    let new_sum = new_job_summary.expect("linked retry job must complete");
    assert_eq!(
        new_sum.retry_of,
        Some(orig_job_id),
        "retry job must reference original job via retry_of"
    );
    assert_eq!(
        new_sum.counts.total_items, 1,
        "FailedOnly retry must include only the 1 failed item (never reprocess Succeeded items)"
    );
    assert_eq!(new_sum.counts.succeeded_items, 1);
    assert!(retry_dir.join("bad3.webp").is_file());

    // Verify original job and its item rows in SQLite are 100% immutable!
    let mut c = JobsClient::connect(&worker.socket_path).expect("connect");
    let (orig_summary_after, orig_items_after, orig_total_after) = c
        .get_job(orig_job_id, Some(0), Some(10))
        .expect("snapshot original job after retry");
    assert_eq!(orig_summary_after, orig_summary_before);
    assert_eq!(orig_items_after, orig_items_before);
    assert_eq!(orig_total_after, orig_total_before);

    // --- 3. Retry to Folder (RetryMode::All with destination_override) ---
    let custom_out_dir = dir.join("retry_custom_out");
    fs::create_dir_all(&custom_out_dir).expect("create custom_out_dir");
    cmd_tx
        .send(JobsCommand::RetryJob {
            job_id: orig_job_id,
            mode: RetryMode::All,
            destination_override: Some(custom_out_dir.clone()),
        })
        .expect("send RetryJob to folder");

    let mut folder_retry_id = None;
    let folder_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < folder_deadline {
        app.drain_all_events();
        if let Some(sel_id) = app.jobs_view.selected_job_id
            && sel_id != orig_job_id
            && sel_id != new_job_id
        {
            folder_retry_id = Some(sel_id);
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let folder_job_id = folder_retry_id.expect("Retry to folder must create linked job");

    let folder_finish = Instant::now() + Duration::from_secs(5);
    while Instant::now() < folder_finish {
        let mut c = JobsClient::connect(&worker.socket_path).expect("connect");
        let (sum, _, _) = c.get_job(folder_job_id, Some(0), Some(10)).expect("get");
        if sum.state.is_terminal() {
            assert_eq!(sum.state, JobState::Completed);
            assert_eq!(sum.counts.succeeded_items, 3);
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    assert!(custom_out_dir.join("good1.webp").is_file());
    assert!(custom_out_dir.join("good2.webp").is_file());
    assert!(custom_out_dir.join("bad3.webp").is_file());

    // --- 4. Open Folder URI formatting & command dispatch ---
    let uri = path_to_file_uri(&custom_out_dir.join("good1.webp"));
    assert!(uri.starts_with("file:///"));
    assert!(uri.ends_with("/good1.webp"));

    // --- 5. Staging Inspection & Cleanup ---
    cmd_tx
        .send(JobsCommand::InspectStaging)
        .expect("send InspectStaging");
    cmd_tx
        .send(JobsCommand::CleanupStaging)
        .expect("send CleanupStaging");
    let stage_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < stage_deadline {
        app.drain_all_events();
        if app.jobs_view.staging_report.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        app.jobs_view.staging_report.is_some(),
        "staging report must populate after InspectStaging/CleanupStaging"
    );

    // --- 6. Clear History (removes terminal jobs from SQLite without deleting converted files) ---
    cmd_tx
        .send(JobsCommand::ClearHistory)
        .expect("send ClearHistory");
    let clear_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < clear_deadline {
        app.drain_all_events();
        if app.jobs_view.jobs.is_empty()
            && app
                .jobs_view
                .banner_ok
                .as_deref()
                .is_some_and(|m| m.contains("Cleared"))
        {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        app.jobs_view.jobs.is_empty(),
        "all terminal jobs must be removed after ClearHistory"
    );
    // Verify converted files on disk were NOT deleted
    assert!(custom_out_dir.join("good1.webp").is_file());
    assert!(custom_out_dir.join("good2.webp").is_file());
    assert!(custom_out_dir.join("bad3.webp").is_file());
    assert!(retry_dir.join("good1.webp").is_file());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ac_c4_4_conversion_settings_persistence_validation_conflicts_comments_and_worker_reload() {
    let dir = make_temp_test_dir("ac-c4-4");
    let paths = isolated_service_paths(&dir);
    let _worker = TestWorkerGuard::spawn(&dir, &paths, None);

    let config_path = paths.config_path.clone().expect("config_path");
    let convert_path = convert_path_for(&config_path);

    let initial_config = "\
# User top-level comment in config.toml
[appearance]
theme = \"dark\"

[jobs]
# Preserve this comment inside [jobs]
max_parallel = \"auto\"
retention_max_jobs = 200
retention_days = 30
custom_unknown_jobs_key = \"preserve_me\"
";
    let initial_convert = "\
# User comment at top of convert.toml
schema_version = 1
default_preset = \"builtin.web-webp\"
quick_convert_target = \"webp\"
future_custom_convert_key = 42

[defaults.output]
# Comment inside output table
location = { kind = \"same_directory\" }
";
    fs::write(&config_path, initial_config).expect("write initial config.toml");
    fs::write(&convert_path, initial_convert).expect("write initial convert.toml");

    let ctx = eframe::egui::Context::default();
    let mut app = ControlCenterApp::new(
        ControlPage::Conversion,
        paths.clone(),
        None,
        Some(ctx.clone()),
        false,
    );

    // 1. Invalid values must fail validation and NEVER write to disk
    app.conversion_page_state.max_dimension_text = "999999".to_string();
    let invalid_res = app.save_convert_and_jobs(false);
    assert!(
        invalid_res.is_err(),
        "out-of-range max_dimension must be rejected"
    );
    let disk_after_invalid = fs::read_to_string(&convert_path).expect("read convert.toml");
    assert_eq!(
        disk_after_invalid, initial_convert,
        "invalid save must not modify convert.toml on disk"
    );

    // 2. Fix validation, modify settings + [jobs], and save atomically
    app.conversion_page_state.max_dimension_text = "16384".to_string();
    app.conversion_page_state.max_megapixels_text = "100".to_string();
    app.conversion_page_state.max_file_mb_text = "256".to_string();
    app.conversion_page_state.bg_mode = "custom".to_string();
    app.conversion_page_state.bg_custom_hex = "#F0F4F8".to_string();
    app.conversion_page_state.destination_mode = "subfolder".to_string();
    app.conversion_page_state.subfolder_text = "ConvertedOut".to_string();
    app.conversion_page_state.filename_suffix_text = "-hq".to_string();
    app.conversion_page_state.jpeg_ext = JpegExt::Jpg;
    app.draft_convert.quick_convert_target = FormatId::Png;
    app.draft_convert.defaults.output.location =
        OutputLocation::Subfolder("ConvertedOut".to_string());
    app.draft_convert.defaults.output.naming = NamingPolicy::AppendSuffix {
        suffix: "-hq".to_string(),
        jpeg_ext: JpegExt::Jpg,
    };
    app.draft_convert.defaults.metadata.exif = ExifPolicy::PreserveAll;
    app.draft_jobs.max_parallel = MaxParallelSetting::Fixed(4);
    app.draft_jobs.retention_max_jobs = 150;
    app.draft_jobs.retention_days = 14;

    app.save_convert_and_jobs(false)
        .expect("valid save_convert_and_jobs must succeed");

    // 3. Verify comments and unknown keys were preserved in both TOML files
    let saved_convert_text = fs::read_to_string(&convert_path).expect("read saved convert.toml");
    assert!(saved_convert_text.contains("# User comment at top of convert.toml"));
    assert!(saved_convert_text.contains("# Comment inside output table"));
    assert!(saved_convert_text.contains("future_custom_convert_key = 42"));

    let saved_config_text = fs::read_to_string(&config_path).expect("read saved config.toml");
    assert!(saved_config_text.contains("# User top-level comment in config.toml"));
    assert!(saved_config_text.contains("# Preserve this comment inside [jobs]"));
    assert!(saved_config_text.contains("custom_unknown_jobs_key = \"preserve_me\""));

    // 4. Reopen a second ControlCenterApp and verify round-trip persistence
    let app2 = ControlCenterApp::new(ControlPage::Conversion, paths.clone(), None, None, false);
    assert_eq!(app2.saved_convert.quick_convert_target, FormatId::Png);
    assert_eq!(
        app2.saved_convert.defaults.output.location,
        OutputLocation::Subfolder("ConvertedOut".to_string())
    );
    assert_eq!(
        app2.saved_convert.defaults.metadata.exif,
        ExifPolicy::PreserveAll
    );
    assert_eq!(app2.saved_jobs.max_parallel, MaxParallelSetting::Fixed(4));
    assert_eq!(app2.saved_jobs.retention_max_jobs, 150);
    assert_eq!(app2.saved_jobs.retention_days, 14);
    drop(app2);

    // 5. External modification conflict detection & backup creation (.bak)
    let external_convert = format!("{saved_convert_text}\n# External editor change\n");
    fs::write(&convert_path, &external_convert).expect("simulate external edit on convert.toml");

    app.draft_convert.quick_convert_target = FormatId::Jpeg;
    let conflict_res = app.save_convert_and_jobs(false);
    assert!(
        conflict_res.is_err(),
        "external edit must trigger conflict detection when force_overwrite_with_backup == false"
    );

    app.save_convert_and_jobs(true)
        .expect("force_overwrite_with_backup == true must succeed and create .bak");
    let bak_path = convert_path.with_extension("toml.bak");
    assert!(bak_path.is_file(), "convert.toml.bak must be created");
    assert_eq!(
        fs::read_to_string(&bak_path).expect("read .bak"),
        external_convert
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ac_c4_5_advanced_convert_selection_handoff_zero_pixel_decode_validation_and_job_submission() {
    let dir = make_temp_test_dir("ac-c4-5");
    let paths = isolated_service_paths(&dir);

    assert_eq!(
        pixel_decode_count(),
        0,
        "pixel_decode_count must start at 0"
    );

    // 1. Test --stdin path reader with non-UTF-8 Linux filenames and security bounds
    let raw_non_utf8 = b"/tmp/dir/photo_\xff_raw.png\n/tmp/dir/second.png\n";
    let parsed_stdin =
        read_bounded_stdin_paths(Cursor::new(&raw_non_utf8[..])).expect("parse non-UTF-8 stdin");
    assert_eq!(parsed_stdin.len(), 2);
    assert_eq!(
        parsed_stdin[0]
            .to_path_buf()
            .unwrap()
            .as_os_str()
            .as_bytes(),
        b"/tmp/dir/photo_\xff_raw.png"
    );

    // Create actual file on disk with non-UTF-8 filename + standard PNGs
    let img_dir = dir.join("adv_images");
    fs::create_dir_all(&img_dir).expect("create adv_images");
    let non_utf8_name = OsString::from_vec(b"photo_\xff_alpha.png".to_vec());
    let non_utf8_path = img_dir.join(&non_utf8_name);
    write_valid_rgba_png(&non_utf8_path, 64, 48, 10);

    let std_png_path = img_dir.join("banner.png");
    write_valid_rgba_png(&std_png_path, 128, 96, 20);

    let unsupported_txt_path = img_dir.join("notes.txt");
    fs::write(&unsupported_txt_path, b"plain text notes").expect("write notes.txt");

    // 2. Test > 1,000 header probe cap (`inspect_selection_headers`)
    let mut large_selection = Vec::with_capacity(1_005);
    large_selection.push(WirePath::from_path(&non_utf8_path).expect("WirePath"));
    large_selection.push(WirePath::from_path(&std_png_path).expect("WirePath"));
    large_selection.push(WirePath::from_path(&unsupported_txt_path).expect("WirePath"));
    for _ in 3..1_005 {
        large_selection.push(WirePath::from_path(&std_png_path).expect("WirePath"));
    }
    let summary = inspect_selection_headers(&large_selection, &ResourceLimitsConfig::default());
    assert_eq!(summary.total_selected, 1_005);
    assert_eq!(summary.supported_count, 1_004);
    assert_eq!(summary.unsupported_items.len(), 1);
    assert!(summary.truncated_header_probe);
    assert_eq!(summary.min_dimensions, Some((64, 48)));
    assert_eq!(summary.max_dimensions, Some((128, 96)));
    assert!(summary.alpha_count >= 2);
    assert_eq!(
        pixel_decode_count(),
        0,
        "header inspection must NEVER decode image pixels"
    );

    // 3. Measure AdvancedConvertApp cold-start + first headless frame render (< 600 ms target)
    let selected_paths = vec![
        WirePath::from_path(&non_utf8_path).expect("WirePath"),
        WirePath::from_path(&std_png_path).expect("WirePath"),
    ];
    let ctx = eframe::egui::Context::default();
    let cold_start_t0 = Instant::now();
    let mut adv_app =
        AdvancedConvertApp::new(selected_paths.clone(), paths.clone(), Some(ctx.clone()));
    step_ui(&ctx, |ui| {
        adv_app.render_frame(ui);
    });
    let cold_start_ms = cold_start_t0.elapsed().as_secs_f64() * 1000.0;
    println!(
        "[C4-PERF] AdvancedConvertApp cold-start + first frame render: {:.3} ms (target < 600 ms)",
        cold_start_ms
    );
    assert!(
        cold_start_ms < 600.0,
        "AC-C4.5: AdvancedConvertApp cold-start ({cold_start_ms:.2} ms) must be < 600 ms"
    );

    // Wait for background header inspection to arrive
    let insp_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < insp_deadline {
        adv_app.drain_background_messages();
        if adv_app.inspection.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(adv_app.inspection.is_some());
    assert!(adv_app.can_submit());

    // 4. Verify live validation disables Convert when settings are invalid
    adv_app.resize_mode = UiResizeMode::Width;
    adv_app.resize_width_text = "0".to_string();
    adv_app.recompute_validation();
    assert!(!adv_app.can_submit());
    assert!(!adv_app.validation_errors.is_empty());

    // Restore valid width (32 px) and configure same-format PNG re-encoding with suffix "-reenc"
    adv_app.resize_width_text = "32".to_string();
    adv_app.select_target_format(FormatId::Png);
    adv_app.resize_mode = UiResizeMode::Width;
    adv_app.resize_width_text = "32".to_string();
    adv_app.append_suffix = true;
    adv_app.suffix_text = "-reenc".to_string();
    adv_app.dest_mode = UiDestMode::SameFolder;
    adv_app.recompute_validation();
    assert!(adv_app.can_submit());

    // 5. Save custom user preset to convert.toml
    adv_app.save_preset_name = "Thumb 32px PNG".to_string();
    let preset_id = adv_app
        .save_current_as_user_preset()
        .expect("save user preset");
    assert_eq!(preset_id, "user.thumb-32px-png");
    let config_path = paths.config_path.clone().unwrap();
    let (reloaded_convert, _) = ConvertSettings::load_or_default(&convert_path_for(&config_path));
    assert!(
        reloaded_convert
            .user_presets
            .iter()
            .any(|p| p.id == "user.thumb-32px-png")
    );

    // 6. Verify submission failure when worker socket is invalid keeps window open with error banner
    let broken_sock = dir.join(format!("invalid_sun_len_{}.sock", "x".repeat(120)));
    let err_res = adv_app.submit_blocking(&broken_sock);
    assert!(err_res.is_err());
    assert!(adv_app.submit_error.is_some());
    assert!(
        !adv_app.should_close,
        "window must remain open on submission error"
    );

    // 7. Start isolated worker and submit durable job with Origin::Advanced
    let worker = TestWorkerGuard::spawn(&dir, &paths, None);
    adv_app.trigger_submit();
    let sub_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < sub_deadline {
        adv_app.drain_background_messages();
        if adv_app.submitted_job_id.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }
    let job_id = adv_app
        .submitted_job_id
        .expect("job must be submitted to worker");
    assert!(adv_app.submit_error.is_none());

    // Wait for worker to finish the job and verify Origin::Advanced & resized output
    let done_deadline = Instant::now() + Duration::from_secs(5);
    let mut final_sum = None;
    while Instant::now() < done_deadline {
        let mut c = JobsClient::connect(&worker.socket_path).expect("connect worker");
        let (sum, _, _) = c.get_job(job_id, Some(0), Some(10)).expect("get_job");
        if sum.state.is_terminal() {
            final_sum = Some(sum);
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let sum = final_sum.expect("Advanced Convert job must finish");
    assert_eq!(sum.origin, Origin::Advanced);
    assert_eq!(sum.state, JobState::Completed);
    assert_eq!(sum.counts.succeeded_items, 2);

    let reenc_std = img_dir.join("banner-reenc.png");
    assert!(
        reenc_std.is_file(),
        "same-format re-encoded banner-reenc.png must be created"
    );
    let hdr = radivine_convert::inspect_file_header(&reenc_std, &ResourceLimitsConfig::default())
        .expect("probe resized output header");
    assert_eq!(hdr.width, 32);
    assert_eq!(hdr.height, 24);

    // 8. Verify multiple AdvancedConvertApp instances coexist with ControlInstanceLock held (ADR-C15)
    let control_sock = dir.join("runtime").join("control.sock");
    let lock = ControlInstanceLock::acquire_or_notify_existing(&control_sock, None)
        .expect("acquire control lock")
        .expect("own lock");
    let _main_app = ControlCenterApp::new(
        ControlPage::Dashboard,
        paths.clone(),
        Some(lock),
        None,
        false,
    );
    let mut adv_app_2 = AdvancedConvertApp::new(selected_paths, paths, None);
    step_ui(&ctx, |ui| {
        adv_app_2.render_frame(ui);
    });

    assert_eq!(
        pixel_decode_count(),
        0,
        "AC-C4.5: pixel_decode_count must remain 0 across the entire Advanced Convert lifecycle"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ac_c4_6_worker_unavailable_empty_history_and_newer_schema_readonly_states() {
    let dir = make_temp_test_dir("ac-c4-6");
    let paths = isolated_service_paths(&dir);
    let ctx = eframe::egui::Context::default();
    let mut rasterizer = EguiSoftRasterizer::new();

    // 1. Worker-unavailable state (no worker process, no jobs.sqlite3)
    let mut app_offline = ControlCenterApp::new(
        ControlPage::Jobs,
        paths.clone(),
        None,
        Some(ctx.clone()),
        false,
    );
    let offline_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < offline_deadline {
        app_offline.drain_all_events();
        if app_offline.jobs_view.worker_error.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!app_offline.jobs_view.worker_online);
    let offline_png = dir.join("jobs-worker-unavailable.png");
    rasterizer
        .render_to_png(
            &ctx,
            eframe::egui::Vec2::new(960.0, 620.0),
            1.0,
            &offline_png,
            |ui| {
                app_offline.render_frame(ui);
            },
        )
        .expect("render worker-unavailable state");
    assert!(offline_png.is_file());
    drop(app_offline);

    // 2. Empty history state (worker online, 0 jobs)
    let mut worker = TestWorkerGuard::spawn(&dir, &paths, None);
    let mut app_empty = ControlCenterApp::new(
        ControlPage::Jobs,
        paths.clone(),
        None,
        Some(ctx.clone()),
        false,
    );
    let empty_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < empty_deadline {
        app_empty.drain_all_events();
        if app_empty.jobs_view.worker_online {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(app_empty.jobs_view.worker_online);
    assert!(app_empty.jobs_view.jobs.is_empty());
    assert!(!app_empty.jobs_view.newer_schema_readonly);
    let empty_png = dir.join("jobs-empty-history.png");
    rasterizer
        .render_to_png(
            &ctx,
            eframe::egui::Vec2::new(960.0, 620.0),
            1.0,
            &empty_png,
            |ui| {
                app_empty.render_frame(ui);
            },
        )
        .expect("render empty-history state");
    assert!(empty_png.is_file());
    drop(app_empty);

    // 3. Newer DB schema read-only state (`schema_version = 99 > CURRENT_JOB_SCHEMA_VERSION`)
    assert!(
        worker.db_path.is_file(),
        "jobs.sqlite3 must exist on disk at {}",
        worker.db_path.display()
    );
    worker.stop();
    let mut app_ro = ControlCenterApp::new(
        ControlPage::Jobs,
        paths.clone(),
        None,
        Some(ctx.clone()),
        false,
    );
    app_ro.jobs_view.apply_event(UiJobsEvent::SnapshotLoaded {
        worker_online: true,
        worker_pid: Some(9999),
        schema_version: 99,
        jobs: Vec::new(),
        error: None,
    });
    assert_eq!(app_ro.jobs_view.schema_version, 99);
    assert!(app_ro.jobs_view.newer_schema_readonly);

    let ro_png = dir.join("jobs-newer-schema-readonly.png");
    rasterizer
        .render_to_png(
            &ctx,
            eframe::egui::Vec2::new(960.0, 620.0),
            1.0,
            &ro_png,
            |ui| {
                app_ro.render_frame(ui);
            },
        )
        .expect("render newer-schema read-only state");
    assert!(ro_png.is_file());
    drop(app_ro);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ac_c4_7_screenshots_export_all_pages_and_advanced_convert_themes() {
    let dir = make_temp_test_dir("ac-c4-7");
    let paths = isolated_service_paths(&dir);
    let out_dir = dir.join("screenshots");

    let exported = export_all_page_screenshots(&out_dir, paths.clone())
        .expect("export all Control Center page screenshots");
    assert_eq!(exported.len(), 10);

    for required in [
        "control-conversion-dark.png",
        "control-conversion-light.png",
        "control-jobs-dark.png",
        "control-jobs-light.png",
    ] {
        let p = out_dir.join(required);
        assert!(p.is_file(), "expected screenshot {required} to exist");
        let bytes = fs::read(&p).expect("read screenshot");
        assert!(
            bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "{required} must have valid PNG signature"
        );
        assert!(
            bytes.len() > 10_000,
            "{required} must be non-trivial ({} bytes)",
            bytes.len()
        );
    }

    // Also render standalone Advanced Convert window in Dark and Light themes
    let img_path = dir.join("sample.png");
    write_valid_rgba_png(&img_path, 256, 192, 7);
    let ctx = eframe::egui::Context::default();
    let mut rasterizer = EguiSoftRasterizer::new();

    for (theme_name, pal) in [
        ("dark", BrandPalette::dark()),
        ("light", BrandPalette::light()),
    ] {
        let mut adv = AdvancedConvertApp::new(
            vec![WirePath::from_path(&img_path).expect("WirePath")],
            paths.clone(),
            Some(ctx.clone()),
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            adv.drain_background_messages();
            if adv.inspection.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let target_png = out_dir.join(format!("advanced-convert-{theme_name}.png"));
        rasterizer
            .render_to_png(
                &ctx,
                eframe::egui::Vec2::new(640.0, 720.0),
                1.0,
                &target_png,
                |ui| {
                    radivine_control::ui::apply_palette_visuals(
                        ui.ctx(),
                        theme_name == "dark",
                        &pal,
                    );
                    adv.render_frame(ui);
                },
            )
            .expect("render AdvancedConvertApp PNG");
        let bytes = fs::read(&target_png).expect("read AdvancedConvertApp PNG");
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(bytes.len() > 10_000);
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn c4_bugfix_advanced_convert_spaces_unicode_arabic_literal_percent_and_uris() {
    let dir = make_temp_test_dir("c4-bugfix-paths");
    let paths = isolated_service_paths(&dir);

    // 1. Create parent directories containing spaces and Arabic/Unicode characters
    let space_parent = dir.join("My Game Captures 2026");
    let arabic_parent = space_parent.join("الصور المفضلة");
    fs::create_dir_all(&arabic_parent).expect("create directories with spaces and Arabic");

    // 2. Create test images covering:
    //    - Filename with spaces ("DAYS GONE.png", "Red Dead Redemption 2.png")
    //    - Parent directory with spaces ("My Game Captures 2026")
    //    - Unicode & Arabic filename ("لقطة شاشة ١ — تجربة.png")
    //    - Literal "%20", "%", "#", and apostrophes ("literal%20in%name 100% #1 'final'.png")
    let file_days_gone = space_parent.join("DAYS GONE.png");
    let file_rdr2 = space_parent.join("Red Dead Redemption 2.png");
    let file_arabic = arabic_parent.join("لقطة شاشة ١ — تجربة.png");
    let file_literal_specials = space_parent.join("literal%20in%name 100% #1 'final'.png");

    write_valid_rgba_png(&file_days_gone, 64, 48, 1);
    write_valid_rgba_png(&file_rdr2, 80, 60, 2);
    write_valid_rgba_png(&file_arabic, 96, 72, 3);
    write_valid_rgba_png(&file_literal_specials, 112, 84, 4);

    let source_files = [
        &file_days_gone,
        &file_rdr2,
        &file_arabic,
        &file_literal_specials,
    ];

    // 3. Test complete file-manager -> wheel -> AdvancedConvertAction -> --stdin handoff:
    //    Nemo produces `file://` URIs -> `BasicContext::from_uris` decodes them to `PathBuf` ->
    //    `encode_advanced_convert_stdin` writes `wire:<WirePath>` lines ->
    //    `read_bounded_stdin_paths` parses them back into `WirePath` without double-encoding.
    let file_uris: Vec<String> = source_files
        .iter()
        .map(|p| radivine_core::local_path_to_file_uri(p))
        .collect();
    // Verify Nemo-style URI encoding has `%20` for spaces and `%2520` for literal `%20`
    assert!(
        file_uris[0].ends_with("/My%20Game%20Captures%202026/DAYS%20GONE.png"),
        "expected URI with %20, got {}",
        file_uris[0]
    );
    assert!(
        file_uris[3].contains("literal%2520in%25name%20100%25%20%231%20'final'.png")
            || file_uris[3].contains("literal%2520in%25name%20100%25%20%231%20%27final%27.png"),
        "expected literal %20 to be encoded as %2520 in file:// URI, got {}",
        file_uris[3]
    );

    let basic_ctx = radivine_core::BasicContext::from_uris(
        &file_uris,
        radivine_core::InvocationMode::Invoke,
        radivine_core::SourceInfo::default(),
    );
    assert_eq!(basic_ctx.items.len(), 4);
    for (item, expected_path) in basic_ctx.items.iter().zip(source_files.iter()) {
        assert_eq!(
            item.local_path.as_deref(),
            Some(expected_path.as_path()),
            "BasicContext::from_uris must decode file:// URI to exact local PathBuf"
        );
    }

    let (stdin_payload, encoded_count) =
        radivine_actions::encode_advanced_convert_stdin(&basic_ctx)
            .expect("encode_advanced_convert_stdin");
    assert_eq!(encoded_count, 4);
    assert!(
        stdin_payload.lines().all(|line| line.starts_with("wire:/")),
        "every line from encode_advanced_convert_stdin must carry explicit 'wire:' prefix"
    );

    let handoff_wire_paths = read_bounded_stdin_paths(Cursor::new(stdin_payload.as_bytes()))
        .expect("read_bounded_stdin_paths from wheel handoff");
    assert_eq!(handoff_wire_paths.len(), 4);
    for (wp, expected_path) in handoff_wire_paths.iter().zip(source_files.iter()) {
        let decoded = wp.to_path_buf().expect("to_path_buf");
        assert_eq!(
            &decoded, *expected_path,
            "wheel -> --stdin handoff must preserve exact filesystem path without %20 corruption"
        );
        assert!(
            decoded.is_file(),
            "decoded path must exist on disk: {}",
            decoded.display()
        );
    }

    // 4. Test direct CLI arguments and raw --stdin inputs mixing:
    //    - Ordinary filesystem paths (with spaces, Arabic, literal `%20`, `%`, `#`, `'`)
    //    - `file:///...` URIs
    //    - `file://localhost/...` URIs
    let localhost_uri = file_uris[1].replacen("file:///", "file://localhost/", 1);
    let cli_args = vec![
        file_days_gone.as_os_str().to_os_string(), // ordinary filesystem path with spaces
        OsString::from(&localhost_uri),            // file://localhost/... URI
        OsString::from(&file_uris[2]),             // file:///... URI with Arabic
        file_literal_specials.as_os_str().to_os_string(), // ordinary filesystem path with literal `%20`, `%`, `#`, `'`
    ];
    let cli_res = radivine_control::ui::advanced_convert::parse_convert_cli_args(&cli_args)
        .expect("parse mixed CLI args");
    assert!(!cli_res.read_stdin);
    assert_eq!(cli_res.paths.len(), 4);
    for (wp, expected_path) in cli_res.paths.iter().zip(source_files.iter()) {
        assert_eq!(&wp.to_path_buf().unwrap(), *expected_path);
    }

    // Also test raw --stdin mixing ordinary filesystem paths and `file://` URIs
    let mixed_raw_stdin = format!(
        "{}\n{}\n{}\n{}\n",
        file_days_gone.display(),
        localhost_uri,
        file_uris[2],
        file_literal_specials.display()
    );
    let parsed_mixed_stdin = read_bounded_stdin_paths(Cursor::new(mixed_raw_stdin.as_bytes()))
        .expect("parse mixed raw stdin");
    assert_eq!(parsed_mixed_stdin.len(), 4);
    for (wp, expected_path) in parsed_mixed_stdin.iter().zip(source_files.iter()) {
        assert_eq!(&wp.to_path_buf().unwrap(), *expected_path);
    }

    // 5. Verify Advanced Convert selection summary (`inspect_selection_headers` & `AdvancedConvertApp`)
    let summary = inspect_selection_headers(&handoff_wire_paths, &ResourceLimitsConfig::default());
    assert_eq!(summary.total_selected, 4);
    assert_eq!(
        summary.supported_count, 4,
        "all 4 images (including spaces, Arabic, and literal %20) must be recognized as supported"
    );
    assert!(
        summary.unsupported_items.is_empty(),
        "expected 0 unsupported items, got: {:?}",
        summary.unsupported_items
    );

    let ctx = eframe::egui::Context::default();
    let mut adv_app = AdvancedConvertApp::new(handoff_wire_paths.clone(), paths.clone(), Some(ctx));
    let insp_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < insp_deadline {
        adv_app.drain_background_messages();
        if adv_app.inspection.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let app_insp = adv_app
        .inspection
        .as_ref()
        .expect("AdvancedConvertApp background inspection must complete");
    assert_eq!(app_insp.supported_count, 4);
    assert!(app_insp.unsupported_items.is_empty());
    assert!(adv_app.can_submit());

    // 6. Execute actual conversion job to WebP via radivine-worker and verify every output file
    let worker = TestWorkerGuard::spawn(&dir, &paths, None);
    adv_app.select_target_format(FormatId::WebP);
    adv_app.dest_mode = UiDestMode::SameFolder;
    adv_app.append_suffix = false;
    adv_app.recompute_validation();
    assert!(adv_app.can_submit());

    adv_app.trigger_submit();
    let sub_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < sub_deadline {
        adv_app.drain_background_messages();
        if adv_app.submitted_job_id.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }
    let job_id = adv_app
        .submitted_job_id
        .expect("Advanced Convert job must submit cleanly");
    assert!(adv_app.submit_error.is_none());

    let done_deadline = Instant::now() + Duration::from_secs(5);
    let mut final_sum = None;
    let mut final_items = Vec::new();
    while Instant::now() < done_deadline {
        let mut c = JobsClient::connect(&worker.socket_path).expect("connect worker");
        let (sum, items, _) = c.get_job(job_id, Some(0), Some(10)).expect("get_job");
        if sum.state.is_terminal() {
            final_sum = Some(sum);
            final_items = items;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let sum = final_sum.expect("conversion job must reach terminal state");
    assert_eq!(
        sum.state,
        JobState::Completed,
        "all 4 items must convert without os error 2; items: {final_items:?}"
    );
    assert_eq!(sum.counts.succeeded_items, 4);
    assert_eq!(sum.counts.failed_items, 0);

    // Verify all 4 converted .webp files exist at their exact uncorrupted paths
    let expected_outputs = [
        space_parent.join("DAYS GONE.webp"),
        space_parent.join("Red Dead Redemption 2.webp"),
        arabic_parent.join("لقطة شاشة ١ — تجربة.webp"),
        space_parent.join("literal%20in%name 100% #1 'final'.webp"),
    ];
    for out_path in &expected_outputs {
        assert!(
            out_path.is_file(),
            "expected converted WebP output file at {}",
            out_path.display()
        );
        let hdr = radivine_convert::inspect_file_header(out_path, &ResourceLimitsConfig::default())
            .expect("inspect converted WebP header");
        assert_eq!(hdr.format, FormatId::WebP);
    }

    let _ = fs::remove_dir_all(&dir);
}
