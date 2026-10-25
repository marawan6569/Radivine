//! Cached alpha masks for labels (`cosmic-text`), icons (`resvg`), and shadow sprites
//! (`WHEEL_ENGINE_DESIGN.md` §13.1, §13.2).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap,
    fontdb::{Database, Source},
};
use smallvec::SmallVec;
use tiny_skia::{Pixmap, Transform};

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

/// Single-channel (`A8`) coverage mask cached for fast tinted blitting.
#[derive(Clone, Debug)]
pub struct AlphaMask {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// Row-major `width * height` coverage bytes (`0..=255`).
    pub data: Vec<u8>,
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct BreadcrumbCacheKey {
    crumbs: SmallVec<[Arc<str>; 6]>,
    font_px_tenths: u32,
    weight: u16,
    max_width_px: u32,
}

pub(crate) struct AssetCache {
    font_system: FontSystem,
    swash_cache: SwashCache,
    svg_sources: HashMap<String, String>,
    icon_masks: HashMap<u32, HashMap<Box<str>, AlphaMask>>,
    badge_masks: HashMap<(u32, u32), AlphaMask>,
    text_masks: HashMap<TextCacheKey, AlphaMask>,
    breadcrumb_masks: HashMap<BreadcrumbCacheKey, AlphaMask>,
    shadow_sprites: HashMap<(u32, u32), AlphaMask>,
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
            breadcrumb_masks: HashMap::with_capacity(32),
            shadow_sprites: HashMap::with_capacity(16),
        })
    }

    pub(crate) fn clear_session_labels(&mut self) {
        if self.text_masks.len() > 256 {
            self.text_masks.clear();
        }
        if self.breadcrumb_masks.len() > 64 {
            self.breadcrumb_masks.clear();
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

    pub(crate) fn breadcrumb_mask(
        &mut self,
        crumbs: &SmallVec<[Arc<str>; 6]>,
        font_px: f32,
        weight: u16,
        max_width_px: u32,
    ) -> &AlphaMask {
        let font_px_tenths = (font_px.clamp(6.0, 96.0) * 10.0).round() as u32;
        let max_width_px = max_width_px.max(16);
        let key = BreadcrumbCacheKey {
            crumbs: crumbs.clone(),
            font_px_tenths,
            weight,
            max_width_px,
        };

        if !self.breadcrumb_masks.contains_key(&key) {
            let actual_font_px = (font_px_tenths as f32) * 0.1;
            let mask = rasterize_breadcrumb_mask(
                &mut self.font_system,
                &mut self.swash_cache,
                crumbs,
                actual_font_px,
                weight,
                max_width_px,
            );
            self.breadcrumb_masks.insert(key.clone(), mask);
        }

        self.breadcrumb_masks.get(&key).unwrap()
    }

    pub(crate) fn breadcrumb_dims(
        &mut self,
        crumbs: &SmallVec<[Arc<str>; 6]>,
        font_px: f32,
        weight: u16,
        max_width_px: u32,
    ) -> (u32, u32) {
        let m = self.breadcrumb_mask(crumbs, font_px, weight, max_width_px);
        (m.width, m.height)
    }

    pub(crate) fn shadow_sprite(&mut self, r_out_px: u32, blur_px: u32) -> &AlphaMask {
        let key = (r_out_px, blur_px);
        self.shadow_sprites
            .entry(key)
            .or_insert_with(|| build_shadow_sprite(r_out_px, blur_px))
    }
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
    Some(AlphaMask {
        width: size_px,
        height: size_px,
        data,
    })
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
    AlphaMask {
        width: size_px,
        height: size_px,
        data,
    }
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

/// Formats and shapes a breadcrumb chain (`A0 › A1 › ...`), eliding the oldest middle ancestors
/// first (`A0 › … › A_{k} › ...`) so the most recent context and root context remain visible
/// (`INTERACTION_DESIGN.md` §I8).
fn rasterize_breadcrumb_mask(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    crumbs: &[Arc<str>],
    font_px: f32,
    weight: u16,
    max_width_px: u32,
) -> AlphaMask {
    if crumbs.is_empty() {
        return AlphaMask {
            width: 1,
            height: 1,
            data: vec![0],
        };
    }

    let line_h = (font_px * 1.3).ceil();
    let metrics = Metrics::new(font_px, line_h);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_wrap(font_system, Wrap::None);
    buffer.set_size(font_system, Some(4096.0), Some(line_h * 2.0));
    let attrs = Attrs::new()
        .family(Family::Name("Inter"))
        .weight(Weight(weight));

    let fits_width = |buf: &mut Buffer, fs: &mut FontSystem, s: &str| -> bool {
        buf.set_text(fs, s, attrs, Shaping::Advanced);
        buf.shape_until_scroll(fs, false);
        buf.layout_runs().next().map_or(0.0, |run| run.line_w) <= (max_width_px as f32)
    };

    let full = crumbs
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join(" › ");
    if fits_width(&mut buffer, font_system, &full) {
        return buffer_to_alpha_mask(&buffer, font_system, swash_cache, max_width_px, 1);
    }

    // Elide oldest middle ancestors first: keep `crumbs[0]`, `"…"`, and `crumbs[start_tail..]`
    if crumbs.len() >= 3 {
        for start_tail in 2..crumbs.len() {
            let mut parts = Vec::with_capacity(crumbs.len() - start_tail + 2);
            parts.push(crumbs[0].as_ref());
            parts.push("…");
            for item in &crumbs[start_tail..] {
                parts.push(item.as_ref());
            }
            let cand = parts.join(" › ");
            if fits_width(&mut buffer, font_system, &cand) {
                return buffer_to_alpha_mask(&buffer, font_system, swash_cache, max_width_px, 1);
            }
        }
    }

    // Try `"… › <deepest>"`
    if crumbs.len() >= 2 {
        let tail_only = format!("… › {}", crumbs[crumbs.len() - 1]);
        if fits_width(&mut buffer, font_system, &tail_only) {
            return buffer_to_alpha_mask(&buffer, font_system, swash_cache, max_width_px, 1);
        }
        return rasterize_text_mask(
            font_system,
            swash_cache,
            &tail_only,
            font_px,
            weight,
            max_width_px,
        );
    }

    rasterize_text_mask(
        font_system,
        swash_cache,
        &full,
        font_px,
        weight,
        max_width_px,
    )
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
        return AlphaMask {
            width: 1,
            height: 1,
            data: vec![0],
        };
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

    AlphaMask {
        width,
        height,
        data,
    }
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

    AlphaMask {
        width: size,
        height: size,
        data,
    }
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
