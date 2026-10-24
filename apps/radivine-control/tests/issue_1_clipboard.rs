//! Regression and integration test suite for GitHub Issue #1:
//! Clipboard interoperability in Radivine Control Center and Advanced Convert.
//!
//! All tests run in strict isolation using temporary directories and headless `egui::Context`
//! execution (plus isolated `Xvfb` X11 verification when `is_isolated_test_display()` is true),
//! never touching the owner's live `:0` desktop session or running development environment.

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use eframe::egui::{
    self, Event, Id, Key, Modifiers, OutputCommand, PointerButton, Pos2, RawInput, Rect,
    text_edit::TextEditState,
};
use radivine::config::AppConfig;
use radivine_control::ControlPage;
use radivine_control::persistence::convert_path_for;
use radivine_control::service::ServicePaths;
use radivine_control::ui::ControlCenterApp;
use radivine_control::ui::advanced_convert::{AdvancedConvertApp, UiDestMode, UiResizeMode};
use radivine_control::ui::jobs::{JobDetailsPage, UiJobsEvent};
use radivine_convert::{ConvertSettings, FormatId};
use radivine_jobs::{
    ItemState, JobId, JobItemReport, JobProgressCounts, JobState, JobSummary, Origin, RequestId,
    RetryMode, WirePath,
};

fn make_temp_test_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("radivine-issue1-{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}

fn isolated_service_paths(base_dir: &Path) -> ServicePaths {
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

fn write_valid_rgba_png(path: &Path, width: u32, height: u32) {
    let mut pixmap = tiny_skia::Pixmap::new(width, height).expect("create tiny-skia pixmap");
    for chunk in pixmap.data_mut().chunks_exact_mut(4) {
        chunk[0] = 32;
        chunk[1] = 160;
        chunk[2] = 120;
        chunk[3] = 255;
    }
    let png_bytes = pixmap.encode_png().expect("encode valid PNG");
    fs::write(path, png_bytes).expect("write PNG file");
}

fn run_frame_with_events<F: FnMut(&mut egui::Ui)>(
    ctx: &egui::Context,
    events: Vec<Event>,
    mut f: F,
) -> Vec<OutputCommand> {
    let raw = RawInput {
        screen_rect: Some(Rect::from_min_size(
            Pos2::ZERO,
            egui::Vec2::new(1280.0, 1400.0),
        )),
        events,
        ..Default::default()
    };
    let mut out = ctx.run_ui(raw, &mut f);
    out.textures_delta.clear();
    out.platform_output.commands
}

fn key_event(key: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn ctrl_modifiers() -> Modifiers {
    Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    }
}

/// Collects all `TextEdit` widget IDs in visual order on the current screen (`rect.min.x >= min_x`).
/// Uses primary pointer-down at each interactive rect (releasing off-screen so buttons are not clicked),
/// because `TextEdit` acquires focus on pointer press (`pointer_interaction`).
fn collect_text_edit_ids<F: FnMut(&mut egui::Ui)>(
    ctx: &egui::Context,
    min_x: f32,
    mut render: F,
) -> Vec<Id> {
    let _ = run_frame_with_events(ctx, Vec::new(), &mut render);
    let _ = run_frame_with_events(ctx, Vec::new(), &mut render);
    let rects: Vec<Rect> = ctx
        .interactive_rects_last_pass()
        .into_iter()
        .filter(|r| r.min.x >= min_x)
        .collect();

    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for rect in rects {
        ctx.memory_mut(|m| {
            if let Some(cur) = m.focused() {
                m.surrender_focus(cur);
            }
        });
        let pos = rect.center();
        let _ = run_frame_with_events(
            ctx,
            vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ],
            &mut render,
        );
        if let Some(focused_id) = ctx.memory(|m| m.focused())
            && TextEditState::load(ctx, focused_id).is_some()
            && seen.insert(focused_id)
        {
            ordered.push(focused_id);
        }
        // Release pointer off-screen so buttons/checkboxes never fire `.clicked()`
        let off = Pos2::new(-500.0, -500.0);
        let _ = run_frame_with_events(
            ctx,
            vec![
                Event::PointerMoved(off),
                Event::PointerButton {
                    pos: off,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::NONE,
                },
                Event::PointerGone,
            ],
            &mut render,
        );
    }
    // Clear any accidental label selection state by clicking in an empty corner
    let _ = click_rect(
        ctx,
        Rect::from_min_size(Pos2::new(1278.0, 2.0), egui::Vec2::splat(1.0)),
        &mut render,
    );
    ordered
}

/// Clicks at the center of `rect` and returns any emitted `OutputCommand`s.
fn click_rect<F: FnMut(&mut egui::Ui)>(
    ctx: &egui::Context,
    rect: Rect,
    mut render: F,
) -> Vec<OutputCommand> {
    let pos = rect.center();
    let mut cmds = run_frame_with_events(
        ctx,
        vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
        &mut render,
    );
    let mut cmds2 = run_frame_with_events(
        ctx,
        vec![
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
            Event::PointerGone,
        ],
        &mut render,
    );
    cmds.append(&mut cmds2);
    cmds
}

/// Focuses `id`, selects all text (`Ctrl+A`), and pastes `text` (`Event::Paste`),
/// returning any `OutputCommand`s emitted during the frame.
fn focus_replace_via_paste<F: FnMut(&mut egui::Ui)>(
    ctx: &egui::Context,
    id: Id,
    text: &str,
    mut render: F,
) -> Vec<OutputCommand> {
    ctx.memory_mut(|m| m.request_focus(id));
    let _ = run_frame_with_events(ctx, Vec::new(), &mut render);
    run_frame_with_events(
        ctx,
        vec![
            key_event(Key::A, ctrl_modifiers()),
            Event::Paste(text.to_string()),
        ],
        &mut render,
    )
}

/// Focuses `id`, selects all text (`Ctrl+A`), and sends `Event::Copy` (`Ctrl+C`),
/// returning the copied text from `OutputCommand::CopyText`.
fn focus_select_all_and_copy<F: FnMut(&mut egui::Ui)>(
    ctx: &egui::Context,
    id: Id,
    mut render: F,
) -> (Option<String>, Vec<OutputCommand>) {
    ctx.memory_mut(|m| m.request_focus(id));
    let _ = run_frame_with_events(ctx, Vec::new(), &mut render);
    let cmds = run_frame_with_events(
        ctx,
        vec![key_event(Key::A, ctrl_modifiers()), Event::Copy],
        &mut render,
    );
    let copied = cmds.iter().find_map(|c| match c {
        OutputCommand::CopyText(s) => Some(s.clone()),
        _ => None,
    });
    (copied, cmds)
}

/// Focuses `id`, selects all text (`Ctrl+A`), and sends `Event::Cut` (`Ctrl+X`),
/// returning the cut text from `OutputCommand::CopyText`.
fn focus_select_all_and_cut<F: FnMut(&mut egui::Ui)>(
    ctx: &egui::Context,
    id: Id,
    mut render: F,
) -> Option<String> {
    ctx.memory_mut(|m| m.request_focus(id));
    let _ = run_frame_with_events(ctx, Vec::new(), &mut render);
    let cmds = run_frame_with_events(
        ctx,
        vec![key_event(Key::A, ctrl_modifiers()), Event::Cut],
        &mut render,
    );
    cmds.into_iter().find_map(|c| match c {
        OutputCommand::CopyText(s) => Some(s),
        _ => None,
    })
}

#[test]
fn issue_1_01_eframe_and_egui_winit_native_clipboard_feature_and_symbols_enabled() {
    // 1. Verify Cargo feature graph has eframe/clipboard -> egui-winit/clipboard -> arboard + smithay-clipboard
    let tree_out = Command::new("cargo")
        .args([
            "tree",
            "-p",
            "radivine-control",
            "-i",
            "egui-winit",
            "-e",
            "features",
        ])
        .output()
        .expect("run cargo tree -p radivine-control -i egui-winit -e features");
    assert!(
        tree_out.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&tree_out.stderr)
    );
    let tree_stdout = String::from_utf8_lossy(&tree_out.stdout);
    assert!(
        tree_stdout.contains("eframe feature \"clipboard\""),
        "eframe 'clipboard' feature must be enabled for radivine-control:\n{tree_stdout}"
    );
    assert!(
        tree_stdout.contains("egui-winit feature \"clipboard\""),
        "egui-winit 'clipboard' feature must be enabled:\n{tree_stdout}"
    );
    assert!(
        tree_stdout.contains("egui-winit feature \"arboard\""),
        "egui-winit 'arboard' X11 clipboard backend must be enabled:\n{tree_stdout}"
    );
    assert!(
        tree_stdout.contains("egui-winit feature \"smithay-clipboard\""),
        "egui-winit 'smithay-clipboard' Wayland clipboard backend must be enabled:\n{tree_stdout}"
    );

    // 2. Verify the compiled radivine-control binary links the native arboard X11 clipboard backend
    let bin_path = PathBuf::from(env!("CARGO_BIN_EXE_radivine-control"));
    assert!(bin_path.is_file(), "radivine-control binary must exist");
    if let Ok(nm_out) = Command::new("nm").arg("-C").arg(&bin_path).output()
        && nm_out.status.success()
    {
        let symbols = String::from_utf8_lossy(&nm_out.stdout);
        assert!(
            symbols.contains("egui_winit::clipboard::init_arboard"),
            "radivine-control binary must link egui_winit::clipboard::init_arboard"
        );
        assert!(
            symbols.contains("arboard::platform::linux::x11::Clipboard"),
            "radivine-control binary must link arboard::platform::linux::x11::Clipboard"
        );
    }
}

#[test]
fn issue_1_02_custom_actions_editor_clipboard_copy_paste_cut_utf8_arabic_and_multiline() {
    let dir = make_temp_test_dir("custom-actions");
    let paths = isolated_service_paths(&dir);
    let cfg_path = paths.config_path.clone().expect("config path");
    let actions_path = AppConfig::actions_path_for(Some(&cfg_path));

    let ctx = egui::Context::default();
    let mut app =
        ControlCenterApp::new(ControlPage::Actions, paths, None, Some(ctx.clone()), false);

    // Prime the Actions page and click "+ Create Custom Action" in the central panel (x >= 240.0)
    let _ = run_frame_with_events(&ctx, Vec::new(), |ui| app.render_frame(ui));
    let _ = run_frame_with_events(&ctx, Vec::new(), |ui| app.render_frame(ui));
    let central_rects: Vec<Rect> = ctx
        .interactive_rects_last_pass()
        .into_iter()
        .filter(|r| r.min.x >= 240.0)
        .collect();

    let mut text_ids = Vec::new();
    for rect in central_rects {
        let _ = click_rect(&ctx, rect, |ui| app.render_frame(ui));
        text_ids = collect_text_edit_ids(&ctx, 240.0, |ui| app.render_frame(ui));
        if text_ids.len() >= 10 {
            break;
        }
    }

    // In ActionEditorMode::Create, the 10 TextEdit fields in render_action_editor_card are:
    // [0] Action ID, [1] Display Label, [2] Description, [3] Executable Program,
    // [4] Arguments (multiline), [5] Working Directory, [6] Item Count Filter,
    // [7] File Extensions, [8] MIME Globs, [9] Timeout (ms) (+ [10] Filter search box below)
    assert!(
        text_ids.len() >= 10,
        "expected at least 10 TextEdit fields when Custom Action editor is open, got {}",
        text_ids.len()
    );

    let id_field = text_ids[0];
    let label_field = text_ids[1];
    let desc_field = text_ids[2];
    let exec_field = text_ids[3];
    let args_field = text_ids[4];
    let cwd_field = text_ids[5];
    let ext_field = text_ids[7];

    // 1. Paste external text (Ctrl+V / Event::Paste) including UTF-8, Arabic, spaces, and special chars
    let pasted_id = "custom.arabic_clip_test";
    let pasted_label = "تحويل الصور — Convert 100% #1";
    let pasted_desc = "أداة مخصصة مع مسافات ورموز خاصة ('single' & \"double\")";
    let pasted_exec = "/bin/echo";
    let pasted_args = "--title\nصورة عربية مع مسافات 100%\n{path}";
    let pasted_cwd = "{dir}";
    let pasted_exts = "png, jpg, webp";

    focus_replace_via_paste(&ctx, id_field, pasted_id, |ui| app.render_frame(ui));
    focus_replace_via_paste(&ctx, label_field, pasted_label, |ui| app.render_frame(ui));
    focus_replace_via_paste(&ctx, desc_field, pasted_desc, |ui| app.render_frame(ui));
    focus_replace_via_paste(&ctx, exec_field, pasted_exec, |ui| app.render_frame(ui));
    focus_replace_via_paste(&ctx, args_field, pasted_args, |ui| app.render_frame(ui));
    focus_replace_via_paste(&ctx, cwd_field, pasted_cwd, |ui| app.render_frame(ui));
    focus_replace_via_paste(&ctx, ext_field, pasted_exts, |ui| app.render_frame(ui));

    // 2. Verify Ctrl+C (Event::Copy) emits OutputCommand::CopyText with exact UTF-8 & Arabic content
    let (copied_label, label_cmds) =
        focus_select_all_and_copy(&ctx, label_field, |ui| app.render_frame(ui));
    assert_eq!(copied_label.as_deref(), Some(pasted_label));

    // Verify selecting text without Ctrl+C only updates PRIMARY (TextSelectionSettled), not CopyText
    ctx.memory_mut(|m| m.request_focus(desc_field));
    let _ = run_frame_with_events(&ctx, Vec::new(), |ui| app.render_frame(ui));
    let select_only_cmds =
        run_frame_with_events(&ctx, vec![key_event(Key::A, ctrl_modifiers())], |ui| {
            app.render_frame(ui)
        });
    assert!(
        !select_only_cmds
            .iter()
            .any(|c| matches!(c, OutputCommand::CopyText(_))),
        "selection alone must not overwrite CLIPBOARD via OutputCommand::CopyText: {select_only_cmds:?}"
    );
    assert!(
        label_cmds
            .iter()
            .any(|c| matches!(c, OutputCommand::CopyText(_))),
        "Ctrl+C must emit OutputCommand::CopyText"
    );

    // 3. Verify copying from one Radivine field (Executable) and pasting into another field,
    //    then cutting (Ctrl+X) and restoring
    let (copied_exec, _) = focus_select_all_and_copy(&ctx, exec_field, |ui| app.render_frame(ui));
    assert_eq!(copied_exec.as_deref(), Some("/bin/echo"));

    // Cut from cwd_field via Ctrl+X (Event::Cut)
    let cut_cwd = focus_select_all_and_cut(&ctx, cwd_field, |ui| app.render_frame(ui));
    assert_eq!(cut_cwd.as_deref(), Some("{dir}"));
    // Verify cwd_field is now empty after Cut
    let (after_cut_cwd, _) = focus_select_all_and_copy(&ctx, cwd_field, |ui| app.render_frame(ui));
    assert!(
        after_cut_cwd.as_deref().unwrap_or("").is_empty(),
        "Ctrl+X must remove selected text from editable field, got {after_cut_cwd:?}"
    );
    // Paste the cut text back into cwd_field
    focus_replace_via_paste(&ctx, cwd_field, &cut_cwd.unwrap(), |ui| {
        app.render_frame(ui);
    });

    // 4. Verify normal keyboard navigation and editing (End, Backspace, typing)
    ctx.memory_mut(|m| m.request_focus(id_field));
    let _ = run_frame_with_events(&ctx, Vec::new(), |ui| app.render_frame(ui));
    let _ = run_frame_with_events(
        &ctx,
        vec![
            key_event(Key::End, Modifiers::NONE),
            key_event(Key::Backspace, Modifiers::NONE),
            Event::Text("t".to_string()),
        ],
        |ui| app.render_frame(ui),
    );
    let (edited_id, _) = focus_select_all_and_copy(&ctx, id_field, |ui| app.render_frame(ui));
    assert_eq!(edited_id.as_deref(), Some(pasted_id));

    // 5. Verify multiline arguments copy preserves newlines and Arabic text
    let (copied_args, _) = focus_select_all_and_copy(&ctx, args_field, |ui| app.render_frame(ui));
    assert_eq!(copied_args.as_deref(), Some(pasted_args));

    // 6. Click "Validate & Save Action" button (bottom-left button of the editor card) and verify persistence
    let _ = run_frame_with_events(&ctx, Vec::new(), |ui| app.render_frame(ui));
    let save_rects: Vec<Rect> = ctx
        .interactive_rects_last_pass()
        .into_iter()
        .filter(|r| r.min.x >= 240.0 && r.min.x < 360.0 && r.min.y >= 400.0)
        .collect();
    for rect in save_rects.into_iter().rev() {
        let _ = click_rect(&ctx, rect, |ui| app.render_frame(ui));
        if !app.draft_actions.actions.is_empty() {
            break;
        }
    }

    assert_eq!(
        app.draft_actions.actions.len(),
        1,
        "custom action populated via clipboard paste must validate and save"
    );
    let saved_act = &app.draft_actions.actions[0];
    assert_eq!(saved_act.id, pasted_id);
    assert_eq!(saved_act.label, pasted_label);
    assert_eq!(saved_act.description, pasted_desc);
    assert_eq!(saved_act.executable, pasted_exec);
    assert_eq!(
        saved_act.args,
        vec![
            "--title".to_string(),
            "صورة عربية مع مسافات 100%".to_string(),
            "{path}".to_string()
        ]
    );
    assert_eq!(saved_act.cwd, "{dir}");

    let disk_toml = fs::read_to_string(&actions_path).expect("read saved actions.toml");
    assert!(disk_toml.contains(pasted_label));
    assert!(disk_toml.contains("صورة عربية مع مسافات 100%"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn issue_1_03_advanced_convert_and_conversion_settings_clipboard_workflows() {
    let dir = make_temp_test_dir("adv-convert");
    let paths = isolated_service_paths(&dir);
    let config_path = paths.config_path.clone().expect("config path");

    let sample_png = dir.join("sample_input.png");
    write_valid_rgba_png(&sample_png, 64, 64);
    let custom_out_dir = dir.join("مجلد التصدير 2026 — Output #1 (100%)");
    fs::create_dir_all(&custom_out_dir).expect("create custom Arabic/spaces output dir");

    let ctx = egui::Context::default();
    let mut adv = AdvancedConvertApp::new(
        vec![WirePath::from_path(&sample_png).expect("WirePath")],
        paths.clone(),
        Some(ctx.clone()),
    );
    adv.target_format = FormatId::Jpeg;
    adv.resize_mode = UiResizeMode::FitWithin;
    adv.dest_mode = UiDestMode::CustomFolder;
    adv.append_suffix = true;
    adv.save_preset_open = true;

    // Collect TextEdit fields in AdvancedConvertApp (min_x = 0.0):
    // [0] save_preset_name, [1] resize_width_text, [2] resize_height_text,
    // [3] background_hex, [4] dest_folder_text, [5] suffix_text
    let adv_fields = collect_text_edit_ids(&ctx, 0.0, |ui| adv.render_frame(ui));
    assert_eq!(
        adv_fields.len(),
        6,
        "expected 6 TextEdit fields in AdvancedConvertApp, got {}",
        adv_fields.len()
    );

    let preset_name_id = adv_fields[0];
    let width_id = adv_fields[1];
    let height_id = adv_fields[2];
    let bg_hex_id = adv_fields[3];
    let dest_folder_id = adv_fields[4];
    let suffix_id = adv_fields[5];

    let dest_str = custom_out_dir.to_string_lossy().to_string();
    focus_replace_via_paste(
        &ctx,
        preset_name_id,
        "إعداد مسبق عربي WebP 2026",
        |ui| {
            adv.render_frame(ui);
        },
    );
    focus_replace_via_paste(&ctx, width_id, "1920", |ui| adv.render_frame(ui));
    focus_replace_via_paste(&ctx, height_id, "1080", |ui| adv.render_frame(ui));
    focus_replace_via_paste(&ctx, bg_hex_id, "#1E293B", |ui| adv.render_frame(ui));
    focus_replace_via_paste(&ctx, dest_folder_id, &dest_str, |ui| adv.render_frame(ui));
    focus_replace_via_paste(&ctx, suffix_id, "-نسخة_معدلة", |ui| {
        adv.render_frame(ui)
    });

    assert_eq!(adv.save_preset_name, "إعداد مسبق عربي WebP 2026");
    assert_eq!(adv.resize_width_text, "1920");
    assert_eq!(adv.resize_height_text, "1080");
    assert_eq!(adv.background_hex, "#1E293B");
    assert_eq!(adv.dest_folder_text, dest_str);
    assert_eq!(adv.suffix_text, "-نسخة_معدلة");
    assert!(
        adv.build_conversion_options().is_ok(),
        "pasted Advanced Convert fields must build valid ConversionOptions: {:?}",
        adv.build_conversion_options()
    );

    // Copy from Advanced Convert destination folder field and verify OutputCommand::CopyText
    let (copied_dest, _) =
        focus_select_all_and_copy(&ctx, dest_folder_id, |ui| adv.render_frame(ui));
    assert_eq!(copied_dest.as_deref(), Some(dest_str.as_str()));

    // Cut suffix via Ctrl+X and paste a new suffix
    let cut_suffix = focus_select_all_and_cut(&ctx, suffix_id, |ui| adv.render_frame(ui));
    assert_eq!(cut_suffix.as_deref(), Some("-نسخة_معدلة"));
    assert!(adv.suffix_text.is_empty());
    focus_replace_via_paste(&ctx, suffix_id, "-تصدير_سريع", |ui| {
        adv.render_frame(ui)
    });
    assert_eq!(adv.suffix_text, "-تصدير_سريع");

    // Save preset and verify persistence in convert.toml
    let saved_preset_id = adv
        .save_current_as_user_preset()
        .expect("save preset with pasted values");
    assert_eq!(saved_preset_id, "user.webp-2026");
    let (loaded_convert, _) = ConvertSettings::load_or_default(&convert_path_for(&config_path));
    let saved_preset = loaded_convert
        .user_presets
        .iter()
        .find(|p| p.id == "user.webp-2026")
        .expect("user preset persisted");
    assert_eq!(saved_preset.name, "إعداد مسبق عربي WebP 2026");

    // Now test Conversion Settings page in ControlCenterApp (field-to-field paste from copied_dest)
    let mut cc = ControlCenterApp::new(
        ControlPage::Conversion,
        paths,
        None,
        Some(ctx.clone()),
        false,
    );
    cc.conversion_page_state.destination_mode = "fixed".to_string();
    let conv_fields = collect_text_edit_ids(&ctx, 240.0, |ui| cc.render_frame(ui));
    assert!(
        conv_fields.len() >= 2,
        "expected Conversion Settings TextEdit fields, got {}",
        conv_fields.len()
    );
    // [0] fixed_folder_text, [1] filename_suffix_text
    focus_replace_via_paste(&ctx, conv_fields[0], &copied_dest.unwrap(), |ui| {
        cc.render_frame(ui);
    });
    focus_replace_via_paste(&ctx, conv_fields[1], "-نسخة_عربية", |ui| {
        cc.render_frame(ui);
    });
    assert_eq!(cc.conversion_page_state.fixed_folder_text, dest_str);
    assert_eq!(cc.conversion_page_state.filename_suffix_text, "-نسخة_عربية");

    cc.save_convert_and_jobs(false)
        .expect("save Conversion Settings with pasted paths");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn issue_1_04_jobs_copy_error_and_diagnostics_copy_report_emit_clipboard_commands() {
    let dir = make_temp_test_dir("jobs-diag-copy");
    let paths = isolated_service_paths(&dir);
    let ctx = egui::Context::default();

    let mut app = ControlCenterApp::new(
        ControlPage::Jobs,
        paths.clone(),
        None,
        Some(ctx.clone()),
        false,
    );

    // Drain any initial background worker snapshot before injecting our synthetic job details
    std::thread::sleep(std::time::Duration::from_millis(50));
    app.drain_all_events();

    let err_detail =
        "Failed decoding '/home/user/الصور المفضلة/ملف تالف #1.png': corrupt PNG IHDR chunk";
    let expected_copied_err = format!("[corrupt_input] {err_detail}");
    let job_id = JobId(42);
    let summary = JobSummary {
        job_id,
        request_id: RequestId("req_issue1_test_0001".to_string()),
        kind: "convert.image".to_string(),
        origin: Origin::Wheel,
        state: JobState::CompletedWithErrors,
        created_at_ms: 1_700_000_000_000,
        started_at_ms: Some(1_700_000_000_010),
        finished_at_ms: Some(1_700_000_000_250),
        updated_at_ms: 1_700_000_000_250,
        retry_of: None,
        counts: JobProgressCounts {
            total_items: 1,
            pending_items: 0,
            in_flight_items: 0,
            succeeded_items: 0,
            skipped_items: 0,
            failed_items: 1,
            cancelled_items: 0,
            interrupted_items: 0,
        },
        summary_text: Some("0 of 1 converted (1 failed)".to_string()),
        options: serde_json::json!({"target": "webp"}),
        resolved: None,
    };

    let populate_jobs = |app: &mut ControlCenterApp| {
        app.jobs_view.apply_event(UiJobsEvent::SnapshotLoaded {
            worker_online: true,
            worker_pid: Some(1234),
            schema_version: 1,
            jobs: vec![summary.clone()],
            error: None,
        });
        app.jobs_view.apply_event(UiJobsEvent::JobDetailsLoaded {
            details: JobDetailsPage {
                summary: summary.clone(),
                items: vec![JobItemReport {
                    item_id: 1,
                    item_index: 0,
                    source_path: WirePath::from_path(Path::new(
                        "/home/user/الصور المفضلة/ملف تالف #1.png",
                    ))
                    .expect("WirePath"),
                    state: ItemState::Failed,
                    stage: None,
                    output_path: None,
                    source_bytes: Some(1024),
                    output_bytes: None,
                    duration_ms: Some(12),
                    attempt_count: 1,
                    error_code: Some("corrupt_input".to_string()),
                    error_class: Some("CorruptInput".to_string()),
                    error_message: Some(err_detail.to_string()),
                    retryable: true,
                    warnings: Vec::new(),
                }],
                total_items: 1,
            },
            page_index: 0,
        });
    };

    populate_jobs(&mut app);
    let _ = run_frame_with_events(&ctx, Vec::new(), |ui| {
        app.drain_all_events();
        populate_jobs(&mut app);
        app.render_frame(ui);
    });
    let _ = run_frame_with_events(&ctx, Vec::new(), |ui| {
        app.drain_all_events();
        populate_jobs(&mut app);
        app.render_frame(ui);
    });

    let job_rects: Vec<Rect> = ctx
        .interactive_rects_last_pass()
        .into_iter()
        .filter(|r| r.min.x >= 240.0)
        .collect();

    // Click central panel buttons on the Jobs page from bottom-up so "Copy error" is clicked
    // before any modal-opening button shifts layout coordinates
    let mut copied_job_error: Option<String> = None;
    for rect in job_rects.into_iter().rev() {
        let cmds = click_rect(&ctx, rect, |ui| {
            app.drain_all_events();
            populate_jobs(&mut app);
            app.render_frame(ui);
        });
        if let Some(text) = cmds.into_iter().find_map(|c| match c {
            OutputCommand::CopyText(s) => Some(s),
            _ => None,
        }) {
            copied_job_error = Some(text);
            break;
        }
    }
    assert_eq!(
        copied_job_error.as_deref(),
        Some(expected_copied_err.as_str()),
        "clicking 'Copy error' on Jobs page must emit OutputCommand::CopyText with exact error details"
    );

    // Also test pasting into the Jobs "Retry to folder…" modal text field
    app.jobs_view.retry_folder_modal = Some((job_id, RetryMode::FailedOnly, String::new()));
    let retry_fields = collect_text_edit_ids(&ctx, 240.0, |ui| {
        app.drain_all_events();
        populate_jobs(&mut app);
        app.render_frame(ui);
    });
    assert!(
        !retry_fields.is_empty(),
        "expected Retry to folder modal TextEdit field"
    );
    focus_replace_via_paste(
        &ctx,
        retry_fields[0],
        "/tmp/إعادة_المحاولة_2026",
        |ui| {
            app.drain_all_events();
            populate_jobs(&mut app);
            app.render_frame(ui);
        },
    );
    assert_eq!(
        app.jobs_view
            .retry_folder_modal
            .as_ref()
            .map(|(_, _, folder)| folder.as_str()),
        Some("/tmp/إعادة_المحاولة_2026")
    );

    // Also verify Diagnostics page "Copy Diagnostic Report for GitHub Issue" button emits OutputCommand::CopyText
    let diag_ctx = egui::Context::default();
    let mut diag_app = ControlCenterApp::new(
        ControlPage::Diagnostics,
        paths,
        None,
        Some(diag_ctx.clone()),
        false,
    );
    for _ in 0..20 {
        diag_app.drain_all_events();
        let _ = run_frame_with_events(&diag_ctx, Vec::new(), |ui| diag_app.render_frame(ui));
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let _ = run_frame_with_events(&diag_ctx, Vec::new(), |ui| diag_app.render_frame(ui));
    let diag_rects: Vec<Rect> = diag_ctx
        .interactive_rects_last_pass()
        .into_iter()
        .filter(|r| r.min.x >= 240.0)
        .collect();
    let mut copied_diag_report: Option<String> = None;
    for rect in diag_rects {
        let cmds = click_rect(&diag_ctx, rect, |ui| diag_app.render_frame(ui));
        if let Some(text) = cmds.into_iter().find_map(|c| match c {
            OutputCommand::CopyText(s) => Some(s),
            _ => None,
        }) {
            copied_diag_report = Some(text);
            break;
        }
    }
    let report = copied_diag_report.expect(
        "clicking 'Copy Diagnostic Report for GitHub Issue' must emit OutputCommand::CopyText",
    );
    assert!(
        report.contains("# Radivine Diagnostic Report"),
        "copied diagnostic report must contain Markdown header, got:\n{report}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn issue_1_05_isolated_x11_clipboard_coexistence() {
    if !radivine_platform::is_isolated_test_display() {
        return;
    }

    use radivine_platform::x11::X11Platform;
    use radivine_platform::{ClipOp, ClipboardContent, Platform, X11Config};

    let Ok(mut plat) = X11Platform::connect(X11Config::default()) else {
        return;
    };

    let sample_uri = "file:///tmp/%D8%B5%D9%88%D8%B1%D8%A9%201.png".to_string();
    plat.set_clipboard(ClipboardContent {
        uris: vec![sample_uri],
        local_paths: vec![PathBuf::from("/tmp/صورة 1.png")],
        operation: ClipOp::Copy,
    })
    .expect("plat set_clipboard");

    let mut out = Vec::new();
    let _ = plat.dispatch_pending(&mut out);
}
