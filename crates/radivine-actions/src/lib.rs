//! `radivine-actions` — Built-in actions and default menu specification (`ACTION_SYSTEM.md` §6.1, §8).
//!
//! In Milestones M2–M3, all built-in actions are registered as [`radivine_core::LogAction`]s with
//! their production metadata and menu structure (`IMPLEMENTATION_PLAN.md` §M2 Task 3).

#![forbid(unsafe_code)]

use std::sync::Arc;

use radivine_core::{
    ActionId, ActionMeta, ActionRegistry, ContextKind, IconKey, LogAction, MenuOverrides, MenuSpec,
    RegistryError,
};

/// Returns the default declarative [`MenuSpec`] tree (`ACTION_SYSTEM.md` §6.1).
#[must_use]
pub fn default_menu() -> Vec<MenuSpec> {
    vec![
        MenuSpec::Submenu {
            id: Arc::from("radivine.preview"),
            label: "File Preview".to_string(),
            icon: Some(IconKey::from("lucide:eye")),
            description: Some("Preview the file".to_string()),
            children: vec![
                MenuSpec::Action(ActionId::from("radivine.preview.full")),
                MenuSpec::Action(ActionId::from("radivine.preview.head")),
                MenuSpec::Action(ActionId::from("radivine.preview.tail")),
            ],
            default: None,
        },
        MenuSpec::Action(ActionId::from("radivine.copy")),
        MenuSpec::Action(ActionId::from("radivine.properties")),
        MenuSpec::Submenu {
            id: Arc::from("radivine.custom"),
            label: "Custom".to_string(),
            icon: Some(IconKey::from("lucide:sparkles")),
            description: Some("Your custom actions".to_string()),
            children: vec![
                MenuSpec::CustomActions,
                MenuSpec::Submenu {
                    id: Arc::from("custom.scripts"),
                    label: "Scripts".to_string(),
                    icon: Some(IconKey::from("lucide:code")),
                    description: Some("Run user automation scripts".to_string()),
                    children: vec![
                        MenuSpec::Action(ActionId::from("custom.scripts.sha256")),
                        MenuSpec::Action(ActionId::from("custom.scripts.chmod")),
                    ],
                    default: None,
                },
            ],
            default: None,
        },
    ]
}

/// Registers all built-in actions (as [`LogAction`]s for M2–M3) and sets [`default_menu`] on `registry`.
pub fn register_builtins(registry: &mut ActionRegistry) -> Result<(), RegistryError> {
    let builtins = [
        (
            "radivine.preview.full",
            "Full Preview",
            "lucide:file-text",
            "Open full interactive document preview",
        ),
        (
            "radivine.preview.head",
            "Head",
            "lucide:arrow-up-to-line",
            "Preview first 50 lines of file",
        ),
        (
            "radivine.preview.tail",
            "Tail",
            "lucide:arrow-down-to-line",
            "Preview last 50 lines of file",
        ),
        (
            "radivine.copy",
            "Copy",
            "lucide:copy",
            "Copy selected file to clipboard",
        ),
        (
            "radivine.properties",
            "Properties",
            "lucide:info",
            "Show file properties",
        ),
        (
            "custom.terminal",
            "Open in Terminal",
            "lucide:terminal",
            "Launch terminal in parent directory",
        ),
        (
            "custom.scripts.sha256",
            "Compute SHA-256",
            "lucide:code",
            "Calculate SHA-256 digest",
        ),
        (
            "custom.scripts.chmod",
            "Make Executable",
            "lucide:sparkles",
            "Set +x permission bit",
        ),
    ];

    for (id, label, icon, desc) in builtins {
        let action = Arc::new(LogAction::new(ActionMeta {
            id: ActionId::from(id),
            label: label.to_string(),
            icon: Some(IconKey::from(icon)),
            description: Some(desc.to_string()),
            kinds: &[ContextKind::Files],
        }));
        registry.register(action)?;
    }

    registry.set_menu(default_menu(), MenuOverrides::default());
    Ok(())
}

/// Convenience helper that constructs a new [`ActionRegistry`] pre-populated with [`register_builtins`].
#[must_use]
pub fn builtin_registry() -> ActionRegistry {
    let mut reg = ActionRegistry::new();
    register_builtins(&mut reg).expect("built-in action IDs are unique");
    reg
}
