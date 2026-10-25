//! Brand colour tokens and dark/light themes (`WHEEL_ENGINE_DESIGN.md` §13.4).

use tiny_skia::Color;

/// Straight (unpremultiplied) 8-bit RGBA colour token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ColorRgba {
    /// Red channel (`0..=255`).
    pub r: u8,
    /// Green channel (`0..=255`).
    pub g: u8,
    /// Blue channel (`0..=255`).
    pub b: u8,
    /// Alpha channel (`0..=255`).
    pub a: u8,
}

impl ColorRgba {
    /// Creates an RGBA color from `(r, g, b, a)`.
    #[inline]
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Creates an opaque RGB color (`a = 255`).
    #[inline]
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Returns a copy with alpha multiplied by `factor` (`0.0..=1.0`).
    #[inline]
    #[must_use]
    pub fn with_opacity(self, factor: f32) -> Self {
        let a = ((self.a as f32) * factor.clamp(0.0, 1.0)).round() as u8;
        Self { a, ..self }
    }

    /// Linearly interpolates between `self` and `other` by `t` (`0.0..=1.0`).
    #[inline]
    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let mix =
            |a: u8, b: u8| -> u8 { ((a as f32) + ((b as f32) - (a as f32)) * t).round() as u8 };
        Self {
            r: mix(self.r, other.r),
            g: mix(self.g, other.g),
            b: mix(self.b, other.b),
            a: mix(self.a, other.a),
        }
    }

    /// Brightens RGB channels by `boost` (`0.0..=0.5`) for commit flash (`highlight > 1.0`).
    #[inline]
    #[must_use]
    pub fn brighten(self, boost: f32) -> Self {
        let b = boost.clamp(0.0, 0.5);
        let up = |c: u8| -> u8 { ((c as f32) + (255.0 - (c as f32)) * b).round() as u8 };
        Self {
            r: up(self.r),
            g: up(self.g),
            b: up(self.b),
            a: self.a,
        }
    }

    /// Converts to a `tiny_skia::Color`.
    #[inline]
    #[must_use]
    pub fn to_skia(self) -> Color {
        Color::from_rgba8(self.r, self.g, self.b, self.a)
    }
}

/// Visual colour theme for [`crate::Renderer`] (`WHEEL_ENGINE_DESIGN.md` §13.4).
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// Ring annulus fill (`#0A1A16` @ 92% in dark; `#F4FBF8` @ 94% in light).
    pub ring_fill: ColorRgba,
    /// Ring separator/border stroke (`#1E4D42` in dark).
    pub ring_stroke: ColorRgba,
    /// Hovered segment fill (`#00A87C` leaf green in dark; `#2BC79A` in light).
    pub segment_highlight: ColorRgba,
    /// Segment on open submenu path (`#0E6B52` in dark).
    pub segment_on_path: ColorRgba,
    /// Center hub disc fill (`#044C3C` vine green).
    pub hub_fill: ColorRgba,
    /// Center hub inner decorative ring (`#E8F5F0` at 70% hub radius).
    pub hub_inner_ring: ColorRgba,
    /// Primary label/icon colour (`#E8F5F0` in dark; `#073B30` in light).
    pub text_primary: ColorRgba,
    /// Secondary caption detail colour (`#9FC9BC` in dark).
    pub text_secondary: ColorRgba,
    /// Disabled item label/icon colour (`#5C7A72` in dark).
    pub text_disabled: ColorRgba,
    /// Label/icon colour on highlighted segment (`#04241C` in dark and light).
    pub text_on_accent: ColorRgba,
    /// Muted amber colour for `disabled_reason` in caption (`§4.4`).
    pub text_disabled_reason: ColorRgba,
    /// Default stem stroke colour (`#0E6B52` in dark).
    pub stem: ColorRgba,
    /// Active on-path stem stroke colour (`#00A87C` in dark).
    pub stem_on_path: ColorRgba,
    /// Drop shadow opacity (`0.35` in dark).
    pub shadow_alpha: f32,
    /// Caption pill background fill (`#0A1A16` @ 94% in dark).
    pub caption_fill: ColorRgba,
    /// Armed / keyboard-focus `2 lp` inside outline for concentric sectors (`#E8F5F0` in dark; `#073B30` in light).
    pub nested_outline: ColorRgba,
}

impl Theme {
    /// Default dark theme derived from Radivine brand colours (`#044C3C` vine, `#00A87C` leaf).
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            ring_fill: ColorRgba::new(0x0A, 0x1A, 0x16, 235), // #0A1A16 @ 92%
            ring_stroke: ColorRgba::rgb(0x1E, 0x4D, 0x42),    // #1E4D42
            segment_highlight: ColorRgba::rgb(0x00, 0xA8, 0x7C), // #00A87C
            segment_on_path: ColorRgba::rgb(0x0E, 0x6B, 0x52), // #0E6B52
            hub_fill: ColorRgba::rgb(0x04, 0x4C, 0x3C),       // #044C3C
            hub_inner_ring: ColorRgba::new(0xE8, 0xF5, 0xF0, 115), // #E8F5F0 subtle inner ring
            text_primary: ColorRgba::rgb(0xE8, 0xF5, 0xF0),   // #E8F5F0 (16.0:1)
            text_secondary: ColorRgba::rgb(0x9F, 0xC9, 0xBC), // #9FC9BC (9.9:1)
            text_disabled: ColorRgba::rgb(0x5C, 0x7A, 0x72),  // #5C7A72 (3.8:1)
            text_on_accent: ColorRgba::rgb(0x04, 0x24, 0x1C), // #04241C (5.4:1 on #00A87C)
            text_disabled_reason: ColorRgba::rgb(0xE5, 0xB5, 0x67), // muted amber
            stem: ColorRgba::rgb(0x0E, 0x6B, 0x52),           // #0E6B52
            stem_on_path: ColorRgba::rgb(0x00, 0xA8, 0x7C),   // #00A87C
            shadow_alpha: 0.35,
            caption_fill: ColorRgba::new(0x0A, 0x1A, 0x16, 240), // #0A1A16 @ 94%
            nested_outline: ColorRgba::rgb(0xE8, 0xF5, 0xF0),    // #E8F5F0
        }
    }

    /// Light theme (`WHEEL_ENGINE_DESIGN.md` §13.4).
    #[must_use]
    pub const fn light() -> Self {
        Self {
            ring_fill: ColorRgba::new(0xF4, 0xFB, 0xF8, 240), // #F4FBF8 @ 94%
            ring_stroke: ColorRgba::rgb(0xB8, 0xDC, 0xD1),    // #B8DCD1
            segment_highlight: ColorRgba::rgb(0x2B, 0xC7, 0x9A), // #2BC79A
            segment_on_path: ColorRgba::rgb(0xBC, 0xEE, 0xE0), // #BCEEE0
            hub_fill: ColorRgba::rgb(0x04, 0x4C, 0x3C),       // #044C3C
            hub_inner_ring: ColorRgba::new(0xE8, 0xF5, 0xF0, 140), // #E8F5F0
            text_primary: ColorRgba::rgb(0x07, 0x3B, 0x30),   // #073B30 (11.9:1)
            text_secondary: ColorRgba::rgb(0x2B, 0x63, 0x56), // #2B6356
            text_disabled: ColorRgba::rgb(0x7F, 0xA3, 0x9A),  // #7FA39A
            text_on_accent: ColorRgba::rgb(0x04, 0x24, 0x1C), // #04241C (7.6:1 on #2BC79A)
            text_disabled_reason: ColorRgba::rgb(0x9E, 0x58, 0x08),
            stem: ColorRgba::rgb(0x6C, 0xB8, 0xA1),
            stem_on_path: ColorRgba::rgb(0x00, 0xA8, 0x7C),
            shadow_alpha: 0.24,
            caption_fill: ColorRgba::new(0xF4, 0xFB, 0xF8, 245),
            nested_outline: ColorRgba::rgb(0x07, 0x3B, 0x30), // #073B30
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}
