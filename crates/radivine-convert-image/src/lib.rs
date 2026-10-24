//! `radivine-convert-image` — Memory-safe built-in image conversion backend (`builtin.image`).
//!
//! Enforces `#![forbid(unsafe_code)]` (`ADR-028`, `ADR-030`):
//! - All decoding of untrusted PNG, JPEG, and WebP inputs happens in pure memory-safe Rust via
//!   `image` with strict `image::Limits` applied prior to pixel buffer allocation.
//! - PNG and lossless WebP encoding use `image::codecs::png::PngEncoder` and
//!   `image::codecs::webp::WebPEncoder`.
//! - JPEG encoding uses pure-Rust `jpeg_encoder::Encoder` (supporting baseline and progressive
//!   DCT, optimized Huffman coding, 4:4:4 / 4:2:2 / 4:2:0 chroma subsampling, ICC, and EXIF).
//! - Lossy WebP encoding delegates to the audited encode-only wrapper `radivine_webp::encode_lossy`
//!   plus pure-Rust RIFF/VP8X metadata muxing ([`riff::mux_webp_metadata`]).

#![forbid(unsafe_code)]

pub mod exif;
pub mod riff;

use std::io::{Read, Seek, SeekFrom, Write};

use image::codecs::jpeg::JpegDecoder;
use image::codecs::png::{CompressionType, FilterType as PngFilterType, PngDecoder, PngEncoder};
use image::codecs::webp::{WebPDecoder, WebPEncoder};
use image::error::{ImageError, LimitErrorKind};
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{
    ColorType, DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, Limits, RgbImage,
    RgbaImage,
};
use jpeg_encoder::SamplingFactor;
use radivine_convert::{
    BackendDescriptor, BackendId, BackendKind, BackendOutput, ChromaSubsampling, ConversionBackend,
    ConversionError, ExifPolicy, FeatureSet, FormatId, FormatQuality, FsOp, IccPolicy, ItemControl,
    Limit, OrientationPolicy, OutputSink, PngCompression, ProbeSpec, ResizeSpec, ResourceEstimate,
    ResourceLimitsConfig, Rgb8, SourceProbe, SourceReader, Stage, UnsupportedFeature,
    ValidatedOptions, Warning, WebPMode,
};
use radivine_webp::{MAX_WEBP_DIMENSION, WebPEncodeError, WebPEncodeInput, WebPLossyConfig};

/// Canonical backend identifier for the built-in image backend (`"builtin.image"`).
pub const BUILTIN_IMAGE_BACKEND_ID: BackendId = BackendId("builtin.image");

const ALL_IMAGE_FEATURES: FeatureSet = FeatureSet(
    FeatureSet::LOSSY_QUALITY.0
        | FeatureSet::LOSSLESS.0
        | FeatureSet::ALPHA.0
        | FeatureSet::RESIZE.0
        | FeatureSet::ICC_PRESERVE.0
        | FeatureSet::EXIF_PRESERVE.0
        | FeatureSet::EXIF_ORIENT.0
        | FeatureSet::PROGRESSIVE_JPEG.0
        | FeatureSet::WEBP_ALPHA_Q.0
        | FeatureSet::WEBP_METHOD.0,
);

static BUILTIN_CAPABILITIES: [radivine_convert::Capability; 9] = [
    // From PNG
    radivine_convert::Capability {
        from: FormatId::Png,
        to: FormatId::Png,
        features: ALL_IMAGE_FEATURES,
        max_dimension: 16_384,
        max_pixels: 100_000_000,
        priority: 100,
    },
    radivine_convert::Capability {
        from: FormatId::Png,
        to: FormatId::Jpeg,
        features: ALL_IMAGE_FEATURES,
        max_dimension: 16_384,
        max_pixels: 100_000_000,
        priority: 100,
    },
    radivine_convert::Capability {
        from: FormatId::Png,
        to: FormatId::WebP,
        features: ALL_IMAGE_FEATURES,
        max_dimension: MAX_WEBP_DIMENSION,
        max_pixels: 100_000_000,
        priority: 100,
    },
    // From JPEG
    radivine_convert::Capability {
        from: FormatId::Jpeg,
        to: FormatId::Png,
        features: ALL_IMAGE_FEATURES,
        max_dimension: 16_384,
        max_pixels: 100_000_000,
        priority: 100,
    },
    radivine_convert::Capability {
        from: FormatId::Jpeg,
        to: FormatId::Jpeg,
        features: ALL_IMAGE_FEATURES,
        max_dimension: 16_384,
        max_pixels: 100_000_000,
        priority: 100,
    },
    radivine_convert::Capability {
        from: FormatId::Jpeg,
        to: FormatId::WebP,
        features: ALL_IMAGE_FEATURES,
        max_dimension: MAX_WEBP_DIMENSION,
        max_pixels: 100_000_000,
        priority: 100,
    },
    // From WebP
    radivine_convert::Capability {
        from: FormatId::WebP,
        to: FormatId::Png,
        features: ALL_IMAGE_FEATURES,
        max_dimension: 16_384,
        max_pixels: 100_000_000,
        priority: 100,
    },
    radivine_convert::Capability {
        from: FormatId::WebP,
        to: FormatId::Jpeg,
        features: ALL_IMAGE_FEATURES,
        max_dimension: 16_384,
        max_pixels: 100_000_000,
        priority: 100,
    },
    radivine_convert::Capability {
        from: FormatId::WebP,
        to: FormatId::WebP,
        features: ALL_IMAGE_FEATURES,
        max_dimension: MAX_WEBP_DIMENSION,
        max_pixels: 100_000_000,
        priority: 100,
    },
];

/// Returns the static [`BackendDescriptor`] for `builtin.image`.
#[must_use]
pub fn descriptor() -> BackendDescriptor {
    BackendDescriptor {
        id: BUILTIN_IMAGE_BACKEND_ID,
        display_name: "Built-in Image Codec (image + libwebp)",
        kind: BackendKind::BuiltIn,
        version: env!("CARGO_PKG_VERSION"),
        capabilities: &BUILTIN_CAPABILITIES,
        probe: ProbeSpec::AlwaysAvailable,
        install_hint_debian: None,
    }
}

/// Built-in image conversion backend (`builtin.image`).
#[derive(Debug, Clone)]
pub struct ImageBackend {
    limits: ResourceLimitsConfig,
}

impl Default for ImageBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageBackend {
    /// Creates an [`ImageBackend`] with default [`ResourceLimitsConfig`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            limits: ResourceLimitsConfig::default(),
        }
    }

    /// Creates an [`ImageBackend`] with custom [`ResourceLimitsConfig`].
    #[must_use]
    pub fn with_limits(limits: ResourceLimitsConfig) -> Self {
        Self { limits }
    }

    fn image_limits(&self) -> Limits {
        let mut lim = Limits::default();
        lim.max_image_width = Some(self.limits.max_dimension);
        lim.max_image_height = Some(self.limits.max_dimension);
        lim.max_alloc = Some(self.limits.max_alloc_bytes);
        lim
    }
}

impl ConversionBackend for ImageBackend {
    fn id(&self) -> BackendId {
        BUILTIN_IMAGE_BACKEND_ID
    }

    fn inspect(
        &self,
        source: &mut SourceReader<'_>,
        sniffed: FormatId,
    ) -> Result<SourceProbe, ConversionError> {
        source
            .seek(SeekFrom::Start(0))
            .map_err(|e| ConversionError::CorruptInput {
                format: sniffed,
                detail: format!("failed to seek input stream: {e}"),
            })?;

        match sniffed {
            FormatId::Png => {
                // Pre-check PNG IHDR dimensions (bytes 16..24) and APNG acTL chunk before PngDecoder scans for IDAT
                let mut hdr64 = [0u8; 64];
                let n = source.read(&mut hdr64).unwrap_or(0);
                if n >= 24 && &hdr64[12..16] == b"IHDR" {
                    let w = u32::from_be_bytes([hdr64[16], hdr64[17], hdr64[18], hdr64[19]]);
                    let h = u32::from_be_bytes([hdr64[20], hdr64[21], hdr64[22], hdr64[23]]);
                    check_dimensions_against_limits(w, h, &self.limits)?;
                    if n >= 41 && &hdr64[37..41] == b"acTL" {
                        return Err(ConversionError::UnsupportedFeature {
                            feature: UnsupportedFeature::Animation,
                        });
                    }
                }
                source
                    .seek(SeekFrom::Start(0))
                    .map_err(|e| ConversionError::CorruptInput {
                        format: FormatId::Png,
                        detail: format!("failed to rewind PNG stream: {e}"),
                    })?;

                let mut dec = PngDecoder::new(source)
                    .map_err(|e| map_image_error(e, FormatId::Png, &self.limits))?;
                let (width, height) = dec.dimensions();
                if width == 0 || height == 0 {
                    return Err(ConversionError::CorruptInput {
                        format: FormatId::Png,
                        detail: "zero-dimension PNG".to_string(),
                    });
                }
                let is_animated = dec.is_apng().unwrap_or(false);
                if is_animated {
                    return Err(ConversionError::UnsupportedFeature {
                        feature: UnsupportedFeature::Animation,
                    });
                }
                let color = dec.color_type();
                let bit_depth = color_type_bit_depth(color);
                let has_icc = dec
                    .icc_profile()
                    .ok()
                    .flatten()
                    .is_some_and(|v| !v.is_empty());
                let exif_bytes = dec.exif_metadata().ok().flatten();
                let has_exif = exif_bytes.as_ref().is_some_and(|v| !v.is_empty());
                let orientation = exif_bytes
                    .as_deref()
                    .and_then(exif::parse_orientation)
                    .unwrap_or(1);

                Ok(SourceProbe {
                    format: FormatId::Png,
                    width,
                    height,
                    has_alpha: color.has_alpha(),
                    is_animated,
                    orientation,
                    has_icc,
                    has_exif,
                    bit_depth,
                })
            }
            FormatId::Jpeg => {
                // Pre-scan JPEG markers before SOS for arithmetic coding (SOF9/10/11), 12-bit precision, and dimension bombs
                let mut buf = Vec::new();
                source
                    .read_to_end(&mut buf)
                    .map_err(|e| ConversionError::CorruptInput {
                        format: FormatId::Jpeg,
                        detail: format!("failed to read JPEG stream: {e}"),
                    })?;
                check_jpeg_markers(&buf, &self.limits)?;

                let mut dec = JpegDecoder::new(std::io::Cursor::new(&buf))
                    .map_err(|e| map_image_error(e, FormatId::Jpeg, &self.limits))?;
                let (width, height) = dec.dimensions();
                if width == 0 || height == 0 {
                    return Err(ConversionError::CorruptInput {
                        format: FormatId::Jpeg,
                        detail: "zero-dimension JPEG".to_string(),
                    });
                }
                let has_icc = dec
                    .icc_profile()
                    .ok()
                    .flatten()
                    .is_some_and(|v| !v.is_empty());
                let exif_bytes = dec.exif_metadata().ok().flatten();
                let has_exif = exif_bytes.as_ref().is_some_and(|v| !v.is_empty());
                let orientation = exif_bytes
                    .as_deref()
                    .and_then(exif::parse_orientation)
                    .unwrap_or(1);

                Ok(SourceProbe {
                    format: FormatId::Jpeg,
                    width,
                    height,
                    has_alpha: false,
                    is_animated: false,
                    orientation,
                    has_icc,
                    has_exif,
                    bit_depth: 8,
                })
            }
            FormatId::WebP => {
                let mut buf = Vec::new();
                source
                    .read_to_end(&mut buf)
                    .map_err(|e| ConversionError::CorruptInput {
                        format: FormatId::WebP,
                        detail: format!("failed to read WebP stream: {e}"),
                    })?;
                let container_info = riff::inspect_webp_container(&buf)?;
                if container_info.is_animated {
                    return Err(ConversionError::UnsupportedFeature {
                        feature: UnsupportedFeature::Animation,
                    });
                }

                let mut dec = WebPDecoder::new(std::io::Cursor::new(&buf))
                    .map_err(|e| map_image_error(e, FormatId::WebP, &self.limits))?;
                let (width, height) = dec.dimensions();
                if width == 0 || height == 0 {
                    return Err(ConversionError::CorruptInput {
                        format: FormatId::WebP,
                        detail: "zero-dimension WebP".to_string(),
                    });
                }
                let is_animated = dec.has_animation() || container_info.is_animated;
                let color = dec.color_type();
                let has_icc = dec
                    .icc_profile()
                    .ok()
                    .flatten()
                    .is_some_and(|v| !v.is_empty())
                    || container_info.has_icc;
                let exif_bytes = dec.exif_metadata().ok().flatten();
                let has_exif =
                    exif_bytes.as_ref().is_some_and(|v| !v.is_empty()) || container_info.has_exif;
                let orientation = exif_bytes
                    .as_deref()
                    .and_then(exif::parse_orientation)
                    .unwrap_or(1);

                Ok(SourceProbe {
                    format: FormatId::WebP,
                    width,
                    height,
                    has_alpha: color.has_alpha() || container_info.has_alpha,
                    is_animated,
                    orientation,
                    has_icc,
                    has_exif,
                    bit_depth: 8,
                })
            }
            other => Err(ConversionError::CorruptInput {
                format: other,
                detail: format!("unsupported format {other} for builtin.image"),
            }),
        }
    }

    fn estimate_resources(
        &self,
        probe: &SourceProbe,
        _opts: &ValidatedOptions,
    ) -> ResourceEstimate {
        let decoded_rgba = u64::from(probe.width)
            .saturating_mul(u64::from(probe.height))
            .saturating_mul(4);
        // Spike P7 calibrated upper bound: 2.2 * decoded_rgba + 16 MiB
        let peak_ram_bytes =
            (decoded_rgba.saturating_mul(22) / 10).saturating_add(16 * 1024 * 1024);
        let temp_disk_bytes = (decoded_rgba / 2).saturating_add(64 * 1024);
        ResourceEstimate {
            peak_ram_bytes,
            temp_disk_bytes,
        }
    }

    fn convert(
        &self,
        source: &mut SourceReader<'_>,
        probe: &SourceProbe,
        opts: &ValidatedOptions,
        sink: &mut OutputSink<'_>,
        ctrl: &mut dyn ItemControl,
    ) -> Result<BackendOutput, ConversionError> {
        ctrl.check_cancelled()?;
        ctrl.on_stage(Stage::Decoding);

        source
            .seek(SeekFrom::Start(0))
            .map_err(|e| ConversionError::CorruptInput {
                format: probe.format,
                detail: format!("failed to rewind source stream: {e}"),
            })?;

        let (mut img, raw_icc, raw_exif) = self.decode_with_metadata(source, probe.format)?;
        ctrl.check_cancelled()?;
        ctrl.on_stage(Stage::Transforming);

        let mut warnings = Vec::new();

        // 1. EXIF orientation handling (1..=8)
        let mut applied_auto_orient = false;
        let orient_val = raw_exif
            .as_deref()
            .and_then(exif::parse_orientation)
            .unwrap_or(probe.orientation);
        if opts.metadata.orientation == OrientationPolicy::AutoOrient
            && (2..=8).contains(&orient_val)
            && let Some(orient) = Orientation::from_exif(orient_val)
        {
            img.apply_orientation(orient);
            applied_auto_orient = true;
            warnings.push(Warning::OrientationNormalized { from: orient_val });
        }

        // 2. Metadata policy filtering (ICC & EXIF)
        let out_icc = match (opts.metadata.icc, raw_icc) {
            (IccPolicy::Preserve, Some(icc)) if !icc.is_empty() => Some(icc),
            (IccPolicy::Strip, Some(icc)) if !icc.is_empty() => {
                warnings.push(Warning::IccStrippedByPolicy);
                None
            }
            _ => None,
        };

        let out_exif = match (opts.metadata.exif, raw_exif) {
            (ExifPolicy::Strip, Some(ex)) if !ex.is_empty() => {
                warnings.push(Warning::ExifStrippedByPolicy);
                None
            }
            (ExifPolicy::KeepSafe, Some(ex)) if !ex.is_empty() => {
                exif::sanitize_exif_keep_safe(&ex, applied_auto_orient)
            }
            (ExifPolicy::PreserveAll, Some(ex)) if !ex.is_empty() => {
                let mut normalized = exif::normalize_exif_tiff_slice(&ex).to_vec();
                if applied_auto_orient {
                    let _ = exif::reset_orientation_in_place(&mut normalized);
                }
                Some(normalized)
            }
            _ => None,
        };

        // 3. Resize geometry (Lanczos3, no-upscale default)
        let cur_w = img.width();
        let cur_h = img.height();
        if let Some((target_w, target_h)) =
            compute_resize_dimensions(cur_w, cur_h, opts.resize, &mut warnings)
        {
            let target_max_dim = if opts.target() == FormatId::WebP {
                MAX_WEBP_DIMENSION.min(self.limits.max_dimension)
            } else {
                self.limits.max_dimension
            };
            if target_w > target_max_dim || target_h > target_max_dim {
                return Err(ConversionError::LimitExceeded {
                    limit: Limit::Dimensions {
                        width: target_w,
                        height: target_h,
                        max: target_max_dim,
                    },
                });
            }
            img = img.resize_exact(target_w, target_h, FilterType::Lanczos3);
        } else if opts.target() == FormatId::WebP
            && (img.width() > MAX_WEBP_DIMENSION || img.height() > MAX_WEBP_DIMENSION)
        {
            return Err(ConversionError::LimitExceeded {
                limit: Limit::Dimensions {
                    width: img.width(),
                    height: img.height(),
                    max: MAX_WEBP_DIMENSION,
                },
            });
        }

        let output_width = img.width();
        let output_height = img.height();

        ctrl.check_cancelled()?;
        ctrl.on_stage(Stage::Encoding);

        // 4. Target-specific encoding
        match opts.resolved_quality() {
            FormatQuality::Png { compression } => {
                self.encode_png(img, compression, out_icc, out_exif, sink)?;
            }
            FormatQuality::Jpeg {
                quality,
                progressive,
                optimize_coding,
                subsampling,
            } => {
                let (rgb_img, had_translucent_pixel) =
                    flatten_alpha_to_rgb8(img, opts.alpha.background);
                if had_translucent_pixel {
                    warnings.push(Warning::AlphaFlattened {
                        background: opts.alpha.background,
                    });
                }
                self.encode_jpeg(
                    &rgb_img,
                    quality,
                    progressive,
                    optimize_coding,
                    subsampling,
                    out_icc.as_deref(),
                    out_exif.as_deref(),
                    sink,
                )?;
            }
            FormatQuality::WebP {
                mode,
                quality,
                alpha_quality,
                method,
            } => match mode {
                WebPMode::Lossless => {
                    self.encode_webp_lossless(img, out_icc, out_exif, sink)?;
                }
                WebPMode::Lossy => {
                    self.encode_webp_lossy(
                        img,
                        quality,
                        alpha_quality,
                        method,
                        out_icc.as_deref(),
                        out_exif.as_deref(),
                        sink,
                        ctrl,
                    )?;
                }
            },
        }

        Ok(BackendOutput {
            output_width,
            output_height,
            warnings,
        })
    }
}

type DecodedWithMetadata = (DynamicImage, Option<Vec<u8>>, Option<Vec<u8>>);

impl ImageBackend {
    fn decode_with_metadata(
        &self,
        source: &mut SourceReader<'_>,
        format: FormatId,
    ) -> Result<DecodedWithMetadata, ConversionError> {
        let limits = self.image_limits();
        match format {
            FormatId::Png => {
                let mut dec = PngDecoder::new(source)
                    .map_err(|e| map_image_error(e, FormatId::Png, &self.limits))?;
                if dec.is_apng().unwrap_or(false) {
                    return Err(ConversionError::UnsupportedFeature {
                        feature: UnsupportedFeature::Animation,
                    });
                }
                dec.set_limits(limits)
                    .map_err(|e| map_image_error(e, FormatId::Png, &self.limits))?;
                let icc = dec.icc_profile().ok().flatten().filter(|v| !v.is_empty());
                let exif = dec.exif_metadata().ok().flatten().filter(|v| !v.is_empty());
                let img = DynamicImage::from_decoder(dec)
                    .map_err(|e| map_image_error(e, FormatId::Png, &self.limits))?;
                Ok((img, icc, exif))
            }
            FormatId::Jpeg => {
                let mut buf = Vec::new();
                source
                    .read_to_end(&mut buf)
                    .map_err(|e| ConversionError::CorruptInput {
                        format: FormatId::Jpeg,
                        detail: format!("failed to read JPEG stream: {e}"),
                    })?;
                check_jpeg_markers(&buf, &self.limits)?;

                let mut dec = JpegDecoder::new(std::io::Cursor::new(&buf))
                    .map_err(|e| map_image_error(e, FormatId::Jpeg, &self.limits))?;
                dec.set_limits(limits)
                    .map_err(|e| map_image_error(e, FormatId::Jpeg, &self.limits))?;
                let icc = dec.icc_profile().ok().flatten().filter(|v| !v.is_empty());
                let exif = dec.exif_metadata().ok().flatten().filter(|v| !v.is_empty());
                let img = DynamicImage::from_decoder(dec)
                    .map_err(|e| map_image_error(e, FormatId::Jpeg, &self.limits))?;
                Ok((img, icc, exif))
            }
            FormatId::WebP => {
                let mut buf = Vec::new();
                source
                    .read_to_end(&mut buf)
                    .map_err(|e| ConversionError::CorruptInput {
                        format: FormatId::WebP,
                        detail: format!("failed to read WebP stream: {e}"),
                    })?;
                let container_info = riff::inspect_webp_container(&buf)?;
                if container_info.is_animated {
                    return Err(ConversionError::UnsupportedFeature {
                        feature: UnsupportedFeature::Animation,
                    });
                }

                let mut dec = WebPDecoder::new(std::io::Cursor::new(&buf))
                    .map_err(|e| map_image_error(e, FormatId::WebP, &self.limits))?;
                if dec.has_animation() {
                    return Err(ConversionError::UnsupportedFeature {
                        feature: UnsupportedFeature::Animation,
                    });
                }
                dec.set_limits(limits)
                    .map_err(|e| map_image_error(e, FormatId::WebP, &self.limits))?;
                let icc = dec.icc_profile().ok().flatten().filter(|v| !v.is_empty());
                let exif = dec.exif_metadata().ok().flatten().filter(|v| !v.is_empty());
                let img = DynamicImage::from_decoder(dec)
                    .map_err(|e| map_image_error(e, FormatId::WebP, &self.limits))?;
                Ok((img, icc, exif))
            }
            other => Err(ConversionError::CorruptInput {
                format: other,
                detail: format!("unsupported format {other}"),
            }),
        }
    }

    fn encode_png(
        &self,
        img: DynamicImage,
        compression: PngCompression,
        icc: Option<Vec<u8>>,
        exif: Option<Vec<u8>>,
        sink: &mut OutputSink<'_>,
    ) -> Result<(), ConversionError> {
        let comp_type = match compression {
            PngCompression::Fast => CompressionType::Fast,
            PngCompression::Default => CompressionType::Default,
            PngCompression::Best => CompressionType::Best,
        };
        let mut enc = PngEncoder::new_with_quality(sink, comp_type, PngFilterType::Adaptive);
        if let Some(icc_bytes) = icc {
            let _ = enc.set_icc_profile(icc_bytes);
        }
        if let Some(exif_bytes) = exif {
            let _ = enc.set_exif_metadata(exif_bytes);
        }

        if img.color().has_alpha() {
            let rgba = img.into_rgba8();
            enc.write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                ExtendedColorType::Rgba8,
            )
        } else {
            let rgb = img.into_rgb8();
            enc.write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                ExtendedColorType::Rgb8,
            )
        }
        .map_err(|e| ConversionError::BackendFailure {
            backend: BUILTIN_IMAGE_BACKEND_ID,
            detail: format!("PNG encoding failed: {e}"),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_jpeg(
        &self,
        rgb: &RgbImage,
        quality: u8,
        progressive: bool,
        optimize_coding: bool,
        subsampling: ChromaSubsampling,
        icc: Option<&[u8]>,
        exif: Option<&[u8]>,
        sink: &mut OutputSink<'_>,
    ) -> Result<(), ConversionError> {
        let width_u16 = u16::try_from(rgb.width()).map_err(|_| ConversionError::LimitExceeded {
            limit: Limit::Dimensions {
                width: rgb.width(),
                height: rgb.height(),
                max: 65_535,
            },
        })?;
        let height_u16 =
            u16::try_from(rgb.height()).map_err(|_| ConversionError::LimitExceeded {
                limit: Limit::Dimensions {
                    width: rgb.width(),
                    height: rgb.height(),
                    max: 65_535,
                },
            })?;

        let mut enc = jpeg_encoder::Encoder::new(sink, quality.clamp(1, 100));
        enc.set_progressive(progressive);
        enc.set_optimized_huffman_tables(optimize_coding);

        let sf = match subsampling {
            ChromaSubsampling::Cs444 => SamplingFactor::R_4_4_4,
            ChromaSubsampling::Cs422 => SamplingFactor::R_4_2_2,
            ChromaSubsampling::Cs420 => SamplingFactor::R_4_2_0,
            ChromaSubsampling::Auto => {
                if quality >= 90 {
                    SamplingFactor::R_4_4_4
                } else {
                    SamplingFactor::R_4_2_0
                }
            }
        };
        enc.set_sampling_factor(sf);

        if let Some(icc_bytes) = icc {
            enc.add_icc_profile(icc_bytes)
                .map_err(|e| ConversionError::BackendFailure {
                    backend: BUILTIN_IMAGE_BACKEND_ID,
                    detail: format!("failed to embed ICC profile in JPEG: {e}"),
                })?;
        }
        if let Some(exif_bytes) = exif {
            let clean_tiff = exif::normalize_exif_tiff_slice(exif_bytes);
            if clean_tiff.len() <= 65_527 {
                enc.add_exif_metadata(clean_tiff)
                    .map_err(|e| ConversionError::BackendFailure {
                        backend: BUILTIN_IMAGE_BACKEND_ID,
                        detail: format!("failed to embed EXIF metadata in JPEG: {e}"),
                    })?;
            }
        }

        enc.encode(
            rgb.as_raw(),
            width_u16,
            height_u16,
            jpeg_encoder::ColorType::Rgb,
        )
        .map_err(|e| ConversionError::BackendFailure {
            backend: BUILTIN_IMAGE_BACKEND_ID,
            detail: format!("JPEG encoding failed: {e}"),
        })
    }

    fn encode_webp_lossless(
        &self,
        img: DynamicImage,
        icc: Option<Vec<u8>>,
        exif: Option<Vec<u8>>,
        sink: &mut OutputSink<'_>,
    ) -> Result<(), ConversionError> {
        let mut enc = WebPEncoder::new_lossless(sink);
        if let Some(icc_bytes) = icc {
            let _ = enc.set_icc_profile(icc_bytes);
        }
        if let Some(exif_bytes) = exif {
            let _ = enc.set_exif_metadata(exif_bytes);
        }

        if img.color().has_alpha() {
            let rgba = img.into_rgba8();
            enc.write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                ExtendedColorType::Rgba8,
            )
        } else {
            let rgb = img.into_rgb8();
            enc.write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                ExtendedColorType::Rgb8,
            )
        }
        .map_err(|e| ConversionError::BackendFailure {
            backend: BUILTIN_IMAGE_BACKEND_ID,
            detail: format!("lossless WebP encoding failed: {e}"),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_webp_lossy(
        &self,
        img: DynamicImage,
        quality: u8,
        alpha_quality: u8,
        method: u8,
        icc: Option<&[u8]>,
        exif: Option<&[u8]>,
        sink: &mut OutputSink<'_>,
        ctrl: &mut dyn ItemControl,
    ) -> Result<(), ConversionError> {
        let width = img.width();
        let height = img.height();
        let input = if img.color().has_alpha() {
            let rgba: RgbaImage = img.into_rgba8();
            // Only encode an alpha plane if at least one pixel has alpha < 255
            if rgba.pixels().any(|p| p.0[3] < 255) {
                WebPEncodeInput::Rgba {
                    pixels: rgba.into_raw(),
                    width,
                    height,
                }
            } else {
                let rgb = DynamicImage::ImageRgba8(rgba).into_rgb8();
                WebPEncodeInput::Rgb {
                    pixels: rgb.into_raw(),
                    width,
                    height,
                }
            }
        } else {
            let rgb = img.into_rgb8();
            WebPEncodeInput::Rgb {
                pixels: rgb.into_raw(),
                width,
                height,
            }
        };

        let cfg = WebPLossyConfig {
            quality,
            alpha_quality,
            method,
        };

        let webp_raw =
            radivine_webp::encode_lossy(input, &cfg, &mut |_pct| ctrl.check_cancelled().is_err())
                .map_err(|e| match e {
                WebPEncodeError::Cancelled => ConversionError::Cancelled,
                WebPEncodeError::InvalidDimensions { width, height } => {
                    ConversionError::LimitExceeded {
                        limit: Limit::Dimensions {
                            width,
                            height,
                            max: MAX_WEBP_DIMENSION,
                        },
                    }
                }
                WebPEncodeError::OutOfMemory => ConversionError::LimitExceeded {
                    limit: Limit::MemoryBytes {
                        needed: self.limits.max_alloc_bytes.saturating_add(1),
                        max: self.limits.max_alloc_bytes,
                    },
                },
                other => ConversionError::BackendFailure {
                    backend: BUILTIN_IMAGE_BACKEND_ID,
                    detail: other.to_string(),
                },
            })?;

        let final_webp = riff::mux_webp_metadata(&webp_raw, width, height, icc, exif)?;
        sink.write_all(&final_webp)
            .map_err(|source| ConversionError::Io {
                op: FsOp::WriteTemp,
                path: std::path::PathBuf::from("<temp.part>"),
                source,
            })
    }
}

/// Flattens an image onto `bg` in sRGB byte space in a single pass.
///
/// Returns `(rgb_image, had_any_translucent_pixel)` where `had_any_translucent_pixel` is `true`
/// if and only if at least one pixel had `alpha < 255` (`AC-C1.4`).
fn flatten_alpha_to_rgb8(img: DynamicImage, bg: Rgb8) -> (RgbImage, bool) {
    if !img.color().has_alpha() {
        return (img.into_rgb8(), false);
    }

    let rgba = img.into_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    let src = rgba.as_raw();
    let mut dst = vec![0u8; (width as usize) * (height as usize) * 3];
    let mut had_translucent = false;

    let bg_r = u32::from(bg.r);
    let bg_g = u32::from(bg.g);
    let bg_b = u32::from(bg.b);

    for (rgba_px, rgb_out) in src.chunks_exact(4).zip(dst.chunks_exact_mut(3)) {
        let a = u32::from(rgba_px[3]);
        if a == 255 {
            rgb_out[0] = rgba_px[0];
            rgb_out[1] = rgba_px[1];
            rgb_out[2] = rgba_px[2];
        } else {
            had_translucent = true;
            if a == 0 {
                rgb_out[0] = bg.r;
                rgb_out[1] = bg.g;
                rgb_out[2] = bg.b;
            } else {
                let inv_a = 255 - a;
                rgb_out[0] = ((u32::from(rgba_px[0]) * a + bg_r * inv_a + 127) / 255) as u8;
                rgb_out[1] = ((u32::from(rgba_px[1]) * a + bg_g * inv_a + 127) / 255) as u8;
                rgb_out[2] = ((u32::from(rgba_px[2]) * a + bg_b * inv_a + 127) / 255) as u8;
            }
        }
    }

    let out_img = RgbImage::from_raw(width, height, dst).expect("exact RGB buffer length");
    (out_img, had_translucent)
}

/// Computes target `(new_w, new_h)` for `resize_spec`, returning `None` if no resize should be performed.
fn compute_resize_dimensions(
    cur_w: u32,
    cur_h: u32,
    resize_spec: ResizeSpec,
    warnings: &mut Vec<Warning>,
) -> Option<(u32, u32)> {
    match resize_spec {
        ResizeSpec::Original => None,
        ResizeSpec::Width { px, allow_upscale } => {
            let target_w = px.get();
            if !allow_upscale && cur_w <= target_w {
                if cur_w < target_w {
                    warnings.push(Warning::KeptOriginalSize {
                        width: cur_w,
                        height: cur_h,
                        requested: resize_spec,
                    });
                }
                return None;
            }
            if target_w == cur_w {
                return None;
            }
            let scale = f64::from(target_w) / f64::from(cur_w);
            let target_h = ((f64::from(cur_h) * scale).round() as u32).max(1);
            Some((target_w, target_h))
        }
        ResizeSpec::Height { px, allow_upscale } => {
            let target_h = px.get();
            if !allow_upscale && cur_h <= target_h {
                if cur_h < target_h {
                    warnings.push(Warning::KeptOriginalSize {
                        width: cur_w,
                        height: cur_h,
                        requested: resize_spec,
                    });
                }
                return None;
            }
            if target_h == cur_h {
                return None;
            }
            let scale = f64::from(target_h) / f64::from(cur_h);
            let target_w = ((f64::from(cur_w) * scale).round() as u32).max(1);
            Some((target_w, target_h))
        }
        ResizeSpec::Fit {
            width,
            height,
            allow_upscale,
        } => {
            let box_w = width.get();
            let box_h = height.get();
            if !allow_upscale && cur_w <= box_w && cur_h <= box_h {
                if cur_w < box_w && cur_h < box_h {
                    warnings.push(Warning::KeptOriginalSize {
                        width: cur_w,
                        height: cur_h,
                        requested: resize_spec,
                    });
                }
                return None;
            }
            let scale_w = f64::from(box_w) / f64::from(cur_w);
            let scale_h = f64::from(box_h) / f64::from(cur_h);
            let scale = scale_w.min(scale_h);
            let target_w = ((f64::from(cur_w) * scale).round() as u32).clamp(1, box_w);
            let target_h = ((f64::from(cur_h) * scale).round() as u32).clamp(1, box_h);
            if target_w == cur_w && target_h == cur_h {
                None
            } else {
                Some((target_w, target_h))
            }
        }
    }
}

const fn color_type_bit_depth(color: ColorType) -> u8 {
    match color {
        ColorType::L8 | ColorType::La8 | ColorType::Rgb8 | ColorType::Rgba8 => 8,
        ColorType::L16 | ColorType::La16 | ColorType::Rgb16 | ColorType::Rgba16 => 16,
        ColorType::Rgb32F | ColorType::Rgba32F => 32,
        _ => 8,
    }
}

fn check_dimensions_against_limits(
    width: u32,
    height: u32,
    limits: &ResourceLimitsConfig,
) -> Result<(), ConversionError> {
    if width > limits.max_dimension || height > limits.max_dimension {
        return Err(ConversionError::LimitExceeded {
            limit: Limit::Dimensions {
                width,
                height,
                max: limits.max_dimension,
            },
        });
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > limits.max_pixels {
        return Err(ConversionError::LimitExceeded {
            limit: Limit::PixelCount {
                pixels,
                max: limits.max_pixels,
            },
        });
    }
    Ok(())
}

/// Scans JPEG segments prior to SOS (`0xFFDA`) to detect arithmetic-coded SOF markers
/// (`0xFFC9`..=`0xFFCB`, `0xFFCD`..=`0xFFCF`), non-8-bit sample precision (`SOF` precision != 8),
/// and dimension bombs.
fn check_jpeg_markers(data: &[u8], limits: &ResourceLimitsConfig) -> Result<(), ConversionError> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(ConversionError::CorruptInput {
            format: FormatId::Jpeg,
            detail: "missing JPEG SOI marker".to_string(),
        });
    }
    let mut pos = 2usize;
    while pos + 1 < data.len() {
        if data[pos] != 0xFF {
            pos += 1;
            continue;
        }
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        if pos >= data.len() {
            break;
        }
        let marker = data[pos];
        pos += 1;

        if marker == 0xD9 || marker == 0xDA {
            // EOI or SOS reached
            break;
        }
        if marker == 0x00 || (0xD0..=0xD8).contains(&marker) {
            continue;
        }
        if pos + 2 > data.len() {
            break;
        }
        let seg_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        if seg_len < 2 || pos + seg_len > data.len() {
            break;
        }

        // Arithmetic coding SOF markers: SOF9 (0xC9), SOF10 (0xCA), SOF11 (0xCB), SOF13..15 (0xCD..0xCF)
        if matches!(marker, 0xC9..=0xCB | 0xCD..=0xCF) {
            return Err(ConversionError::UnsupportedFeature {
                feature: UnsupportedFeature::ArithmeticCoding,
            });
        }
        // Standard SOF0..SOF3 (0xC0..=0xC3): check sample precision and dimensions
        if matches!(marker, 0xC0..=0xC3) && seg_len >= 8 {
            let precision = data[pos + 2];
            if precision != 8 {
                return Err(ConversionError::UnsupportedFeature {
                    feature: UnsupportedFeature::BitDepth(precision),
                });
            }
            let h = u32::from(u16::from_be_bytes([data[pos + 3], data[pos + 4]]));
            let w = u32::from(u16::from_be_bytes([data[pos + 5], data[pos + 6]]));
            check_dimensions_against_limits(w, h, limits)?;
        }

        pos += seg_len;
    }
    Ok(())
}

fn map_image_error(
    err: ImageError,
    format: FormatId,
    limits: &ResourceLimitsConfig,
) -> ConversionError {
    match err {
        ImageError::Limits(lim_err) => match lim_err.kind() {
            LimitErrorKind::DimensionError => ConversionError::LimitExceeded {
                limit: Limit::Dimensions {
                    width: limits.max_dimension.saturating_add(1),
                    height: limits.max_dimension.saturating_add(1),
                    max: limits.max_dimension,
                },
            },
            LimitErrorKind::InsufficientMemory => ConversionError::LimitExceeded {
                limit: Limit::MemoryBytes {
                    needed: limits.max_alloc_bytes.saturating_add(1),
                    max: limits.max_alloc_bytes,
                },
            },
            _ => ConversionError::LimitExceeded {
                limit: Limit::MemoryBytes {
                    needed: limits.max_alloc_bytes.saturating_add(1),
                    max: limits.max_alloc_bytes,
                },
            },
        },
        ImageError::Unsupported(unsup) => ConversionError::UnsupportedFeature {
            feature: UnsupportedFeature::TargetFeature(unsup.to_string()),
        },
        other => ConversionError::CorruptInput {
            format,
            detail: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use radivine_convert::{
        AlphaPolicy, CapabilityCatalog, CollisionPolicy, ConversionEngine, ConversionOptions,
        ConvertSettings, MetadataPolicy, NoopControl, OutputPolicy, PresetRef, QualityIntent,
        QualitySetting, SameFormatPolicy, SourceSpec, builtin_presets,
    };
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::num::NonZeroU32;
    use std::sync::Arc;

    fn sha256_hex(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    fn test_engine() -> ConversionEngine {
        let catalog = CapabilityCatalog::builtin_only(descriptor());
        let backend = Arc::new(ImageBackend::new());
        ConversionEngine::new(catalog, vec![backend], ResourceLimitsConfig::default())
    }

    fn make_photo_rgb(width: u32, height: u32) -> RgbImage {
        let mut img = RgbImage::new(width, height);
        let mut state = 0x1234_5678u32;
        for (x, y, px) in img.enumerate_pixels_mut() {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let n = (state & 0x1F) as u8;
            let r = ((x * 255) / width.max(1)) as u8;
            let g = ((y * 255) / height.max(1)) as u8;
            let b = ((x ^ y) & 0xFF) as u8;
            *px = image::Rgb([r.wrapping_add(n), g.wrapping_add(n), b.wrapping_sub(n >> 1)]);
        }
        img
    }

    /// `AC-C1.1`: All 9 `(PNG, JPEG, WebP) × (PNG, JPEG, WebP)` pairs convert and decode cleanly,
    /// and all 4 built-in presets produce valid output.
    #[test]
    fn ac_c1_1_all_nine_pairs_and_four_builtin_presets() {
        let engine = test_engine();
        let tmp = tempfile::tempdir().unwrap();
        let rgb = make_photo_rgb(96, 64);

        let src_png = tmp.path().join("src.png");
        let src_jpg = tmp.path().join("src.jpg");
        let src_webp = tmp.path().join("src.webp");

        {
            let mut f = fs::File::create(&src_png).unwrap();
            PngEncoder::new(&mut f)
                .write_image(rgb.as_raw(), 96, 64, ExtendedColorType::Rgb8)
                .unwrap();
        }
        {
            let mut f = fs::File::create(&src_jpg).unwrap();
            jpeg_encoder::Encoder::new(&mut f, 85)
                .encode(rgb.as_raw(), 96, 64, jpeg_encoder::ColorType::Rgb)
                .unwrap();
        }
        {
            let mut f = fs::File::create(&src_webp).unwrap();
            WebPEncoder::new_lossless(&mut f)
                .write_image(rgb.as_raw(), 96, 64, ExtendedColorType::Rgb8)
                .unwrap();
        }

        let sources = [
            (FormatId::Png, &src_png),
            (FormatId::Jpeg, &src_jpg),
            (FormatId::WebP, &src_webp),
        ];

        for &(src_fmt, src_path) in &sources {
            for &dst_fmt in FormatId::ALL {
                let sub = tmp.path().join(format!("{src_fmt}_to_{dst_fmt}"));
                fs::create_dir_all(&sub).unwrap();
                let item_src = sub.join(src_path.file_name().unwrap());
                fs::copy(src_path, &item_src).unwrap();

                let opts = ConversionOptions {
                    target: dst_fmt,
                    same_format: SameFormatPolicy::Reencode,
                    ..ConversionOptions::default()
                };
                let (validated, _) = opts.validate(engine.catalog()).unwrap();
                let plan = engine.plan(&[SourceSpec::from_path(&item_src)], &validated);
                assert_eq!(plan.items.len(), 1);
                let res = engine
                    .convert_item(&plan.items[0], &validated, &mut NoopControl)
                    .unwrap();
                let out_path = res.output_path.unwrap();
                assert!(out_path.exists());
                assert_eq!(res.output_width, 96);
                assert_eq!(res.output_height, 64);

                let probe = engine.inspect(&out_path).unwrap();
                assert_eq!(probe.format, dst_fmt);
                assert_eq!((probe.width, probe.height), (96, 64));
            }
        }

        // Verify all 4 built-in presets
        let settings = ConvertSettings::default();
        for preset in builtin_presets() {
            let sub = tmp.path().join(format!("preset_{}", preset.id));
            fs::create_dir_all(&sub).unwrap();
            let item_src = sub.join("input.png");
            fs::copy(&src_png, &item_src).unwrap();

            let mut opts = settings
                .resolve(&PresetRef::PresetId(preset.id.clone()), None)
                .unwrap();
            opts.same_format = SameFormatPolicy::Reencode;
            let (validated, _) = opts.validate(engine.catalog()).unwrap();
            let plan = engine.plan(&[SourceSpec::from_path(&item_src)], &validated);
            let res = engine
                .convert_item(&plan.items[0], &validated, &mut NoopControl)
                .unwrap();
            assert!(res.output_path.unwrap().exists());
        }
    }

    /// `AC-C1.2`: Lossy WebP quality monotonicity (`q=60 < q=80 < q=92`) and lossless WebP pixel-exact round-trip.
    #[test]
    fn ac_c1_2_webp_lossy_monotonicity_and_lossless_pixel_exact() {
        let engine = test_engine();
        let tmp = tempfile::tempdir().unwrap();
        let photo = make_photo_rgb(256, 192);
        let src_png = tmp.path().join("photo.png");
        {
            let mut f = fs::File::create(&src_png).unwrap();
            PngEncoder::new(&mut f)
                .write_image(photo.as_raw(), 256, 192, ExtendedColorType::Rgb8)
                .unwrap();
        }

        let mut sizes = Vec::new();
        for &q in &[60u8, 80, 92] {
            let opts = ConversionOptions {
                target: FormatId::WebP,
                quality: QualitySetting::PerFormat(FormatQuality::WebP {
                    mode: WebPMode::Lossy,
                    quality: q,
                    alpha_quality: 90,
                    method: 4,
                }),
                ..ConversionOptions::default()
            };
            let (validated, _) = opts.validate(engine.catalog()).unwrap();
            let plan = engine.plan(&[SourceSpec::from_path(&src_png)], &validated);
            let res = engine
                .convert_item(&plan.items[0], &validated, &mut NoopControl)
                .unwrap();
            sizes.push(res.output_bytes);
            fs::remove_file(res.output_path.unwrap()).unwrap();
        }
        assert!(
            sizes[0] < sizes[1] && sizes[1] < sizes[2],
            "expected monotonic lossy WebP sizes for q=60,80,92, got {sizes:?}"
        );

        // Lossless WebP pixel-exact round-trip
        let opts_ll = ConversionOptions {
            target: FormatId::WebP,
            quality: QualitySetting::Intent(QualityIntent::Lossless),
            ..ConversionOptions::default()
        };
        let (validated_ll, _) = opts_ll.validate(engine.catalog()).unwrap();
        let plan_ll = engine.plan(&[SourceSpec::from_path(&src_png)], &validated_ll);
        let res_ll = engine
            .convert_item(&plan_ll.items[0], &validated_ll, &mut NoopControl)
            .unwrap();
        let decoded_ll = image::open(res_ll.output_path.unwrap())
            .unwrap()
            .into_rgb8();
        assert_eq!(decoded_ll.as_raw(), photo.as_raw());
    }

    /// `AC-C1.3`: EXIF orientations `1..=8` auto-oriented and orientation tag reset to `1`;
    /// `PreserveRaw` leaves pixel grid and orientation tag untouched.
    #[test]
    fn ac_c1_3_exif_orientations_1_to_8_and_preserve_raw() {
        let engine = test_engine();
        let tmp = tempfile::tempdir().unwrap();

        for orient_val in 1u8..=8 {
            let mut img = RgbImage::new(60, 40);
            img.put_pixel(0, 0, image::Rgb([250, 10, 10]));
            let mut exif_tiff = Vec::new();
            exif_tiff.extend_from_slice(b"II\x2a\x00\x08\x00\x00\x00\x01\x00");
            exif_tiff.extend_from_slice(&0x0112u16.to_le_bytes());
            exif_tiff.extend_from_slice(&3u16.to_le_bytes());
            exif_tiff.extend_from_slice(&1u32.to_le_bytes());
            exif_tiff.extend_from_slice(&u16::from(orient_val).to_le_bytes());
            exif_tiff.extend_from_slice(&0u16.to_le_bytes());
            exif_tiff.extend_from_slice(&0u32.to_le_bytes());

            let src_path = tmp.path().join(format!("orient_{orient_val}.png"));
            {
                let mut f = fs::File::create(&src_path).unwrap();
                let mut enc = PngEncoder::new(&mut f);
                enc.set_exif_metadata(exif_tiff.clone()).unwrap();
                enc.write_image(img.as_raw(), 60, 40, ExtendedColorType::Rgb8)
                    .unwrap();
            }

            // 1. AutoOrient + PreserveAll -> oriented dimensions and EXIF orientation tag == 1
            let opts = ConversionOptions {
                target: FormatId::WebP,
                metadata: MetadataPolicy {
                    orientation: OrientationPolicy::AutoOrient,
                    icc: IccPolicy::Preserve,
                    exif: ExifPolicy::PreserveAll,
                },
                ..ConversionOptions::default()
            };
            let (validated, _) = opts.validate(engine.catalog()).unwrap();
            let plan = engine.plan(&[SourceSpec::from_path(&src_path)], &validated);
            let res = engine
                .convert_item(&plan.items[0], &validated, &mut NoopControl)
                .unwrap();
            let expected_dims = if orient_val <= 4 { (60, 40) } else { (40, 60) };
            assert_eq!((res.output_width, res.output_height), expected_dims);
            let out_path = res.output_path.unwrap();
            let out_probe = engine.inspect(&out_path).unwrap();
            assert_eq!(
                out_probe.orientation, 1,
                "orientation tag must be reset to 1"
            );
            fs::remove_file(&out_path).unwrap();

            // 2. PreserveRaw + PreserveAll -> keeps (60, 40) and keeps original orientation tag
            let opts_raw = ConversionOptions {
                target: FormatId::WebP,
                metadata: MetadataPolicy {
                    orientation: OrientationPolicy::PreserveRaw,
                    icc: IccPolicy::Preserve,
                    exif: ExifPolicy::PreserveAll,
                },
                ..ConversionOptions::default()
            };
            let (validated_raw, _) = opts_raw.validate(engine.catalog()).unwrap();
            let plan_raw = engine.plan(&[SourceSpec::from_path(&src_path)], &validated_raw);
            let res_raw = engine
                .convert_item(&plan_raw.items[0], &validated_raw, &mut NoopControl)
                .unwrap();
            assert_eq!((res_raw.output_width, res_raw.output_height), (60, 40));
            let out_raw_path = res_raw.output_path.unwrap();
            let raw_probe = engine.inspect(&out_raw_path).unwrap();
            assert_eq!(raw_probe.orientation, orient_val);
        }
    }

    /// `AC-C1.4`: Alpha handling (translucent RGBA -> JPEG flattens onto configured background and
    /// emits `AlphaFlattened`; fully opaque RGBA -> JPEG does NOT emit `AlphaFlattened`;
    /// RGBA -> WebP preserves alpha).
    #[test]
    fn ac_c1_4_alpha_flattening_and_preservation() {
        let engine = test_engine();
        let tmp = tempfile::tempdir().unwrap();

        // 1. Fully transparent 16x16 RGBA -> JPEG with custom background #1040C0
        let mut trans_rgba = RgbaImage::new(16, 16);
        for p in trans_rgba.pixels_mut() {
            *p = image::Rgba([0, 0, 0, 0]);
        }
        let trans_png = tmp.path().join("trans.png");
        trans_rgba.save(&trans_png).unwrap();

        let bg = Rgb8 {
            r: 16,
            g: 64,
            b: 192,
        };
        let opts_jpg = ConversionOptions {
            target: FormatId::Jpeg,
            quality: QualitySetting::Intent(QualityIntent::Maximum),
            alpha: AlphaPolicy { background: bg },
            ..ConversionOptions::default()
        };
        let (val_jpg, _) = opts_jpg.validate(engine.catalog()).unwrap();
        let plan = engine.plan(&[SourceSpec::from_path(&trans_png)], &val_jpg);
        let res = engine
            .convert_item(&plan.items[0], &val_jpg, &mut NoopControl)
            .unwrap();
        assert!(
            res.warnings
                .contains(&Warning::AlphaFlattened { background: bg })
        );
        let decoded_jpg = image::open(res.output_path.unwrap()).unwrap().into_rgb8();
        let px = decoded_jpg.get_pixel(8, 8).0;
        assert!((i16::from(px[0]) - 16).abs() <= 2);
        assert!((i16::from(px[1]) - 64).abs() <= 2);
        assert!((i16::from(px[2]) - 192).abs() <= 2);

        // 2. Opaque RGBA (all alpha == 255) -> JPEG must NOT emit AlphaFlattened warning
        let mut opaque_rgba = RgbaImage::new(16, 16);
        for p in opaque_rgba.pixels_mut() {
            *p = image::Rgba([100, 150, 200, 255]);
        }
        let opaque_png = tmp.path().join("opaque_rgba.png");
        opaque_rgba.save(&opaque_png).unwrap();
        let plan_op = engine.plan(&[SourceSpec::from_path(&opaque_png)], &val_jpg);
        let res_op = engine
            .convert_item(&plan_op.items[0], &val_jpg, &mut NoopControl)
            .unwrap();
        assert!(
            !res_op
                .warnings
                .iter()
                .any(|w| matches!(w, Warning::AlphaFlattened { .. })),
            "fully opaque RGBA must not emit AlphaFlattened warning"
        );

        // 3. Translucent RGBA -> WebP (lossy & lossless) preserves alpha
        let opts_webp = ConversionOptions {
            target: FormatId::WebP,
            ..ConversionOptions::default()
        };
        let (val_webp, _) = opts_webp.validate(engine.catalog()).unwrap();
        let plan_w = engine.plan(&[SourceSpec::from_path(&trans_png)], &val_webp);
        let res_w = engine
            .convert_item(&plan_w.items[0], &val_webp, &mut NoopControl)
            .unwrap();
        let probe_w = engine.inspect(&res_w.output_path.unwrap()).unwrap();
        assert!(probe_w.has_alpha);
    }

    /// `AC-C1.5`: ICC profile preserved byte-for-byte (SHA-256 verified) across all 9 pairs,
    /// and stripped when `IccPolicy::Strip`.
    #[test]
    fn ac_c1_5_icc_preservation_and_stripping_across_all_pairs() {
        let engine = test_engine();
        let tmp = tempfile::tempdir().unwrap();
        let icc_payload: Vec<u8> = (0..600u32).map(|i| ((i * 73 + 19) & 0xFF) as u8).collect();
        let icc_hash = sha256_hex(&icc_payload);
        let rgb = make_photo_rgb(48, 32);

        let src_png = tmp.path().join("icc.png");
        {
            let mut f = fs::File::create(&src_png).unwrap();
            let mut enc = PngEncoder::new(&mut f);
            enc.set_icc_profile(icc_payload.clone()).unwrap();
            enc.write_image(rgb.as_raw(), 48, 32, ExtendedColorType::Rgb8)
                .unwrap();
        }

        for &target in FormatId::ALL {
            let sub = tmp.path().join(format!("icc_to_{target}"));
            fs::create_dir_all(&sub).unwrap();
            let in_file = sub.join("in.png");
            fs::copy(&src_png, &in_file).unwrap();

            let opts = ConversionOptions {
                target,
                same_format: SameFormatPolicy::Reencode,
                metadata: MetadataPolicy {
                    orientation: OrientationPolicy::AutoOrient,
                    icc: IccPolicy::Preserve,
                    exif: ExifPolicy::Strip,
                },
                ..ConversionOptions::default()
            };
            let (validated, _) = opts.validate(engine.catalog()).unwrap();
            let plan = engine.plan(&[SourceSpec::from_path(&in_file)], &validated);
            let res = engine
                .convert_item(&plan.items[0], &validated, &mut NoopControl)
                .unwrap();
            let out_path = res.output_path.unwrap();
            let out_bytes = fs::read(&out_path).unwrap();
            let extracted_icc = match target {
                FormatId::Png => PngDecoder::new(std::io::Cursor::new(&out_bytes))
                    .unwrap()
                    .icc_profile()
                    .unwrap()
                    .unwrap(),
                FormatId::Jpeg => JpegDecoder::new(std::io::Cursor::new(&out_bytes))
                    .unwrap()
                    .icc_profile()
                    .unwrap()
                    .unwrap(),
                FormatId::WebP => WebPDecoder::new(std::io::Cursor::new(&out_bytes))
                    .unwrap()
                    .icc_profile()
                    .unwrap()
                    .unwrap(),
                _ => unreachable!(),
            };
            assert_eq!(sha256_hex(&extracted_icc), icc_hash);

            // Strip policy removes ICC and records Warning::IccStrippedByPolicy
            let opts_strip = ConversionOptions {
                target,
                same_format: SameFormatPolicy::Reencode,
                metadata: MetadataPolicy {
                    orientation: OrientationPolicy::AutoOrient,
                    icc: IccPolicy::Strip,
                    exif: ExifPolicy::Strip,
                },
                ..ConversionOptions::default()
            };
            let (val_strip, _) = opts_strip.validate(engine.catalog()).unwrap();
            let plan_s = engine.plan(&[SourceSpec::from_path(&in_file)], &val_strip);
            let res_s = engine
                .convert_item(&plan_s.items[0], &val_strip, &mut NoopControl)
                .unwrap();
            assert!(res_s.warnings.contains(&Warning::IccStrippedByPolicy));
            let probe_s = engine.inspect(&res_s.output_path.unwrap()).unwrap();
            assert!(!probe_s.has_icc);
        }
    }

    /// `AC-C1.6`: Resize modes (`Width`, `Height`, `Fit`, no-upscale default with `KeptOriginalSize`).
    #[test]
    fn ac_c1_6_resize_modes_and_no_upscale_default() {
        let engine = test_engine();
        let tmp = tempfile::tempdir().unwrap();
        let rgb = make_photo_rgb(400, 200);
        let src = tmp.path().join("wide.png");
        rgb.save(&src).unwrap();

        // 1. Fit(100, 100) on 400x200 -> 100x50
        let opts_fit = ConversionOptions {
            target: FormatId::WebP,
            resize: ResizeSpec::Fit {
                width: NonZeroU32::new(100).unwrap(),
                height: NonZeroU32::new(100).unwrap(),
                allow_upscale: false,
            },
            ..ConversionOptions::default()
        };
        let (val_fit, _) = opts_fit.validate(engine.catalog()).unwrap();
        let plan = engine.plan(&[SourceSpec::from_path(&src)], &val_fit);
        let res = engine
            .convert_item(&plan.items[0], &val_fit, &mut NoopControl)
            .unwrap();
        assert_eq!((res.output_width, res.output_height), (100, 50));
        fs::remove_file(res.output_path.unwrap()).unwrap();

        // 2. Fit(1000, 1000, allow_upscale=false) on 400x200 -> keeps 400x200 and emits KeptOriginalSize
        let opts_noupscale = ConversionOptions {
            target: FormatId::WebP,
            resize: ResizeSpec::Fit {
                width: NonZeroU32::new(1000).unwrap(),
                height: NonZeroU32::new(1000).unwrap(),
                allow_upscale: false,
            },
            ..ConversionOptions::default()
        };
        let (val_no, _) = opts_noupscale.validate(engine.catalog()).unwrap();
        let plan_no = engine.plan(&[SourceSpec::from_path(&src)], &val_no);
        let res_no = engine
            .convert_item(&plan_no.items[0], &val_no, &mut NoopControl)
            .unwrap();
        assert_eq!((res_no.output_width, res_no.output_height), (400, 200));
        assert!(
            res_no
                .warnings
                .iter()
                .any(|w| matches!(w, Warning::KeptOriginalSize { .. }))
        );
    }

    /// `AC-C1.7` & `AC-C1.8`: Animated WebP/APNG rejected with `UnsupportedFeature::Animation`,
    /// and `65535×65535` bomb headers rejected with `LimitExceeded`.
    #[test]
    fn ac_c1_7_and_c1_8_animated_and_dimension_bomb_rejection() {
        let engine = test_engine();
        let tmp = tempfile::tempdir().unwrap();

        // 1. Animated WebP (VP8X with animation flag 1 << 1 and ANIM chunk)
        let mut anim_webp = Vec::new();
        anim_webp.extend_from_slice(b"RIFF");
        anim_webp.extend_from_slice(&36u32.to_le_bytes());
        anim_webp.extend_from_slice(b"WEBP");
        anim_webp.extend_from_slice(b"VP8X");
        anim_webp.extend_from_slice(&10u32.to_le_bytes());
        anim_webp.extend_from_slice(&[0b0000_0010, 0, 0, 0, 15, 0, 0, 15, 0, 0]);
        anim_webp.extend_from_slice(b"ANIM");
        anim_webp.extend_from_slice(&6u32.to_le_bytes());
        anim_webp.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let anim_path = tmp.path().join("anim.webp");
        fs::write(&anim_path, &anim_webp).unwrap();

        let err_anim = engine.inspect(&anim_path).unwrap_err();
        assert!(matches!(
            err_anim,
            ConversionError::UnsupportedFeature {
                feature: UnsupportedFeature::Animation
            }
        ));

        // 2. 65535x65535 PNG bomb header
        let mut png_bomb = Vec::new();
        png_bomb.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        png_bomb.extend_from_slice(&13u32.to_be_bytes());
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(b"IHDR");
        ihdr.extend_from_slice(&65535u32.to_be_bytes());
        ihdr.extend_from_slice(&65535u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in &ihdr {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if (crc & 1) != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        png_bomb.extend_from_slice(&ihdr);
        png_bomb.extend_from_slice(&(!crc).to_be_bytes());
        png_bomb.extend_from_slice(&0u32.to_be_bytes());
        png_bomb.extend_from_slice(b"IEND\xae\x42\x60\x82");
        let bomb_path = tmp.path().join("bomb.png");
        fs::write(&bomb_path, &png_bomb).unwrap();

        let opts = ConversionOptions::default();
        let (validated, _) = opts.validate(engine.catalog()).unwrap();
        let plan = engine.plan(&[SourceSpec::from_path(&bomb_path)], &validated);
        let err_bomb = engine
            .convert_item(&plan.items[0], &validated, &mut NoopControl)
            .unwrap_err();
        assert!(matches!(err_bomb, ConversionError::LimitExceeded { .. }));
    }

    /// `AC-C1.9` & `AC-C1.10`: SameFormatPolicy (`Skip` vs `Reencode`) and CollisionPolicy (`Suffix`, `Skip`, `Fail`).
    #[test]
    fn ac_c1_9_and_c1_10_same_format_and_collision_policies() {
        let engine = test_engine();
        let tmp = tempfile::tempdir().unwrap();
        let rgb = make_photo_rgb(32, 32);
        let src_png = tmp.path().join("sample.png");
        rgb.save(&src_png).unwrap();

        // 1. SameFormatPolicy::Skip skips PNG -> PNG in plan()
        let opts_skip_same = ConversionOptions {
            target: FormatId::Png,
            same_format: SameFormatPolicy::Skip,
            ..ConversionOptions::default()
        };
        let (val_skip_same, _) = opts_skip_same.validate(engine.catalog()).unwrap();
        let plan_same = engine.plan(&[SourceSpec::from_path(&src_png)], &val_skip_same);
        assert!(plan_same.items.is_empty());
        assert_eq!(plan_same.skipped.len(), 1);

        // 2. CollisionPolicy::Suffix creates sample.webp, sample (1).webp, sample (2).webp
        let opts_suffix = ConversionOptions {
            target: FormatId::WebP,
            output: OutputPolicy {
                collision: CollisionPolicy::Suffix,
                ..OutputPolicy::default()
            },
            ..ConversionOptions::default()
        };
        let (val_suffix, _) = opts_suffix.validate(engine.catalog()).unwrap();
        let plan_s = engine.plan(&[SourceSpec::from_path(&src_png)], &val_suffix);
        let r0 = engine
            .convert_item(&plan_s.items[0], &val_suffix, &mut NoopControl)
            .unwrap();
        let r1 = engine
            .convert_item(&plan_s.items[0], &val_suffix, &mut NoopControl)
            .unwrap();
        let r2 = engine
            .convert_item(&plan_s.items[0], &val_suffix, &mut NoopControl)
            .unwrap();
        assert_eq!(r0.output_path.unwrap(), tmp.path().join("sample.webp"));
        assert_eq!(r1.output_path.unwrap(), tmp.path().join("sample (1).webp"));
        assert_eq!(r2.output_path.unwrap(), tmp.path().join("sample (2).webp"));

        // 3. CollisionPolicy::Fail returns OutputExists without modifying sample.webp
        let orig_bytes = fs::read(tmp.path().join("sample.webp")).unwrap();
        let opts_fail = ConversionOptions {
            target: FormatId::WebP,
            output: OutputPolicy {
                collision: CollisionPolicy::Fail,
                ..OutputPolicy::default()
            },
            ..ConversionOptions::default()
        };
        let (val_fail, _) = opts_fail.validate(engine.catalog()).unwrap();
        let plan_f = engine.plan(&[SourceSpec::from_path(&src_png)], &val_fail);
        let err_f = engine
            .convert_item(&plan_f.items[0], &val_fail, &mut NoopControl)
            .unwrap_err();
        assert!(matches!(err_f, ConversionError::OutputExists { .. }));
        assert_eq!(
            fs::read(tmp.path().join("sample.webp")).unwrap(),
            orig_bytes
        );

        // 4. CollisionPolicy::Skip skips cleanly
        let opts_skip = ConversionOptions {
            target: FormatId::WebP,
            output: OutputPolicy {
                collision: CollisionPolicy::Skip,
                ..OutputPolicy::default()
            },
            ..ConversionOptions::default()
        };
        let (val_skip, _) = opts_skip.validate(engine.catalog()).unwrap();
        let res_skip = engine
            .convert_item(&plan_f.items[0], &val_skip, &mut NoopControl)
            .unwrap();
        assert!(res_skip.skipped_existing);
        assert!(res_skip.output_path.is_none());
    }

    /// `AC-C1.12` & edge cases: mid-encode cancellation leaves zero partial files on disk;
    /// 12-bit and arithmetic JPEG headers return typed `UnsupportedFeature`;
    /// mid-conversion source mutation returns `SourceModifiedDuringConversion`.
    #[test]
    fn ac_c1_12_cancellation_and_unsupported_jpeg_and_source_race() {
        use radivine_convert::{
            CommitPermit, CommitRequest, ItemControl, Reservation, ResourceEstimate, Stage,
            StageCreatedJournal, StageIntentJournal, TempCreatedJournal,
        };
        use std::sync::atomic::{AtomicBool, Ordering};

        let engine = test_engine();
        let tmp = tempfile::tempdir().unwrap();
        let rgb = make_photo_rgb(256, 256);
        let src_png = tmp.path().join("cancel_me.png");
        rgb.save(&src_png).unwrap();

        struct CancelDuringEncode {
            cancel_flag: AtomicBool,
        }
        impl ItemControl for CancelDuringEncode {
            fn check_cancelled(&self) -> Result<(), ConversionError> {
                if self.cancel_flag.load(Ordering::Relaxed) {
                    return Err(ConversionError::Cancelled);
                }
                Ok(())
            }
            fn on_stage(&mut self, stage: Stage) {
                if stage == Stage::Encoding {
                    self.cancel_flag.store(true, Ordering::Relaxed);
                }
            }
            fn reserve(
                &mut self,
                _estimate: ResourceEstimate,
            ) -> Result<Reservation, ConversionError> {
                Ok(Reservation::empty())
            }
            fn record_stage_intent(
                &mut self,
                _stage: &StageIntentJournal<'_>,
            ) -> Result<(), ConversionError> {
                Ok(())
            }
            fn record_stage_created(
                &mut self,
                _stage: &StageCreatedJournal<'_>,
            ) -> Result<(), ConversionError> {
                Ok(())
            }
            fn record_temp_created(
                &mut self,
                _temp: &TempCreatedJournal<'_>,
            ) -> Result<(), ConversionError> {
                Ok(())
            }
            fn acquire_commit_permit<'a>(
                &mut self,
                req: &CommitRequest<'a>,
            ) -> Result<CommitPermit<'a>, ConversionError> {
                Ok(CommitPermit::issue(req))
            }
        }

        let opts = ConversionOptions::default();
        let (validated, _) = opts.validate(engine.catalog()).unwrap();
        let plan = engine.plan(&[SourceSpec::from_path(&src_png)], &validated);
        let err_cancel = engine
            .convert_item(
                &plan.items[0],
                &validated,
                &mut CancelDuringEncode {
                    cancel_flag: AtomicBool::new(false),
                },
            )
            .unwrap_err();
        assert!(matches!(err_cancel, ConversionError::Cancelled));
        assert!(!tmp.path().join("cancel_me.webp").exists());
        for entry in fs::read_dir(tmp.path()).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(
                !name.starts_with(".rvstage-"),
                "no staging directory may remain after cancellation: {name}"
            );
        }

        // 12-bit JPEG SOF1 (0xFFC1, length 11, precision 12, height 16, width 16, 1 component)
        let jpeg_12bit = [
            0xFF, 0xD8, 0xFF, 0xC1, 0x00, 0x0B, 12, 0x00, 16, 0x00, 16, 1, 1, 0x11, 0, 0xFF, 0xD9,
        ];
        let p_12bit = tmp.path().join("12bit.jpg");
        fs::write(&p_12bit, jpeg_12bit).unwrap();
        let err_12 = engine.inspect(&p_12bit).unwrap_err();
        assert!(matches!(
            err_12,
            ConversionError::UnsupportedFeature {
                feature: UnsupportedFeature::BitDepth(12)
            }
        ));

        // Arithmetic-coded JPEG SOF9 (0xFFC9)
        let jpeg_arith = [
            0xFF, 0xD8, 0xFF, 0xC9, 0x00, 0x0B, 8, 0x00, 16, 0x00, 16, 1, 1, 0x11, 0, 0xFF, 0xD9,
        ];
        let p_arith = tmp.path().join("arith.jpg");
        fs::write(&p_arith, jpeg_arith).unwrap();
        let err_arith = engine.inspect(&p_arith).unwrap_err();
        assert!(matches!(
            err_arith,
            ConversionError::UnsupportedFeature {
                feature: UnsupportedFeature::ArithmeticCoding
            }
        ));

        // Mid-conversion source file modification race detection
        struct MutateSourceBeforeCommit {
            src: std::path::PathBuf,
        }
        impl ItemControl for MutateSourceBeforeCommit {
            fn check_cancelled(&self) -> Result<(), ConversionError> {
                Ok(())
            }
            fn on_stage(&mut self, stage: Stage) {
                if stage == Stage::ValidatingOutput {
                    // Append a byte to the source file before pre-commit verification!
                    let mut f = fs::OpenOptions::new().append(true).open(&self.src).unwrap();
                    std::io::Write::write_all(&mut f, b"X").unwrap();
                }
            }
            fn reserve(
                &mut self,
                _estimate: ResourceEstimate,
            ) -> Result<Reservation, ConversionError> {
                Ok(Reservation::empty())
            }
            fn record_stage_intent(
                &mut self,
                _stage: &StageIntentJournal<'_>,
            ) -> Result<(), ConversionError> {
                Ok(())
            }
            fn record_stage_created(
                &mut self,
                _stage: &StageCreatedJournal<'_>,
            ) -> Result<(), ConversionError> {
                Ok(())
            }
            fn record_temp_created(
                &mut self,
                _temp: &TempCreatedJournal<'_>,
            ) -> Result<(), ConversionError> {
                Ok(())
            }
            fn acquire_commit_permit<'a>(
                &mut self,
                req: &CommitRequest<'a>,
            ) -> Result<CommitPermit<'a>, ConversionError> {
                Ok(CommitPermit::issue(req))
            }
        }

        let race_png = tmp.path().join("race.png");
        rgb.save(&race_png).unwrap();
        let plan_r = engine.plan(&[SourceSpec::from_path(&race_png)], &validated);
        let err_race = engine
            .convert_item(
                &plan_r.items[0],
                &validated,
                &mut MutateSourceBeforeCommit {
                    src: race_png.clone(),
                },
            )
            .unwrap_err();
        assert!(matches!(
            err_race,
            ConversionError::SourceModifiedDuringConversion { .. }
        ));
        assert!(!tmp.path().join("race.webp").exists());
    }
}
