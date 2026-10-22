//! Hierarchical menu model consumed by `radivine-wheel`.

use std::sync::Arc;

use smallvec::SmallVec;

use crate::config::WheelConfig;

/// Opaque icon identifier resolved by the renderer (e.g., `"lucide:eye"`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IconKey(pub Arc<str>);

impl IconKey {
    /// Creates a new `IconKey` from a string slice.
    #[must_use]
    pub fn new(s: impl Into<Arc<str>>) -> Self {
        Self(s.into())
    }

    /// Returns the string slice of this icon key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for IconKey {
    fn from(s: &str) -> Self {
        Self(Arc::from(s))
    }
}

/// Sequence of child indices from the root wheel identifying a node in a [`MenuTree`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ItemPath(pub SmallVec<[u8; 6]>);

impl ItemPath {
    /// Creates an empty path (root).
    #[must_use]
    pub fn root() -> Self {
        Self(SmallVec::new())
    }

    /// Creates a path from a slice of child indices.
    #[must_use]
    pub fn from_slice(indices: &[u8]) -> Self {
        Self(SmallVec::from_slice(indices))
    }

    /// Returns the indices as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Returns the number of levels in the path (`1` for a root item).
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` if the path is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns a new path with `idx` appended.
    #[must_use]
    pub fn child(&self, idx: u8) -> Self {
        let mut next = self.0.clone();
        next.push(idx);
        Self(next)
    }

    /// Returns `true` if `self` starts with `prefix`.
    #[must_use]
    pub fn starts_with(&self, prefix: &ItemPath) -> bool {
        self.0.starts_with(&prefix.0)
    }
}

/// Kind of a [`MenuNode`]: either an executable leaf or a submenu with children.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// Executable leaf item.
    Leaf,
    /// Submenu item that opens an adjacent child wheel.
    Submenu {
        /// Child nodes displayed in the adjacent child wheel.
        children: Vec<MenuNode>,
        /// Optional index of a default child action executed if dropped directly on the submenu parent.
        default_child: Option<usize>,
    },
}

/// A single item or submenu node in a [`MenuTree`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuNode {
    /// Stable identifier (action ID for leaves, submenu ID for submenus).
    pub id: Arc<str>,
    /// Short display label rendered inside the segment and caption title.
    pub label: Arc<str>,
    /// Optional icon key (e.g. `"lucide:eye"`).
    pub icon: Option<IconKey>,
    /// Optional description shown in the caption when enabled.
    pub description: Option<Arc<str>>,
    /// Optional explanation shown in the caption when `enabled == false`.
    pub disabled_reason: Option<Arc<str>>,
    /// Whether the item is enabled for interaction/selection.
    pub enabled: bool,
    /// Leaf or submenu payload.
    pub kind: NodeKind,
}

impl MenuNode {
    /// Creates an enabled leaf node.
    #[must_use]
    pub fn leaf(id: impl Into<Arc<str>>, label: impl Into<Arc<str>>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            description: None,
            disabled_reason: None,
            enabled: true,
            kind: NodeKind::Leaf,
        }
    }

    /// Creates an enabled submenu node with the given children.
    #[must_use]
    pub fn submenu(
        id: impl Into<Arc<str>>,
        label: impl Into<Arc<str>>,
        children: Vec<MenuNode>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            description: None,
            disabled_reason: None,
            enabled: true,
            kind: NodeKind::Submenu {
                children,
                default_child: None,
            },
        }
    }

    /// Builder helper to set the icon.
    #[must_use]
    pub fn with_icon(mut self, icon: impl Into<IconKey>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Builder helper to set the description.
    #[must_use]
    pub fn with_description(mut self, desc: impl Into<Arc<str>>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Builder helper to mark the node disabled with a reason.
    #[must_use]
    pub fn with_disabled(mut self, reason: impl Into<Arc<str>>) -> Self {
        self.enabled = false;
        self.disabled_reason = Some(reason.into());
        self
    }

    /// Builder helper to set `default_child` on a submenu node.
    #[must_use]
    pub fn with_default_child(mut self, default_idx: usize) -> Self {
        if let NodeKind::Submenu {
            ref mut default_child,
            ..
        } = self.kind
        {
            *default_child = Some(default_idx);
        }
        self
    }

    /// Returns `true` if this node is a submenu with at least one child.
    #[must_use]
    pub fn has_children(&self) -> bool {
        matches!(&self.kind, NodeKind::Submenu { children, .. } if !children.is_empty())
    }

    /// Returns the children slice if this is a submenu.
    #[must_use]
    pub fn children(&self) -> Option<&[MenuNode]> {
        match &self.kind {
            NodeKind::Leaf => None,
            NodeKind::Submenu { children, .. } => Some(children),
        }
    }
}

/// Complete hierarchical menu tree for a wheel session (`root.kind` must be `NodeKind::Submenu`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuTree {
    /// Root node whose children form the root wheel's segments.
    pub root: MenuNode,
}

impl MenuTree {
    /// Creates a `MenuTree` from a list of root wheel items.
    #[must_use]
    pub fn new(items: Vec<MenuNode>) -> Self {
        Self {
            root: MenuNode {
                id: Arc::from("root"),
                label: Arc::from("Root"),
                icon: None,
                description: None,
                disabled_reason: None,
                enabled: true,
                kind: NodeKind::Submenu {
                    children: items,
                    default_child: None,
                },
            },
        }
    }

    /// Looks up a node by [`ItemPath`]. Returns `Some(&self.root)` for an empty path.
    #[must_use]
    pub fn node(&self, path: &ItemPath) -> Option<&MenuNode> {
        let mut cur = &self.root;
        for &idx in path.as_slice() {
            let children = cur.children()?;
            cur = children.get(idx as usize)?;
        }
        Some(cur)
    }

    /// Mutably looks up a node by [`ItemPath`].
    pub fn node_mut(&mut self, path: &ItemPath) -> Option<&mut MenuNode> {
        let mut cur = &mut self.root;
        for &idx in path.as_slice() {
            match &mut cur.kind {
                NodeKind::Leaf => return None,
                NodeKind::Submenu { children, .. } => {
                    cur = children.get_mut(idx as usize)?;
                }
            }
        }
        Some(cur)
    }

    /// Validates and clamps the menu tree according to `WHEEL_ENGINE_DESIGN.md` §3:
    /// - Prunes empty submenus.
    /// - Wraps overflow items (`> max_items_per_wheel`, hard cap 12) into an automatic `"More…"` submenu.
    /// - Marks submenus at the depth limit (`max_depth`, hard cap 6, where root = 1) as disabled
    ///   with reason `"Too deeply nested"`.
    /// - Returns `false` if the root has 0 items after pruning.
    pub fn sanitize(&mut self, cfg: &WheelConfig) -> bool {
        let max_items = cfg.max_items_per_wheel.clamp(2, 12) as usize;
        let max_depth = cfg.max_depth.clamp(1, 6) as usize;
        sanitize_node(&mut self.root, 1, max_items, max_depth)
    }
}

fn sanitize_node(node: &mut MenuNode, depth: usize, max_items: usize, max_depth: usize) -> bool {
    let NodeKind::Submenu {
        ref mut children,
        ref mut default_child,
    } = node.kind
    else {
        return true;
    };

    // First prune empty submenus among direct children (before overflow wrapping).
    children.retain_mut(|child| {
        if matches!(child.kind, NodeKind::Submenu { .. }) {
            has_any_leaf(child)
        } else {
            true
        }
    });

    if children.is_empty() {
        return false;
    }

    // Wrap overflow items into an automatic "More…" submenu if children.len() > max_items.
    if children.len() > max_items {
        let keep = max_items - 1;
        let overflow: Vec<MenuNode> = children.drain(keep..).collect();
        let more_node = MenuNode {
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
        children.push(more_node);
        if let Some(dc) = *default_child
            && dc >= keep
        {
            *default_child = None;
        }
    }

    // Now handle depth limits and recurse into child submenus.
    for child in children.iter_mut() {
        if let NodeKind::Submenu {
            children: ref mut sub_children,
            default_child: ref mut sub_default,
        } = child.kind
        {
            if depth >= max_depth {
                // Submenu at the depth limit cannot open a wheel at depth + 1:
                // disable it with reason "Too deeply nested".
                child.enabled = false;
                child.disabled_reason = Some(Arc::from("Too deeply nested"));
                *sub_default = None;
                let _ = sub_children;
            } else {
                sanitize_node(child, depth + 1, max_items, max_depth);
            }
        }
    }

    if let Some(dc) = *default_child
        && dc >= children.len()
    {
        *default_child = None;
    }

    !children.is_empty()
}

fn has_any_leaf(node: &MenuNode) -> bool {
    match &node.kind {
        NodeKind::Leaf => true,
        NodeKind::Submenu { children, .. } => children.iter().any(has_any_leaf),
    }
}
