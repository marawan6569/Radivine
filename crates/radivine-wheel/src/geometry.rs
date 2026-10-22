//! Pure 2D polar & Euclidean geometry primitives for `radivine-wheel`.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use smallvec::SmallVec;

/// Monotonic time in milliseconds supplied by the caller.
pub type Millis = u64;

/// 2D point in overlay-local logical pixels (`+x` right, `+y` down).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    /// Horizontal coordinate in logical pixels.
    pub x: f32,
    /// Vertical coordinate in logical pixels.
    pub y: f32,
}

impl Point {
    /// Creates a new point `(x, y)`.
    #[inline]
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Euclidean distance between `self` and `other`.
    #[inline]
    #[must_use]
    pub fn distance(self, other: Self) -> f32 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    /// Squared Euclidean distance between `self` and `other`.
    #[inline]
    #[must_use]
    pub fn distance_sq(self, other: Self) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx * dx + dy * dy
    }

    /// Offsets a point by polar coordinates `(r, angle_rad)`.
    #[inline]
    #[must_use]
    pub fn offset_polar(self, r: f32, angle_rad: f32) -> Self {
        let (sin_a, cos_a) = angle_rad.sin_cos();
        Self {
            x: self.x + r * cos_a,
            y: self.y + r * sin_a,
        }
    }

    /// Angle from `self` toward `target` normalized to `[0, 2π)`.
    #[inline]
    #[must_use]
    pub fn angle_to(self, target: Self) -> f32 {
        normalize_angle((target.y - self.y).atan2(target.x - self.x))
    }

    /// Linearly interpolates between `self` and `other`.
    #[inline]
    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            x: self.x + (other.x - self.x) * t,
            y: self.y + (other.y - self.y) * t,
        }
    }
}

/// Axis-aligned rectangle in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Left coordinate in logical pixels.
    pub x: f32,
    /// Top coordinate in logical pixels.
    pub y: f32,
    /// Width in logical pixels.
    pub w: f32,
    /// Height in logical pixels.
    pub h: f32,
}

impl Rect {
    /// Creates a new rectangle `(x, y, w, h)`.
    #[inline]
    #[must_use]
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Right edge `x + w`.
    #[inline]
    #[must_use]
    pub fn right(self) -> f32 {
        self.x + self.w
    }

    /// Bottom edge `y + h`.
    #[inline]
    #[must_use]
    pub fn bottom(self) -> f32 {
        self.y + self.h
    }

    /// Center point of the rectangle.
    #[inline]
    #[must_use]
    pub fn center(self) -> Point {
        Point::new(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    /// Returns a rectangle inset by `margin` on all four sides.
    #[inline]
    #[must_use]
    pub fn inset(self, margin: f32) -> Self {
        let max_mx = (self.w * 0.5).max(0.0);
        let max_my = (self.h * 0.5).max(0.0);
        let mx = margin.min(max_mx);
        let my = margin.min(max_my);
        Self {
            x: self.x + mx,
            y: self.y + my,
            w: (self.w - 2.0 * mx).max(0.0),
            h: (self.h - 2.0 * my).max(0.0),
        }
    }

    /// Clamps a point to lie within this rectangle.
    #[inline]
    #[must_use]
    pub fn clamp_point(self, p: Point) -> Point {
        Point::new(
            p.x.clamp(self.x, (self.x + self.w).max(self.x)),
            p.y.clamp(self.y, (self.y + self.h).max(self.y)),
        )
    }

    /// Returns `true` if `p` is inside or on the boundary of `self`.
    #[inline]
    #[must_use]
    pub fn contains_point(self, p: Point) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

    /// Returns `true` if the closed disc `(c, r)` lies completely inside `self`.
    #[inline]
    #[must_use]
    pub fn contains_disc(self, c: Point, r: f32) -> bool {
        c.x - r >= self.x
            && c.x + r <= self.right()
            && c.y - r >= self.y
            && c.y + r <= self.bottom()
    }

    /// Returns `true` if `self` lies completely inside `outer`.
    #[inline]
    #[must_use]
    pub fn is_inside(self, outer: Self) -> bool {
        self.x >= outer.x
            && self.y >= outer.y
            && self.right() <= outer.right()
            && self.bottom() <= outer.bottom()
    }

    /// Clamps `self` so that it fits inside `outer` (if smaller than `outer`).
    #[inline]
    #[must_use]
    pub fn clamp_within(self, outer: Self) -> Self {
        let max_x = (outer.right() - self.w).max(outer.x);
        let max_y = (outer.bottom() - self.h).max(outer.y);
        Self {
            x: self.x.clamp(outer.x, max_x),
            y: self.y.clamp(outer.y, max_y),
            w: self.w,
            h: self.h,
        }
    }

    /// Returns `true` if this rectangle intersects the disc `(c, r)`.
    #[inline]
    #[must_use]
    pub fn intersects_disc(self, c: Point, r: f32) -> bool {
        let closest = self.clamp_point(c);
        closest.distance_sq(c) <= r * r
    }
}

/// Overlay viewport dimensions and DPI scale factor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Logical bounds of the monitor covered by the overlay.
    pub bounds: Rect,
    /// Physical-to-logical scale factor (`physical = logical * scale`).
    pub scale: f32,
}

impl Viewport {
    /// Creates a new `Viewport` with logical bounds `(0, 0, w, h)` and `scale`.
    #[inline]
    #[must_use]
    pub const fn new(w: f32, h: f32, scale: f32) -> Self {
        Self {
            bounds: Rect::new(0.0, 0.0, w, h),
            scale,
        }
    }
}

/// Normalizes an angle in radians into `[0, 2π)`.
#[inline]
#[must_use]
pub fn normalize_angle(theta: f32) -> f32 {
    if !theta.is_finite() {
        return 0.0;
    }
    let rem = theta.rem_euclid(TAU);
    if rem >= TAU { 0.0 } else { rem }
}

/// Signed shortest angular difference `a - b` in `(-π, π]`.
#[inline]
#[must_use]
pub fn angle_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(TAU);
    if d > PI { d - TAU } else { d }
}

/// Checks whether angle `theta` lies inside the directed clockwise arc from `start` to `end`
/// (`[start, end)` modulo `2π`).
///
/// If the arc covers a full circle (`|end - start| >= 2π - 1e-5`), returns `true`.
#[inline]
#[must_use]
pub fn angle_in_span(theta: f32, start: f32, end: f32) -> bool {
    let raw_span = end - start;
    if raw_span >= TAU - 1e-5 {
        return true;
    }
    if raw_span <= 0.0 {
        return false;
    }
    let t = normalize_angle(theta);
    let s0 = normalize_angle(start);
    let s1 = normalize_angle(end);
    if (s0 - s1).abs() <= 1e-6 {
        return raw_span > PI;
    }
    if s0 < s1 {
        t >= s0 && t < s1
    } else {
        t >= s0 || t < s1
    }
}

/// Widens an angular span `[start, end)` by `eps` radians on both sides, capped at `2π`.
#[inline]
#[must_use]
pub fn widen_span(start: f32, end: f32, eps: f32) -> (f32, f32) {
    let span = end - start;
    if span + 2.0 * eps >= TAU {
        (start, start + TAU)
    } else {
        (start - eps, end + eps)
    }
}

/// Angular span `(a0, a1)` and center angle `center` for a segment (`a1 > a0`, `a1 - a0 = delta`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentSpan {
    /// Unwrapped start angle in radians (`0 <= a0 < 2π`).
    pub a0: f32,
    /// Unwrapped end angle in radians (`a0 < a1 <= a0 + 2π`).
    pub a1: f32,
    /// Center angle in `[0, 2π)`.
    pub center: f32,
}

impl SegmentSpan {
    /// Angular width `a1 - a0` in radians.
    #[inline]
    #[must_use]
    pub fn width(self) -> f32 {
        self.a1 - self.a0
    }

    /// Returns `true` if `theta` falls within `[a0, a1)` modulo `2π`.
    #[inline]
    #[must_use]
    pub fn contains(self, theta: f32) -> bool {
        angle_in_span(theta, self.a0, self.a1)
    }

    /// Returns `true` if `theta` falls within `[a0 - eps, a1 + eps)` modulo `2π`.
    #[inline]
    #[must_use]
    pub fn contains_widened(self, theta: f32, eps: f32) -> bool {
        let (w0, w1) = widen_span(self.a0, self.a1, eps);
        angle_in_span(theta, w0, w1)
    }
}

/// Computes the segment spans for a root wheel with `n` items (`WHEEL_ENGINE_DESIGN.md` §4.2).
///
/// Item 0 is centered at top (`3π/2`), advancing clockwise by `Δ = 2π / n`.
#[must_use]
pub fn root_segment_spans(n: usize) -> SmallVec<[SegmentSpan; 12]> {
    let mut out = SmallVec::new();
    if n == 0 {
        return out;
    }
    let delta = TAU / (n as f32);
    let top = 3.0 * FRAC_PI_2;
    let base = normalize_angle(top - delta * 0.5);
    for k in 0..n {
        let a0 = if k == 0 {
            base
        } else {
            normalize_angle(base + (k as f32) * delta)
        };
        let a1 = if n == 1 {
            a0 + TAU
        } else {
            let next_boundary = if k + 1 == n {
                base
            } else {
                normalize_angle(base + ((k + 1) as f32) * delta)
            };
            let span = (next_boundary - a0).rem_euclid(TAU);
            a0 + if span <= 1e-5 { TAU } else { span }
        };
        let center = normalize_angle(a0 + (a1 - a0) * 0.5);
        out.push(SegmentSpan { a0, a1, center });
    }
    out
}

/// Computes the bridge span `(b0, b1)` and segment spans for a child wheel with `n` items
/// (`WHEEL_ENGINE_DESIGN.md` §4.2).
///
/// - `beta`: angle from the child center toward the parent center.
/// - `bridge_angle_rad`: angular width of the neutral bridge sector centered on `beta` (default `70°`).
#[must_use]
pub fn child_segment_spans(
    n: usize,
    beta: f32,
    bridge_angle_rad: f32,
) -> (SegmentSpan, SmallVec<[SegmentSpan; 12]>) {
    let w_b = bridge_angle_rad.clamp(0.1, PI);
    let b_center = normalize_angle(beta);
    let b0 = normalize_angle(b_center - w_b * 0.5);
    let bridge = SegmentSpan {
        a0: b0,
        a1: b0 + w_b,
        center: b_center,
    };

    let mut out = SmallVec::new();
    if n == 0 {
        return (bridge, out);
    }

    let max_delta = 72.0_f32.to_radians();
    let full_avail = TAU - w_b;
    let delta = (full_avail / (n as f32)).min(max_delta);
    let total_arc = (n as f32) * delta;
    let away = normalize_angle(beta + PI);
    let start_arc = if (total_arc - full_avail).abs() <= 1e-5 {
        normalize_angle(b0 + w_b)
    } else {
        normalize_angle(away - total_arc * 0.5)
    };

    for k in 0..n {
        let a0 = if k == 0 {
            start_arc
        } else {
            normalize_angle(start_arc + (k as f32) * delta)
        };
        let next_boundary = if k + 1 == n && (total_arc - full_avail).abs() <= 1e-5 {
            b0
        } else {
            normalize_angle(start_arc + ((k + 1) as f32) * delta)
        };
        let span = (next_boundary - a0).rem_euclid(TAU);
        let a1 = a0 + span;
        let center = normalize_angle(a0 + span * 0.5);
        out.push(SegmentSpan { a0, a1, center });
    }
    (bridge, out)
}

/// Angular inset `δ(r) = (gap / 2) / r` at radius `r` for a constant linear visual gap (`§4.2`).
#[inline]
#[must_use]
pub fn gap_inset_angle(gap: f32, r: f32) -> f32 {
    if r <= 1e-3 {
        0.0
    } else {
        (gap * 0.5 / r).min(0.25)
    }
}

/// Centroid radius `r_c = r_in + 0.55 * (r_out - r_in)` for placing a segment's content block (`§4.3`).
#[inline]
#[must_use]
pub fn segment_content_radius(r_in: f32, r_out: f32) -> f32 {
    r_in + 0.55 * (r_out - r_in)
}

/// Maximum horizontal label width inside a segment (`WHEEL_ENGINE_DESIGN.md` §4.3).
#[inline]
#[must_use]
pub fn segment_label_max_width(r_in: f32, r_out: f32, span_rad: f32, gap: f32) -> f32 {
    let r_c = segment_content_radius(r_in, r_out);
    let delta = gap_inset_angle(gap, r_c);
    let eff_angle = (span_rad - 2.0 * delta).max(0.05);
    let chord = 2.0 * r_c * (eff_angle * 0.5).sin();
    0.82 * chord.min(r_out - r_in)
}

/// Computes the two tangent points from external point `p` to circle `(center, radius)`.
///
/// If `p` is inside or on the circle (`d <= radius`), returns the two endpoints of the diameter
/// perpendicular to `p -> center`.
#[must_use]
pub fn circle_tangent_points(p: Point, center: Point, radius: f32) -> (Point, Point) {
    let d = p.distance(center);
    let base_angle = center.angle_to(p);
    if d <= radius + 1e-3 {
        let a1 = base_angle + FRAC_PI_2;
        let a2 = base_angle - FRAC_PI_2;
        return (
            center.offset_polar(radius, a1),
            center.offset_polar(radius, a2),
        );
    }
    // Angle at `center` between `center -> p` and `center -> tangent_point` is acos(radius / d)
    let alpha = (radius / d).clamp(-1.0, 1.0).acos();
    let t1 = center.offset_polar(radius, base_angle + alpha);
    let t2 = center.offset_polar(radius, base_angle - alpha);
    (t1, t2)
}

/// Returns `true` if `p` lies inside (or on the boundary of) triangle `(a, b, c)`.
#[must_use]
pub fn point_in_triangle(p: Point, a: Point, b: Point, c: Point) -> bool {
    let c1 = cross(a, b, p);
    let c2 = cross(b, c, p);
    let c3 = cross(c, a, p);
    let has_neg = (c1 < -1e-4) || (c2 < -1e-4) || (c3 < -1e-4);
    let has_pos = (c1 > 1e-4) || (c2 > 1e-4) || (c3 > 1e-4);
    !(has_neg && has_pos)
}

/// Computes the convex hull in clockwise order (in `+y` down coordinates) of up to 8 points.
#[must_use]
pub fn convex_hull_4(pts: [Point; 4]) -> SmallVec<[Point; 6]> {
    let mut sorted = pts;
    sorted.sort_by(|a, b| {
        a.x.partial_cmp(&b.x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut lower: SmallVec<[Point; 6]> = SmallVec::new();
    for &p in &sorted {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 1e-5 {
            lower.pop();
        }
        lower.push(p);
    }

    let mut upper: SmallVec<[Point; 6]> = SmallVec::new();
    for &p in sorted.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 1e-5 {
            upper.pop();
        }
        upper.push(p);
    }

    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Computes the navigation corridor polygon between a parent segment and its child wheel
/// (`WHEEL_ENGINE_DESIGN.md` §6.2).
///
/// The corridor is the convex hull of:
/// - The two outer arc endpoints of the parent segment at `r_p` (`span` inset by `gap_inset_angle`).
/// - The two points on the child's `hit_radius` circle perpendicular to `c_p -> c_c`.
#[must_use]
pub fn child_corridor_polygon(
    c_p: Point,
    r_p: f32,
    parent_span: SegmentSpan,
    segment_gap: f32,
    c_c: Point,
    child_hit_radius: f32,
) -> SmallVec<[Point; 6]> {
    let delta = gap_inset_angle(segment_gap, r_p);
    let a0 = parent_span.a0 + delta;
    let a1 = (parent_span.a1 - delta).max(a0);
    let p_arc0 = c_p.offset_polar(r_p, a0);
    let p_arc1 = c_p.offset_polar(r_p, a1);

    let axis = c_p.angle_to(c_c);
    let p_c0 = c_c.offset_polar(child_hit_radius, axis + FRAC_PI_2);
    let p_c1 = c_c.offset_polar(child_hit_radius, axis - FRAC_PI_2);

    convex_hull_4([p_arc0, p_arc1, p_c0, p_c1])
}

/// Computes the displacement corridor polygon between the summon anchor disc (`r = 24 lp`)
/// and a displaced root wheel's `hit_radius` disc (`WHEEL_ENGINE_DESIGN.md` §5.1).
#[must_use]
pub fn anchor_corridor_polygon(
    anchor: Point,
    anchor_r: f32,
    c_root: Point,
    root_hit_r: f32,
) -> SmallVec<[Point; 6]> {
    let axis = anchor.angle_to(c_root);
    let a0 = anchor.offset_polar(anchor_r, axis + FRAC_PI_2);
    let a1 = anchor.offset_polar(anchor_r, axis - FRAC_PI_2);
    let r0 = c_root.offset_polar(root_hit_r, axis + FRAC_PI_2);
    let r1 = c_root.offset_polar(root_hit_r, axis - FRAC_PI_2);
    convex_hull_4([a0, a1, r0, r1])
}

/// Returns `true` if `p` is inside or on the boundary of the convex polygon `poly`.
#[must_use]
pub fn point_in_convex_polygon(p: Point, poly: &[Point]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let mut has_pos = false;
    let mut has_neg = false;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let c = cross(a, b, p);
        if c > 1e-4 {
            has_pos = true;
        } else if c < -1e-4 {
            has_neg = true;
        }
        if has_pos && has_neg {
            return false;
        }
    }
    true
}

/// Distance from point `p` to line segment `a..b`.
#[must_use]
pub fn point_to_segment_distance(p: Point, a: Point, b: Point) -> f32 {
    let vx = b.x - a.x;
    let vy = b.y - a.y;
    let len_sq = vx * vx + vy * vy;
    if len_sq <= 1e-6 {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * vx + (p.y - a.y) * vy) / len_sq).clamp(0.0, 1.0);
    let proj = Point::new(a.x + t * vx, a.y + t * vy);
    p.distance(proj)
}

#[inline]
fn cross(o: Point, a: Point, b: Point) -> f32 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}
