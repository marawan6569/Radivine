//! Concentric (nested) wheel navigation state, guards, and timers
//! (`INTERACTION_DESIGN.md` §I3–§I7; `ARCHITECTURE.md` §A2, §A3).

use smallvec::SmallVec;

use crate::anim::Millis;
use crate::concentric::{
    CONCENTRIC_FRESH_MOTION_LP, ConcentricPlan, NestedHit, RingGeometry, ring_geometry,
};
use crate::geometry::Point;
use crate::hit::Zone;
use crate::menu::ItemPath;

/// Active input source for a concentric session (`INTERACTION_DESIGN.md` §I6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ConcentricInputSource {
    /// Pointer motion/buttons control hover and selection.
    Pointer,
    /// Keyboard navigation controls focus; pointer must move `>= 2.0 lp` from `takeover_point`
    /// to resume pointer control.
    Keyboard { takeover_point: Point },
}

/// Pending submenu dwell timer for the deepest open ring (`INTERACTION_DESIGN.md` §I5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConcentricDwellTimer {
    /// Full path of the submenu sector being dwelled.
    pub path: ItemPath,
    /// Geometry generation when the dwell started.
    pub generation: u64,
    /// Timestamp when the pointer first entered this submenu sector.
    pub entered_at: Millis,
    /// Timestamp when the submenu should open (`max(entered_at + dwell_ms, adoption_time)`).
    pub deadline: Millis,
}

/// Pending ancestor sibling adoption timer (`INTERACTION_DESIGN.md` §I3, §I5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingSibling {
    /// 0-based ring level of the ancestor sector.
    pub level: u8,
    /// 0-based sector index on `level`.
    pub item: u8,
    /// Full path of the candidate ancestor sibling (`wheels[level].path.child(item)`).
    pub path: ItemPath,
    /// Geometry generation when the candidate was first entered.
    pub generation: u64,
    /// Timestamp when the pointer first entered this candidate sibling.
    pub entered_at: Millis,
    /// Timestamp (`entered_at + grace_ms`) when descendants after `level` close and this sibling is adopted.
    pub deadline: Millis,
}

/// Pending center-hub dwell timer to collapse open descendants back to the root ring (`INTERACTION_DESIGN.md` §I5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ConcentricHubCloseTimer {
    /// Geometry generation when the hub dwell started.
    pub generation: u64,
    /// Timestamp (`entered_at + hub_close_ms`) when descendants after root (`level 0`) close.
    pub deadline: Millis,
}

/// Recorded pointer press state for `InputMode::Pointer` (`INTERACTION_DESIGN.md` §I6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConcentricPress {
    /// Pressed inside the shared center hub (`NestedHit::Hub`).
    /// Supports both hub-click cancellation and hub-origin marking gestures across descendant openings.
    Hub,
    /// Pressed outside the active cluster (`NestedHit::Outside`).
    Outside,
    /// Pressed on an item sector at `path` during `generation`.
    Item {
        path: ItemPath,
        generation: u64,
        eligible_at_press: bool,
    },
    /// Pressed in a neutral non-action zone (`RadialGap`, `RadialBoundary`, `Separator`, `OuterMargin`, `Approach`).
    Neutral,
}

/// Complete concentric layout navigation state stored inside `LayoutState::Concentric` (`ARCHITECTURE.md` §A3).
#[derive(Clone, Debug)]
pub(crate) struct ConcentricState {
    /// Preflight reservation plan (center, reserved radius, caption box, per-level radii).
    pub plan: ConcentricPlan,
    /// Whether a real pointer motion sample has entered the hub (`distance(p, center) < h - 1e-4`).
    pub hub_visited_by_pointer: bool,
    /// Precomputed ring geometry for each currently open ring (`0..wheels.len()`).
    pub open_rings: SmallVec<[RingGeometry; 6]>,
    /// Monotonically increasing counter incremented on every ring open, close, or menu installation.
    pub geometry_gen: u64,
    /// Latest strict hit-test zone at `last_pointer`.
    pub raw_hit: NestedHit,
    /// Per-level pointer-exposure guard (`Some(exposed_at_pointer)` until pointer moves `>= 2.0 lp`).
    pub exposure_guards: [Option<Point>; 6],
    /// Active input source (`Pointer` vs `Keyboard { takeover_point }`).
    pub input_source: ConcentricInputSource,
    /// Pending deepest-ring submenu dwell timer, if any.
    pub dwell: Option<ConcentricDwellTimer>,
    /// Pending ancestor-ring sibling replacement timer, if any.
    pub pending_sibling: Option<PendingSibling>,
    /// Pending center-hub collapse timer, if any.
    pub hub_close: Option<ConcentricHubCloseTimer>,
    /// Active pointer button press state in `InputMode::Pointer`.
    pub pressed: Option<ConcentricPress>,
    /// Irreversible interaction suspension guard (`suspend_interaction`).
    pub interaction_suspended: bool,
    /// Last injected timestamp.
    pub last_timestamp: Millis,
}

impl ConcentricState {
    /// Creates a new [`ConcentricState`] from a validated [`ConcentricPlan`] and root item count.
    #[must_use]
    pub fn new(plan: ConcentricPlan, root_item_count: usize, now: Millis) -> Self {
        let initial_approach_required = plan.approach_required;
        let hub_visited_by_pointer = !initial_approach_required;
        let mut open_rings = SmallVec::new();
        open_rings.push(ring_geometry(&plan, 0, root_item_count));

        Self {
            plan,
            hub_visited_by_pointer,
            open_rings,
            geometry_gen: 1,
            raw_hit: if initial_approach_required {
                NestedHit::Approach
            } else {
                NestedHit::Outside
            },
            exposure_guards: [None; 6],
            input_source: ConcentricInputSource::Pointer,
            dwell: None,
            pending_sibling: None,
            hub_close: None,
            pressed: None,
            interaction_suspended: false,
            last_timestamp: now,
        }
    }

    /// Updates all per-level `exposure_guards` that have moved at least `CONCENTRIC_FRESH_MOTION_LP` (`2.0 lp`)
    /// from their recorded exposure point.
    pub fn update_exposure_guards(&mut self, p: Point) {
        for guard in &mut self.exposure_guards {
            if let Some(exposed_at) = *guard
                && p.distance(exposed_at) >= CONCENTRIC_FRESH_MOTION_LP
            {
                *guard = None;
            }
        }
    }

    /// Returns `true` if `level` is currently guarded by an uncleared `exposed_at_pointer`.
    #[inline]
    #[must_use]
    pub fn is_level_exposure_guarded(&self, level: u8) -> bool {
        self.exposure_guards
            .get(level as usize)
            .copied()
            .flatten()
            .is_some()
    }

    /// Maps a [`NestedHit`] to the legacy [`Zone`] representation for `DebugState::raw_zone`.
    #[inline]
    #[must_use]
    pub fn hit_to_debug_zone(hit: NestedHit) -> Zone {
        match hit {
            NestedHit::Hub => Zone::Hub { wheel: 0 },
            NestedHit::Item { level, index } => Zone::Item {
                wheel: level,
                index,
            },
            NestedHit::Separator { level } | NestedHit::RadialBoundary { level } => {
                Zone::Empty { wheel: level }
            }
            NestedHit::RadialGap { inner_level } => Zone::Empty { wheel: inner_level },
            NestedHit::OuterMargin | NestedHit::Approach | NestedHit::Outside => Zone::Outside,
        }
    }

    /// Returns the earliest active interactive timer deadline (`dwell`, `pending_sibling`, or `hub_close`).
    #[must_use]
    pub fn next_deadline(&self) -> Option<Millis> {
        if self.interaction_suspended {
            return None;
        }
        let mut earliest: Option<Millis> = None;
        for cand in [
            self.dwell.as_ref().map(|d| d.deadline),
            self.pending_sibling.as_ref().map(|p| p.deadline),
            self.hub_close.as_ref().map(|h| h.deadline),
        ]
        .into_iter()
        .flatten()
        {
            earliest = Some(earliest.map_or(cand, |e| e.min(cand)));
        }
        earliest
    }
}
