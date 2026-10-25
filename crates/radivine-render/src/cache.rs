//! Cached alpha masks for labels (`cosmic-text`), icons (`resvg`), and shadow sprites
//! (`WHEEL_ENGINE_DESIGN.md` §13.1, §13.2).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap,
    fontdb::{Database, Source},
};
use radivine_wheel::geometry::gap_inset_angle;
use smallvec::SmallVec;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, Transform,
};

/// Source for font and SVG icon assets (`WHEEL_ENGINE_DESIGN.md` §13.1).
#[derive(Clone, Debug)]
pub enum AssetSource {
    /// Uses bundled assets compiled into the binary (`assets/fonts` + `assets/wheel-icons`).
    Embedded,
    /// Loads assets from a filesystem directory containing `fonts/` and `wheel-icons/`.
    Dir(PathBuf),
}

/// Errors returned during [`crate::Renderer`] initialization (`WHEEL_ENGINE_DESIGN.md` §13.1).
#[derive(thiserror::Error, Debug)]
pub enum RenderError {
    /// Failed to load or parse the bundled font.
    #[error("font: {0}")]
    Font(String),
    /// Failed to load or parse a required bundled SVG icon.
    #[error("icon: {0}")]
    Icon(String),
}

/// Non-zero coverage span `[x0, x1)` within a single row of an [`AlphaMask`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct MaskSpan {
    pub(crate) x0: u16,
    pub(crate) x1: u16,
}

/// Single-channel (`A8`) coverage mask cached for fast tinted blitting.
#[derive(Clone, Debug)]
pub struct AlphaMask {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// Row-major `width * height` coverage bytes (`0..=255`).
    pub data: Vec<u8>,
    /// Contiguous non-zero horizontal spans across all rows.
    pub(crate) spans: Vec<MaskSpan>,
    /// Per-row `[span_start, span_end)` slice indices into `spans`.
    pub(crate) row_spans: Vec<(u32, u32)>,
}

impl AlphaMask {
    /// Constructs an [`AlphaMask`] and precomputes per-row non-zero horizontal spans.
    #[must_use]
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        let w = width as usize;
        let h = height as usize;
        let mut spans = Vec::with_capacity(h * 2);
        let mut row_spans = Vec::with_capacity(h);
        for y in 0..h {
            let start_idx = spans.len() as u32;
            let row = &data[y * w..(y + 1) * w];
            let mut x = 0usize;
            while x < w {
                if let Some(rel_first) = row[x..].iter().position(|&b| b != 0) {
                    let s0 = x + rel_first;
                    let s1 = row[s0..]
                        .iter()
                        .position(|&b| b == 0)
                        .map_or(w, |rel_zero| s0 + rel_zero);
                    spans.push(MaskSpan {
                        x0: s0 as u16,
                        x1: s1.min(u16::MAX as usize) as u16,
                    });
                    x = s1;
                } else {
                    break;
                }
            }
            let end_idx = spans.len() as u32;
            row_spans.push((start_idx, end_idx));
        }
        Self {
            width,
            height,
            data,
            spans,
            row_spans,
        }
    }
}

const EMBEDDED_INTER_TTF: &[u8] = include_bytes!("../../../assets/fonts/Inter-Variable.ttf");

const EMBEDDED_ICONS: &[(&str, &str)] = &[
    ("eye", include_str!("../../../assets/wheel-icons/eye.svg")),
    (
        "file-text",
        include_str!("../../../assets/wheel-icons/file-text.svg"),
    ),
    (
        "arrow-up-to-line",
        include_str!("../../../assets/wheel-icons/arrow-up-to-line.svg"),
    ),
    (
        "arrow-down-to-line",
        include_str!("../../../assets/wheel-icons/arrow-down-to-line.svg"),
    ),
    ("copy", include_str!("../../../assets/wheel-icons/copy.svg")),
    ("info", include_str!("../../../assets/wheel-icons/info.svg")),
    (
        "sparkles",
        include_str!("../../../assets/wheel-icons/sparkles.svg"),
    ),
    (
        "terminal",
        include_str!("../../../assets/wheel-icons/terminal.svg"),
    ),
    ("code", include_str!("../../../assets/wheel-icons/code.svg")),
    (
        "more-horizontal",
        include_str!("../../../assets/wheel-icons/more-horizontal.svg"),
    ),
    ("x", include_str!("../../../assets/wheel-icons/x.svg")),
    (
        "chevron-left",
        include_str!("../../../assets/wheel-icons/chevron-left.svg"),
    ),
    ("file", include_str!("../../../assets/wheel-icons/file.svg")),
    (
        "files",
        include_str!("../../../assets/wheel-icons/files.svg"),
    ),
    (
        "folder",
        include_str!("../../../assets/wheel-icons/folder.svg"),
    ),
    (
        "image",
        include_str!("../../../assets/wheel-icons/image.svg"),
    ),
    ("link", include_str!("../../../assets/wheel-icons/link.svg")),
    (
        "arrow-right-left",
        include_str!("../../../assets/wheel-icons/arrow-right-left.svg"),
    ),
    (
        "file-image",
        include_str!("../../../assets/wheel-icons/file-image.svg"),
    ),
    (
        "sliders-horizontal",
        include_str!("../../../assets/wheel-icons/sliders-horizontal.svg"),
    ),
];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TextCacheKey {
    text: Arc<str>,
    font_px_tenths: u32,
    weight: u16,
    max_width_px: u32,
    max_lines: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BreadcrumbRun {
    Crumb { index: usize, max_w: u32 },
    Sep,
    Ellipsis,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BreadcrumbLayout {
    pub(crate) runs: SmallVec<[BreadcrumbRun; 13]>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) sep_pad: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ConcentricSectorKey {
    pub(crate) cx_bits: u32,
    pub(crate) cy_bits: u32,
    pub(crate) w_r_in_bits: u32,
    pub(crate) w_r_out_bits: u32,
    pub(crate) a0_bits: u32,
    pub(crate) a1_bits: u32,
    pub(crate) sep_bits: u32,
    pub(crate) count: u16,
    pub(crate) scale_bits: u32,
}

pub(crate) struct ConcentricSectorMasks {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) fill: AlphaMask,
    pub(crate) stroke: AlphaMask,
    pub(crate) outline: AlphaMask,
    pub(crate) marker_x: i32,
    pub(crate) marker_y: i32,
    pub(crate) marker_fill: AlphaMask,
    pub(crate) marker_stroke: AlphaMask,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct HubCacheKey {
    pub(crate) cx_bits: u32,
    pub(crate) cy_bits: u32,
    pub(crate) r_in_bits: u32,
    pub(crate) ws_bits: u32,
    pub(crate) scale_bits: u32,
}

pub(crate) struct HubMasks {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) fill: AlphaMask,
    pub(crate) border: AlphaMask,
    pub(crate) inner_ring: Option<AlphaMask>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CircleCacheKey {
    pub(crate) cx_bits: u32,
    pub(crate) cy_bits: u32,
    pub(crate) r_bits: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PillCacheKey {
    pub(crate) w: u32,
    pub(crate) h: u32,
    pub(crate) radius_bits: u32,
    pub(crate) scale_bits: u32,
}

pub(crate) struct PillMasks {
    pub(crate) pad: i32,
    pub(crate) fill: AlphaMask,
    pub(crate) stroke: AlphaMask,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct GuideCacheKey {
    pub(crate) ax_bits: u32,
    pub(crate) ay_bits: u32,
    pub(crate) cx_bits: u32,
    pub(crate) cy_bits: u32,
    pub(crate) scale_bits: u32,
}

pub(crate) struct AssetCache {
    font_system: FontSystem,
    swash_cache: SwashCache,
    svg_sources: HashMap<String, String>,
    icon_masks: HashMap<u32, HashMap<Box<str>, AlphaMask>>,
    badge_masks: HashMap<(u32, u32), AlphaMask>,
    text_masks: HashMap<TextCacheKey, AlphaMask>,
    pub(crate) crumb_sep: Arc<str>,
    pub(crate) crumb_ellipsis: Arc<str>,
    shadow_sprites: HashMap<(u32, u32), AlphaMask>,
    concentric_sectors: HashMap<ConcentricSectorKey, ConcentricSectorMasks>,
    hub_masks: HashMap<HubCacheKey, HubMasks>,
    circle_masks: HashMap<CircleCacheKey, (i32, i32, AlphaMask)>,
    pill_masks: HashMap<PillCacheKey, PillMasks>,
    guide_masks: HashMap<GuideCacheKey, (i32, i32, AlphaMask)>,
}

impl AssetCache {
    pub(crate) fn new(source: AssetSource) -> Result<Self, RenderError> {
        let mut db = Database::new();
        let mut svg_sources = HashMap::with_capacity(24);

        match source {
            AssetSource::Embedded => {
                db.load_font_source(Source::Binary(Arc::new(EMBEDDED_INTER_TTF)));
                for &(name, svg) in EMBEDDED_ICONS {
                    // Validate SVG parses cleanly at init
                    let opts = resvg::usvg::Options::default();
                    resvg::usvg::Tree::from_str(svg, &opts)
                        .map_err(|e| RenderError::Icon(format!("{name}: {e}")))?;
                    svg_sources.insert(name.to_owned(), svg.to_owned());
                }
            }
            AssetSource::Dir(dir) => {
                let font_path = dir.join("fonts/Inter-Variable.ttf");
                let font_bytes = std::fs::read(&font_path)
                    .map_err(|e| RenderError::Font(format!("{}: {e}", font_path.display())))?;
                db.load_font_source(Source::Binary(Arc::new(font_bytes)));

                for &(name, _) in EMBEDDED_ICONS {
                    let svg_path = dir.join("wheel-icons").join(format!("{name}.svg"));
                    let svg = std::fs::read_to_string(&svg_path)
                        .map_err(|e| RenderError::Icon(format!("{}: {e}", svg_path.display())))?;
                    let opts = resvg::usvg::Options::default();
                    resvg::usvg::Tree::from_str(&svg, &opts)
                        .map_err(|e| RenderError::Icon(format!("{name}: {e}")))?;
                    svg_sources.insert(name.to_owned(), svg);
                }
            }
        }

        if db.faces().count() == 0 {
            return Err(RenderError::Font(
                "no font faces loaded from Inter-Variable.ttf".to_owned(),
            ));
        }

        let font_system = FontSystem::new_with_locale_and_db("en-US".to_owned(), db);
        let swash_cache = SwashCache::new();

        Ok(Self {
            font_system,
            swash_cache,
            svg_sources,
            icon_masks: HashMap::with_capacity(8),
            badge_masks: HashMap::with_capacity(16),
            text_masks: HashMap::with_capacity(128),
            crumb_sep: Arc::from(" › "),
            crumb_ellipsis: Arc::from("…"),
            shadow_sprites: HashMap::with_capacity(16),
            concentric_sectors: HashMap::with_capacity(64),
            hub_masks: HashMap::with_capacity(8),
            circle_masks: HashMap::with_capacity(8),
            pill_masks: HashMap::with_capacity(16),
            guide_masks: HashMap::with_capacity(4),
        })
    }

    pub(crate) fn clear_session_labels(&mut self) {
        if self.text_masks.len() > 256 {
            self.text_masks.clear();
        }
        if self.concentric_sectors.len() > 128 {
            self.concentric_sectors.clear();
        }
        if self.hub_masks.len() > 32 {
            self.hub_masks.clear();
        }
        if self.circle_masks.len() > 32 {
            self.circle_masks.clear();
        }
        if self.pill_masks.len() > 64 {
            self.pill_masks.clear();
        }
        if self.guide_masks.len() > 16 {
            self.guide_masks.clear();
        }
    }

    pub(crate) fn icon_mask(&mut self, raw_key: &str, size_px: u32) -> &AlphaMask {
        let size_px = size_px.clamp(8, 256);
        let key_name = raw_key.strip_prefix("lucide:").unwrap_or(raw_key);

        let needs_insert = self
            .icon_masks
            .get(&size_px)
            .is_none_or(|by_name| !by_name.contains_key(key_name));
        if needs_insert {
            let mask = if let Some(svg_str) = self.svg_sources.get(key_name) {
                rasterize_svg_mask(svg_str, size_px).unwrap_or_else(|| fallback_icon_mask(size_px))
            } else {
                fallback_icon_mask(size_px)
            };
            self.icon_masks
                .entry(size_px)
                .or_insert_with(|| HashMap::with_capacity(24))
                .insert(key_name.into(), mask);
        }

        self.icon_masks
            .get(&size_px)
            .and_then(|by_name| by_name.get(key_name))
            .unwrap()
    }

    pub(crate) fn icon_dims(&mut self, raw_key: &str, size_px: u32) -> (u32, u32) {
        let m = self.icon_mask(raw_key, size_px);
        (m.width, m.height)
    }

    pub(crate) fn badge_mask(&mut self, count: u32, font_px: f32) -> &AlphaMask {
        let font_px_tenths = (font_px.clamp(6.0, 96.0) * 10.0).round() as u32;
        let key = (count, font_px_tenths);
        if !self.badge_masks.contains_key(&key) {
            let actual_font_px = (font_px_tenths as f32) * 0.1;
            let text = count.to_string();
            let mask = rasterize_text_mask(
                &mut self.font_system,
                &mut self.swash_cache,
                &text,
                actual_font_px,
                700,
                48,
            );
            self.badge_masks.insert(key, mask);
        }
        self.badge_masks.get(&key).unwrap()
    }

    pub(crate) fn badge_dims(&mut self, count: u32, font_px: f32) -> (u32, u32) {
        let m = self.badge_mask(count, font_px);
        (m.width, m.height)
    }

    pub(crate) fn text_mask(
        &mut self,
        text: &Arc<str>,
        font_px: f32,
        weight: u16,
        max_width_px: u32,
    ) -> &AlphaMask {
        let font_px_tenths = (font_px.clamp(6.0, 96.0) * 10.0).round() as u32;
        let max_width_px = max_width_px.max(16);
        let key = TextCacheKey {
            text: text.clone(),
            font_px_tenths,
            weight,
            max_width_px,
            max_lines: 1,
        };

        if !self.text_masks.contains_key(&key) {
            let actual_font_px = (font_px_tenths as f32) * 0.1;
            let mask = rasterize_text_mask(
                &mut self.font_system,
                &mut self.swash_cache,
                text,
                actual_font_px,
                weight,
                max_width_px,
            );
            self.text_masks.insert(key.clone(), mask);
        }

        self.text_masks.get(&key).unwrap()
    }

    pub(crate) fn text_dims(
        &mut self,
        text: &Arc<str>,
        font_px: f32,
        weight: u16,
        max_width_px: u32,
    ) -> (u32, u32) {
        let m = self.text_mask(text, font_px, weight, max_width_px);
        (m.width, m.height)
    }

    pub(crate) fn multiline_text_mask(
        &mut self,
        text: &Arc<str>,
        font_px: f32,
        weight: u16,
        max_width_px: u32,
        max_lines: u8,
    ) -> &AlphaMask {
        let max_lines = max_lines.max(1);
        if max_lines == 1 {
            return self.text_mask(text, font_px, weight, max_width_px);
        }
        let font_px_tenths = (font_px.clamp(6.0, 96.0) * 10.0).round() as u32;
        let max_width_px = max_width_px.max(16);
        let key = TextCacheKey {
            text: text.clone(),
            font_px_tenths,
            weight,
            max_width_px,
            max_lines,
        };

        if !self.text_masks.contains_key(&key) {
            let actual_font_px = (font_px_tenths as f32) * 0.1;
            let mask = rasterize_multiline_text_mask(
                &mut self.font_system,
                &mut self.swash_cache,
                text,
                actual_font_px,
                weight,
                max_width_px,
                usize::from(max_lines),
            );
            self.text_masks.insert(key.clone(), mask);
        }

        self.text_masks.get(&key).unwrap()
    }

    pub(crate) fn multiline_text_dims(
        &mut self,
        text: &Arc<str>,
        font_px: f32,
        weight: u16,
        max_width_px: u32,
        max_lines: u8,
    ) -> (u32, u32) {
        let m = self.multiline_text_mask(text, font_px, weight, max_width_px, max_lines);
        (m.width, m.height)
    }

    pub(crate) fn layout_breadcrumb(
        &mut self,
        crumbs: &[Arc<str>],
        font_px: f32,
        weight: u16,
        max_width_px: u32,
    ) -> BreadcrumbLayout {
        let max_width_px = max_width_px.max(16);
        let sep_pad = (font_px * 0.35).round().max(2.0) as u32;
        let n = crumbs.len().min(6);
        if n == 0 {
            return BreadcrumbLayout {
                runs: SmallVec::new(),
                width: 0,
                height: 0,
                sep_pad,
            };
        }

        let sep = self.crumb_sep.clone();
        let ell = self.crumb_ellipsis.clone();
        let (sep_glyph_w, sep_h) = self.text_dims(&sep, font_px, weight, max_width_px);
        let sep_w = sep_glyph_w + 2 * sep_pad;
        let (ell_w, ell_h) = self.text_dims(&ell, font_px, weight, max_width_px);

        let mut widths = [0u32; 6];
        let mut max_h = sep_h.max(ell_h);
        for (i, crumb) in crumbs.iter().take(n).enumerate() {
            let (w, h) = self.text_dims(crumb, font_px, weight, max_width_px);
            widths[i] = w;
            max_h = max_h.max(h);
        }

        if n == 1 {
            let mut runs = SmallVec::new();
            runs.push(BreadcrumbRun::Crumb {
                index: 0,
                max_w: max_width_px,
            });
            return BreadcrumbLayout {
                runs,
                width: widths[0],
                height: max_h,
                sep_pad,
            };
        }

        let full_w: u32 = widths[..n].iter().sum::<u32>() + ((n - 1) as u32) * sep_w;
        if full_w <= max_width_px {
            let mut runs = SmallVec::new();
            for i in 0..n {
                if i > 0 {
                    runs.push(BreadcrumbRun::Sep);
                }
                runs.push(BreadcrumbRun::Crumb {
                    index: i,
                    max_w: max_width_px,
                });
            }
            return BreadcrumbLayout {
                runs,
                width: full_w,
                height: max_h,
                sep_pad,
            };
        }

        // Elide oldest middle ancestors first: keep `crumbs[0]`, `"…"`, and `crumbs[start_tail..n]`
        if n >= 3 {
            for start_tail in 2..n {
                let tail_sum: u32 = widths[start_tail..n].iter().sum::<u32>()
                    + ((n - 1 - start_tail) as u32) * sep_w;
                let cand_w = widths[0] + sep_w + ell_w + sep_w + tail_sum;
                if cand_w <= max_width_px {
                    let mut runs = SmallVec::new();
                    runs.push(BreadcrumbRun::Crumb {
                        index: 0,
                        max_w: max_width_px,
                    });
                    runs.push(BreadcrumbRun::Sep);
                    runs.push(BreadcrumbRun::Ellipsis);
                    for idx in start_tail..n {
                        runs.push(BreadcrumbRun::Sep);
                        runs.push(BreadcrumbRun::Crumb {
                            index: idx,
                            max_w: max_width_px,
                        });
                    }
                    return BreadcrumbLayout {
                        runs,
                        width: cand_w,
                        height: max_h,
                        sep_pad,
                    };
                }
            }
        }

        // Fallback: `"… › <deepest>"` with `<deepest>` truncated to remaining width
        let prefix_w = ell_w + sep_w;
        let last_max_w = max_width_px.saturating_sub(prefix_w).max(16);
        let (last_w, last_h) = self.text_dims(&crumbs[n - 1], font_px, weight, last_max_w);
        let mut runs = SmallVec::new();
        runs.push(BreadcrumbRun::Ellipsis);
        runs.push(BreadcrumbRun::Sep);
        runs.push(BreadcrumbRun::Crumb {
            index: n - 1,
            max_w: last_max_w,
        });
        BreadcrumbLayout {
            runs,
            width: (prefix_w + last_w).min(max_width_px),
            height: max_h.max(last_h),
            sep_pad,
        }
    }

    pub(crate) fn shadow_sprite(&mut self, r_out_px: u32, blur_px: u32) -> &AlphaMask {
        let key = (r_out_px, blur_px);
        self.shadow_sprites
            .entry(key)
            .or_insert_with(|| build_shadow_sprite(r_out_px, blur_px))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn concentric_sector_masks(
        &mut self,
        cx: f32,
        cy: f32,
        w_r_in: f32,
        w_r_out: f32,
        a0: f32,
        a1: f32,
        sep_half: f32,
        count: usize,
        scale: f32,
    ) -> &ConcentricSectorMasks {
        let key = ConcentricSectorKey {
            cx_bits: (cx + 0.0).to_bits(),
            cy_bits: (cy + 0.0).to_bits(),
            w_r_in_bits: (w_r_in + 0.0).to_bits(),
            w_r_out_bits: (w_r_out + 0.0).to_bits(),
            a0_bits: (a0 + 0.0).to_bits(),
            a1_bits: (a1 + 0.0).to_bits(),
            sep_bits: (sep_half + 0.0).to_bits(),
            count: count as u16,
            scale_bits: (scale + 0.0).to_bits(),
        };
        self.concentric_sectors.entry(key).or_insert_with(|| {
            build_concentric_sector_masks(cx, cy, w_r_in, w_r_out, a0, a1, sep_half, count, scale)
        })
    }

    pub(crate) fn hub_masks(
        &mut self,
        cx: f32,
        cy: f32,
        r_in: f32,
        ws: f32,
        scale: f32,
    ) -> &HubMasks {
        let key = HubCacheKey {
            cx_bits: (cx + 0.0).to_bits(),
            cy_bits: (cy + 0.0).to_bits(),
            r_in_bits: (r_in + 0.0).to_bits(),
            ws_bits: (ws + 0.0).to_bits(),
            scale_bits: (scale + 0.0).to_bits(),
        };
        self.hub_masks
            .entry(key)
            .or_insert_with(|| build_hub_masks(cx, cy, r_in, ws, scale))
    }

    pub(crate) fn circle_mask(&mut self, cx: f32, cy: f32, r: f32) -> &(i32, i32, AlphaMask) {
        let key = CircleCacheKey {
            cx_bits: (cx + 0.0).to_bits(),
            cy_bits: (cy + 0.0).to_bits(),
            r_bits: (r + 0.0).to_bits(),
        };
        self.circle_masks
            .entry(key)
            .or_insert_with(|| build_circle_mask(cx, cy, r))
    }

    pub(crate) fn pill_masks(&mut self, w: u32, h: u32, radius: f32, scale: f32) -> &PillMasks {
        let key = PillCacheKey {
            w,
            h,
            radius_bits: (radius + 0.0).to_bits(),
            scale_bits: (scale + 0.0).to_bits(),
        };
        self.pill_masks
            .entry(key)
            .or_insert_with(|| build_pill_masks(w, h, radius, scale))
    }

    pub(crate) fn guide_mask(
        &mut self,
        ax: f32,
        ay: f32,
        cx: f32,
        cy: f32,
        scale: f32,
    ) -> &(i32, i32, AlphaMask) {
        let key = GuideCacheKey {
            ax_bits: (ax + 0.0).to_bits(),
            ay_bits: (ay + 0.0).to_bits(),
            cx_bits: (cx + 0.0).to_bits(),
            cy_bits: (cy + 0.0).to_bits(),
            scale_bits: (scale + 0.0).to_bits(),
        };
        self.guide_masks
            .entry(key)
            .or_insert_with(|| build_guide_mask(ax, ay, cx, cy, scale))
    }
}

fn empty_alpha_mask() -> AlphaMask {
    AlphaMask::new(1, 1, vec![0])
}

fn pixmap_to_alpha_mask(pm: &Pixmap) -> AlphaMask {
    let w = pm.width();
    let h = pm.height();
    let mut data = Vec::with_capacity((w * h) as usize);
    for px in pm.pixels() {
        data.push(px.alpha());
    }
    AlphaMask::new(w, h, data)
}

fn rasterize_fill_mask(w: u32, h: u32, path: &Path, rule: FillRule) -> AlphaMask {
    let Some(mut pm) = Pixmap::new(w.max(1), h.max(1)) else {
        return empty_alpha_mask();
    };
    let mut paint = Paint::default();
    paint.set_color(Color::WHITE);
    paint.anti_alias = true;
    pm.fill_path(path, &paint, rule, Transform::identity(), None);
    pixmap_to_alpha_mask(&pm)
}

fn rasterize_stroke_mask(w: u32, h: u32, path: &Path, stroke: &Stroke) -> AlphaMask {
    let Some(mut pm) = Pixmap::new(w.max(1), h.max(1)) else {
        return empty_alpha_mask();
    };
    let mut paint = Paint::default();
    paint.set_color(Color::WHITE);
    paint.anti_alias = true;
    pm.stroke_path(path, &paint, stroke, Transform::identity(), None);
    pixmap_to_alpha_mask(&pm)
}

#[allow(clippy::too_many_arguments)]
fn build_sector_path_local(
    cx: f32,
    cy: f32,
    r_in: f32,
    r_out: f32,
    a0_in: f32,
    a1_in: f32,
    a0_out: f32,
    a1_out: f32,
) -> Option<Path> {
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

fn build_annulus_path_local(cx: f32, cy: f32, r_in: f32, r_out: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, r_out);
    pb.push_circle(cx, cy, r_in);
    pb.finish()
}

fn build_rounded_rect_path_local(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
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

#[allow(clippy::too_many_arguments)]
fn build_concentric_sector_masks(
    cx: f32,
    cy: f32,
    w_r_in: f32,
    w_r_out: f32,
    a0: f32,
    a1: f32,
    sep_half: f32,
    count: usize,
    scale: f32,
) -> ConcentricSectorMasks {
    let r_in = w_r_in * scale;
    let r_out = w_r_out * scale;
    let pad = (4.0 * scale).ceil() as i32;

    let (x0, y0, w, h) = if count <= 1 {
        let max_r = r_out.ceil() as i32 + pad;
        let x0 = (cx.floor() as i32) - max_r;
        let y0 = (cy.floor() as i32) - max_r;
        let side = (max_r * 2 + 2).max(2) as u32;
        (x0, y0, side, side)
    } else {
        let r_lo = (r_in - 2.0 * scale).max(0.0);
        let r_hi = r_out + 2.0 * scale;
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
        include_angle(a0);
        include_angle(a1);
        include_angle((a0 + a1) * 0.5);
        let half_pi = std::f32::consts::FRAC_PI_2;
        let k_start = (a0 / half_pi).ceil() as i32;
        let k_end = (a1 / half_pi).floor() as i32;
        for k in k_start..=k_end {
            include_angle((k as f32) * half_pi);
        }
        let x0 = (min_x.floor() as i32) - pad;
        let y0 = (min_y.floor() as i32) - pad;
        let x1 = (max_x.ceil() as i32) + pad;
        let y1 = (max_y.ceil() as i32) + pad;
        (x0, y0, (x1 - x0).max(2) as u32, (y1 - y0).max(2) as u32)
    };

    let lcx = cx - (x0 as f32);
    let lcy = cy - (y0 as f32);

    let (fill, stroke, outline) = if count <= 1 {
        let fill_and_stroke = build_annulus_path_local(lcx, lcy, r_in, r_out);
        let f_mask = fill_and_stroke.as_ref().map_or_else(empty_alpha_mask, |p| {
            rasterize_fill_mask(w, h, p, FillRule::EvenOdd)
        });
        let st = Stroke {
            width: (1.0 * scale).max(1.0),
            ..Stroke::default()
        };
        let s_mask = fill_and_stroke
            .as_ref()
            .map_or_else(empty_alpha_mask, |p| rasterize_stroke_mask(w, h, p, &st));

        let r_in_outl = r_in + 1.0 * scale;
        let r_out_outl = (r_out - 1.0 * scale).max(r_in_outl + 1.0);
        let ost = Stroke {
            width: (2.0 * scale).max(1.0),
            ..Stroke::default()
        };
        let o_mask = build_annulus_path_local(lcx, lcy, r_in_outl, r_out_outl)
            .map_or_else(empty_alpha_mask, |p| rasterize_stroke_mask(w, h, &p, &ost));
        (f_mask, s_mask, o_mask)
    } else {
        let a0_s = a0 + sep_half;
        let a1_s = (a1 - sep_half).max(a0_s + 0.01);
        let sector = build_sector_path_local(lcx, lcy, r_in, r_out, a0_s, a1_s, a0_s, a1_s);
        let f_mask = sector.as_ref().map_or_else(empty_alpha_mask, |p| {
            rasterize_fill_mask(w, h, p, FillRule::Winding)
        });
        let st = Stroke {
            width: (1.0 * scale).max(1.0),
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        let s_mask = sector
            .as_ref()
            .map_or_else(empty_alpha_mask, |p| rasterize_stroke_mask(w, h, p, &st));

        let r_in_outl = r_in + 1.0 * scale;
        let r_out_outl = (r_out - 1.0 * scale).max(r_in_outl + 1.0);
        let d_in_outl = gap_inset_angle(1.0, w_r_in + 1.0);
        let d_out_outl = gap_inset_angle(1.0, (w_r_out - 1.0).max(w_r_in + 2.0));
        let a0_in_o = a0_s + d_in_outl;
        let a1_in_o = (a1_s - d_in_outl).max(a0_in_o + 0.005);
        let a0_out_o = a0_s + d_out_outl;
        let a1_out_o = (a1_s - d_out_outl).max(a0_out_o + 0.005);
        let ost = Stroke {
            width: (2.0 * scale).max(1.0),
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        let o_mask = build_sector_path_local(
            lcx, lcy, r_in_outl, r_out_outl, a0_in_o, a1_in_o, a0_out_o, a1_out_o,
        )
        .map_or_else(empty_alpha_mask, |p| rasterize_stroke_mask(w, h, &p, &ost));
        (f_mask, s_mask, o_mask)
    };

    let phi = (a0 + a1) * 0.5;
    let (sin_p, cos_p) = phi.sin_cos();
    let nx = cx + r_out * cos_p;
    let ny = cy + r_out * sin_p;
    let node_r = (3.5 * scale).max(2.0);
    let m_pad = (node_r + 3.0 * scale).ceil() as i32;
    let mx0 = (nx.floor() as i32) - m_pad;
    let my0 = (ny.floor() as i32) - m_pad;
    let m_side = (m_pad * 2 + 2).max(2) as u32;
    let m_circle = PathBuilder::from_circle(nx - (mx0 as f32), ny - (my0 as f32), node_r);
    let marker_fill = m_circle.as_ref().map_or_else(empty_alpha_mask, |p| {
        rasterize_fill_mask(m_side, m_side, p, FillRule::Winding)
    });
    let mst = Stroke {
        width: (1.5 * scale).max(1.0),
        ..Stroke::default()
    };
    let marker_stroke = m_circle.as_ref().map_or_else(empty_alpha_mask, |p| {
        rasterize_stroke_mask(m_side, m_side, p, &mst)
    });

    ConcentricSectorMasks {
        x: x0,
        y: y0,
        fill,
        stroke,
        outline,
        marker_x: mx0,
        marker_y: my0,
        marker_fill,
        marker_stroke,
    }
}

fn build_hub_masks(cx: f32, cy: f32, r_in: f32, ws: f32, scale: f32) -> HubMasks {
    let pad = (r_in + 4.0 * scale).ceil() as i32;
    let x0 = (cx.floor() as i32) - pad;
    let y0 = (cy.floor() as i32) - pad;
    let side = (pad * 2 + 2).max(2) as u32;
    let lcx = cx - (x0 as f32);
    let lcy = cy - (y0 as f32);

    let outer = PathBuilder::from_circle(lcx, lcy, r_in);
    let fill = outer.as_ref().map_or_else(empty_alpha_mask, |p| {
        rasterize_fill_mask(side, side, p, FillRule::Winding)
    });
    let border_st = Stroke {
        width: (1.2 * scale).max(1.0),
        ..Stroke::default()
    };
    let border = outer.as_ref().map_or_else(empty_alpha_mask, |p| {
        rasterize_stroke_mask(side, side, p, &border_st)
    });

    let inner_r = r_in * 0.70;
    let inner_ring = if inner_r >= 4.0 {
        let ring_st = Stroke {
            width: (1.5 * ws * scale).max(1.0),
            ..Stroke::default()
        };
        PathBuilder::from_circle(lcx, lcy, inner_r)
            .map(|p| rasterize_stroke_mask(side, side, &p, &ring_st))
    } else {
        None
    };

    HubMasks {
        x: x0,
        y: y0,
        fill,
        border,
        inner_ring,
    }
}

fn build_circle_mask(cx: f32, cy: f32, r: f32) -> (i32, i32, AlphaMask) {
    let pad = (r + 3.0).ceil() as i32;
    let x0 = (cx.floor() as i32) - pad;
    let y0 = (cy.floor() as i32) - pad;
    let side = (pad * 2 + 2).max(2) as u32;
    let mask = PathBuilder::from_circle(cx - (x0 as f32), cy - (y0 as f32), r)
        .map_or_else(empty_alpha_mask, |p| {
            rasterize_fill_mask(side, side, &p, FillRule::Winding)
        });
    (x0, y0, mask)
}

fn build_pill_masks(w: u32, h: u32, radius: f32, scale: f32) -> PillMasks {
    let pad = (3.0 * scale).ceil().max(2.0) as i32;
    let mw = w + (pad as u32) * 2;
    let mh = h + (pad as u32) * 2;
    let path = build_rounded_rect_path_local(pad as f32, pad as f32, w as f32, h as f32, radius);
    let fill = path.as_ref().map_or_else(empty_alpha_mask, |p| {
        rasterize_fill_mask(mw, mh, p, FillRule::Winding)
    });
    let st = Stroke {
        width: (1.0 * scale).max(1.0),
        ..Stroke::default()
    };
    let stroke = path
        .as_ref()
        .map_or_else(empty_alpha_mask, |p| rasterize_stroke_mask(mw, mh, p, &st));
    PillMasks { pad, fill, stroke }
}

fn build_guide_mask(ax: f32, ay: f32, cx: f32, cy: f32, scale: f32) -> (i32, i32, AlphaMask) {
    let pad = (4.0 * scale).ceil() as i32;
    let x0 = (ax.min(cx) * scale).floor() as i32 - pad;
    let y0 = (ay.min(cy) * scale).floor() as i32 - pad;
    let x1 = (ax.max(cx) * scale).ceil() as i32 + pad;
    let y1 = (ay.max(cy) * scale).ceil() as i32 + pad;
    let w = (x1 - x0).max(2) as u32;
    let h = (y1 - y0).max(2) as u32;
    let mut pb = PathBuilder::new();
    pb.move_to(ax * scale - (x0 as f32), ay * scale - (y0 as f32));
    pb.line_to(cx * scale - (x0 as f32), cy * scale - (y0 as f32));
    let st = Stroke {
        width: (1.5 * scale).max(1.0),
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let mask = pb
        .finish()
        .map_or_else(empty_alpha_mask, |p| rasterize_stroke_mask(w, h, &p, &st));
    (x0, y0, mask)
}

fn rasterize_svg_mask(svg_str: &str, size_px: u32) -> Option<AlphaMask> {
    let opts = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_str(svg_str, &opts).ok()?;
    let mut pixmap = Pixmap::new(size_px, size_px)?;
    let tree_size = tree.size();
    let sx = (size_px as f32) / tree_size.width().max(1.0);
    let sy = (size_px as f32) / tree_size.height().max(1.0);
    resvg::render(&tree, Transform::from_scale(sx, sy), &mut pixmap.as_mut());

    let mut data = Vec::with_capacity((size_px * size_px) as usize);
    for px in pixmap.pixels() {
        data.push(px.alpha());
    }
    Some(AlphaMask::new(size_px, size_px, data))
}

fn fallback_icon_mask(size_px: u32) -> AlphaMask {
    let mut data = vec![0u8; (size_px * size_px) as usize];
    let c = (size_px as f32) * 0.5;
    let r_outer = c * 0.68;
    let r_inner = c * 0.48;
    for y in 0..size_px {
        for x in 0..size_px {
            let d = ((x as f32) + 0.5 - c).hypot((y as f32) + 0.5 - c);
            if d >= r_inner && d <= r_outer {
                data[(y * size_px + x) as usize] = 220;
            }
        }
    }
    AlphaMask::new(size_px, size_px, data)
}

fn rasterize_text_mask(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    raw_text: &str,
    font_px: f32,
    weight: u16,
    max_width_px: u32,
) -> AlphaMask {
    let line_h = (font_px * 1.3).ceil();
    let metrics = Metrics::new(font_px, line_h);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_wrap(font_system, Wrap::None);
    buffer.set_size(font_system, Some(4096.0), Some(line_h * 2.0));

    let attrs = Attrs::new()
        .family(Family::Name("Inter"))
        .weight(Weight(weight));

    // Fit text within max_width_px, truncating with "…" if needed
    buffer.set_text(font_system, raw_text, attrs, Shaping::Advanced);
    buffer.shape_until_scroll(font_system, false);

    let measure_w =
        |buf: &Buffer| -> f32 { buf.layout_runs().next().map_or(0.0, |run| run.line_w) };

    if measure_w(&buffer) > (max_width_px as f32) && raw_text.chars().count() > 2 {
        let chars: Vec<char> = raw_text.chars().collect();
        let mut keep = chars.len().saturating_sub(1);
        while keep > 1 {
            let mut candidate: String = chars[..keep].iter().collect();
            candidate.push('…');
            buffer.set_text(font_system, &candidate, attrs, Shaping::Advanced);
            buffer.shape_until_scroll(font_system, false);
            if measure_w(&buffer) <= (max_width_px as f32) {
                break;
            }
            keep -= 1;
        }
        if measure_w(&buffer) > (max_width_px as f32) {
            buffer.set_text(font_system, "…", attrs, Shaping::Advanced);
            buffer.shape_until_scroll(font_system, false);
        }
    }

    buffer_to_alpha_mask(&buffer, font_system, swash_cache, max_width_px, 1)
}

fn rasterize_multiline_text_mask(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    raw_text: &str,
    font_px: f32,
    weight: u16,
    max_width_px: u32,
    max_lines: usize,
) -> AlphaMask {
    let line_h = (font_px * 1.25).ceil();
    let metrics = Metrics::new(font_px, line_h);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_wrap(font_system, Wrap::WordOrGlyph);
    buffer.set_size(
        font_system,
        Some(max_width_px as f32),
        Some(line_h * ((max_lines + 4) as f32)),
    );

    let attrs = Attrs::new()
        .family(Family::Name("Inter"))
        .weight(Weight(weight));

    buffer.set_text(font_system, raw_text, attrs, Shaping::Advanced);
    buffer.shape_until_scroll(font_system, false);

    let count_runs = |buf: &Buffer| -> usize { buf.layout_runs().count() };

    if count_runs(&buffer) > max_lines && raw_text.chars().count() > 2 {
        let chars: Vec<char> = raw_text.chars().collect();
        let mut low = 1usize;
        let mut high = chars.len().saturating_sub(1);
        let mut best = 1usize;
        while low <= high {
            let mid = (low + high) / 2;
            let mut candidate: String = chars[..mid].iter().collect();
            candidate.push('…');
            buffer.set_text(font_system, &candidate, attrs, Shaping::Advanced);
            buffer.shape_until_scroll(font_system, false);
            if count_runs(&buffer) <= max_lines {
                best = mid;
                low = mid + 1;
            } else if mid == 0 {
                break;
            } else {
                high = mid - 1;
            }
        }
        let mut final_cand: String = chars[..best].iter().collect();
        final_cand.push('…');
        buffer.set_text(font_system, &final_cand, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(font_system, false);
    }

    buffer_to_alpha_mask(&buffer, font_system, swash_cache, max_width_px, max_lines)
}

fn buffer_to_alpha_mask(
    buffer: &Buffer,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    max_width_px: u32,
    max_lines: usize,
) -> AlphaMask {
    struct PlacedPixel {
        x: i32,
        y: i32,
        a: u8,
    }
    let mut pixels: Vec<PlacedPixel> = Vec::with_capacity(512);
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;

    for run in buffer.layout_runs().take(max_lines) {
        let line_y = run.line_y;
        for glyph in run.glyphs {
            let physical = glyph.physical((0.0, line_y), 1.0);
            let Some(img) = swash_cache.get_image(font_system, physical.cache_key) else {
                continue;
            };
            let w = img.placement.width as i32;
            let h = img.placement.height as i32;
            if w <= 0 || h <= 0 {
                continue;
            }
            let gx0 = physical.x + img.placement.left;
            let gy0 = physical.y - img.placement.top;

            match img.content {
                cosmic_text::SwashContent::Mask => {
                    for row in 0..h {
                        for col in 0..w {
                            let a = img.data[(row * w + col) as usize];
                            if a > 0 {
                                let px = gx0 + col;
                                let py = gy0 + row;
                                min_x = min_x.min(px);
                                min_y = min_y.min(py);
                                max_x = max_x.max(px);
                                max_y = max_y.max(py);
                                pixels.push(PlacedPixel { x: px, y: py, a });
                            }
                        }
                    }
                }
                cosmic_text::SwashContent::Color => {
                    for row in 0..h {
                        for col in 0..w {
                            let idx = ((row * w + col) * 4) as usize;
                            let a = img.data[idx + 3];
                            if a > 0 {
                                let px = gx0 + col;
                                let py = gy0 + row;
                                min_x = min_x.min(px);
                                min_y = min_y.min(py);
                                max_x = max_x.max(px);
                                max_y = max_y.max(py);
                                pixels.push(PlacedPixel { x: px, y: py, a });
                            }
                        }
                    }
                }
                cosmic_text::SwashContent::SubpixelMask => {
                    for row in 0..h {
                        for col in 0..w {
                            let idx = ((row * w + col) * 3) as usize;
                            let a = ((u16::from(img.data[idx])
                                + u16::from(img.data[idx + 1])
                                + u16::from(img.data[idx + 2]))
                                / 3) as u8;
                            if a > 0 {
                                let px = gx0 + col;
                                let py = gy0 + row;
                                min_x = min_x.min(px);
                                min_y = min_y.min(py);
                                max_x = max_x.max(px);
                                max_y = max_y.max(py);
                                pixels.push(PlacedPixel { x: px, y: py, a });
                            }
                        }
                    }
                }
            }
        }
    }

    if pixels.is_empty() || min_x > max_x || min_y > max_y {
        return AlphaMask::new(1, 1, vec![0]);
    }

    let raw_width = ((max_x - min_x) + 1) as u32;
    let width = raw_width.min(max_width_px.max(1));
    let height = ((max_y - min_y) + 1) as u32;
    let mut data = vec![0u8; (width * height) as usize];
    for p in pixels {
        let ux = (p.x - min_x) as u32;
        let uy = (p.y - min_y) as u32;
        if ux < width && uy < height {
            let idx = (uy * width + ux) as usize;
            data[idx] = data[idx].max(p.a);
        }
    }

    AlphaMask::new(width, height, data)
}

fn build_shadow_sprite(r_out_px: u32, blur_px: u32) -> AlphaMask {
    let r = r_out_px as f32;
    let blur = (blur_px as f32).max(1.0);
    let half = r_out_px + blur_px + 2;
    let size = half * 2;
    let c = half as f32;
    let mut data = vec![0u8; (size * size) as usize];

    for y in 0..size {
        let dy = (y as f32) + 0.5 - c;
        for x in 0..size {
            let dx = (x as f32) + 0.5 - c;
            let dist = dx.hypot(dy);
            let alpha = if dist <= r - blur * 0.35 {
                1.0
            } else if dist >= r + blur {
                0.0
            } else {
                // Smooth cubic falloff across [r - 0.35*blur, r + blur]
                let t = ((dist - (r - blur * 0.35)) / (blur * 1.35)).clamp(0.0, 1.0);
                let inv = 1.0 - t;
                inv * inv * (3.0 - 2.0 * inv)
            };
            data[(y * size + x) as usize] = (alpha * 255.0).round() as u8;
        }
    }

    AlphaMask::new(size, size, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_lucide_wheel_icons_rasterize_embedded_and_dir() {
        let mut embedded = AssetCache::new(AssetSource::Embedded).expect("embedded AssetCache");
        let assets_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let mut dir_cache = AssetCache::new(AssetSource::Dir(assets_dir)).expect("dir AssetCache");

        for key in [
            "lucide:arrow-right-left",
            "lucide:file-image",
            "lucide:sliders-horizontal",
        ] {
            let mask_emb = embedded.icon_mask(key, 24).clone();
            assert_eq!(mask_emb.width, 24);
            assert_eq!(mask_emb.height, 24);
            assert!(
                mask_emb.data.iter().any(|&a| a > 0),
                "embedded icon {key} rasterized to all-zero alpha"
            );

            let mask_dir = dir_cache.icon_mask(key, 24);
            assert_eq!(mask_dir.data, mask_emb.data);
        }
    }
}
