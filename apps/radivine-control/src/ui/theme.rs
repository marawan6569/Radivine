//! Brand palette, theme visuals, and reusable layout primitives (`apps/radivine-control/src/ui/theme.rs`).

use std::sync::Arc;

use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin,
    RichText, Stroke, Vec2,
};

use crate::service::DaemonHealthState;

pub(crate) const EMBEDDED_INTER_TTF: &[u8] =
    include_bytes!("../../../../assets/fonts/Inter-Variable.ttf");
pub(crate) const EMBEDDED_APP_ICON_PNG: &[u8] =
    include_bytes!("../../../../assets/icons/app-icon.png");
pub(crate) const EMBEDDED_LOGO_HORIZONTAL_PNG: &[u8] =
    include_bytes!("../../../../assets/branding/logo-horizontal.png");

/// Radivine Control Center color palette for dark and light themes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrandPalette {
    /// Background color for the main canvas.
    pub bg_canvas: Color32,
    /// Background color for the navigation sidebar.
    pub bg_sidebar: Color32,
    /// Background color for content cards.
    pub bg_card: Color32,
    /// Background color for elevated/interactive controls.
    pub bg_elevated: Color32,
    /// Primary text color.
    pub fg_primary: Color32,
    /// Secondary label text color.
    pub fg_secondary: Color32,
    /// Muted caption/hint text color.
    pub fg_muted: Color32,
    /// Brand vine green color.
    pub vine_green: Color32,
    /// Brand leaf green accent color.
    pub leaf_green: Color32,
    /// Accent background fill.
    pub accent_bg: Color32,
    /// Text color placed on top of `leaf_green`.
    pub fg_on_accent: Color32,
    /// Card and control border color.
    pub border: Color32,
    /// Positive/running status foreground color.
    pub ok_fg: Color32,
    /// Positive/running status background color.
    pub ok_bg: Color32,
    /// Warning/transitional status foreground color.
    pub warn_fg: Color32,
    /// Warning/transitional status background color.
    pub warn_bg: Color32,
    /// Error/failed status foreground color.
    pub err_fg: Color32,
    /// Error/failed status background color.
    pub err_bg: Color32,
}

impl BrandPalette {
    /// Returns the dark theme palette.
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            bg_canvas: Color32::from_rgb(0x0A, 0x1A, 0x16),
            bg_sidebar: Color32::from_rgb(0x07, 0x13, 0x10),
            bg_card: Color32::from_rgb(0x10, 0x26, 0x20),
            bg_elevated: Color32::from_rgb(0x16, 0x33, 0x2B),
            fg_primary: Color32::from_rgb(0xE8, 0xF5, 0xF0),
            fg_secondary: Color32::from_rgb(0x9F, 0xC9, 0xBC),
            fg_muted: Color32::from_rgb(0x64, 0x8C, 0x80),
            vine_green: Color32::from_rgb(0x04, 0x4C, 0x3C),
            leaf_green: Color32::from_rgb(0x00, 0xA8, 0x7C),
            accent_bg: Color32::from_rgb(0x0E, 0x42, 0x35),
            fg_on_accent: Color32::from_rgb(0x04, 0x24, 0x1C),
            border: Color32::from_rgb(0x1E, 0x4D, 0x42),
            ok_fg: Color32::from_rgb(0x34, 0xD3, 0x99),
            ok_bg: Color32::from_rgb(0x0A, 0x36, 0x29),
            warn_fg: Color32::from_rgb(0xFB, 0xBF, 0x24),
            warn_bg: Color32::from_rgb(0x3B, 0x2A, 0x10),
            err_fg: Color32::from_rgb(0xFB, 0x71, 0x85),
            err_bg: Color32::from_rgb(0x3B, 0x12, 0x19),
        }
    }

    /// Returns the light theme palette.
    #[must_use]
    pub const fn light() -> Self {
        Self {
            bg_canvas: Color32::from_rgb(0xF4, 0xFB, 0xF8),
            bg_sidebar: Color32::from_rgb(0xE6, 0xF4, 0xEE),
            bg_card: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            bg_elevated: Color32::from_rgb(0xEB, 0xF7, 0xF2),
            fg_primary: Color32::from_rgb(0x07, 0x3B, 0x30),
            fg_secondary: Color32::from_rgb(0x2B, 0x63, 0x56),
            fg_muted: Color32::from_rgb(0x5F, 0x8A, 0x7E),
            vine_green: Color32::from_rgb(0x04, 0x4C, 0x3C),
            leaf_green: Color32::from_rgb(0x00, 0x8F, 0x68),
            accent_bg: Color32::from_rgb(0xD1, 0xF2, 0xE6),
            fg_on_accent: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            border: Color32::from_rgb(0xB8, 0xDC, 0xD1),
            ok_fg: Color32::from_rgb(0x04, 0x78, 0x57),
            ok_bg: Color32::from_rgb(0xD1, 0xFA, 0xE5),
            warn_fg: Color32::from_rgb(0xB4, 0x53, 0x09),
            warn_bg: Color32::from_rgb(0xFE, 0xF3, 0xC7),
            err_fg: Color32::from_rgb(0xBE, 0x12, 0x3C),
            err_bg: Color32::from_rgb(0xFF, 0xE4, 0xE6),
        }
    }

    /// Selects the dark or light palette.
    #[must_use]
    pub const fn for_theme(is_dark: bool) -> Self {
        if is_dark { Self::dark() } else { Self::light() }
    }
}

static SYSTEM_FONT_PROVIDER: std::sync::LazyLock<Arc<dyn egui::FontProvider>> =
    std::sync::LazyLock::new(|| Arc::new(egui_system_font_provider::SystemFontProvider::new()));

/// Installs the embedded `Inter` primary font family and the native system-font fallback
/// provider (`egui_system_font_provider::SystemFontProvider`) into an `egui::Context`.
///
/// `Inter` remains at index 0 of [`FontFamily::Proportional`] so all Latin/English UI
/// typography is unchanged, while non-Latin scripts (such as Arabic, CJK, Hebrew, and
/// Devanagari) are resolved on demand from installed system fonts via Fontconfig/fontique.
pub fn install_inter_font(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "Inter".to_owned(),
        Arc::new(FontData::from_static(EMBEDDED_INTER_TTF)),
    );
    if let Some(prop) = fonts.families.get_mut(&FontFamily::Proportional) {
        prop.insert(0, "Inter".to_owned());
    }
    ctx.set_fonts(fonts);
    ctx.set_font_providers(vec![Arc::clone(&*SYSTEM_FONT_PROVIDER)]);
}

/// Applies the `BrandPalette` visual styling to an `egui::Context`.
pub fn apply_palette_visuals(ctx: &egui::Context, is_dark: bool, pal: &BrandPalette) {
    let mut visuals = if is_dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.panel_fill = pal.bg_canvas;
    visuals.window_fill = pal.bg_card;
    visuals.extreme_bg_color = pal.bg_sidebar;
    visuals.faint_bg_color = pal.bg_elevated;
    visuals.override_text_color = Some(pal.fg_primary);
    visuals.widgets.noninteractive.bg_fill = pal.bg_card;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, pal.border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, pal.fg_primary);
    visuals.widgets.inactive.bg_fill = pal.bg_elevated;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, pal.border);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, pal.fg_primary);
    visuals.widgets.hovered.bg_fill = pal.accent_bg;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.5, pal.leaf_green);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, pal.fg_primary);
    visuals.widgets.open.bg_fill = pal.bg_elevated;
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, pal.fg_primary);
    visuals.widgets.active.bg_fill = pal.leaf_green;
    visuals.widgets.active.fg_stroke = Stroke::new(1.5, pal.fg_on_accent);
    visuals.selection.bg_fill = pal.leaf_green;
    visuals.selection.stroke = Stroke::new(1.0, pal.fg_on_accent);
    ctx.set_visuals(visuals);
}

/// Renders a standard bordered section card with a bold title.
pub fn section_card<F>(ui: &mut egui::Ui, pal: &BrandPalette, title: &str, body: F)
where
    F: FnOnce(&mut egui::Ui),
{
    let inner_w = (ui.available_width() - 36.0).max(200.0);
    egui::Frame::NONE
        .fill(pal.bg_card)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(16, 14))
        .stroke(Stroke::new(1.0, pal.border))
        .show(ui, |ui| {
            ui.set_min_width(inner_w);
            ui.set_max_width(inner_w);
            ui.label(
                RichText::new(title)
                    .font(FontId::new(15.0, FontFamily::Proportional))
                    .strong()
                    .color(pal.fg_primary),
            );
            ui.add_space(8.0);
            body(ui);
        });
}

/// Renders a key-value pair inside a two-column `egui::Grid`.
pub fn kv_cell(ui: &mut egui::Ui, pal: &BrandPalette, label: &str, value: &str) {
    ui.label(RichText::new(label).color(pal.fg_secondary).size(12.5));
    ui.label(
        RichText::new(value)
            .color(pal.fg_primary)
            .size(12.5)
            .strong(),
    );
}

/// Maps a [`DaemonHealthState`] to `(fg_color, bg_color, glyph)`.
#[must_use]
pub fn health_colors(
    health: DaemonHealthState,
    pal: &BrandPalette,
) -> (Color32, Color32, &'static str) {
    match health {
        DaemonHealthState::Running => (pal.ok_fg, pal.ok_bg, "●"),
        DaemonHealthState::Starting | DaemonHealthState::Stopping => {
            (pal.warn_fg, pal.warn_bg, "◐")
        }
        DaemonHealthState::Failed | DaemonHealthState::Unreachable => (pal.err_fg, pal.err_bg, "▲"),
        DaemonHealthState::Stopped => (pal.fg_muted, pal.bg_elevated, "○"),
    }
}

pub(crate) fn decode_scaled_pixmap(png_bytes: &[u8], max_dim: u32) -> Option<tiny_skia::Pixmap> {
    let src = tiny_skia::Pixmap::decode_png(png_bytes).ok()?;
    let sw = src.width();
    let sh = src.height();
    if sw <= max_dim && sh <= max_dim {
        return Some(src);
    }
    let scale = (max_dim as f32 / sw.max(sh) as f32).clamp(0.05, 1.0);
    let dw = ((sw as f32 * scale).round() as u32).max(1);
    let dh = ((sh as f32 * scale).round() as u32).max(1);
    let mut dst = tiny_skia::Pixmap::new(dw, dh)?;
    let paint = tiny_skia::PixmapPaint {
        quality: tiny_skia::FilterQuality::Bicubic,
        ..Default::default()
    };
    dst.draw_pixmap(
        0,
        0,
        src.as_ref(),
        &paint,
        tiny_skia::Transform::from_scale(dw as f32 / sw as f32, dh as f32 / sh as f32),
        None,
    );
    Some(dst)
}

pub(crate) fn decode_png_to_color_image(png_bytes: &[u8], max_dim: u32) -> Option<ColorImage> {
    let pm = decode_scaled_pixmap(png_bytes, max_dim)?;
    let w = pm.width() as usize;
    let h = pm.height() as usize;
    let data = pm.data();
    let mut pixels = Vec::with_capacity(w * h);
    for px in data.chunks_exact(4) {
        pixels.push(Color32::from_rgba_premultiplied(px[0], px[1], px[2], px[3]));
    }
    Some(ColorImage {
        size: [w, h],
        pixels,
        source_size: Vec2::new(w as f32, h as f32),
    })
}

pub(crate) fn decode_png_to_icon_data(png_bytes: &[u8]) -> Option<egui::IconData> {
    let pm = decode_scaled_pixmap(png_bytes, 128)?;
    let width = pm.width();
    let height = pm.height();
    let mut rgba = Vec::with_capacity(pm.data().len());
    for px in pm.pixels() {
        let c = px.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some(egui::IconData {
        rgba,
        width,
        height,
    })
}
