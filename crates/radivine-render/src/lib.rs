//! `radivine-render` — Software renderer for Radivine radial wheels (`WHEEL_ENGINE_DESIGN.md` §13).
//!
//! Renders a [`radivine_wheel::Scene`] into a premultiplied RGBA8 framebuffer using `tiny-skia`,
//! `cosmic-text`, and `resvg`, with cached alpha masks for labels, icons, and shadow sprites,
//! plus incremental damage tracking (`<= 4` rectangles per frame).

#![forbid(unsafe_code)]

pub mod cache;
pub mod damage;
pub mod theme;

use radivine_wheel::concentric::{
    ContentMode, compute_sector_content, separator_half_angle_for_ring,
};
use radivine_wheel::geometry::{
    SegmentSpan, gap_inset_angle, segment_content_radius, segment_label_max_width,
};
use radivine_wheel::{
    CaptionVisual, OriginGuideVisual, Point, Scene, SegmentVisual, SelectionVisual, StemVisual,
    WheelVisual, WheelVisualKind,
};
use tiny_skia::{FillRule, LineCap, LineJoin, Mask, Paint, PathBuilder, Pixmap, Stroke, Transform};

use cache::BreadcrumbRun;
pub use cache::{AlphaMask, AssetSource, RenderError};
pub use damage::{DamageTracker, RectPx, SizePx};
pub use theme::{ColorRgba, Theme};

/// Borrowed reference to the rendered premultiplied RGBA8 framebuffer (`WHEEL_ENGINE_DESIGN.md` §13.1).
#[derive(Clone, Copy, Debug)]
pub struct FrameRef<'a> {
    /// Premultiplied RGBA8 pixel bytes (`height * stride` bytes).
    pub data: &'a [u8],
    /// Surface width in physical pixels.
    pub width: u32,
    /// Surface height in physical pixels.
    pub height: u32,
    /// Row stride in bytes (`width * 4`).
    pub stride: usize,
}

/// Software renderer for [`Scene`] (`WHEEL_ENGINE_DESIGN.md` §13.1).
pub struct Renderer {
    theme: Theme,
    cache: cache::AssetCache,
    damage: DamageTracker,
    dirty_union: RectPx,
    pixmap: Option<Pixmap>,
    clip_mask: Option<Mask>,
    size_px: SizePx,
    scale: f32,
    opaque_fallback: bool,
}

impl Renderer {
    /// Creates a new [`Renderer`] with the given [`Theme`] and [`AssetSource`].
    pub fn new(theme: Theme, assets: AssetSource) -> Result<Self, RenderError> {
        let cache = cache::AssetCache::new(assets)?;
        Ok(Self {
            theme,
            cache,
            damage: DamageTracker::new(),
            dirty_union: RectPx::default(),
            pixmap: None,
            clip_mask: None,
            size_px: SizePx::new(0, 0),
            scale: 1.0,
            opaque_fallback: false,
        })
    }

    /// Enables or disables the no-compositor opaque card rendering fallback (`PLATFORM_INTEGRATION.md` §3.11).
    pub fn set_opaque_fallback(&mut self, opaque: bool) {
        if self.opaque_fallback != opaque {
            self.opaque_fallback = opaque;
            self.damage.invalidate_all();
        }
    }

    /// Updates the active [`Theme`] and invalidates damage so the next frame redraws with the new palette.
    pub fn set_theme(&mut self, theme: Theme) {
        if self.theme != theme {
            self.theme = theme;
            self.damage.invalidate_all();
        }
    }

    /// Returns a reference to the current [`Theme`].
    #[inline]
    #[must_use]
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Allocates or resizes the backing `tiny_skia::Pixmap` for a session covering `size_px` at `scale`.
    pub fn begin_session(&mut self, size_px: SizePx, scale: f32) {
        let w = size_px.w.max(1);
        let h = size_px.h.max(1);
        self.size_px = SizePx::new(w, h);
        self.scale = scale.clamp(0.5, 4.0);
        self.damage.reset();
        self.cache.clear_session_labels();

        let needs_alloc = self
            .pixmap
            .as_ref()
            .is_none_or(|pm| pm.width() != w || pm.height() != h);
        if needs_alloc {
            self.pixmap = Pixmap::new(w, h);
            self.clip_mask = Mask::new(w, h);
            self.dirty_union = RectPx::default();
        } else if !self.dirty_union.is_empty() {
            if let Some(ref mut pm) = self.pixmap {
                fill_rect_px(pm, self.dirty_union, [0, 0, 0, 0]);
            }
            self.dirty_union = RectPx::default();
        }
    }

    /// Renders `scene` into the backing pixmap and returns the slice of damaged physical rectangles (`<= 4`).
    pub fn render(&mut self, scene: &Scene) -> &[RectPx] {
        if self.pixmap.is_none() {
            self.begin_session(SizePx::new(1920, 1080), 1.0);
        }

        let size = self.size_px;
        let scale = self.scale;
        let dmg_slice = self.damage.compute_damage(scene, size, scale);
        if dmg_slice.is_empty() {
            return self.damage.last_damage();
        }

        // Partition the <= 4 damage rectangles into mutually disjoint clip rectangles so
        // overlapping damage regions never clear or alpha-blend the same pixel twice.
        let mut clip_rects: smallvec::SmallVec<[RectPx; 16]> = smallvec::SmallVec::new();
        let mut next_clips: smallvec::SmallVec<[RectPx; 16]> = smallvec::SmallVec::new();
        for &r in dmg_slice {
            if r.is_empty() {
                continue;
            }
            next_clips.clear();
            for &existing in &clip_rects {
                subtract_rect_px(existing, r, &mut next_clips);
            }
            next_clips.push(r);
            std::mem::swap(&mut clip_rects, &mut next_clips);
        }

        // 1. Clear damaged rectangles to transparent (or opaque theme card background in no-compositor fallback)
        // and populate clip_mask (§13.2 step 1, §13.3)
        let bg_pixel = if self.opaque_fallback {
            [
                self.theme.ring_fill.r,
                self.theme.ring_fill.g,
                self.theme.ring_fill.b,
                0xFF,
            ]
        } else {
            [0, 0, 0, 0]
        };
        if let Some(ref mut pm) = self.pixmap {
            for r in &clip_rects {
                fill_rect_px(pm, *r, bg_pixel);
                self.dirty_union = self.dirty_union.union(*r);
            }
        }
        let needs_skia_clip = !scene.stems.is_empty()
            || scene
                .wheels
                .iter()
                .any(|w| w.kind == WheelVisualKind::Branching)
            || scene
                .caption
                .as_ref()
                .is_some_and(|c| c.breadcrumb.is_none() && c.box_.h <= 48.0);
        if needs_skia_clip && let Some(ref mut cm) = self.clip_mask {
            for r in &clip_rects {
                fill_mask_rect(cm, *r, 255);
            }
        }

        // 1b. Draw displaced-origin guide below concentric rings (`ARCHITECTURE.md` §A4)
        if let Some(ref guide) = scene.origin_guide {
            self.draw_origin_guide(guide, &clip_rects);
        }

        // 2. Draw stems (§13.2 step 2)
        for stem in &scene.stems {
            self.draw_stem(stem, &clip_rects);
        }

        // 3. Draw wheels from root to deepest (§13.2 step 3)
        for wheel in &scene.wheels {
            self.draw_wheel(wheel, &clip_rects);
        }

        // 4. Draw caption pill (§13.2 step 4)
        if let Some(ref caption) = scene.caption {
            self.draw_caption(caption, &clip_rects);
        }

        // Reset clip_mask regions back to 0 in O(damage_area)
        if needs_skia_clip && let Some(ref mut cm) = self.clip_mask {
            for r in &clip_rects {
                fill_mask_rect(cm, *r, 0);
            }
        }

        self.damage.last_damage()
    }

    /// Returns a [`FrameRef`] to the rendered premultiplied RGBA8 pixel buffer.
    #[must_use]
    pub fn frame(&self) -> FrameRef<'_> {
        match self.pixmap.as_ref() {
            Some(pm) => FrameRef {
                data: pm.data(),
                width: pm.width(),
                height: pm.height(),
                stride: (pm.width() as usize) * 4,
            },
            None => FrameRef {
                data: &[],
                width: 0,
                height: 0,
                stride: 0,
            },
        }
    }

    /// Mutable access to the underlying `tiny_skia::Pixmap` (used by `wheel-sandbox` for debug overlays).
    pub fn pixmap_mut(&mut self) -> Option<&mut Pixmap> {
        self.pixmap.as_mut()
    }

    /// Ends the current rendering session.
    pub fn end_session(&mut self) {
        if !self.dirty_union.is_empty() {
            if let Some(ref mut pm) = self.pixmap {
                fill_rect_px(pm, self.dirty_union, [0, 0, 0, 0]);
            }
            self.dirty_union = RectPx::default();
        }
        self.damage.reset();
    }

    /// Frees the backing `Pixmap` and `Mask` framebuffers when the daemon has been idle (`PLATFORM_INTEGRATION.md` §3.4).
    pub fn release_buffers(&mut self) {
        self.pixmap = None;
        self.clip_mask = None;
        self.dirty_union = RectPx::default();
        self.size_px = SizePx::new(0, 0);
        self.damage.reset();
    }

    // --- Internal drawing pipeline ---

    fn draw_origin_guide(&mut self, guide: &OriginGuideVisual, clips: &[RectPx]) {
        if guide.opacity <= 1e-3 || guide.anchor.distance(guide.center) <= 0.5 {
            return;
        }
        let scale = self.scale;
        let pad = (4.0 * scale).ceil() as i32;
        let x0 = (guide.anchor.x.min(guide.center.x) * scale).floor() as i32 - pad;
        let y0 = (guide.anchor.y.min(guide.center.y) * scale).floor() as i32 - pad;
        let x1 = (guide.anchor.x.max(guide.center.x) * scale).ceil() as i32 + pad;
        let y1 = (guide.anchor.y.max(guide.center.y) * scale).ceil() as i32 + pad;
        let g_rect = RectPx::new(x0, y0, (x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32);
        if !clips.iter().any(|c| c.overlaps_or_touches(g_rect, 0)) {
            return;
        }
        let col = self.theme.stem_on_path.with_opacity(guide.opacity);
        let (gx, gy, ref mask) = *self.cache.guide_mask(
            guide.anchor.x,
            guide.anchor.y,
            guide.center.x,
            guide.center.y,
            scale,
        );
        if let Some(ref mut pm) = self.pixmap {
            blit_alpha_mask_clipped(pm, mask, gx, gy, col, clips);
        }
    }

    fn draw_stem(&mut self, stem: &StemVisual, clips: &[RectPx]) {
        if stem.progress <= 1e-3 || stem.opacity <= 1e-3 {
            return;
        }
        let Some(ref mut pm) = self.pixmap else {
            return;
        };

        let scale = self.scale;
        let t = stem.progress.clamp(0.0, 1.0);
        // de Casteljau subdivision of quadratic Bézier (p0, ctrl, p1) for exact trim [0, t]
        let q0 = stem.p0;
        let q1 = stem.p0.lerp(stem.ctrl, t);
        let c_mid = stem.ctrl.lerp(stem.p1, t);
        let q2 = q1.lerp(c_mid, t);

        let mut pb = PathBuilder::new();
        pb.move_to(q0.x * scale, q0.y * scale);
        pb.quad_to(q1.x * scale, q1.y * scale, q2.x * scale, q2.y * scale);
        let Some(path) = pb.finish() else {
            return;
        };

        let base_col = if stem.on_path {
            self.theme.stem_on_path
        } else {
            self.theme.stem
        };
        let col = base_col.with_opacity(stem.opacity);
        let mut paint = Paint::default();
        paint.set_color(col.to_skia());
        paint.anti_alias = true;

        let stroke = Stroke {
            width: (3.0 * scale).max(1.0),
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };

        let _ = clips;
        pm.stroke_path(
            &path,
            &paint,
            &stroke,
            Transform::identity(),
            self.clip_mask.as_ref(),
        );
    }

    fn draw_wheel(&mut self, wheel: &WheelVisual, clips: &[RectPx]) {
        if wheel.opacity <= 1e-3 || wheel.scale <= 1e-3 {
            return;
        }
        let scale = self.scale;
        let ws = wheel.scale;
        let cx_lp = wheel.origin.x + (wheel.center.x - wheel.origin.x) * ws;
        let cy_lp = wheel.origin.y + (wheel.center.y - wheel.origin.y) * ws;
        let cx = cx_lp * scale;
        let cy = cy_lp * scale;
        let r_in = wheel.r_in * ws * scale;
        let r_out = wheel.r_out * ws * scale;

        // Skip wheel entirely if its outer bbox doesn't intersect any clip rect
        let max_r = (r_out + 28.0 * scale).ceil() as i32;
        let w_rect = RectPx::new(
            (cx as i32) - max_r,
            (cy as i32) - max_r,
            (max_r * 2).max(0) as u32,
            (max_r * 2 + 8).max(0) as u32,
        );
        if !clips.iter().any(|c| c.overlaps_or_touches(w_rect, 0)) {
            return;
        }

        match wheel.kind {
            WheelVisualKind::Branching => {
                // 1. Soft shadow sprite (pre-blurred disc per radius, cached, offset (0, 4 lp); skipped in no-compositor fallback)
                let r_out_px = r_out.round().max(8.0) as u32;
                let blur_px = (18.0 * scale).round().max(4.0) as u32;
                let shadow_alpha = if self.opaque_fallback {
                    0.0
                } else {
                    (self.theme.shadow_alpha * wheel.opacity).clamp(0.0, 1.0)
                };
                if shadow_alpha > 1e-3 {
                    let shadow_col = ColorRgba::new(0, 0, 0, (shadow_alpha * 255.0).round() as u8);
                    let sprite = self.cache.shadow_sprite(r_out_px, blur_px);
                    let sx = (cx - (sprite.width as f32) * 0.5).round() as i32;
                    let sy = (cy + 4.0 * scale - (sprite.height as f32) * 0.5).round() as i32;
                    if let Some(ref mut pm) = self.pixmap {
                        blit_alpha_mask_clipped(pm, sprite, sx, sy, shadow_col, clips);
                    }
                }

                // 2. Child wheel neutral ring / bridge backdrop
                if let Some((b0, b1)) = wheel.bridge
                    && let Some(ref mut pm) = self.pixmap
                {
                    if let Some(ring_path) = build_annulus_path(cx, cy, r_in, r_out) {
                        let mut fill_paint = Paint::default();
                        fill_paint.set_color(
                            self.theme
                                .ring_fill
                                .with_opacity(wheel.opacity * 0.72)
                                .to_skia(),
                        );
                        fill_paint.anti_alias = true;
                        pm.fill_path(
                            &ring_path,
                            &fill_paint,
                            FillRule::EvenOdd,
                            Transform::identity(),
                            self.clip_mask.as_ref(),
                        );
                    }
                    let b_mid = (b0 + b1) * 0.5;
                    let b_half = ((b1 - b0) * 0.32).min(0.45);
                    if let Some(notch_path) =
                        build_arc_path(cx, cy, r_out - 2.5 * scale, b_mid - b_half, b_mid + b_half)
                    {
                        let mut p = Paint::default();
                        p.set_color(
                            self.theme
                                .stem_on_path
                                .with_opacity(wheel.opacity * 0.55)
                                .to_skia(),
                        );
                        p.anti_alias = true;
                        let st = Stroke {
                            width: (2.0 * scale).max(1.0),
                            line_cap: LineCap::Round,
                            ..Stroke::default()
                        };
                        pm.stroke_path(
                            &notch_path,
                            &p,
                            &st,
                            Transform::identity(),
                            self.clip_mask.as_ref(),
                        );
                    }
                }

                // 3. Segments (annular sector with constant linear gap inset, highlight, pop, rim stroke)
                for seg in &wheel.segments {
                    self.draw_segment(wheel, seg, cx, cy, r_in, r_out, clips);
                }

                // 4. Center hub (vine green disc + 1.5 lp inner ring at 70% radius + icon + optional count badge)
                self.draw_hub(wheel, cx, cy, r_in, clips);
            }
            WheelVisualKind::ConcentricRoot => {
                // ConcentricRoot draws level-0 annular sectors and the shared central hub,
                // without a blurred full-disc shadow (`ARCHITECTURE.md` §A4, `INTERACTION_DESIGN.md` §I8).
                for (seg_idx, seg) in wheel.segments.iter().enumerate() {
                    self.draw_concentric_segment(wheel, seg_idx, seg, cx, cy, r_in, r_out, clips);
                }
                self.draw_hub(wheel, cx, cy, r_in, clips);
            }
            WheelVisualKind::ConcentricRing => {
                // ConcentricRing draws annular sectors, separators, rim markers, and sector content only —
                // never an inner hub disc, never a full-disc drop shadow, and never a bridge notch.
                for (seg_idx, seg) in wheel.segments.iter().enumerate() {
                    self.draw_concentric_segment(wheel, seg_idx, seg, cx, cy, r_in, r_out, clips);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_segment(
        &mut self,
        wheel: &WheelVisual,
        seg: &SegmentVisual,
        cx: f32,
        cy: f32,
        r_in: f32,
        r_out_base: f32,
        clips: &[RectPx],
    ) {
        let scale = self.scale;
        let ws = wheel.scale;
        let r_out = r_out_base + seg.pop * ws * scale;

        // Constant linear gap = 2.5 lp evaluated at logical r_in and r_out + pop
        let d_in = gap_inset_angle(2.5, wheel.r_in);
        let d_out = gap_inset_angle(2.5, wheel.r_out + seg.pop);

        let a0_in = seg.a0 + d_in;
        let a1_in = (seg.a1 - d_in).max(a0_in + 0.01);
        let a0_out = seg.a0 + d_out;
        let a1_out = (seg.a1 - d_out).max(a0_out + 0.01);

        let Some(sector_path) =
            build_sector_path(cx, cy, r_in, r_out, a0_in, a1_in, a0_out, a1_out)
        else {
            return;
        };

        let base_fill = if seg.on_path {
            self.theme.segment_on_path
        } else {
            self.theme.ring_fill
        };
        let hl_t = seg.highlight.clamp(0.0, 1.0);
        let mut seg_col = base_fill.lerp(self.theme.segment_highlight, hl_t);
        if seg.highlight > 1.0 {
            seg_col = seg_col.brighten((seg.highlight - 1.0) * 1.4);
        }
        seg_col = seg_col.with_opacity(wheel.opacity);

        let stroke_col = if seg.highlight > 0.3 || seg.on_path {
            self.theme
                .segment_highlight
                .lerp(self.theme.ring_stroke, 0.35)
                .with_opacity(wheel.opacity)
        } else {
            self.theme.ring_stroke.with_opacity(wheel.opacity)
        };

        if let Some(ref mut pm) = self.pixmap {
            let mut fill_paint = Paint::default();
            fill_paint.set_color(seg_col.to_skia());
            fill_paint.anti_alias = true;
            pm.fill_path(
                &sector_path,
                &fill_paint,
                FillRule::Winding,
                Transform::identity(),
                self.clip_mask.as_ref(),
            );

            let mut stroke_paint = Paint::default();
            stroke_paint.set_color(stroke_col.to_skia());
            stroke_paint.anti_alias = true;
            let stroke = Stroke {
                width: (1.0 * scale).max(1.0),
                line_join: LineJoin::Round,
                ..Stroke::default()
            };
            pm.stroke_path(
                &sector_path,
                &stroke_paint,
                &stroke,
                Transform::identity(),
                self.clip_mask.as_ref(),
            );
        }

        let phi = (seg.a0 + seg.a1) * 0.5;
        let (sin_p, cos_p) = phi.sin_cos();

        // Submenu node indicator: hollow circle (radius 3.5 lp, stroke 1.5 lp) centered on outer rim (r_out)
        if seg.has_children
            && let Some(ref mut pm) = self.pixmap
        {
            let nx = cx + r_out * cos_p;
            let ny = cy + r_out * sin_p;
            let node_r = (3.5 * ws * scale).max(2.0);
            if let Some(circle) = PathBuilder::from_circle(nx, ny, node_r) {
                let mut inner_paint = Paint::default();
                inner_paint.set_color(self.theme.ring_fill.with_opacity(wheel.opacity).to_skia());
                inner_paint.anti_alias = true;
                pm.fill_path(
                    &circle,
                    &inner_paint,
                    FillRule::Winding,
                    Transform::identity(),
                    self.clip_mask.as_ref(),
                );

                let node_stroke_col = if !seg.enabled {
                    self.theme.text_disabled
                } else if seg.highlight >= 0.55 {
                    self.theme.text_on_accent
                } else {
                    self.theme.segment_highlight
                }
                .with_opacity(wheel.opacity);

                let mut sp = Paint::default();
                sp.set_color(node_stroke_col.to_skia());
                sp.anti_alias = true;
                let st = Stroke {
                    width: (1.5 * ws * scale).max(1.0),
                    ..Stroke::default()
                };
                pm.stroke_path(
                    &circle,
                    &sp,
                    &st,
                    Transform::identity(),
                    self.clip_mask.as_ref(),
                );
            }
        }

        // Content block: icon stacked above horizontal label (gap 3 lp) at r_c = r_in + 0.55*(r_out - r_in)
        let r_c = segment_content_radius(r_in, r_out);
        let bx = cx + r_c * cos_p;
        let by = cy + r_c * sin_p;

        let content_col = if !seg.enabled {
            self.theme.text_disabled.with_opacity(0.68 * wheel.opacity)
        } else if seg.highlight >= 0.55 {
            self.theme.text_on_accent.with_opacity(wheel.opacity)
        } else {
            self.theme.text_primary.with_opacity(wheel.opacity)
        };

        let icon_lp = if wheel.r_out >= 110.0 {
            22.0
        } else if wheel.r_out >= 80.0 {
            20.0
        } else {
            18.0
        };
        let label_lp = if wheel.r_out >= 90.0 { 12.0 } else { 11.0 };
        let icon_px = (icon_lp * ws * scale).round().max(10.0) as u32;
        let label_font_px = (label_lp * ws * scale).max(8.0);
        let max_label_w_lp = segment_label_max_width(wheel.r_in, wheel.r_out, seg.a1 - seg.a0, 2.5);
        let max_label_w_px = (max_label_w_lp * ws * scale).round().max(24.0) as u32;

        let gap_px = (3.0 * ws * scale).round() as i32;

        let (label_w, label_h) =
            self.cache
                .text_dims(&seg.label, label_font_px, 500, max_label_w_px);

        if let Some(ref icon_key) = seg.icon {
            let (icon_w, icon_h) = self.cache.icon_dims(icon_key.as_str(), icon_px);
            let total_h = (icon_h as i32) + gap_px + (label_h as i32);
            let top_y = (by.round() as i32) - total_h / 2;

            let ix = (bx.round() as i32) - (icon_w as i32) / 2;
            let iy = top_y;
            let lx = (bx.round() as i32) - (label_w as i32) / 2;
            let ly = top_y + (icon_h as i32) + gap_px;

            {
                let icon_mask = self.cache.icon_mask(icon_key.as_str(), icon_px);
                if let Some(ref mut pm) = self.pixmap {
                    blit_alpha_mask_clipped(pm, icon_mask, ix, iy, content_col, clips);
                }
            }
            {
                let label_mask =
                    self.cache
                        .text_mask(&seg.label, label_font_px, 500, max_label_w_px);
                if let Some(ref mut pm) = self.pixmap {
                    blit_alpha_mask_clipped(pm, label_mask, lx, ly, content_col, clips);
                }
            }
        } else {
            let lx = (bx.round() as i32) - (label_w as i32) / 2;
            let ly = (by.round() as i32) - (label_h as i32) / 2;
            let label_mask = self
                .cache
                .text_mask(&seg.label, label_font_px, 500, max_label_w_px);
            if let Some(ref mut pm) = self.pixmap {
                blit_alpha_mask_clipped(pm, label_mask, lx, ly, content_col, clips);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_concentric_segment(
        &mut self,
        wheel: &WheelVisual,
        seg_idx: usize,
        seg: &SegmentVisual,
        cx: f32,
        cy: f32,
        r_in: f32,
        r_out: f32,
        clips: &[RectPx],
    ) {
        let scale = self.scale;
        let count = wheel.segments.len();

        // Fast conservative clip check so segments outside damage rectangles skip tiny-skia path work
        if count > 1 {
            let r_lo = (r_in - 4.0 * scale).max(0.0);
            let r_hi = r_out + 8.0 * scale;
            let mut min_x = f32::INFINITY;
            let mut min_y = f32::INFINITY;
            let mut max_x = f32::NEG_INFINITY;
            let mut max_y = f32::NEG_INFINITY;
            let mut include_angle = |a: f32| {
                let (s, c) = a.sin_cos();
                for r in [r_lo, r_hi] {
                    let x = cx + r * c;
                    let y = cy + r * s;
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            };
            include_angle(seg.a0);
            include_angle(seg.a1);
            include_angle((seg.a0 + seg.a1) * 0.5);
            let half_pi = std::f32::consts::FRAC_PI_2;
            let k_start = (seg.a0 / half_pi).ceil() as i32;
            let k_end = (seg.a1 / half_pi).floor() as i32;
            for k in k_start..=k_end {
                include_angle((k as f32) * half_pi);
            }
            if let Some(ref nc_list) = wheel.nested_content
                && let Some(nc) = nc_list.get(seg_idx)
            {
                min_x = min_x.min(nc.rect.x * scale);
                min_y = min_y.min(nc.rect.y * scale);
                max_x = max_x.max((nc.rect.x + nc.rect.w) * scale);
                max_y = max_y.max((nc.rect.y + nc.rect.h) * scale);
            }
            let pad = (4.0 * scale).ceil() as i32;
            let x0 = (min_x.floor() as i32) - pad;
            let y0 = (min_y.floor() as i32) - pad;
            let x1 = (max_x.ceil() as i32) + pad;
            let y1 = (max_y.ceil() as i32) + pad;
            let seg_rect = RectPx::new(x0, y0, (x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32);
            if !clips.iter().any(|c| c.overlaps_or_touches(seg_rect, 0)) {
                return;
            }
        }

        let sep_half = wheel
            .separator_half_angle
            .unwrap_or_else(|| separator_half_angle_for_ring(wheel.r_in, count));

        let base_fill = if seg.on_path {
            self.theme.segment_on_path
        } else {
            self.theme.ring_fill
        };
        let hl_t = seg.highlight.clamp(0.0, 1.0);
        let mut seg_col = base_fill.lerp(self.theme.segment_highlight, hl_t);
        if seg.highlight > 1.0 {
            seg_col = seg_col.brighten((seg.highlight - 1.0) * 1.4);
        }
        seg_col = seg_col.with_opacity(wheel.opacity);

        let stroke_col = if seg.highlight > 0.3 || seg.on_path {
            self.theme
                .segment_highlight
                .lerp(self.theme.ring_stroke, 0.35)
                .with_opacity(wheel.opacity)
        } else {
            self.theme.ring_stroke.with_opacity(wheel.opacity)
        };

        let is_outlined = matches!(
            seg.selection,
            SelectionVisual::Armed | SelectionVisual::KeyboardFocus
        );
        let outline_col = self.theme.nested_outline.with_opacity(wheel.opacity);

        let node_stroke_col = if !seg.enabled {
            self.theme.text_disabled
        } else if seg.highlight >= 0.55 {
            self.theme.text_on_accent
        } else {
            self.theme.segment_highlight
        }
        .with_opacity(wheel.opacity);

        let inner_col = if seg.on_path {
            node_stroke_col
        } else {
            self.theme.ring_fill.with_opacity(wheel.opacity)
        };

        let stroke_rim_col = if seg.on_path {
            self.theme.nested_outline.with_opacity(wheel.opacity * 0.9)
        } else {
            node_stroke_col
        };

        {
            let _ = (r_in, r_out);
            let masks = self.cache.concentric_sector_masks(
                cx,
                cy,
                wheel.r_in,
                wheel.r_out,
                seg.a0,
                seg.a1,
                sep_half,
                count,
                scale,
            );
            if let Some(ref mut pm) = self.pixmap {
                blit_alpha_mask_clipped(pm, &masks.fill, masks.x, masks.y, seg_col, clips);
                blit_alpha_mask_clipped(pm, &masks.stroke, masks.x, masks.y, stroke_col, clips);
                if is_outlined {
                    blit_alpha_mask_clipped(
                        pm,
                        &masks.outline,
                        masks.x,
                        masks.y,
                        outline_col,
                        clips,
                    );
                }
                if seg.has_children {
                    blit_alpha_mask_clipped(
                        pm,
                        &masks.marker_fill,
                        masks.marker_x,
                        masks.marker_y,
                        inner_col,
                        clips,
                    );
                    blit_alpha_mask_clipped(
                        pm,
                        &masks.marker_stroke,
                        masks.marker_x,
                        masks.marker_y,
                        stroke_rim_col,
                        clips,
                    );
                }
            }
        }

        let phi = (seg.a0 + seg.a1) * 0.5;

        // Sector content placement inside N2-supplied `NestedSegmentContent` box (`INTERACTION_DESIGN.md` §I8):
        // Always 20 lp icons and 12 lp labels across all concentric levels.
        let content = wheel
            .nested_content
            .as_ref()
            .and_then(|nc| nc.get(seg_idx).copied())
            .unwrap_or_else(|| {
                compute_sector_content(
                    wheel.center,
                    wheel.r_in,
                    wheel.r_out,
                    SegmentSpan {
                        a0: seg.a0,
                        a1: seg.a1,
                        center: phi,
                    },
                    sep_half,
                    count,
                    seg.icon.is_some(),
                )
            });

        let pad_x = (8.0 * scale).ceil() as i32;
        let pad_y = (16.0 * scale).ceil() as i32;
        let cx0 = (content.rect.x * scale).floor() as i32 - pad_x;
        let cy0 = (content.rect.y * scale).floor() as i32 - pad_y;
        let cx1 = ((content.rect.x + content.rect.w) * scale).ceil() as i32 + pad_x;
        let cy1 = ((content.rect.y + content.rect.h) * scale).ceil() as i32 + pad_y;
        let content_rect_px = RectPx::new(
            cx0,
            cy0,
            (cx1 - cx0).max(0) as u32,
            (cy1 - cy0).max(0) as u32,
        );
        if !clips
            .iter()
            .any(|c| c.overlaps_or_touches(content_rect_px, 0))
        {
            return;
        }

        let content_col = if !seg.enabled {
            self.theme.text_disabled.with_opacity(0.68 * wheel.opacity)
        } else if seg.highlight >= 0.55 {
            self.theme.text_on_accent.with_opacity(wheel.opacity)
        } else {
            self.theme.text_primary.with_opacity(wheel.opacity)
        };

        let box_cx = (content.rect.x + content.rect.w * 0.5) * scale;
        let box_cy = (content.rect.y + content.rect.h * 0.5) * scale;
        let icon_px = (20.0 * scale).round().max(10.0) as u32;
        let label_font_px = (12.0 * scale).max(8.0);
        let max_label_w_px = (content.rect.w * scale).floor().max(16.0) as u32;
        let gap_px = (3.0 * scale).round() as i32;

        match content.mode {
            ContentMode::IconAndLabel => {
                let (label_w, label_h) =
                    self.cache
                        .text_dims(&seg.label, label_font_px, 500, max_label_w_px);
                if let Some(ref icon_key) = seg.icon {
                    let (icon_w, icon_h) = self.cache.icon_dims(icon_key.as_str(), icon_px);
                    let total_h = (icon_h as i32) + gap_px + (label_h as i32);
                    let top_y = (box_cy.round() as i32) - total_h / 2;

                    let ix = (box_cx.round() as i32) - (icon_w as i32) / 2;
                    let iy = top_y;
                    let lx = (box_cx.round() as i32) - (label_w as i32) / 2;
                    let ly = top_y + (icon_h as i32) + gap_px;

                    {
                        let icon_mask = self.cache.icon_mask(icon_key.as_str(), icon_px);
                        if let Some(ref mut pm) = self.pixmap {
                            blit_alpha_mask_clipped(pm, icon_mask, ix, iy, content_col, clips);
                        }
                    }
                    {
                        let label_mask =
                            self.cache
                                .text_mask(&seg.label, label_font_px, 500, max_label_w_px);
                        if let Some(ref mut pm) = self.pixmap {
                            blit_alpha_mask_clipped(pm, label_mask, lx, ly, content_col, clips);
                        }
                    }
                } else {
                    let lx = (box_cx.round() as i32) - (label_w as i32) / 2;
                    let ly = (box_cy.round() as i32) - (label_h as i32) / 2;
                    let label_mask =
                        self.cache
                            .text_mask(&seg.label, label_font_px, 500, max_label_w_px);
                    if let Some(ref mut pm) = self.pixmap {
                        blit_alpha_mask_clipped(pm, label_mask, lx, ly, content_col, clips);
                    }
                }
            }
            ContentMode::IconOnly => {
                if let Some(ref icon_key) = seg.icon {
                    let (icon_w, icon_h) = self.cache.icon_dims(icon_key.as_str(), icon_px);
                    let ix = (box_cx.round() as i32) - (icon_w as i32) / 2;
                    let iy = (box_cy.round() as i32) - (icon_h as i32) / 2;
                    let icon_mask = self.cache.icon_mask(icon_key.as_str(), icon_px);
                    if let Some(ref mut pm) = self.pixmap {
                        blit_alpha_mask_clipped(pm, icon_mask, ix, iy, content_col, clips);
                    }
                } else {
                    let (label_w, label_h) =
                        self.cache
                            .text_dims(&seg.label, label_font_px, 500, max_label_w_px);
                    let lx = (box_cx.round() as i32) - (label_w as i32) / 2;
                    let ly = (box_cy.round() as i32) - (label_h as i32) / 2;
                    let label_mask =
                        self.cache
                            .text_mask(&seg.label, label_font_px, 500, max_label_w_px);
                    if let Some(ref mut pm) = self.pixmap {
                        blit_alpha_mask_clipped(pm, label_mask, lx, ly, content_col, clips);
                    }
                }
            }
            ContentMode::LabelOnly => {
                let (label_w, label_h) =
                    self.cache
                        .text_dims(&seg.label, label_font_px, 500, max_label_w_px);
                let lx = (box_cx.round() as i32) - (label_w as i32) / 2;
                let ly = (box_cy.round() as i32) - (label_h as i32) / 2;
                let label_mask =
                    self.cache
                        .text_mask(&seg.label, label_font_px, 500, max_label_w_px);
                if let Some(ref mut pm) = self.pixmap {
                    blit_alpha_mask_clipped(pm, label_mask, lx, ly, content_col, clips);
                }
            }
        }
    }

    fn draw_hub(&mut self, wheel: &WheelVisual, cx: f32, cy: f32, r_in: f32, clips: &[RectPx]) {
        let scale = self.scale;
        let ws = wheel.scale;
        let hub_pad = (r_in + 14.0 * scale).ceil() as i32;
        let hub_rect = RectPx::new(
            (cx as i32) - hub_pad,
            (cy as i32) - hub_pad,
            (hub_pad * 2).max(0) as u32,
            (hub_pad * 2).max(0) as u32,
        );
        if !clips.iter().any(|c| c.overlaps_or_touches(hub_rect, 0)) {
            return;
        }

        if wheel.kind == WheelVisualKind::Branching {
            if let Some(ref mut pm) = self.pixmap {
                if let Some(hub_circle) = PathBuilder::from_circle(cx, cy, r_in) {
                    let mut fill_paint = Paint::default();
                    fill_paint.set_color(self.theme.hub_fill.with_opacity(wheel.opacity).to_skia());
                    fill_paint.anti_alias = true;
                    pm.fill_path(
                        &hub_circle,
                        &fill_paint,
                        FillRule::Winding,
                        Transform::identity(),
                        self.clip_mask.as_ref(),
                    );

                    let mut border_paint = Paint::default();
                    border_paint
                        .set_color(self.theme.ring_stroke.with_opacity(wheel.opacity).to_skia());
                    border_paint.anti_alias = true;
                    let border_stroke = Stroke {
                        width: (1.2 * scale).max(1.0),
                        ..Stroke::default()
                    };
                    pm.stroke_path(
                        &hub_circle,
                        &border_paint,
                        &border_stroke,
                        Transform::identity(),
                        self.clip_mask.as_ref(),
                    );
                }

                // 1.5 lp #E8F5F0 inner ring at 70% hub radius echoing the Radivine logo's central node (§13.4)
                let inner_r = r_in * 0.70;
                if inner_r >= 4.0
                    && let Some(inner_ring) = PathBuilder::from_circle(cx, cy, inner_r)
                {
                    let mut ring_paint = Paint::default();
                    ring_paint.set_color(
                        self.theme
                            .hub_inner_ring
                            .with_opacity(wheel.opacity)
                            .to_skia(),
                    );
                    ring_paint.anti_alias = true;
                    let ring_stroke = Stroke {
                        width: (1.5 * ws * scale).max(1.0),
                        ..Stroke::default()
                    };
                    pm.stroke_path(
                        &inner_ring,
                        &ring_paint,
                        &ring_stroke,
                        Transform::identity(),
                        self.clip_mask.as_ref(),
                    );
                }
            }
        } else {
            let fill_col = self.theme.hub_fill.with_opacity(wheel.opacity);
            let border_col = self.theme.ring_stroke.with_opacity(wheel.opacity);
            let ring_col = self.theme.hub_inner_ring.with_opacity(wheel.opacity);
            let hm = self.cache.hub_masks(cx, cy, r_in, ws, scale);
            if let Some(ref mut pm) = self.pixmap {
                blit_alpha_mask_clipped(pm, &hm.fill, hm.x, hm.y, fill_col, clips);
                blit_alpha_mask_clipped(pm, &hm.border, hm.x, hm.y, border_col, clips);
                if let Some(ref ir) = hm.inner_ring {
                    blit_alpha_mask_clipped(pm, ir, hm.x, hm.y, ring_col, clips);
                }
            }
        }

        if let Some(ref icon_key) = wheel.hub.icon {
            let icon_lp = if wheel.r_in >= 34.0 { 20.0 } else { 16.0 };
            let icon_px = (icon_lp * ws * scale).round().max(10.0) as u32;
            let mask = self.cache.icon_mask(icon_key.as_str(), icon_px);
            let ix = (cx.round() as i32) - (mask.width as i32) / 2;
            let iy = (cy.round() as i32) - (mask.height as i32) / 2;
            // Hub fill is always #044C3C vine green in both dark and light themes (§13.4),
            // so the hub icon uses #E8F5F0 to maintain high contrast in both themes.
            let col = ColorRgba::rgb(0xE8, 0xF5, 0xF0).with_opacity(wheel.opacity);
            if let Some(ref mut pm) = self.pixmap {
                blit_alpha_mask_clipped(pm, mask, ix, iy, col, clips);
            }
        }

        if let Some(count) = wheel.hub.badge
            && count > 1
        {
            let font_px = (11.0 * ws * scale).max(8.0);
            let (bw, bh) = self.cache.badge_dims(count, font_px);
            let bx = cx + r_in * 0.52;
            let by = cy + r_in * 0.52;
            let pill_r = ((bh.max(bw) as f32) * 0.5 + 4.0 * scale).max(7.0 * scale);
            if wheel.kind == WheelVisualKind::Branching {
                if let Some(ref mut pm) = self.pixmap
                    && let Some(pill) = PathBuilder::from_circle(bx, by, pill_r)
                {
                    let mut p = Paint::default();
                    p.set_color(
                        self.theme
                            .segment_highlight
                            .with_opacity(wheel.opacity)
                            .to_skia(),
                    );
                    p.anti_alias = true;
                    pm.fill_path(
                        &pill,
                        &p,
                        FillRule::Winding,
                        Transform::identity(),
                        self.clip_mask.as_ref(),
                    );
                }
            } else {
                let pill_col = self.theme.segment_highlight.with_opacity(wheel.opacity);
                let (px, py, ref cmask) = *self.cache.circle_mask(bx, by, pill_r);
                if let Some(ref mut pm) = self.pixmap {
                    blit_alpha_mask_clipped(pm, cmask, px, py, pill_col, clips);
                }
            }
            let tx = (bx.round() as i32) - (bw as i32) / 2;
            let ty = (by.round() as i32) - (bh as i32) / 2;
            let mask = self.cache.badge_mask(count, font_px);
            if let Some(ref mut pm) = self.pixmap {
                blit_alpha_mask_clipped(
                    pm,
                    mask,
                    tx,
                    ty,
                    self.theme.text_on_accent.with_opacity(wheel.opacity),
                    clips,
                );
            }
        }
    }

    fn draw_caption(&mut self, caption: &CaptionVisual, clips: &[RectPx]) {
        if caption.opacity <= 1e-3 {
            return;
        }
        let scale = self.scale;
        let pad = (6.0 * scale).ceil() as i32;
        let x0 = (caption.box_.x * scale).floor() as i32 - pad;
        let y0 = (caption.box_.y * scale).floor() as i32 - pad;
        let x1 = ((caption.box_.x + caption.box_.w) * scale).ceil() as i32 + pad;
        let y1 = ((caption.box_.y + caption.box_.h) * scale).ceil() as i32 + pad;
        let cap_rect = RectPx::new(x0, y0, (x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32);
        if !clips.iter().any(|c| c.overlaps_or_touches(cap_rect, 0)) {
            return;
        }
        let is_concentric_caption = caption.breadcrumb.is_some() || caption.box_.h > 48.0;
        if !is_concentric_caption {
            self.draw_caption_branching(caption, clips);
        } else {
            self.draw_caption_concentric(caption, clips);
        }
    }

    fn draw_caption_branching(&mut self, caption: &CaptionVisual, clips: &[RectPx]) {
        let scale = self.scale;
        let max_text_w = ((caption.box_.w - 24.0).max(40.0) * scale).round() as u32;

        let (title_w, title_h) =
            self.cache
                .text_dims(&caption.title, 13.0 * scale, 600, max_text_w);
        let detail_dims = caption
            .detail
            .as_ref()
            .map(|d| self.cache.text_dims(d, 11.0 * scale, 400, max_text_w));

        let pad_x = (14.0 * scale).round() as i32;
        let pad_y = (8.0 * scale).round() as i32;
        let line_gap = (4.0 * scale).round() as i32;

        let content_w = detail_dims.map_or(title_w, |(dw, _)| dw.max(title_w)) as i32;
        let content_h = (title_h as i32) + detail_dims.map_or(0, |(_, dh)| line_gap + (dh as i32));

        let box_x = (caption.box_.x * scale).round() as i32;
        let box_y = (caption.box_.y * scale).round() as i32;
        let box_w = (caption.box_.w * scale).round() as i32;
        let box_h = (caption.box_.h * scale).round() as i32;

        let pill_w = (content_w + pad_x * 2).clamp((88.0 * scale) as i32, box_w);
        let pill_h = (content_h + pad_y * 2).clamp((28.0 * scale) as i32, box_h);
        let pill_x = box_x + (box_w - pill_w) / 2;
        let pill_y = box_y + (box_h - pill_h) / 2;
        let radius = 10.0 * scale;

        if let Some(ref mut pm) = self.pixmap
            && let Some(pill_path) = build_rounded_rect_path(
                pill_x as f32,
                pill_y as f32,
                pill_w as f32,
                pill_h as f32,
                radius,
            )
        {
            let mut fill_p = Paint::default();
            fill_p.set_color(
                self.theme
                    .caption_fill
                    .with_opacity(caption.opacity)
                    .to_skia(),
            );
            fill_p.anti_alias = true;
            pm.fill_path(
                &pill_path,
                &fill_p,
                FillRule::Winding,
                Transform::identity(),
                self.clip_mask.as_ref(),
            );

            let mut stroke_p = Paint::default();
            stroke_p.set_color(
                self.theme
                    .ring_stroke
                    .with_opacity(caption.opacity)
                    .to_skia(),
            );
            stroke_p.anti_alias = true;
            let st = Stroke {
                width: (1.0 * scale).max(1.0),
                ..Stroke::default()
            };
            pm.stroke_path(
                &pill_path,
                &stroke_p,
                &st,
                Transform::identity(),
                self.clip_mask.as_ref(),
            );
        }

        let tx = pill_x + (pill_w - (title_w as i32)) / 2;
        let ty = pill_y + (pill_h - content_h) / 2;
        {
            let title_mask = self
                .cache
                .text_mask(&caption.title, 13.0 * scale, 600, max_text_w);
            if let Some(ref mut pm) = self.pixmap {
                blit_alpha_mask_clipped(
                    pm,
                    title_mask,
                    tx,
                    ty,
                    self.theme.text_primary.with_opacity(caption.opacity),
                    clips,
                );
            }
        }

        if let (Some(detail_str), Some((dw, _))) = (&caption.detail, detail_dims) {
            let dx = pill_x + (pill_w - (dw as i32)) / 2;
            let dy = ty + (title_h as i32) + line_gap;
            let detail_col = if caption.disabled {
                self.theme.text_disabled_reason
            } else {
                self.theme.text_secondary
            }
            .with_opacity(caption.opacity);
            let dm = self
                .cache
                .text_mask(detail_str, 11.0 * scale, 400, max_text_w);
            if let Some(ref mut pm) = self.pixmap {
                blit_alpha_mask_clipped(pm, dm, dx, dy, detail_col, clips);
            }
        }
    }

    fn draw_caption_concentric(&mut self, caption: &CaptionVisual, clips: &[RectPx]) {
        let scale = self.scale;
        let max_text_w = ((caption.box_.w - 24.0).max(40.0) * scale).floor() as u32;

        let crumb_layout = caption
            .breadcrumb
            .as_ref()
            .filter(|c| !c.is_empty())
            .map(|c| {
                self.cache
                    .layout_breadcrumb(c, 11.0 * scale, 500, max_text_w)
            });

        let (title_w, title_h) =
            self.cache
                .text_dims(&caption.title, 13.0 * scale, 600, max_text_w);

        let detail_dims = caption.detail.as_ref().map(|d| {
            self.cache
                .multiline_text_dims(d, 11.0 * scale, 400, max_text_w, 2)
        });

        let pad_x = (12.0 * scale).round() as i32;
        let pad_y = (6.0 * scale).round() as i32;
        let line_gap = (3.0 * scale).round() as i32;

        let mut content_w = title_w as i32;
        let mut content_h = title_h as i32;
        if let Some(ref layout) = crumb_layout {
            content_w = content_w.max(layout.width as i32);
            content_h += line_gap + (layout.height as i32);
        }
        if let Some((dw, dh)) = detail_dims {
            content_w = content_w.max(dw as i32);
            content_h += line_gap + (dh as i32);
        }

        let box_x = (caption.box_.x * scale).round() as i32;
        let box_y = (caption.box_.y * scale).round() as i32;
        let box_w = (caption.box_.w * scale).round() as i32;
        let box_h = (caption.box_.h * scale).round() as i32;

        let pill_w = (content_w + pad_x * 2).clamp((112.0 * scale) as i32, box_w);
        let pill_h = (content_h + pad_y * 2).clamp((32.0 * scale) as i32, box_h);
        let pill_x = box_x + (box_w - pill_w) / 2;
        let pill_y = box_y + (box_h - pill_h) / 2;
        let radius = 10.0 * scale;

        {
            let fill_col = self.theme.caption_fill.with_opacity(caption.opacity);
            let stroke_col = self.theme.ring_stroke.with_opacity(caption.opacity);
            let pm_masks =
                self.cache
                    .pill_masks(pill_w.max(1) as u32, pill_h.max(1) as u32, radius, scale);
            let dst_x = pill_x - pm_masks.pad;
            let dst_y = pill_y - pm_masks.pad;
            if let Some(ref mut pm) = self.pixmap {
                blit_alpha_mask_clipped(pm, &pm_masks.fill, dst_x, dst_y, fill_col, clips);
                blit_alpha_mask_clipped(pm, &pm_masks.stroke, dst_x, dst_y, stroke_col, clips);
            }
        }

        let mut cur_y = (pill_y + (pill_h - content_h) / 2).max(pill_y + 2);

        if let (Some(crumbs), Some(layout)) = (&caption.breadcrumb, &crumb_layout) {
            let mut cx_txt = pill_x + (pill_w - (layout.width as i32)) / 2;
            let crumb_col = self
                .theme
                .stem_on_path
                .lerp(self.theme.text_secondary, 0.45)
                .with_opacity(caption.opacity);
            let sep_arc = self.cache.crumb_sep.clone();
            let ell_arc = self.cache.crumb_ellipsis.clone();
            for run in &layout.runs {
                let (cm, pad_before, pad_after) = match *run {
                    BreadcrumbRun::Crumb { index, max_w } => (
                        self.cache
                            .text_mask(&crumbs[index], 11.0 * scale, 500, max_w),
                        0i32,
                        0i32,
                    ),
                    BreadcrumbRun::Sep => (
                        self.cache
                            .text_mask(&sep_arc, 11.0 * scale, 500, max_text_w),
                        layout.sep_pad as i32,
                        layout.sep_pad as i32,
                    ),
                    BreadcrumbRun::Ellipsis => (
                        self.cache
                            .text_mask(&ell_arc, 11.0 * scale, 500, max_text_w),
                        0i32,
                        0i32,
                    ),
                };
                cx_txt += pad_before;
                let dy = cur_y + ((layout.height as i32) - (cm.height as i32)) / 2;
                let run_w = cm.width as i32;
                if let Some(ref mut pm) = self.pixmap {
                    blit_alpha_mask_clipped(pm, cm, cx_txt, dy, crumb_col, clips);
                }
                cx_txt += run_w + pad_after;
            }
            cur_y += (layout.height as i32) + line_gap;
        }

        let tx = pill_x + (pill_w - (title_w as i32)) / 2;
        {
            let title_mask = self
                .cache
                .text_mask(&caption.title, 13.0 * scale, 600, max_text_w);
            if let Some(ref mut pm) = self.pixmap {
                blit_alpha_mask_clipped(
                    pm,
                    title_mask,
                    tx,
                    cur_y,
                    self.theme.text_primary.with_opacity(caption.opacity),
                    clips,
                );
            }
        }
        cur_y += (title_h as i32) + line_gap;

        if let (Some(detail_str), Some((dw, _))) = (&caption.detail, detail_dims) {
            let dx = pill_x + (pill_w - (dw as i32)) / 2;
            let detail_col = if caption.disabled {
                self.theme.text_disabled_reason
            } else {
                self.theme.text_secondary
            }
            .with_opacity(caption.opacity);
            let dm = self
                .cache
                .multiline_text_mask(detail_str, 11.0 * scale, 400, max_text_w, 2);
            if let Some(ref mut pm) = self.pixmap {
                blit_alpha_mask_clipped(pm, dm, dx, cur_y, detail_col, clips);
            }
        }
    }
}

fn fill_rect_px(pm: &mut Pixmap, rect: RectPx, pixel: [u8; 4]) {
    let w = pm.width() as i32;
    let h = pm.height() as i32;
    let x0 = rect.x.clamp(0, w) as usize;
    let y0 = rect.y.clamp(0, h) as usize;
    let x1 = rect.right().clamp(0, w) as usize;
    let y1 = rect.bottom().clamp(0, h) as usize;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let stride = (w as usize) * 4;
    let row_bytes = (x1 - x0) * 4;
    let data = pm.data_mut();
    if pixel == [0, 0, 0, 0] {
        for y in y0..y1 {
            let start = y * stride + x0 * 4;
            data[start..start + row_bytes].fill(0);
        }
    } else {
        for y in y0..y1 {
            let start = y * stride + x0 * 4;
            for px in data[start..start + row_bytes].chunks_exact_mut(4) {
                px.copy_from_slice(&pixel);
            }
        }
    }
}

fn fill_mask_rect(mask: &mut Mask, rect: RectPx, val: u8) {
    let w = mask.width() as i32;
    let h = mask.height() as i32;
    let x0 = rect.x.clamp(0, w) as usize;
    let y0 = rect.y.clamp(0, h) as usize;
    let x1 = rect.right().clamp(0, w) as usize;
    let y1 = rect.bottom().clamp(0, h) as usize;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let stride = w as usize;
    let row_len = x1 - x0;
    let data = mask.data_mut();
    for y in y0..y1 {
        let start = y * stride + x0;
        data[start..start + row_len].fill(val);
    }
}

/// Splits `r \ cut` into up to 4 disjoint rectangles and appends them to `out`.
fn subtract_rect_px(r: RectPx, cut: RectPx, out: &mut smallvec::SmallVec<[RectPx; 16]>) {
    let ix0 = r.x.max(cut.x);
    let iy0 = r.y.max(cut.y);
    let ix1 = r.right().min(cut.right());
    let iy1 = r.bottom().min(cut.bottom());
    if ix0 >= ix1 || iy0 >= iy1 {
        out.push(r);
        return;
    }
    if iy0 > r.y {
        out.push(RectPx::new(r.x, r.y, r.w, (iy0 - r.y) as u32));
    }
    if iy1 < r.bottom() {
        out.push(RectPx::new(r.x, iy1, r.w, (r.bottom() - iy1) as u32));
    }
    let mid_h = (iy1 - iy0) as u32;
    if ix0 > r.x {
        out.push(RectPx::new(r.x, iy0, (ix0 - r.x) as u32, mid_h));
    }
    if ix1 < r.right() {
        out.push(RectPx::new(ix1, iy0, (r.right() - ix1) as u32, mid_h));
    }
}

/// Blits an `AlphaMask` tinted with `color` (source-over premultiplied RGBA8) into `pm`,
/// strictly clipped to the non-overlapping damage rectangles `clips`.
fn blit_alpha_mask_clipped(
    pm: &mut Pixmap,
    mask: &AlphaMask,
    dst_x: i32,
    dst_y: i32,
    color: ColorRgba,
    clips: &[RectPx],
) {
    if color.a == 0 || mask.width == 0 || mask.height == 0 {
        return;
    }
    let pm_w = pm.width() as i32;
    let pm_h = pm.height() as i32;

    let sr = u32::from(color.r);
    let sg = u32::from(color.g);
    let sb = u32::from(color.b);
    let sa_base = u32::from(color.a);
    let solid_pr = (sr * sa_base + 127) / 255;
    let solid_pg = (sg * sa_base + 127) / 255;
    let solid_pb = (sb * sa_base + 127) / 255;
    let solid_inv_sa = 255 - sa_base;
    let solid_pixel = [
        solid_pr as u8,
        solid_pg as u8,
        solid_pb as u8,
        sa_base as u8,
    ];

    let stride = pm.width() as usize;
    let dst_data = pm.data_mut();

    for clip in clips {
        let x0 = dst_x.max(clip.x).max(0);
        let y0 = dst_y.max(clip.y).max(0);
        let x1 = (dst_x + (mask.width as i32)).min(clip.right()).min(pm_w);
        let y1 = (dst_y + (mask.height as i32)).min(clip.bottom()).min(pm_h);
        if x0 >= x1 || y0 >= y1 {
            continue;
        }

        for py in y0..y1 {
            let my = (py - dst_y) as usize;
            let (s_start, s_end) = mask.row_spans[my];
            if s_start >= s_end {
                continue;
            }
            let mask_row = my * (mask.width as usize);
            let dst_row = (py as usize) * stride;
            for span in &mask.spans[s_start as usize..s_end as usize] {
                let rx0 = x0.max(dst_x + i32::from(span.x0));
                let rx1 = x1.min(dst_x + i32::from(span.x1));
                if rx0 >= rx1 {
                    continue;
                }
                for px in rx0..rx1 {
                    let mx = (px - dst_x) as usize;
                    let cov = u32::from(mask.data[mask_row + mx]);
                    let idx = (dst_row + (px as usize)) * 4;
                    if cov == 255 {
                        if sa_base == 255 || dst_data[idx + 3] == 0 {
                            dst_data[idx..idx + 4].copy_from_slice(&solid_pixel);
                        } else {
                            dst_data[idx] = (solid_pr
                                + (u32::from(dst_data[idx]) * solid_inv_sa + 127) / 255)
                                .min(255) as u8;
                            dst_data[idx + 1] = (solid_pg
                                + (u32::from(dst_data[idx + 1]) * solid_inv_sa + 127) / 255)
                                .min(255) as u8;
                            dst_data[idx + 2] = (solid_pb
                                + (u32::from(dst_data[idx + 2]) * solid_inv_sa + 127) / 255)
                                .min(255) as u8;
                            dst_data[idx + 3] = (sa_base
                                + (u32::from(dst_data[idx + 3]) * solid_inv_sa + 127) / 255)
                                .min(255) as u8;
                        }
                        continue;
                    }
                    let sa = if sa_base == 255 {
                        cov
                    } else {
                        (sa_base * cov + 127) / 255
                    };
                    if sa == 0 {
                        continue;
                    }
                    let pr = (sr * sa + 127) / 255;
                    let pg = (sg * sa + 127) / 255;
                    let pb = (sb * sa + 127) / 255;
                    if dst_data[idx + 3] == 0 {
                        dst_data[idx] = pr as u8;
                        dst_data[idx + 1] = pg as u8;
                        dst_data[idx + 2] = pb as u8;
                        dst_data[idx + 3] = sa as u8;
                        continue;
                    }

                    let inv_sa = 255 - sa;
                    dst_data[idx] =
                        (pr + (u32::from(dst_data[idx]) * inv_sa + 127) / 255).min(255) as u8;
                    dst_data[idx + 1] =
                        (pg + (u32::from(dst_data[idx + 1]) * inv_sa + 127) / 255).min(255) as u8;
                    dst_data[idx + 2] =
                        (pb + (u32::from(dst_data[idx + 2]) * inv_sa + 127) / 255).min(255) as u8;
                    dst_data[idx + 3] =
                        (sa + (u32::from(dst_data[idx + 3]) * inv_sa + 127) / 255).min(255) as u8;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_sector_path(
    cx: f32,
    cy: f32,
    r_in: f32,
    r_out: f32,
    a0_in: f32,
    a1_in: f32,
    a0_out: f32,
    a1_out: f32,
) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let steps = 20usize;

    let (s0, c0) = a0_in.sin_cos();
    pb.move_to(cx + r_in * c0, cy + r_in * s0);

    for i in 1..=steps {
        let t = (i as f32) / (steps as f32);
        let a = a0_in + (a1_in - a0_in) * t;
        let (s, c) = a.sin_cos();
        pb.line_to(cx + r_in * c, cy + r_in * s);
    }

    for i in 0..=steps {
        let t = (i as f32) / (steps as f32);
        let a = a1_out - (a1_out - a0_out) * t;
        let (s, c) = a.sin_cos();
        pb.line_to(cx + r_out * c, cy + r_out * s);
    }

    pb.close();
    pb.finish()
}

fn build_annulus_path(cx: f32, cy: f32, r_in: f32, r_out: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, r_out);
    pb.push_circle(cx, cy, r_in);
    pb.finish()
}

fn build_arc_path(cx: f32, cy: f32, r: f32, a0: f32, a1: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let steps = 16usize;
    for i in 0..=steps {
        let t = (i as f32) / (steps as f32);
        let a = a0 + (a1 - a0) * t;
        let (s, c) = a.sin_cos();
        if i == 0 {
            pb.move_to(cx + r * c, cy + r * s);
        } else {
            pb.line_to(cx + r * c, cy + r * s);
        }
    }
    pb.finish()
}

fn build_rounded_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w * 0.5).min(h * 0.5).max(0.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Helper for encoding a [`FrameRef`] to a PNG file (for snapshot tests and sandbox screenshots).
pub fn save_frame_png(frame: FrameRef<'_>, path: &std::path::Path) -> std::io::Result<()> {
    let Some(size) = tiny_skia::IntSize::from_wh(frame.width, frame.height) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid frame dimensions",
        ));
    };
    let Some(pm) = Pixmap::from_vec(frame.data.to_vec(), size) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid frame buffer",
        ));
    };
    let png_bytes = pm
        .encode_png()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(path, png_bytes)
}

#[allow(dead_code)]
const _: fn() -> Point = || Point::new(0.0, 0.0);
