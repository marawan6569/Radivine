//! Pure-Rust RIFF/VP8X container inspection and metadata muxing for WebP (`ADR-030`).

use radivine_convert::{ConversionError, FormatId};

/// Maximum canvas dimension supported by the WebP extended (`VP8X`) specification.
pub const MAX_WEBP_CANVAS_DIM: u32 = 16_383;

/// Summary of chunks present in a RIFF WEBP container.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebPContainerInfo {
    pub has_alpha: bool,
    pub is_animated: bool,
    pub has_icc: bool,
    pub has_exif: bool,
}

/// Inspects a RIFF WEBP byte slice for animation (`VP8X` bit 1 or `ANIM`/`ANMF` chunks),
/// alpha (`VP8X` bit 4 or `ALPH` chunk), `ICCP`, and `EXIF` chunks without decoding bitstreams.
pub fn inspect_webp_container(webp_bytes: &[u8]) -> Result<WebPContainerInfo, ConversionError> {
    if webp_bytes.len() < 12 || &webp_bytes[0..4] != b"RIFF" || &webp_bytes[8..12] != b"WEBP" {
        return Err(ConversionError::CorruptInput {
            format: FormatId::WebP,
            detail: "missing RIFF WEBP container signature".to_string(),
        });
    }

    let mut info = WebPContainerInfo::default();
    let mut pos = 12usize;

    while pos + 8 <= webp_bytes.len() {
        let fourcc: [u8; 4] = webp_bytes[pos..pos + 4].try_into().unwrap();
        let len = u32::from_le_bytes(webp_bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let data_start = pos + 8;
        let Some(data_end) = data_start.checked_add(len) else {
            return Err(ConversionError::CorruptInput {
                format: FormatId::WebP,
                detail: "RIFF chunk size overflow".to_string(),
            });
        };
        if data_end > webp_bytes.len() {
            return Err(ConversionError::CorruptInput {
                format: FormatId::WebP,
                detail: "truncated RIFF chunk payload".to_string(),
            });
        }
        let payload = &webp_bytes[data_start..data_end];
        match &fourcc {
            b"VP8X" => {
                if payload.len() < 10 {
                    return Err(ConversionError::CorruptInput {
                        format: FormatId::WebP,
                        detail: "VP8X chunk shorter than 10 bytes".to_string(),
                    });
                }
                let flags = payload[0];
                if (flags & (1 << 1)) != 0 {
                    info.is_animated = true;
                }
                if (flags & (1 << 4)) != 0 {
                    info.has_alpha = true;
                }
                if (flags & (1 << 5)) != 0 {
                    info.has_icc = true;
                }
                if (flags & (1 << 3)) != 0 {
                    info.has_exif = true;
                }
            }
            b"ANIM" | b"ANMF" => {
                info.is_animated = true;
            }
            b"ALPH" => {
                info.has_alpha = true;
            }
            b"ICCP" => {
                info.has_icc = !payload.is_empty();
            }
            b"EXIF" => {
                info.has_exif = !payload.is_empty();
            }
            _ => {}
        }

        let padded = len.saturating_add(len & 1);
        pos = data_start.saturating_add(padded);
    }

    Ok(info)
}

/// Muxes optional `ICCP` and `EXIF` chunks into a `libwebp`-encoded WebP container,
/// upgrading simple `VP8 `/`VP8L` containers to extended `VP8X` when metadata is present.
pub fn mux_webp_metadata(
    webp_bytes: &[u8],
    width: u32,
    height: u32,
    icc: Option<&[u8]>,
    exif: Option<&[u8]>,
) -> Result<Vec<u8>, ConversionError> {
    let icc = icc.filter(|b| !b.is_empty());
    let exif = exif.filter(|b| !b.is_empty());
    if icc.is_none() && exif.is_none() {
        return Ok(webp_bytes.to_vec());
    }

    if webp_bytes.len() < 12 || &webp_bytes[0..4] != b"RIFF" || &webp_bytes[8..12] != b"WEBP" {
        return Err(ConversionError::CorruptInput {
            format: FormatId::WebP,
            detail: "invalid RIFF WEBP header for metadata muxing".to_string(),
        });
    }
    if width == 0 || height == 0 || width > MAX_WEBP_CANVAS_DIM || height > MAX_WEBP_CANVAS_DIM {
        return Err(ConversionError::CorruptInput {
            format: FormatId::WebP,
            detail: format!("invalid WebP canvas dimensions {width}x{height}"),
        });
    }

    let mut has_alpha = false;
    let mut alph_chunk: Option<&[u8]> = None;
    let mut image_chunk: Option<([u8; 4], &[u8])> = None;

    let mut pos = 12usize;
    while pos + 8 <= webp_bytes.len() {
        let fourcc: [u8; 4] = webp_bytes[pos..pos + 4].try_into().unwrap();
        let len = u32::from_le_bytes(webp_bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let data_start = pos + 8;
        let Some(data_end) = data_start.checked_add(len) else {
            return Err(ConversionError::CorruptInput {
                format: FormatId::WebP,
                detail: "RIFF chunk size overflow during muxing".to_string(),
            });
        };
        if data_end > webp_bytes.len() {
            return Err(ConversionError::CorruptInput {
                format: FormatId::WebP,
                detail: "truncated RIFF chunk during muxing".to_string(),
            });
        }
        let payload = &webp_bytes[data_start..data_end];
        match &fourcc {
            b"VP8X" if payload.len() >= 10 && (payload[0] & (1 << 4)) != 0 => {
                has_alpha = true;
            }
            b"ALPH" => {
                has_alpha = true;
                alph_chunk = Some(payload);
            }
            b"VP8 " | b"VP8L" => {
                image_chunk = Some((fourcc, payload));
            }
            _ => {}
        }
        let padded = len.saturating_add(len & 1);
        pos = data_start.saturating_add(padded);
    }

    let Some((img_fourcc, img_payload)) = image_chunk else {
        return Err(ConversionError::CorruptInput {
            format: FormatId::WebP,
            detail: "WebP container is missing VP8/VP8L bitstream chunk".to_string(),
        });
    };

    let write_chunk = |buf: &mut Vec<u8>, fourcc: &[u8; 4], payload: &[u8]| {
        buf.extend_from_slice(fourcc);
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            buf.push(0);
        }
    };

    let mut flags: u8 = 0;
    if exif.is_some() {
        flags |= 1 << 3;
    }
    if has_alpha {
        flags |= 1 << 4;
    }
    if icc.is_some() {
        flags |= 1 << 5;
    }

    let extra_cap = icc.map_or(0, |b| b.len() + 10) + exif.map_or(0, |b| b.len() + 10) + 32;
    let mut body = Vec::with_capacity(webp_bytes.len().saturating_add(extra_cap));
    body.extend_from_slice(b"WEBP");

    let mut vp8x = [0u8; 10];
    vp8x[0] = flags;
    let w_minus_1 = (width - 1).to_le_bytes();
    let h_minus_1 = (height - 1).to_le_bytes();
    vp8x[4..7].copy_from_slice(&w_minus_1[0..3]);
    vp8x[7..10].copy_from_slice(&h_minus_1[0..3]);
    write_chunk(&mut body, b"VP8X", &vp8x);

    if let Some(icc_bytes) = icc {
        write_chunk(&mut body, b"ICCP", icc_bytes);
    }
    if let Some(alph_bytes) = alph_chunk {
        write_chunk(&mut body, b"ALPH", alph_bytes);
    }
    write_chunk(&mut body, &img_fourcc, img_payload);
    if let Some(exif_bytes) = exif {
        write_chunk(&mut body, b"EXIF", exif_bytes);
    }

    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mux_and_inspect_odd_length_icc_and_exif_chunks() {
        // Minimal synthetic RIFF WEBP with a 5-byte VP8 payload
        let mut base = Vec::new();
        base.extend_from_slice(b"RIFF");
        base.extend_from_slice(&18u32.to_le_bytes());
        base.extend_from_slice(b"WEBP");
        base.extend_from_slice(b"VP8 ");
        base.extend_from_slice(&5u32.to_le_bytes());
        base.extend_from_slice(&[1, 2, 3, 4, 5, 0]); // padded to 6 bytes

        let icc = [10u8, 20, 30]; // odd length (3)
        let exif = [40u8, 50, 60, 70, 80]; // odd length (5)
        let muxed = mux_webp_metadata(&base, 320, 240, Some(&icc), Some(&exif)).unwrap();
        assert_eq!(muxed.len() % 2, 0, "RIFF container must be even-length");

        let info = inspect_webp_container(&muxed).unwrap();
        assert!(info.has_icc);
        assert!(info.has_exif);
        assert!(!info.has_alpha);
        assert!(!info.is_animated);
    }

    #[test]
    fn reject_truncated_or_non_webp_containers() {
        assert!(inspect_webp_container(b"RIFF\x04\x00\x00\x00WAVE").is_err());
        assert!(mux_webp_metadata(b"not-webp", 10, 10, Some(&[1]), None).is_err());
    }
}
