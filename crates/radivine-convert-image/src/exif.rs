//! Pure-Rust EXIF/TIFF header normalization, orientation parsing, in-place orientation reset,
//! and `KeepSafe` tag filtering.

/// Strips an optional leading `b"Exif\0\0"` APP1 prefix if followed by a TIFF header (`II*\0` or `MM\0*`).
#[must_use]
pub fn normalize_exif_tiff_slice(raw: &[u8]) -> &[u8] {
    if raw.len() >= 14
        && &raw[0..6] == b"Exif\0\0"
        && (&raw[6..10] == b"II\x2a\x00" || &raw[6..10] == b"MM\x00\x2a")
    {
        &raw[6..]
    } else {
        raw
    }
}

/// Parses the EXIF `Orientation` tag (`0x0112`, SHORT, count 1) from a raw TIFF or `Exif\0\0`-prefixed slice.
///
/// Returns `Some(1..=8)` if a valid orientation entry is found, or `None` if absent/invalid.
/// Never panics on malformed or truncated inputs.
#[must_use]
pub fn parse_orientation(raw: &[u8]) -> Option<u8> {
    let tiff = normalize_exif_tiff_slice(raw);
    let (orient, _offset, _big_endian) = locate_orientation_value_offset(tiff)?;
    Some(orient)
}

/// Resets the EXIF `Orientation` tag (`0x0112`) in-place to `1` (`Horizontal / normal`) inside `raw`,
/// returning the previous orientation `1..=8` if found.
pub fn reset_orientation_in_place(raw: &mut [u8]) -> Option<u8> {
    let prefix_len = if raw.len() >= 14
        && &raw[0..6] == b"Exif\0\0"
        && (&raw[6..10] == b"II\x2a\x00" || &raw[6..10] == b"MM\x00\x2a")
    {
        6usize
    } else {
        0usize
    };

    let tiff = &raw[prefix_len..];
    let (orient, val_offset, big_endian) = locate_orientation_value_offset(tiff)?;
    let abs_offset = prefix_len.checked_add(val_offset)?;
    let end_offset = abs_offset.checked_add(2)?;
    if end_offset > raw.len() {
        return None;
    }

    let one_bytes = if big_endian {
        1u16.to_be_bytes()
    } else {
        1u16.to_le_bytes()
    };
    raw[abs_offset..end_offset].copy_from_slice(&one_bytes);
    Some(orient)
}

/// Produces a sanitized copy of `raw` for `ExifPolicy::KeepSafe`:
/// - Normalizes any `Exif\0\0` prefix to canonical raw TIFF
/// - Strips sensitive IFD0 pointers/tags (`0x8825` GPSInfoIFDPointer, `0x013B` Artist,
///   `0x8298` Copyright, `0x927C` MakerNote, `0x9286` UserComment, `0xA430` CameraOwnerName,
///   `0xA431` BodySerialNumber)
/// - Optionally resets `0x0112` Orientation to `1` when `reset_orientation` is `true`.
#[must_use]
pub fn sanitize_exif_keep_safe(raw: &[u8], reset_orientation: bool) -> Option<Vec<u8>> {
    let mut tiff = normalize_exif_tiff_slice(raw).to_vec();
    if tiff.len() < 8 {
        return None;
    }
    let big_endian = match &tiff[0..4] {
        b"II\x2a\x00" => false,
        b"MM\x00\x2a" => true,
        _ => return None,
    };

    if reset_orientation {
        let _ = reset_orientation_in_place(&mut tiff);
    }

    let ifd0_offset = read_u32(&tiff, 4, big_endian)? as usize;
    let count_end = ifd0_offset.checked_add(2)?;
    if count_end > tiff.len() {
        return Some(tiff);
    }
    let num_entries = usize::from(read_u16(&tiff, ifd0_offset, big_endian)?);
    let max_entries = num_entries.min(512);

    for i in 0..max_entries {
        let Some(entry_pos) = count_end.checked_add(i.checked_mul(12)?) else {
            break;
        };
        let Some(entry_end) = entry_pos.checked_add(12) else {
            break;
        };
        if entry_end > tiff.len() {
            break;
        }
        let Some(tag) = read_u16(&tiff, entry_pos, big_endian) else {
            break;
        };
        if matches!(
            tag,
            0x8825 | 0x013B | 0x8298 | 0x927C | 0x9286 | 0xA430 | 0xA431
        ) {
            // Zero the 12-byte IFD entry so decoders ignore the sensitive tag/pointer
            tiff[entry_pos..entry_end].fill(0);
        }
    }

    Some(tiff)
}

fn locate_orientation_value_offset(tiff: &[u8]) -> Option<(u8, usize, bool)> {
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
    // Cap iteration to prevent malicious entry counts on truncated buffers
    let max_entries = num_entries.min((tiff.len() - entries_start) / 12);

    for i in 0..max_entries {
        let entry_pos = entries_start + i * 12;
        let tag = read_u16(tiff, entry_pos, big_endian)?;
        let field_type = read_u16(tiff, entry_pos + 2, big_endian)?;
        let count = read_u32(tiff, entry_pos + 4, big_endian)?;
        if tag == 0x0112 && field_type == 3 && count == 1 {
            let val_offset = entry_pos + 8;
            let raw_val = read_u16(tiff, val_offset, big_endian)?;
            if (1..=8).contains(&raw_val) {
                return Some((raw_val as u8, val_offset, big_endian));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn build_exif(orientation: u16, big_endian: bool, with_exif_header: bool) -> Vec<u8> {
        let mut out = Vec::new();
        if with_exif_header {
            out.extend_from_slice(b"Exif\0\0");
        }
        let u16_b = |v: u16| -> [u8; 2] {
            if big_endian {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let u32_b = |v: u32| -> [u8; 4] {
            if big_endian {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        if big_endian {
            out.extend_from_slice(b"MM\x00\x2a");
        } else {
            out.extend_from_slice(b"II\x2a\x00");
        }
        out.extend_from_slice(&u32_b(8));
        out.extend_from_slice(&u16_b(2));
        // Entry 0: GPS pointer (0x8825)
        out.extend_from_slice(&u16_b(0x8825));
        out.extend_from_slice(&u16_b(4));
        out.extend_from_slice(&u32_b(1));
        out.extend_from_slice(&u32_b(100));
        // Entry 1: Orientation (0x0112)
        out.extend_from_slice(&u16_b(0x0112));
        out.extend_from_slice(&u16_b(3));
        out.extend_from_slice(&u32_b(1));
        out.extend_from_slice(&u16_b(orientation));
        out.extend_from_slice(&u16_b(0));
        out.extend_from_slice(&u32_b(0));
        out
    }

    #[test]
    fn parse_and_reset_all_8_orientations_le_and_be() {
        for orient in 1u16..=8 {
            for &be in &[false, true] {
                for &hdr in &[false, true] {
                    let mut chunk = build_exif(orient, be, hdr);
                    assert_eq!(parse_orientation(&chunk), Some(orient as u8));
                    assert_eq!(reset_orientation_in_place(&mut chunk), Some(orient as u8));
                    assert_eq!(parse_orientation(&chunk), Some(1));
                }
            }
        }
    }

    #[test]
    fn keep_safe_strips_gps_tag_and_resets_orientation() {
        let chunk = build_exif(6, false, true);
        let safe = sanitize_exif_keep_safe(&chunk, true).unwrap();
        assert_eq!(parse_orientation(&safe), Some(1));
        // Entry 0 (at offset 10..22) was 0x8825 (GPS) and must now be zeroed
        assert_eq!(&safe[10..22], &[0u8; 12]);
    }

    #[test]
    fn malformed_or_truncated_exif_never_panics() {
        for slice in [
            &b""[..],
            &b"II"[..],
            &b"II\x2a\x00\xFF\xFF\xFF\xFF"[..],
            &b"MM\x00\x2a\x00\x00\x00\x08\xFF\xFF"[..],
            &b"Exif\0\0II\x2a\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00\x99\x00\x00\x00"[..],
        ] {
            let mut owned = slice.to_vec();
            let _ = parse_orientation(&owned);
            let _ = reset_orientation_in_place(&mut owned);
            let _ = sanitize_exif_keep_safe(&owned, true);
        }
    }
}
