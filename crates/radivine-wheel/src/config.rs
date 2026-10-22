//! Configuration types and validation for `radivine-wheel`.

/// Rule controlling how submenus open when hovered or clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmenuOpen {
    /// Open after dwelling on a submenu item for `dwell_ms`.
    Dwell,
    /// Open when pushing outward into the outer `band` of a submenu item.
    Band,
    /// Open via either dwell or outer-band push (default).
    DwellAndBand,
    /// Open only on explicit click or keyboard activation.
    Click,
}

/// Rule controlling how leaf items commit in Pointer/Invoke mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvokeSelect {
    /// Commit on press+release on the same item, or release on an item after pressing on the hub.
    Click,
    /// Commit on release over an enabled leaf item regardless of where press occurred.
    Release,
}

/// Configuration parameters for the radial wheel engine.
///
/// Mirrors `[wheel]`, `[wheel.intent]`, `[invoke]`, and `appearance.reduced_motion`
/// from `WHEEL_ENGINE_DESIGN.md` §9–§10.
#[derive(Clone, Debug, PartialEq)]
pub struct WheelConfig {
    /// Maximum items per wheel before overflowing into an automatic "More…" submenu (default 8, hard limit 12).
    pub max_items_per_wheel: u8,
    /// Maximum hierarchy depth where root = 1 (default 4, hard limit 6).
    pub max_depth: u8,
    /// Pointer travel distance from summon anchor before hover/drop activates (default 6.0 lp).
    pub activation_slop: f32,
    /// Extra outer hit-test margin beyond `r_out` (default 12.0 lp).
    pub hit_margin: f32,
    /// Linear visual gap between adjacent segments (default 2.5 lp).
    pub segment_gap: f32,
    /// Gap between a parent wheel's outer rim and its child wheel's outer rim (default 10.0 lp).
    pub child_gap: f32,
    /// Angular width in degrees of a child wheel's neutral bridge sector facing its parent (default 70.0°).
    pub bridge_angle_deg: f32,
    /// Angular hysteresis in degrees added to each side of the currently hovered segment (default 4.0°).
    pub hysteresis_deg: f32,
    /// Nominal `[r_in, r_out]` in logical pixels for the root wheel (default `[38.0, 116.0]`).
    pub radius_root: [f32; 2],
    /// Nominal `[r_in, r_out]` in logical pixels for depth-2 (first child) wheels (default `[30.0, 96.0]`).
    pub radius_child: [f32; 2],
    /// Nominal `[r_in, r_out]` in logical pixels for depth-3+ wheels (default `[26.0, 84.0]`).
    pub radius_deep: [f32; 2],
    /// Minimum outer radius when shrinking a child wheel to fit available space (default 64.0 lp).
    pub radius_min_outer: f32,
    /// Submenu activation mode (default `SubmenuOpen::DwellAndBand`).
    pub submenu_open: SubmenuOpen,
    /// Dwell time in milliseconds to open a hovered submenu (default 140 ms).
    pub dwell_ms: u16,
    /// Fractional radial threshold `r_in + band * (r_out - r_in)` for outer-band push (default 0.65).
    pub band: f32,
    /// Grace duration in milliseconds for the hover-intent protection triangle (default 300 ms).
    pub grace_ms: u16,
    /// Dwell time in milliseconds inside a wheel's hub before its open child closes (default 150 ms).
    pub hub_close_ms: u16,
    /// Selection gesture in Pointer/Invoke mode (default `InvokeSelect::Click`).
    pub invoke_select: InvokeSelect,
    /// Whether reduced motion is active (disables scale/stem/pop animations; keeps ≤ 60 ms opacity fades).
    pub reduced_motion: bool,
}

impl Default for WheelConfig {
    fn default() -> Self {
        Self {
            max_items_per_wheel: 8,
            max_depth: 4,
            activation_slop: 6.0,
            hit_margin: 12.0,
            segment_gap: 2.5,
            child_gap: 10.0,
            bridge_angle_deg: 70.0,
            hysteresis_deg: 4.0,
            radius_root: [38.0, 116.0],
            radius_child: [30.0, 96.0],
            radius_deep: [26.0, 84.0],
            radius_min_outer: 64.0,
            submenu_open: SubmenuOpen::DwellAndBand,
            dwell_ms: 140,
            band: 0.65,
            grace_ms: 300,
            hub_close_ms: 150,
            invoke_select: InvokeSelect::Click,
            reduced_motion: false,
        }
    }
}

impl WheelConfig {
    /// Clamps out-of-range configuration values into safe, valid bounds.
    pub fn validate(&mut self) {
        self.max_items_per_wheel = self.max_items_per_wheel.clamp(2, 12);
        self.max_depth = self.max_depth.clamp(1, 6);
        self.activation_slop = self.activation_slop.clamp(0.0, 32.0);
        self.hit_margin = self.hit_margin.clamp(0.0, 32.0);
        self.segment_gap = self.segment_gap.clamp(0.0, 12.0);
        self.child_gap = self.child_gap.clamp(2.0, 48.0);
        self.bridge_angle_deg = self.bridge_angle_deg.clamp(40.0, 100.0);
        self.hysteresis_deg = self.hysteresis_deg.clamp(0.0, 15.0);
        self.radius_min_outer = self.radius_min_outer.clamp(48.0, 120.0);

        for pair in [
            &mut self.radius_root,
            &mut self.radius_child,
            &mut self.radius_deep,
        ] {
            pair[1] = pair[1].clamp(self.radius_min_outer, 240.0);
            pair[0] = pair[0].clamp(16.0, pair[1] - 24.0);
        }

        self.dwell_ms = self.dwell_ms.clamp(0, 1000);
        self.band = self.band.clamp(0.3, 0.95);
        self.grace_ms = self.grace_ms.clamp(0, 1500);
        self.hub_close_ms = self.hub_close_ms.clamp(0, 1000);
    }

    /// Returns a validated clone of this configuration.
    #[must_use]
    pub fn validated(&self) -> Self {
        let mut copy = self.clone();
        copy.validate();
        copy
    }
}
