//! Regression and integration test suite for Arabic system-font fallback, glyph coverage,
//! OpenType contextual joining, diacritics, and mixed-direction strings in `radivine-control`.

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use eframe::egui::{
    self, Color32, Event, FontFamily, FontId, Key, Modifiers, PointerButton, Pos2, RawInput, Rect,
    epaint::text::MissingGlyphPolicy, text_edit::TextEditState,
};
use radivine_control::ControlPage;
use radivine_control::service::ServicePaths;
use radivine_control::ui::advanced_convert::{AdvancedConvertApp, UiDestMode};
use radivine_control::ui::{ControlCenterApp, install_inter_font};
use radivine_jobs::WirePath;

fn make_temp_test_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("radivine-arabic-{tag}-{}-{n}", std::process::id()));
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

fn run_frame_with_events<F: FnMut(&mut egui::Ui)>(
    ctx: &egui::Context,
    events: Vec<Event>,
    mut f: F,
) {
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
}

#[test]
fn arabic_01_eframe_system_fonts_feature_and_provider_enabled_in_control_and_viewer() {
    for pkg in ["radivine-control", "radivine-viewer"] {
        let tree_out = Command::new("cargo")
            .args([
                "tree",
                "-p",
                pkg,
                "-i",
                "egui_system_font_provider",
                "-e",
                "features",
            ])
            .output()
            .expect("run cargo tree for egui_system_font_provider");
        assert!(
            tree_out.status.success(),
            "cargo tree failed for {pkg}: {}",
            String::from_utf8_lossy(&tree_out.stderr)
        );
        let stdout = String::from_utf8_lossy(&tree_out.stdout);
        assert!(
            stdout.contains("eframe feature \"system_fonts\""),
            "{pkg} must enable eframe 'system_fonts' feature:\n{stdout}"
        );
        assert!(
            stdout.contains("egui_system_font_provider"),
            "{pkg} must link egui_system_font_provider:\n{stdout}"
        );
    }
}

#[test]
fn arabic_02_system_font_fallback_resolves_arabic_diacritics_mixed_and_preserves_inter_latin() {
    // 1. Without install_inter_font / SystemFontProvider, default egui fonts lack Arabic glyphs
    let bare_ctx = egui::Context::default();
    let prop_id = FontId::new(14.0, FontFamily::Proportional);
    let mono_id = FontId::new(13.0, FontFamily::Monospace);
    run_frame_with_events(&bare_ctx, Vec::new(), |ui| {
        ui.fonts_mut(|f| {
            assert!(
                !f.has_glyph(&prop_id, 'ع'),
                "default egui fonts without system_fonts do not contain Arabic glyphs"
            );
        });
    });

    // 2. With install_inter_font + MissingGlyphPolicy::Panic, any missing glyph / tofu box panics
    let ctx = egui::Context::default();
    ctx.set_missing_glyph_policy(MissingGlyphPolicy::Panic);
    install_inter_font(&ctx);

    let samples = [
        // Pure Arabic
        "العربية",
        "تحويل الصور",
        "إجراءات مخصصة",
        // Arabic with full diacritics (tashkeel / harakat)
        "مُحَوِّلُ الصُّوَرِ السَّرِيعُ",
        // Arabic numerals + ASCII numerals
        "٠١٢٣٤٥٦٧٨٩ 0123456789",
        // Mixed Arabic / English / numbers / symbols
        "تحويل الصور — Convert 100% #1 (WebP & PNG)",
        // Realistic Linux filenames and paths
        "/home/user/الصور المفضلة/ملف_عربي_01.png",
        // Arabic punctuation
        "هل تريد المتابعة؟ نعم، بالتأكيد؛ شكراً!",
    ];

    run_frame_with_events(&ctx, Vec::new(), |ui| {
        ui.fonts_mut(|f| {
            for sample in samples {
                assert!(
                    f.has_glyphs(&prop_id, sample),
                    "Proportional family must resolve all glyphs in {sample:?}"
                );
                assert!(
                    f.has_glyphs(&mono_id, sample),
                    "Monospace family must resolve all glyphs in {sample:?}"
                );
            }

            // Verify English / Latin characters are still served by primary "Inter" font
            let chars_map = f.characters(&FontFamily::Proportional);
            for ch in ['A', 'Z', 'a', 'z', '0', '9'] {
                let owners = chars_map
                    .get(&ch)
                    .expect("Latin char must be in Proportional family");
                assert_eq!(
                    owners.first().map(String::as_str),
                    Some("Inter"),
                    "English/Latin character {ch:?} must still use primary 'Inter' font, got {owners:?}"
                );
            }
        });

        let discovered = ui.fonts(|f| {
            f.discovered_fonts()
                .iter()
                .map(|ins| ins.name.clone())
                .collect::<Vec<_>>()
        });
        assert!(
            discovered.iter().any(|name| name.starts_with("system:")),
            "SystemFontProvider must discover at least one system fallback font for Arabic, got {discovered:?}"
        );

        // Lay out and paint all samples in both read-only labels and editable TextEdit widgets
        // under MissingGlyphPolicy::Panic (panics if any character renders as a tofu box)
        for sample in samples {
            ui.label(sample);
            let mut editable = sample.to_string();
            ui.text_edit_singleline(&mut editable);
        }
    });
}

#[test]
fn arabic_03_contextual_joining_and_lam_alef_ligature_shaping_verification() {
    let ctx = egui::Context::default();
    ctx.set_missing_glyph_policy(MissingGlyphPolicy::Panic);
    install_inter_font(&ctx);
    let font_id = FontId::new(16.0, FontFamily::Proportional);

    run_frame_with_events(&ctx, Vec::new(), |ui| {
        ui.fonts_mut(|f| {
            // 1. Verify Lam-Alef ("لا" = U+0644 U+0627) forms a connected ligature glyph:
            //    2 input chars -> 1 visible ligature glyph (uv_rect non-empty) + 1 zero-width continuation glyph
            let gal_lam_alef = f.layout_no_wrap("لا".to_string(), font_id.clone(), Color32::WHITE);
            let glyphs_la = &gal_lam_alef.rows[0].row.glyphs;
            assert_eq!(
                glyphs_la.len(),
                2,
                "egui preserves 1 glyph slot per char for cursor indexing"
            );
            let visible_la = glyphs_la
                .iter()
                .filter(|g| !g.uv_rect.is_nothing() && g.advance_width > 0.0)
                .count();
            assert_eq!(
                visible_la, 1,
                "Lam + Alef ('لا') must shape into a single connected ligature glyph via harfrust GSUB rlig"
            );

            // 2. Verify contextual joining ('ب' isolated in "ب" vs medial in "تبت"):
            //    In OpenType Arabic shaping, isolated Ba and medial Ba are distinct glyphs with different UV rects / widths
            let gal_isolated_ba = f.layout_no_wrap("ب".to_string(), font_id.clone(), Color32::WHITE);
            let gal_joined_tabat =
                f.layout_no_wrap("تبت".to_string(), font_id.clone(), Color32::WHITE);

            let iso_ba_uv = gal_isolated_ba.rows[0].row.glyphs[0].uv_rect;
            assert!(
                !iso_ba_uv.is_nothing(),
                "isolated 'ب' must allocate a non-empty glyph in the atlas"
            );

            // In "تبت" (3 connected letters: final ت, medial ب, initial ت in visual order),
            // the middle visual glyph is medial 'ب'
            let joined_visible: Vec<_> = gal_joined_tabat.rows[0]
                .row
                .glyphs
                .iter()
                .filter(|g| !g.uv_rect.is_nothing())
                .collect();
            assert_eq!(
                joined_visible.len(),
                3,
                "'تبت' must produce 3 visible shaped glyphs"
            );
            let medial_ba_uv = joined_visible[1].uv_rect;
            assert_ne!(
                 (iso_ba_uv.min, iso_ba_uv.max),
                 (medial_ba_uv.min, medial_ba_uv.max),
                "medial 'ب' in 'تبت' must use its joined medial glyph rather than isolated 'ب'"
            );
        });
    });
}

#[test]
fn arabic_04_custom_actions_filter_and_editor_and_advanced_convert_render_arabic_without_tofu() {
    let dir = make_temp_test_dir("control-filter");
    let paths = isolated_service_paths(&dir);

    let ctx = egui::Context::default();
    ctx.set_missing_glyph_policy(MissingGlyphPolicy::Panic);

    let mut app = ControlCenterApp::new(
        ControlPage::Actions,
        paths.clone(),
        None,
        Some(ctx.clone()),
        false,
    );
    app.draft_actions
        .actions
        .push(radivine_control::persistence::EditableAction {
            id: "custom.arabic_filter_demo".to_string(),
            label: "تحويل الصور — مُحَوِّلُ الصُّوَرِ 100%".to_string(),
            description: "إجراء مخصص لاختبار البحث والفلترة باللغة العربية مع التشكيل".to_string(),
            executable: "/bin/echo".to_string(),
            args: vec![
                "--title".to_string(),
                "صورة عربية.png".to_string(),
                "{path}".to_string(),
            ],
            cwd: "{dir}".to_string(),
            ..Default::default()
        });

    // 1. Prime the Actions page (renders the Custom Actions filter TextEdit)
    run_frame_with_events(&ctx, Vec::new(), |ui| app.render_frame(ui));
    run_frame_with_events(&ctx, Vec::new(), |ui| app.render_frame(ui));

    // Find the Custom Actions filter TextEdit in the central panel (x >= 240.0)
    let central_rects: Vec<Rect> = ctx
        .interactive_rects_last_pass()
        .into_iter()
        .filter(|r| r.min.x >= 240.0)
        .collect();

    let mut filter_id = None;
    for rect in central_rects {
        let pos = rect.center();
        run_frame_with_events(
            &ctx,
            vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ],
            |ui| app.render_frame(ui),
        );
        if let Some(id) = ctx.memory(|m| m.focused())
            && TextEditState::load(&ctx, id).is_some()
        {
            filter_id = Some(id);
            let off = Pos2::new(-500.0, -500.0);
            run_frame_with_events(
                &ctx,
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
                |ui| app.render_frame(ui),
            );
            break;
        }
        let off = Pos2::new(-500.0, -500.0);
        run_frame_with_events(
            &ctx,
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
            |ui| app.render_frame(ui),
        );
    }

    let focused_id = filter_id.expect("filter TextEdit must be found and focused");
    ctx.memory_mut(|m| m.request_focus(focused_id));
    run_frame_with_events(&ctx, Vec::new(), |ui| app.render_frame(ui));

    // 2. Paste Arabic text, diacritics, mixed Arabic/English, and filenames into the Custom Actions filter
    //    With MissingGlyphPolicy::Panic active, any missing glyph box will immediately panic!
    for query in [
        "تحويل الصور",
        "مُحَوِّلُ الصُّوَرِ 2026",
        "Convert — تحويل 100% #1",
        "/home/user/الصور المفضلة/ملف_01.png",
    ] {
        run_frame_with_events(
            &ctx,
            vec![
                Event::Key {
                    key: Key::A,
                    physical_key: Some(Key::A),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers {
                        ctrl: true,
                        command: true,
                        ..Default::default()
                    },
                },
                Event::Paste(query.to_string()),
            ],
            |ui| app.render_frame(ui),
        );
        // Render an additional frame so both the filter TextEdit and the "No custom actions match filter '...'"
        // read-only label lay out the pasted Arabic string under MissingGlyphPolicy::Panic
        run_frame_with_events(&ctx, Vec::new(), |ui| app.render_frame(ui));
    }

    // 3. Also verify Advanced Convert window renders Arabic input paths, custom folder, and suffix without tofu
    let arabic_png = dir.join("صورة_اختبار_١٠٠٪.png");
    let mut pixmap = tiny_skia::Pixmap::new(16, 16).unwrap();
    pixmap.fill(tiny_skia::Color::from_rgba8(10, 120, 90, 255));
    fs::write(&arabic_png, pixmap.encode_png().unwrap()).unwrap();

    let adv_ctx = egui::Context::default();
    adv_ctx.set_missing_glyph_policy(MissingGlyphPolicy::Panic);
    let mut adv = AdvancedConvertApp::new(
        vec![WirePath::from_path(&arabic_png).unwrap()],
        paths,
        Some(adv_ctx.clone()),
    );
    adv.dest_mode = UiDestMode::CustomFolder;
    adv.dest_folder_text = "/tmp/مجلد_التصدير_العربي_2026".to_string();
    adv.append_suffix = true;
    adv.suffix_text = "-مُعَالَج".to_string();
    adv.save_preset_open = true;
    adv.save_preset_name = "إعداد مسبق للصور ١٠٠٪".to_string();

    run_frame_with_events(&adv_ctx, Vec::new(), |ui| adv.render_frame(ui));
    run_frame_with_events(&adv_ctx, Vec::new(), |ui| adv.render_frame(ui));

    let _ = fs::remove_dir_all(&dir);
}
