//! Unit tests for `radivine-core` (URI list parsing, `ActionRegistry::build_menu`, and `Dispatcher`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use radivine_core::{
    Action, ActionError, ActionId, ActionMeta, ActionRegistry, BasicContext, Context, ContextKind,
    Dispatcher, Enablement, InvocationMode, JobEvent, JobHandle, LogAction, MenuLimits,
    MenuOverrides, MenuSpec, Outcome, RegistryError, SourceInfo, normalize_cli_arg_to_uri,
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
