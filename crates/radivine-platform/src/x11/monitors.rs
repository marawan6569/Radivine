//! RandR 1.5 monitor geometry, refresh rates, `Xft.dpi` scale detection, and compositor check
//! (`PLATFORM_INTEGRATION.md` §3.2, §3.10).

use x11rb::protocol::randr::{ConnectionExt as _, NotifyMask};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, Screen, Window};
use x11rb::rust_connection::RustConnection;

use crate::x11::atoms::Atoms;
use crate::{Monitor, PhysPoint, RectPx};

/// Selects RandR screen/monitor change notifications on `root`.
pub fn select_randr_notifications(conn: &RustConnection, root: Window) {
    let _ = conn.randr_select_input(
        root,
        NotifyMask::SCREEN_CHANGE | NotifyMask::CRTC_CHANGE | NotifyMask::OUTPUT_CHANGE,
    );
}

/// Queries active monitors via RandR 1.5 (`GetMonitors`), falling back to a single monitor
/// covering the X11 screen if RandR fails or returns no monitors.
#[must_use]
pub fn query_monitors(conn: &RustConnection, screen: &Screen, randr_ok: bool) -> Vec<Monitor> {
    if randr_ok
        && let Ok(cookie) = conn.randr_get_monitors(screen.root, true)
        && let Ok(reply) = cookie.reply()
        && !reply.monitors.is_empty()
    {
        let default_refresh_mhz = query_primary_refresh_mhz(conn, screen.root).unwrap_or(60_000);
        let mut out = Vec::with_capacity(reply.monitors.len());
        for (idx, m) in reply.monitors.iter().enumerate() {
            let name = conn
                .get_atom_name(m.name)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                .unwrap_or_else(|| format!("Monitor-{idx}"));

            out.push(Monitor {
                id: idx as u32,
                name,
                rect: RectPx::new(
                    i32::from(m.x),
                    i32::from(m.y),
                    u32::from(m.width),
                    u32::from(m.height),
                ),
                refresh_mhz: default_refresh_mhz,
                primary: m.primary,
            });
        }
        return out;
    }

    vec![Monitor {
        id: 0,
        name: "Screen-0".to_string(),
        rect: RectPx::new(
            0,
            0,
            u32::from(screen.width_in_pixels),
            u32::from(screen.height_in_pixels),
        ),
        refresh_mhz: 60_000,
        primary: true,
    }]
}

fn query_primary_refresh_mhz(conn: &RustConnection, root: Window) -> Option<u32> {
    let res = conn
        .randr_get_screen_resources_current(root)
        .ok()?
        .reply()
        .ok()?;
    for &crtc in &res.crtcs {
        let Ok(cookie) = conn.randr_get_crtc_info(crtc, res.config_timestamp) else {
            continue;
        };
        let Ok(info) = cookie.reply() else {
            continue;
        };
        if info.mode == 0 || info.width == 0 || info.height == 0 {
            continue;
        }
        if let Some(mode) = res.modes.iter().find(|m| m.id == info.mode) {
            let total_px = u64::from(mode.htotal) * u64::from(mode.vtotal);
            if let Some(mhz) = (u64::from(mode.dot_clock) * 1000).checked_div(total_px)
                && (24_000..=500_000).contains(&mhz)
            {
                return Some(mhz as u32);
            }
        }
    }
    None
}

/// Picks the monitor containing `at`, falling back to the primary monitor (or first monitor).
#[must_use]
pub fn pick_monitor_for_point(monitors: &[Monitor], at: PhysPoint) -> Monitor {
    if let Some(found) = monitors.iter().find(|m| m.rect.contains(at)) {
        return found.clone();
    }
    if let Some(primary) = monitors.iter().find(|m| m.primary) {
        return primary.clone();
    }
    monitors.first().cloned().unwrap_or(Monitor {
        id: 0,
        name: "Default".to_string(),
        rect: RectPx::new(0, 0, 1920, 1080),
        refresh_mhz: 60_000,
        primary: true,
    })
}

/// Reads the root window's `RESOURCE_MANAGER` property and extracts `Xft.dpi / 96.0`
/// (`PLATFORM_INTEGRATION.md` §3.10). Returns `1.0` if unset or invalid.
#[must_use]
pub fn query_xft_scale(conn: &RustConnection, root: Window, atoms: &Atoms) -> (Option<f32>, f32) {
    if let Ok(cookie) = conn.get_property(
        false,
        root,
        atoms.RESOURCE_MANAGER,
        AtomEnum::STRING,
        0,
        16_384,
    ) && let Ok(reply) = cookie.reply()
        && !reply.value.is_empty()
    {
        let text = String::from_utf8_lossy(&reply.value);
        if let Some(dpi) = parse_xft_dpi(&text) {
            let scale = (dpi / 96.0).clamp(0.5, 4.0);
            return (Some(dpi), scale);
        }
    }
    (None, 1.0)
}

/// Parses `Xft.dpi: <number>` from an X11 `RESOURCE_MANAGER` string.
#[must_use]
pub fn parse_xft_dpi(resource_manager_str: &str) -> Option<f32> {
    for line in resource_manager_str.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Xft.dpi:")
            && let Ok(val) = rest.trim().parse::<f32>()
            && val > 24.0
            && val <= 768.0
        {
            return Some(val);
        }
    }
    None
}

/// Returns `true` if a compositing manager owns `_NET_WM_CM_S0` (`PLATFORM_INTEGRATION.md` §3.2)
/// or if `RADIVINE_FORCE_ARGB=1` is set (`TESTING_STRATEGY.md` §4).
#[must_use]
pub fn is_compositor_active(conn: &RustConnection, atoms: &Atoms) -> (bool, u32) {
    if let Ok(cookie) = conn.get_selection_owner(atoms._NET_WM_CM_S0)
        && let Ok(reply) = cookie.reply()
        && reply.owner != x11rb::NONE
    {
        return (true, reply.owner);
    }
    if std::env::var("RADIVINE_FORCE_ARGB")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    {
        return (true, 0);
    }
    (false, 0)
}

/// Queries the desktop reduced-motion preference (`PLATFORM_INTEGRATION.md` §3.10).
///
/// Checks `RADIVINE_REDUCED_MOTION` first, then the XSETTINGS `_XSETTINGS_S0` owner's
/// `_XSETTINGS_SETTINGS` property (`Net/EnableAnimations`).
#[must_use]
pub fn query_reduced_motion(conn: &RustConnection, atoms: &Atoms) -> Option<bool> {
    if let Ok(val) = std::env::var("RADIVINE_REDUCED_MOTION") {
        let trimmed = val.trim();
        if trimmed == "1" || trimmed.eq_ignore_ascii_case("true") {
            return Some(true);
        }
        if trimmed == "0" || trimmed.eq_ignore_ascii_case("false") {
            return Some(false);
        }
    }

    let owner = conn
        .get_selection_owner(atoms._XSETTINGS_S0)
        .ok()
        .and_then(|c| c.reply().ok())
        .map_or(x11rb::NONE, |r| r.owner);
    if owner == x11rb::NONE {
        return None;
    }

    let reply = conn
        .get_property(
            false,
            owner,
            atoms._XSETTINGS_SETTINGS,
            AtomEnum::ANY,
            0,
            16_384,
        )
        .ok()?
        .reply()
        .ok()?;
    parse_xsettings_enable_animations(&reply.value).map(|enable_anim| !enable_anim)
}

/// Parses the XSETTINGS binary blob (`_XSETTINGS_SETTINGS`) and extracts `Net/EnableAnimations` (`bool`).
#[must_use]
pub fn parse_xsettings_enable_animations(data: &[u8]) -> Option<bool> {
    if data.len() < 12 {
        return None;
    }
    let little_endian = data[0] == 0;
    let read_u16 = |off: usize| -> Option<u16> {
        let b: [u8; 2] = data.get(off..off + 2)?.try_into().ok()?;
        Some(if little_endian {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let read_u32 = |off: usize| -> Option<u32> {
        let b: [u8; 4] = data.get(off..off + 4)?.try_into().ok()?;
        Some(if little_endian {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };

    let count = read_u32(8)? as usize;
    let mut pos = 12usize;
    for _ in 0..count.min(512) {
        let setting_type = *data.get(pos)?;
        let name_len = read_u16(pos + 2)? as usize;
        pos = pos.checked_add(4)?;
        let name_bytes = data.get(pos..pos.checked_add(name_len)?)?;
        let padded_name = (name_len + 3) & !3;
        pos = pos.checked_add(padded_name)?;
        // Skip last-change-serial (4 bytes)
        pos = pos.checked_add(4)?;

        match setting_type {
            0 => {
                // XSETTINGS_TYPE_INT (4 bytes)
                let val = read_u32(pos)?;
                pos = pos.checked_add(4)?;
                if name_bytes == b"Net/EnableAnimations" {
                    return Some(val != 0);
                }
            }
            1 => {
                // XSETTINGS_TYPE_STRING (4-byte len + padded string)
                let slen = read_u32(pos)? as usize;
                let padded_s = (slen + 3) & !3;
                pos = pos.checked_add(4)?.checked_add(padded_s)?;
            }
            2 => {
                // XSETTINGS_TYPE_COLOR (4 * u16 = 8 bytes)
                pos = pos.checked_add(8)?;
            }
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xft_dpi_and_xsettings_enable_animations() {
        assert_eq!(
            parse_xft_dpi("Xft.antialias:\t1\nXft.dpi:\t192\n"),
            Some(192.0)
        );
        assert_eq!(parse_xft_dpi("Xft.hinting: 1\n"), None);

        // Construct a minimal little-endian XSETTINGS blob with 1 setting: `Net/EnableAnimations` = 0
        let mut blob = Vec::new();
        blob.extend_from_slice(&[0u8, 0, 0, 0]); // byte-order = LSBFirst (0) + 3 pad
        blob.extend_from_slice(&1u32.to_le_bytes()); // serial = 1
        blob.extend_from_slice(&1u32.to_le_bytes()); // n_settings = 1
        let name = b"Net/EnableAnimations"; // 20 bytes (already multiple of 4)
        blob.push(0); // XSETTINGS_TYPE_INT
        blob.push(0); // pad
        blob.extend_from_slice(&(name.len() as u16).to_le_bytes());
        blob.extend_from_slice(name);
        blob.extend_from_slice(&1u32.to_le_bytes()); // last-change-serial
        blob.extend_from_slice(&0u32.to_le_bytes()); // value = 0 (animations disabled)

        assert_eq!(parse_xsettings_enable_animations(&blob), Some(false));
    }
}
