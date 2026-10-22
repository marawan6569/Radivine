//! Deterministic wheel, stem, and caption placement (`WHEEL_ENGINE_DESIGN.md` §4.5, §5).

use std::f32::consts::PI;

use crate::config::WheelConfig;
use crate::geometry::{
    Point, Rect, Viewport, angle_diff, normalize_angle, point_to_segment_distance,
};
use crate::scene::CaptionAlign;

/// Minimum distance between a wheel's outer rim and the viewport edge (`8.0 lp`).
pub const EDGE_MARGIN: f32 = 8.0;

/// Minimum separation between a child wheel's outer rim and any non-parent ancestor's outer rim (`6.0 lp`).
pub const MIN_ANCESTOR_SEP: f32 = 6.0;

/// Reserved caption box width in logical pixels (`240.0 lp`).
pub const CAPTION_BOX_W: f32 = 240.0;

/// Reserved caption box height in logical pixels (`44.0 lp`).
pub const CAPTION_BOX_H: f32 = 44.0;

/// Gap between the active wheel's outer rim and the caption box (`10.0 lp`).
pub const CAPTION_GAP: f32 = 10.0;

/// Computed placement of a wheel in overlay-local logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Center of the wheel in overlay-local logical pixels.
    pub center: Point,
    /// Outer radius of the wheel in logical pixels.
    pub r_out: f32,
    /// Angle in `[0, 2π)` from the parent center to this wheel's center (`0.0` for root).
    pub attach_angle: f32,
    /// `true` if no candidate satisfied all non-overlap + viewport constraints and the fallback was used.
    pub fallback: bool,
}

/// Returns nominal `(r_in, r_out)` for a wheel at 0-based `level` (`0` = root, `1` = first child, `>= 2` = deep).
#[inline]
#[must_use]
pub fn nominal_radii(level: usize, cfg: &WheelConfig) -> (f32, f32) {
    let pair = match level {
        0 => cfg.radius_root,
        1 => cfg.radius_child,
        _ => cfg.radius_deep,
    };
    (pair[0], pair[1])
}

/// Scales `r_in` proportionally when a wheel's `r_out` is shrunk (`§5.2`).
#[inline]
#[must_use]
pub fn scaled_inner_radius(r_out: f32, r_in_nominal: f32, r_out_nominal: f32) -> f32 {
    if r_out_nominal <= 1e-3 {
        r_in_nominal
    } else {
        r_out * (r_in_nominal / r_out_nominal)
    }
}

/// Places the root wheel around `anchor`, clamped inside `viewport.bounds.inset(r_out_root + 8.0)` (`§5.1`).
#[inline]
#[must_use]
pub fn place_root(anchor: Point, r_out_root: f32, viewport: Viewport) -> Point {
    let inset_rect = viewport.bounds.inset(r_out_root + EDGE_MARGIN);
    inset_rect.clamp_point(anchor)
}

/// Places a child wheel adjacent to its parent (`c_p`, `r_p`) opened from a segment at angle `phi`
/// (`WHEEL_ENGINE_DESIGN.md` §5.2).
///
/// - `other_wheels`: slice of `(center, r_out)` for all visible non-retracting wheels *except* the immediate parent.
#[must_use]
pub fn place_child(
    c_p: Point,
    r_p: f32,
    phi: f32,
    r_c_nominal: f32,
    other_wheels: &[(Point, f32)],
    viewport: Viewport,
    cfg: &WheelConfig,
) -> Placement {
    let r_min = cfg.radius_min_outer.min(r_c_nominal);

    // Build candidate radii: [r_c, 0.9*r_c, 0.8*r_c, ... while > r_min] followed by r_min
    let mut radii = [0.0_f32; 8];
    let mut n_radii = 0usize;
    let mut k_scale = 0usize;
    while n_radii < 7 {
        let factor = 1.0 - 0.1 * (k_scale as f32);
        let r = r_c_nominal * factor;
        if r <= r_min + 0.5 {
            break;
        }
        radii[n_radii] = r;
        n_radii += 1;
        k_scale += 1;
    }
    radii[n_radii] = r_min;
    n_radii += 1;

    // Determine preferred rotation sign `s0` that moves `c` closer to the viewport center.
    let step_rad = 12.0_f32.to_radians();
    let d0 = r_p + cfg.child_gap + r_c_nominal;
    let vp_center = viewport.bounds.center();
    let c_plus = c_p.offset_polar(d0, phi + step_rad);
    let c_minus = c_p.offset_polar(d0, phi - step_rad);
    let s0: f32 = if c_plus.distance_sq(vp_center) <= c_minus.distance_sq(vp_center) {
        1.0
    } else {
        -1.0
    };

    // 13 angular offsets per radius: 0°, ±12°, ±24°, ±36°, ±48°, ±60°, ±72°
    let mut best_fallback = Placement {
        center: c_p.offset_polar(d0, phi),
        r_out: r_min,
        attach_angle: normalize_angle(phi),
        fallback: true,
    };
    let mut best_penalty = f32::INFINITY;

    for &r in &radii[..n_radii] {
        let d = r_p + cfg.child_gap + r;
        for step_idx in 0..13usize {
            let (k, s) = if step_idx == 0 {
                (0usize, 1.0_f32)
            } else {
                let k = step_idx.div_ceil(2);
                let s = if step_idx % 2 == 1 { s0 } else { -s0 };
                (k, s)
            };
            let theta = normalize_angle(phi + s * (k as f32) * step_rad);
            let c = c_p.offset_polar(d, theta);

            let in_vp = viewport.bounds.contains_disc(c, r + EDGE_MARGIN);
            let no_overlap = other_wheels
                .iter()
                .all(|&(c_w, r_w)| c.distance(c_w) >= r + r_w + MIN_ANCESTOR_SEP - 1e-3);

            if in_vp && no_overlap {
                return Placement {
                    center: c,
                    r_out: r,
                    attach_angle: theta,
                    fallback: false,
                };
            }

            // Compute fallback score: 10 * area_outside_viewport + overlap_area
            let out_area = disc_area_outside_rect(c, r + EDGE_MARGIN, viewport.bounds);
            let mut ov_area = circle_overlap_area(c, r, c_p, r_p);
            for &(c_w, r_w) in other_wheels {
                ov_area += circle_overlap_area(c, r, c_w, r_w + MIN_ANCESTOR_SEP);
            }
            // Small tie-breaker favoring smaller rotation and larger radius
            let tie = (k as f32) * 0.1 + (r_c_nominal - r) * 0.05;
            let penalty = 10.0 * out_area + ov_area + tie;
            if penalty < best_penalty {
                best_penalty = penalty;
                best_fallback = Placement {
                    center: c,
                    r_out: r,
                    attach_angle: theta,
                    fallback: true,
                };
            }
        }
    }

    // Clamp fallback center into viewport
    let clamp_rect = viewport.bounds.inset(best_fallback.r_out + EDGE_MARGIN);
    let clamped_c = clamp_rect.clamp_point(best_fallback.center);
    let attach_angle = if clamped_c.distance_sq(c_p) > 1e-4 {
        c_p.angle_to(clamped_c)
    } else {
        best_fallback.attach_angle
    };
    Placement {
        center: clamped_c,
        r_out: best_fallback.r_out,
        attach_angle,
        fallback: true,
    }
}

/// Computes the quadratic Bézier stem points `(p0, ctrl, p1)` connecting parent `(c_p, r_p)`
/// at segment angle `phi` to child `(c_c, r_c)` (`WHEEL_ENGINE_DESIGN.md` §5.3).
#[must_use]
pub fn compute_stem_points(
    c_p: Point,
    r_p: f32,
    phi: f32,
    c_c: Point,
    r_c: f32,
    attach_angle: f32,
    child_gap: f32,
) -> (Point, Point, Point) {
    let p0 = c_p.offset_polar(r_p, phi);
    let beta = c_c.angle_to(c_p);
    let p1 = c_c.offset_polar(r_c, beta);
    let ctrl = if angle_diff(attach_angle, phi).abs() < 1e-3 {
        p0.lerp(p1, 0.5)
    } else {
        c_p.offset_polar(r_p + child_gap, phi)
    };
    (p0, ctrl, p1)
}

/// Places the `240 × 44 lp` caption box around the active wheel (`WHEEL_ENGINE_DESIGN.md` §4.5).
///
/// Candidates tried in order: `Below`, `Above`, `Right`, `Left` at distance `r_out + 10 lp`.
#[must_use]
pub fn place_caption(
    active_center: Point,
    active_r_out: f32,
    visible_hit_discs: &[(Point, f32)],
    stems: &[(Point, Point, Point)],
    viewport: Viewport,
) -> (Rect, CaptionAlign) {
    let dist = active_r_out + CAPTION_GAP;
    let candidates = [
        (
            CaptionAlign::Below,
            Rect::new(
                active_center.x - CAPTION_BOX_W * 0.5,
                active_center.y + dist,
                CAPTION_BOX_W,
                CAPTION_BOX_H,
            ),
        ),
        (
            CaptionAlign::Above,
            Rect::new(
                active_center.x - CAPTION_BOX_W * 0.5,
                active_center.y - dist - CAPTION_BOX_H,
                CAPTION_BOX_W,
                CAPTION_BOX_H,
            ),
        ),
        (
            CaptionAlign::Right,
            Rect::new(
                active_center.x + dist,
                active_center.y - CAPTION_BOX_H * 0.5,
                CAPTION_BOX_W,
                CAPTION_BOX_H,
            ),
        ),
        (
            CaptionAlign::Left,
            Rect::new(
                active_center.x - dist - CAPTION_BOX_W,
                active_center.y - CAPTION_BOX_H * 0.5,
                CAPTION_BOX_W,
                CAPTION_BOX_H,
            ),
        ),
    ];

    let inner_vp = viewport.bounds.inset(EDGE_MARGIN);
    for (align, rect) in candidates {
        if !rect.is_inside(inner_vp) {
            continue;
        }
        let hits_wheel = visible_hit_discs
            .iter()
            .any(|&(c, hr)| c.distance_sq(active_center) > 1.0 && rect.intersects_disc(c, hr));
        if hits_wheel {
            continue;
        }
        let hits_stem = stems.iter().any(|&(p0, ctrl, p1)| {
            rect_intersects_segment(rect, p0, ctrl) || rect_intersects_segment(rect, ctrl, p1)
        });
        if !hits_stem {
            return (rect, align);
        }
    }

    // Fallback: Below, clamped into viewport
    let fallback_rect = candidates[0].1.clamp_within(inner_vp);
    (fallback_rect, CaptionAlign::Below)
}

fn rect_intersects_segment(rect: Rect, a: Point, b: Point) -> bool {
    if rect.contains_point(a) || rect.contains_point(b) {
        return true;
    }
    let c = rect.center();
    let half_diag = (rect.w * 0.5).hypot(rect.h * 0.5);
    if point_to_segment_distance(c, a, b) > half_diag {
        return false;
    }
    // Sample 4 points along segment
    for i in 1..=3 {
        let p = a.lerp(b, (i as f32) * 0.25);
        if rect.contains_point(p) {
            return true;
        }
    }
    false
}

fn disc_area_outside_rect(c: Point, r: f32, rect: Rect) -> f32 {
    let dx_left = (rect.x - (c.x - r)).max(0.0);
    let dx_right = ((c.x + r) - rect.right()).max(0.0);
    let dy_top = (rect.y - (c.y - r)).max(0.0);
    let dy_bot = ((c.y + r) - rect.bottom()).max(0.0);
    let ox = dx_left + dx_right;
    let oy = dy_top + dy_bot;
    if ox <= 0.0 && oy <= 0.0 {
        0.0
    } else {
        2.0 * r * (ox + oy)
    }
}

fn circle_overlap_area(c1: Point, r1: f32, c2: Point, r2: f32) -> f32 {
    let d = c1.distance(c2);
    if d >= r1 + r2 {
        return 0.0;
    }
    if d <= (r1 - r2).abs() {
        let r_min = r1.min(r2);
        return PI * r_min * r_min;
    }
    let r1_sq = r1 * r1;
    let r2_sq = r2 * r2;
    let alpha = ((d * d + r1_sq - r2_sq) / (2.0 * d * r1))
        .clamp(-1.0, 1.0)
        .acos();
    let beta = ((d * d + r2_sq - r1_sq) / (2.0 * d * r2))
        .clamp(-1.0, 1.0)
        .acos();
    let term = (-d + r1 + r2) * (d + r1 - r2) * (d - r1 + r2) * (d + r1 + r2);
    r1_sq * alpha + r2_sq * beta - 0.5 * term.max(0.0).sqrt()
}
