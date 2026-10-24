//! Format-independent and format-specific conversion options, quality intents, and validation.

use std::fmt;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::backend::{CapabilityCatalog, FeatureSet};
use crate::format::FormatId;

/// Format-independent quality intent tier (`ADR-032`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityIntent {
    Lossless,
    Maximum,
    High,
    #[default]
    Balanced,
    Compact,
    Custom(u8),
}

/// JPEG chroma subsampling mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChromaSubsampling {
    /// `4:2:0` for `quality <= 89`, `4:4:4` for `quality >= 90`.
    #[default]
    Auto,
    #[serde(rename = "4:4:4", alias = "cs444")]
    Cs444,
    #[serde(rename = "4:2:2", alias = "cs422")]
    Cs422,
    #[serde(rename = "4:2:0", alias = "cs420")]
    Cs420,
}

/// WebP compression mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebPMode {
    #[default]
    Lossy,
    Lossless,
}

/// PNG lossless compression effort tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PngCompression {
    Fast,
    #[default]
    Default,
    Best,
}

/// Format-specific encoder parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "lowercase")]
pub enum FormatQuality {
    Jpeg {
        quality: u8,
        progressive: bool,
        optimize_coding: bool,
        subsampling: ChromaSubsampling,
    },
    WebP {
        mode: WebPMode,
        quality: u8,
        alpha_quality: u8,
        method: u8,
    },
    Png {
        compression: PngCompression,
    },
}

impl FormatQuality {
    /// Returns the format this quality setting applies to.
    #[must_use]
    pub const fn format_id(&self) -> FormatId {
        match self {
            FormatQuality::Jpeg { .. } => FormatId::Jpeg,
            FormatQuality::WebP { .. } => FormatId::WebP,
            FormatQuality::Png { .. } => FormatId::Png,
        }
    }
}

/// Either a format-independent [`QualityIntent`] or explicit [`FormatQuality`] parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QualitySetting {
    Intent(QualityIntent),
    PerFormat(FormatQuality),
}

impl Default for QualitySetting {
    fn default() -> Self {
        Self::Intent(QualityIntent::Balanced)
    }
}

impl QualitySetting {
    /// Resolves this quality setting into concrete [`FormatQuality`] parameters for `target`.
    #[must_use]
    pub fn resolve_for_target(&self, target: FormatId) -> FormatQuality {
        match *self {
            QualitySetting::PerFormat(fq) if fq.format_id() == target => fq,
            QualitySetting::PerFormat(_) => {
                Self::Intent(QualityIntent::Balanced).resolve_for_target(target)
            }
            QualitySetting::Intent(intent) => match (intent, target) {
                (QualityIntent::Lossless, FormatId::Jpeg) => FormatQuality::Jpeg {
                    quality: 95,
                    progressive: true,
                    optimize_coding: true,
                    subsampling: ChromaSubsampling::Cs444,
                },
                (QualityIntent::Maximum, FormatId::Jpeg) => FormatQuality::Jpeg {
                    quality: 92,
                    progressive: true,
                    optimize_coding: true,
                    subsampling: ChromaSubsampling::Cs444,
                },
                (QualityIntent::High, FormatId::Jpeg) => FormatQuality::Jpeg {
                    quality: 86,
                    progressive: true,
                    optimize_coding: true,
                    subsampling: ChromaSubsampling::Auto,
                },
                (QualityIntent::Balanced, FormatId::Jpeg) => FormatQuality::Jpeg {
                    quality: 82,
                    progressive: true,
                    optimize_coding: true,
                    subsampling: ChromaSubsampling::Auto,
                },
                (QualityIntent::Compact, FormatId::Jpeg) => FormatQuality::Jpeg {
                    quality: 72,
                    progressive: true,
                    optimize_coding: true,
                    subsampling: ChromaSubsampling::Auto,
                },
                (QualityIntent::Custom(q), FormatId::Jpeg) => FormatQuality::Jpeg {
                    quality: q.clamp(1, 100),
                    progressive: true,
                    optimize_coding: true,
                    subsampling: ChromaSubsampling::Auto,
                },

                (QualityIntent::Lossless, FormatId::WebP) => FormatQuality::WebP {
                    mode: WebPMode::Lossless,
                    quality: 100,
                    alpha_quality: 100,
                    method: 4,
                },
                (QualityIntent::Maximum, FormatId::WebP) => FormatQuality::WebP {
                    mode: WebPMode::Lossy,
                    quality: 92,
                    alpha_quality: 100,
                    method: 5,
                },
                (QualityIntent::High, FormatId::WebP) => FormatQuality::WebP {
                    mode: WebPMode::Lossy,
                    quality: 85,
                    alpha_quality: 100,
                    method: 4,
                },
                (QualityIntent::Balanced, FormatId::WebP) => FormatQuality::WebP {
                    mode: WebPMode::Lossy,
                    quality: 80,
                    alpha_quality: 100,
                    method: 4,
                },
                (QualityIntent::Compact, FormatId::WebP) => FormatQuality::WebP {
                    mode: WebPMode::Lossy,
                    quality: 70,
                    alpha_quality: 85,
                    method: 5,
                },
                (QualityIntent::Custom(q), FormatId::WebP) => FormatQuality::WebP {
                    mode: WebPMode::Lossy,
                    quality: q.min(100),
                    alpha_quality: 100,
                    method: 4,
                },

                (
                    QualityIntent::Lossless | QualityIntent::Maximum | QualityIntent::Compact,
                    FormatId::Png,
                ) => FormatQuality::Png {
                    compression: PngCompression::Best,
                },
                (
                    QualityIntent::High | QualityIntent::Balanced | QualityIntent::Custom(_),
                    FormatId::Png,
                ) => FormatQuality::Png {
                    compression: PngCompression::Default,
                },
            },
        }
    }
}

/// Optional resize geometry specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ResizeSpec {
    #[default]
    Original,
    Width {
        px: NonZeroU32,
        #[serde(default)]
        allow_upscale: bool,
    },
    Height {
        px: NonZeroU32,
        #[serde(default)]
        allow_upscale: bool,
    },
    Fit {
        width: NonZeroU32,
        height: NonZeroU32,
        #[serde(default)]
        allow_upscale: bool,
    },
}

/// 8-bit sRGB color value used for alpha flattening backgrounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb8 {
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
    };
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0 };

    /// Formats as `#RRGGBB`.
    #[must_use]
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

impl Default for Rgb8 {
    fn default() -> Self {
        Self::WHITE
    }
}

impl fmt::Display for Rgb8 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

impl FromStr for Rgb8 {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s.trim().strip_prefix('#').unwrap_or(s.trim());
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("invalid #RRGGBB color: {s:?}"));
        }
        let r = u8::from_str_radix(&hex[0..2], 16).map_err(|e| e.to_string())?;
        let g = u8::from_str_radix(&hex[2..4], 16).map_err(|e| e.to_string())?;
        let b = u8::from_str_radix(&hex[4..6], 16).map_err(|e| e.to_string())?;
        Ok(Self { r, g, b })
    }
}

/// Policy for handling alpha channels when converting to an opaque target format (e.g. JPEG).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AlphaPolicy {
    pub background: Rgb8,
}

/// EXIF orientation handling policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrientationPolicy {
    /// Apply EXIF orientation 1..=8 to pixel data and normalize the EXIF orientation tag to 1.
    #[default]
    AutoOrient,
    /// Leave pixel grid and EXIF orientation tag unchanged.
    PreserveRaw,
}

/// ICC color profile handling policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IccPolicy {
    #[default]
    Preserve,
    Strip,
}

/// EXIF metadata handling policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExifPolicy {
    #[default]
    Strip,
    KeepSafe,
    PreserveAll,
}

/// Combined metadata handling policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MetadataPolicy {
    pub orientation: OrientationPolicy,
    pub icc: IccPolicy,
    pub exif: ExifPolicy,
}

/// Output directory placement rule.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum OutputLocation {
    #[default]
    SameDirectory,
    Subfolder(String),
    Directory(PathBuf),
}

/// File extension choice for JPEG outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JpegExt {
    #[default]
    Jpg,
    Jpeg,
}

impl JpegExt {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            JpegExt::Jpg => "jpg",
            JpegExt::Jpeg => "jpeg",
        }
    }
}

/// Filename derivation rule for converted outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum NamingPolicy {
    PreserveStem {
        #[serde(default)]
        jpeg_ext: JpegExt,
    },
    AppendSuffix {
        suffix: String,
        #[serde(default)]
        jpeg_ext: JpegExt,
    },
}

impl Default for NamingPolicy {
    fn default() -> Self {
        Self::PreserveStem {
            jpeg_ext: JpegExt::Jpg,
        }
    }
}

impl NamingPolicy {
    #[must_use]
    pub const fn jpeg_ext(&self) -> JpegExt {
        match self {
            NamingPolicy::PreserveStem { jpeg_ext }
            | NamingPolicy::AppendSuffix { jpeg_ext, .. } => *jpeg_ext,
        }
    }
}

/// Collision handling rule when the target path already exists (`ADR-034`, `D4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollisionPolicy {
    /// Append ` (1)`, ` (2)`, ... up to `max_collision_probes`.
    #[default]
    Suffix,
    /// Skip the item cleanly without modifying the existing destination file.
    Skip,
    /// Fail the item with `ConversionError::OutputExists`.
    Fail,
}

/// Complete output destination, naming, and collision policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputPolicy {
    pub location: OutputLocation,
    pub naming: NamingPolicy,
    pub collision: CollisionPolicy,
    pub max_collision_probes: u32,
    pub preserve_timestamps: bool,
}

impl Default for OutputPolicy {
    fn default() -> Self {
        Self {
            location: OutputLocation::SameDirectory,
            naming: NamingPolicy::default(),
            collision: CollisionPolicy::Suffix,
            max_collision_probes: 999,
            preserve_timestamps: false,
        }
    }
}

/// Behavior when the sniffed source format already matches `target` (`D8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SameFormatPolicy {
    #[default]
    Skip,
    Reencode,
}

/// Complete conversion options for a job or item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionOptions {
    pub target: FormatId,
    pub quality: QualitySetting,
    pub resize: ResizeSpec,
    pub alpha: AlphaPolicy,
    pub metadata: MetadataPolicy,
    pub output: OutputPolicy,
    pub same_format: SameFormatPolicy,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            target: FormatId::WebP,
            quality: QualitySetting::Intent(QualityIntent::Balanced),
            resize: ResizeSpec::Original,
            alpha: AlphaPolicy::default(),
            metadata: MetadataPolicy::default(),
            output: OutputPolicy::default(),
            same_format: SameFormatPolicy::Skip,
        }
    }
}

/// Severity of an [`OptionIssue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    Error,
}

/// Validation diagnostic produced by [`ConversionOptions::validate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionIssue {
    pub field: &'static str,
    pub message: String,
    pub severity: Severity,
}

/// Validated, immutable conversion options ready for planning and execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedOptions {
    inner: ConversionOptions,
    resolved_quality: FormatQuality,
    required_features: FeatureSet,
}

impl ValidatedOptions {
    /// Returns the underlying [`ConversionOptions`].
    #[must_use]
    pub const fn options(&self) -> &ConversionOptions {
        &self.inner
    }

    /// Returns the target format.
    #[must_use]
    pub const fn target(&self) -> FormatId {
        self.inner.target
    }

    /// Returns the concrete [`FormatQuality`] resolved for `target`.
    #[must_use]
    pub const fn resolved_quality(&self) -> FormatQuality {
        self.resolved_quality
    }

    /// Returns the [`FeatureSet`] required from a backend capability.
    #[must_use]
    pub const fn required_features(&self) -> FeatureSet {
        self.required_features
    }
}

impl std::ops::Deref for ValidatedOptions {
    type Target = ConversionOptions;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl ConversionOptions {
    /// Computes the [`FeatureSet`] required by these options.
    #[must_use]
    pub fn required_features(&self) -> FeatureSet {
        let mut features = FeatureSet::empty();
        let fq = self.quality.resolve_for_target(self.target);
        match fq {
            FormatQuality::Jpeg { progressive, .. } => {
                features |= FeatureSet::LOSSY_QUALITY;
                if progressive {
                    features |= FeatureSet::PROGRESSIVE_JPEG;
                }
            }
            FormatQuality::WebP {
                mode,
                alpha_quality,
                method,
                ..
            } => {
                match mode {
                    WebPMode::Lossy => features |= FeatureSet::LOSSY_QUALITY,
                    WebPMode::Lossless => features |= FeatureSet::LOSSLESS,
                }
                if alpha_quality != 100 {
                    features |= FeatureSet::WEBP_ALPHA_Q;
                }
                if method != 4 {
                    features |= FeatureSet::WEBP_METHOD;
                }
            }
            FormatQuality::Png { .. } => {
                features |= FeatureSet::LOSSLESS;
            }
        }

        if !matches!(self.resize, ResizeSpec::Original) {
            features |= FeatureSet::RESIZE;
        }
        if matches!(self.metadata.icc, IccPolicy::Preserve) {
            features |= FeatureSet::ICC_PRESERVE;
        }
        if !matches!(self.metadata.exif, ExifPolicy::Strip) {
            features |= FeatureSet::EXIF_PRESERVE;
        }
        if matches!(self.metadata.orientation, OrientationPolicy::AutoOrient) {
            features |= FeatureSet::EXIF_ORIENT;
        }
        features
    }

    /// Validates options against format traits and the [`CapabilityCatalog`].
    pub fn validate(
        &self,
        catalog: &CapabilityCatalog,
    ) -> Result<(ValidatedOptions, Vec<OptionIssue>), Vec<OptionIssue>> {
        let mut issues = Vec::new();
        let mut normalized = self.clone();

        // Check Lossless intent on JPEG (JPEG in v0.2.0 is lossy-only)
        if matches!(
            normalized.quality,
            QualitySetting::Intent(QualityIntent::Lossless)
        ) && normalized.target == FormatId::Jpeg
        {
            issues.push(OptionIssue {
                field: "quality",
                message:
                    "JPEG does not support true lossless compression; using quality 95 (4:4:4)"
                        .to_string(),
                severity: Severity::Warning,
            });
        }

        // Validate explicit PerFormat quality ranges
        if let QualitySetting::PerFormat(ref mut fq) = normalized.quality {
            if fq.format_id() != normalized.target {
                issues.push(OptionIssue {
                    field: "quality",
                    message: format!(
                        "per-format quality is for {}, but target format is {}",
                        fq.format_id(),
                        normalized.target
                    ),
                    severity: Severity::Error,
                });
            }
            match fq {
                FormatQuality::Jpeg { quality, .. } => {
                    if *quality == 0 || *quality > 100 {
                        issues.push(OptionIssue {
                            field: "quality.jpeg.quality",
                            message: format!("JPEG quality must be in 1..=100 (got {quality})"),
                            severity: Severity::Error,
                        });
                    }
                }
                FormatQuality::WebP {
                    quality,
                    alpha_quality,
                    method,
                    ..
                } => {
                    if *quality > 100 {
                        issues.push(OptionIssue {
                            field: "quality.webp.quality",
                            message: format!("WebP quality must be in 0..=100 (got {quality})"),
                            severity: Severity::Error,
                        });
                    }
                    if *alpha_quality > 100 {
                        issues.push(OptionIssue {
                            field: "quality.webp.alpha_quality",
                            message: format!(
                                "WebP alpha_quality must be in 0..=100 (got {alpha_quality})"
                            ),
                            severity: Severity::Error,
                        });
                    }
                    if *method > 6 {
                        issues.push(OptionIssue {
                            field: "quality.webp.method",
                            message: format!("WebP method must be in 0..=6 (got {method})"),
                            severity: Severity::Error,
                        });
                    }
                }
                FormatQuality::Png { .. } => {}
            }
        } else if let QualitySetting::Intent(QualityIntent::Custom(q)) = normalized.quality
            && (q == 0 || q > 100)
        {
            issues.push(OptionIssue {
                field: "quality",
                message: format!("custom quality must be in 1..=100 (got {q})"),
                severity: Severity::Error,
            });
        }

        // Validate resize dimensions against target format max_dimension
        let target_max_dim = normalized.target.descriptor().traits.max_dimension;
        match normalized.resize {
            ResizeSpec::Original => {}
            ResizeSpec::Width { px, .. } | ResizeSpec::Height { px, .. } => {
                if px.get() > target_max_dim {
                    issues.push(OptionIssue {
                        field: "resize",
                        message: format!(
                            "resize dimension {} exceeds {} maximum of {}",
                            px.get(),
                            normalized.target.descriptor().display_name,
                            target_max_dim
                        ),
                        severity: Severity::Error,
                    });
                }
            }
            ResizeSpec::Fit { width, height, .. } => {
                if width.get() > target_max_dim || height.get() > target_max_dim {
                    issues.push(OptionIssue {
                        field: "resize",
                        message: format!(
                            "resize dimensions {}x{} exceed {} maximum of {}",
                            width.get(),
                            height.get(),
                            normalized.target.descriptor().display_name,
                            target_max_dim
                        ),
                        severity: Severity::Error,
                    });
                }
            }
        }

        // Validate OutputLocation security rules (no '..', '/', '\', NUL in Subfolder; absolute path in Directory)
        match &normalized.output.location {
            OutputLocation::SameDirectory => {}
            OutputLocation::Subfolder(name) => {
                let trimmed = name.trim();
                if trimmed.is_empty()
                    || trimmed == "."
                    || trimmed == ".."
                    || trimmed.contains('/')
                    || trimmed.contains('\\')
                    || trimmed.contains('\0')
                    || trimmed.contains("..")
                {
                    issues.push(OptionIssue {
                        field: "output.location",
                        message: format!(
                            "subfolder name {name:?} must be a single relative directory component without '..' or path separators"
                        ),
                        severity: Severity::Error,
                    });
                }
            }
            OutputLocation::Directory(dir) => {
                if !dir.is_absolute() {
                    issues.push(OptionIssue {
                        field: "output.location",
                        message: format!(
                            "output directory {:?} must be an absolute path",
                            dir.display()
                        ),
                        severity: Severity::Error,
                    });
                }
            }
        }

        // Validate NamingPolicy::AppendSuffix has no path separators or NUL
        if let NamingPolicy::AppendSuffix { suffix, .. } = &normalized.output.naming
            && (suffix.contains('/') || suffix.contains('\\') || suffix.contains('\0'))
        {
            issues.push(OptionIssue {
                field: "output.naming",
                message: "filename suffix must not contain path separators or NUL".to_string(),
                severity: Severity::Error,
            });
        }

        if normalized.output.max_collision_probes == 0 {
            normalized.output.max_collision_probes = 999;
        }

        // Verify at least one backend in catalog supports target
        let target_supported = catalog.targets().into_iter().any(|(fmt, status)| {
            fmt == normalized.target
                && matches!(status, crate::backend::TargetStatus::Available { .. })
        });
        if !target_supported {
            issues.push(OptionIssue {
                field: "target",
                message: format!(
                    "no available backend supports target format {}",
                    normalized.target
                ),
                severity: Severity::Error,
            });
        }

        if issues.iter().any(|i| i.severity == Severity::Error) {
            return Err(issues);
        }

        let resolved_quality = normalized.quality.resolve_for_target(normalized.target);
        let required_features = normalized.required_features();

        Ok((
            ValidatedOptions {
                inner: normalized,
                resolved_quality,
                required_features,
            },
            issues,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{BackendDescriptor, BackendId, BackendKind, Capability, ProbeSpec};

    static TEST_CAPS: &[Capability] = &[
        Capability {
            from: FormatId::Png,
            to: FormatId::WebP,
            features: FeatureSet(0xFFFF),
            max_dimension: 16_383,
            max_pixels: 100_000_000,
            priority: 100,
        },
        Capability {
            from: FormatId::Png,
            to: FormatId::Jpeg,
            features: FeatureSet(0xFFFF),
            max_dimension: 65_535,
            max_pixels: 100_000_000,
            priority: 100,
        },
    ];

    fn test_catalog() -> CapabilityCatalog {
        CapabilityCatalog::builtin_only(BackendDescriptor {
            id: BackendId("builtin.image"),
            display_name: "Built-in Image",
            kind: BackendKind::BuiltIn,
            version: "0.2.0",
            capabilities: TEST_CAPS,
            probe: ProbeSpec::AlwaysAvailable,
            install_hint_debian: None,
        })
    }

    #[test]
    fn rgb8_hex_roundtrip() {
        let color: Rgb8 = "#1A2B3C".parse().unwrap();
        assert_eq!(
            color,
            Rgb8 {
                r: 0x1A,
                g: 0x2B,
                b: 0x3C
            }
        );
        assert_eq!(color.to_hex(), "#1A2B3C");
        assert!("invalid".parse::<Rgb8>().is_err());
    }

    #[test]
    fn lossless_on_jpeg_emits_warning_not_error() {
        let cat = test_catalog();
        let opts = ConversionOptions {
            target: FormatId::Jpeg,
            quality: QualitySetting::Intent(QualityIntent::Lossless),
            ..ConversionOptions::default()
        };
        let (validated, warnings) = opts.validate(&cat).expect("should validate with warning");
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].severity, Severity::Warning);
        assert_eq!(
            validated.resolved_quality(),
            FormatQuality::Jpeg {
                quality: 95,
                progressive: true,
                optimize_coding: true,
                subsampling: ChromaSubsampling::Cs444,
            }
        );
    }

    #[test]
    fn path_traversal_in_subfolder_rejected() {
        let cat = test_catalog();
        for bad in ["../escape", "a/b", "a\\b", "..", "", "."] {
            let opts = ConversionOptions {
                output: OutputPolicy {
                    location: OutputLocation::Subfolder(bad.to_string()),
                    ..OutputPolicy::default()
                },
                ..ConversionOptions::default()
            };
            assert!(
                opts.validate(&cat).is_err(),
                "subfolder {bad:?} must be rejected"
            );
        }
    }
}
