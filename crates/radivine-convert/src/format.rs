//! Media format identifiers, metadata descriptors, and magic-byte sniffing.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Canonical format identifier supported by the conversion engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum FormatId {
    Png,
    Jpeg,
    WebP,
}

impl FormatId {
    /// All v0.2.0 supported format identifiers in canonical display order.
    pub const ALL: &'static [FormatId] = &[FormatId::Png, FormatId::Jpeg, FormatId::WebP];

    /// Canonical lowercase identifier string (`"png"`, `"jpeg"`, `"webp"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            FormatId::Png => "png",
            FormatId::Jpeg => "jpeg",
            FormatId::WebP => "webp",
        }
    }

    /// Descriptor for this format.
    #[must_use]
    pub const fn descriptor(self) -> &'static FormatDescriptor {
        format(self)
    }
}

impl fmt::Display for FormatId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FormatId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.eq_ignore_ascii_case("png") {
            Ok(FormatId::Png)
        } else if trimmed.eq_ignore_ascii_case("jpeg") || trimmed.eq_ignore_ascii_case("jpg") {
            Ok(FormatId::Jpeg)
        } else if trimmed.eq_ignore_ascii_case("webp") {
            Ok(FormatId::WebP)
        } else {
            Err(format!("unknown format id: {trimmed:?}"))
        }
    }
}

/// High-level media category for a format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum MediaKind {
    Image,
}

/// Capability traits intrinsic to a container/codec format specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatTraits {
    pub supports_lossy: bool,
    pub supports_lossless: bool,
    pub supports_alpha: bool,
    pub supports_icc: bool,
    pub supports_exif: bool,
    pub supports_animation: bool,
    pub max_dimension: u32,
}

/// Static descriptor for a registered format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatDescriptor {
    pub id: FormatId,
    pub media_kind: MediaKind,
    pub display_name: &'static str,
    pub canonical_ext: &'static str,
    pub extensions: &'static [&'static str],
    pub mime_types: &'static [&'static str],
    pub traits: FormatTraits,
}

static FORMAT_DESCRIPTORS: [FormatDescriptor; 3] = [
    FormatDescriptor {
        id: FormatId::Png,
        media_kind: MediaKind::Image,
        display_name: "PNG",
        canonical_ext: "png",
        extensions: &["png"],
        mime_types: &["image/png", "image/x-png"],
        traits: FormatTraits {
            supports_lossy: false,
            supports_lossless: true,
            supports_alpha: true,
            supports_icc: true,
            supports_exif: true,
            supports_animation: true,
            max_dimension: 65_535,
        },
    },
    FormatDescriptor {
        id: FormatId::Jpeg,
        media_kind: MediaKind::Image,
        display_name: "JPEG",
        canonical_ext: "jpg",
        extensions: &["jpg", "jpeg", "jpe", "jfif"],
        mime_types: &["image/jpeg", "image/jpg", "image/pjpeg"],
        traits: FormatTraits {
            supports_lossy: true,
            supports_lossless: false,
            supports_alpha: false,
            supports_icc: true,
            supports_exif: true,
            supports_animation: false,
            max_dimension: 65_535,
        },
    },
    FormatDescriptor {
        id: FormatId::WebP,
        media_kind: MediaKind::Image,
        display_name: "WebP",
        canonical_ext: "webp",
        extensions: &["webp"],
        mime_types: &["image/webp"],
        traits: FormatTraits {
            supports_lossy: true,
            supports_lossless: true,
            supports_alpha: true,
            supports_icc: true,
            supports_exif: true,
            supports_animation: true,
            max_dimension: 16_383,
        },
    },
];

/// Returns all registered format descriptors.
#[must_use]
pub const fn formats() -> &'static [FormatDescriptor] {
    &FORMAT_DESCRIPTORS
}

/// Returns the static descriptor for a given [`FormatId`].
#[must_use]
pub const fn format(id: FormatId) -> &'static FormatDescriptor {
    match id {
        FormatId::Png => &FORMAT_DESCRIPTORS[0],
        FormatId::Jpeg => &FORMAT_DESCRIPTORS[1],
        FormatId::WebP => &FORMAT_DESCRIPTORS[2],
    }
}

/// Resolves a file extension (with or without a leading `.`) to a [`FormatId`].
#[must_use]
pub fn format_from_extension(ext: &str) -> Option<FormatId> {
    let clean = ext.trim().trim_start_matches('.');
    if clean.is_empty() {
        return None;
    }
    for desc in &FORMAT_DESCRIPTORS {
        if desc
            .extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(clean))
        {
            return Some(desc.id);
        }
    }
    None
}

/// Resolves a MIME type string (ignoring optional parameters after `;`) to a [`FormatId`].
#[must_use]
pub fn format_from_mime(mime: &str) -> Option<FormatId> {
    let base = mime.split(';').next()?.trim();
    if base.is_empty() {
        return None;
    }
    for desc in &FORMAT_DESCRIPTORS {
        if desc
            .mime_types
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(base))
        {
            return Some(desc.id);
        }
    }
    None
}

/// Sniffs a [`FormatId`] from the first up-to-16 bytes of a file header.
///
/// Magic-byte signatures:
/// - **PNG:** `\x89PNG\r\n\x1a\n` (8 bytes)
/// - **JPEG:** `\xFF\xD8\xFF` (3 bytes)
/// - **WebP:** `RIFF` at `0..4` and `WEBP` at `8..12` (12 bytes)
#[must_use]
pub fn sniff_format(header: &[u8]) -> Option<FormatId> {
    if header.len() >= 8 && &header[0..8] == b"\x89PNG\r\n\x1a\n" {
        return Some(FormatId::Png);
    }
    if header.len() >= 3 && &header[0..3] == b"\xFF\xD8\xFF" {
        return Some(FormatId::Jpeg);
    }
    if header.len() >= 12 && &header[0..4] == b"RIFF" && &header[8..12] == b"WEBP" {
        return Some(FormatId::WebP);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_magic_bytes() {
        assert_eq!(
            sniff_format(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR"),
            Some(FormatId::Png)
        );
        assert_eq!(
            sniff_format(b"\xFF\xD8\xFF\xE0\x00\x10JFIF"),
            Some(FormatId::Jpeg)
        );
        assert_eq!(
            sniff_format(b"RIFF\x24\x00\x00\x00WEBPVP8 "),
            Some(FormatId::WebP)
        );
        // RIFF WAVE is not WebP
        assert_eq!(sniff_format(b"RIFF\x24\x00\x00\x00WAVEfmt "), None);
        // Truncated headers
        assert_eq!(sniff_format(b"\x89PNG\r\n"), None);
        assert_eq!(sniff_format(b"\xFF\xD8"), None);
        assert_eq!(sniff_format(b"RIFF\x00\x00\x00\x00WEB"), None);
        assert_eq!(sniff_format(b""), None);
    }

    #[test]
    fn lookup_by_extension_and_mime() {
        assert_eq!(format_from_extension("png"), Some(FormatId::Png));
        assert_eq!(format_from_extension(".PNG"), Some(FormatId::Png));
        assert_eq!(format_from_extension("jpg"), Some(FormatId::Jpeg));
        assert_eq!(format_from_extension("JPEG"), Some(FormatId::Jpeg));
        assert_eq!(format_from_extension("jpe"), Some(FormatId::Jpeg));
        assert_eq!(format_from_extension("jfif"), Some(FormatId::Jpeg));
        assert_eq!(format_from_extension("webp"), Some(FormatId::WebP));
        assert_eq!(format_from_extension("gif"), None);
        assert_eq!(format_from_extension(""), None);

        assert_eq!(format_from_mime("image/png"), Some(FormatId::Png));
        assert_eq!(
            format_from_mime("IMAGE/JPEG; charset=binary"),
            Some(FormatId::Jpeg)
        );
        assert_eq!(format_from_mime("image/webp"), Some(FormatId::WebP));
        assert_eq!(format_from_mime("application/octet-stream"), None);
    }

    #[test]
    fn format_descriptors_match_ids() {
        for desc in formats() {
            assert_eq!(format(desc.id), desc);
            assert_eq!(format_from_extension(desc.canonical_ext), Some(desc.id));
        }
    }
}
