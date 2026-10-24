//! Unit tests for `radivine-core` (URI list parsing, `ActionRegistry::build_menu`, and `Dispatcher`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use radivine_core::{
    Action, ActionError, ActionId, ActionMeta, ActionRegistry, BasicContext, Context, ContextKind,
    Dispatcher, Enablement, EnrichEvent, Enricher, FileKind, InvocationMode, Item, JobEvent,
    JobHandle, LogAction, MenuLimits, MenuOverrides, MenuSpec, Outcome, RegistryError,
    SNIFF_BUFFER_BYTES, SourceInfo, TextEncoding, TextVerdict, enrich_context_sync, enrich_item,
    local_path_to_file_uri, normalize_cli_arg_to_uri, sniff_bytes,
};
use radivine_wheel::{IconKey, ItemPath};

#[test]
fn test_rfc2483_uri_list_parsing_spaces_non_ascii_and_schemes() {
    let raw = b"# Comment line\r\n\
file:///home/user/My%20Documents/caf%C3%A9%20r%C3%A9sum%C3%A9.txt\r\n\
\r\n\
file://localhost/tmp/archive.tar.gz\n\
sftp://server.local/var/log/syslog\r\n\
file://remote-host/share/doc.pdf\r\n";

    let basic = BasicContext::from_uri_list_bytes(
        raw,
        InvocationMode::Drag,
        SourceInfo {
            app: Some("nemo".to_string()),
            pid: Some(1234),
        },
    );

    assert_eq!(basic.kind, ContextKind::Files);
    assert_eq!(basic.mode, InvocationMode::Drag);
    assert_eq!(basic.source.app.as_deref(), Some("nemo"));
    assert!(!basic.truncated);
    assert_eq!(basic.items.len(), 4);

    // 1. Space + UTF-8 percent-encoded local file
    let i0 = &basic.items[0];
    assert_eq!(
        i0.local_path,
        Some(PathBuf::from("/home/user/My Documents/café résumé.txt"))
    );
    assert_eq!(i0.name, "café résumé.txt");
    assert_eq!(i0.ext.as_deref(), Some("txt"));
    assert_eq!(i0.guessed_mime, Some("text/plain"));

    // 2. file://localhost/...
    let i1 = &basic.items[1];
    assert_eq!(i1.local_path, Some(PathBuf::from("/tmp/archive.tar.gz")));
    assert_eq!(i1.name, "archive.tar.gz");
    assert_eq!(i1.ext.as_deref(), Some("gz"));

    // 3. sftp:// remote URI has local_path == None
    let i2 = &basic.items[2];
    assert_eq!(i2.local_path, None);
    assert_eq!(i2.name, "syslog");
    assert_eq!(i2.ext, None);

    // 4. Non-localhost file:// host has local_path == None
    let i3 = &basic.items[3];
    assert_eq!(i3.local_path, None);
    assert_eq!(i3.name, "doc.pdf");
    assert_eq!(i3.ext.as_deref(), Some("pdf"));
}

#[test]
fn test_cli_arg_normalization_preserves_spaces_and_unicode() {
    let base = Path::new("/home/alice/work");
    let uri = normalize_cli_arg_to_uri("sub dir/notes 📝.md", base).unwrap();
    assert_eq!(
        uri,
        "file:///home/alice/work/sub%20dir/notes%20%F0%9F%93%9D.md"
    );

    let basic = BasicContext::from_uris([&uri], InvocationMode::Invoke, SourceInfo::default());
    assert_eq!(basic.items.len(), 1);
    assert_eq!(
        basic.items[0].local_path,
        Some(PathBuf::from("/home/alice/work/sub dir/notes 📝.md"))
    );
    assert_eq!(basic.items[0].name, "notes 📝.md");
}

#[test]
fn test_registry_build_menu_overrides_custom_expansion_and_overflow() {
    let mut reg = ActionRegistry::new();

    let copy_act = Arc::new(LogAction::new(ActionMeta {
        id: ActionId::from("radivine.copy"),
        label: "Copy".to_string(),
        icon: Some(IconKey::from("lucide:copy")),
        description: Some("Copy file".to_string()),
        kinds: &[ContextKind::Files],
    }));
    reg.register(copy_act.clone()).unwrap();
    assert_eq!(
        reg.register(copy_act),
        Err(RegistryError::DuplicateId("radivine.copy".to_string()))
    );

    reg.register(Arc::new(LogAction::new(ActionMeta {
        id: ActionId::from("radivine.properties"),
        label: "Properties".to_string(),
        icon: Some(IconKey::from("lucide:info")),
        description: None,
        kinds: &[ContextKind::Files],
    })))
    .unwrap();

    for idx in 1..=3 {
        reg.register(Arc::new(LogAction::new(ActionMeta {
            id: ActionId::from(format!("custom.tool_{idx}")),
            label: format!("Tool {idx}"),
            icon: Some(IconKey::from("lucide:terminal")),
            description: None,
            kinds: &[ContextKind::Files],
        })))
        .unwrap();
    }

    let spec = vec![
        MenuSpec::Action(ActionId::from("radivine.copy")),
        MenuSpec::Action(ActionId::from("radivine.properties")),
        MenuSpec::Submenu {
            id: Arc::from("radivine.custom"),
            label: "Custom".to_string(),
            icon: Some(IconKey::from("lucide:sparkles")),
            description: None,
            children: vec![MenuSpec::CustomActions],
            default: None,
        },
    ];

    let overrides = MenuOverrides {
        hide: vec![ActionId::from("custom.tool_2")],
        order: vec![Arc::from("radivine.custom"), Arc::from("radivine.copy")],
    };
    reg.set_menu(spec, overrides);

    let ctx = Context::from_basic(BasicContext::from_uris(
        ["file:///tmp/test.txt"],
        InvocationMode::Invoke,
        SourceInfo::default(),
    ));

    let (tree, index) = reg
        .build_menu(
            &ctx,
            MenuLimits {
                max_items_per_wheel: 8,
                max_depth: 4,
            },
        )
        .expect("menu should be non-empty");

    let roots = tree.root.children().unwrap();
    assert_eq!(roots.len(), 3);
    // Reordered: radivine.custom first, then radivine.copy, then unlisted radivine.properties
    assert_eq!(roots[0].id.as_ref(), "radivine.custom");
    assert_eq!(roots[1].id.as_ref(), "radivine.copy");
    assert_eq!(roots[2].id.as_ref(), "radivine.properties");

    // Custom submenu has tool_1 and tool_3 (tool_2 was hidden)
    let custom_children = roots[0].children().unwrap();
    assert_eq!(custom_children.len(), 2);
    assert_eq!(
        index.get(&ItemPath::from_slice(&[0, 0])),
        Some(&ActionId::from("custom.tool_1"))
    );
    assert_eq!(
        index.get(&ItemPath::from_slice(&[0, 1])),
        Some(&ActionId::from("custom.tool_3"))
    );
    assert_eq!(
        index.get(&ItemPath::from_slice(&[1])),
        Some(&ActionId::from("radivine.copy"))
    );
}

struct PanickingAction {
    meta: ActionMeta,
}

impl Action for PanickingAction {
    fn meta(&self) -> &ActionMeta {
        &self.meta
    }

    fn is_visible(&self, _ctx: &BasicContext) -> bool {
        true
    }

    fn enablement(&self, _ctx: &Context) -> Enablement {
        Enablement::Enabled
    }

    fn run(&self, _ctx: &Context, _job: &JobHandle) -> Result<Outcome, ActionError> {
        panic!("simulated action panic");
    }
}

#[test]
fn test_dispatcher_executes_jobs_and_catches_panics() {
    let (tx, rx) = mpsc::channel();
    let mut dispatcher = Dispatcher::new(
        Box::new(move |ev| {
            let _ = tx.send(ev);
        }),
        2,
    );

    let ctx = Arc::new(Context::from_basic(BasicContext::from_uris(
        ["file:///tmp/demo.txt"],
        InvocationMode::Invoke,
        SourceInfo::default(),
    )));

    let ok_action = Arc::new(LogAction::new(ActionMeta {
        id: ActionId::from("radivine.copy"),
        label: "Copy".to_string(),
        icon: None,
        description: None,
        kinds: &[ContextKind::Files],
    }));

    let panic_action = Arc::new(PanickingAction {
        meta: ActionMeta {
            id: ActionId::from("test.panic"),
            label: "Panic".to_string(),
            icon: None,
            description: None,
            kinds: &[ContextKind::Files],
        },
    });

    let j1 = dispatcher.dispatch(ok_action, ctx.clone());
    let j2 = dispatcher.dispatch(panic_action, ctx);

    let ev1 = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let ev2 = rx.recv_timeout(Duration::from_secs(2)).unwrap();

    for ev in [ev1, ev2] {
        match ev {
            JobEvent::Finished {
                job,
                action,
                result,
            } if job == j1 => {
                assert_eq!(action, ActionId::from("radivine.copy"));
                assert_eq!(result.unwrap(), Outcome::Done);
            }
            JobEvent::Finished {
                job,
                action,
                result,
            } if job == j2 => {
                assert_eq!(action, ActionId::from("test.panic"));
                let err = result.unwrap_err();
                assert_eq!(err.to_string(), "internal error");
            }
            other => panic!("unexpected JobEvent: {other:?}"),
        }
    }
}

#[test]
fn test_sniff_bytes_encodings_magic_and_control_bytes() {
    // 1. Empty -> Text(Utf8)
    assert_eq!(
        sniff_bytes(b""),
        (TextVerdict::Text(TextEncoding::Utf8), None)
    );

    // 2. BOMs
    assert_eq!(
        sniff_bytes(b"\xEF\xBB\xBFhello world\n"),
        (TextVerdict::Text(TextEncoding::Utf8Bom), None)
    );
    assert_eq!(
        sniff_bytes(b"\xFF\xFEh\x00i\x00"),
        (TextVerdict::Text(TextEncoding::Utf16Le), None)
    );
    assert_eq!(
        sniff_bytes(b"\xFE\xFF\x00h\x00i"),
        (TextVerdict::Text(TextEncoding::Utf16Be), None)
    );

    // 3. Magic numbers -> Binary + sniffed_mime
    assert_eq!(
        sniff_bytes(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR"),
        (TextVerdict::Binary, Some("image/png"))
    );
    assert_eq!(
        sniff_bytes(b"\x7FELF\x02\x01\x01\x00"),
        (TextVerdict::Binary, Some("application/x-executable"))
    );
    assert_eq!(
        sniff_bytes(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n"),
        (TextVerdict::Binary, Some("application/pdf"))
    );
    assert_eq!(
        sniff_bytes(b"PK\x03\x04\x14\x00\x00\x00"),
        (TextVerdict::Binary, Some("application/zip"))
    );

    // 4. NUL byte -> Binary
    assert_eq!(
        sniff_bytes(b"some text with a \x00 nul byte inside"),
        (TextVerdict::Binary, None)
    );

    // 5. Valid UTF-8 with an incomplete 3-byte sequence at the 8 KiB boundary
    let mut boundary_utf8 = vec![b'a'; SNIFF_BUFFER_BYTES - 2];
    // '€' is E2 82 AC; put only E2 82 at the end of the 8 KiB buffer
    boundary_utf8.extend_from_slice(&[0xE2, 0x82]);
    assert_eq!(
        sniff_bytes(&boundary_utf8),
        (TextVerdict::Text(TextEncoding::Utf8), None)
    );

    // 6. Legacy 8-bit text (ISO-8859-1 "café\n") with 0% disallowed control bytes -> Unknown8Bit
    assert_eq!(
        sniff_bytes(b"caf\xE9 au lait\n"),
        (TextVerdict::Text(TextEncoding::Unknown8Bit), None)
    );

    // 7. > 10% control bytes -> Binary
    let ctrl_heavy = b"\x01\x02\x03hello";
    assert_eq!(sniff_bytes(ctrl_heavy), (TextVerdict::Binary, None));
}

#[test]
fn test_enrich_item_regular_binary_directory_permissions_symlinks_and_fifo() {
    use std::os::unix::fs::PermissionsExt;

    let tmp_dir = std::env::temp_dir().join(format!("radivine_core_enrich_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // 1. Regular UTF-8 text file
    let txt_path = tmp_dir.join("notes.txt");
    std::fs::write(&txt_path, "Hello Radivine M4!\nLine 2\n").unwrap();
    let txt_item = Item::from_uri(&local_path_to_file_uri(&txt_path));
    let (is_sym, facts) = enrich_item(&txt_item);
    assert!(!is_sym);
    assert_eq!(facts.kind, FileKind::Regular);
    assert!(facts.readable);
    assert_eq!(facts.text, TextVerdict::Text(TextEncoding::Utf8));
    assert_eq!(facts.error, None);

    // 2. PNG renamed to .txt (AC-4.1: guessed_mime is text/plain, sniffed is Binary + image/png)
    let fake_txt_path = tmp_dir.join("disguised.txt");
    std::fs::write(&fake_txt_path, b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR").unwrap();
    let fake_txt_item = Item::from_uri(&local_path_to_file_uri(&fake_txt_path));
    assert_eq!(fake_txt_item.guessed_mime, Some("text/plain"));
    let (is_sym, facts) = enrich_item(&fake_txt_item);
    assert!(!is_sym);
    assert_eq!(facts.kind, FileKind::Regular);
    assert!(facts.readable);
    assert_eq!(facts.text, TextVerdict::Binary);
    assert_eq!(facts.sniffed_mime, Some("image/png"));

    // 3. Directory
    let sub_dir = tmp_dir.join("folder");
    std::fs::create_dir_all(&sub_dir).unwrap();
    let dir_item = Item::from_uri(&local_path_to_file_uri(&sub_dir));
    let (_, facts) = enrich_item(&dir_item);
    assert_eq!(facts.kind, FileKind::Directory);
    assert!(facts.readable);
    assert_eq!(facts.text, TextVerdict::Unknown);

    // 4. Unreadable file (0o000)
    let secret_path = tmp_dir.join("secret.txt");
    std::fs::write(&secret_path, "classified").unwrap();
    std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let secret_item = Item::from_uri(&local_path_to_file_uri(&secret_path));
    let (_, facts) = enrich_item(&secret_item);
    assert_eq!(facts.kind, FileKind::Regular);
    assert!(!facts.readable);
    assert_eq!(facts.error.as_deref(), Some("No permission to read"));
    // Restore permissions for cleanup
    std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600)).unwrap();

    // 5. Valid symlink and broken symlink
    let valid_link = tmp_dir.join("link_ok.txt");
    std::os::unix::fs::symlink(&txt_path, &valid_link).unwrap();
    let (is_sym, facts) = enrich_item(&Item::from_uri(&local_path_to_file_uri(&valid_link)));
    assert!(is_sym);
    assert_eq!(facts.kind, FileKind::Regular);
    assert!(facts.readable);
    assert_eq!(facts.text, TextVerdict::Text(TextEncoding::Utf8));

    let broken_link = tmp_dir.join("link_broken.txt");
    std::os::unix::fs::symlink(tmp_dir.join("does_not_exist.txt"), &broken_link).unwrap();
    let (is_sym, facts) = enrich_item(&Item::from_uri(&local_path_to_file_uri(&broken_link)));
    assert!(is_sym);
    assert_eq!(facts.kind, FileKind::BrokenSymlink);
    assert!(!facts.readable);
    assert_eq!(facts.error.as_deref(), Some("Broken link"));

    // 6. Missing file
    let missing_path = tmp_dir.join("missing.txt");
    let (is_sym, facts) = enrich_item(&Item::from_uri(&local_path_to_file_uri(&missing_path)));
    assert!(!is_sym);
    assert_eq!(facts.kind, FileKind::Missing);
    assert!(!facts.readable);
    assert_eq!(facts.error.as_deref(), Some("File not found"));

    // 7. Real named pipe (FIFO) — must NEVER block even with no writer attached!
    let fifo_path = tmp_dir.join("test_pipe.fifo");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo_path,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .unwrap();
    let start = std::time::Instant::now();
    let (_, facts) = enrich_item(&Item::from_uri(&local_path_to_file_uri(&fifo_path)));
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "FIFO enrichment must not block"
    );
    assert_eq!(facts.kind, FileKind::Fifo);
    assert!(!facts.readable);
    assert_eq!(facts.error.as_deref(), Some("Special file"));

    // 8. Non-local URI
    let remote_item = Item::from_uri("sftp://remote.example.com/var/log/syslog");
    let (_, facts) = enrich_item(&remote_item);
    assert_eq!(facts.kind, FileKind::Other);
    assert!(!facts.readable);
    assert_eq!(facts.error.as_deref(), Some("Remote file"));

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_enricher_hung_stat_times_out_to_couldnt_inspect_file_ac_4_2() {
    let (tx, rx) = mpsc::channel();
    let mut enricher = Enricher::new(Box::new(move |ev| {
        let _ = tx.send(ev);
    }))
    .with_timeout(Duration::from_millis(200))
    .with_inspector(Arc::new(|_item| {
        // Simulate a hung NFS/GVFS stat that blocks far longer than the session deadline
        std::thread::sleep(Duration::from_secs(3));
        unreachable_or_ignored()
    }));

    let basic = BasicContext::from_uris(
        ["file:///mnt/hung-nfs/report.txt"],
        InvocationMode::Drag,
        SourceInfo::default(),
    );

    let start = std::time::Instant::now();
    enricher.start(42, basic);

    let ev = rx
        .recv_timeout(Duration::from_millis(1000))
        .expect("watchdog should emit timeout EnrichEvent without waiting for hung worker");
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(150) && elapsed < Duration::from_millis(800),
        "expected ~200ms timeout, got {elapsed:?}"
    );

    match ev {
        EnrichEvent::Updated {
            session_id,
            context,
            complete,
        } => {
            assert_eq!(session_id, 42);
            assert!(complete);
            let facts = context.facts.as_ref().expect("facts should be populated");
            assert_eq!(facts.len(), 1);
            assert!(!facts[0].readable);
            assert_eq!(facts[0].text, TextVerdict::Unknown);
            assert_eq!(facts[0].error.as_deref(), Some("Couldn't inspect the file"));
        }
    }

    // Verify enrich_context_sync works as expected as well
    let sync_ctx = enrich_context_sync(BasicContext::from_uris(
        ["sftp://host/file.txt"],
        InvocationMode::Invoke,
        SourceInfo::default(),
    ));
    assert_eq!(
        sync_ctx.facts.unwrap()[0].error.as_deref(),
        Some("Remote file")
    );
}

fn unreachable_or_ignored() -> (bool, radivine_core::FileFacts) {
    (false, radivine_core::FileFacts::timed_out())
}

struct DynamicDescriptionAction {
    meta: ActionMeta,
}

impl Action for DynamicDescriptionAction {
    fn meta(&self) -> &ActionMeta {
        &self.meta
    }

    fn is_visible(&self, _ctx: &BasicContext) -> bool {
        true
    }

    fn description_for(&self, ctx: &BasicContext) -> Option<std::borrow::Cow<'_, str>> {
        Some(std::borrow::Cow::Owned(format!(
            "Convert {} item(s)",
            ctx.items.len()
        )))
    }

    fn run(&self, _ctx: &Context, _job: &JobHandle) -> Result<Outcome, ActionError> {
        Ok(Outcome::Done)
    }
}

#[test]
fn test_e1_dynamic_action_description_for_populates_menu_node() {
    let mut reg = ActionRegistry::new();
    reg.register(Arc::new(DynamicDescriptionAction {
        meta: ActionMeta {
            id: ActionId::from("test.dynamic"),
            label: "Dynamic".to_string(),
            icon: None,
            description: Some("Static fallback".to_string()),
            kinds: &[ContextKind::Files],
        },
    }))
    .unwrap();
    reg.set_menu(
        vec![MenuSpec::Action(ActionId::from("test.dynamic"))],
        MenuOverrides::default(),
    );

    let ctx = Context::from_basic(BasicContext::from_uris(
        [
            "file:///tmp/a.png",
            "file:///tmp/b.png",
            "file:///tmp/c.png",
        ],
        InvocationMode::Drag,
        SourceInfo::default(),
    ));
    let (tree, _) = reg.build_menu(&ctx, MenuLimits::default()).unwrap();
    let leaf = &tree.root.children().unwrap()[0];
    assert_eq!(leaf.description.as_deref(), Some("Convert 3 item(s)"));
}

#[test]
fn test_e2_file_facts_is_uninspected_vs_pending_and_inspected() {
    use radivine_core::{
        CAP_EXCEEDED_REASON, ENRICH_TIMEOUT_REASON, FileFacts, MAX_ENRICHED_ITEMS,
    };

    let pending = FileFacts::pending();
    assert!(pending.is_pending());
    assert!(!pending.is_uninspected());

    let timed_out = FileFacts::timed_out();
    assert!(!timed_out.is_pending());
    assert!(timed_out.is_uninspected());
    assert_eq!(timed_out.error.as_deref(), Some(ENRICH_TIMEOUT_REASON));

    let cap_exceeded = FileFacts::cap_exceeded();
    assert!(!cap_exceeded.is_pending());
    assert!(cap_exceeded.is_uninspected());
    assert_eq!(cap_exceeded.error.as_deref(), Some(CAP_EXCEEDED_REASON));

    // Inspected regular file or real error must NOT be treated as uninspected
    let regular_ok = FileFacts {
        kind: FileKind::Regular,
        size: Some(42),
        readable: true,
        text: TextVerdict::Binary,
        sniffed_mime: Some("image/png"),
        error: None,
    };
    assert!(!regular_ok.is_uninspected());

    let no_perm = FileFacts {
        kind: FileKind::Regular,
        size: Some(42),
        readable: false,
        text: TextVerdict::Unknown,
        sniffed_mime: None,
        error: Some("No permission to read".to_string()),
    };
    assert!(!no_perm.is_uninspected());

    // Enriching 70 items marks items 64..70 as cap_exceeded -> is_uninspected() == true
    let uris: Vec<String> = (0..70)
        .map(|i| format!("file:///tmp/nonexistent_item_{i}.png"))
        .collect();
    let ctx = enrich_context_sync(BasicContext::from_uris(
        uris,
        InvocationMode::Drag,
        SourceInfo::default(),
    ));
    let facts = ctx.facts.unwrap();
    assert_eq!(facts.len(), 70);
    for f in &facts[..MAX_ENRICHED_ITEMS] {
        assert_eq!(f.kind, FileKind::Missing);
        assert!(!f.is_uninspected());
    }
    for f in &facts[MAX_ENRICHED_ITEMS..] {
        assert!(f.is_uninspected());
        assert_eq!(f.error.as_deref(), Some(CAP_EXCEEDED_REASON));
    }
}

#[cfg(unix)]
#[test]
fn test_e8_lossless_non_utf8_path_to_file_uri_and_cli_normalization() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let raw_rel = OsStr::from_bytes(b"sub dir/photo_\x80\xff_raw.PNG");
    let base = Path::new("/tmp/radivine_e8");
    let uri = normalize_cli_arg_to_uri(raw_rel, base).unwrap();
    assert_eq!(
        uri,
        "file:///tmp/radivine_e8/sub%20dir/photo_%80%FF_raw.PNG"
    );

    let item = Item::from_uri(&uri);
    let expected_abs = PathBuf::from(OsStr::from_bytes(
        b"/tmp/radivine_e8/sub dir/photo_\x80\xff_raw.PNG",
    ));
    assert_eq!(item.local_path.as_ref(), Some(&expected_abs));
    assert_eq!(
        item.local_path.as_ref().unwrap().as_os_str().as_bytes(),
        expected_abs.as_os_str().as_bytes()
    );
    assert_eq!(item.ext.as_deref(), Some("png"));
    assert_eq!(item.guessed_mime, Some("image/png"));
}
