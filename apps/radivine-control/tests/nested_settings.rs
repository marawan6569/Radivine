//! Milestone N4 Control Center layout selector and persistence integration tests
//! (`TEST_MATRIX.md` `T-CFG1`–`T-CFG8`, `PRODUCT_SPEC.md` §P4, `ARCHITECTURE.md` §A6).

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use radivine::config::AppConfig;
use radivine_control::ControlPage;
use radivine_control::persistence::convert_path_for;
use radivine_control::screenshot::EguiSoftRasterizer;
use radivine_control::service::{DaemonHealthState, ServiceController, ServicePaths};
use radivine_control::ui::ControlCenterApp;

fn make_temp_test_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("radivine-n4-ctrl-{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}

fn ensure_daemon_bin() -> PathBuf {
    static DAEMON_BIN: OnceLock<PathBuf> = OnceLock::new();
    DAEMON_BIN
        .get_or_init(|| {
            let control_bin = PathBuf::from(env!("CARGO_BIN_EXE_radivine-control"));
            let bin_dir = control_bin.parent().expect("target debug dir");
            let candidate = bin_dir.join("radivine");
            let mut cmd = Command::new("cargo");
            cmd.args(["build", "-q", "-p", "radivine", "--bin", "radivine"]);
            if cfg!(not(debug_assertions)) {
                cmd.arg("--release");
            }
            let status = cmd
                .status()
                .expect("build radivine binary for N4 control tests");
            assert!(status.success(), "cargo build -p radivine failed");
            assert!(
                candidate.is_file(),
                "expected radivine binary at {}",
                candidate.display()
            );
            candidate
        })
        .clone()
}

fn isolated_service_paths(base_dir: &Path) -> ServicePaths {
    let daemon_bin = ensure_daemon_bin();

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

/// `T-CFG1` & `T-CFG2`: Missing/legacy `config.toml` defaults to `"branching"`, and invalid
/// `wheel.layout` values fall back only `wheel.layout` to `"branching"` while preserving
/// non-default trigger, safety, jobs, and unknown settings.
#[test]
fn t_cfg1_t_cfg2_control_center_defaults_and_field_local_fallback() {
    let dir = make_temp_test_dir("cfg1-cfg2");
    let paths = isolated_service_paths(&dir);
    let cfg_path = paths.config_path.clone().expect("config path");

    // 1. Missing config.toml -> defaults to "branching"
    let app_missing = ControlCenterApp::new(ControlPage::Wheel, paths.clone(), None, None, false);
    assert_eq!(app_missing.saved_config.wheel.layout, "branching");
    assert_eq!(app_missing.draft_config.wheel.layout, "branching");
    assert!(app_missing.is_branching_geometry_enabled());

    // 2. Legacy config.toml with invalid `wheel.layout` type/string -> falls back only `wheel.layout`
    let raw_invalid = r#"# Header comment
version = 1

[trigger]
drag_modifier = "ctrl"
toggle_off_on_repress = false

[input]
passthrough = true

[wheel]
layout = "CONCENTRIC"
root_inner_radius = 46.0
root_outer_radius = 140.0
child_inner_radius = 38.0
child_outer_radius = 116.0
child_offset_gap = 24.0

[jobs]
max_parallel = 3
"#;
    fs::write(&cfg_path, raw_invalid).expect("write invalid layout config");

    let app_fallback = ControlCenterApp::new(ControlPage::Wheel, paths, None, None, false);
    assert_eq!(app_fallback.saved_config.wheel.layout, "branching");
    assert_eq!(app_fallback.draft_config.wheel.layout, "branching");
    // Other settings are preserved intact
    assert_eq!(app_fallback.saved_config.trigger.drag_modifier, "ctrl");
    assert!(!app_fallback.saved_config.trigger.toggle_off_on_repress);
    assert!(app_fallback.saved_config.input.passthrough);
    assert_eq!(app_fallback.saved_config.wheel.root_inner_radius, 46.0);
    assert_eq!(app_fallback.saved_config.wheel.child_inner_radius, 38.0);
    assert_eq!(
        app_fallback.saved_jobs.max_parallel,
        radivine_jobs::MaxParallelSetting::Fixed(3)
    );

    let _ = fs::remove_dir_all(&dir);
}

/// `T-CFG3` & `T-CFG4`: Selecting Concentric Rings and saving through the established Save workflow
/// preserves TOML comments, unknown sections, trigger preferences, conversion settings, job settings,
/// and backups, and does NOT rewrite `actions.toml` or `convert.toml`.
#[test]
fn t_cfg3_t_cfg4_atomic_save_preserves_toml_comments_and_never_touches_actions_or_convert() {
    let dir = make_temp_test_dir("cfg3-cfg4");
    let paths = isolated_service_paths(&dir);
    let cfg_path = paths.config_path.clone().expect("config path");
    let actions_path = AppConfig::actions_path_for(Some(&cfg_path));
    let convert_path = convert_path_for(&cfg_path);

    let initial_config = r#"# User top-level comment
version = 1

[trigger]
# Keep custom trigger modifier
drag_modifier = "alt"

[wheel]
# Custom wheel geometry comment
root_inner_radius = 42.0
root_outer_radius = 132.0
child_inner_radius = 36.0
child_outer_radius = 112.0
child_offset_gap = 22.0

[jobs]
# Custom jobs comment
max_parallel = 2

[custom_unknown_table]
preserve_flag = true
note = "keep me untouched"
"#;
    let initial_actions = "# Do not rewrite actions.toml\nversion = 1\n";
    let initial_convert = "# Do not rewrite convert.toml\nversion = 1\n";

    fs::write(&cfg_path, initial_config).expect("write initial config.toml");
    fs::write(&actions_path, initial_actions).expect("write initial actions.toml");
    fs::write(&convert_path, initial_convert).expect("write initial convert.toml");

    let actions_mtime_before = fs::metadata(&actions_path).unwrap().modified().unwrap();
    let convert_mtime_before = fs::metadata(&convert_path).unwrap().modified().unwrap();

    let mut app = ControlCenterApp::new(ControlPage::Wheel, paths, None, None, false);
    assert_eq!(app.saved_config.wheel.layout, "branching");

    // Switching the selector changes draft only until Save is invoked
    app.select_draft_wheel_layout("concentric");
    assert_eq!(app.draft_config.wheel.layout, "concentric");
    assert_eq!(app.saved_config.wheel.layout, "branching");
    let on_disk_before_save = fs::read_to_string(&cfg_path).unwrap();
    assert!(!on_disk_before_save.contains("layout = \"concentric\""));

    // Save through the established Save workflow
    app.save_config(false).expect("save concentric layout");
    assert_eq!(app.saved_config.wheel.layout, "concentric");

    let saved_text = fs::read_to_string(&cfg_path).expect("read saved config.toml");
    assert!(saved_text.contains("# User top-level comment"));
    assert!(saved_text.contains("# Keep custom trigger modifier"));
    assert!(saved_text.contains("# Custom wheel geometry comment"));
    assert!(saved_text.contains("# Custom jobs comment"));
    assert!(saved_text.contains("layout = \"concentric\""));
    assert!(saved_text.contains("drag_modifier = \"alt\""));
    assert!(saved_text.contains("max_parallel = \"2\""));
    assert_eq!(
        app.saved_jobs.max_parallel,
        radivine_jobs::MaxParallelSetting::Fixed(2)
    );
    assert!(saved_text.contains("[custom_unknown_table]"));
    assert!(saved_text.contains("note = \"keep me untouched\""));

    // Verify actions.toml and convert.toml were not rewritten
    assert_eq!(fs::read_to_string(&actions_path).unwrap(), initial_actions);
    assert_eq!(fs::read_to_string(&convert_path).unwrap(), initial_convert);
    assert_eq!(
        fs::metadata(&actions_path).unwrap().modified().unwrap(),
        actions_mtime_before
    );
    assert_eq!(
        fs::metadata(&convert_path).unwrap().modified().unwrap(),
        convert_mtime_before
    );

    let _ = fs::remove_dir_all(&dir);
}

/// `T-CFG5` & `T-CFG8`: Selecting Concentric disables Branching-specific child geometry controls
/// contextually while preserving their stored values, and Restore Wheel Defaults resets
/// `wheel.layout` to `"branching"`.
#[test]
fn t_cfg5_t_cfg8_contextual_branching_geometry_and_restore_defaults() {
    let dir = make_temp_test_dir("cfg5-cfg8");
    let paths = isolated_service_paths(&dir);
    let mut app = ControlCenterApp::new(ControlPage::Wheel, paths, None, None, false);

    // Customize branching child geometry
    app.draft_config.wheel.child_inner_radius = 44.0;
    app.draft_config.wheel.child_outer_radius = 126.0;
    app.draft_config.wheel.child_offset_gap = 28.0;
    assert!(app.is_branching_geometry_enabled());

    // Switch to Concentric -> Branching geometry controls are disabled, values remain stored
    app.select_draft_wheel_layout("concentric");
    assert!(!app.is_branching_geometry_enabled());
    assert_eq!(app.draft_config.wheel.child_inner_radius, 44.0);
    assert_eq!(app.draft_config.wheel.child_outer_radius, 126.0);
    assert_eq!(app.draft_config.wheel.child_offset_gap, 28.0);

    app.save_config(false)
        .expect("save concentric with custom child geometry");
    assert_eq!(app.saved_config.wheel.layout, "concentric");
    assert_eq!(app.saved_config.wheel.child_inner_radius, 44.0);
    assert_eq!(app.saved_config.wheel.child_outer_radius, 126.0);
    assert_eq!(app.saved_config.wheel.child_offset_gap, 28.0);

    // Render Wheel page headlessly in Concentric mode to verify UI widgets & disabled group execute cleanly
    let ctx = eframe::egui::Context::default();
    let mut rasterizer = EguiSoftRasterizer::new();
    let png_concentric = dir.join("wheel-concentric.png");
    rasterizer
        .render_to_png(
            &ctx,
            eframe::egui::Vec2::new(1120.0, 720.0),
            1.0,
            &png_concentric,
            |ui| {
                app.render_frame(ui);
            },
        )
        .expect("render wheel page in concentric mode");
    assert!(png_concentric.is_file());

    // Restore Wheel Defaults resets layout to "branching"
    app.restore_wheel_defaults();
    assert_eq!(app.draft_config.wheel.layout, "branching");
    assert!(app.is_branching_geometry_enabled());
    app.save_config(false)
        .expect("save restored wheel defaults");
    assert_eq!(app.saved_config.wheel.layout, "branching");

    let _ = fs::remove_dir_all(&dir);
}

/// `T-CFG6` & `T-CFG7`: Repeated alternating saves (`branching` <-> `concentric`), live daemon reload,
/// daemon-offline save, and failed IPC (dead socket) produce truthful status messages.
#[test]
fn t_cfg6_t_cfg7_alternating_saves_live_daemon_offline_and_failed_ipc_banners() {
    let dir = make_temp_test_dir("cfg6-cfg7");
    let paths = isolated_service_paths(&dir);
    let cfg_path = paths.config_path.clone().expect("config path");

    // 1. Daemon offline save -> truthful offline banner
    let mut app = ControlCenterApp::new(ControlPage::Wheel, paths.clone(), None, None, false);
    assert_eq!(app.service_snapshot().health, DaemonHealthState::Stopped);
    app.select_draft_wheel_layout("concentric");
    app.save_config(false)
        .expect("save while daemon is offline");
    assert!(
        app.banner_ok()
            .unwrap_or_default()
            .contains("when the daemon starts"),
        "expected offline banner, got {:?}",
        app.banner_ok()
    );
    assert!(app.banner_err().is_none());
    assert!(
        app.wheel_layout_status_message()
            .contains("Saved preference: concentric · Daemon offline")
    );

    // 2. Start isolated daemon and verify live alternating saves (`concentric` -> `branching` -> `concentric`)
    let ctrl = ServiceController::new(paths.clone());
    let snap_start = ctrl.start_daemon().expect("start isolated daemon");
    assert_eq!(snap_start.health, DaemonHealthState::Running);
    app.refresh_service_snapshot_sync();
    assert_eq!(
        app.service_snapshot()
            .daemon
            .as_ref()
            .unwrap()
            .requested_layout,
        "concentric"
    );

    for target_layout in ["branching", "concentric", "branching", "concentric"] {
        app.select_draft_wheel_layout(target_layout);
        app.save_config(false).expect("alternating save");
        assert!(
            app.banner_ok()
                .unwrap_or_default()
                .contains("Saved config.toml"),
            "expected reload success banner, got {:?}",
            app.banner_ok()
        );
        assert!(app.banner_err().is_none());
        let daemon = app
            .service_snapshot()
            .daemon
            .as_ref()
            .expect("daemon running");
        assert_eq!(daemon.requested_layout, target_layout);
    }

    // 3. Stop daemon, plant a dead UNIX socket at `ipc_socket` to force IPC reload failure on save
    ctrl.stop_daemon().expect("stop daemon");
    let _dead_listener =
        std::os::unix::net::UnixListener::bind(&paths.ipc_socket).expect("bind dead socket");
    drop(_dead_listener); // leaves stale socket file on disk with no listener

    app.select_draft_wheel_layout("branching");
    app.save_config(false)
        .expect("file save still succeeds when IPC reload fails");
    // File on disk must retain the newly saved layout
    let reloaded_disk = AppConfig::load_or_default(Some(&cfg_path));
    assert_eq!(reloaded_disk.wheel.layout, "branching");
    // Banner must truthfully report "Saved; daemon reload failed" (`PRODUCT_SPEC.md` §P4)
    assert!(
        app.banner_ok().is_none(),
        "must not claim daemon adopted reload when IPC failed"
    );
    let err_banner = app.banner_err().expect("must set banner_err on failed IPC");
    assert!(
        err_banner.contains("Saved; daemon reload failed"),
        "unexpected error banner: {err_banner}"
    );

    let _ = fs::remove_dir_all(&dir);
}
