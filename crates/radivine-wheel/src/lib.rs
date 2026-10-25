//! `radivine-wheel` — Pure, deterministic, platform-independent hierarchical radial wheel engine.
//!
//! Implements the menu model, geometry, layout, hit-testing, navigation/hover-intent state machine,
//! deterministic tween animations, and scene output specified in `docs/WHEEL_ENGINE_DESIGN.md`.

#![forbid(unsafe_code)]

pub mod anim;
pub mod concentric;
pub mod config;
pub mod geometry;
pub mod hit;
pub mod layout;
pub mod menu;
pub mod scene;
pub mod session;

pub use anim::{Ease, Millis, Tween};
pub use concentric::{
    CONCENTRIC_ANGULAR_EPS, CONCENTRIC_CAPTION_GAP, CONCENTRIC_CAPTION_H, CONCENTRIC_CAPTION_W,
    CONCENTRIC_CONTENT_PAD, CONCENTRIC_EDGE_PAD, CONCENTRIC_FRESH_MOTION_LP,
    CONCENTRIC_MIN_TARGET_CORE, CONCENTRIC_RADIAL_EPS, CONCENTRIC_RADIAL_HYSTERESIS_MAX,
    CONCENTRIC_RING_GAP, CONCENTRIC_RING_THICKNESS, CONCENTRIC_SEPARATOR_WIDTH, ConcentricPlan,
    ContentMode, NestedHit, NestedSegmentContent, RingGeometry, compute_sector_content,
    concentric_ring_radii, hit_concentric, plan_concentric, retain_hover_hysteresis, ring_geometry,
    sector_box_fits, separator_half_angle_for_ring, target_core_fits_ring,
};
pub use config::{
    InvokeSelect, LayoutFallback, LayoutSelection, SubmenuOpen, WheelConfig, WheelLayout,
};
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
