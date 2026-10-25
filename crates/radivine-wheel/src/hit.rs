//! Polar hit-testing with radial/angular hysteresis and submenu margin protection
//! (`WHEEL_ENGINE_DESIGN.md` §6).

use smallvec::SmallVec;

use crate::geometry::{Point, SegmentSpan, point_in_convex_polygon};

/// Hit-test zone returned by [`hit_test`] (`WHEEL_ENGINE_DESIGN.md` §6.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Zone {
    /// Inside the center dead-zone hub of `wheel`.
    Hub {
        /// Index of the wheel in the active chain (`0` = root).
        wheel: u8,
    },
    /// Inside segment `index` of `wheel`.
    Item {
        /// Index of the wheel in the active chain (`0` = root).
        wheel: u8,
        /// 0-based segment index within `wheel`.
        index: u8,
    },
    /// Inside the neutral parent-facing bridge sector of child `wheel`.
    Bridge {
        /// Index of the child wheel (`>= 1`).
        wheel: u8,
    },
    /// Inside the ring of child `wheel`, but outside all item spans and the bridge sector.
    Empty {
        /// Index of the wheel.
        wheel: u8,
    },
    /// Inside the convex navigation corridor leading from a parent segment to `child`.
    Corridor {
        /// Index of the child wheel (`>= 1`).
        child: u8,
    },
    /// Outside all wheels and corridors.
    #[default]
    Outside,
}

/// Precomputed hit-test geometry for a single active (non-retracting) wheel in the chain.
///
/// Note (`ARCHITECTURE.md` §A2): For [`crate::WheelLayout::Concentric`] sessions, `WheelHitGeometry`
/// is retained as a representation for debug and `item_center` queries only. Concentric hit testing
/// MUST use [`crate::concentric::hit_concentric`] rather than [`hit_test`], because [`hit_test`]
/// evaluates child wheels deepest-first and would classify inner ancestor rings as child hubs.
#[derive(Clone, Debug)]
pub struct WheelHitGeometry {
    /// Index of this wheel in the chain (`0` = root).
    pub index: u8,
    /// Index of this wheel's parent in the chain (`None` for root).
    pub parent_index: Option<u8>,
    /// Center of the wheel in overlay-local logical pixels.
    pub center: Point,
    /// Inner hub radius in logical pixels.
    pub r_in: f32,
    /// Drawn outer ring radius in logical pixels.
    pub r_out: f32,
    /// Outer hit-test radius (`r_out + hit_margin`) in logical pixels.
    pub hit_radius: f32,
    /// Optional bridge sector span (present for child wheels `index >= 1`).
    pub bridge: Option<SegmentSpan>,
    /// Segment angular spans in clockwise order.
    pub spans: SmallVec<[SegmentSpan; 12]>,
    /// Optional convex corridor polygon from the parent segment to this child wheel.
    pub corridor: Option<SmallVec<[Point; 6]>>,
}

/// Evaluates the hit-test zone for point `p` (`WHEEL_ENGINE_DESIGN.md` §6.2).
///
/// Priority is deepest wheel first (`wheels` is ordered from root `0` to deepest `len - 1`).
///
/// Includes the M0 S-1 submenu overlap guard: a child wheel's outer `hit_margin`
/// (`r_out < d <= hit_radius`, which extends `12 lp` across the `10 lp` `child_gap`)
/// never preempts a point inside its parent wheel's **drawn** ring (`d_parent <= r_out(parent)`).
#[must_use]
pub fn hit_test(
    p: Point,
    wheels: &[WheelHitGeometry],
    current_hovered: Zone,
    hysteresis_rad: f32,
) -> Zone {
    for w in wheels.iter().rev() {
        let d = p.distance(w.center);
        let r_hub = if current_hovered == (Zone::Hub { wheel: w.index }) {
            w.r_in + 3.0
        } else {
            w.r_in
        };
        if d < r_hub {
            return Zone::Hub { wheel: w.index };
        }

        // M0 Submenu Hit-Margin Overlap Guard (WHEEL_ENGINE_DESIGN.md §6.2):
        // Prevent a child wheel's outer hit_margin (r_out < d <= hit_radius) from preempting
        // its parent's drawn ring (d_parent <= r_out(parent)) when a submenu opens while the
        // pointer is near the outer rim of the parent segment.
        if let Some(p_idx) = w.parent_index
            && d > w.r_out
            && let Some(parent) = wheels.iter().find(|pw| pw.index == p_idx)
            && p.distance(parent.center) <= parent.r_out
        {
            continue;
        }

        if d <= w.hit_radius {
            let theta = w.center.angle_to(p);

            // Check angular hysteresis on currently hovered item of this wheel first
            // (so boundary jitter against either an adjacent item or the bridge sector is absorbed).
            if let Zone::Item { wheel, index: h } = current_hovered
                && wheel == w.index
                && let Some(h_span) = w.spans.get(h as usize)
                && h_span.contains_widened(theta, hysteresis_rad)
            {
                return Zone::Item {
                    wheel: w.index,
                    index: h,
                };
            }

            if let Some(bridge) = w.bridge
                && bridge.contains(theta)
            {
                return Zone::Bridge { wheel: w.index };
            }

            for (k, span) in w.spans.iter().enumerate() {
                if span.contains(theta) {
                    return Zone::Item {
                        wheel: w.index,
                        index: k as u8,
                    };
                }
            }

            return Zone::Empty { wheel: w.index };
        }
    }

    for w in wheels.iter().rev() {
        if let Some(ref poly) = w.corridor
            && point_in_convex_polygon(p, poly)
        {
            return Zone::Corridor { child: w.index };
        }
    }

    Zone::Outside
}
