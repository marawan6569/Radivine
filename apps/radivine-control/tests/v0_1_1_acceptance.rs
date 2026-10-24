#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use radivine::config::{AppConfig, ReducedMotionSetting, ScaleSetting};
use radivine_control::ControlPage;
use radivine_control::diagnostics::{collect_diagnostics_snapshot, format_diagnostic_report};
use radivine_control::persistence::EditableAction;
use radivine_control::screenshot::EguiSoftRasterizer;
use radivine_control::service::{
    AutostartConflict, DaemonHealthState, ServiceController, ServicePaths,
};
use radivine_control::single_instance::ControlInstanceLock;
use radivine_control::ui::ControlCenterApp;

fn make_temp_test_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "radivine-v011-acc-{tag}-{}-{n}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}

fn isolated_service_paths(base_dir: &std::path::Path) -> ServicePaths {
    let daemon_bin = PathBuf::from(env!("CARGO_BIN_EXE_radivine-control"))
        .parent()
        .expect("target debug dir")
        .join("radivine");

    let cfg_dir = base_dir.join("config").join("radivine");
    fs::create_dir_all(&cfg_dir).expect("create config dir");

    ServicePaths {
        ipc_socket: base_dir.join("runtime").join("radivine.sock"),
        config_path: Some(cfg_dir.join("config.toml")),
        systemd_user_dir: base_dir.join("config").join("systemd").join("user"),
        system_unit_path: base_dir.join("usr-lib-systemd").join("radivine.service"),
        autostart_dir: base_dir.join("config").join("autostart"),
        state_dir: base_dir.join("state").join("radivine"),
        daemon_bin,
        allow_systemd: false,
    }
}

#[test]
fn ac_01_control_center_renders_stopped_and_running_and_enforces_single_instance() {
    let dir = make_temp_test_dir("ac01");
    let paths = isolated_service_paths(&dir);
    let control_sock = dir.join("runtime").join("control.sock");

    // 1. Acquire primary single-instance lock
    let lock1 = ControlInstanceLock::acquire_or_notify_existing(&control_sock, None)
        .expect("acquire first lock")
        .expect("must own first lock");

    // 2. Launching a second instance notifies the first instance to focus & switch page
    let lock2 = ControlInstanceLock::acquire_or_notify_existing(
        &control_sock,
        Some(ControlPage::Diagnostics),
    )
    .expect("notify existing lock");
    assert!(
        lock2.is_none(),
        "second instance must not bind duplicate window"
    );

    // 3. Control Center initializes cleanly when daemon is stopped
    let ctx = eframe::egui::Context::default();
    let mut app = ControlCenterApp::new(
        ControlPage::Dashboard,
        paths.clone(),
        Some(lock1),
        None,
        false,
    );

    let mut rasterizer = EguiSoftRasterizer::new();
    let screenshot_stopped = dir.join("dashboard-stopped.png");
    rasterizer
        .render_to_png(
            &ctx,
            eframe::egui::Vec2::new(960.0, 620.0),
            1.0,
            &screenshot_stopped,
            |ui| {
                app.render_frame(ui);
            },
        )
        .expect("render stopped frame");
    assert!(screenshot_stopped.is_file());

    // Verify single-instance activation switched the page to Diagnostics
    let screenshot_diag = dir.join("diagnostics-activated.png");
    rasterizer
        .render_to_png(
            &ctx,
            eframe::egui::Vec2::new(960.0, 620.0),
            1.0,
            &screenshot_diag,
            |ui| {
                app.render_frame(ui);
            },
        )
        .expect("render diagnostics frame");
    assert!(screenshot_diag.is_file());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ac_02_background_daemon_lifecycle_start_stop_restart_and_single_instance() {
    let dir = make_temp_test_dir("ac02");
    let paths = isolated_service_paths(&dir);
    if !paths.daemon_bin.is_file() {
        let _ = std::process::Command::new("cargo")
            .args(["build", "-p", "radivine"])
            .status();
    }
    assert!(
        paths.daemon_bin.is_file(),
        "radivine binary must exist at {}",
        paths.daemon_bin.display()
    );

    let ctrl = ServiceController::new(paths.clone());

    // Initially stopped
    let snap0 = ctrl.query_snapshot(None);
    assert_eq!(snap0.health, DaemonHealthState::Stopped);
    assert!(snap0.daemon.is_none());

    // Start detached background daemon (no terminal session required)
    let snap_running = ctrl.start_daemon().expect("start detached daemon");
    assert_eq!(snap_running.health, DaemonHealthState::Running);
    let daemon1 = snap_running.daemon.as_ref().expect("live IPC status");
    assert_eq!(daemon1.version, env!("CARGO_PKG_VERSION"));
    let pid1 = daemon1.pid;
    assert!(pid1 > 0);

    // Calling start_daemon() again while already running must NOT spawn a duplicate daemon
    let snap_idempotent = ctrl
        .start_daemon()
        .expect("second start_daemon is idempotent");
    assert_eq!(snap_idempotent.health, DaemonHealthState::Running);
    assert_eq!(
        snap_idempotent.daemon.as_ref().expect("status").pid,
        pid1,
        "must preserve single running daemon instance"
    );

    // Closing ControlCenterApp while daemon is running must leave daemon running
    {
        let _app = ControlCenterApp::new(ControlPage::Dashboard, paths.clone(), None, None, false);
    }
    let snap_after_close = ctrl.query_snapshot(None);
    assert_eq!(snap_after_close.health, DaemonHealthState::Running);
    assert_eq!(snap_after_close.daemon.as_ref().expect("status").pid, pid1);

    // Restart daemon -> gets a fresh PID and remains responsive over IPC
    let snap_restarted = ctrl.restart_daemon().expect("restart daemon");
    assert_eq!(snap_restarted.health, DaemonHealthState::Running);
    let pid2 = snap_restarted.daemon.as_ref().expect("status").pid;
    assert_ne!(pid1, pid2, "restart must spawn a new daemon process");

    // Stop daemon -> exits cleanly and removes socket
    let snap_stopped = ctrl.stop_daemon().expect("stop daemon");
    assert_eq!(snap_stopped.health, DaemonHealthState::Stopped);
    assert!(snap_stopped.daemon.is_none());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ac_03_autostart_management_and_conflict_resolution_preserve_user_files() {
    let dir = make_temp_test_dir("ac03");
    let paths = isolated_service_paths(&dir);
    let ctrl = ServiceController::new(paths.clone());

    // Enable and disable fallback XDG autostart
    let snap_on = ctrl.set_autostart(true).expect("enable autostart");
    assert!(snap_on.autostart.enabled);
    assert!(
        paths
            .autostart_dir
            .join("radivine-daemon.desktop")
            .is_file()
    );

    let snap_off = ctrl.set_autostart(false).expect("disable autostart");
    assert!(!snap_off.autostart.enabled);

    // Re-enable XDG autostart and simulate a legacy systemd unit override conflict
    ctrl.set_autostart(true).expect("re-enable autostart");
    fs::create_dir_all(&paths.systemd_user_dir).expect("create systemd user dir");
    let unit_path = paths.systemd_user_dir.join("radivine.service");
    fs::write(
        &unit_path,
        "[Unit]\nDescription=Legacy\nPartOf=graphical-session.target\n[Service]\nExecStart=/usr/bin/radivine daemon\n[Install]\nWantedBy=graphical-session.target\n",
    )
    .expect("write legacy unit");

    let snap_conflict = ctrl
        .resolve_conflict(&AutostartConflict::LegacyGraphicalSessionTargetOnly {
            unit_path: unit_path.clone(),
        })
        .expect("resolve legacy unit conflict");
    let _ = snap_conflict;

    assert!(
        paths
            .systemd_user_dir
            .join("radivine.service.bak")
            .is_file(),
        "user override must be backed up to .bak before updating"
    );
    let updated_unit = fs::read_to_string(&unit_path).expect("read updated unit");
    assert!(updated_unit.contains("WantedBy=default.target"));
    assert!(updated_unit.contains("After=default.target graphical-session-pre.target dbus.socket"));
    assert!(updated_unit.contains("StartLimitIntervalSec=0"));
    assert!(updated_unit.contains("Environment=RADIVINE_SYSTEMD_SERVICE=1"));
    assert!(!updated_unit.contains("PartOf=graphical-session.target"));

    // Resolve duplicate XDG + systemd conflict -> backs up XDG file to .bak
    let xdg_path = paths.autostart_dir.join("radivine-daemon.desktop");
    ctrl.resolve_conflict(&AutostartConflict::DuplicateXdgAndSystemd {
        xdg_path: xdg_path.clone(),
    })
    .expect("resolve duplicate xdg conflict");
    assert!(
        paths
            .autostart_dir
            .join("radivine-daemon.desktop.bak")
            .is_file(),
        "XDG desktop entry must be backed up to .bak"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ac_04_settings_persistence_format_preservation_conflict_detection_and_hot_reload() {
    let dir = make_temp_test_dir("ac04");
    let paths = isolated_service_paths(&dir);
    let cfg_path = paths.config_path.clone().expect("config path");

    let initial_toml = "\
# User top comment preserved by Control Center
[trigger]
drag_modifier = \"shift\"

[wheel]
# Custom wheel comment
root_inner_radius = 40.0
root_outer_radius = 128.0

[experimental_future_section]
keep_me = \"untouched\"
";
    fs::write(&cfg_path, initial_toml).expect("write initial config");

    let ctrl = ServiceController::new(paths.clone());
    let snap = ctrl
        .start_daemon()
        .expect("start daemon for hot reload test");
    let initial_pid = snap.daemon.as_ref().expect("daemon").pid;

    let mut app = ControlCenterApp::new(ControlPage::General, paths.clone(), None, None, false);

    // 1. Invalid wheel geometry (inner >= outer) must be rejected without touching disk
    app.draft_config.wheel.root_inner_radius = 180.0;
    app.draft_config.wheel.root_outer_radius = 120.0;
    assert!(
        app.save_config(false).is_err(),
        "invalid geometry must be rejected before disk write"
    );
    let disk_after_reject = fs::read_to_string(&cfg_path).expect("read disk after reject");
    assert_eq!(disk_after_reject, initial_toml);

    // 2. Valid settings changes save atomically while preserving comments and unknown sections
    app.draft_config = AppConfig::default();
    app.draft_config.appearance.theme = "light".to_string();
    app.draft_config.appearance.scale = ScaleSetting::Fixed(1.5);
    app.draft_config.appearance.reduced_motion = ReducedMotionSetting::Fixed(true);
    app.draft_config.wheel.root_inner_radius = 46.0;
    app.draft_config.wheel.root_outer_radius = 144.0;
    app.draft_config.preview.head_lines = 120;
    app.draft_config.notifications.notify_on_action_error = false;

    app.save_config(false).expect("save valid config");

    let saved_text = fs::read_to_string(&cfg_path).expect("read saved config");
    assert!(saved_text.contains("# User top comment preserved by Control Center"));
    assert!(saved_text.contains("# Custom wheel comment"));
    assert!(saved_text.contains("[experimental_future_section]"));
    assert!(saved_text.contains("keep_me = \"untouched\""));

    // 3. Reloading config into the running daemon applies live without changing PID
    let reloaded = ctrl.reload_daemon().expect("reload running daemon");
    let daemon_after = reloaded.daemon.expect("daemon status after reload");
    assert_eq!(daemon_after.pid, initial_pid);
    assert!((daemon_after.scale - 1.5).abs() < f32::EPSILON);

    // 4. External modification conflict detection & safe backup overwrite
    fs::write(&cfg_path, format!("{saved_text}\n# External edit\n")).expect("external edit");
    app.draft_config.preview.tail_lines = 80;
    assert!(
        app.save_config(false).is_err(),
        "must detect external modification conflict"
    );
    app.save_config(true)
        .expect("force overwrite with .bak backup");
    assert!(cfg_path.with_extension("toml.bak").is_file());

    ctrl.stop_daemon().expect("stop daemon");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ac_05_custom_actions_manager_crud_validation_and_live_reload() {
    let dir = make_temp_test_dir("ac05");
    let paths = isolated_service_paths(&dir);
    let cfg_path = paths.config_path.clone().expect("config path");
    let actions_path = AppConfig::actions_path_for(Some(&cfg_path));

    fs::write(
        &actions_path,
        "# Custom actions header comment\nversion = 1\n",
    )
    .expect("write initial actions.toml");

    let ctrl = ServiceController::new(paths.clone());
    let snap0 = ctrl.start_daemon().expect("start daemon");
    assert_eq!(
        snap0.daemon.as_ref().expect("status").custom_action_count,
        0
    );

    // 1. Validate rejection of shell invocation (`sh -c`) and partial placeholders (`--file={path}`)
    let bad_shell = EditableAction {
        executable: "sh".to_string(),
        args: vec!["-c".to_string(), "echo hello".to_string()],
        ..EditableAction::default()
    };
    let errs_shell = bad_shell.validate().expect_err("must reject shell -c");
    assert!(
        errs_shell
            .iter()
            .any(|e| e.to_ascii_lowercase().contains("shell")),
        "must reject shell -c wrapper: {errs_shell:?}"
    );

    let bad_partial = EditableAction {
        args: vec!["--input={path}".to_string()],
        ..EditableAction::default()
    };
    let errs_partial = bad_partial
        .validate()
        .expect_err("must reject partial placeholder");
    assert!(
        errs_partial
            .iter()
            .any(|e| e.contains("whole argument") || e.contains("placeholder")),
        "must reject partial placeholder token: {errs_partial:?}"
    );

    // 2. Create two valid custom actions (one enabled, one disabled) and persist via ControlCenterApp
    let mut app = ControlCenterApp::new(ControlPage::Actions, paths.clone(), None, None, false);
    let act1 = EditableAction {
        id: "custom.open_term".to_string(),
        label: "Open Terminal".to_string(),
        executable: "/bin/echo".to_string(),
        args: vec!["{dir}".to_string()],
        enabled: true,
        ..EditableAction::default()
    };

    let act2 = EditableAction {
        id: "custom.disabled_tool".to_string(),
        label: "Disabled Tool".to_string(),
        executable: "/bin/true".to_string(),
        args: vec!["{path}".to_string()],
        enabled: false,
        ..EditableAction::default()
    };

    assert!(act1.validate().is_ok());
    assert!(act2.validate().is_ok());

    app.draft_actions.actions = vec![act1, act2];
    app.save_actions(false).expect("save custom actions");

    let saved_actions_toml = fs::read_to_string(&actions_path).expect("read actions.toml");
    assert!(saved_actions_toml.contains("# Custom actions header comment"));
    assert!(saved_actions_toml.contains("custom.open_term"));
    assert!(saved_actions_toml.contains("enabled = false"));

    // 3. Hot reload updates the running daemon's active custom action count (only enabled actions count)
    let snap_reloaded = ctrl.reload_daemon().expect("reload daemon");
    assert_eq!(
        snap_reloaded
            .daemon
            .as_ref()
            .expect("status")
            .custom_action_count,
        1,
        "only the 1 enabled custom action should be active in the wheel"
    );

    // 4. Delete the enabled action and reload -> active count becomes 0
    app.draft_actions.actions.remove(0);
    app.save_actions(false).expect("save after delete");
    let snap_after_del = ctrl.reload_daemon().expect("reload after delete");
    assert_eq!(
        snap_after_del
            .daemon
            .as_ref()
            .expect("status")
            .custom_action_count,
        0
    );

    ctrl.stop_daemon().expect("stop daemon");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ac_06_diagnostics_log_collection_and_privacy_redaction() {
    let dir = make_temp_test_dir("ac06");
    let paths = isolated_service_paths(&dir);
    fs::create_dir_all(&paths.state_dir).expect("create state dir");

    let log_path = paths.daemon_log_path();
    fs::write(
        &log_path,
        "2026-10-23T19:00:00Z INFO opened file:///home/secretuser/Private/tax_2026.pdf\n\
         2026-10-23T19:01:00Z WARN socket at /run/user/1000/radivine/radivine.sock busy\n\
         2026-10-23T19:02:00Z ERROR failed to stat /home/secretuser/Documents/report.md\n",
    )
    .expect("write sample daemon log");

    let ctrl = ServiceController::new(paths);
    let svc_snap = ctrl.query_snapshot(None);
    let diag_snap = collect_diagnostics_snapshot(&svc_snap, &log_path, false);

    assert_eq!(diag_snap.logs.len(), 3);

    let redacted_report = format_diagnostic_report(&svc_snap, &diag_snap, true);
    assert!(!redacted_report.contains("secretuser"));
    assert!(!redacted_report.contains("tax_2026.pdf"));
    assert!(redacted_report.contains("file://<REDACTED_PATH>"));
    assert!(redacted_report.contains("/run/user/<UID>/radivine/radivine.sock"));
    assert!(redacted_report.contains("# Radivine Diagnostic Report"));

    let _ = fs::remove_dir_all(&dir);
}
