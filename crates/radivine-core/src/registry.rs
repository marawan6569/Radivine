//! `MenuSpec`, `ActionRegistry`, `ActionIndex`, and `build_menu` (`ACTION_SYSTEM.md` §6).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use radivine_wheel::{IconKey, ItemPath, MenuNode, MenuTree, NodeKind};

use crate::action::{Action, ActionId, Enablement};
use crate::context::Context;

/// Declarative menu specification node (`ACTION_SYSTEM.md` §6.1).
#[derive(Clone, Debug)]
pub enum MenuSpec {
    /// Leaf action reference.
    Action(ActionId),
    /// Named submenu containing child [`MenuSpec`] entries.
    Submenu {
        /// Stable submenu identifier (e.g. `"radivine.preview"`).
        id: Arc<str>,
        /// Display label shown on the segment.
        label: String,
        /// Optional icon key.
        icon: Option<IconKey>,
        /// Optional caption description.
        description: Option<String>,
        /// Child menu specifications.
        children: Vec<MenuSpec>,
        /// Optional default child [`ActionId`] executed if a drop occurs directly on the submenu segment.
        default: Option<ActionId>,
    },
    /// Placeholder expanded with matching user custom actions (`custom.*`).
    CustomActions,
}

impl MenuSpec {
    fn top_level_id(&self) -> Option<&str> {
        match self {
            Self::Action(id) => Some(id.as_str()),
            Self::Submenu { id, .. } => Some(id.as_ref()),
            Self::CustomActions => None,
        }
    }
}

/// Limits applied when constructing a [`MenuTree`] (`ACTION_SYSTEM.md` §6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuLimits {
    /// Maximum items per wheel before overflowing into an automatic `"More…"` submenu (default 8).
    pub max_items_per_wheel: u8,
    /// Maximum wheel hierarchy depth where root = 1 (default 4).
    pub max_depth: u8,
}

impl Default for MenuLimits {
    fn default() -> Self {
        Self {
            max_items_per_wheel: 8,
            max_depth: 4,
        }
    }
}

/// User-configured menu overrides from `actions.toml` (`ACTION_SYSTEM.md` §6.2, §10).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuOverrides {
    /// Action or submenu IDs to hide from the wheel.
    pub hide: Vec<ActionId>,
    /// Preferred ordering of top-level item IDs.
    pub order: Vec<Arc<str>>,
}

/// Live enablement update for a single [`ItemPath`] (`ACTION_SYSTEM.md` §6.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnablementUpdate {
    /// Path of the leaf item in the active [`MenuTree`].
    pub path: ItemPath,
    /// Whether the item is now enabled.
    pub enabled: bool,
    /// Optional reason displayed in the caption when `enabled == false`.
    pub disabled_reason: Option<Arc<str>>,
}

/// Error returned by [`ActionRegistry::register`] (`ACTION_SYSTEM.md` §6.2).
#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum RegistryError {
    /// An action with the same [`ActionId`] is already registered.
    #[error("duplicate action id {0}")]
    DuplicateId(String),
}

/// Session-scoped map from wheel [`ItemPath`] to [`ActionId`] (`ACTION_SYSTEM.md` §2, §6.2).
#[derive(Clone, Debug, Default)]
pub struct ActionIndex {
    by_path: HashMap<ItemPath, ActionId>,
    ordered: Vec<(ItemPath, ActionId)>,
}

impl ActionIndex {
    /// Looks up the [`ActionId`] corresponding to `path`.
    #[must_use]
    pub fn get(&self, path: &ItemPath) -> Option<&ActionId> {
        self.by_path.get(path)
    }

    /// Returns all `(ItemPath, ActionId)` entries in depth-first order.
    #[must_use]
    pub fn entries(&self) -> &[(ItemPath, ActionId)] {
        &self.ordered
    }

    /// Returns the number of indexed leaf actions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ordered.len()
    }

    /// Returns `true` if no leaf actions are indexed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ordered.is_empty()
    }

    fn insert(&mut self, path: ItemPath, id: ActionId) {
        self.by_path.insert(path.clone(), id.clone());
        self.ordered.push((path, id));
    }
}

/// Registry of available [`Action`]s and menu layout specification (`ACTION_SYSTEM.md` §6.2).
pub struct ActionRegistry {
    actions: HashMap<ActionId, Arc<dyn Action>>,
    registration_order: Vec<ActionId>,
    spec: Vec<MenuSpec>,
    overrides: MenuOverrides,
}

impl Default for ActionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionRegistry {
    /// Creates an empty [`ActionRegistry`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            actions: HashMap::new(),
            registration_order: Vec::new(),
            spec: Vec::new(),
            overrides: MenuOverrides::default(),
        }
    }

    /// Registers an [`Action`]. Returns [`RegistryError::DuplicateId`] if its ID is already present.
    pub fn register(&mut self, action: Arc<dyn Action>) -> Result<(), RegistryError> {
        let id = action.meta().id.clone();
        if self.actions.contains_key(&id) {
            return Err(RegistryError::DuplicateId(id.to_string()));
        }
        self.registration_order.push(id.clone());
        self.actions.insert(id, action);
        Ok(())
    }

    /// Sets the declarative [`MenuSpec`] tree and [`MenuOverrides`].
    pub fn set_menu(&mut self, spec: Vec<MenuSpec>, overrides: MenuOverrides) {
        self.spec = spec;
        self.overrides = overrides;
    }

    /// Looks up a registered [`Action`] by [`ActionId`].
    #[must_use]
    pub fn get(&self, id: &ActionId) -> Option<Arc<dyn Action>> {
        self.actions.get(id).cloned()
    }

    /// Builds a context-filtered [`MenuTree`] and [`ActionIndex`] for `ctx` (`ACTION_SYSTEM.md` §6.3).
    ///
    /// Performs **zero I/O**. Returns `None` if no actions are visible for `ctx`.
    #[must_use]
    pub fn build_menu(&self, ctx: &Context, limits: MenuLimits) -> Option<(MenuTree, ActionIndex)> {
        let max_items = limits.max_items_per_wheel.clamp(2, 12) as usize;
        let max_depth = limits.max_depth.clamp(1, 6) as usize;

        let hidden: HashSet<&str> = self.overrides.hide.iter().map(ActionId::as_str).collect();

        // Collect all explicitly referenced ActionIds so `MenuSpec::CustomActions` only expands
        // custom.* actions that aren't already explicitly placed in the spec tree.
        let mut explicit_ids = HashSet::new();
        collect_explicit_ids(&self.spec, &mut explicit_ids);

        // 1. Reorder top-level entries according to `overrides.order`
        let ordered_top = reorder_top_level(&self.spec, &self.overrides.order);

        // 2–4. Build filtered MenuNodes recursively
        let root_children =
            self.build_children_from_specs(&ordered_top, ctx, &hidden, &explicit_ids);

        if root_children.is_empty() {
            return None;
        }

        // 5. Enforce `max_items_per_wheel` and `max_depth` exactly as `MenuTree::sanitize`
        let mut tree = MenuTree::new(root_children);
        if let Some(first) = ctx.basic.items.first() {
            let hub_icon = match first.guessed_mime {
                Some(m) if m.starts_with("image/") => "lucide:image",
                _ if ctx.basic.items.len() > 1 => "lucide:files",
                _ => "lucide:file",
            };
            tree.root.icon = Some(IconKey::from(hub_icon));
        }

        enforce_limits_on_node(&mut tree.root, 1, max_items, max_depth);
        if tree.root.children().is_none_or(<[MenuNode]>::is_empty) {
            return None;
        }

        // 6–7. Build ActionIndex from the finalized MenuTree
        let mut index = ActionIndex::default();
        if let Some(children) = tree.root.children() {
            self.populate_index(children, ItemPath::root(), &mut index);
        }

        Some((tree, index))
    }

    /// Computes live [`EnablementUpdate`]s for all leaf actions in `index` (`ACTION_SYSTEM.md` §6.2).
    #[must_use]
    pub fn enablement_updates(&self, ctx: &Context, index: &ActionIndex) -> Vec<EnablementUpdate> {
        let mut updates = Vec::with_capacity(index.len());
        for (path, id) in index.entries() {
            let Some(action) = self.actions.get(id) else {
                continue;
            };
            let (enabled, disabled_reason) = eval_enablement(action.as_ref(), ctx);
            updates.push(EnablementUpdate {
                path: path.clone(),
                enabled,
                disabled_reason,
            });
        }
        updates
    }

    fn build_children_from_specs(
        &self,
        specs: &[MenuSpec],
        ctx: &Context,
        hidden: &HashSet<&str>,
        explicit_ids: &HashSet<&str>,
    ) -> Vec<MenuNode> {
        let mut out = Vec::new();

        for spec in specs {
            match spec {
                MenuSpec::Action(id) => {
                    if hidden.contains(id.as_str()) {
                        continue;
                    }
                    if let Some(node) = self.build_action_leaf(id, ctx) {
                        out.push(node);
                    }
                }
                MenuSpec::CustomActions => {
                    for id in &self.registration_order {
                        if !id.as_str().starts_with("custom.") {
                            continue;
                        }
                        if explicit_ids.contains(id.as_str()) || hidden.contains(id.as_str()) {
                            continue;
                        }
                        if let Some(node) = self.build_action_leaf(id, ctx) {
                            out.push(node);
                        }
                    }
                }
                MenuSpec::Submenu {
                    id,
                    label,
                    icon,
                    description,
                    children,
                    default,
                } => {
                    if hidden.contains(id.as_ref()) {
                        continue;
                    }
                    let built_children =
                        self.build_children_from_specs(children, ctx, hidden, explicit_ids);
                    if built_children.is_empty() {
                        continue;
                    }
                    let default_idx = default.as_ref().and_then(|def_id| {
                        built_children
                            .iter()
                            .position(|c| c.id.as_ref() == def_id.as_str())
                    });

                    let mut sub = MenuNode::submenu(id.clone(), label.as_str(), built_children);
                    sub.icon.clone_from(icon);
                    if let Some(desc) = description {
                        sub.description = Some(Arc::from(desc.as_str()));
                    }
                    if let Some(idx) = default_idx {
                        sub = sub.with_default_child(idx);
                    }
                    out.push(sub);
                }
            }
        }

        out
    }

    fn build_action_leaf(&self, id: &ActionId, ctx: &Context) -> Option<MenuNode> {
        let action = self.actions.get(id)?;
        let meta = action.meta();
        if !meta.kinds.contains(&ctx.basic.kind) || !action.is_visible(&ctx.basic) {
            return None;
        }

        let (enabled, disabled_reason) = eval_enablement(action.as_ref(), ctx);
        let mut node = MenuNode::leaf(meta.id.0.clone(), meta.label.as_str());
        node.icon.clone_from(&meta.icon);
        if let Some(ref desc) = meta.description {
            node.description = Some(Arc::from(desc.as_str()));
        }
        node.enabled = enabled;
        node.disabled_reason = disabled_reason;
        Some(node)
    }

    fn populate_index(&self, nodes: &[MenuNode], prefix: ItemPath, index: &mut ActionIndex) {
        for (i, node) in nodes.iter().enumerate() {
            let path = prefix.child(i as u8);
            match &node.kind {
                NodeKind::Leaf => {
                    let id = ActionId(node.id.clone());
                    if self.actions.contains_key(&id) {
                        index.insert(path, id);
                    }
                }
                NodeKind::Submenu { children, .. } => {
                    self.populate_index(children, path, index);
                }
            }
        }
    }
}

fn eval_enablement(action: &dyn Action, ctx: &Context) -> (bool, Option<Arc<str>>) {
    match action.enablement(ctx) {
        Enablement::Enabled => (true, None),
        Enablement::Disabled { reason } => (false, Some(Arc::from(reason.as_ref()))),
        Enablement::Pending => (false, Some(Arc::from("Checking…"))),
    }
}

fn collect_explicit_ids<'a>(specs: &'a [MenuSpec], out: &mut HashSet<&'a str>) {
    for spec in specs {
        match spec {
            MenuSpec::Action(id) => {
                out.insert(id.as_str());
            }
            MenuSpec::Submenu { children, .. } => {
                collect_explicit_ids(children, out);
            }
            MenuSpec::CustomActions => {}
        }
    }
}

fn reorder_top_level(specs: &[MenuSpec], order: &[Arc<str>]) -> Vec<MenuSpec> {
    if order.is_empty() {
        return specs.to_vec();
    }
    let mut used = vec![false; specs.len()];
    let mut out = Vec::with_capacity(specs.len());

    for wanted in order {
        for (idx, spec) in specs.iter().enumerate() {
            if !used[idx] && spec.top_level_id() == Some(wanted.as_ref()) {
                used[idx] = true;
                out.push(spec.clone());
            }
        }
    }
    for (idx, spec) in specs.iter().enumerate() {
        if !used[idx] {
            out.push(spec.clone());
        }
    }
    out
}

fn enforce_limits_on_node(node: &mut MenuNode, depth: usize, max_items: usize, max_depth: usize) {
    let NodeKind::Submenu {
        ref mut children,
        ref mut default_child,
    } = node.kind
    else {
        return;
    };

    if depth > max_depth {
        node.enabled = false;
        node.disabled_reason = Some(Arc::from("Too deeply nested"));
        node.kind = NodeKind::Leaf;
        return;
    }

    while children.len() > max_items {
        let keep = max_items - 1;
        let overflow: Vec<MenuNode> = children.drain(keep..).collect();
        let more = MenuNode {
            id: Arc::from("radivine.more"),
            label: Arc::from("More…"),
            icon: Some(IconKey::from("lucide:more-horizontal")),
            description: Some(Arc::from("Additional actions")),
            disabled_reason: None,
            enabled: true,
            kind: NodeKind::Submenu {
                children: overflow,
                default_child: None,
            },
        };
        children.push(more);
    }

    for child in children.iter_mut() {
        if matches!(child.kind, NodeKind::Submenu { .. }) {
            if depth >= max_depth {
                child.enabled = false;
                child.disabled_reason = Some(Arc::from("Too deeply nested"));
                child.kind = NodeKind::Leaf;
            } else {
                enforce_limits_on_node(child, depth + 1, max_items, max_depth);
            }
        }
    }

    if let Some(idx) = *default_child
        && idx >= children.len()
    {
        *default_child = None;
    }
}
