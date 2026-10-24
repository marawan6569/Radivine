//! Pure-Rust, codec-free image header inspection (`crates/radivine-convert/src/header_probe.rs`).
//!
//! Used by the Advanced Convert window (`radivine-control convert`) to inspect up to 1,000 image
//! headers asynchronously without linking any pixel decoders or decoding image bitstreams (`AC-C4.5`).

use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::engine::SourceProbe;
use crate::error::{ConversionError, FsOp, Limit, UnsupportedFeature};
use crate::format::{FormatId, sniff_format};
use crate::settings::ResourceLimitsConfig;

static HEADER_INSPECTIONS: AtomicU64 = AtomicU64::new(0);
static PIXEL_DECODES: AtomicU64 = AtomicU64::new(0);

/// Returns the total number of header-only inspections performed in this process (`AC-C4.5`).
#[must_use]
pub fn header_inspection_count() -> u64 {
    HEADER_INSPECTIONS.load(Ordering::Relaxed)
}

/// Returns the number of pixel decode operations performed by `header_probe` (always `0`, `AC-C4.5`).
#[must_use]
pub fn pixel_decode_count() -> u64 {
    PIXEL_DECODES.load(Ordering::Relaxed)
}

/// Resets the header inspection counter (for deterministic test assertions).
pub fn reset_header_inspection_count() {
    HEADER_INSPECTIONS.store(0, Ordering::Relaxed);
}

/// Inspects a file's magic bytes and container/chunk headers without decoding pixels (`AC-C4.5`).
pub fn inspect_file_header(
    path: &Path,
    limits: &ResourceLimitsConfig,
) -> Result<SourceProbe, ConversionError> {
    HEADER_INSPECTIONS.fetch_add(1, Ordering::Relaxed);

    let sym_meta = fs::symlink_metadata(path).map_err(|source| ConversionError::Io {
        op: FsOp::StatSource,
        path: path.to_path_buf(),
        source,
    })?;
    let meta = if sym_meta.file_type().is_symlink() {
        fs::metadata(path).map_err(|source| ConversionError::Io {
            op: FsOp::StatSource,
            path: path.to_path_buf(),
            source,
        })?
    } else {
        sym_meta
    };

    if !meta.file_type().is_file() {
        return Err(ConversionError::Io {
            op: FsOp::StatSource,
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file"),
        });
    }

    let size = meta.len();
    if size > limits.max_source_bytes {
        return Err(ConversionError::LimitExceeded {
            limit: Limit::SourceFileBytes {
                size,
                max: limits.max_source_bytes,
            },
        });
    }

    let file = File::open(path).map_err(|source| ConversionError::Io {
        op: FsOp::OpenSource,
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    inspect_reader_header(&mut reader, limits).map_err(|err| match err {
        ConversionError::UnknownInputFormat { .. } => ConversionError::UnknownInputFormat {
            path: path.to_path_buf(),
        },
        other => other,
    })
}

/// Inspects an in-memory or seekable stream's magic bytes and container headers without decoding pixels.
pub fn inspect_reader_header<R: Read + Seek>(
    reader: &mut R,
    limits: &ResourceLimitsConfig,
) -> Result<SourceProbe, ConversionError> {
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|e| ConversionError::CorruptInput {
            format: FormatId::Png,
            detail: format!("failed to seek input stream: {e}"),
        })?;

    let mut magic = [0u8; 32];
    let n = read_up_to(reader, &mut magic).map_err(|e| ConversionError::CorruptInput {
        format: FormatId::Png,
        detail: format!("failed to read header bytes: {e}"),
    })?;
    let Some(format) = sniff_format(&magic[..n]) else {
        return Err(ConversionError::UnknownInputFormat {
            path: std::path::PathBuf::from(format!(
                "<stream:{}>",
                hex_preview(&magic[..n.min(16)])
            )),
        });
    };

    reader
        .seek(SeekFrom::Start(0))
        .map_err(|e| ConversionError::CorruptInput {
            format,
            detail: format!("failed to rewind input stream: {e}"),
        })?;

    match format {
        FormatId::Png => inspect_png_header(reader, limits),
        FormatId::Jpeg => inspect_jpeg_header(reader, limits),
        FormatId::WebP => inspect_webp_header(reader, limits),
    }
}

fn inspect_png_header<R: Read + Seek>(
    reader: &mut R,
    limits: &ResourceLimitsConfig,
) -> Result<SourceProbe, ConversionError> {
    let mut sig = [0u8; 8];
    reader
        .read_exact(&mut sig)
        .map_err(|e| ConversionError::CorruptInput {
            format: FormatId::Png,
            detail: format!("truncated PNG signature: {e}"),
        })?;
    if &sig != b"\x89PNG\r\n\x1a\n" {
        return Err(ConversionError::CorruptInput {
            format: FormatId::Png,
            detail: "invalid PNG signature".to_string(),
        });
    }

    // First chunk must be IHDR (13 bytes)
    let mut chunk_hdr = [0u8; 8];
    reader
        .read_exact(&mut chunk_hdr)
        .map_err(|e| ConversionError::CorruptInput {
            format: FormatId::Png,
            detail: format!("missing IHDR chunk: {e}"),
        })?;
    let ihdr_len = u32::from_be_bytes([chunk_hdr[0], chunk_hdr[1], chunk_hdr[2], chunk_hdr[3]]);
    if &chunk_hdr[4..8] != b"IHDR" || ihdr_len != 13 {
        return Err(ConversionError::CorruptInput {
            format: FormatId::Png,
            detail: "first PNG chunk is not a 13-byte IHDR".to_string(),
        });
    }

    let mut ihdr = [0u8; 17]; // 13 bytes payload + 4 bytes CRC
    reader
        .read_exact(&mut ihdr)
        .map_err(|e| ConversionError::CorruptInput {
            format: FormatId::Png,
            detail: format!("truncated IHDR payload: {e}"),
        })?;
    let width = u32::from_be_bytes([ihdr[0], ihdr[1], ihdr[2], ihdr[3]]);
    let height = u32::from_be_bytes([ihdr[4], ihdr[5], ihdr[6], ihdr[7]]);
    if width == 0 || height == 0 {
        return Err(ConversionError::CorruptInput {
            format: FormatId::Png,
            detail: "zero-dimension PNG".to_string(),
        });
    }
    check_dimensions_against_limits(width, height, limits)?;

    let bit_depth = ihdr[8];
    let color_type = ihdr[9];
    let mut has_alpha = matches!(color_type, 4 | 6);
    let mut has_icc = false;
    let mut has_exif = false;
    let mut orientation = 1u8;

    // Scan ancillary chunks before IDAT/IEND (cap at 64 chunks to bound header I/O)
    for _ in 0..64 {
        if reader.read_exact(&mut chunk_hdr).is_err() {
            break;
        }
        let len = u32::from_be_bytes([chunk_hdr[0], chunk_hdr[1], chunk_hdr[2], chunk_hdr[3]]);
        let ctype = [chunk_hdr[4], chunk_hdr[5], chunk_hdr[6], chunk_hdr[7]];
        match &ctype {
            b"acTL" => {
                return Err(ConversionError::UnsupportedFeature {
                    feature: UnsupportedFeature::Animation,
                });
            }
            b"tRNS" => {
                if len > 0 {
                    has_alpha = true;
                }
                let _ = reader.seek(SeekFrom::Current(i64::from(len) + 4));
            }
            b"iCCP" => {
                if len > 0 {
                    has_icc = true;
                }
                let _ = reader.seek(SeekFrom::Current(i64::from(len) + 4));
            }
            b"eXIf" => {
                if len > 0 && len <= 65_536 {
                    has_exif = true;
                    let mut exif_buf = vec![0u8; len as usize];
                    if reader.read_exact(&mut exif_buf).is_ok() {
                        orientation = parse_exif_orientation(&exif_buf).unwrap_or(1);
                    }
                    let _ = reader.seek(SeekFrom::Current(4));
                } else {
                    has_exif = len > 0;
                    let _ = reader.seek(SeekFrom::Current(i64::from(len) + 4));
                }
            }
            b"IDAT" | b"IEND" => break,
            _ => {
                if reader.seek(SeekFrom::Current(i64::from(len) + 4)).is_err() {
                    break;
                }
            }
        }
    }

    Ok(SourceProbe {
        format: FormatId::Png,
        width,
        height,
        has_alpha,
        is_animated: false,
        orientation,
        has_icc,
        has_exif,
        bit_depth,
    })
}

fn inspect_jpeg_header<R: Read + Seek>(
    reader: &mut R,
    limits: &ResourceLimitsConfig,
) -> Result<SourceProbe, ConversionError> {
    let mut soi = [0u8; 2];
    reader
        .read_exact(&mut soi)
        .map_err(|e| ConversionError::CorruptInput {
            format: FormatId::Jpeg,
            detail: format!("truncated JPEG SOI: {e}"),
        })?;
    if soi != [0xFF, 0xD8] {
        return Err(ConversionError::CorruptInput {
            format: FormatId::Jpeg,
            detail: "missing JPEG SOI marker".to_string(),
        });
    }

    let mut width = 0u32;
    let mut height = 0u32;
    let mut has_icc = false;
    let mut has_exif = false;
    let mut orientation = 1u8;

    for _ in 0..128 {
        let mut b = [0u8; 1];
        loop {
            if reader.read_exact(&mut b).is_err() {
                b[0] = 0;
                break;
            }
            if b[0] == 0xFF {
                break;
            }
        }
        if b[0] != 0xFF {
            break;
        }
        // Skip fill 0xFF bytes
        let marker = loop {
            if reader.read_exact(&mut b).is_err() {
                return Err(ConversionError::CorruptInput {
                    format: FormatId::Jpeg,
                    detail: "unexpected EOF in JPEG marker stream".to_string(),
                });
            }
            if b[0] != 0xFF {
                break b[0];
            }
        };

        if marker == 0xD9 || marker == 0xDA {
            break;
        }
        if marker == 0x00 || (0xD0..=0xD8).contains(&marker) {
            continue;
        }

        let mut len_bytes = [0u8; 2];
        reader
            .read_exact(&mut len_bytes)
            .map_err(|e| ConversionError::CorruptInput {
                format: FormatId::Jpeg,
                detail: format!("truncated JPEG segment length: {e}"),
            })?;
        let seg_len = u16::from_be_bytes(len_bytes) as usize;
        if seg_len < 2 {
            return Err(ConversionError::CorruptInput {
                format: FormatId::Jpeg,
                detail: "invalid JPEG segment length < 2".to_string(),
            });
        }
        let payload_len = seg_len - 2;

        if matches!(marker, 0xC9..=0xCB | 0xCD..=0xCF) {
            return Err(ConversionError::UnsupportedFeature {
                feature: UnsupportedFeature::ArithmeticCoding,
            });
        }

        if matches!(marker, 0xC0..=0xC3) {
            if payload_len < 6 {
                return Err(ConversionError::CorruptInput {
                    format: FormatId::Jpeg,
                    detail: "truncated JPEG SOF segment".to_string(),
                });
            }
            let mut sof = [0u8; 6];
            reader
                .read_exact(&mut sof)
                .map_err(|e| ConversionError::CorruptInput {
                    format: FormatId::Jpeg,
                    detail: format!("truncated JPEG SOF header: {e}"),
                })?;
            let precision = sof[0];
            if precision != 8 {
                return Err(ConversionError::UnsupportedFeature {
                    feature: UnsupportedFeature::BitDepth(precision),
                });
            }
            height = u32::from(u16::from_be_bytes([sof[1], sof[2]]));
            width = u32::from(u16::from_be_bytes([sof[3], sof[4]]));
            if width == 0 || height == 0 {
                return Err(ConversionError::CorruptInput {
                    format: FormatId::Jpeg,
                    detail: "zero-dimension JPEG".to_string(),
                });
            }
            check_dimensions_against_limits(width, height, limits)?;
            let rem = payload_len - 6;
            if rem > 0 {
                let _ = reader.seek(SeekFrom::Current(rem as i64));
            }
        } else if marker == 0xE1 && payload_len >= 6 {
            let mut app1 = vec![0u8; payload_len];
            if reader.read_exact(&mut app1).is_ok() && app1.starts_with(b"Exif\0\0") {
                has_exif = true;
                orientation = parse_exif_orientation(&app1).unwrap_or(1);
            }
        } else if marker == 0xE2 && payload_len >= 14 {
            let mut app2_hdr = [0u8; 14];
            if reader.read_exact(&mut app2_hdr).is_ok() && app2_hdr.starts_with(b"ICC_profile\0") {
                has_icc = true;
            }
            let rem = payload_len - 14;
            if rem > 0 {
                let _ = reader.seek(SeekFrom::Current(rem as i64));
            }
        } else if payload_len > 0 && reader.seek(SeekFrom::Current(payload_len as i64)).is_err() {
            break;
        }
    }

    if width == 0 || height == 0 {
        return Err(ConversionError::CorruptInput {
            format: FormatId::Jpeg,
            detail: "missing SOF dimensions in JPEG header".to_string(),
        });
    }

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

fn inspect_webp_header<R: Read + Seek>(
    reader: &mut R,
    limits: &ResourceLimitsConfig,
) -> Result<SourceProbe, ConversionError> {
    let mut riff_hdr = [0u8; 12];
    reader
        .read_exact(&mut riff_hdr)
        .map_err(|e| ConversionError::CorruptInput {
            format: FormatId::WebP,
            detail: format!("truncated RIFF WEBP header: {e}"),
        })?;
    if &riff_hdr[0..4] != b"RIFF" || &riff_hdr[8..12] != b"WEBP" {
        return Err(ConversionError::CorruptInput {
            format: FormatId::WebP,
            detail: "invalid RIFF WEBP signature".to_string(),
        });
    }

    let mut width = 0u32;
    let mut height = 0u32;
    let mut has_alpha = false;
    let mut has_icc = false;
    let mut has_exif = false;
    let mut orientation = 1u8;

    for _ in 0..32 {
        let mut chdr = [0u8; 8];
        if reader.read_exact(&mut chdr).is_err() {
            break;
        }
        let fourcc = [chdr[0], chdr[1], chdr[2], chdr[3]];
        let len = u32::from_le_bytes([chdr[4], chdr[5], chdr[6], chdr[7]]) as usize;
        let pad = len & 1;

        match &fourcc {
            b"VP8X" => {
                if len < 10 {
                    return Err(ConversionError::CorruptInput {
                        format: FormatId::WebP,
                        detail: "VP8X chunk shorter than 10 bytes".to_string(),
                    });
                }
                let mut vp8x = [0u8; 10];
                reader
                    .read_exact(&mut vp8x)
                    .map_err(|e| ConversionError::CorruptInput {
                        format: FormatId::WebP,
                        detail: format!("truncated VP8X chunk: {e}"),
                    })?;
                let flags = vp8x[0];
                if (flags & (1 << 1)) != 0 {
                    return Err(ConversionError::UnsupportedFeature {
                        feature: UnsupportedFeature::Animation,
                    });
                }
                if (flags & (1 << 4)) != 0 {
                    has_alpha = true;
                }
                if (flags & (1 << 5)) != 0 {
                    has_icc = true;
                }
                if (flags & (1 << 3)) != 0 {
                    has_exif = true;
                }
                let w_minus_1 =
                    u32::from(vp8x[4]) | (u32::from(vp8x[5]) << 8) | (u32::from(vp8x[6]) << 16);
                let h_minus_1 =
                    u32::from(vp8x[7]) | (u32::from(vp8x[8]) << 8) | (u32::from(vp8x[9]) << 16);
                width = w_minus_1.saturating_add(1);
                height = h_minus_1.saturating_add(1);
                check_dimensions_against_limits(width, height, limits)?;
                let rem = (len - 10) + pad;
                if rem > 0 {
                    let _ = reader.seek(SeekFrom::Current(rem as i64));
                }
            }
            b"ANIM" | b"ANMF" => {
                return Err(ConversionError::UnsupportedFeature {
                    feature: UnsupportedFeature::Animation,
                });
            }
            b"ALPH" => {
                has_alpha = true;
                let _ = reader.seek(SeekFrom::Current((len + pad) as i64));
            }
            b"VP8 " => {
                if width == 0 && len >= 10 {
                    let mut vp8 = [0u8; 10];
                    if reader.read_exact(&mut vp8).is_ok()
                        && vp8[3] == 0x9D
                        && vp8[4] == 0x01
                        && vp8[5] == 0x2A
                    {
                        width = u32::from(u16::from_le_bytes([vp8[6], vp8[7]]) & 0x3FFF);
                        height = u32::from(u16::from_le_bytes([vp8[8], vp8[9]]) & 0x3FFF);
                        check_dimensions_against_limits(width, height, limits)?;
                    }
                    let rem = (len - 10) + pad;
                    if rem > 0 {
                        let _ = reader.seek(SeekFrom::Current(rem as i64));
                    }
                } else {
                    let _ = reader.seek(SeekFrom::Current((len + pad) as i64));
                }
            }
            b"VP8L" => {
                if len >= 5 {
                    let mut vp8l = [0u8; 5];
                    if reader.read_exact(&mut vp8l).is_ok() && vp8l[0] == 0x2F {
                        let bits = u32::from_le_bytes([vp8l[1], vp8l[2], vp8l[3], vp8l[4]]);
                        if width == 0 {
                            width = (bits & 0x3FFF) + 1;
                            height = ((bits >> 14) & 0x3FFF) + 1;
                            check_dimensions_against_limits(width, height, limits)?;
                        }
                        if ((bits >> 28) & 1) != 0 {
                            has_alpha = true;
                        }
                    }
                    let rem = (len - 5) + pad;
                    if rem > 0 {
                        let _ = reader.seek(SeekFrom::Current(rem as i64));
                    }
                } else {
                    let _ = reader.seek(SeekFrom::Current((len + pad) as i64));
                }
            }
            b"ICCP" => {
                if len > 0 {
                    has_icc = true;
                }
                let _ = reader.seek(SeekFrom::Current((len + pad) as i64));
            }
            b"EXIF" => {
                if len > 0 && len <= 65_536 {
                    has_exif = true;
                    let mut exif_buf = vec![0u8; len];
                    if reader.read_exact(&mut exif_buf).is_ok() {
                        orientation = parse_exif_orientation(&exif_buf).unwrap_or(1);
                    }
                    if pad > 0 {
                        let _ = reader.seek(SeekFrom::Current(pad as i64));
                    }
                } else {
                    has_exif = len > 0;
                    let _ = reader.seek(SeekFrom::Current((len + pad) as i64));
                }
            }
            _ => {
                if reader.seek(SeekFrom::Current((len + pad) as i64)).is_err() {
                    break;
                }
            }
        }
    }

    if width == 0 || height == 0 {
        return Err(ConversionError::CorruptInput {
            format: FormatId::WebP,
            detail: "missing canvas dimensions in WebP header".to_string(),
        });
    }

    Ok(SourceProbe {
        format: FormatId::WebP,
        width,
        height,
        has_alpha,
        is_animated: false,
        orientation,
        has_icc,
        has_exif,
        bit_depth: 8,
    })
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

/// Parses EXIF Orientation (`0x0112`, SHORT, count 1) from raw TIFF or `Exif\0\0`-prefixed bytes.
#[must_use]
pub fn parse_exif_orientation(raw: &[u8]) -> Option<u8> {
    let tiff = if raw.len() >= 14
        && &raw[0..6] == b"Exif\0\0"
        && (&raw[6..10] == b"II\x2a\x00" || &raw[6..10] == b"MM\x00\x2a")
    {
        &raw[6..]
    } else {
        raw
    };
    if tiff.len() < 8 {
        return None;
    }
    let big_endian = match &tiff[0..4] {
        b"II\x2a\x00" => false,
        b"MM\x00\x2a" => true,
        _ => return None,
    };
    let ifd0_offset = read_u32(tiff, 4, big_endian)? as usize;
    let entries_start = ifd0_offset.checked_add(2)?;
    if entries_start > tiff.len() {
        return None;
    }
    let num_entries = usize::from(read_u16(tiff, ifd0_offset, big_endian)?);
    let max_entries = num_entries.min((tiff.len() - entries_start) / 12);

    for i in 0..max_entries {
        let entry_pos = entries_start + i * 12;
        let tag = read_u16(tiff, entry_pos, big_endian)?;
        let field_type = read_u16(tiff, entry_pos + 2, big_endian)?;
        let count = read_u32(tiff, entry_pos + 4, big_endian)?;
        if tag == 0x0112 && field_type == 3 && count == 1 {
            let val = read_u16(tiff, entry_pos + 8, big_endian)?;
            if (1..=8).contains(&val) {
                return Some(val as u8);
            }
            return None;
        }
    }
    None
}

fn read_u16(buf: &[u8], offset: usize, big_endian: bool) -> Option<u16> {
    let bytes: [u8; 2] = buf.get(offset..offset.checked_add(2)?)?.try_into().ok()?;
    Some(if big_endian {
        u16::from_be_bytes(bytes)
    } else {
        u16::from_le_bytes(bytes)
    })
}

fn read_u32(buf: &[u8], offset: usize, big_endian: bool) -> Option<u32> {
    let bytes: [u8; 4] = buf.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
    Some(if big_endian {
        u32::from_be_bytes(bytes)
    } else {
        u32::from_le_bytes(bytes)
    })
}

fn read_up_to<R: Read>(reader: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match reader.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

fn hex_preview(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn inspects_png_rgba_and_rejects_apng_without_pixel_decode() {
        reset_header_inspection_count();
        // Minimal PNG with 64x32 RGBA8 IHDR + IEND
        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&64u32.to_be_bytes());
        png.extend_from_slice(&32u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]); // bit_depth=8, color_type=6 (RGBA)
        png.extend_from_slice(&[0, 0, 0, 0]); // CRC
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(b"IEND");
        png.extend_from_slice(&[0, 0, 0, 0]);

        let limits = ResourceLimitsConfig::default();
        let probe = inspect_reader_header(&mut Cursor::new(&png), &limits).unwrap();
        assert_eq!(probe.format, FormatId::Png);
        assert_eq!(probe.width, 64);
        assert_eq!(probe.height, 32);
        assert!(probe.has_alpha);
        assert_eq!(pixel_decode_count(), 0);
    }

    #[test]
    fn inspects_webp_vp8l_and_vp8x_headers() {
        // VP8X container 800x600 with alpha + ICC
        let mut webp = Vec::new();
        webp.extend_from_slice(b"RIFF");
        webp.extend_from_slice(&22u32.to_le_bytes());
        webp.extend_from_slice(b"WEBP");
        webp.extend_from_slice(b"VP8X");
        webp.extend_from_slice(&10u32.to_le_bytes());
        // flags: alpha (1<<4) | icc (1<<5)
        let flags = (1u8 << 4) | (1u8 << 5);
        let w_m1 = 800u32 - 1;
        let h_m1 = 600u32 - 1;
        webp.extend_from_slice(&[
            flags,
            0,
            0,
            0,
            (w_m1 & 0xFF) as u8,
            ((w_m1 >> 8) & 0xFF) as u8,
            ((w_m1 >> 16) & 0xFF) as u8,
            (h_m1 & 0xFF) as u8,
            ((h_m1 >> 8) & 0xFF) as u8,
            ((h_m1 >> 16) & 0xFF) as u8,
        ]);

        let limits = ResourceLimitsConfig::default();
        let probe = inspect_reader_header(&mut Cursor::new(&webp), &limits).unwrap();
        assert_eq!(probe.format, FormatId::WebP);
        assert_eq!(probe.width, 800);
        assert_eq!(probe.height, 600);
        assert!(probe.has_alpha);
        assert!(probe.has_icc);
    }
}
