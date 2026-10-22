//! `radivine-wheel` — Pure, deterministic, platform-independent hierarchical radial wheel engine.
//!
//! Implements the menu model, geometry, layout, hit-testing, navigation/hover-intent state machine,
//! deterministic tween animations, and scene output specified in `docs/WHEEL_ENGINE_DESIGN.md`.

#![forbid(unsafe_code)]

pub mod anim;
pub mod config;
pub mod geometry;
pub mod hit;
pub mod layout;
pub mod menu;
pub mod scene;
pub mod session;

pub use anim::{Ease, Millis, Tween};
pub use config::{InvokeSelect, SubmenuOpen, WheelConfig};
pub use geometry::{Point, Rect, SegmentSpan, Viewport};
pub use hit::{WheelHitGeometry, Zone, hit_test};
pub use layout::Placement;
pub use menu::{IconKey, ItemPath, MenuNode, MenuTree, NodeKind};
pub use scene::{
    CaptionAlign, CaptionVisual, HubVisual, Scene, SegmentVisual, Shape, StemVisual, WheelRole,
    WheelVisual,
};
pub use session::{
    DebugState, DismissReason, DropTarget, InputMode, NavKey, SessionPhase, WheelEvent, WheelInput,
    WheelSession,
};
