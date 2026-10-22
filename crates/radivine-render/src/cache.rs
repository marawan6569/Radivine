//! Cached alpha masks for labels (`cosmic-text`), icons (`resvg`), and shadow sprites
//! (`WHEEL_ENGINE_DESIGN.md` §13.1, §13.2).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap,
    fontdb::{Database, Source},
};
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
];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TextCacheKey {
    text: Arc<str>,
    font_px_tenths: u32,
    weight: u16,
    max_width_px: u32,
}

pub(crate) struct AssetCache {
    font_system: FontSystem,
    swash_cache: SwashCache,
    svg_sources: HashMap<String, String>,
    icon_masks: HashMap<(String, u32), AlphaMask>,
    text_masks: HashMap<TextCacheKey, AlphaMask>,
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
            icon_masks: HashMap::with_capacity(64),
            text_masks: HashMap::with_capacity(128),
            shadow_sprites: HashMap::with_capacity(16),
        })
    }

    pub(crate) fn clear_session_labels(&mut self) {
        if self.text_masks.len() > 256 {
            self.text_masks.clear();
        }
    }

    pub(crate) fn icon_mask(&mut self, raw_key: &str, size_px: u32) -> &AlphaMask {
        let size_px = size_px.clamp(8, 256);
        let key_name = raw_key.strip_prefix("lucide:").unwrap_or(raw_key);
        let map_key = (key_name.to_owned(), size_px);

        if !self.icon_masks.contains_key(&map_key) {
            let mask = if let Some(svg_str) = self.svg_sources.get(key_name) {
                rasterize_svg_mask(svg_str, size_px).unwrap_or_else(|| fallback_icon_mask(size_px))
            } else {
                fallback_icon_mask(size_px)
            };
            self.icon_masks.insert(map_key.clone(), mask);
        }

        self.icon_masks.get(&map_key).unwrap()
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
    }

    // Collect glyph coverages and bounding box
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

    for run in buffer.layout_runs() {
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

    let width = ((max_x - min_x) + 1) as u32;
    let height = ((max_y - min_y) + 1) as u32;
    let mut data = vec![0u8; (width * height) as usize];
    for p in pixels {
        let ux = (p.x - min_x) as u32;
        let uy = (p.y - min_y) as u32;
        let idx = (uy * width + ux) as usize;
        data[idx] = data[idx].max(p.a);
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
