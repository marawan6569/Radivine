//! Unit and integration tests for `radivine-actions` (`AC-4.1`, `AC-4.3`, `AC-4.4`, `AC-4.5`, `AC-4.6`).

use std::borrow::Cow;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use radivine_actions::{
    ActionsConfig, CopyAction, PreviewAction, PreviewKind, PreviewPlaceholderAction,
    PreviewSettings, PropertiesAction, PropertiesBackend, SessionDbusPropertiesBackend,
    builtin_registry, parse_actions_toml, registry_from_actions_and_preview_config,
    registry_from_actions_config,
};
use radivine_core::{
    Action, ActionError, ActionId, BasicContext, ClipOp, ClipboardRequest, Context, Dispatcher,
    Effect, Enablement, FileFacts, FileKind, InvocationMode, ItemPath, JobEvent, MenuLimits,
    Outcome, SourceInfo, TextEncoding, TextVerdict, enrich_context_sync, local_path_to_file_uri,
};

#[test]
fn test_preview_visibility_and_enablement_matrix_ac_4_1() {
    let preview = PreviewPlaceholderAction::new(
        "radivine.preview.head",
        "Head",
        "lucide:arrow-up-to-line",
        "Preview first 50 lines",
    );

    // 1. Known binary extensions (.png, .mp3, .pdf, .zip) -> HIDDEN in BasicContext (no I/O)
    for uri in [
        "file:///tmp/photo.png",
        "file:///tmp/song.mp3",
        "file:///tmp/manual.pdf",
        "file:///tmp/archive.zip",
        "sftp://remote.host/notes.txt",
    ] {
        let basic = BasicContext::from_uris([uri], InvocationMode::Drag, SourceInfo::default());
        assert!(
            !preview.is_visible(&basic),
            "Preview must be hidden for {uri}"
        );
    }

    // 2. Text file before enrichment -> visible = true, enablement = Pending ("Checking…")
    let txt_basic = BasicContext::from_uris(
        ["file:///tmp/readme.txt"],
        InvocationMode::Drag,
        SourceInfo::default(),
    );
    assert!(preview.is_visible(&txt_basic));
    let unenriched_ctx = Context::from_basic(txt_basic.clone());
    assert_eq!(preview.enablement(&unenriched_ctx), Enablement::Pending);

    // Verify ActionRegistry::build_menu translates Pending -> disabled_reason = "Checking…"
    let reg = builtin_registry();
    let (tree, index) = reg
        .build_menu(&unenriched_ctx, MenuLimits::default())
        .unwrap();
    let preview_sub = &tree.root.children().unwrap()[0];
    assert_eq!(preview_sub.id.as_ref(), "radivine.preview");
    let head_node = &preview_sub.children().unwrap()[1];
    assert!(!head_node.enabled);
    assert_eq!(head_node.disabled_reason.as_deref(), Some("Checking…"));

    // 3. Enriched regular UTF-8 text file -> Enabled
    let mut enriched_txt = unenriched_ctx.clone();
    enriched_txt.facts = Some(vec![FileFacts {
        kind: FileKind::Regular,
        size: Some(128),
        readable: true,
        text: TextVerdict::Text(TextEncoding::Utf8),
        sniffed_mime: None,
        error: None,
    }]);
    assert_eq!(preview.enablement(&enriched_txt), Enablement::Enabled);
    let updates = reg.enablement_updates(&enriched_txt, &index);
    let head_upd = updates
        .iter()
        .find(|u| u.path == ItemPath::from_slice(&[0, 1]))
        .unwrap();
    assert!(head_upd.enabled);
    assert_eq!(head_upd.disabled_reason, None);

    // 4. PNG renamed to .txt -> visible (extension is .txt), disabled after sniff ("Not a text file")
    let mut disguised_png = unenriched_ctx.clone();
    disguised_png.facts = Some(vec![FileFacts {
        kind: FileKind::Regular,
        size: Some(4096),
        readable: true,
        text: TextVerdict::Binary,
        sniffed_mime: Some("image/png"),
        error: None,
    }]);
    assert_eq!(
        preview.enablement(&disguised_png),
        Enablement::Disabled {
            reason: Cow::Borrowed("Not a text file")
        }
    );

    // 5. Folder -> "Folders can't be previewed"
    let mut folder_ctx = unenriched_ctx.clone();
    folder_ctx.facts = Some(vec![FileFacts {
        kind: FileKind::Directory,
        size: None,
        readable: true,
        text: TextVerdict::Unknown,
        sniffed_mime: None,
        error: None,
    }]);
    assert_eq!(
        preview.enablement(&folder_ctx),
        Enablement::Disabled {
            reason: Cow::Borrowed("Folders can't be previewed")
        }
    );

    // 6. Unreadable file -> "No permission to read"
    let mut unread_ctx = unenriched_ctx.clone();
    unread_ctx.facts = Some(vec![FileFacts {
        kind: FileKind::Regular,
        size: Some(64),
        readable: false,
        text: TextVerdict::Unknown,
        sniffed_mime: None,
        error: Some("No permission to read".to_string()),
    }]);
    assert_eq!(
        preview.enablement(&unread_ctx),
        Enablement::Disabled {
            reason: Cow::Borrowed("No permission to read")
        }
    );

    // 7. Broken symlink -> "Broken link"
    let mut broken_ctx = unenriched_ctx.clone();
    broken_ctx.facts = Some(vec![FileFacts {
        kind: FileKind::BrokenSymlink,
        size: None,
        readable: false,
        text: TextVerdict::Unknown,
        sniffed_mime: None,
        error: Some("Broken link".to_string()),
    }]);
    assert_eq!(
        preview.enablement(&broken_ctx),
        Enablement::Disabled {
            reason: Cow::Borrowed("Broken link")
        }
    );

    // 8. FIFO / special file -> "Special file"
    let mut fifo_ctx = unenriched_ctx.clone();
    fifo_ctx.facts = Some(vec![FileFacts {
        kind: FileKind::Fifo,
        size: None,
        readable: false,
        text: TextVerdict::Unknown,
        sniffed_mime: None,
        error: Some("Special file".to_string()),
    }]);
    assert_eq!(
        preview.enablement(&fifo_ctx),
        Enablement::Disabled {
            reason: Cow::Borrowed("Special file")
        }
    );

    // 9. Timed-out inspection -> "Couldn't inspect the file"
    let mut timeout_ctx = unenriched_ctx.clone();
    timeout_ctx.apply_enrichment_timeout();
    assert_eq!(
        preview.enablement(&timeout_ctx),
        Enablement::Disabled {
            reason: Cow::Borrowed("Couldn't inspect the file")
        }
    );

    // 10. Multi-file selection -> "Select a single file to preview"
    let multi_ctx = Context::from_basic(BasicContext::from_uris(
        ["file:///tmp/a.txt", "file:///tmp/b.txt"],
        InvocationMode::Drag,
        SourceInfo::default(),
    ));
    assert_eq!(
        preview.enablement(&multi_ctx),
        Enablement::Disabled {
            reason: Cow::Borrowed("Select a single file to preview")
        }
    );
}

#[test]
fn test_copy_and_properties_actions() {
    let copy_act = Arc::new(CopyAction::new());
    let recorded_uris: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    struct MockPropsBackend {
        recorded: Arc<Mutex<Vec<String>>>,
    }
    impl PropertiesBackend for MockPropsBackend {
        fn show_item_properties(&self, uris: &[String]) -> Result<(), ActionError> {
            *self.recorded.lock().unwrap() = uris.to_vec();
            Ok(())
        }
    }

    let props_act = Arc::new(PropertiesAction::with_backend(Arc::new(MockPropsBackend {
        recorded: Arc::clone(&recorded_uris),
    })));

    let ctx = Arc::new(Context::from_basic(BasicContext::from_uris(
        [
            "file:///tmp/My%20File%20%F0%9F%9A%80.txt",
            "file:///tmp/second.rs",
        ],
        InvocationMode::Drag,
        SourceInfo::default(),
    )));

    let (tx, rx) = std::sync::mpsc::channel();
    let mut dispatcher = Dispatcher::new(
        Box::new(move |ev| {
            let _ = tx.send(ev);
        }),
        2,
    );

    let j_copy = dispatcher.dispatch(copy_act, Arc::clone(&ctx));
    let j_props = dispatcher.dispatch(props_act, Arc::clone(&ctx));

    for _ in 0..2 {
        match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
            JobEvent::Finished {
                job,
                action,
                result,
            } if job == j_copy => {
                assert_eq!(action, ActionId::from("radivine.copy"));
                assert_eq!(
                    result.unwrap(),
                    Outcome::Effects(vec![Effect::SetClipboard(ClipboardRequest {
                        uris: vec![
                            "file:///tmp/My%20File%20%F0%9F%9A%80.txt".to_string(),
                            "file:///tmp/second.rs".to_string(),
                        ],
                        local_paths: vec![
                            PathBuf::from("/tmp/My File 🚀.txt"),
                            PathBuf::from("/tmp/second.rs"),
                        ],
                        op: ClipOp::Copy,
                    })])
                );
            }
            JobEvent::Finished {
                job,
                action,
                result,
            } if job == j_props => {
                assert_eq!(action, ActionId::from("radivine.properties"));
                assert_eq!(result.unwrap(), Outcome::Done);
            }
            other => panic!("unexpected JobEvent: {other:?}"),
        }
    }

    assert_eq!(
        *recorded_uris.lock().unwrap(),
        vec![
            "file:///tmp/My%20File%20%F0%9F%9A%80.txt".to_string(),
            "file:///tmp/second.rs".to_string(),
        ]
    );

    // Also verify missing D-Bus socket returns "File manager doesn't support properties"
    let missing_backend =
        SessionDbusPropertiesBackend::with_socket(PathBuf::from("/tmp/nonexistent-dbus-sock-9999"));
    let err = missing_backend
        .show_item_properties(&["file:///tmp/a.txt".to_string()])
        .unwrap_err();
    assert_eq!(err.to_string(), "File manager doesn't support properties");
}

#[test]
fn test_custom_actions_parsing_security_validation_and_execution_ac_4_5_and_4_6() {
    // 1. Malformed TOML syntax reports 1-based line and column
    let bad_syntax = "[menu\nhide = [\"radivine.copy\"]\n";
    let err = parse_actions_toml(bad_syntax).unwrap_err();
    assert!(
        err.to_string().contains("line 1"),
        "expected line number in error, got: {err}"
    );

    // 2. Security validation: partial placeholder interpolation `--file={path}` MUST be rejected!
    //    Also `{path}` without `count = "1"` or `per_item = true` MUST be rejected!
    let mixed_toml = r#"
[menu]
hide = ["custom.terminal"]
order = ["radivine.custom", "radivine.copy", "radivine.properties", "radivine.preview"]

[[action]]
id = "bad-partial"
label = "Bad Partial"
exec = ["my-tool", "--file={path}"]

[[action]]
id = "bad-multi-path"
label = "Bad Multi Path"
exec = ["my-tool", "{path}"]
[action.when]
count = "1.."

[[action]]
id = "good-rust-tool"
label = "Rust Tool"
icon = "lucide:code"
description = "Inspect Rust files"
exec = ["/bin/echo", "--", "{paths}"]
cwd = "{dir}"
notify = "always"
[action.when]
scheme = ["file"]
count = "1..5"
mime = ["text/x-rust"]
extensions = ["rs", "toml"]
require_text = true
"#;

    let parsed = parse_actions_toml(mixed_toml).expect("valid TOML structure");
    assert_eq!(
        parsed.warnings.len(),
        2,
        "2 invalid actions should produce warnings"
    );
    assert!(parsed.warnings[0].contains("bad-partial"));
    assert!(parsed.warnings[0].contains("entire argv element"));
    assert!(parsed.warnings[1].contains("bad-multi-path"));
    assert!(parsed.warnings[1].contains("per_item = true"));

    assert_eq!(parsed.actions.len(), 1);
    let act = &parsed.actions[0];
    assert_eq!(act.meta().id, ActionId::from("custom.good-rust-tool"));

    // Verify visibility filtering: matches .rs and .toml (1..=5 items), rejects .md or 6 items
    let rs_basic = BasicContext::from_uris(
        ["file:///tmp/lib.rs", "file:///tmp/Cargo.toml"],
        InvocationMode::Drag,
        SourceInfo::default(),
    );
    assert!(act.is_visible(&rs_basic));

    let md_basic = BasicContext::from_uris(
        ["file:///tmp/README.md"],
        InvocationMode::Drag,
        SourceInfo::default(),
    );
    assert!(!act.is_visible(&md_basic));

    // Verify argv expansion preserves exact argument boundaries even with spaces/shell metacharacters
    let tricky_basic = BasicContext::from_uris(
        [
            "file:///tmp/space%20%26%20%3B%20rm%20-rf%20.rs",
            "file:///tmp/second%20file.toml",
        ],
        InvocationMode::Drag,
        SourceInfo::default(),
    );
    let argv = act.expand_argv(&tricky_basic.items).unwrap();
    assert_eq!(
        argv,
        vec![
            OsString::from("/bin/echo"),
            OsString::from("--"),
            OsString::from("/tmp/space & ; rm -rf .rs"),
            OsString::from("/tmp/second file.toml"),
        ]
    );

    // 3. End-to-end execution of a harmless custom action script that dumps exact argv (`AC-4.5`)
    let tmp_dir = std::env::temp_dir().join(format!("radivine_custom_exec_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let dump_out = tmp_dir.join("argv_dump.txt");
    let script_path = tmp_dir.join("dump_args.sh");
    // Harmless script writing each positional argument on its own line
    let script_body = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n",
        dump_out.display()
    );
    std::fs::write(&script_path, script_body).unwrap();
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let target_a = tmp_dir.join("file one 🚀.rs");
    let target_b = tmp_dir.join("file;two$().rs");
    std::fs::write(&target_a, "fn main() {}\n").unwrap();
    std::fs::write(&target_b, "pub fn helper() {}\n").unwrap();

    let exec_toml = format!(
        r#"
[[action]]
id = "dump-argv"
label = "Dump Argv"
exec = ["{}", "--flag", "{{name}}", "{{paths}}"]
cwd = "{{dir}}"
wait = true
notify = "always"
[action.when]
extensions = ["rs"]
require_text = true
"#,
        script_path.display()
    );

    let cfg = parse_actions_toml(&exec_toml).unwrap();
    assert!(cfg.warnings.is_empty());
    let reg = registry_from_actions_config(&cfg).unwrap();
    let custom_action = reg.get(&ActionId::from("custom.dump-argv")).unwrap();

    let exec_ctx = Arc::new(enrich_context_sync(BasicContext::from_uris(
        [
            local_path_to_file_uri(&target_a),
            local_path_to_file_uri(&target_b),
        ],
        InvocationMode::Drag,
        SourceInfo::default(),
    )));
    assert_eq!(custom_action.enablement(&exec_ctx), Enablement::Enabled);

    let (tx, rx) = std::sync::mpsc::channel();
    let mut dispatcher = Dispatcher::new(
        Box::new(move |ev| {
            let _ = tx.send(ev);
        }),
        2,
    );
    dispatcher.dispatch(custom_action, exec_ctx);

    let ev = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    match ev {
        JobEvent::Finished { result, .. } => {
            assert_eq!(
                result.unwrap(),
                Outcome::Effects(vec![Effect::Notify {
                    summary: "Dump Argv completed".to_string(),
                    body: None,
                }])
            );
        }
    }

    let dumped = std::fs::read_to_string(&dump_out).unwrap();
    let lines: Vec<&str> = dumped.lines().collect();
    assert_eq!(
        lines,
        vec![
            "--flag",
            "file one 🚀.rs",
            target_a.to_str().unwrap(),
            target_b.to_str().unwrap(),
        ]
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_preview_action_viewer_command_arguments_and_settings() {
    let path = PathBuf::from("/tmp/My File 🚀.txt");

    let full = PreviewAction::with_settings(
        "radivine.preview.full",
        "Full Preview",
        "lucide:file-text",
        "Open full interactive document preview",
        PreviewKind::Full,
        50,
        "dark",
    );
    assert_eq!(
        full.build_viewer_args(&path),
        vec![
            OsString::from("--mode"),
            OsString::from("full"),
            OsString::from("--theme"),
            OsString::from("dark"),
            OsString::from("--"),
            OsString::from("/tmp/My File 🚀.txt"),
        ]
    );

    let head = PreviewAction::with_settings(
        "radivine.preview.head",
        "Head",
        "lucide:arrow-up-to-line",
        "Preview first 25 lines of file",
        PreviewKind::Head,
        25,
        "light",
    );
    assert_eq!(
        head.build_viewer_args(&path),
        vec![
            OsString::from("--mode"),
            OsString::from("head"),
            OsString::from("--lines"),
            OsString::from("25"),
            OsString::from("--theme"),
            OsString::from("light"),
            OsString::from("--"),
            OsString::from("/tmp/My File 🚀.txt"),
        ]
    );

    let tail = PreviewAction::with_settings(
        "radivine.preview.tail",
        "Tail",
        "lucide:arrow-down-to-line",
        "Preview last 75 lines of file",
        PreviewKind::Tail,
        75,
        "auto",
    );
    assert_eq!(
        tail.build_viewer_args(&path),
        vec![
            OsString::from("--mode"),
            OsString::from("tail"),
            OsString::from("--lines"),
            OsString::from("75"),
            OsString::from("--theme"),
            OsString::from("auto"),
            OsString::from("--"),
            OsString::from("/tmp/My File 🚀.txt"),
        ]
    );

    let custom_preview = PreviewSettings {
        head_lines: 30,
        tail_lines: 60,
        theme: "light".to_string(),
    };
    let reg = registry_from_actions_and_preview_config(&ActionsConfig::default(), &custom_preview)
        .unwrap();
    let head_act = reg.get(&ActionId::from("radivine.preview.head")).unwrap();
    let tail_act = reg.get(&ActionId::from("radivine.preview.tail")).unwrap();
    assert_eq!(
        head_act.meta().description.as_deref(),
        Some("Preview first 30 lines of file")
    );
    assert_eq!(
        tail_act.meta().description.as_deref(),
        Some("Preview last 60 lines of file")
    );
}
