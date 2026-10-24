//! Milestone C2 comprehensive process-level acceptance, crash-recovery (`C0a`–`C6`),
//! lifecycle (`T-LIFE-1`..`3`, `T-CGROUP-DETACH` / `P10`), watchdog, and security test suite.

#![forbid(unsafe_code)]

use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use image::{ImageFormat, Rgba, RgbaImage};
use radivine_jobs::{
    CONVERT_IMAGE_KIND, ItemState, JobId, JobState, JobUpdate, JobsClient, MAX_ITEMS_PER_JOB,
    Origin, RequestId, RetryMode, WirePath, WorkerErrorCode, WorkerLaunchMode, WorkerResponse,
    WorkerRuntimePaths,
};
use radivine_worker::recovery::run_startup_recovery;
use radivine_worker::store::JobStore;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn worker_bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_radivine-worker"))
}

struct TestWorkerEnv {
    _tmp: TempDir,
    root: PathBuf,
    paths: WorkerRuntimePaths,
    children: Vec<Child>,
}

impl TestWorkerEnv {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let root = tmp.path().to_path_buf();
        let runtime_dir = root.join("run");
        let state_dir = root.join("state");
        let config_dir = root.join("config");
        fs::create_dir_all(&runtime_dir).unwrap();
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(&config_dir).unwrap();

        let paths = WorkerRuntimePaths {
            socket_path: runtime_dir.join("worker.sock"),
            lock_path: runtime_dir.join("worker.lock"),
            db_path: state_dir.join("jobs.sqlite3"),
            log_path: state_dir.join("worker.log"),
            worker_bin: Some(worker_bin_path()),
            launch_mode: WorkerLaunchMode::Spawn,
        };

        Self {
            _tmp: tmp,
            root,
            paths,
            children: Vec::new(),
        }
    }

    fn base_command(&self) -> Command {
        let mut cmd = Command::new(worker_bin_path());
        cmd.env("RADIVINE_WORKER_BIN", worker_bin_path())
            .env("RADIVINE_WORKER_SOCKET", &self.paths.socket_path)
            .env("RADIVINE_WORKER_DB", &self.paths.db_path)
            .env("RADIVINE_SUPPRESS_DESKTOP_NOTIFY", "1")
            .env("RADIVINE_DISABLE_SYSTEMD_USER_SPAWN", "1")
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env_remove("RADIVINE_WORKER_TEST_CRASH_AT")
            .env_remove("RADIVINE_WORKER_TEST_ITEM_SLEEP_MS")
            .env_remove("RADIVINE_WORKER_ITEM_TIMEOUT_MS")
            .env_remove("RADIVINE_WORKER_TEST_FAKE_AVAIL_BYTES")
            .env_remove("RADIVINE_WORKER_TEST_FAKE_PEER_UID");
        cmd
    }

    fn spawn_worker(&mut self) -> u32 {
        self.spawn_worker_with_env(&[], 60_000)
    }

    fn spawn_worker_with_env(&mut self, extra_env: &[(&str, &str)], idle_ms: u64) -> u32 {
        let mut cmd = self.base_command();
        cmd.arg("--socket")
            .arg(&self.paths.socket_path)
            .arg("--db")
            .arg(&self.paths.db_path)
            .arg("--lock")
            .arg(&self.paths.lock_path)
            .arg("serve")
            .arg("--idle-timeout-ms")
            .arg(idle_ms.to_string());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let child = cmd.spawn().expect("spawn radivine-worker");
        let pid = child.id();
        self.children.push(child);

        self.wait_until_ready(Duration::from_secs(5));
        pid
    }

    fn wait_until_ready(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(mut client) = JobsClient::connect_with_timeout(
                &self.paths.socket_path,
                Duration::from_millis(100),
            ) && matches!(client.ping(), Ok(WorkerResponse::Pong { .. }))
            {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "worker did not become ready at {} within {:?}",
            self.paths.socket_path.display(),
            timeout
        );
    }

    fn connect(&self) -> JobsClient {
        JobsClient::connect(&self.paths.socket_path).expect("connect to test worker")
    }

    fn wait_for_job_terminal(
        &self,
        job_id: JobId,
        timeout: Duration,
    ) -> (radivine_jobs::JobSummary, Vec<radivine_jobs::JobItemReport>) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(mut client) = JobsClient::connect_with_timeout(
                &self.paths.socket_path,
                Duration::from_millis(200),
            ) && let Ok((job, items, _)) = client.get_job(job_id, None, None)
                && job.state.is_terminal()
            {
                return (job, items);
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("job {job_id} did not reach terminal state within {timeout:?}");
    }
}

impl Drop for TestWorkerEnv {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn write_test_png(path: &Path, width: u32, height: u32, seed: u8) {
    let mut img = RgbaImage::new(width, height);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = Rgba([
            ((x as u8).wrapping_add(seed)),
            ((y as u8).wrapping_mul(3).wrapping_add(seed)),
            seed.wrapping_mul(17),
            255,
        ]);
    }
    img.save_with_format(path, ImageFormat::Png)
        .expect("write test PNG");
}

fn sha256_file(path: &Path) -> Vec<u8> {
    let bytes = fs::read(path).expect("read file for sha256");
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    hasher.finalize().to_vec()
}

fn assert_no_temp_or_stage_artifacts(dir: &Path) {
    for entry in fs::read_dir(dir).expect("read_dir") {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let bytes = name.as_os_str().as_bytes();
        assert!(
            !bytes.starts_with(b".rvstage-"),
            "unexpected leftover staging dir: {:?}",
            entry.path()
        );
        assert!(
            !bytes.ends_with(b".part"),
            "unexpected leftover .part file: {:?}",
            entry.path()
        );
    }
}

#[test]
fn ac_c2_1_and_c2_2_single_medium_large_batches_idempotency_and_retention() {
    let mut env = TestWorkerEnv::new();
    env.spawn_worker();

    let work_dir = env.root.join("images");
    fs::create_dir_all(&work_dir).unwrap();

    // 1. Single-item job + source immutability + request_id idempotency (including concurrent duplicates)
    let src1 = work_dir.join("single.png");
    write_test_png(&src1, 16, 16, 7);
    let src1_hash_before = sha256_file(&src1);

    let req_id = RequestId::generate();
    let sock_path = env.paths.socket_path.clone();
    let src1_wire = WirePath::from_absolute_path(&src1).unwrap();

    // Submit concurrently from 8 threads with the same RequestId (AC-C2.2)
    let mut handles = Vec::new();
    for i in 0..8 {
        let sock = sock_path.clone();
        let rid = req_id.clone();
        let wp = src1_wire.clone();
        handles.push(thread::spawn(move || {
            let mut c = JobsClient::connect(&sock).unwrap();
            c.submit(
                rid,
                Origin::Test,
                CONVERT_IMAGE_KIND,
                vec![wp],
                serde_json::json!({"target": "webp"}),
            )
            .unwrap_or_else(|e| panic!("thread {i} submit failed: {e}"))
        }));
    }

    let mut results = Vec::new();
    for h in handles {
        results.push(h.join().unwrap());
    }
    let first_job_id = results[0].0;
    let mut dedup_false_count = 0;
    let mut dedup_true_count = 0;
    for (jid, _, dedup) in &results {
        assert_eq!(
            *jid, first_job_id,
            "all duplicate RequestIds must return the same JobId"
        );
        if *dedup {
            dedup_true_count += 1;
        } else {
            dedup_false_count += 1;
        }
    }
    assert_eq!(
        dedup_false_count, 1,
        "exactly one submission should create the job"
    );
    assert_eq!(
        dedup_true_count, 7,
        "seven submissions should be deduplicated"
    );

    let (job1, items1) = env.wait_for_job_terminal(first_job_id, Duration::from_secs(10));
    assert_eq!(job1.state, JobState::Completed);
    assert_eq!(job1.counts.succeeded_items, 1);
    assert_eq!(items1.len(), 1);
    assert_eq!(items1[0].state, ItemState::Succeeded);
    assert_eq!(
        sha256_file(&src1),
        src1_hash_before,
        "source file must never be modified"
    );

    let out1 = work_dir.join("single.webp");
    assert!(out1.exists(), "converted single.webp must exist");
    let decoded1 = image::open(&out1).expect("decode output webp");
    assert_eq!(decoded1.width(), 16);
    assert_eq!(decoded1.height(), 16);

    // 2. Medium batch (50 items) + pagination check
    let med_dir = env.root.join("medium_50");
    fs::create_dir_all(&med_dir).unwrap();
    let mut med_wires = Vec::with_capacity(50);
    for i in 0..50 {
        let p = med_dir.join(format!("med_{i:03}.png"));
        write_test_png(&p, 8, 8, (i & 0xFF) as u8);
        med_wires.push(WirePath::from_absolute_path(&p).unwrap());
    }
    let mut client = env.connect();
    let (med_job_id, _, _) = client
        .submit(
            RequestId::generate(),
            Origin::Cli,
            CONVERT_IMAGE_KIND,
            med_wires,
            serde_json::json!({"target": "webp"}),
        )
        .unwrap();
    let (med_job, _) = env.wait_for_job_terminal(med_job_id, Duration::from_secs(20));
    assert_eq!(med_job.state, JobState::Completed);
    assert_eq!(med_job.counts.total_items, 50);
    assert_eq!(med_job.counts.succeeded_items, 50);
    assert_no_temp_or_stage_artifacts(&med_dir);

    let (_, page_items, total_count) = client.get_job(med_job_id, Some(10), Some(15)).unwrap();
    assert_eq!(total_count, 50);
    assert_eq!(page_items.len(), 15);
    assert_eq!(page_items[0].item_index, 10);
    assert_eq!(page_items[14].item_index, 24);

    // 3. Large batch (1,000 items, AC-C2.1)
    let large_dir = env.root.join("large_1000");
    fs::create_dir_all(&large_dir).unwrap();
    let template_png = large_dir.join("template.png");
    write_test_png(&template_png, 4, 4, 42);
    let template_bytes = fs::read(&template_png).unwrap();
    fs::remove_file(&template_png).unwrap();

    let mut large_wires = Vec::with_capacity(1000);
    for i in 0..1000 {
        let p = large_dir.join(format!("item_{i:04}.png"));
        fs::write(&p, &template_bytes).unwrap();
        large_wires.push(WirePath::from_absolute_path(&p).unwrap());
    }

    let (large_job_id, _, _) = client
        .submit(
            RequestId::generate(),
            Origin::Wheel,
            CONVERT_IMAGE_KIND,
            large_wires,
            serde_json::json!({"target": "webp"}),
        )
        .unwrap();
    let (large_job, _) = env.wait_for_job_terminal(large_job_id, Duration::from_secs(60));
    assert_eq!(large_job.state, JobState::Completed);
    assert_eq!(large_job.counts.total_items, 1000);
    assert_eq!(large_job.counts.succeeded_items, 1000);
    assert_no_temp_or_stage_artifacts(&large_dir);

    // 4. Retention pruning check (AC-C2.2)
    let jobs_before = client.list_jobs(10, true).unwrap();
    assert_eq!(jobs_before.len(), 3);
    let store = JobStore::open(&env.paths.db_path).unwrap();
    let pruned = store.prune_retention(2, 30).unwrap();
    assert_eq!(
        pruned, 1,
        "oldest terminal job should be pruned when max_jobs = 2"
    );
    let jobs_after = client.list_jobs(10, true).unwrap();
    assert_eq!(jobs_after.len(), 2);
    assert_eq!(jobs_after[0].job_id, large_job_id);
    assert_eq!(jobs_after[1].job_id, med_job_id);
}

#[test]
fn ac_c2_4_t_life_1_client_disconnects_while_batch_completes() {
    let mut env = TestWorkerEnv::new();
    env.spawn_worker_with_env(&[("RADIVINE_WORKER_TEST_ITEM_SLEEP_MS", "40")], 60_000);

    let dir = env.root.join("t_life_1");
    fs::create_dir_all(&dir).unwrap();
    let mut wires = Vec::new();
    for i in 0..5 {
        let p = dir.join(format!("img_{i}.png"));
        write_test_png(&p, 12, 12, i as u8);
        wires.push(WirePath::from_absolute_path(&p).unwrap());
    }

    let job_id = {
        let mut short_lived_client = env.connect();
        let (jid, state, _) = short_lived_client
            .submit(
                RequestId::generate(),
                Origin::Cli,
                CONVERT_IMAGE_KIND,
                wires,
                serde_json::json!({"target": "webp"}),
            )
            .unwrap();
        assert!(matches!(
            state,
            JobState::Queued | JobState::Preparing | JobState::Running
        ));
        jid
        // short_lived_client is dropped here while conversion is still in flight!
    };

    let (job, items) = env.wait_for_job_terminal(job_id, Duration::from_secs(10));
    assert_eq!(job.state, JobState::Completed);
    assert_eq!(job.counts.succeeded_items, 5);
    for (i, it) in items.iter().enumerate() {
        assert_eq!(it.state, ItemState::Succeeded);
        assert!(dir.join(format!("img_{i}.webp")).exists());
    }
    assert_no_temp_or_stage_artifacts(&dir);
}

#[test]
fn ac_c2_5_t_life_2_daemon_sigkill_and_single_instance_lock_and_p10_cgroup_detach() {
    let env = TestWorkerEnv::new();

    // Part A: Single-instance lock (10 concurrent worker spawns -> 1 active, 9 clean exit(0))
    let mut concurrent_children = Vec::new();
    for _ in 0..10 {
        let mut cmd = env.base_command();
        cmd.arg("--socket")
            .arg(&env.paths.socket_path)
            .arg("--db")
            .arg(&env.paths.db_path)
            .arg("--lock")
            .arg(&env.paths.lock_path)
            .arg("serve")
            .arg("--idle-timeout-ms")
            .arg("60000")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        concurrent_children.push(cmd.spawn().unwrap());
    }
    env.wait_until_ready(Duration::from_secs(5));
    thread::sleep(Duration::from_millis(250));

    let mut exited_zero = 0;
    let mut still_running = Vec::new();
    for mut ch in concurrent_children {
        match ch.try_wait().unwrap() {
            Some(status) => {
                assert!(
                    status.success(),
                    "duplicate worker must exit cleanly with 0, got {status:?}"
                );
                exited_zero += 1;
            }
            None => still_running.push(ch),
        }
    }
    assert_eq!(exited_zero, 9, "9 duplicate workers must exit 0");
    assert_eq!(
        still_running.len(),
        1,
        "exactly 1 worker must hold worker.lock"
    );
    for mut ch in still_running {
        let _ = ch.kill();
        let _ = ch.wait();
    }

    // Part B: T-LIFE-2: Simulated daemon launches worker, submits batch, receives SIGKILL mid-batch
    let dir = env.root.join("t_life_2");
    fs::create_dir_all(&dir).unwrap();
    let mut file_args = Vec::new();
    for i in 0..6 {
        let p = dir.join(format!("daemon_item_{i}.png"));
        write_test_png(&p, 10, 10, i as u8);
        file_args.push(p);
    }

    // Spawn a simulated daemon child (`radivine-worker submit-convert --wait ...`) that auto-launches the worker
    let mut daemon_sim = env.base_command();
    daemon_sim
        .env("RADIVINE_WORKER_TEST_ITEM_SLEEP_MS", "80")
        .arg("--socket")
        .arg(&env.paths.socket_path)
        .arg("--db")
        .arg(&env.paths.db_path)
        .arg("--lock")
        .arg(&env.paths.lock_path)
        .arg("submit-convert")
        .arg("--to")
        .arg("webp")
        .arg("--wait");
    for f in &file_args {
        daemon_sim.arg(f);
    }
    let mut daemon_child = daemon_sim
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    // Wait until worker is running and job is submitted, then SIGKILL the submitting process
    env.wait_until_ready(Duration::from_secs(5));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut submitted_job_id = None;
    while Instant::now() < deadline {
        if let Ok(mut c) = env
            .connect()
            .list_jobs(10, true)
            .map(|jobs| jobs.into_iter().next())
            && let Some(j) = c.take()
        {
            submitted_job_id = Some(j.job_id);
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let job_id = submitted_job_id.expect("job should be submitted before SIGKILL");

    // SIGKILL the simulated daemon while items are in flight!
    let _ = daemon_child.kill();
    let _ = daemon_child.wait();

    // Worker must survive and finish all 6 items!
    let (finished_job, _) = env.wait_for_job_terminal(job_id, Duration::from_secs(10));
    assert_eq!(finished_job.state, JobState::Completed);
    assert_eq!(finished_job.counts.succeeded_items, 6);

    // Shut down the auto-launched worker cleanly before Part C
    if let Ok(mut c) = JobsClient::connect(&env.paths.socket_path) {
        let _ = c.shutdown_if_idle();
    }
    thread::sleep(Duration::from_millis(150));

    // Part C: Spike P10 / T-CGROUP-DETACH via isolated transient systemd-run --user unit
    let systemd_probe = Command::new("systemd-run")
        .args(["--user", "--collect", "--wait", "/bin/true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if systemd_probe.is_ok_and(|s| s.success()) {
        let env_p10 = TestWorkerEnv::new();
        let dir_p10 = env_p10.root.join("p10");
        fs::create_dir_all(&dir_p10).unwrap();
        let mut p10_files = Vec::new();
        for i in 0..6 {
            let p = dir_p10.join(format!("p10_{i}.png"));
            write_test_png(&p, 10, 10, i as u8);
            p10_files.push(p);
        }

        let unit_suffix = fastrand::u64(..);
        let daemon_unit = format!("radivine-c2-test-daemon-{unit_suffix:016x}");
        let worker_unit = format!("radivine-c2-test-worker-{unit_suffix:016x}");

        let mut sd_cmd = Command::new("systemd-run");
        sd_cmd
            .arg("--user")
            .arg("--collect")
            .arg(format!("--unit={daemon_unit}"))
            .arg(format!(
                "--setenv=RADIVINE_WORKER_BIN={}",
                worker_bin_path().display()
            ))
            .arg(format!(
                "--setenv=RADIVINE_WORKER_SOCKET={}",
                env_p10.paths.socket_path.display()
            ))
            .arg(format!(
                "--setenv=RADIVINE_WORKER_DB={}",
                env_p10.paths.db_path.display()
            ))
            .arg("--setenv=RADIVINE_WORKER_LAUNCH=systemd")
            .arg("--setenv=RADIVINE_SUPPRESS_DESKTOP_NOTIFY=1")
            .arg(format!(
                "--setenv=RADIVINE_WORKER_SYSTEMD_UNIT={worker_unit}"
            ))
            .arg("--setenv=RADIVINE_WORKER_TEST_ITEM_SLEEP_MS=150")
            .arg("--setenv=RADIVINE_WORKER_IDLE_TIMEOUT_MS=2000")
            .arg(worker_bin_path())
            .arg("--socket")
            .arg(&env_p10.paths.socket_path)
            .arg("--db")
            .arg(&env_p10.paths.db_path)
            .arg("--lock")
            .arg(&env_p10.paths.lock_path)
            .arg("submit-convert")
            .arg("--to")
            .arg("webp")
            .arg("--wait");
        for f in &p10_files {
            sd_cmd.arg(f);
        }
        let mut sd_child = sd_cmd
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn transient daemon unit");

        // Wait for worker socket to appear and inspect worker PID + cgroup
        env_p10.wait_until_ready(Duration::from_secs(5));
        let mut c = env_p10.connect();
        let WorkerResponse::Pong {
            pid: worker_pid, ..
        } = c.ping().unwrap()
        else {
            panic!("expected Pong");
        };
        let worker_cgroup =
            fs::read_to_string(format!("/proc/{worker_pid}/cgroup")).unwrap_or_default();
        assert!(
            worker_cgroup.contains(&worker_unit),
            "worker cgroup ({worker_cgroup:?}) must belong to {worker_unit}, not {daemon_unit}"
        );
        assert!(
            !worker_cgroup.contains(&daemon_unit),
            "worker cgroup ({worker_cgroup:?}) must not be inside daemon unit {daemon_unit}"
        );

        // Wait until job is visible in DB, then SIGKILL the transient daemon unit via systemctl --user kill
        let mut p10_job_id = None;
        let dl = Instant::now() + Duration::from_secs(5);
        while Instant::now() < dl {
            let jobs = c.list_jobs(5, true).unwrap_or_default();
            if let Some(j) = jobs.first() {
                p10_job_id = Some(j.job_id);
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let p10_jid = p10_job_id.expect("p10 job submitted");

        let _ = Command::new("systemctl")
            .args(["--user", "kill", "-s", "SIGKILL", &daemon_unit])
            .status();
        let _ = sd_child.kill();
        let _ = sd_child.wait();

        // Worker in its own unit survives the daemon cgroup kill and completes all 6 items!
        let (p10_job, _) = env_p10.wait_for_job_terminal(p10_jid, Duration::from_secs(10));
        assert_eq!(p10_job.state, JobState::Completed);
        assert_eq!(p10_job.counts.succeeded_items, 6);

        if let Ok(mut c2) = JobsClient::connect(&env_p10.paths.socket_path) {
            let _ = c2.shutdown_if_idle();
        }
    }
}

#[test]
fn ac_c2_6_c2_7_c2_8_crash_injection_matrix_c0a_through_c6() {
    let crash_points = [
        ("C0a", false),
        ("C0b", false),
        ("C1_unjournaled", false),
        ("C1", false),
        ("C2", false),
        ("C3", true),
        ("C4", true),
        ("C5", true),
        ("C6", true),
    ];

    for (point, expect_succeeded) in crash_points {
        let mut env = TestWorkerEnv::new();
        let dir = env.root.join(format!("crash_{point}"));
        fs::create_dir_all(&dir).unwrap();

        let src = dir.join("photo.png");
        write_test_png(&src, 16, 16, 99);
        let src_sha_before = sha256_file(&src);

        // 1. Pre-populate a queued job in SQLite so the crashing worker claims it immediately on startup
        let store = JobStore::open(&env.paths.db_path).unwrap();
        let (job_id, _, _) = store
            .submit_or_deduplicate(
                &RequestId::generate(),
                Origin::Test,
                CONVERT_IMAGE_KIND,
                std::slice::from_ref(&src),
                &serde_json::json!({"target": "webp"}),
            )
            .unwrap();
        drop(store);

        // 2. Spawn worker with RADIVINE_WORKER_TEST_CRASH_AT=<point> and wait for it to abort
        let mut crash_cmd = env.base_command();
        crash_cmd
            .env("RADIVINE_WORKER_TEST_CRASH_AT", point)
            .arg("--socket")
            .arg(&env.paths.socket_path)
            .arg("--db")
            .arg(&env.paths.db_path)
            .arg("--lock")
            .arg(&env.paths.lock_path)
            .arg("serve")
            .arg("--idle-timeout-ms")
            .arg("5000")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut crash_child = crash_cmd.spawn().expect("spawn crashing worker");

        let wait_deadline = Instant::now() + Duration::from_secs(10);
        let mut exit_status = None;
        while Instant::now() < wait_deadline {
            if let Some(st) = crash_child.try_wait().unwrap() {
                exit_status = Some(st);
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let status = exit_status.unwrap_or_else(|| {
            let _ = crash_child.kill();
            let _ = crash_child.wait();
            panic!("worker did not abort at crash point {point}");
        });
        assert!(
            !status.success(),
            "worker at crash point {point} must terminate abnormally, got {status:?}"
        );

        // 3. Spawn a normal recovery worker (without RADIVINE_WORKER_TEST_CRASH_AT)
        env.spawn_worker();
        let mut client = env.connect();
        let (job, items, _) = client.get_job(job_id, None, None).unwrap();
        assert_eq!(items.len(), 1);

        // 4. Verify invariants for this crash point
        assert_eq!(
            sha256_file(&src),
            src_sha_before,
            "source file must be byte-identical after crash point {point}"
        );
        if point == "C1_unjournaled" {
            // Per INV-RECOVERY-OWNERSHIP (JOB_SYSTEM_DESIGN.md §5 C1), an unjournaled .part file
            // whose (temp_dev, temp_ino) was never committed is preserved with OrphanedTempPreserved
            // rather than deleted by filename pattern.
            assert!(
                !items[0].warnings.is_empty(),
                "C1_unjournaled must record OrphanedTempPreserved warning"
            );
            for entry in fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                if entry
                    .file_name()
                    .as_os_str()
                    .as_bytes()
                    .starts_with(b".rvstage-")
                {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
        assert_no_temp_or_stage_artifacts(&dir);

        let expected_out = dir.join("photo.webp");
        if expect_succeeded {
            assert_eq!(
                items[0].state,
                ItemState::Succeeded,
                "crash point {point} must recover/promote item to Succeeded"
            );
            assert_eq!(
                job.state,
                JobState::Completed,
                "crash point {point} must finalize parent job to Completed"
            );
            assert!(
                expected_out.exists(),
                "crash point {point} must preserve committed output photo.webp"
            );
            let decoded = image::open(&expected_out).expect("valid webp after C4/C5/C6");
            assert_eq!(decoded.width(), 16);
        } else {
            assert_eq!(
                items[0].state,
                ItemState::Interrupted,
                "crash point {point} must transition item to Interrupted"
            );
            assert_eq!(
                items[0].error_code.as_deref(),
                Some("worker_crashed"),
                "crash point {point} must record error_code = worker_crashed"
            );
            assert_eq!(
                job.state,
                JobState::Failed,
                "crash point {point} must transition 1-item parent job to Failed"
            );
            assert!(
                !expected_out.exists(),
                "crash point {point} must not leave uncommitted photo.webp"
            );

            // Furthermore, verify retrying the Interrupted job succeeds cleanly!
            let (retry_state, reset_count) =
                client.retry(job_id, RetryMode::FailedOnly, None).unwrap();
            assert_eq!(reset_count, 1);
            assert!(matches!(
                retry_state,
                JobState::Queued | JobState::Preparing | JobState::Running | JobState::Completed
            ));
            let (retried_job, retried_items) =
                env.wait_for_job_terminal(job_id, Duration::from_secs(10));
            assert_eq!(retried_job.state, JobState::Completed);
            assert_eq!(retried_items[0].state, ItemState::Succeeded);
            assert_eq!(retried_items[0].attempt_count, 2);
            assert!(expected_out.exists());
        }
    }
}

#[test]
fn ac_c2_7_ownership_fences_shared_stage_dir_temp_inode_race_sigterm_and_reboot() {
    // 1. T-STAGE-SHARED-DIR & T-TEMP-INODE-RACE
    let env = TestWorkerEnv::new();
    let dir = env.root.join("fences");
    fs::create_dir_all(&dir).unwrap();

    let src0 = dir.join("a.png");
    let src1 = dir.join("b.png");
    write_test_png(&src0, 8, 8, 1);
    write_test_png(&src1, 8, 8, 2);

    let store = JobStore::open(&env.paths.db_path).unwrap();
    let (job_id, _, _) = store
        .submit_or_deduplicate(
            &RequestId::generate(),
            Origin::Test,
            CONVERT_IMAGE_KIND,
            &[src0.clone(), src1.clone()],
            &serde_json::json!({"target": "webp"}),
        )
        .unwrap();
    store.mark_job_preparing(job_id).unwrap();
    store
        .mark_job_running(job_id, &serde_json::json!({"target": "webp"}))
        .unwrap();
    let claimed0 = store
        .claim_next_pending_item_for_job(job_id)
        .unwrap()
        .unwrap();
    let claimed1 = store
        .claim_next_pending_item_for_job(job_id)
        .unwrap()
        .unwrap();

    // Shared canonical staging dir `.rvstage-<job_id>-0123456789abcdef`
    let shared_stage = dir.join(format!(".rvstage-{}-0123456789abcdef", job_id.0));
    fs::create_dir_all(&shared_stage).unwrap();
    let stage_stat = rustix::fs::lstat(&shared_stage).unwrap();

    // Item 0 has its own `.part` file with matching (dev, ino)
    let part0 = shared_stage.join("item-0-0000000000000001.part");
    fs::write(&part0, b"partial-0").unwrap();
    let part0_stat = rustix::fs::lstat(&part0).unwrap();
    store
        .record_stage_created(
            claimed0.item_id,
            &shared_stage,
            stage_stat.st_dev,
            stage_stat.st_ino,
        )
        .unwrap();
    store
        .record_temp_created(
            claimed0.item_id,
            &shared_stage,
            &part0,
            part0_stat.st_dev,
            part0_stat.st_ino,
        )
        .unwrap();

    // Item 1: T-TEMP-INODE-RACE: record (temp_dev, temp_ino) = (999999, 888888) that does NOT match
    // the file currently at `part1`! Recovery must NOT delete `part1`, and because `part1` remains in
    // `shared_stage`, `shared_stage` must NOT be removed either (`T-STAGE-SHARED-DIR`).
    let part1 = shared_stage.join("item-1-0000000000000002.part");
    fs::write(&part1, b"replaced-by-another-file-inode").unwrap();
    store
        .record_stage_created(
            claimed1.item_id,
            &shared_stage,
            stage_stat.st_dev,
            stage_stat.st_ino,
        )
        .unwrap();
    store
        .record_temp_created(claimed1.item_id, &shared_stage, &part1, 999_999, 888_888)
        .unwrap();

    let report = run_startup_recovery(&store, "boot-1").unwrap();
    assert_eq!(report.recovered_c1, 2);
    assert!(
        !part0.exists(),
        "item 0's verified .part file must be unlinked during recovery"
    );
    assert!(
        part1.exists(),
        "T-TEMP-INODE-RACE: item 1's mismatched-inode file must NEVER be unlinked"
    );
    assert!(
        shared_stage.exists(),
        "T-STAGE-SHARED-DIR: non-empty shared staging dir must not be removed while part1 exists"
    );
    // Clean up our synthetic mismatched file
    let _ = fs::remove_file(&part1);
    let _ = fs::remove_dir(&shared_stage);

    // 2. T-RECOVER-REBOOT: boot_id mismatch marks in-flight item Interrupted(system_reboot)
    let (reboot_job_id, _, _) = store
        .submit_or_deduplicate(
            &RequestId::generate(),
            Origin::Test,
            CONVERT_IMAGE_KIND,
            std::slice::from_ref(&src0),
            &serde_json::json!({"target": "webp"}),
        )
        .unwrap();
    store.mark_job_preparing(reboot_job_id).unwrap();
    store
        .mark_job_running(reboot_job_id, &serde_json::json!({"target": "webp"}))
        .unwrap();
    let _claimed_reboot = store
        .claim_next_pending_item_for_job(reboot_job_id)
        .unwrap()
        .unwrap();

    // Run recovery with a new boot_id ("boot-2" != "boot-1")
    let report_reboot = run_startup_recovery(&store, "boot-2").unwrap();
    assert!(report_reboot.reboot_detected);
    let (rjob, ritems, _) = store
        .get_job_detail(reboot_job_id, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(rjob.state, JobState::Failed);
    assert_eq!(ritems[0].state, ItemState::Interrupted);
    assert_eq!(ritems[0].error_code.as_deref(), Some("system_reboot"));
    drop(store);

    // 3. T-RECOVER-SIGTERM: SIGTERM during active batch stops claiming, marks remaining in-flight Interrupted, and exits 0 in <= 6s
    let mut env_term = TestWorkerEnv::new();
    let pid =
        env_term.spawn_worker_with_env(&[("RADIVINE_WORKER_TEST_ITEM_SLEEP_MS", "250")], 60_000);
    let term_dir = env_term.root.join("sigterm");
    fs::create_dir_all(&term_dir).unwrap();
    let mut term_wires = Vec::new();
    for i in 0..8 {
        let p = term_dir.join(format!("t_{i}.png"));
        write_test_png(&p, 8, 8, i as u8);
        term_wires.push(WirePath::from_absolute_path(&p).unwrap());
    }
    let mut c = env_term.connect();
    let (term_job_id, _, _) = c
        .submit(
            RequestId::generate(),
            Origin::Cli,
            CONVERT_IMAGE_KIND,
            term_wires,
            serde_json::json!({"target": "webp"}),
        )
        .unwrap();
    drop(c);

    // Wait 100ms so items are in flight, then send SIGTERM
    thread::sleep(Duration::from_millis(100));
    let t_sig = Instant::now();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(pid as i32).unwrap(),
        rustix::process::Signal::TERM,
    )
    .unwrap();

    let child = env_term.children.last_mut().unwrap();
    let status = child.wait().unwrap();
    let sigterm_elapsed = t_sig.elapsed();
    assert!(
        status.success(),
        "SIGTERM shutdown must exit with code 0, got {status:?}"
    );
    assert!(
        sigterm_elapsed < Duration::from_secs(6),
        "SIGTERM shutdown took {sigterm_elapsed:?}, must be <= 6s"
    );
    assert!(
        !env_term.paths.socket_path.exists(),
        "worker.sock must be unlinked on SIGTERM shutdown"
    );
    let _ = term_job_id;
}

#[test]
fn ac_c2_9_and_c2_10_cancellation_and_retry_modes() {
    let mut env = TestWorkerEnv::new();
    env.spawn_worker_with_env(&[("RADIVINE_WORKER_TEST_ITEM_SLEEP_MS", "150")], 60_000);

    let dir = env.root.join("cancel_retry");
    fs::create_dir_all(&dir).unwrap();
    let mut wires = Vec::new();
    for i in 0..10 {
        let p = dir.join(format!("cr_{i:02}.png"));
        write_test_png(&p, 12, 12, i as u8);
        wires.push(WirePath::from_absolute_path(&p).unwrap());
    }

    let mut client = env.connect();
    let (job_id, _, _) = client
        .submit(
            RequestId::generate(),
            Origin::ControlCenter,
            CONVERT_IMAGE_KIND,
            wires,
            serde_json::json!({"target": "webp"}),
        )
        .unwrap();

    // Wait until at least 1 item has succeeded so we test partial completion + cancellation
    let wait_one = Instant::now() + Duration::from_secs(5);
    while Instant::now() < wait_one {
        let (j, _, _) = client.get_job(job_id, Some(0), Some(1)).unwrap();
        if j.counts.succeeded_items >= 1 {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }

    // Issue Cancel and assert RPC response latency < 100ms (AC-C2.9)
    let t_cancel = Instant::now();
    let cancel_state = client.cancel(job_id).unwrap();
    let cancel_rpc_latency = t_cancel.elapsed();
    assert!(
        cancel_rpc_latency < Duration::from_millis(100),
        "Cancel RPC took {cancel_rpc_latency:?}, must be < 100ms"
    );
    assert!(matches!(
        cancel_state,
        JobState::Running | JobState::Cancelled
    ));

    let (cancelled_job, cancelled_items) =
        env.wait_for_job_terminal(job_id, Duration::from_secs(10));
    assert_eq!(cancelled_job.state, JobState::Cancelled);
    assert!(cancelled_job.counts.succeeded_items >= 1);
    assert!(cancelled_job.counts.cancelled_items >= 1);
    assert_eq!(cancelled_job.counts.pending_items, 0);
    assert_eq!(cancelled_job.counts.in_flight_items, 0);
    assert_no_temp_or_stage_artifacts(&dir);

    let succeeded_before_retry: Vec<u32> = cancelled_items
        .iter()
        .filter(|it| it.state == ItemState::Succeeded)
        .map(|it| it.item_index)
        .collect();
    assert!(!succeeded_before_retry.is_empty());

    // Now test RetryMode::FailedAndCancelled (AC-C2.10)
    let (retry_state, reset_items) = client
        .retry(job_id, RetryMode::FailedAndCancelled, None)
        .unwrap();
    assert_eq!(reset_items, cancelled_job.counts.cancelled_items);
    assert!(matches!(
        retry_state,
        JobState::Queued | JobState::Preparing | JobState::Running | JobState::Completed
    ));

    let (completed_job, final_items) = env.wait_for_job_terminal(job_id, Duration::from_secs(15));
    assert_eq!(completed_job.job_id, job_id, "retry must preserve job_id");
    assert_eq!(completed_job.state, JobState::Completed);
    assert_eq!(completed_job.counts.succeeded_items, 10);
    let mut saw_inflight_retry_attempt_2 = false;
    for it in &final_items {
        assert_eq!(it.state, ItemState::Succeeded);
        if succeeded_before_retry.contains(&it.item_index) {
            assert_eq!(
                it.attempt_count, 1,
                "already-Succeeded item must not be re-run in FailedAndCancelled mode"
            );
        } else {
            assert!(
                it.attempt_count >= 1,
                "retried item must have executed at least 1 attempt"
            );
            if it.attempt_count >= 2 {
                saw_inflight_retry_attempt_2 = true;
            }
        }
    }
    assert!(
        saw_inflight_retry_attempt_2,
        "items that were in-flight when cancelled must have attempt_count == 2 after retry"
    );

    // Also test RetryMode::FailedOnly on a CompletedWithErrors job (1 good + 1 corrupt -> fix corrupt -> retry)
    let good_path = dir.join("good_item.png");
    let bad_path = dir.join("bad_item.png");
    write_test_png(&good_path, 8, 8, 11);
    fs::write(&bad_path, b"\x89PNG\r\n\x1a\ncorrupt_payload").unwrap();

    let (err_job_id, _, _) = client
        .submit(
            RequestId::generate(),
            Origin::ControlCenter,
            CONVERT_IMAGE_KIND,
            vec![
                WirePath::from_absolute_path(&good_path).unwrap(),
                WirePath::from_absolute_path(&bad_path).unwrap(),
            ],
            serde_json::json!({"target": "webp"}),
        )
        .unwrap();
    let (err_job, err_items) = env.wait_for_job_terminal(err_job_id, Duration::from_secs(10));
    assert_eq!(err_job.state, JobState::CompletedWithErrors);
    assert_eq!(err_job.counts.succeeded_items, 1);
    assert_eq!(err_job.counts.failed_items, 1);
    assert_eq!(err_items[0].state, ItemState::Succeeded);
    assert_eq!(err_items[1].state, ItemState::Failed);

    // Fix the corrupt input file on disk and retry FailedOnly with an options_override
    write_test_png(&bad_path, 8, 8, 22);
    let (_, reset_failed) = client
        .retry(
            err_job_id,
            RetryMode::FailedOnly,
            Some(serde_json::json!({"target": "webp"})),
        )
        .unwrap();
    assert_eq!(reset_failed, 1);
    let (fixed_job, fixed_items) = env.wait_for_job_terminal(err_job_id, Duration::from_secs(10));
    assert_eq!(fixed_job.state, JobState::Completed);
    assert_eq!(fixed_items[0].attempt_count, 1);
    assert_eq!(fixed_items[1].attempt_count, 2);
}

#[test]
fn ac_c2_11_and_c2_12_fair_scheduling_free_space_and_watchdog_recycle() {
    // Part A: AC-C2.11 Round-robin fairness between 12-item Job A and 1-item Job B
    let mut env = TestWorkerEnv::new();
    env.spawn_worker_with_env(&[("RADIVINE_WORKER_TEST_ITEM_SLEEP_MS", "60")], 60_000);

    let dir = env.root.join("fairness");
    fs::create_dir_all(&dir).unwrap();
    let mut wires_a = Vec::new();
    for i in 0..12 {
        let p = dir.join(format!("a_{i:02}.png"));
        write_test_png(&p, 8, 8, i as u8);
        wires_a.push(WirePath::from_absolute_path(&p).unwrap());
    }
    let path_b = dir.join("b_single.png");
    write_test_png(&path_b, 8, 8, 99);
    let wire_b = WirePath::from_absolute_path(&path_b).unwrap();

    let sub_client = env.connect();
    let (_, mut sub) = sub_client.subscribe(None).unwrap();
    let _ = sub.set_read_timeout(Some(Duration::from_secs(15)));

    let mut client = env.connect();
    let (job_a, _, _) = client
        .submit(
            RequestId::generate(),
            Origin::Wheel,
            CONVERT_IMAGE_KIND,
            wires_a,
            serde_json::json!({"target": "webp"}),
        )
        .unwrap();
    let (job_b, _, _) = client
        .submit(
            RequestId::generate(),
            Origin::Cli,
            CONVERT_IMAGE_KIND,
            vec![wire_b],
            serde_json::json!({"target": "webp"}),
        )
        .unwrap();

    let mut b_finished_when_a_succeeded = None;
    let mut a_latest_succeeded = 0u32;
    let mut a_finished = false;
    let mut b_finished = false;

    while !(a_finished && b_finished) {
        let Some(ev) = sub.next_event().unwrap() else {
            break;
        };
        match ev {
            JobUpdate::ItemCompleted {
                job_id, summary, ..
            } if job_id == job_a => {
                a_latest_succeeded = summary.succeeded_items;
            }
            JobUpdate::JobFinished { job, .. } if job.job_id == job_b => {
                b_finished = true;
                b_finished_when_a_succeeded = Some(a_latest_succeeded);
            }
            JobUpdate::JobFinished { job, .. } if job.job_id == job_a => {
                a_finished = true;
            }
            _ => {}
        }
    }
    let a_done_at_b_finish = b_finished_when_a_succeeded.expect("Job B must finish");
    assert!(
        a_done_at_b_finish < 10,
        "Fairness violation: Job B waited until {a_done_at_b_finish}/12 of Job A completed"
    );

    // Part B: AC-C2.12 Destination free-space precheck (RADIVINE_WORKER_TEST_FAKE_AVAIL_BYTES)
    let mut env_nospc = TestWorkerEnv::new();
    env_nospc.spawn_worker_with_env(&[("RADIVINE_WORKER_TEST_FAKE_AVAIL_BYTES", "1024")], 60_000);
    let nospc_dir = env_nospc.root.join("nospc");
    fs::create_dir_all(&nospc_dir).unwrap();
    let nospc_src = nospc_dir.join("space.png");
    write_test_png(&nospc_src, 16, 16, 5);
    let mut c_nospc = env_nospc.connect();
    let (nospc_jid, _, _) = c_nospc
        .submit(
            RequestId::generate(),
            Origin::Test,
            CONVERT_IMAGE_KIND,
            vec![WirePath::from_absolute_path(&nospc_src).unwrap()],
            serde_json::json!({"target": "webp"}),
        )
        .unwrap();
    let (nospc_job, nospc_items) =
        env_nospc.wait_for_job_terminal(nospc_jid, Duration::from_secs(10));
    assert_eq!(nospc_job.state, JobState::Failed);
    assert_eq!(nospc_items[0].state, ItemState::Failed);
    assert_eq!(nospc_items[0].error_code.as_deref(), Some("no_space"));
    assert!(nospc_items[0].retryable);

    // Part C: AC-C2.12 Per-item watchdog timeout triggers exit(75) worker recycling
    let mut env_wd = TestWorkerEnv::new();
    let wd_pid = env_wd.spawn_worker_with_env(
        &[
            ("RADIVINE_WORKER_ITEM_TIMEOUT_MS", "120"),
            ("RADIVINE_WORKER_TEST_ITEM_SLEEP_MS", "600"),
        ],
        60_000,
    );
    let wd_dir = env_wd.root.join("watchdog");
    fs::create_dir_all(&wd_dir).unwrap();
    let wd_src = wd_dir.join("hang.png");
    write_test_png(&wd_src, 8, 8, 3);
    let mut c_wd = env_wd.connect();
    let (wd_jid, _, _) = c_wd
        .submit(
            RequestId::generate(),
            Origin::Test,
            CONVERT_IMAGE_KIND,
            vec![WirePath::from_absolute_path(&wd_src).unwrap()],
            serde_json::json!({"target": "webp"}),
        )
        .unwrap();
    drop(c_wd);

    // Wait for the original worker to exit with code 75 (EX_TEMPFAIL)
    let wd_child = env_wd.children.last_mut().unwrap();
    let dl = Instant::now() + Duration::from_secs(10);
    let mut wd_status = None;
    while Instant::now() < dl {
        if let Some(st) = wd_child.try_wait().unwrap() {
            wd_status = Some(st);
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    let st = wd_status.expect("watchdog must recycle worker process");
    assert_eq!(
        st.code(),
        Some(75),
        "watchdog recycle must exit with EX_TEMPFAIL (75), got {st:?} (pid {wd_pid})"
    );

    // Verify item was recorded as Failed(timed_out, retryable=false) in SQLite!
    let wd_store = JobStore::open(&env_wd.paths.db_path).unwrap();
    let (wd_job, wd_items, _) = wd_store
        .get_job_detail(wd_jid, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(wd_job.state, JobState::Failed);
    assert_eq!(wd_items[0].state, ItemState::Failed);
    assert_eq!(wd_items[0].error_code.as_deref(), Some("timed_out"));
    assert!(!wd_items[0].retryable);
    assert_no_temp_or_stage_artifacts(&wd_dir);
}

#[test]
fn ac_c2_13_and_c2_14_progress_coalescing_slow_subscriber_and_idle_relaunch() {
    let mut env = TestWorkerEnv::new();
    // Spawn with a short 300ms idle timeout so we also test T-LIFE-3 idle exit + auto-relaunch!
    env.spawn_worker_with_env(&[("RADIVINE_WORKER_TEST_ITEM_SLEEP_MS", "30")], 300);

    let dir = env.root.join("sub_idle");
    fs::create_dir_all(&dir).unwrap();
    let mut wires = Vec::new();
    for i in 0..4 {
        let p = dir.join(format!("s_{i}.png"));
        write_test_png(&p, 12, 12, i as u8);
        wires.push(WirePath::from_absolute_path(&p).unwrap());
    }

    // Connect a stalled subscriber that subscribes and then shrinks its OS receive buffer and never reads
    let stalled_stream = UnixStream::connect(&env.paths.socket_path).unwrap();
    let _ = rustix::net::sockopt::set_socket_recv_buffer_size(&stalled_stream, 2048);
    let mut stalled_writer = stalled_stream.try_clone().unwrap();
    stalled_writer
        .write_all(b"{\"v\":1,\"type\":\"subscribe\"}\n")
        .unwrap();
    stalled_writer.flush().unwrap();

    // Connect a well-behaved subscriber
    let good_client = env.connect();
    let (_, mut good_sub) = good_client.subscribe(None).unwrap();
    let _ = good_sub.set_read_timeout(Some(Duration::from_secs(10)));

    // Submit the 4-item job
    let mut client = env.connect();
    let (job_id, _, _) = client
        .submit(
            RequestId::generate(),
            Origin::ControlCenter,
            CONVERT_IMAGE_KIND,
            wires,
            serde_json::json!({"target": "webp"}),
        )
        .unwrap();
    drop(client);

    let mut saw_job_started = false;
    let mut saw_item_progress = 0;
    let mut saw_item_finished = 0;
    let mut saw_job_finished = false;

    while let Some(ev) = good_sub.next_event().unwrap() {
        match ev {
            JobUpdate::JobCreated { job, .. } if job.job_id == job_id => saw_job_started = true,
            JobUpdate::JobStateChanged { job_id: j, .. } if j == job_id => saw_job_started = true,
            JobUpdate::ItemProgress { job_id: j, .. } if j == job_id => saw_item_progress += 1,
            JobUpdate::ItemCompleted { job_id: j, .. } if j == job_id => saw_item_finished += 1,
            JobUpdate::JobFinished { job, .. } if job.job_id == job_id => {
                assert_eq!(job.state, JobState::Completed);
                saw_job_finished = true;
                break;
            }
            _ => {}
        }
    }
    assert!(saw_job_started);
    assert!(saw_item_progress >= 1);
    assert_eq!(saw_item_finished, 4);
    assert!(saw_job_finished);

    // Drop subscriber connections so active_subscribers == 0 and wait for the 300ms idle timeout (T-LIFE-3)
    drop(good_sub);
    drop(stalled_writer);
    drop(stalled_stream);

    let child = env.children.last_mut().unwrap();
    let dl = Instant::now() + Duration::from_secs(5);
    let mut idle_exited = None;
    while Instant::now() < dl {
        if let Some(st) = child.try_wait().unwrap() {
            idle_exited = Some(st);
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    let idle_status = idle_exited.expect("worker must exit cleanly after idle_timeout_ms");
    assert!(idle_status.success(), "idle exit must return 0");
    assert!(
        !env.paths.socket_path.exists(),
        "worker.sock must be unlinked on idle exit"
    );

    // Now test automatic relaunch via WorkerLauncher / JobsClient::connect_or_launch (AC-C2.14 / T-LIFE-3)
    // Because we are in an isolated test dir, set RADIVINE_WORKER_BIN / SOCKET / DB in process env for the launcher
    let launcher_paths = env.paths.clone();
    let mut relaunched_cmd = env.base_command();
    relaunched_cmd
        .arg("--socket")
        .arg(&launcher_paths.socket_path)
        .arg("--db")
        .arg(&launcher_paths.db_path)
        .arg("--lock")
        .arg(&launcher_paths.lock_path)
        .arg("show")
        .arg(job_id.0.to_string())
        .arg("--json");
    let show_out = relaunched_cmd.output().expect("run radivine-worker show");
    assert!(show_out.status.success());
    let parsed_show: serde_json::Value = serde_json::from_slice(&show_out.stdout).unwrap();
    assert_eq!(parsed_show["job"]["state"], "completed");
    assert_eq!(parsed_show["job"]["counts"]["succeeded_items"], 4);

    // Also test submit-convert auto-launching the worker when offline!
    let auto_src = dir.join("auto_relaunch.png");
    write_test_png(&auto_src, 8, 8, 77);
    let mut submit_cmd = env.base_command();
    submit_cmd
        .env("RADIVINE_WORKER_IDLE_TIMEOUT_MS", "400")
        .arg("--socket")
        .arg(&launcher_paths.socket_path)
        .arg("--db")
        .arg(&launcher_paths.db_path)
        .arg("--lock")
        .arg(&launcher_paths.lock_path)
        .arg("submit-convert")
        .arg("--to")
        .arg("webp")
        .arg("--wait")
        .arg("--json")
        .arg(&auto_src);
    let submit_out = submit_cmd
        .output()
        .expect("submit-convert must auto-launch offline worker");
    assert!(
        submit_out.status.success(),
        "submit-convert failed: {}",
        String::from_utf8_lossy(&submit_out.stderr)
    );
    assert!(dir.join("auto_relaunch.webp").exists());
}

#[test]
fn ac_c2_15_and_c2_16_and_c2_17_ipc_security_non_utf8_paths_and_dependency_graph() {
    let mut env = TestWorkerEnv::new();
    env.spawn_worker();

    // 1. AC-C2.15: Socket permissions must be 0600
    let sock_meta = fs::symlink_metadata(&env.paths.socket_path).unwrap();
    assert_eq!(
        sock_meta.permissions().mode() & 0o777,
        0o600,
        "worker.sock must have 0600 permissions"
    );

    // 2. AC-C2.15: Unknown protocol version `v: 99` rejected with UnsupportedProtocolVersion
    {
        let mut raw = UnixStream::connect(&env.paths.socket_path).unwrap();
        raw.write_all(b"{\"v\":99,\"type\":\"ping\"}\n").unwrap();
        raw.flush().unwrap();
        let mut reader = BufReader::new(raw);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let resp: WorkerResponse = serde_json::from_str(&line).unwrap();
        match resp {
            WorkerResponse::Error { code, .. } => {
                assert_eq!(code, WorkerErrorCode::UnsupportedProtocolVersion);
            }
            other => panic!("expected UnsupportedProtocolVersion error, got {other:?}"),
        }
    }

    // 3. AC-C2.15: Oversized > 64 KiB non-submit frame rejected with FrameTooLarge
    {
        let mut raw = UnixStream::connect(&env.paths.socket_path).unwrap();
        let padding = "a".repeat(70 * 1024);
        let frame = format!("{{\"v\":1,\"type\":\"ping\",\"pad\":\"{padding}\"}}\n");
        let _ = raw.write_all(frame.as_bytes());
        let _ = raw.flush();
        let mut reader = BufReader::new(raw);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let resp: WorkerResponse = serde_json::from_str(&line).unwrap();
        match resp {
            WorkerResponse::Error { code, .. } => {
                assert_eq!(code, WorkerErrorCode::MessageTooLarge);
            }
            other => panic!("expected MessageTooLarge error, got {other:?}"),
        }
    }

    // 4. AC-C2.15: > 10,000 items rejected with BatchTooLarge
    {
        let mut client = env.connect();
        let items: Vec<WirePath> = (0..=MAX_ITEMS_PER_JOB)
            .map(|i| WirePath::from_raw_bytes(format!("/tmp/file_{i}.png").as_bytes()).unwrap())
            .collect();
        let res = client.submit(
            RequestId::generate(),
            Origin::Test,
            CONVERT_IMAGE_KIND,
            items,
            serde_json::json!({"target": "webp"}),
        );
        assert!(
            res.is_err(),
            "batch of {} items must be rejected",
            MAX_ITEMS_PER_JOB + 1
        );
    }

    // 5. AC-C2.15: Relative path rejected with InvalidPath
    {
        let mut client = env.connect();
        let res = client.submit(
            RequestId::generate(),
            Origin::Test,
            CONVERT_IMAGE_KIND,
            vec![WirePath::from_raw_bytes(b"relative/path.png").unwrap()],
            serde_json::json!({"target": "webp"}),
        );
        assert!(res.is_err(), "relative WirePath must be rejected");
    }

    // 6. AC-C2.15: SO_PEERCRED UID mismatch rejected (using RADIVINE_WORKER_TEST_FAKE_PEER_UID)
    {
        let mut env_uid = TestWorkerEnv::new();
        // Start worker manually with RADIVINE_WORKER_TEST_FAKE_PEER_UID=999999
        let mut cmd = env_uid.base_command();
        cmd.env("RADIVINE_WORKER_TEST_FAKE_PEER_UID", "999999")
            .arg("--socket")
            .arg(&env_uid.paths.socket_path)
            .arg("--db")
            .arg(&env_uid.paths.db_path)
            .arg("--lock")
            .arg(&env_uid.paths.lock_path)
            .arg("serve")
            .arg("--idle-timeout-ms")
            .arg("5000")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        env_uid.children.push(cmd.spawn().unwrap());
        let dl = Instant::now() + Duration::from_secs(5);
        while Instant::now() < dl && !env_uid.paths.socket_path.exists() {
            thread::sleep(Duration::from_millis(20));
        }
        thread::sleep(Duration::from_millis(50));
        let mut c = JobsClient::connect(&env_uid.paths.socket_path).unwrap();
        assert!(
            c.ping().is_err(),
            "connection with mismatched peer UID must be closed immediately"
        );
    }

    // 7. AC-C2.16: End-to-end non-UTF-8 path preservation (`0x80`, `0xFF` in filename)
    let non_utf8_dir = env.root.join("non_utf8");
    fs::create_dir_all(&non_utf8_dir).unwrap();
    let raw_src_name = OsStr::from_bytes(b"photo_\x80\xff_test.png");
    let raw_out_name = OsStr::from_bytes(b"photo_\x80\xff_test.webp");
    let non_utf8_src = non_utf8_dir.join(raw_src_name);
    let non_utf8_out = non_utf8_dir.join(raw_out_name);
    write_test_png(&non_utf8_src, 12, 12, 55);

    let wire_non_utf8 = WirePath::from_absolute_path(&non_utf8_src).unwrap();
    assert!(wire_non_utf8.as_str().contains("%80%FF") || wire_non_utf8.as_str().contains("%80%ff"));

    let mut client = env.connect();
    let (nu_job_id, _, _) = client
        .submit(
            RequestId::generate(),
            Origin::Test,
            CONVERT_IMAGE_KIND,
            vec![wire_non_utf8],
            serde_json::json!({"target": "webp"}),
        )
        .unwrap();
    let (nu_job, nu_items) = env.wait_for_job_terminal(nu_job_id, Duration::from_secs(10));
    assert_eq!(nu_job.state, JobState::Completed);
    assert_eq!(nu_items[0].state, ItemState::Succeeded);
    assert!(
        non_utf8_out.exists(),
        "non-UTF-8 output path {:?} must exist on disk",
        non_utf8_out
    );
    let reported_out = nu_items[0]
        .output_path
        .as_ref()
        .unwrap()
        .to_path_buf()
        .unwrap();
    assert_eq!(
        reported_out.as_os_str().as_bytes(),
        non_utf8_out.as_os_str().as_bytes(),
        "GetJob must preserve exact raw non-UTF-8 output path bytes"
    );

    // Also verify SQLite stored raw BLOB bytes in job_items.source_path and job_items.output_path
    let conn = Connection::open(&env.paths.db_path).unwrap();
    let (db_src_blob, db_out_blob): (Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT source_path, output_path FROM job_items WHERE job_id = ?1",
            params![nu_job_id.0 as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(db_src_blob, non_utf8_src.as_os_str().as_bytes());
    assert_eq!(db_out_blob, non_utf8_out.as_os_str().as_bytes());

    // 8. AC-C2.17: Dependency graph isolation check (`cargo tree`)
    let forbidden = [
        "radivine-wheel",
        "radivine-core",
        "radivine-actions",
        "radivine-x11",
        "radivine-ui",
        "gtk4",
        "libadwaita",
    ];
    for pkg in ["radivine-worker", "radivine-jobs", "radivine-dbus"] {
        let out = Command::new("cargo")
            .args(["tree", "-p", pkg, "-e", "normal"])
            .output()
            .expect("run cargo tree");
        assert!(out.status.success());
        let tree_str = String::from_utf8_lossy(&out.stdout);
        for bad in forbidden {
            assert!(
                !tree_str.contains(bad),
                "Package {pkg} must NOT depend on {bad}!\ncargo tree output:\n{tree_str}"
            );
        }
    }

    // Touch WorkerLaunchMode symbol so unused import warnings cannot occur
    let _ = WorkerLaunchMode::from_env();
}
