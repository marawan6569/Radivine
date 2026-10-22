//! `radivine-actions` — Built-in context-aware actions, D-Bus integration, and `actions.toml`
//! user-defined custom actions (`ACTION_SYSTEM.md` §6.1, §8, §9, §10).

#![forbid(unsafe_code)]

pub mod builtins;
pub mod custom;
pub mod dbus;

use std::sync::Arc;

use radivine_core::{
    ActionId, ActionMeta, ActionRegistry, ContextKind, IconKey, LogAction, MenuSpec, RegistryError,
};

pub use builtins::{
    CopyAction, PreviewPlaceholderAction, PropertiesAction, PropertiesBackend,
    SessionDbusPropertiesBackend, evaluate_text_file_facts, is_known_non_text_mime,
};
pub use custom::{
    ActionsConfig, ActionsLoadError, ArgPlaceholder, ArgToken, CountRange, CustomAction,
    NotifyPolicy, WhenRule, load_actions_file, mime_glob_matches, parse_actions_toml,
};
pub use dbus::{
    DbusError, call_desktop_notify, call_show_item_properties, resolve_session_bus_path,
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

/// Registers all built-in actions and sets [`default_menu`] on `registry`.
pub fn register_builtins(registry: &mut ActionRegistry) -> Result<(), RegistryError> {
    register_builtins_with_config(registry, &ActionsConfig::default())
}

/// Registers all built-in actions together with validated user custom actions and `[menu]` overrides
/// from [`ActionsConfig`] (`ACTION_SYSTEM.md` §8–§10).
pub fn register_builtins_with_config(
    registry: &mut ActionRegistry,
    config: &ActionsConfig,
) -> Result<(), RegistryError> {
    // 1. Preview actions (M4 visibility + FileFacts enablement; M5 viewer placeholder on execution)
    let preview_builtins = [
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
    ];
    for (id, label, icon, desc) in preview_builtins {
        registry.register(Arc::new(PreviewPlaceholderAction::new(
            id, label, icon, desc,
        )))?;
    }

    // 2. Real Copy and Properties actions
    registry.register(Arc::new(CopyAction::new()))?;
    registry.register(Arc::new(PropertiesAction::new()))?;

    // 3. User-defined custom actions from `actions.toml` (registered before built-in custom demo items
    //    so user actions appear first in `Custom`)
    for custom_act in &config.actions {
        registry.register(Arc::new(custom_act.clone()))?;
    }

    // 4. Non-destructive built-in custom demo entries (skipped if overridden by a user custom action with the same id)
    let demo_customs = [
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
    for (id, label, icon, desc) in demo_customs {
        let act_id = ActionId::from(id);
        if registry.get(&act_id).is_none() {
            registry.register(Arc::new(LogAction::new(ActionMeta {
                id: act_id,
                label: label.to_string(),
                icon: Some(IconKey::from(icon)),
                description: Some(desc.to_string()),
                kinds: &[ContextKind::Files],
            })))?;
        }
    }

    registry.set_menu(default_menu(), config.overrides.clone());
    Ok(())
}

/// Convenience helper that constructs a new [`ActionRegistry`] pre-populated with [`register_builtins`].
#[must_use]
pub fn builtin_registry() -> ActionRegistry {
    let mut reg = ActionRegistry::new();
    register_builtins(&mut reg).expect("built-in action IDs are unique");
    reg
}

/// Constructs a new [`ActionRegistry`] populated with built-ins and the given [`ActionsConfig`].
pub fn registry_from_actions_config(
    config: &ActionsConfig,
) -> Result<ActionRegistry, RegistryError> {
    let mut reg = ActionRegistry::new();
    register_builtins_with_config(&mut reg, config)?;
    Ok(reg)
}
