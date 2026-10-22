//! Scene description contract between `radivine-wheel` and `radivine-render`
//! (`WHEEL_ENGINE_DESIGN.md` §12).

use std::sync::Arc;

use smallvec::SmallVec;

use crate::geometry::{Point, Rect};
use crate::menu::IconKey;

/// Role of a visible wheel in the open hierarchy chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelRole {
    /// Deepest active wheel in the chain.
    Active,
    /// Parent/ancestor wheel kept visible (dimmed to 0.72 opacity).
    Ancestor,
}

/// Side of the active wheel on which the caption pill sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionAlign {
    /// Below the active wheel.
    Below,
    /// Above the active wheel.
    Above,
    /// To the right of the active wheel.
    Right,
    /// To the left of the active wheel.
    Left,
}

/// Logical-pixel shape primitive for platform input pass-through (`WHEEL_ENGINE_DESIGN.md` §6.3).
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// Circular hit region centered at `c` with radius `r`.
    Circle {
        /// Center in overlay-local logical pixels.
        c: Point,
        /// Radius in logical pixels.
        r: f32,
    },
    /// Convex polygon hit region (e.g. parent->child or anchor->root corridor).
    Polygon(SmallVec<[Point; 6]>),
}

/// Complete visual frame output produced by [`crate::WheelSession::scene`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    /// Monotonically increasing generation counter incremented whenever the scene changes.
    pub generation: u64,
    /// Connecting vine stems drawn underneath all wheels.
    pub stems: SmallVec<[StemVisual; 6]>,
    /// Visible wheels ordered from root (first) to deepest (last).
    pub wheels: SmallVec<[WheelVisual; 6]>,
    /// Optional caption pill attached to the active wheel.
    pub caption: Option<CaptionVisual>,
}

/// Visual state of a single radial wheel in a [`Scene`].
#[derive(Clone, Debug, PartialEq)]
pub struct WheelVisual {
    /// Wheel center in overlay-local logical pixels.
    pub center: Point,
    /// Inner hub radius in logical pixels.
    pub r_in: f32,
    /// Outer ring radius in logical pixels.
    pub r_out: f32,
    /// Animated scale factor (`0.6..=1.0`).
    pub scale: f32,
    /// Transform origin for scaling (`anchor` for root, bridge point for child wheels).
    pub origin: Point,
    /// Animated opacity (`0.0..=1.0`, `0.72` when dimmed ancestor).
    pub opacity: f32,
    /// Whether this wheel is `Active` or an `Ancestor`.
    pub role: WheelRole,
    /// Optional unwrapped `(a0, a1)` angle span in radians of the bridge sector (for child wheels).
    pub bridge: Option<(f32, f32)>,
    /// Center hub visual state.
    pub hub: HubVisual,
    /// Radial segments in clockwise order.
    pub segments: SmallVec<[SegmentVisual; 12]>,
}

/// Visual state of a single radial segment inside a [`WheelVisual`].
#[derive(Clone, Debug, PartialEq)]
pub struct SegmentVisual {
    /// Start angle in radians (before linear gap inset).
    pub a0: f32,
    /// End angle in radians (before linear gap inset, `a1 > a0`).
    pub a1: f32,
    /// Animated highlight intensity (`0.0..=1.25`; `> 1.0` during commit flash).
    pub highlight: f32,
    /// Animated extra outer radius pop in logical pixels (`0.0..=4.0`).
    pub pop: f32,
    /// `true` if this segment opened a currently visible child wheel.
    pub on_path: bool,
    /// `true` if this item is enabled.
    pub enabled: bool,
    /// `true` if this item is a submenu with children (draws rim node indicator).
    pub has_children: bool,
    /// Display label.
    pub label: Arc<str>,
    /// Optional icon key.
    pub icon: Option<IconKey>,
}

/// Visual state of a wheel's center hub.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HubVisual {
    /// Optional icon drawn inside the hub (context glyph for root, parent item icon for child).
    pub icon: Option<IconKey>,
    /// Optional item count badge (`> 1`) on the root hub.
    pub badge: Option<u32>,
}

/// Visual state of a quadratic Bézier vine stem connecting a parent segment to its child wheel.
#[derive(Clone, Debug, PartialEq)]
pub struct StemVisual {
    /// Start point on the outer midpoint of the parent segment.
    pub p0: Point,
    /// Quadratic Bézier control point.
    pub ctrl: Point,
    /// End point at the center of the child wheel's bridge arc.
    pub p1: Point,
    /// Animated trim progress (`0.0..=1.0`).
    pub progress: f32,
    /// Animated opacity (`0.0..=1.0`).
    pub opacity: f32,
    /// `true` if this stem is on the active open chain.
    pub on_path: bool,
}

/// Visual state of the caption pill attached to the active wheel.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptionVisual {
    /// Reserved bounding box (`240 × 44 lp`) in overlay-local logical pixels.
    pub box_: Rect,
    /// Which side of the active wheel the caption sits on.
    pub align: CaptionAlign,
    /// Hovered item label (`13 lp` semibold).
    pub title: Arc<str>,
    /// Optional description (when enabled) or `disabled_reason` (when disabled).
    pub detail: Option<Arc<str>>,
    /// `true` if the hovered item is disabled.
    pub disabled: bool,
    /// Animated opacity (`0.0..=1.0`).
    pub opacity: f32,
}
