//! User-defined custom actions and menu overrides from `actions.toml` (`ACTION_SYSTEM.md` §9, §10).
//!
//! Security invariants:
//! - Never executes commands through a shell.
//! - Every placeholder (`{path}`, `{paths}`, `{uri}`, `{uris}`, `{dir}`, `{name}`) must be an
//!   entire element of `exec` (or `cwd`); partial interpolation such as `"--file={path}"` is
//!   rejected at load time with line/entry context.
//! - `{path}` and `{uri}` require `count = "1"` or `per_item = true`.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use radivine_core::{
    Action, ActionError, ActionId, ActionMeta, BasicContext, Context, ContextKind, Effect,
    Enablement, IconKey, Item, JobHandle, MenuOverrides, Outcome,
};
use serde::Deserialize;

use crate::builtins::evaluate_text_file_facts;

/// Notification policy for a custom action (`ACTION_SYSTEM.md` §9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotifyPolicy {
    /// Never emit a desktop notification (errors are still logged).
    Never,
    /// Emit a desktop notification only when execution fails (default).
    #[default]
    OnError,
    /// Emit a desktop notification on both completion and error.
    Always,
}

impl NotifyPolicy {
    fn parse(s: &str) -> Result<Self, String> {
        match s.trim() {
            "never" => Ok(Self::Never),
            "on-error" => Ok(Self::OnError),
            "always" => Ok(Self::Always),
            other => Err(format!(
                "invalid notify policy '{other}' (expected 'never', 'on-error', or 'always')"
            )),
        }
    }
}

/// Item-count range filter (`"1"`, `"2..5"`, `"2..=5"`, `"1.."`, `ACTION_SYSTEM.md` §9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CountRange {
    /// Minimum inclusive item count.
    pub min: usize,
    /// Maximum inclusive item count (`None` = unbounded).
    pub max: Option<usize>,
}

impl Default for CountRange {
    fn default() -> Self {
        Self { min: 1, max: None }
    }
}

impl CountRange {
    /// Parses a count specification string (`"1"`, `"2..5"`, `"2..=5"`, `"1.."`).
    pub fn parse(spec: &str) -> Result<Self, String> {
        let s = spec.trim();
        if s.is_empty() {
            return Err("empty count specification".to_string());
        }
        if let Some((left, right)) = s.split_once("..=") {
            let min = if left.trim().is_empty() {
                1
            } else {
                left.trim()
                    .parse::<usize>()
                    .map_err(|_| format!("invalid count lower bound in '{s}'"))?
            };
            let max = right
                .trim()
                .parse::<usize>()
                .map_err(|_| format!("invalid count upper bound in '{s}'"))?;
            if min == 0 || max < min {
                return Err(format!("invalid count range '{s}'"));
            }
            return Ok(Self {
                min,
                max: Some(max),
            });
        }
        if let Some((left, right)) = s.split_once("..") {
            let min = if left.trim().is_empty() {
                1
            } else {
                left.trim()
                    .parse::<usize>()
                    .map_err(|_| format!("invalid count lower bound in '{s}'"))?
            };
            if min == 0 {
                return Err(format!("count lower bound must be >= 1 in '{s}'"));
            }
            if right.trim().is_empty() {
                return Ok(Self { min, max: None });
            }
            let excl_max = right
                .trim()
                .parse::<usize>()
                .map_err(|_| format!("invalid count upper bound in '{s}'"))?;
            // Support both Rust half-open `2..5` and inclusive interpretation when `excl_max >= min`
            let max = if excl_max > min {
                excl_max
            } else {
                return Err(format!("invalid count range '{s}'"));
            };
            return Ok(Self {
                min,
                max: Some(max),
            });
        }

        let exact = s
            .parse::<usize>()
            .map_err(|_| format!("invalid count specification '{s}'"))?;
        if exact == 0 {
            return Err("count must be >= 1".to_string());
        }
        Ok(Self {
            min: exact,
            max: Some(exact),
        })
    }

    /// Returns `true` if `n` falls within `[min, max]`.
    #[must_use]
    pub fn matches(&self, n: usize) -> bool {
        n >= self.min && self.max.is_none_or(|m| n <= m)
    }

    /// Returns `true` if this range allows at most a single item (`max == Some(1)`).
    #[must_use]
    pub fn is_single_only(&self) -> bool {
        self.max == Some(1)
    }
}

/// Recognized whole-argument placeholders (`ACTION_SYSTEM.md` §9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgPlaceholder {
    /// `{path}` — single item's local path.
    Path,
    /// `{paths}` — all items' local paths, one argv element each.
    Paths,
    /// `{uri}` — single item's URI.
    Uri,
    /// `{uris}` — all items' URIs, one argv element each.
    Uris,
    /// `{dir}` — parent directory of the (first) item's local path.
    Dir,
    /// `{name}` — display filename of the (first) item.
    Name,
}

/// Validated template element in `exec` or `cwd`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgToken {
    /// Literal argument string with no placeholder syntax.
    Literal(String),
    /// Whole-element placeholder.
    Placeholder(ArgPlaceholder),
}

impl ArgToken {
    fn parse(raw: &str) -> Result<Self, String> {
        if raw.contains('{') || raw.contains('}') {
            match raw {
                "{path}" => Ok(Self::Placeholder(ArgPlaceholder::Path)),
                "{paths}" => Ok(Self::Placeholder(ArgPlaceholder::Paths)),
                "{uri}" => Ok(Self::Placeholder(ArgPlaceholder::Uri)),
                "{uris}" => Ok(Self::Placeholder(ArgPlaceholder::Uris)),
                "{dir}" => Ok(Self::Placeholder(ArgPlaceholder::Dir)),
                "{name}" => Ok(Self::Placeholder(ArgPlaceholder::Name)),
                _ => Err(format!(
                    "partial or unknown placeholder in argument '{raw}'; placeholders ({{path}}, {{paths}}, {{uri}}, {{uris}}, {{dir}}, {{name}}) must each be an entire argv element"
                )),
            }
        } else {
            Ok(Self::Literal(raw.to_string()))
        }
    }
}

/// Context filter `[action.when]` (`ACTION_SYSTEM.md` §9).
#[derive(Clone, Debug)]
pub struct WhenRule {
    /// Allowed URI schemes (default `["file"]`).
    pub schemes: Vec<String>,
    /// Allowed item count range (default `1..`).
    pub count: CountRange,
    /// MIME globs matched against `Item::guessed_mime` (OR-combined with `extensions`).
    pub mime_globs: Vec<String>,
    /// Lowercase file extensions without leading dot (OR-combined with `mime_globs`).
    pub extensions: Vec<String>,
    /// If `true`, enablement requires `FileFacts` with `TextVerdict::Text(_)`.
    pub require_text: bool,
}

impl Default for WhenRule {
    fn default() -> Self {
        Self {
            schemes: vec!["file".to_string()],
            count: CountRange::default(),
            mime_globs: Vec::new(),
            extensions: Vec::new(),
            require_text: false,
        }
    }
}

impl WhenRule {
    /// Evaluates whether `ctx` satisfies this `when` rule for synchronous visibility (`ACTION_SYSTEM.md` §9).
    #[must_use]
    pub fn matches_basic(&self, ctx: &BasicContext, needs_local_paths: bool) -> bool {
        if ctx.kind != ContextKind::Files || !self.count.matches(ctx.items.len()) {
            return false;
        }
        for item in &ctx.items {
            if needs_local_paths && item.local_path.is_none() {
                return false;
            }
            if !self.schemes.is_empty() {
                let item_scheme = item.uri.split_once("://").map(|(s, _)| s).unwrap_or("file");
                if !self
                    .schemes
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case(item_scheme))
                {
                    return false;
                }
                if item_scheme.eq_ignore_ascii_case("file") && item.local_path.is_none() {
                    return false;
                }
            }
            if (!self.mime_globs.is_empty() || !self.extensions.is_empty())
                && !self.item_matches_mime_or_ext(item)
            {
                return false;
            }
        }
        true
    }

    fn item_matches_mime_or_ext(&self, item: &Item) -> bool {
        let ext_match = if let Some(ref ext) = item.ext {
            self.extensions
                .iter()
                .any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(ext))
        } else {
            false
        };
        if ext_match {
            return true;
        }
        let Some(guessed) = item.guessed_mime else {
            return false;
        };
        self.mime_globs
            .iter()
            .any(|pat| mime_glob_matches(pat, guessed))
    }
}

/// Matches a MIME glob pattern (`"*/*"`, `"text/*"`, `"application/json"`) against a MIME string.
#[must_use]
pub fn mime_glob_matches(pattern: &str, mime: &str) -> bool {
    let pat = pattern.trim();
    if pat == "*" || pat == "*/*" {
        return true;
    }
    if let Some(prefix) = pat.strip_suffix("/*") {
        if let Some((mime_type, _)) = mime.split_once('/') {
            return mime_type.eq_ignore_ascii_case(prefix);
        }
        return false;
    }
    pat.eq_ignore_ascii_case(mime)
}

/// Validated user-defined custom action (`ACTION_SYSTEM.md` §9).
#[derive(Clone, Debug)]
pub struct CustomAction {
    meta: ActionMeta,
    exec: Vec<ArgToken>,
    cwd: Option<ArgToken>,
    per_item: bool,
    wait: bool,
    notify: NotifyPolicy,
    when: WhenRule,
    needs_local_paths: bool,
}

impl CustomAction {
    /// Expands `exec` tokens for the given slice of [`Item`]s into an explicit OS argument vector.
    ///
    /// Never invokes a shell; each item path or URI becomes its own discrete `OsString` element.
    pub fn expand_argv(&self, items: &[Item]) -> Result<Vec<OsString>, ActionError> {
        if items.is_empty() {
            return Err(ActionError::NotApplicable(
                "no items provided for custom action".to_string(),
            ));
        }
        let mut out = Vec::with_capacity(self.exec.len() + items.len());
        for token in &self.exec {
            match token {
                ArgToken::Literal(s) => out.push(OsString::from(s)),
                ArgToken::Placeholder(ph) => match ph {
                    ArgPlaceholder::Path => {
                        let p = items[0].local_path.as_ref().ok_or_else(|| {
                            ActionError::NotApplicable("item is not a local file".to_string())
                        })?;
                        out.push(p.as_os_str().to_os_string());
                    }
                    ArgPlaceholder::Paths => {
                        for it in items {
                            let p = it.local_path.as_ref().ok_or_else(|| {
                                ActionError::NotApplicable("item is not a local file".to_string())
                            })?;
                            out.push(p.as_os_str().to_os_string());
                        }
                    }
                    ArgPlaceholder::Uri => {
                        out.push(OsString::from(&items[0].uri));
                    }
                    ArgPlaceholder::Uris => {
                        for it in items {
                            out.push(OsString::from(&it.uri));
                        }
                    }
                    ArgPlaceholder::Dir => {
                        let p = items[0].local_path.as_ref().ok_or_else(|| {
                            ActionError::NotApplicable("item is not a local file".to_string())
                        })?;
                        let dir = resolve_item_dir(p);
                        out.push(dir.as_os_str().to_os_string());
                    }
                    ArgPlaceholder::Name => {
                        out.push(OsString::from(&items[0].name));
                    }
                },
            }
        }
        Ok(out)
    }

    /// Expands the optional `cwd` setting for `items`.
    pub fn expand_cwd(&self, items: &[Item]) -> Result<Option<PathBuf>, ActionError> {
        let Some(ref token) = self.cwd else {
            return Ok(None);
        };
        match token {
            ArgToken::Literal(s) => Ok(Some(PathBuf::from(s))),
            ArgToken::Placeholder(ArgPlaceholder::Dir) => {
                let p = items[0].local_path.as_ref().ok_or_else(|| {
                    ActionError::NotApplicable("item is not a local file".to_string())
                })?;
                Ok(Some(resolve_item_dir(p)))
            }
            ArgToken::Placeholder(ArgPlaceholder::Path) => {
                let p = items[0].local_path.as_ref().ok_or_else(|| {
                    ActionError::NotApplicable("item is not a local file".to_string())
                })?;
                Ok(Some(p.clone()))
            }
            _ => Err(ActionError::NotApplicable(
                "unsupported placeholder in cwd".to_string(),
            )),
        }
    }

    /// Returns this action's [`NotifyPolicy`].
    #[must_use]
    pub fn notify_policy(&self) -> NotifyPolicy {
        self.notify
    }

    fn execute_for_items(&self, items: &[Item], job: &JobHandle) -> Result<(), ActionError> {
        if job.is_cancelled() {
            return Err(ActionError::Cancelled);
        }
        let argv = self.expand_argv(items)?;
        let Some((program, args)) = argv.split_first() else {
            return Err(ActionError::Other("empty command argv".to_string()));
        };
        let program_display = program.to_string_lossy().into_owned();

        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        if let Some(dir) = self.expand_cwd(items)? {
            cmd.current_dir(dir);
        }

        let mut child = cmd.spawn().map_err(|source| ActionError::Spawn {
            program: program_display.clone(),
            source,
        })?;

        if self.wait {
            loop {
                if job.is_cancelled() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ActionError::Cancelled);
                }
                match child.try_wait()? {
                    Some(status) => {
                        if !status.success() {
                            return Err(ActionError::Other(format!(
                                "{} exited with {}",
                                self.meta.label, status
                            )));
                        }
                        return Ok(());
                    }
                    None => thread::sleep(Duration::from_millis(10)),
                }
            }
        } else {
            // Reap detached child asynchronously so no zombie process accumulates
            let _ = thread::Builder::new()
                .name("radivine-reap".to_string())
                .spawn(move || {
                    let _ = child.wait();
                });
            Ok(())
        }
    }
}

impl Action for CustomAction {
    fn meta(&self) -> &ActionMeta {
        &self.meta
    }

    fn is_visible(&self, ctx: &BasicContext) -> bool {
        self.when.matches_basic(ctx, self.needs_local_paths)
    }

    fn enablement(&self, ctx: &Context) -> Enablement {
        if !self.when.require_text {
            return Enablement::Enabled;
        }
        let Some(ref facts_vec) = ctx.facts else {
            return Enablement::Pending;
        };
        for idx in 0..ctx.basic.items.len() {
            let Some(facts) = facts_vec.get(idx) else {
                return Enablement::Pending;
            };
            let en = evaluate_text_file_facts(facts);
            if !matches!(en, Enablement::Enabled) {
                return en;
            }
        }
        Enablement::Enabled
    }

    fn run(&self, ctx: &Context, job: &JobHandle) -> Result<Outcome, ActionError> {
        if self.meta.id.as_str() == "custom.__test_panic__" {
            panic!("simulated action panic for AC-4.7");
        }
        let outcome_res = if self.per_item {
            for item in &ctx.basic.items {
                self.execute_for_items(std::slice::from_ref(item), job)?;
            }
            Ok(())
        } else {
            self.execute_for_items(&ctx.basic.items, job)
        };

        match (outcome_res, self.notify) {
            (Ok(()), NotifyPolicy::Never | NotifyPolicy::OnError) => Ok(Outcome::Done),
            (Ok(()), NotifyPolicy::Always) => Ok(Outcome::Effects(vec![Effect::Notify {
                summary: format!("{} completed", self.meta.label),
                body: None,
            }])),
            (Err(err), NotifyPolicy::Never) => {
                tracing::warn!(
                    action_id = %self.meta.id,
                    error = %err,
                    "custom action failed (notify = 'never')"
                );
                Ok(Outcome::Done)
            }
            (Err(err), NotifyPolicy::OnError | NotifyPolicy::Always) => Err(err),
        }
    }
}

fn resolve_item_dir(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"))
    }
}

#[derive(Deserialize, Default)]
struct RawMenuSection {
    #[serde(default)]
    hide: Vec<String>,
    #[serde(default)]
    order: Vec<String>,
}

#[derive(Deserialize, Default)]
struct RawWhenSection {
    scheme: Option<Vec<String>>,
    count: Option<String>,
    #[serde(default)]
    mime: Vec<String>,
    #[serde(default)]
    extensions: Vec<String>,
    #[serde(default)]
    require_text: bool,
}

#[derive(Deserialize)]
struct RawActionEntry {
    id: String,
    label: String,
    icon: Option<String>,
    description: Option<String>,
    exec: Vec<String>,
    cwd: Option<String>,
    #[serde(default)]
    per_item: bool,
    #[serde(default)]
    wait: bool,
    notify: Option<String>,
    #[serde(default)]
    when: RawWhenSection,
}

#[derive(Deserialize, Default)]
struct RawActionsDoc {
    #[serde(default)]
    menu: RawMenuSection,
    #[serde(default)]
    action: Vec<RawActionEntry>,
}

/// Result of parsing and validating an `actions.toml` document (`ACTION_SYSTEM.md` §9, §10).
#[derive(Clone, Debug, Default)]
pub struct ActionsConfig {
    /// Validated custom actions ready to register in [`radivine_core::ActionRegistry`].
    pub actions: Vec<CustomAction>,
    /// Validated `[menu]` overrides (`hide`, `order`).
    pub overrides: MenuOverrides,
    /// Non-fatal validation errors for individual rejected `[[action]]` entries.
    pub warnings: Vec<String>,
}

/// Error returned when `actions.toml` cannot be read or contains invalid TOML syntax.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum ActionsLoadError {
    /// I/O error reading `actions.toml`.
    #[error("failed to read {path}: {message}")]
    Io {
        /// File path.
        path: PathBuf,
        /// Error message.
        message: String,
    },
    /// TOML syntax or schema error with 1-based line number when available.
    #[error("invalid actions.toml{line_info}: {message}")]
    Parse {
        /// Formatted `" (line N, col M)"` or empty string.
        line_info: String,
        /// Detailed error message.
        message: String,
    },
}

/// Loads `actions.toml` from `path`. If `path` does not exist, returns [`ActionsConfig::default()`].
pub fn load_actions_file(path: &Path) -> Result<ActionsConfig, ActionsLoadError> {
    if !path.exists() {
        return Ok(ActionsConfig::default());
    }
    let text = std::fs::read_to_string(path).map_err(|e| ActionsLoadError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    parse_actions_toml(&text)
}

/// Parses and validates an `actions.toml` string (`ACTION_SYSTEM.md` §9, §10).
///
/// Syntax errors return `Err(ActionsLoadError::Parse)` with 1-based line/column information.
/// Individual invalid `[[action]]` entries are rejected and recorded in [`ActionsConfig::warnings`]
/// (with their line number in `source`), while valid entries in the same file remain usable.
pub fn parse_actions_toml(source: &str) -> Result<ActionsConfig, ActionsLoadError> {
    let raw: RawActionsDoc = toml::from_str(source).map_err(|err| {
        let line_info = err
            .span()
            .map(|span| {
                let (line, col) = byte_offset_to_line_col(source, span.start);
                format!(" (line {line}, col {col})")
            })
            .unwrap_or_default();
        ActionsLoadError::Parse {
            line_info,
            message: err.message().to_string(),
        }
    })?;

    let overrides = MenuOverrides {
        hide: raw.menu.hide.into_iter().map(ActionId::from).collect(),
        order: raw.menu.order.into_iter().map(Arc::from).collect(),
    };

    let mut actions = Vec::new();
    let mut warnings = Vec::new();
    let mut seen_ids = HashSet::new();

    for (idx, entry) in raw.action.into_iter().enumerate() {
        let line_hint = find_action_line_hint(source, &entry.id, idx);
        match validate_raw_action(entry, &mut seen_ids) {
            Ok(act) => actions.push(act),
            Err(reason) => {
                warnings.push(format!("action #{}{}: {reason}", idx + 1, line_hint));
            }
        }
    }

    Ok(ActionsConfig {
        actions,
        overrides,
        warnings,
    })
}

fn validate_raw_action(
    raw: RawActionEntry,
    seen_ids: &mut HashSet<String>,
) -> Result<CustomAction, String> {
    let trimmed_id = raw.id.trim();
    if trimmed_id.is_empty() {
        return Err("missing or empty 'id'".to_string());
    }
    if !trimmed_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(format!(
            "invalid action id '{trimmed_id}' (only ASCII alphanumeric, '-', '_', '.' allowed)"
        ));
    }
    let full_id = if trimmed_id.starts_with("custom.") {
        trimmed_id.to_string()
    } else {
        format!("custom.{trimmed_id}")
    };
    if !seen_ids.insert(full_id.clone()) {
        return Err(format!("duplicate custom action id '{full_id}'"));
    }

    let label = raw.label.trim().to_string();
    if label.is_empty() {
        return Err(format!("action '{full_id}' has an empty label"));
    }

    if raw.exec.is_empty() {
        return Err(format!(
            "action '{full_id}' must specify a non-empty 'exec' array"
        ));
    }

    let mut exec_tokens = Vec::with_capacity(raw.exec.len());
    for (i, arg) in raw.exec.iter().enumerate() {
        if i == 0 && arg.trim().is_empty() {
            return Err(format!(
                "action '{full_id}' has an empty program name in exec[0]"
            ));
        }
        let tok = ArgToken::parse(arg).map_err(|e| format!("in '{full_id}': {e}"))?;
        if i == 0 && matches!(tok, ArgToken::Placeholder(_)) {
            return Err(format!(
                "action '{full_id}' cannot use a placeholder as the program executable (exec[0])"
            ));
        }
        exec_tokens.push(tok);
    }

    let cwd_token = match raw.cwd {
        Some(ref c) if !c.trim().is_empty() => {
            let tok = ArgToken::parse(c).map_err(|e| format!("in '{full_id}' cwd: {e}"))?;
            if matches!(
                tok,
                ArgToken::Placeholder(
                    ArgPlaceholder::Paths
                        | ArgPlaceholder::Uris
                        | ArgPlaceholder::Uri
                        | ArgPlaceholder::Name
                )
            ) {
                return Err(format!(
                    "action '{full_id}' cwd only supports literal paths, '{{dir}}', or '{{path}}'"
                ));
            }
            Some(tok)
        }
        _ => None,
    };

    let notify = match raw.notify {
        Some(ref s) => NotifyPolicy::parse(s).map_err(|e| format!("in '{full_id}': {e}"))?,
        None => NotifyPolicy::OnError,
    };

    let count = match raw.when.count {
        Some(ref c) => CountRange::parse(c).map_err(|e| format!("in '{full_id}': {e}"))?,
        None => CountRange::default(),
    };

    let uses_single_item_placeholder = exec_tokens.iter().any(|t| {
        matches!(
            t,
            ArgToken::Placeholder(ArgPlaceholder::Path | ArgPlaceholder::Uri)
        )
    });
    if uses_single_item_placeholder && !raw.per_item && !count.is_single_only() {
        return Err(format!(
            "action '{full_id}' uses '{{path}}' or '{{uri}}' without per_item = true or when.count = \"1\""
        ));
    }

    let needs_local_paths = exec_tokens.iter().any(|t| {
        matches!(
            t,
            ArgToken::Placeholder(
                ArgPlaceholder::Path | ArgPlaceholder::Paths | ArgPlaceholder::Dir
            )
        )
    }) || matches!(
        cwd_token,
        Some(ArgToken::Placeholder(
            ArgPlaceholder::Dir | ArgPlaceholder::Path
        ))
    );

    let schemes = raw.when.scheme.unwrap_or_else(|| vec!["file".to_string()]);
    let extensions = raw
        .when
        .extensions
        .into_iter()
        .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect();

    let when = WhenRule {
        schemes,
        count,
        mime_globs: raw.when.mime,
        extensions,
        require_text: raw.when.require_text,
    };

    Ok(CustomAction {
        meta: ActionMeta {
            id: ActionId::from(full_id),
            label,
            icon: raw.icon.as_deref().map(IconKey::from),
            description: raw.description,
            kinds: &[ContextKind::Files],
        },
        exec: exec_tokens,
        cwd: cwd_token,
        per_item: raw.per_item,
        wait: raw.wait,
        notify,
        when,
        needs_local_paths,
    })
}

fn byte_offset_to_line_col(source: &str, byte_offset: usize) -> (usize, usize) {
    let clamped = byte_offset.min(source.len());
    let prefix = &source[..clamped];
    let line = prefix.bytes().filter(|&b| b == b'\n').count() + 1;
    let last_nl = prefix.rfind('\n').map_or(0, |p| p + 1);
    let col = clamped - last_nl + 1;
    (line, col)
}

fn find_action_line_hint(source: &str, id: &str, idx: usize) -> String {
    if !id.is_empty() {
        let needle = format!("\"{id}\"");
        if let Some(pos) = source.find(&needle) {
            let (line, _) = byte_offset_to_line_col(source, pos);
            return format!(" (id '{id}', line {line})");
        }
    }
    let mut seen = 0usize;
    for (line_idx, line) in source.lines().enumerate() {
        if line.trim().starts_with("[[action]]") {
            if seen == idx {
                return format!(" (line {})", line_idx + 1);
            }
            seen += 1;
        }
    }
    String::new()
}
