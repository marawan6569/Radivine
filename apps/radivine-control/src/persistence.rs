//! Safe, format-preserving TOML configuration and custom-actions persistence (`toml_edit`).
//!
//! Guarantees:
//! 1. Validates the complete resulting `config.toml` or `actions.toml` document before touching disk.
//! 2. Preserves comments, formatting, and unknown supported keys via `toml_edit::DocumentMut`.
//! 3. Detects external modification conflicts (`mtime` + content digest) before overwriting.
//! 4. Supports creating a `.bak` backup file for safe conflict resolution or recovery.
//! 5. Replaces files atomically within the same directory (`.tmp.<pid>` + `sync_all` + `rename`)
//!    while preserving existing file permissions.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use radivine::config::{AppConfig, ReducedMotionSetting, ScaleSetting};
use radivine_actions::{
    ActionsConfig, ArgToken, CountRange, CustomAction, NotifyPolicy, load_actions_file,
    parse_actions_toml,
};
use radivine_convert::ConvertSettings;
use radivine_jobs::{JobsSettings, MaxParallelSetting, MemoryBudgetSetting};
use toml_edit::{Array, DocumentMut, Item as TomlItem, Table, value};

/// Default commented `config.toml` template used when creating a new user configuration file.
pub const DEFAULT_CONFIG_TOML_TEMPLATE: &str = r#"# Radivine Configuration (`~/.config/radivine/config.toml`)
# Managed by Radivine Control Center (`radivine-control`) or edited manually.

version = 1

[trigger]
# Modifier key pressed during an active file drag: "shift", "ctrl", "alt", "super", or "disabled"
drag_modifier = "shift"
# Press the modifier key a second time mid-drag to hide the wheel and continue dragging normally
toggle_off_on_repress = true
# Require holding the modifier key to keep the wheel open
hold_to_keep_open = false
# Minimum drag distance in pixels before arming
min_drag_distance_px = 6

[wheel]
# Radial wheel layout mode: "branching" (default) or "concentric"
layout = "branching"
# Root wheel inner and outer radii in logical pixels
root_inner_radius = 40.0
root_outer_radius = 128.0
# Child submenu wheel inner and outer radii in logical pixels
child_inner_radius = 34.0
child_outer_radius = 108.0
# Radial gap between parent outer radius and child inner radius in logical pixels
child_offset_gap = 20.0
# Hover dwell time in milliseconds before opening a submenu (0..=2000)
hover_dwell_ms = 110
# Grace period in milliseconds before closing a submenu during diagonal travel (0..=2000)
submenu_grace_ms = 180
# Grace period in milliseconds after pointer leaves the shaped input region in drag mode
leave_grace_ms = 350
# Angular hysteresis in degrees at segment boundaries (0.0..=30.0)
angular_hysteresis_deg = 3.5

[appearance]
# Color theme: "dark", "light", or "auto"
theme = "dark"
# UI scale factor: "auto" (queries Xft.dpi) or a numeric factor in 0.5..=4.0
scale = "auto"
# Reduced motion: "auto", true, or false
reduced_motion = "auto"
# Full-monitor backdrop dim alpha (0.0..=0.5)
backdrop_dim = 0.0

[notifications]
# Master switch for desktop notifications
enabled = true
# Emit a desktop notification when an action fails
notify_on_action_error = true
# Emit a desktop notification when actions.toml has validation warnings
notify_on_config_warning = true

[preview]
# Default line count for `File Preview -> Head` (1..=10000)
head_lines = 50
# Default line count for `File Preview -> Tail` (1..=10000)
tail_lines = 50

[input]
# Safety default: false (capture-all overlay during active sessions so releasing outside
# the wheel cancels cleanly instead of dropping files into underlying windows; ADR-0001).
passthrough = false
release_outside_cancels = true

[x11]
trigger_detection = "raw"
window_type = "dnd"
pointer_nudge = true
"#;

/// Default commented `actions.toml` header used when creating a new custom actions file.
pub const DEFAULT_ACTIONS_TOML_HEADER: &str = r#"# Radivine Custom Actions (`~/.config/radivine/actions.toml`)
# Managed by Radivine Control Center (`radivine-control`) or edited manually.
#
# Security Invariants:
# - Commands execute directly via execve without a shell.
# - Placeholders ({path}, {paths}, {uri}, {uris}, {dir}, {name}) must each be a whole argument.
# - {path} and {uri} require either per_item = true or when.count = "1".
"#;

/// Snapshot of a configuration file on disk used to detect external modification conflicts.
#[derive(Clone, Debug)]
pub struct DiskFileSnapshot {
    /// Target file path.
    pub path: PathBuf,
    /// Whether the file existed on disk when loaded.
    pub existed: bool,
    /// Modification timestamp when loaded, if the file existed.
    pub mtime: Option<SystemTime>,
    /// 64-bit FNV-1a content digest when loaded.
    pub content_hash: u64,
    /// Raw file text when loaded (or empty string if it did not exist).
    pub raw_text: String,
}

impl DiskFileSnapshot {
    /// Captures a snapshot of `path` from disk.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        if !path.exists() {
            return Self {
                path: path.to_path_buf(),
                existed: false,
                mtime: None,
                content_hash: fnv1a_hash(b""),
                raw_text: String::new(),
            };
        }
        let mtime = fs::metadata(path).ok().and_then(|m| m.modified().ok());
        let raw_text = fs::read_to_string(path).unwrap_or_default();
        let content_hash = fnv1a_hash(raw_text.as_bytes());
        Self {
            path: path.to_path_buf(),
            existed: true,
            mtime,
            content_hash,
            raw_text,
        }
    }

    /// Returns `true` if the file on disk has been created, deleted, or modified since this snapshot.
    #[must_use]
    pub fn has_external_conflict(&self) -> bool {
        let now_exists = self.path.exists();
        if !self.existed {
            return now_exists;
        }
        if !now_exists {
            return true;
        }
        if let Ok(current_bytes) = fs::read(&self.path) {
            return fnv1a_hash(&current_bytes) != self.content_hash;
        }
        false
    }
}

/// Editable representation of a single `[[action]]` entry for the Custom Actions Manager UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditableAction {
    /// Action identifier (with or without leading `"custom."`).
    pub id: String,
    /// Display segment label.
    pub label: String,
    /// Whether this action is enabled (`true` by default).
    pub enabled: bool,
    /// Optional icon key (e.g. `"lucide:terminal"`).
    pub icon: String,
    /// Optional caption description.
    pub description: String,
    /// Executable program path or name (`exec[0]`).
    pub executable: String,
    /// Discrete argument elements (`exec[1..]`).
    pub args: Vec<String>,
    /// Optional working directory (`""`, `"{dir}"`, `"{path}"`, or literal path).
    pub cwd: String,
    /// Whether to invoke once per item (`true`) or once for the batch (`false`).
    pub per_item: bool,
    /// Whether to wait for process completion (`true`) or run detached (`false`).
    pub wait: bool,
    /// Optional timeout in milliseconds when `wait = true`.
    pub timeout_ms: Option<u64>,
    /// Notification policy (`"never"`, `"on-error"`, `"always"`).
    pub notify: String,
    /// Allowed URI schemes (default `["file"]`).
    pub schemes: Vec<String>,
    /// Item count specification (`"1"`, `"1.."`, `"2..=5"`).
    pub count: String,
    /// MIME glob patterns (`["text/*", "application/json"]`).
    pub mime_globs: Vec<String>,
    /// File extensions without leading dot (`["rs", "toml", "md"]`).
    pub extensions: Vec<String>,
    /// Whether enablement requires `FileFacts` confirming a text file.
    pub require_text: bool,
}

impl Default for EditableAction {
    fn default() -> Self {
        Self {
            id: "custom.new_action".to_string(),
            label: "New Action".to_string(),
            enabled: true,
            icon: "lucide:sparkles".to_string(),
            description: String::new(),
            executable: "/usr/bin/xdg-open".to_string(),
            args: vec!["{path}".to_string()],
            cwd: String::new(),
            per_item: true,
            wait: false,
            timeout_ms: None,
            notify: "on-error".to_string(),
            schemes: vec!["file".to_string()],
            count: "1..".to_string(),
            mime_globs: Vec::new(),
            extensions: Vec::new(),
            require_text: false,
        }
    }
}

impl EditableAction {
    /// Converts a validated [`CustomAction`] into an [`EditableAction`] for form editing.
    #[must_use]
    pub fn from_custom_action(act: &CustomAction) -> Self {
        let exec_all = act.exec_strings();
        let executable = exec_all.first().cloned().unwrap_or_default();
        let args = if exec_all.len() > 1 {
            exec_all[1..].to_vec()
        } else {
            Vec::new()
        };
        Self {
            id: act.id_str().to_string(),
            label: act.label().to_string(),
            enabled: act.is_enabled(),
            icon: act.icon_str().unwrap_or("").to_string(),
            description: act.description().unwrap_or("").to_string(),
            executable,
            args,
            cwd: act.cwd_string().unwrap_or_default(),
            per_item: act.per_item(),
            wait: act.wait(),
            timeout_ms: act.timeout_ms(),
            notify: act.notify_policy().as_str().to_string(),
            schemes: act.when().schemes.clone(),
            count: act.when().count.to_spec_string(),
            mime_globs: act.when().mime_globs.clone(),
            extensions: act.when().extensions.clone(),
            require_text: act.when().require_text,
        }
    }

    /// Validates this [`EditableAction`] in isolation using Radivine's exact security and schema rules.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errs = Vec::new();
        let trimmed_id = self.id.trim();
        if trimmed_id.is_empty() {
            errs.push("Action ID cannot be empty.".to_string());
        } else if !trimmed_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            errs.push(
                "Action ID may only contain ASCII letters, digits, '-', '_', and '.'.".to_string(),
            );
        }

        if self.label.trim().is_empty() {
            errs.push("Display label cannot be empty.".to_string());
        }

        let prog = self.executable.trim();
        if prog.is_empty() {
            errs.push("Executable path/program cannot be empty.".to_string());
        } else {
            match ArgToken::parse(prog) {
                Ok(ArgToken::Placeholder(_)) => {
                    errs.push("Executable (exec[0]) cannot be a placeholder.".to_string());
                }
                Err(e) => errs.push(format!("Executable error: {e}")),
                Ok(ArgToken::Literal(_)) => {}
            }
            if matches!(prog, "sh" | "bash" | "zsh" | "/bin/sh" | "/bin/bash")
                && self.args.iter().any(|a| a.trim() == "-c")
            {
                errs.push(
                    "Shell '-c' execution is prohibited for security; specify the target program and whole-element arguments directly."
                        .to_string(),
                );
            }
        }

        let mut uses_single_placeholder = false;
        for (idx, arg) in self.args.iter().enumerate() {
            match ArgToken::parse(arg) {
                Ok(ArgToken::Placeholder(ph)) => {
                    if matches!(ph.as_str(), "{path}" | "{uri}") {
                        uses_single_placeholder = true;
                    }
                }
                Ok(ArgToken::Literal(_)) => {}
                Err(e) => errs.push(format!("Argument #{}: {e}", idx + 1)),
            }
        }

        if !self.cwd.trim().is_empty() {
            match ArgToken::parse(self.cwd.trim()) {
                Ok(ArgToken::Placeholder(ph)) if !matches!(ph.as_str(), "{dir}" | "{path}") => {
                    errs.push(
                        "Working directory (cwd) only supports literal paths, '{dir}', or '{path}'."
                            .to_string(),
                    );
                }
                Err(e) => errs.push(format!("Working directory (cwd): {e}")),
                _ => {}
            }
        }

        if let Err(e) = NotifyPolicy::parse(&self.notify) {
            errs.push(e);
        }

        match CountRange::parse(&self.count) {
            Ok(cr) => {
                if uses_single_placeholder && !self.per_item && !cr.is_single_only() {
                    errs.push(
                        "Using '{path}' or '{uri}' requires either 'Run once per item (per_item = true)' or 'Item count = 1'."
                            .to_string(),
                    );
                }
            }
            Err(e) => errs.push(format!("Item count filter: {e}")),
        }

        if let Some(ms) = self.timeout_ms
            && !(1..=3_600_000).contains(&ms)
        {
            errs.push("Timeout must be between 1 and 3,600,000 ms.".to_string());
        }

        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }

    /// Normalizes the action ID to include the `"custom."` prefix.
    #[must_use]
    pub fn canonical_id(&self) -> String {
        let t = self.id.trim();
        if t.starts_with("custom.") {
            t.to_string()
        } else {
            format!("custom.{t}")
        }
    }
}

/// Complete editable representation of `actions.toml` (`[menu]` overrides + `[[action]]` list).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditableActionsDoc {
    /// Hidden action/group IDs (`[menu] hide`).
    pub menu_hide: Vec<String>,
    /// Top-level or custom ordering IDs (`[menu] order`).
    pub menu_order: Vec<String>,
    /// Custom actions (`[[action]]`).
    pub actions: Vec<EditableAction>,
}

impl EditableActionsDoc {
    /// Builds an [`EditableActionsDoc`] from a parsed [`ActionsConfig`].
    #[must_use]
    pub fn from_actions_config(cfg: &ActionsConfig) -> Self {
        Self {
            menu_hide: cfg
                .overrides
                .hide
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            menu_order: cfg
                .overrides
                .order
                .iter()
                .map(ToString::to_string)
                .collect(),
            actions: cfg
                .actions
                .iter()
                .map(EditableAction::from_custom_action)
                .collect(),
        }
    }

    /// Loads `actions.toml` from `path` alongside its [`DiskFileSnapshot`] and any non-fatal warnings.
    pub fn load_with_snapshot(
        path: &Path,
    ) -> Result<(Self, DiskFileSnapshot, Vec<String>), String> {
        let snapshot = DiskFileSnapshot::load(path);
        let cfg = load_actions_file(path).map_err(|e| e.to_string())?;
        let warnings = cfg.warnings.clone();
        Ok((Self::from_actions_config(&cfg), snapshot, warnings))
    }
}

/// Updates `existing_toml` with the values in `cfg` using `toml_edit::DocumentMut`, preserving
/// comments, formatting, and unknown keys, and validates the complete resulting TOML document.
pub fn apply_app_config_to_toml_doc(
    existing_toml: &str,
    cfg: &AppConfig,
) -> Result<String, Vec<String>> {
    cfg.validate_strict()?;

    let base = if existing_toml.trim().is_empty() {
        DEFAULT_CONFIG_TOML_TEMPLATE
    } else {
        existing_toml
    };

    let mut doc: DocumentMut = base
        .parse::<DocumentMut>()
        .unwrap_or_else(|_| DEFAULT_CONFIG_TOML_TEMPLATE.parse::<DocumentMut>().unwrap());

    doc["version"] = value(i64::from(cfg.version));

    ensure_table(&mut doc, "trigger");
    doc["trigger"]["drag_modifier"] = value(cfg.trigger.drag_modifier.trim());
    doc["trigger"]["toggle_off_on_repress"] = value(cfg.trigger.toggle_off_on_repress);
    doc["trigger"]["hold_to_keep_open"] = value(cfg.trigger.hold_to_keep_open);
    doc["trigger"]["min_drag_distance_px"] = value(i64::from(cfg.trigger.min_drag_distance_px));

    ensure_table(&mut doc, "wheel");
    doc["wheel"]["layout"] = value(cfg.wheel.layout.trim().to_ascii_lowercase());
    doc["wheel"]["root_inner_radius"] = value(f64::from(cfg.wheel.root_inner_radius));
    doc["wheel"]["root_outer_radius"] = value(f64::from(cfg.wheel.root_outer_radius));
    doc["wheel"]["child_inner_radius"] = value(f64::from(cfg.wheel.child_inner_radius));
    doc["wheel"]["child_outer_radius"] = value(f64::from(cfg.wheel.child_outer_radius));
    doc["wheel"]["child_offset_gap"] = value(f64::from(cfg.wheel.child_offset_gap));
    doc["wheel"]["hover_dwell_ms"] = value(cfg.wheel.hover_dwell_ms as i64);
    doc["wheel"]["submenu_grace_ms"] = value(cfg.wheel.submenu_grace_ms as i64);
    doc["wheel"]["leave_grace_ms"] = value(cfg.wheel.leave_grace_ms as i64);
    doc["wheel"]["angular_hysteresis_deg"] = value(f64::from(cfg.wheel.angular_hysteresis_deg));

    ensure_table(&mut doc, "appearance");
    doc["appearance"]["theme"] = value(cfg.appearance.theme.trim());
    match cfg.appearance.scale {
        ScaleSetting::Auto => {
            doc["appearance"]["scale"] = value("auto");
        }
        ScaleSetting::Fixed(s) => {
            doc["appearance"]["scale"] = value(f64::from((s * 100.0).round() / 100.0));
        }
    }
    match cfg.appearance.reduced_motion {
        ReducedMotionSetting::Auto => {
            doc["appearance"]["reduced_motion"] = value("auto");
        }
        ReducedMotionSetting::Fixed(b) => {
            doc["appearance"]["reduced_motion"] = value(b);
        }
    }
    doc["appearance"]["backdrop_dim"] = value(f64::from(
        (cfg.appearance.backdrop_dim * 100.0).round() / 100.0,
    ));

    ensure_table(&mut doc, "notifications");
    doc["notifications"]["enabled"] = value(cfg.notifications.enabled);
    doc["notifications"]["notify_on_action_error"] =
        value(cfg.notifications.notify_on_action_error);
    doc["notifications"]["notify_on_config_warning"] =
        value(cfg.notifications.notify_on_config_warning);

    ensure_table(&mut doc, "preview");
    doc["preview"]["head_lines"] = value(cfg.preview.head_lines as i64);
    doc["preview"]["tail_lines"] = value(cfg.preview.tail_lines as i64);

    ensure_table(&mut doc, "input");
    doc["input"]["passthrough"] = value(cfg.input.passthrough);
    doc["input"]["release_outside_cancels"] = value(cfg.input.release_outside_cancels);

    ensure_table(&mut doc, "x11");
    doc["x11"]["trigger_detection"] = value(cfg.x11.trigger_detection.trim());
    doc["x11"]["window_type"] = value(cfg.x11.window_type.trim());
    doc["x11"]["pointer_nudge"] = value(cfg.x11.pointer_nudge);

    let out = doc.to_string();
    let parsed: AppConfig = toml::from_str(&out).map_err(|e| vec![e.to_string()])?;
    parsed.validate_strict()?;
    Ok(out)
}

/// Resolves the path to `convert.toml` adjacent to `config_path` (`~/.config/radivine/convert.toml`).
#[must_use]
pub fn convert_path_for(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("convert.toml")
}

/// Strictly validates [`JobsSettings`] ranges (`JOB_SYSTEM_DESIGN.md` §10).
pub fn validate_jobs_settings_strict(jobs: &JobsSettings) -> Result<(), Vec<String>> {
    let mut errs = Vec::new();
    if let MaxParallelSetting::Fixed(n) = jobs.max_parallel
        && !(1..=16).contains(&n)
    {
        errs.push(format!("jobs.max_parallel must be in 1..=16 (got {n})"));
    }
    if let MemoryBudgetSetting::FixedMb(mb) = jobs.memory_budget_mb
        && !(64..=4096).contains(&mb)
    {
        errs.push(format!(
            "jobs.memory_budget_mb must be in 64..=4096 MiB (got {mb})"
        ));
    }
    if !(5..=3600).contains(&jobs.idle_timeout_secs) {
        errs.push(format!(
            "jobs.idle_timeout_secs must be in 5..=3600 (got {})",
            jobs.idle_timeout_secs
        ));
    }
    if !(10..=1000).contains(&jobs.retention_max_jobs) {
        errs.push(format!(
            "jobs.retention_max_jobs must be in 10..=1000 (got {})",
            jobs.retention_max_jobs
        ));
    }
    if !(1..=90).contains(&jobs.retention_days) {
        errs.push(format!(
            "jobs.retention_days must be in 1..=90 (got {})",
            jobs.retention_days
        ));
    }
    if errs.is_empty() { Ok(()) } else { Err(errs) }
}

/// Updates `existing_toml` with both `cfg` (`AppConfig`) and `jobs` (`[jobs]`), preserving
/// comments, formatting, and unknown keys, and validates the complete resulting TOML document.
pub fn apply_app_and_jobs_config_to_toml_doc(
    existing_toml: &str,
    cfg: &AppConfig,
    jobs: &JobsSettings,
) -> Result<String, Vec<String>> {
    validate_jobs_settings_strict(jobs)?;
    let with_app = apply_app_config_to_toml_doc(existing_toml, cfg)?;
    let with_jobs = jobs
        .apply_to_toml_document(&with_app)
        .map_err(|e| vec![e])?;
    let parsed: AppConfig = toml::from_str(&with_jobs).map_err(|e| vec![e.to_string()])?;
    parsed.validate_strict()?;
    Ok(with_jobs)
}

/// Updates `existing_toml` (`convert.toml`) with `convert`, preserving comments, formatting, and
/// unknown keys, and validates the complete resulting TOML document (`AC-C4.4`).
pub fn apply_convert_settings_doc(
    existing_toml: &str,
    convert: &ConvertSettings,
) -> Result<String, Vec<String>> {
    let out = convert
        .save_preserving_comments(existing_toml)
        .map_err(|e| vec![e])?;
    ConvertSettings::validate_strict(&out)?;
    Ok(out)
}

/// Updates `existing_toml` with `actions_doc` using `toml_edit::DocumentMut`, preserving top-level
/// comments and unknown sections, and validates the complete resulting `actions.toml` document.
pub fn apply_actions_to_toml_doc(
    existing_toml: &str,
    actions_doc: &EditableActionsDoc,
) -> Result<String, Vec<String>> {
    let mut validation_errors = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();
    for act in &actions_doc.actions {
        if let Err(errs) = act.validate() {
            for e in errs {
                validation_errors.push(format!("{}: {e}", act.label));
            }
        }
        let cid = act.canonical_id();
        if !seen_ids.insert(cid.clone()) {
            validation_errors.push(format!("Duplicate custom action ID '{cid}'."));
        }
    }
    if !validation_errors.is_empty() {
        return Err(validation_errors);
    }

    let base = if existing_toml.trim().is_empty() {
        DEFAULT_ACTIONS_TOML_HEADER
    } else {
        existing_toml
    };

    let mut doc: DocumentMut = base
        .parse::<DocumentMut>()
        .unwrap_or_else(|_| DEFAULT_ACTIONS_TOML_HEADER.parse::<DocumentMut>().unwrap());

    if !actions_doc.menu_hide.is_empty()
        || !actions_doc.menu_order.is_empty()
        || doc.contains_key("menu")
    {
        ensure_table(&mut doc, "menu");
        let mut hide_arr = Array::new();
        for h in &actions_doc.menu_hide {
            if !h.trim().is_empty() {
                hide_arr.push(h.trim());
            }
        }
        let mut order_arr = Array::new();
        for o in &actions_doc.menu_order {
            if !o.trim().is_empty() {
                order_arr.push(o.trim());
            }
        }
        doc["menu"]["hide"] = value(hide_arr);
        doc["menu"]["order"] = value(order_arr);
    }

    let mut action_tables = toml_edit::ArrayOfTables::new();
    for act in &actions_doc.actions {
        let mut tbl = Table::new();
        tbl["id"] = value(act.canonical_id());
        tbl["label"] = value(act.label.trim());
        if !act.enabled {
            tbl["enabled"] = value(false);
        }
        if !act.icon.trim().is_empty() {
            tbl["icon"] = value(act.icon.trim());
        }
        if !act.description.trim().is_empty() {
            tbl["description"] = value(act.description.trim());
        }

        let mut exec_arr = Array::new();
        exec_arr.push(act.executable.trim());
        for arg in &act.args {
            exec_arr.push(arg.as_str());
        }
        tbl["exec"] = value(exec_arr);

        if !act.cwd.trim().is_empty() {
            tbl["cwd"] = value(act.cwd.trim());
        }
        tbl["per_item"] = value(act.per_item);
        tbl["wait"] = value(act.wait);
        if let Some(ms) = act.timeout_ms {
            tbl["timeout_ms"] = value(ms as i64);
        }
        tbl["notify"] = value(act.notify.trim());

        let mut when_tbl = Table::new();
        if !act.schemes.is_empty() && act.schemes != vec!["file".to_string()] {
            let mut s_arr = Array::new();
            for s in &act.schemes {
                if !s.trim().is_empty() {
                    s_arr.push(s.trim());
                }
            }
            when_tbl["scheme"] = value(s_arr);
        }
        if !act.count.trim().is_empty() {
            when_tbl["count"] = value(act.count.trim());
        }
        if !act.mime_globs.is_empty() {
            let mut m_arr = Array::new();
            for m in &act.mime_globs {
                if !m.trim().is_empty() {
                    m_arr.push(m.trim());
                }
            }
            when_tbl["mime"] = value(m_arr);
        }
        if !act.extensions.is_empty() {
            let mut e_arr = Array::new();
            for ext in &act.extensions {
                let clean = ext.trim().trim_start_matches('.').to_ascii_lowercase();
                if !clean.is_empty() {
                    e_arr.push(clean);
                }
            }
            when_tbl["extensions"] = value(e_arr);
        }
        if act.require_text {
            when_tbl["require_text"] = value(true);
        }
        tbl["when"] = TomlItem::Table(when_tbl);

        action_tables.push(tbl);
    }

    if action_tables.is_empty() {
        doc.remove("action");
    } else {
        doc["action"] = TomlItem::ArrayOfTables(action_tables);
    }

    let out = doc.to_string();
    let validated = parse_actions_toml(&out).map_err(|e| vec![e.to_string()])?;
    if !validated.warnings.is_empty() {
        return Err(validated.warnings);
    }
    Ok(out)
}

/// Atomically writes `new_content` to `path` within the same directory, preserving file
/// permissions and optionally creating a `<path>.bak` backup first.
pub fn atomic_write_with_backup(
    path: &Path,
    new_content: &str,
    create_backup: bool,
) -> anyhow::Result<DiskFileSnapshot> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("target path {} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)?;

    let existing_mode = fs::metadata(path)
        .ok()
        .map(|m| m.permissions().mode())
        .unwrap_or(0o644);

    if create_backup && path.exists() {
        let mut bak_os = path.as_os_str().to_os_string();
        bak_os.push(".bak");
        let bak_path = PathBuf::from(bak_os);
        fs::copy(path, &bak_path)?;
    }

    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("radivine.toml");
    let tmp_path = parent.join(format!(".{file_name}.tmp.{}", std::process::id()));

    {
        let mut tmp_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;
        let _ = tmp_file.set_permissions(fs::Permissions::from_mode(existing_mode));
        tmp_file.write_all(new_content.as_bytes())?;
        tmp_file.sync_all()?;
    }

    fs::rename(&tmp_path, path)?;
    Ok(DiskFileSnapshot::load(path))
}

fn ensure_table(doc: &mut DocumentMut, key: &str) {
    if !doc.get(key).is_some_and(TomlItem::is_table) {
        doc[key] = TomlItem::Table(Table::new());
    }
}

fn fnv1a_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_persistence_preserves_comments_and_unknown_fields() {
        let original = r#"# User's custom header comment
version = 1

[custom_extra_section]
keep_me = "preserved_value"

[wheel]
# Custom radius comment
root_inner_radius = 40.0
root_outer_radius = 128.0
"#;
        let mut cfg = AppConfig::from_toml_str(original);
        cfg.appearance.theme = "light".to_string();
        cfg.preview.head_lines = 120;

        let updated = apply_app_config_to_toml_doc(original, &cfg).expect("valid config");
        assert!(updated.contains("# User's custom header comment"));
        assert!(updated.contains("# Custom radius comment"));
        assert!(updated.contains("[custom_extra_section]"));
        assert!(updated.contains("keep_me = \"preserved_value\""));
        assert!(updated.contains("theme = \"light\""));
        assert!(updated.contains("head_lines = 120"));
    }

    #[test]
    fn actions_persistence_roundtrips_and_rejects_partial_placeholders() {
        let mut doc = EditableActionsDoc::default();
        let mut act = EditableAction {
            id: "sha256".to_string(),
            label: "SHA-256".to_string(),
            executable: "/usr/bin/sha256sum".to_string(),
            args: vec!["--file={path}".to_string()],
            per_item: true,
            ..EditableAction::default()
        };
        assert!(
            apply_actions_to_toml_doc(
                "",
                &EditableActionsDoc {
                    actions: vec![act.clone()],
                    ..EditableActionsDoc::default()
                }
            )
            .is_err(),
            "partial placeholder must be rejected"
        );

        act.args = vec!["{path}".to_string()];
        doc.actions.push(act);
        let serialized = apply_actions_to_toml_doc("# Header comment\n", &doc)
            .expect("whole-element placeholder should succeed");
        assert!(serialized.contains("# Header comment"));
        let parsed = parse_actions_toml(&serialized).unwrap();
        assert!(parsed.warnings.is_empty());
        assert_eq!(parsed.actions.len(), 1);
        assert_eq!(parsed.actions[0].id_str(), "custom.sha256");
    }

    #[test]
    fn atomic_write_detects_external_conflicts_and_creates_backup() {
        let dir = std::env::temp_dir().join(format!(
            "radivine-control-persist-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let snap1 = atomic_write_with_backup(&path, "version = 1\n", false).unwrap();
        assert!(!snap1.has_external_conflict());

        // Simulate external edit
        fs::write(&path, "version = 1\n# external edit\n").unwrap();
        assert!(snap1.has_external_conflict());

        // Save with backup
        let snap2 = atomic_write_with_backup(&path, "version = 1\n# resolved\n", true).unwrap();
        assert!(!snap2.has_external_conflict());
        assert!(dir.join("config.toml.bak").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wheel_layout_roundtrips_and_rejects_invalid_strings() {
        let original = r#"# User header comment
[wheel]
# Layout preference comment
root_inner_radius = 40.0
root_outer_radius = 128.0
"#;
        let mut cfg = AppConfig::from_toml_str(original);
        assert_eq!(cfg.wheel.layout, "branching");

        cfg.wheel.layout = "concentric".to_string();
        let updated = apply_app_config_to_toml_doc(original, &cfg).expect("save concentric");
        assert!(updated.contains("# User header comment"));
        assert!(updated.contains("# Layout preference comment"));
        assert!(updated.contains("layout = \"concentric\""));

        let reloaded = AppConfig::from_toml_str(&updated);
        assert_eq!(reloaded.wheel.layout, "concentric");

        // Strict validation rejects uppercase or unknown layout strings
        cfg.wheel.layout = "Concentric".to_string();
        assert!(apply_app_config_to_toml_doc(&updated, &cfg).is_err());

        cfg.wheel.layout = "spiral".to_string();
        assert!(apply_app_config_to_toml_doc(&updated, &cfg).is_err());
    }
}
