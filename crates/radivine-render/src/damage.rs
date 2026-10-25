//! Physical-pixel damage rectangle tracking and merging (`WHEEL_ENGINE_DESIGN.md` §13.3, `ARCHITECTURE.md` §A4).

use std::collections::hash_map::DefaultHasher;
use std::f32::consts::FRAC_PI_2;
use std::hash::{Hash, Hasher};
use std::mem::discriminant;

use radivine_wheel::{
    CaptionVisual, OriginGuideVisual, Scene, StemVisual, WheelVisual, WheelVisualKind,
};
use smallvec::SmallVec;

/// Physical pixel dimensions of the render surface (`WHEEL_ENGINE_DESIGN.md` §13.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SizePx {
    /// Width in physical pixels.
    pub w: u32,
    /// Height in physical pixels.
    pub h: u32,
}

impl SizePx {
    /// Creates a new `SizePx { w, h }`.
    #[inline]
    #[must_use]
    pub const fn new(w: u32, h: u32) -> Self {
        Self { w, h }
    }
}

/// Physical-pixel damage rectangle (`WHEEL_ENGINE_DESIGN.md` §13.1, §13.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RectPx {
    /// Left coordinate in physical pixels.
    pub x: i32,
    /// Top coordinate in physical pixels.
    pub y: i32,
    /// Width in physical pixels.
    pub w: u32,
    /// Height in physical pixels.
    pub h: u32,
}

impl RectPx {
    /// Creates a new `RectPx`.
    #[inline]
    #[must_use]
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// Right edge `x + w as i32`.
    #[inline]
    #[must_use]
    pub const fn right(self) -> i32 {
        self.x + (self.w as i32)
    }

    /// Bottom edge `y + h as i32`.
    #[inline]
    #[must_use]
    pub const fn bottom(self) -> i32 {
        self.y + (self.h as i32)
    }

    /// Area in square physical pixels.
    #[inline]
    #[must_use]
    pub const fn area(self) -> u64 {
        (self.w as u64) * (self.h as u64)
    }

    /// Returns `true` if `w == 0` or `h == 0`.
    #[inline]
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// Returns the bounding union of `self` and `other`.
    #[inline]
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self.right().max(other.right());
        let y1 = self.bottom().max(other.bottom());
        Self {
            x: x0,
            y: y0,
            w: (x1 - x0).max(0) as u32,
            h: (y1 - y0).max(0) as u32,
        }
    }

    /// Returns `true` if `self` overlaps or touches `other` within `pad` pixels.
    #[inline]
    #[must_use]
    pub fn overlaps_or_touches(self, other: Self, pad: i32) -> bool {
        self.x - pad <= other.right()
            && self.right() + pad >= other.x
            && self.y - pad <= other.bottom()
            && self.bottom() + pad >= other.y
    }

    /// Clamps this rectangle to the surface bounds `size`.
    #[inline]
    #[must_use]
    pub fn clamp_to_size(self, size: SizePx) -> Self {
        let x0 = self.x.clamp(0, size.w as i32);
        let y0 = self.y.clamp(0, size.h as i32);
        let x1 = self.right().clamp(x0, size.w as i32);
        let y1 = self.bottom().clamp(y0, size.h as i32);
        Self {
            x: x0,
            y: y0,
            w: (x1 - x0) as u32,
            h: (y1 - y0) as u32,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ElementRecord {
    id_key: u64,
    param_hash: u64,
    bbox: RectPx,
}

/// Tracks element bounding boxes across frames to emit `<= 4` physical damage rectangles (`§13.3`).
#[derive(Clone, Debug, Default)]
pub struct DamageTracker {
    prev_elements: Vec<ElementRecord>,
    curr_elements: Vec<ElementRecord>,
    damage_out: SmallVec<[RectPx; 4]>,
    first_frame: bool,
}

impl DamageTracker {
    /// Creates a new `DamageTracker`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            prev_elements: Vec::with_capacity(24),
            curr_elements: Vec::with_capacity(24),
            damage_out: SmallVec::new(),
            first_frame: true,
        }
    }

    /// Resets the tracker at the start of a session.
    pub fn reset(&mut self) {
        self.prev_elements.clear();
        self.curr_elements.clear();
        self.damage_out.clear();
        self.first_frame = true;
    }

    /// Force-marks the entire active scene as damaged (e.g. when theme changes).
    pub fn invalidate_all(&mut self) {
        self.prev_elements.clear();
        self.first_frame = true;
    }

    /// Computes the damage rectangles (`<= 4`) for `scene` at `scale` inside `size`.
    pub fn compute_damage(&mut self, scene: &Scene, size: SizePx, scale: f32) -> &[RectPx] {
        self.curr_elements.clear();
        self.damage_out.clear();

        // 0. Displaced-origin guide
        if let Some(ref og) = scene.origin_guide {
            let bbox = origin_guide_bbox_px(og, scale).clamp_to_size(size);
            if !bbox.is_empty() {
                self.curr_elements.push(ElementRecord {
                    id_key: 0x0800_0000,
                    param_hash: hash_origin_guide(og),
                    bbox,
                });
            }
        }

        // 1. Stems
        for (i, stem) in scene.stems.iter().enumerate() {
            let bbox = stem_bbox_px(stem, scale).clamp_to_size(size);
            if !bbox.is_empty() {
                self.curr_elements.push(ElementRecord {
                    id_key: 0x1000_0000 | (i as u64),
                    param_hash: hash_stem(stem),
                    bbox,
                });
            }
        }

        // 2. Wheels
        for (w_idx, wheel) in scene.wheels.iter().enumerate() {
            let wheel_key = 0x2000_0000 | ((w_idx as u64) << 16);
            let w_bbox = wheel_bbox_px(wheel, scale).clamp_to_size(size);
            if !w_bbox.is_empty() {
                self.curr_elements.push(ElementRecord {
                    id_key: wheel_key,
                    param_hash: hash_wheel_base(wheel),
                    bbox: w_bbox,
                });
            }
            // Track each segment's highlight/pop/selection/content individually so hovering between
            // segments only damages the changed segments' bounding boxes rather than the whole wheel!
            for (s_idx, seg) in wheel.segments.iter().enumerate() {
                let s_bbox = segment_bbox_px(wheel, s_idx, seg, scale).clamp_to_size(size);
                if !s_bbox.is_empty() {
                    self.curr_elements.push(ElementRecord {
                        id_key: wheel_key | ((s_idx as u64) + 1),
                        param_hash: hash_segment(wheel, s_idx, seg),
                        bbox: s_bbox,
                    });
                }
            }
        }

        // 3. Caption
        if let Some(ref cap) = scene.caption {
            let bbox = caption_bbox_px(cap, scale).clamp_to_size(size);
            if !bbox.is_empty() {
                self.curr_elements.push(ElementRecord {
                    id_key: 0x3000_0000,
                    param_hash: hash_caption(cap),
                    bbox,
                });
            }
        }

        let mut raw_rects: SmallVec<[RectPx; 24]> = SmallVec::new();

        if self.first_frame {
            self.first_frame = false;
            // First frame of a session: damage = union of all element boxes (§13.3)
            let mut u = RectPx::default();
            for el in &self.curr_elements {
                u = u.union(el.bbox);
            }
            if !u.is_empty() {
                raw_rects.push(u);
            }
        } else {
            // Added or changed elements
            for curr in &self.curr_elements {
                match self.prev_elements.iter().find(|p| p.id_key == curr.id_key) {
                    Some(prev) => {
                        if prev.param_hash != curr.param_hash || prev.bbox != curr.bbox {
                            raw_rects.push(prev.bbox.union(curr.bbox));
                        }
                    }
                    None => {
                        raw_rects.push(curr.bbox);
                    }
                }
            }
            // Removed elements
            for prev in &self.prev_elements {
                if !self.curr_elements.iter().any(|c| c.id_key == prev.id_key) {
                    raw_rects.push(prev.bbox);
                }
            }
        }

        self.prev_elements.clear();
        self.prev_elements.extend_from_slice(&self.curr_elements);

        self.damage_out = merge_into_at_most_4(&mut raw_rects);
        &self.damage_out
    }

    /// Returns the most recently computed damage rectangles.
    #[inline]
    #[must_use]
    pub fn last_damage(&self) -> &[RectPx] {
        &self.damage_out
    }
}

/// Merges overlapping rectangles and reduces count to `<= 4` by greedily merging pairs with minimal dead area.
#[must_use]
pub fn merge_into_at_most_4(rects: &mut SmallVec<[RectPx; 24]>) -> SmallVec<[RectPx; 4]> {
    rects.retain(|r| !r.is_empty());

    // First pass: merge overlapping or touching rectangles when doing so adds minimal dead area
    let mut merged_any = true;
    while merged_any {
        merged_any = false;
        'outer: for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                if rects[i].overlaps_or_touches(rects[j], 2) {
                    let a = rects[i];
                    let b = rects[j];
                    let u = a.union(b);
                    let ix0 = a.x.max(b.x);
                    let iy0 = a.y.max(b.y);
                    let ix1 = a.right().min(b.right());
                    let iy1 = a.bottom().min(b.bottom());
                    let inter = ((ix1 - ix0).max(0) as u64) * ((iy1 - iy0).max(0) as u64);
                    let true_union = a.area() + b.area() - inter;
                    let dead_area = u.area().saturating_sub(true_union);
                    if dead_area <= true_union / 5 {
                        rects[i] = u;
                        rects.remove(j);
                        merged_any = true;
                        break 'outer;
                    }
                }
            }
        }
    }

    // Second pass: if still > 4, greedily merge the pair that adds the least dead area
    while rects.len() > 4 {
        let mut best_pair = (0usize, 1usize);
        let mut best_waste = u64::MAX;
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                let u = rects[i].union(rects[j]);
                let waste = u.area().saturating_sub(rects[i].area() + rects[j].area());
                if waste < best_waste {
                    best_waste = waste;
                    best_pair = (i, j);
                }
            }
        }
        let (i, j) = best_pair;
        rects[i] = rects[i].union(rects[j]);
        rects.remove(j);
    }

    let mut out = SmallVec::new();
    for &r in rects.iter() {
        out.push(r);
    }
    out
}

fn origin_guide_bbox_px(og: &OriginGuideVisual, scale: f32) -> RectPx {
    let min_x = og.anchor.x.min(og.center.x) - 4.0;
    let min_y = og.anchor.y.min(og.center.y) - 4.0;
    let max_x = og.anchor.x.max(og.center.x) + 4.0;
    let max_y = og.anchor.y.max(og.center.y) + 4.0;
    let x0 = (min_x * scale).floor() as i32;
    let y0 = (min_y * scale).floor() as i32;
    let x1 = (max_x * scale).ceil() as i32;
    let y1 = (max_y * scale).ceil() as i32;
    RectPx::new(x0, y0, (x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32)
}

fn wheel_bbox_px(w: &WheelVisual, scale: f32) -> RectPx {
    let cx = w.origin.x + (w.center.x - w.origin.x) * w.scale;
    let cy = w.origin.y + (w.center.y - w.origin.y) * w.scale;
    match w.kind {
        WheelVisualKind::Branching => {
            // Transform center by scale about w.origin, and include shadow blur (22 lp) + offset (0, 4 lp) + max pop (4 lp)
            let r = (w.r_out + 6.0) * w.scale + 22.0;
            let min_x = ((cx - r) * scale).floor() as i32;
            let min_y = ((cy - r) * scale).floor() as i32;
            let max_x = ((cx + r) * scale).ceil() as i32;
            let max_y = ((cy + r + 5.0) * scale).ceil() as i32;
            RectPx::new(
                min_x,
                min_y,
                (max_x - min_x).max(0) as u32,
                (max_y - min_y).max(0) as u32,
            )
        }
        WheelVisualKind::ConcentricRoot | WheelVisualKind::ConcentricRing => {
            // Concentric rings have no blurred full-disc shadow; bound tightly to r_out + rim marker + AA pad
            let r = w.r_out * w.scale + 8.0;
            let min_x = ((cx - r) * scale).floor() as i32;
            let min_y = ((cy - r) * scale).floor() as i32;
            let max_x = ((cx + r) * scale).ceil() as i32;
            let max_y = ((cy + r) * scale).ceil() as i32;
            RectPx::new(
                min_x,
                min_y,
                (max_x - min_x).max(0) as u32,
                (max_y - min_y).max(0) as u32,
            )
        }
    }
}

fn segment_bbox_px(
    w: &WheelVisual,
    s_idx: usize,
    seg: &radivine_wheel::SegmentVisual,
    scale: f32,
) -> RectPx {
    match w.kind {
        WheelVisualKind::Branching => segment_bbox_px_branching(w, seg, scale),
        WheelVisualKind::ConcentricRoot | WheelVisualKind::ConcentricRing => {
            segment_bbox_px_concentric(w, s_idx, seg, scale)
        }
    }
}

fn segment_bbox_px_branching(
    w: &WheelVisual,
    seg: &radivine_wheel::SegmentVisual,
    scale: f32,
) -> RectPx {
    let cx = w.origin.x + (w.center.x - w.origin.x) * w.scale;
    let cy = w.origin.y + (w.center.y - w.origin.y) * w.scale;
    let r_in = (w.r_in - 3.0).max(0.0) * w.scale;
    let r_out = (w.r_out + seg.pop + 6.0) * w.scale;

    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;

    // Sample 7 angles across [a0, a1] at both r_in and r_out
    for step in 0..=6 {
        let t = (step as f32) / 6.0;
        let a = seg.a0 + (seg.a1 - seg.a0) * t;
        let (sin_a, cos_a) = a.sin_cos();
        for r in [r_in, r_out] {
            let px = cx + r * cos_a;
            let py = cy + r * sin_a;
            min_x = min_x.min(px);
            min_y = min_y.min(py);
            max_x = max_x.max(px);
            max_y = max_y.max(py);
        }
    }

    let x0 = ((min_x - 4.0) * scale).floor() as i32;
    let y0 = ((min_y - 4.0) * scale).floor() as i32;
    let x1 = ((max_x + 4.0) * scale).ceil() as i32;
    let y1 = ((max_y + 4.0) * scale).ceil() as i32;
    RectPx::new(x0, y0, (x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32)
}

/// Conservative analytic bounding box for a concentric sector (`ARCHITECTURE.md` §A4, `T-REN3`).
///
/// Evaluates the exact angular extrema (`a0`, `a1`, and any cardinal angles `k * π/2` inside
/// `[a0, a1]`) at both `r_in` and `r_out`, unions the precomputed `NestedSegmentContent.rect`
/// and submenu rim marker, and treats `n == 1` as a full disc.
fn segment_bbox_px_concentric(
    w: &WheelVisual,
    s_idx: usize,
    seg: &radivine_wheel::SegmentVisual,
    scale: f32,
) -> RectPx {
    let cx = w.center.x;
    let cy = w.center.y;
    let r_in = (w.r_in - 3.0).max(0.0);
    let r_out = w.r_out + 6.0;

    if w.segments.len() <= 1 || (seg.a1 - seg.a0) >= std::f32::consts::TAU - 1e-3 {
        let pad = r_out + 4.0;
        let x0 = ((cx - pad) * scale).floor() as i32;
        let y0 = ((cy - pad) * scale).floor() as i32;
        let x1 = ((cx + pad) * scale).ceil() as i32;
        let y1 = ((cy + pad) * scale).ceil() as i32;
        return RectPx::new(x0, y0, (x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32);
    }

    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;

    let mut include_angle = |a: f32| {
        let (sin_a, cos_a) = a.sin_cos();
        for r in [r_in, r_out] {
            let px = cx + r * cos_a;
            let py = cy + r * sin_a;
            min_x = min_x.min(px);
            min_y = min_y.min(py);
            max_x = max_x.max(px);
            max_y = max_y.max(py);
        }
    };

    include_angle(seg.a0);
    include_angle(seg.a1);
    include_angle((seg.a0 + seg.a1) * 0.5);

    // Include all cardinal angles `k * (π / 2)` that fall inside `[seg.a0, seg.a1]`
    let k_start = (seg.a0 / FRAC_PI_2).floor() as i32 - 1;
    let k_end = (seg.a1 / FRAC_PI_2).ceil() as i32 + 1;
    for k in k_start..=k_end {
        let card = (k as f32) * FRAC_PI_2;
        if card >= seg.a0 && card <= seg.a1 {
            include_angle(card);
        }
    }

    // Union the N2-supplied sector content box if present
    if let Some(nc) = w.nested_content.as_ref().and_then(|list| list.get(s_idx)) {
        min_x = min_x.min(nc.rect.x - 2.0);
        min_y = min_y.min(nc.rect.y - 2.0);
        max_x = max_x.max(nc.rect.right() + 2.0);
        max_y = max_y.max(nc.rect.bottom() + 2.0);
    }

    let x0 = ((min_x - 4.0) * scale).floor() as i32;
    let y0 = ((min_y - 4.0) * scale).floor() as i32;
    let x1 = ((max_x + 4.0) * scale).ceil() as i32;
    let y1 = ((max_y + 4.0) * scale).ceil() as i32;
    RectPx::new(x0, y0, (x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32)
}

fn stem_bbox_px(s: &StemVisual, scale: f32) -> RectPx {
    let min_x = s.p0.x.min(s.ctrl.x).min(s.p1.x) - 6.0;
    let min_y = s.p0.y.min(s.ctrl.y).min(s.p1.y) - 6.0;
    let max_x = s.p0.x.max(s.ctrl.x).max(s.p1.x) + 6.0;
    let max_y = s.p0.y.max(s.ctrl.y).max(s.p1.y) + 6.0;
    let x0 = (min_x * scale).floor() as i32;
    let y0 = (min_y * scale).floor() as i32;
    let x1 = (max_x * scale).ceil() as i32;
    let y1 = (max_y * scale).ceil() as i32;
    RectPx::new(x0, y0, (x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32)
}

fn caption_bbox_px(c: &CaptionVisual, scale: f32) -> RectPx {
    let pad = 6.0;
    let x0 = ((c.box_.x - pad) * scale).floor() as i32;
    let y0 = ((c.box_.y - pad) * scale).floor() as i32;
    let x1 = ((c.box_.right() + pad) * scale).ceil() as i32;
    let y1 = ((c.box_.bottom() + pad) * scale).ceil() as i32;
    RectPx::new(x0, y0, (x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32)
}

#[inline]
fn fbits(v: f32) -> u32 {
    (v + 0.0).to_bits()
}

fn hash_origin_guide(og: &OriginGuideVisual) -> u64 {
    let mut h = DefaultHasher::new();
    fbits(og.anchor.x).hash(&mut h);
    fbits(og.anchor.y).hash(&mut h);
    fbits(og.center.x).hash(&mut h);
    fbits(og.center.y).hash(&mut h);
    fbits(og.opacity).hash(&mut h);
    h.finish()
}

fn hash_wheel_base(w: &WheelVisual) -> u64 {
    let mut h = DefaultHasher::new();
    discriminant(&w.kind).hash(&mut h);
    discriminant(&w.role).hash(&mut h);
    fbits(w.center.x).hash(&mut h);
    fbits(w.center.y).hash(&mut h);
    fbits(w.r_in).hash(&mut h);
    fbits(w.r_out).hash(&mut h);
    fbits(w.scale).hash(&mut h);
    fbits(w.origin.x).hash(&mut h);
    fbits(w.origin.y).hash(&mut h);
    fbits(w.opacity).hash(&mut h);
    w.bridge.map(|(b0, b1)| (fbits(b0), fbits(b1))).hash(&mut h);
    w.separator_half_angle.map(fbits).hash(&mut h);
    w.hub.icon.hash(&mut h);
    w.hub.badge.hash(&mut h);
    w.segments.len().hash(&mut h);
    h.finish()
}

fn hash_segment(w: &WheelVisual, s_idx: usize, s: &radivine_wheel::SegmentVisual) -> u64 {
    let mut h = DefaultHasher::new();
    discriminant(&w.kind).hash(&mut h);
    discriminant(&w.role).hash(&mut h);
    fbits(w.center.x).hash(&mut h);
    fbits(w.center.y).hash(&mut h);
    fbits(w.r_in).hash(&mut h);
    fbits(w.r_out).hash(&mut h);
    fbits(w.scale).hash(&mut h);
    fbits(w.opacity).hash(&mut h);
    w.separator_half_angle.map(fbits).hash(&mut h);
    if let Some(nc) = w.nested_content.as_ref().and_then(|list| list.get(s_idx)) {
        fbits(nc.rect.x).hash(&mut h);
        fbits(nc.rect.y).hash(&mut h);
        fbits(nc.rect.w).hash(&mut h);
        fbits(nc.rect.h).hash(&mut h);
        discriminant(&nc.mode).hash(&mut h);
    }
    fbits(s.a0).hash(&mut h);
    fbits(s.a1).hash(&mut h);
    fbits(s.highlight).hash(&mut h);
    fbits(s.pop).hash(&mut h);
    discriminant(&s.selection).hash(&mut h);
    s.on_path.hash(&mut h);
    s.enabled.hash(&mut h);
    s.has_children.hash(&mut h);
    s.label.hash(&mut h);
    s.icon.hash(&mut h);
    h.finish()
}

fn hash_stem(s: &StemVisual) -> u64 {
    let mut h = DefaultHasher::new();
    fbits(s.p0.x).hash(&mut h);
    fbits(s.p0.y).hash(&mut h);
    fbits(s.ctrl.x).hash(&mut h);
    fbits(s.ctrl.y).hash(&mut h);
    fbits(s.p1.x).hash(&mut h);
    fbits(s.p1.y).hash(&mut h);
    fbits(s.progress).hash(&mut h);
    fbits(s.opacity).hash(&mut h);
    s.on_path.hash(&mut h);
    h.finish()
}

fn hash_caption(c: &CaptionVisual) -> u64 {
    let mut h = DefaultHasher::new();
    fbits(c.box_.x).hash(&mut h);
    fbits(c.box_.y).hash(&mut h);
    fbits(c.box_.w).hash(&mut h);
    fbits(c.box_.h).hash(&mut h);
    discriminant(&c.align).hash(&mut h);
    c.breadcrumb.hash(&mut h);
    c.title.hash(&mut h);
    c.detail.hash(&mut h);
    c.disabled.hash(&mut h);
    fbits(c.opacity).hash(&mut h);
    h.finish()
}
