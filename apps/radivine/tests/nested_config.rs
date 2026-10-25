//! Milestone N1 configuration and compatibility tests (`TEST_MATRIX.md` §T1: `T-CFG1`..`T-CFG4`).

use std::fs;

use radivine::config::{AppConfig, ReducedMotionSetting, ScaleSetting};
use radivine_wheel::{InvokeSelect, SubmenuOpen, WheelConfig, WheelLayout};
use tempfile::tempdir;

/// `T-CFG1`: Missing file, missing `[wheel]`, and missing `layout` all map to Branching;
/// existing numeric/default values remain unchanged.
#[test]
fn t_cfg1_missing_file_section_and_key_default_to_branching() {
    let dir = tempdir().expect("create tempdir");
    let missing_path = dir.path().join("does-not-exist.toml");

    // 1. Missing file
    let cfg_missing_file = AppConfig::load_or_default(Some(&missing_path));
    assert_eq!(cfg_missing_file, AppConfig::default());
    assert_eq!(cfg_missing_file.version, 1);
    assert_eq!(cfg_missing_file.wheel.layout, "branching");
    assert_eq!(cfg_missing_file.wheel.root_inner_radius, 40.0);
    assert_eq!(cfg_missing_file.wheel.root_outer_radius, 128.0);
    assert_eq!(cfg_missing_file.wheel.child_inner_radius, 34.0);
    assert_eq!(cfg_missing_file.wheel.child_outer_radius, 108.0);
    assert_eq!(cfg_missing_file.wheel.child_offset_gap, 20.0);
    assert_eq!(cfg_missing_file.wheel.hover_dwell_ms, 110);
    assert_eq!(cfg_missing_file.wheel.submenu_grace_ms, 180);
    assert_eq!(cfg_missing_file.wheel.leave_grace_ms, 350);
    assert_eq!(cfg_missing_file.wheel.angular_hysteresis_deg, 3.5);
    assert!(!cfg_missing_file.input.passthrough);
    assert_eq!(
        cfg_missing_file.to_wheel_config(false).layout,
        WheelLayout::Branching
    );

    // 2. Missing `[wheel]` section
    let cfg_no_wheel = AppConfig::from_toml_str(
        r#"
        version = 1

        [trigger]
        drag_modifier = "alt"
        min_drag_distance_px = 9
        "#,
    );
    assert_eq!(cfg_no_wheel.wheel.layout, "branching");
    assert_eq!(cfg_no_wheel.wheel.root_inner_radius, 40.0);
    assert_eq!(cfg_no_wheel.wheel.root_outer_radius, 128.0);
    assert_eq!(cfg_no_wheel.trigger.drag_modifier, "alt");
    assert_eq!(cfg_no_wheel.trigger.min_drag_distance_px, 9);
    assert_eq!(
        cfg_no_wheel.to_wheel_config(false).layout,
        WheelLayout::Branching
    );

    // 3. Present `[wheel]` section without `layout` key (v0.1.0 / v0.2.0 legacy config)
    let cfg_missing_key = AppConfig::from_toml_str(
        r#"
        version = 1

        [wheel]
        root_inner_radius = 44.0
        root_outer_radius = 136.0
        child_inner_radius = 36.0
        child_outer_radius = 112.0
        child_offset_gap = 18.0
        hover_dwell_ms = 125
        submenu_grace_ms = 210
        leave_grace_ms = 400
        angular_hysteresis_deg = 4.5
        "#,
    );
    assert_eq!(cfg_missing_key.wheel.layout, "branching");
    assert_eq!(cfg_missing_key.wheel.root_inner_radius, 44.0);
    assert_eq!(cfg_missing_key.wheel.root_outer_radius, 136.0);
    assert_eq!(cfg_missing_key.wheel.child_inner_radius, 36.0);
    assert_eq!(cfg_missing_key.wheel.child_outer_radius, 112.0);
    assert_eq!(cfg_missing_key.wheel.child_offset_gap, 18.0);
    assert_eq!(cfg_missing_key.wheel.hover_dwell_ms, 125);
    assert_eq!(cfg_missing_key.wheel.submenu_grace_ms, 210);
    assert_eq!(cfg_missing_key.wheel.leave_grace_ms, 400);
    assert_eq!(cfg_missing_key.wheel.angular_hysteresis_deg, 4.5);
    assert_eq!(
        cfg_missing_key.to_wheel_config(false).layout,
        WheelLayout::Branching
    );
}

/// `T-CFG2`: Exact strings `branching`, `concentric` round-trip. Unknown, empty, mixed-case,
/// whitespace-padded string and TOML integer/bool/array/table/datetime for layout warn and
/// reset only layout. Preserve nondefault trigger, passthrough, preview and unknown/jobs values.
#[test]
fn t_cfg2_exact_roundtrip_and_field_local_fallback_for_invalid_layout_values() {
    // 1. Exact canonical values round-trip
    for (canonical, expected_enum) in [
        ("branching", WheelLayout::Branching),
        ("concentric", WheelLayout::Concentric),
    ] {
        let raw = format!(
            r#"
            version = 1
            [wheel]
            layout = "{canonical}"
            root_inner_radius = 42.0
            "#
        );
        let cfg = AppConfig::from_toml_str(&raw);
        assert_eq!(cfg.wheel.layout, canonical);
        assert_eq!(cfg.wheel.root_inner_radius, 42.0);
        assert!(cfg.validate_strict().is_ok());
        assert_eq!(cfg.to_wheel_config(false).layout, expected_enum);
    }

    // 2. Malformed strings and non-string TOML types reset ONLY `wheel.layout` while preserving
    //    all nondefault trigger, safety (`input`), preview, appearance, notifications, x11,
    //    jobs, and unknown keys.
    let invalid_layout_snippets = [
        // Invalid strings
        r#"layout = """#,
        r#"layout = "unknown""#,
        r#"layout = "vine""#,
        r#"layout = "nested""#,
        r#"layout = "Branching""#,
        r#"layout = "Concentric""#,
        r#"layout = "CONCENTRIC""#,
        r#"layout = " branching""#,
        r#"layout = "concentric ""#,
        r#"layout = " concentric ""#,
        // Invalid TOML value types
        r#"layout = 0"#,
        r#"layout = 42"#,
        r#"layout = 3.14"#,
        r#"layout = true"#,
        r#"layout = false"#,
        r#"layout = ["concentric"]"#,
        r#"layout = { mode = "concentric" }"#,
        r#"layout = 1979-05-27T07:32:00Z"#,
    ];

    for snippet in invalid_layout_snippets {
        let doc = format!(
            r#"
            version = 1
            custom_top_level_key = "preserved"

            [trigger]
            drag_modifier = "ctrl"
            toggle_off_on_repress = false
            hold_to_keep_open = true
            min_drag_distance_px = 11

            [wheel]
            {snippet}
            root_inner_radius = 46.0
            root_outer_radius = 144.0
            child_inner_radius = 38.0
            child_outer_radius = 118.0
            child_offset_gap = 24.0
            hover_dwell_ms = 165
            submenu_grace_ms = 240
            leave_grace_ms = 420
            angular_hysteresis_deg = 5.0
            future_wheel_hint = "ignored"

            [appearance]
            theme = "light"
            scale = 1.5
            reduced_motion = true
            backdrop_dim = 0.25

            [notifications]
            enabled = false
            notify_on_action_error = false
            notify_on_config_warning = false

            [preview]
            head_lines = 120
            tail_lines = 85

            [input]
            passthrough = true
            release_outside_cancels = false

            [x11]
            trigger_detection = "poll"
            window_type = "popup_menu"
            pointer_nudge = false

            [jobs]
            max_parallel = 4
            memory_budget_mb = 512
            idle_timeout_secs = 60
            "#
        );

        let cfg = AppConfig::from_toml_str(&doc);
        assert_eq!(
            cfg.wheel.layout, "branching",
            "snippet `{snippet}` must reset wheel.layout to branching"
        );
        assert_eq!(
            cfg.to_wheel_config(false).layout,
            WheelLayout::Branching,
            "snippet `{snippet}` must map to WheelLayout::Branching"
        );

        // Verify unrelated `[wheel]` settings are preserved
        assert_eq!(cfg.wheel.root_inner_radius, 46.0, "snippet `{snippet}`");
        assert_eq!(cfg.wheel.root_outer_radius, 144.0, "snippet `{snippet}`");
        assert_eq!(cfg.wheel.child_inner_radius, 38.0, "snippet `{snippet}`");
        assert_eq!(cfg.wheel.child_outer_radius, 118.0, "snippet `{snippet}`");
        assert_eq!(cfg.wheel.child_offset_gap, 24.0, "snippet `{snippet}`");
        assert_eq!(cfg.wheel.hover_dwell_ms, 165, "snippet `{snippet}`");
        assert_eq!(cfg.wheel.submenu_grace_ms, 240, "snippet `{snippet}`");
        assert_eq!(cfg.wheel.leave_grace_ms, 420, "snippet `{snippet}`");
        assert_eq!(cfg.wheel.angular_hysteresis_deg, 5.0, "snippet `{snippet}`");

        // Verify `[trigger]` settings are preserved
        assert_eq!(cfg.trigger.drag_modifier, "ctrl", "snippet `{snippet}`");
        assert!(!cfg.trigger.toggle_off_on_repress, "snippet `{snippet}`");
        assert!(cfg.trigger.hold_to_keep_open, "snippet `{snippet}`");
        assert_eq!(cfg.trigger.min_drag_distance_px, 11, "snippet `{snippet}`");

        // Verify `[input]` safety settings are preserved
        assert!(cfg.input.passthrough, "snippet `{snippet}`");
        assert!(!cfg.input.release_outside_cancels, "snippet `{snippet}`");

        // Verify `[preview]` settings are preserved
        assert_eq!(cfg.preview.head_lines, 120, "snippet `{snippet}`");
        assert_eq!(cfg.preview.tail_lines, 85, "snippet `{snippet}`");

        // Verify `[appearance]` settings are preserved
        assert_eq!(cfg.appearance.theme, "light", "snippet `{snippet}`");
        assert_eq!(
            cfg.appearance.scale,
            ScaleSetting::Fixed(1.5),
            "snippet `{snippet}`"
        );
        assert_eq!(
            cfg.appearance.reduced_motion,
            ReducedMotionSetting::Fixed(true),
            "snippet `{snippet}`"
        );
        assert_eq!(cfg.appearance.backdrop_dim, 0.25, "snippet `{snippet}`");

        // Verify `[notifications]` and `[x11]` settings are preserved
        assert!(!cfg.notifications.enabled, "snippet `{snippet}`");
        assert!(
            !cfg.notifications.notify_on_action_error,
            "snippet `{snippet}`"
        );
        assert!(
            !cfg.notifications.notify_on_config_warning,
            "snippet `{snippet}`"
        );
        assert_eq!(cfg.x11.trigger_detection, "poll", "snippet `{snippet}`");
        assert_eq!(cfg.x11.window_type, "popup_menu", "snippet `{snippet}`");
        assert!(!cfg.x11.pointer_nudge, "snippet `{snippet}`");
    }
}

/// `T-CFG3`: Strict validation rejects all invalid values; valid candidate preserves `version = 1`.
/// Broken TOML syntax retains existing startup fallback, distinctly reported.
#[test]
fn t_cfg3_strict_validation_and_syntax_error_handling() {
    // 1. Valid candidates pass strict validation and preserve `version = 1`
    for valid_layout in ["branching", "concentric"] {
        let raw = format!(
            r#"
            version = 1
            [wheel]
            layout = "{valid_layout}"
            "#
        );
        let parsed: AppConfig = toml::from_str(&raw).expect("valid TOML should parse");
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.wheel.layout, valid_layout);
        assert!(parsed.validate_strict().is_ok());
    }

    // 2. Direct deserialization (`toml::from_str::<AppConfig>`) + `validate_strict()` rejects
    //    every invalid string AND every wrong TOML value type (via the sentinel deserializer).
    let strict_reject_snippets = [
        r#"layout = """#,
        r#"layout = "unknown""#,
        r#"layout = "Branching""#,
        r#"layout = "Concentric""#,
        r#"layout = "CONCENTRIC""#,
        r#"layout = " branching""#,
        r#"layout = "concentric ""#,
        r#"layout = 123"#,
        r#"layout = true"#,
        r#"layout = [1, 2]"#,
        r#"layout = { nested = true }"#,
        r#"layout = 1979-05-27T07:32:00Z"#,
    ];

    for snippet in strict_reject_snippets {
        let raw = format!(
            r#"
            version = 1
            [wheel]
            {snippet}
            "#
        );
        let parsed: AppConfig =
            toml::from_str(&raw).expect("custom deserializer should capture wrong layout type");
        let err = parsed
            .validate_strict()
            .expect_err(&format!("snippet `{snippet}` must fail validate_strict"));
        assert!(
            err.iter().any(|m| m.contains("wheel.layout")),
            "expected wheel.layout error for `{snippet}`, got {err:?}"
        );
    }

    // 3. Broken TOML syntax: direct `toml::from_str` reports syntax error, while startup
    //    `AppConfig::from_toml_str` / `load_or_default` retains whole-file default fallback.
    let broken_toml = "[wheel\nlayout = \"concentric\"\nroot_inner_radius = 55.0\n";
    assert!(toml::from_str::<AppConfig>(broken_toml).is_err());

    let fallback_cfg = AppConfig::from_toml_str(broken_toml);
    assert_eq!(fallback_cfg, AppConfig::default());
    assert_eq!(fallback_cfg.wheel.layout, "branching");

    let dir = tempdir().expect("create tempdir");
    let broken_path = dir.path().join("config.toml");
    fs::write(&broken_path, broken_toml).expect("write broken config");
    let loaded = AppConfig::load_or_default(Some(&broken_path));
    assert_eq!(loaded, AppConfig::default());
}

/// `T-CFG4`: `to_wheel_config` maps enum without serde/runtime dependencies in wheel;
/// app defaults and pure engine defaults tested separately.
#[test]
fn t_cfg4_to_wheel_config_enum_mapping_and_separate_app_vs_engine_defaults() {
    // 1. Pure engine defaults (`WheelConfig::default()`)
    let engine_def = WheelConfig::default();
    assert_eq!(engine_def.layout, WheelLayout::Branching);
    assert_eq!(engine_def.max_items_per_wheel, 8);
    assert_eq!(engine_def.max_depth, 4);
    assert_eq!(engine_def.activation_slop, 6.0);
    assert_eq!(engine_def.hit_margin, 12.0);
    assert_eq!(engine_def.segment_gap, 2.5);
    assert_eq!(engine_def.child_gap, 10.0);
    assert_eq!(engine_def.bridge_angle_deg, 70.0);
    assert_eq!(engine_def.hysteresis_deg, 4.0);
    assert_eq!(engine_def.radius_root, [38.0, 116.0]);
    assert_eq!(engine_def.radius_child, [30.0, 96.0]);
    assert_eq!(engine_def.radius_deep, [26.0, 84.0]);
    assert_eq!(engine_def.radius_min_outer, 64.0);
    assert_eq!(engine_def.submenu_open, SubmenuOpen::DwellAndBand);
    assert_eq!(engine_def.dwell_ms, 140);
    assert_eq!(engine_def.band, 0.65);
    assert_eq!(engine_def.grace_ms, 300);
    assert_eq!(engine_def.hub_close_ms, 150);
    assert_eq!(engine_def.invoke_select, InvokeSelect::Click);
    assert!(!engine_def.reduced_motion);

    // 2. Shipped app defaults (`AppConfig::default().to_wheel_config(false)`)
    let app_def = AppConfig::default();
    let app_wheel_cfg = app_def.to_wheel_config(false);
    assert_eq!(app_wheel_cfg.layout, WheelLayout::Branching);
    assert_eq!(app_wheel_cfg.radius_root, [40.0, 128.0]);
    assert_eq!(app_wheel_cfg.radius_child, [34.0, 108.0]);
    assert_eq!(app_wheel_cfg.child_gap, 20.0);
    assert_eq!(app_wheel_cfg.dwell_ms, 110);
    assert_eq!(app_wheel_cfg.grace_ms, 180);
    assert_eq!(app_wheel_cfg.hysteresis_deg, 3.5);
    assert!(!app_wheel_cfg.reduced_motion);

    // 3. Explicit `"concentric"` maps to `WheelLayout::Concentric` and survives `WheelConfig::validate()`
    let concentric_app = AppConfig::from_toml_str(
        r#"
        [wheel]
        layout = "concentric"
        "#,
    );
    let concentric_wheel_cfg = concentric_app.to_wheel_config(true);
    assert_eq!(concentric_wheel_cfg.layout, WheelLayout::Concentric);
    assert!(concentric_wheel_cfg.reduced_motion);
    assert_eq!(
        concentric_wheel_cfg.validated().layout,
        WheelLayout::Concentric
    );
}

/// `T-MENU1` & `T-MENU2` (App / Registry level):
/// Preview/Copy/Properties/Custom/Convert IDs, order, labels, descriptions, and visibility are
/// identical across text, image, mixed, folder, unreadable, remote, and large selections;
/// engine `sanitize` under both `Branching` and `Concentric` never remaps `ActionIndex`.
#[test]
fn t_menu1_and_t_menu2_app_registry_action_index_and_context_parity() {
    use radivine_actions::{
        ConvertMenuContext, PreviewSettings, parse_actions_toml, register_builtins_with_options,
    };
    use radivine_core::{
        ActionRegistry, BasicContext, Context, FileFacts, FileKind, InvocationMode, ItemPath,
        MenuLimits, MenuNode, NodeKind, SourceInfo, TextEncoding, TextVerdict,
    };
    use radivine_wheel::{InputMode, Point, Viewport, WheelSession};

    let custom_toml = r#"
        [[action]]
        id = "custom.echo_path"
        label = "Echo Path"
        description = "Print path to stdout"
        icon = "lucide:terminal"
        exec = ["/bin/echo", "{paths}"]
    "#;
    let actions_cfg = parse_actions_toml(custom_toml).expect("parse custom actions");
    let mut reg = ActionRegistry::new();
    register_builtins_with_options(
        &mut reg,
        &actions_cfg,
        &PreviewSettings::default(),
        &ConvertMenuContext::default(),
    )
    .expect("register_builtins_with_options");

    // Build the 7 required contexts: text, image, mixed, folder, unreadable, remote, large
    let make_ctx = |uris: Vec<String>, facts: Option<Vec<FileFacts>>| -> Context {
        let basic = BasicContext::from_uris(uris, InvocationMode::Drag, SourceInfo::default());
        let mut ctx = Context::from_basic(basic);
        ctx.facts = facts;
        ctx
    };

    let text_facts = FileFacts {
        kind: FileKind::Regular,
        size: Some(128),
        readable: true,
        text: TextVerdict::Text(TextEncoding::Utf8),
        sniffed_mime: Some("text/plain"),
        error: None,
    };
    let image_facts = FileFacts {
        kind: FileKind::Regular,
        size: Some(4096),
        readable: true,
        text: TextVerdict::Binary,
        sniffed_mime: Some("image/png"),
        error: None,
    };
    let folder_facts = FileFacts {
        kind: FileKind::Directory,
        size: Some(0),
        readable: true,
        text: TextVerdict::Binary,
        sniffed_mime: Some("inode/directory"),
        error: None,
    };
    let unreadable_facts = FileFacts {
        kind: FileKind::Regular,
        size: Some(64),
        readable: false,
        text: TextVerdict::Unknown,
        sniffed_mime: None,
        error: Some("Permission denied".to_string()),
    };

    let large_uris: Vec<String> = (0..20)
        .map(|i| format!("file:///tmp/item_{i}.txt"))
        .collect();

    let contexts = [
        (
            "text",
            make_ctx(
                vec!["file:///tmp/notes.txt".into()],
                Some(vec![text_facts.clone()]),
            ),
        ),
        (
            "image",
            make_ctx(
                vec!["file:///tmp/photo.png".into()],
                Some(vec![image_facts.clone()]),
            ),
        ),
        (
            "mixed",
            make_ctx(
                vec![
                    "file:///tmp/notes.txt".into(),
                    "file:///tmp/photo.png".into(),
                ],
                Some(vec![text_facts, image_facts]),
            ),
        ),
        (
            "folder",
            make_ctx(vec!["file:///tmp/docs".into()], Some(vec![folder_facts])),
        ),
        (
            "unreadable",
            make_ctx(
                vec!["file:///tmp/secret.txt".into()],
                Some(vec![unreadable_facts]),
            ),
        ),
        (
            "remote",
            make_ctx(vec!["sftp://server/home/user/report.txt".into()], None),
        ),
        ("large", make_ctx(large_uris, None)),
    ];

    fn collect_leaves(node: &MenuNode, prefix: ItemPath, out: &mut Vec<(ItemPath, String)>) {
        match &node.kind {
            NodeKind::Leaf => out.push((prefix, node.id.to_string())),
            NodeKind::Submenu { children, .. } => {
                for (i, ch) in children.iter().enumerate() {
                    collect_leaves(ch, prefix.child(i as u8), out);
                }
            }
        }
    }

    let app_b = AppConfig::default().to_wheel_config(false);
    let mut app_c = app_b.clone();
    app_c.layout = WheelLayout::Concentric;
    let vp = Viewport::new(1920.0, 1080.0, 1.0);
    let center = Point::new(960.0, 540.0);

    for (ctx_name, ctx) in &contexts {
        let (tree, index) = reg
            .build_menu(ctx, MenuLimits::default())
            .expect("build_menu must succeed for all 7 contexts");

        // Verify engine `sanitize` is a no-op on a tree built with matching MenuLimits (8, 4)
        // and does not remap any ActionIndex path in either Branching or Concentric!
        let mut san_b = tree.clone();
        assert!(
            san_b.sanitize(&app_b),
            "context `{ctx_name}` must remain non-empty after sanitize"
        );
        let mut san_c = tree.clone();
        assert!(
            san_c.sanitize(&app_c),
            "context `{ctx_name}` must remain non-empty after sanitize"
        );
        assert_eq!(san_b, tree, "context `{ctx_name}` Branching tree equality");
        assert_eq!(san_c, tree, "context `{ctx_name}` Concentric tree equality");

        let mut leaves = Vec::new();
        collect_leaves(&tree.root, ItemPath::root(), &mut leaves);
        for (leaf_path, node_id) in &leaves {
            let indexed_action = index
                .get(leaf_path)
                .expect("every leaf path in MenuTree must exist in ActionIndex");
            assert_eq!(
                indexed_action.as_str(),
                node_id.as_str(),
                "ActionIndex mapping mismatch at {leaf_path:?} in context `{ctx_name}`"
            );
        }

        // Verify both sessions expose identical root item labels, icons, enabled flags, and has_children
        let s_b = WheelSession::new(Some(tree.clone()), center, vp, InputMode::Drag, &app_b, 0);
        let s_c = WheelSession::new(Some(tree), center, vp, InputMode::Drag, &app_c, 0);
        let segs_b = &s_b.scene().wheels[0].segments;
        let segs_c = &s_c.scene().wheels[0].segments;
        assert_eq!(segs_b.len(), segs_c.len(), "context `{ctx_name}`");
        for (sb, sc) in segs_b.iter().zip(segs_c.iter()) {
            assert_eq!(sb.label, sc.label, "context `{ctx_name}`");
            assert_eq!(sb.icon, sc.icon, "context `{ctx_name}`");
            assert_eq!(sb.enabled, sc.enabled, "context `{ctx_name}`");
            assert_eq!(sb.has_children, sc.has_children, "context `{ctx_name}`");
        }
    }
}
