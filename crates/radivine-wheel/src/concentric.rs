//! Pure concentric (nested) wheel preflight, annular geometry, sector spans, and content boxes
//! (`INTERACTION_DESIGN.md` §I1, §I2, §I8; `ARCHITECTURE.md` §A2, §A3).

use std::f32::consts::PI;

use smallvec::SmallVec;

use crate::config::{LayoutFallback, WheelConfig};
use crate::geometry::{
    Point, Rect, SegmentSpan, Viewport, angle_diff, root_segment_spans, segment_content_radius,
};

/// Fixed radial thickness of each concentric child annulus (`60.0 lp`, `INTERACTION_DESIGN.md` §I1).
pub const CONCENTRIC_RING_THICKNESS: f32 = 60.0;

/// Fixed neutral radial gap between adjacent concentric rings (`6.0 lp`, `INTERACTION_DESIGN.md` §I1).
pub const CONCENTRIC_RING_GAP: f32 = 6.0;

/// Linear sector separator width (`2.5 lp`, `INTERACTION_DESIGN.md` §I1).
pub const CONCENTRIC_SEPARATOR_WIDTH: f32 = 2.5;

/// Minimum radial/tangential target core side length (`32.0 lp`, `INTERACTION_DESIGN.md` §I1).
pub const CONCENTRIC_MIN_TARGET_CORE: f32 = 32.0;

/// Minimum distance from the reserved cluster/caption to the viewport edge (`8.0 lp`, `INTERACTION_DESIGN.md` §I1, §I2).
pub const CONCENTRIC_EDGE_PAD: f32 = 8.0;

/// Reserved concentric caption box width (`280.0 lp`, `INTERACTION_DESIGN.md` §I2, §I8).
pub const CONCENTRIC_CAPTION_W: f32 = 280.0;

/// Reserved concentric caption box height (`64.0 lp`, `INTERACTION_DESIGN.md` §I2, §I8).
pub const CONCENTRIC_CAPTION_H: f32 = 64.0;

/// Vertical gap between the outermost reserved ring (`Rreserve`) and the caption box (`10.0 lp`, `INTERACTION_DESIGN.md` §I2).
pub const CONCENTRIC_CAPTION_GAP: f32 = 10.0;

/// Neutral epsilon band around inner and outer radial ring boundaries (`1e-4 lp`, `INTERACTION_DESIGN.md` §I1, §I4).
pub const CONCENTRIC_RADIAL_EPS: f32 = 1e-4;

/// Neutral epsilon band around sector separator angular boundaries (`1e-5 rad`, `INTERACTION_DESIGN.md` §I1, §I4).
pub const CONCENTRIC_ANGULAR_EPS: f32 = 1e-5;

/// Content box padding inside an annular sector (`4.0 lp`, `INTERACTION_DESIGN.md` §I8).
pub const CONCENTRIC_CONTENT_PAD: f32 = 4.0;

/// Maximum radial hover-retention distance into an inter-ring gap (`3.0 lp`, never across the
/// `6.0 lp` gap midpoint; `INTERACTION_DESIGN.md` §I4).
pub const CONCENTRIC_RADIAL_HYSTERESIS_MAX: f32 = 3.0;

/// Minimum fresh pointer movement (`2.0 lp`) required after new geometry appears before targets
/// or dwell/band on that level become eligible (`INTERACTION_DESIGN.md` §I4, §I5).
pub const CONCENTRIC_FRESH_MOTION_LP: f32 = 2.0;

/// Strict hit-test zone for a [`crate::WheelLayout::Concentric`] session (`INTERACTION_DESIGN.md` §I4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NestedHit {
    /// Inside the single shared center hub (`distance(p, C) < inner(0) - 1e-4`).
    Hub,
    /// Strictly inside an actionable sector of open ring `level`.
    Item {
        /// 0-based ring level (`0` = root).
        level: u8,
        /// 0-based clockwise sector index on `level`.
        index: u8,
    },
    /// Inside the `6.0 lp` radial gap between `inner_level` and `inner_level + 1`.
    RadialGap {
        /// 0-based level of the inner ring bordering this gap.
        inner_level: u8,
    },
    /// Within `1e-4 lp` of an inner or outer radial boundary of `level`.
    RadialBoundary {
        /// 0-based ring level (`0` = root).
        level: u8,
    },
    /// Inside the neutral angular separator (or `1e-5 rad` separator edge epsilon) on `level`.
    Separator {
        /// 0-based ring level (`0` = root).
        level: u8,
    },
    /// Beyond the outermost open ring's outer boundary up to `last_open_r_out + hit_margin`.
    OuterMargin,
    /// Displaced-origin approach guard active outside the hub interior.
    Approach,
    /// Outside `last_open_r_out + hit_margin` (or non-finite coordinates).
    #[default]
    Outside,
}

/// Content layout mode inside a concentric sector (`INTERACTION_DESIGN.md` §I8, `ARCHITECTURE.md` §A4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentMode {
    /// `20 lp` icon plus one-line `12 lp` label (`height = 37 lp`, `width in 48..=128 lp`).
    IconAndLabel,
    /// Centered `20 × 20 lp` icon only when no width `>= 48 lp` fits at height `37 lp`.
    IconOnly,
    /// One-line `12 lp` ellipsized label (`height = 14 lp`, `width in 24..=128 lp`) when no icon exists.
    LabelOnly,
}

/// Precomputed axis-aligned content box and presentation mode for a concentric sector (`ARCHITECTURE.md` §A4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NestedSegmentContent {
    /// Axis-aligned bounding box in overlay-local logical pixels.
    pub rect: Rect,
    /// Selected content presentation mode.
    pub mode: ContentMode,
}

/// Precomputed geometry for one concentric ring at `level` (`ARCHITECTURE.md` §A3).
#[derive(Clone, Debug, PartialEq)]
pub struct RingGeometry {
    /// 0-based ring level (`0` = root).
    pub level: u8,
    /// Shared cluster center `C` in overlay-local logical pixels.
    pub center: Point,
    /// Inner radius of this annulus in logical pixels.
    pub r_in: f32,
    /// Outer radius of this annulus in logical pixels.
    pub r_out: f32,
    /// Constant angular half-width `δ = asin(min(1, S / (2 * r_in)))` of each sector separator (`0.0` when `count <= 1`).
    pub separator_half_angle: f32,
    /// Equal-angle sector spans in clockwise order starting at `3π/2`.
    pub spans: SmallVec<[SegmentSpan; 12]>,
    /// Axis-aligned content boxes inside each sector.
    pub content_boxes: SmallVec<[Rect; 12]>,
}

/// Deterministic preflight reservation plan for a [` crate::WheelLayout::Concentric`] session (`ARCHITECTURE.md` §A3).
#[derive(Clone, Debug, PartialEq)]
pub struct ConcentricPlan {
    /// Clamped shared center `C` in overlay-local logical pixels.
    pub center: Point,
    /// Outermost reserved ring radius `Rreserve = outer(max_depth - 1)`.
    pub reserved_radius: f32,
    /// Capture/navigation outer hit margin (`cfg.hit_margin`).
    pub hit_margin: f32,
    /// Reserved `280 × 64 lp` caption box `10 lp` below `reserved_radius`.
    pub caption_box: Rect,
    /// Precomputed `(inner(d), outer(d))` radii for all configured levels `0..max_depth`.
    pub radii: SmallVec<[(f32, f32); 6]>,
    /// `true` when `anchor.distance(center) > 0.5`, requiring pointer entry into the hub before arming targets.
    pub approach_required: bool,
}

/// Computes `(inner(d), outer(d))` for 0-based ring `level` (`INTERACTION_DESIGN.md` §I1).
///
/// ```text
/// inner(0) = h                       outer(0) = r0
/// inner(d) = r0 + (d - 1)*(T+G) + G  outer(d) = r0 + d*(T+G), d >= 1
/// ```
#[inline]
#[must_use]
pub fn concentric_ring_radii(level: u8, root_r_in: f32, root_r_out: f32) -> (f32, f32) {
    if level == 0 {
        (root_r_in, root_r_out)
    } else {
        let d = f32::from(level);
        let step = CONCENTRIC_RING_THICKNESS + CONCENTRIC_RING_GAP;
        let r_in = root_r_out + (d - 1.0) * step + CONCENTRIC_RING_GAP;
        let r_out = root_r_out + d * step;
        (r_in, r_out)
    }
}

/// Computes the constant neutral angular half-width `δ = asin(min(1, S / (2 * r_in)))` for a ring
/// with `count` sectors (`INTERACTION_DESIGN.md` §I1).
///
/// For `count <= 1`, returns `0.0` so a single-item ring forms a complete seamless annulus.
#[inline]
#[must_use]
pub fn separator_half_angle_for_ring(r_in: f32, count: usize) -> f32 {
    if count <= 1 || !r_in.is_finite() || r_in <= 1e-3 {
        0.0
    } else {
        (CONCENTRIC_SEPARATOR_WIDTH / (2.0 * r_in))
            .clamp(0.0, 1.0)
            .asin()
    }
}

/// Checks whether a ring `(r_in, r_out)` is at least `60 lp` thick and can contain a `32 × 32 lp`
/// radial/tangential target core centered at `rc = r_in + 0.55 * (r_out - r_in)` at `n = max_items`
/// (`INTERACTION_DESIGN.md` §I1).
#[must_use]
pub fn target_core_fits_ring(r_in: f32, r_out: f32, max_items: usize) -> bool {
    if !r_in.is_finite() || !r_out.is_finite() || r_in < 16.0 {
        return false;
    }
    let thickness = r_out - r_in;
    if thickness < CONCENTRIC_RING_THICKNESS - 1e-5 {
        return false;
    }

    let rc = segment_content_radius(r_in, r_out);
    let half = CONCENTRIC_MIN_TARGET_CORE * 0.5;
    let u_in = rc - half;
    let u_out = rc + half;

    // Radial bounds for the 4 corners (u_in, ±half), (u_out, ±half) and inner edge (u_in, 0)
    if u_in <= r_in + CONCENTRIC_RADIAL_EPS {
        return false;
    }
    if u_in.hypot(half) <= r_in + CONCENTRIC_RADIAL_EPS {
        return false;
    }
    if u_out.hypot(half) >= r_out - CONCENTRIC_RADIAL_EPS {
        return false;
    }

    let n = max_items.clamp(1, 12);
    if n > 1 {
        let half_span = PI / (n as f32);
        let delta = separator_half_angle_for_ring(r_in, n);
        let max_allowed_angle = half_span - delta - CONCENTRIC_ANGULAR_EPS;
        if max_allowed_angle <= 0.0 {
            return false;
        }
        let inner_corner_angle = half.atan2(u_in);
        let outer_corner_angle = half.atan2(u_out);
        if inner_corner_angle >= max_allowed_angle || outer_corner_angle >= max_allowed_angle {
            return false;
        }
    }

    true
}

/// Computes the deterministic concentric session plan for `anchor`, `viewport`, and `cfg`,
/// or returns a [`LayoutFallback`] reason if target-core or viewport capacity checks fail
/// (`INTERACTION_DESIGN.md` §I1, §I2; `ARCHITECTURE.md` §A3).
pub fn plan_concentric(
    anchor: Point,
    viewport: Viewport,
    cfg: &WheelConfig,
) -> Result<ConcentricPlan, LayoutFallback> {
    if !anchor.x.is_finite()
        || !anchor.y.is_finite()
        || !viewport.bounds.x.is_finite()
        || !viewport.bounds.y.is_finite()
        || !viewport.bounds.w.is_finite()
        || !viewport.bounds.h.is_finite()
        || !viewport.scale.is_finite()
        || viewport.scale <= 0.0
        || viewport.bounds.w <= 0.0
        || viewport.bounds.h <= 0.0
    {
        return Err(LayoutFallback::InsufficientSpace);
    }

    // Reject nonfinite or out-of-bounds root radii before validation clamping can mask them.
    if !cfg.radius_root[0].is_finite()
        || !cfg.radius_root[1].is_finite()
        || !cfg.activation_slop.is_finite()
        || !cfg.hit_margin.is_finite()
        || !cfg.segment_gap.is_finite()
        || !cfg.child_gap.is_finite()
        || !cfg.bridge_angle_deg.is_finite()
        || !cfg.hysteresis_deg.is_finite()
        || !cfg.radius_min_outer.is_finite()
        || !cfg.band.is_finite()
    {
        return Err(LayoutFallback::TargetTooSmall);
    }

    if cfg.radius_root[0] < 16.0
        || cfg.radius_root[1] > 240.0
        || cfg.radius_root[1] - cfg.radius_root[0] < CONCENTRIC_RING_THICKNESS - 1e-5
    {
        return Err(LayoutFallback::TargetTooSmall);
    }

    let validated = cfg.validated();
    let h = validated.radius_root[0];
    let r0 = validated.radius_root[1];
    if r0 - h < CONCENTRIC_RING_THICKNESS - 1e-5 {
        return Err(LayoutFallback::TargetTooSmall);
    }

    let max_depth = validated.max_depth as usize;
    let max_items = validated.max_items_per_wheel as usize;

    let mut radii: SmallVec<[(f32, f32); 6]> = SmallVec::new();
    for level in 0..(max_depth as u8) {
        let (r_in, r_out) = concentric_ring_radii(level, h, r0);
        if !target_core_fits_ring(r_in, r_out, max_items) {
            return Err(LayoutFallback::TargetTooSmall);
        }
        radii.push((r_in, r_out));
    }

    let reserved_radius = radii.last().map_or(r0, |&(_, r_out)| r_out);

    let vp = viewport.bounds;
    let half_x = reserved_radius.max(CONCENTRIC_CAPTION_W * 0.5);
    let required_w = 2.0 * (CONCENTRIC_EDGE_PAD + half_x);
    let required_h = 2.0 * (CONCENTRIC_EDGE_PAD + reserved_radius)
        + CONCENTRIC_CAPTION_GAP
        + CONCENTRIC_CAPTION_H;

    if vp.w < required_w || vp.h < required_h {
        return Err(LayoutFallback::InsufficientSpace);
    }

    let min_cx = vp.x + CONCENTRIC_EDGE_PAD + half_x;
    let max_cx = (vp.right() - CONCENTRIC_EDGE_PAD - half_x).max(min_cx);
    let min_cy = vp.y + CONCENTRIC_EDGE_PAD + reserved_radius;
    let max_cy = (vp.bottom()
        - CONCENTRIC_EDGE_PAD
        - reserved_radius
        - CONCENTRIC_CAPTION_GAP
        - CONCENTRIC_CAPTION_H)
        .max(min_cy);

    let center = Point::new(
        anchor.x.clamp(min_cx, max_cx),
        anchor.y.clamp(min_cy, max_cy),
    );
    let caption_box = Rect::new(
        center.x - CONCENTRIC_CAPTION_W * 0.5,
        center.y + reserved_radius + CONCENTRIC_CAPTION_GAP,
        CONCENTRIC_CAPTION_W,
        CONCENTRIC_CAPTION_H,
    );
    let approach_required = anchor.distance(center) > 0.5;

    Ok(ConcentricPlan {
        center,
        reserved_radius,
        hit_margin: validated.hit_margin,
        caption_box,
        radii,
        approach_required,
    })
}

/// Computes the [`RingGeometry`] for 0-based `level` with `count` sectors (`0..=12`)
/// (`INTERACTION_DESIGN.md` §I1, §I8; `ARCHITECTURE.md` §A3).
#[must_use]
pub fn ring_geometry(plan: &ConcentricPlan, level: u8, count: usize) -> RingGeometry {
    let (r_in, r_out) = plan.radii.get(level as usize).copied().unwrap_or_else(|| {
        let (h, r0) = plan.radii.first().copied().unwrap_or((40.0, 128.0));
        concentric_ring_radii(level, h, r0)
    });

    let clamped_count = count.min(12);
    if clamped_count == 0 {
        return RingGeometry {
            level,
            center: plan.center,
            r_in,
            r_out,
            separator_half_angle: 0.0,
            spans: SmallVec::new(),
            content_boxes: SmallVec::new(),
        };
    }

    let spans = root_segment_spans(clamped_count);
    let separator_half_angle = separator_half_angle_for_ring(r_in, clamped_count);
    let mut content_boxes = SmallVec::new();
    for &span in &spans {
        let content = compute_sector_content(
            plan.center,
            r_in,
            r_out,
            span,
            separator_half_angle,
            clamped_count,
            true,
        );
        content_boxes.push(content.rect);
    }

    RingGeometry {
        level,
        center: plan.center,
        r_in,
        r_out,
        separator_half_angle,
        spans,
        content_boxes,
    }
}

/// Computes the largest axis-aligned content box and [`ContentMode`] centered at
/// `r_c = r_in + 0.55 * (r_out - r_in)` inside `span` with `4.0 lp` clearance from
/// the inner circle, outer circle, and separator-inset sector half-planes (`INTERACTION_DESIGN.md` §I8).
#[must_use]
pub fn compute_sector_content(
    center: Point,
    r_in: f32,
    r_out: f32,
    span: SegmentSpan,
    separator_half_angle: f32,
    count: usize,
    has_icon: bool,
) -> NestedSegmentContent {
    let r_c = segment_content_radius(r_in, r_out);
    let p_c = center.offset_polar(r_c, span.center);

    let fits = |w: f32, h: f32| -> bool {
        sector_box_fits(
            center,
            r_in,
            r_out,
            span,
            separator_half_angle,
            count,
            p_c,
            w,
            h,
        )
    };

    let binary_search_width = |height: f32| -> Option<i32> {
        let mut low = 24i32;
        let mut high = 128i32;
        let mut best = None;
        while low <= high {
            let mid = (low + high) / 2;
            if fits(mid as f32, height) {
                best = Some(mid);
                low = mid + 1;
            } else {
                high = mid - 1;
            }
        }
        best
    };

    if let Some(w_int) = binary_search_width(37.0)
        && w_int >= 48
    {
        let w = w_int as f32;
        return NestedSegmentContent {
            rect: Rect::new(p_c.x - w * 0.5, p_c.y - 37.0 * 0.5, w, 37.0),
            mode: ContentMode::IconAndLabel,
        };
    }

    if has_icon {
        NestedSegmentContent {
            rect: Rect::new(p_c.x - 10.0, p_c.y - 10.0, 20.0, 20.0),
            mode: ContentMode::IconOnly,
        }
    } else {
        let w = binary_search_width(14.0).unwrap_or(24) as f32;
        NestedSegmentContent {
            rect: Rect::new(p_c.x - w * 0.5, p_c.y - 14.0 * 0.5, w, 14.0),
            mode: ContentMode::LabelOnly,
        }
    }
}

/// Tests whether an axis-aligned box of size `(w, h)` centered at `p_c` lies inside the
/// annular sector with `CONCENTRIC_CONTENT_PAD` (`4.0 lp`) clearance from the inner circle,
/// outer circle, and separator-inset sector boundary rays (`INTERACTION_DESIGN.md` §I8).
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn sector_box_fits(
    center: Point,
    r_in: f32,
    r_out: f32,
    span: SegmentSpan,
    separator_half_angle: f32,
    count: usize,
    p_c: Point,
    w: f32,
    h: f32,
) -> bool {
    let pad = CONCENTRIC_CONTENT_PAD;
    if r_out <= r_in + 2.0 * pad {
        return false;
    }

    let rect = Rect::new(p_c.x - w * 0.5, p_c.y - h * 0.5, w, h);

    // 1. Exact distance from `center` to the axis-aligned box for inner-circle clearance.
    let closest = rect.clamp_point(center);
    if closest.distance(center) < r_in + pad - 1e-4 {
        return false;
    }

    // 2. All four corners must lie inside the padded outer circle `r_out - pad`.
    let max_r = r_out - pad + 1e-4;
    let corners = [
        Point::new(rect.x, rect.y),
        Point::new(rect.right(), rect.y),
        Point::new(rect.right(), rect.bottom()),
        Point::new(rect.x, rect.bottom()),
    ];
    for c in corners {
        if c.distance(center) > max_r {
            return false;
        }
    }

    // 3. For `count > 1`, all four corners must lie at least `pad` inside both separator-inset half-planes.
    if count > 1 {
        let s0 = span.a0 + separator_half_angle;
        let s1 = span.a1 - separator_half_angle;
        if s1 <= s0 {
            return false;
        }
        let (sin0, cos0) = s0.sin_cos();
        let (sin1, cos1) = s1.sin_cos();
        for c in corners {
            let dx = c.x - center.x;
            let dy = c.y - center.y;
            let dist0 = dx * (-sin0) + dy * cos0;
            let dist1 = dx * sin1 - dy * cos1;
            if dist0 < pad - 1e-4 || dist1 < pad - 1e-4 {
                return false;
            }
        }
    }

    true
}

/// Evaluates the strict annular hit-test zone at `p` for `plan` and `open` rings
/// (`INTERACTION_DESIGN.md` §I4; `ARCHITECTURE.md` §A3).
///
/// Strict hit testing is stateless and independent of current hover or hysteresis:
/// 1. Non-finite coordinates return [`NestedHit::Outside`].
/// 2. When `plan.approach_required` is active, points inside the hub interior (`d < h - 1e-4`)
///    return [`NestedHit::Hub`] (allowing hub entry to clear the approach guard) and all other
///    points return [`NestedHit::Approach`].
/// 3. Radial boundary epsilon bands (`|d - r_in| <= 1e-4` or `|d - r_out| <= 1e-4`) return
///    [`NestedHit::RadialBoundary`].
/// 4. Hub interior (`d < h - 1e-4`) is tested once and returns [`NestedHit::Hub`].
/// 5. Open annulus interiors (`r_in + 1e-4 < d < r_out - 1e-4`) return [`NestedHit::Item`] when
///    strictly inside `(a0 + δ + 1e-5, a1 - δ - 1e-5)` (or full `2π` when `n == 1`), and
///    [`NestedHit::Separator`] otherwise.
/// 6. Inter-ring `6.0 lp` gaps between adjacent open rings return [`NestedHit::RadialGap`].
/// 7. Capture margin beyond the last open ring (`last_r_out + 1e-4 < d <= last_r_out + hit_margin`)
///    returns [`NestedHit::OuterMargin`].
/// 8. All points beyond `last_r_out + hit_margin` return [`NestedHit::Outside`].
#[must_use]
pub fn hit_concentric(p: Point, plan: &ConcentricPlan, open: &[RingGeometry]) -> NestedHit {
    if !p.x.is_finite()
        || !p.y.is_finite()
        || !plan.center.x.is_finite()
        || !plan.center.y.is_finite()
    {
        return NestedHit::Outside;
    }

    let d = p.distance(plan.center);
    let (h, r0) = open
        .first()
        .map(|r| (r.r_in, r.r_out))
        .or_else(|| plan.radii.first().copied())
        .unwrap_or((40.0, 128.0));

    if plan.approach_required {
        if d < h - CONCENTRIC_RADIAL_EPS {
            return NestedHit::Hub;
        }
        return NestedHit::Approach;
    }

    for ring in open {
        if (d - ring.r_in).abs() <= CONCENTRIC_RADIAL_EPS
            || (d - ring.r_out).abs() <= CONCENTRIC_RADIAL_EPS
        {
            return NestedHit::RadialBoundary { level: ring.level };
        }
    }
    if open.is_empty()
        && ((d - h).abs() <= CONCENTRIC_RADIAL_EPS || (d - r0).abs() <= CONCENTRIC_RADIAL_EPS)
    {
        return NestedHit::RadialBoundary { level: 0 };
    }

    if d < h - CONCENTRIC_RADIAL_EPS {
        return NestedHit::Hub;
    }

    for ring in open {
        if d > ring.r_in + CONCENTRIC_RADIAL_EPS && d < ring.r_out - CONCENTRIC_RADIAL_EPS {
            let n = ring.spans.len();
            if n == 0 {
                return NestedHit::Separator { level: ring.level };
            }
            if n == 1 {
                return NestedHit::Item {
                    level: ring.level,
                    index: 0,
                };
            }
            let theta = plan.center.angle_to(p);
            for (idx, span) in ring.spans.iter().enumerate() {
                let half_span = (span.a1 - span.a0) * 0.5;
                let max_selectable = half_span - ring.separator_half_angle - CONCENTRIC_ANGULAR_EPS;
                if max_selectable > 0.0 && angle_diff(theta, span.center).abs() < max_selectable {
                    return NestedHit::Item {
                        level: ring.level,
                        index: idx as u8,
                    };
                }
            }
            return NestedHit::Separator { level: ring.level };
        }
    }

    for pair in open.windows(2) {
        let inner_ring = &pair[0];
        let outer_ring = &pair[1];
        if d > inner_ring.r_out + CONCENTRIC_RADIAL_EPS
            && d < outer_ring.r_in - CONCENTRIC_RADIAL_EPS
        {
            return NestedHit::RadialGap {
                inner_level: inner_ring.level,
            };
        }
    }

    let last_open_r_out = open.last().map_or(r0, |r| r.r_out);
    if d > last_open_r_out + CONCENTRIC_RADIAL_EPS
        && d <= last_open_r_out + plan.hit_margin.max(0.0)
    {
        return NestedHit::OuterMargin;
    }

    NestedHit::Outside
}

/// Evaluates whether hover explanation/highlight may be retained at `p` when `strict_hit` is in a
/// neutral separator, boundary, or `< 3.0 lp` into an adjacent inter-ring gap (`INTERACTION_DESIGN.md` §I4).
///
/// Never alters `strict_hit` or action eligibility: if the returned hover target differs from
/// `strict_hit`, commit/drop MUST reject and no armed outline may be shown.
#[must_use]
pub fn retain_hover_hysteresis(
    p: Point,
    strict_hit: NestedHit,
    prev_hover: Option<(u8, u8)>,
    plan: &ConcentricPlan,
    open: &[RingGeometry],
    hysteresis_deg: f32,
) -> Option<(u8, u8)> {
    if let NestedHit::Item { level, index } = strict_hit {
        return Some((level, index));
    }

    let (prev_level, prev_index) = prev_hover?;
    let ring = open.iter().find(|r| r.level == prev_level)?;
    let span = *ring.spans.get(prev_index as usize)?;

    let d = p.distance(plan.center);
    let theta = plan.center.angle_to(p);

    let angle_ok = if ring.spans.len() <= 1 {
        true
    } else {
        let span_width = span.a1 - span.a0;
        let hyst_rad = hysteresis_deg.max(0.0).to_radians().min(span_width * 0.15);
        let half_span = span_width * 0.5;
        angle_diff(theta, span.center).abs() <= half_span - ring.separator_half_angle + hyst_rad
    };
    if !angle_ok {
        return None;
    }

    match strict_hit {
        NestedHit::Separator { level } if level == prev_level => {
            if d > ring.r_in + CONCENTRIC_RADIAL_EPS && d < ring.r_out - CONCENTRIC_RADIAL_EPS {
                Some((prev_level, prev_index))
            } else {
                None
            }
        }
        NestedHit::RadialBoundary { level } if level == prev_level => {
            let has_outer_gap = open.iter().any(|r| r.level == prev_level + 1);
            let on_outer_into_gap =
                has_outer_gap && (d - ring.r_out).abs() <= CONCENTRIC_RADIAL_EPS;
            let on_inner_into_gap =
                prev_level >= 1 && (d - ring.r_in).abs() <= CONCENTRIC_RADIAL_EPS;
            if on_outer_into_gap || on_inner_into_gap {
                Some((prev_level, prev_index))
            } else {
                None
            }
        }
        NestedHit::RadialGap { inner_level } => {
            let max_gap_retention = CONCENTRIC_RADIAL_HYSTERESIS_MAX.min(CONCENTRIC_RING_GAP * 0.5);
            let into_outer_gap =
                inner_level == prev_level && d > ring.r_out && (d - ring.r_out) < max_gap_retention;
            let into_inner_gap = inner_level + 1 == prev_level
                && d < ring.r_in
                && (ring.r_in - d) < max_gap_retention;
            if into_outer_gap || into_inner_gap {
                Some((prev_level, prev_index))
            } else {
                None
            }
        }
        _ => None,
    }
}
