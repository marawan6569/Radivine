//! `radivine-actions` — Built-in context-aware actions, D-Bus integration, and `actions.toml`
//! user-defined custom actions (`ACTION_SYSTEM.md` §6.1, §8, §9, §10).

#![forbid(unsafe_code)]

pub mod builtins;
pub mod convert;
pub mod custom;
pub use radivine_dbus as dbus;

use std::sync::Arc;

use radivine_core::{
    ActionId, ActionMeta, ActionRegistry, ContextKind, IconKey, LogAction, MenuSpec, RegistryError,
};

pub use builtins::{
    CopyAction, PreviewAction, PreviewKind, PreviewPlaceholderAction, PreviewSettings,
    PropertiesAction, PropertiesBackend, SessionDbusPropertiesBackend, evaluate_text_file_facts,
    is_known_non_text_mime, resolve_viewer_binary,
};
pub use convert::{
    AdvancedConvertAction, ConvertAction, ConvertMenuContext, encode_advanced_convert_stdin,
    register_convert_actions, resolve_control_binary,
};
pub use custom::{
    ActionsConfig, ActionsLoadError, ArgPlaceholder, ArgToken, CountRange, CustomAction,
    NotifyPolicy, WhenRule, load_actions_file, mime_glob_matches, parse_actions_toml,
};
pub use dbus::{
    DbusError, call_desktop_notify, call_show_item_properties, call_show_items,
    resolve_session_bus_path,
};

/// Returns the default declarative [`MenuSpec`] tree (`ACTION_SYSTEM.md` §6.1, `CONVERSION_ARCHITECTURE.md` §7.1).
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
        MenuSpec::Submenu {
            id: Arc::from("radivine.convert"),
            label: "Convert".to_string(),
            icon: Some(IconKey::from("lucide:arrow-right-left")),
            description: Some("Convert to another format".to_string()),
            children: vec![MenuSpec::Submenu {
                id: Arc::from("radivine.convert.image"),
                label: "Image".to_string(),
                icon: Some(IconKey::from("lucide:image")),
                description: Some("Image formats".to_string()),
                children: vec![
                    MenuSpec::Action(ActionId::from("radivine.convert.image.png")),
                    MenuSpec::Action(ActionId::from("radivine.convert.image.jpeg")),
                    MenuSpec::Action(ActionId::from("radivine.convert.image.webp")),
                    MenuSpec::Action(ActionId::from("radivine.convert.image.advanced")),
                ],
                default: None,
            }],
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
    register_builtins_with_options(
        registry,
        &ActionsConfig::default(),
        &PreviewSettings::default(),
        &ConvertMenuContext::default(),
    )
}

/// Registers all built-in actions together with validated user custom actions and `[menu]` overrides
/// from [`ActionsConfig`] (`ACTION_SYSTEM.md` §8–§10).
pub fn register_builtins_with_config(
    registry: &mut ActionRegistry,
    config: &ActionsConfig,
) -> Result<(), RegistryError> {
    register_builtins_with_options(
        registry,
        config,
        &PreviewSettings::default(),
        &ConvertMenuContext::default(),
    )
}

/// Registers all built-in actions with explicit [`PreviewSettings`] together with validated user custom
/// actions and `[menu]` overrides from [`ActionsConfig`] (`ACTION_SYSTEM.md` §8–§10).
pub fn register_builtins_with_preview(
    registry: &mut ActionRegistry,
    config: &ActionsConfig,
    preview: &PreviewSettings,
) -> Result<(), RegistryError> {
    register_builtins_with_options(registry, config, preview, &ConvertMenuContext::default())
}

/// Registers all built-in actions (`Preview`, `Convert`, `Copy`, `Properties`) with explicit
/// [`PreviewSettings`] and [`ConvertMenuContext`] together with validated user custom actions and
/// `[menu]` overrides from [`ActionsConfig`].
pub fn register_builtins_with_options(
    registry: &mut ActionRegistry,
    config: &ActionsConfig,
    preview: &PreviewSettings,
    convert_ctx: &ConvertMenuContext,
) -> Result<(), RegistryError> {
    // 1. Real file preview actions (`radivine.preview.full`, `radivine.preview.head`, `radivine.preview.tail`)
    let head_lines = preview.head_lines.max(1);
    let tail_lines = preview.tail_lines.max(1);
    let head_desc = format!("Preview first {head_lines} lines of file");
    let tail_desc = format!("Preview last {tail_lines} lines of file");

    let preview_builtins = [
        PreviewAction::with_settings(
            "radivine.preview.full",
            "Full Preview",
            "lucide:file-text",
            "Open full interactive document preview",
            PreviewKind::Full,
            50,
            &preview.theme,
        ),
        PreviewAction::with_settings(
            "radivine.preview.head",
            "Head",
            "lucide:arrow-up-to-line",
            &head_desc,
            PreviewKind::Head,
            head_lines,
            &preview.theme,
        ),
        PreviewAction::with_settings(
            "radivine.preview.tail",
            "Tail",
            "lucide:arrow-down-to-line",
            &tail_desc,
            PreviewKind::Tail,
            tail_lines,
            &preview.theme,
        ),
    ];
    for action in preview_builtins {
        registry.register(Arc::new(action))?;
    }

    // 2. Quick Convert actions (`radivine.convert.image.{png,jpeg,webp}`)
    register_convert_actions(registry, convert_ctx)?;

    // 3. Real Copy and Properties actions
    registry.register(Arc::new(CopyAction::new()))?;
    registry.register(Arc::new(PropertiesAction::new()))?;

    // 4. User-defined custom actions from `actions.toml` (registered before built-in custom demo items
    //    so user actions appear first in `Custom`)
    for custom_act in &config.actions {
        registry.register(Arc::new(custom_act.clone()))?;
    }

    // 5. Non-destructive built-in custom demo entries (skipped if overridden by a user custom action with the same id)
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

/// Constructs a new [`ActionRegistry`] populated with built-ins, [`PreviewSettings`], and [`ActionsConfig`].
pub fn registry_from_actions_and_preview_config(
    config: &ActionsConfig,
    preview: &PreviewSettings,
) -> Result<ActionRegistry, RegistryError> {
    let mut reg = ActionRegistry::new();
    register_builtins_with_preview(&mut reg, config, preview)?;
    Ok(reg)
}

/// Constructs a new [`ActionRegistry`] populated with built-ins, [`PreviewSettings`],
/// [`ConvertMenuContext`], and [`ActionsConfig`].
pub fn registry_from_options(
    config: &ActionsConfig,
    preview: &PreviewSettings,
    convert_ctx: &ConvertMenuContext,
) -> Result<ActionRegistry, RegistryError> {
    let mut reg = ActionRegistry::new();
    register_builtins_with_options(&mut reg, config, preview, convert_ctx)?;
    Ok(reg)
}
