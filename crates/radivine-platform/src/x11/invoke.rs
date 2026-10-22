//! Invoke-mode pointer and keyboard grabs (`GrabPointer` + `GrabKeyboard`) and keycode-to-[`Key`]
//! translation (`PLATFORM_INTEGRATION.md` §3.9).

use std::thread;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt as _, Cursor, EventMask, GrabMode, GrabStatus, KeyButMask, Keysym, Setup, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::{Key, PlatformError};

// Standard X11 keysyms (`<X11/keysymdef.h>`)
const XK_BACKSPACE: Keysym = 0xff08;
const XK_TAB: Keysym = 0xff09;
const XK_RETURN: Keysym = 0xff0d;
const XK_ESCAPE: Keysym = 0xff1b;
const XK_LEFT: Keysym = 0xff51;
const XK_UP: Keysym = 0xff52;
const XK_RIGHT: Keysym = 0xff53;
const XK_DOWN: Keysym = 0xff54;
const XK_KP_ENTER: Keysym = 0xff8d;
const XK_KP_LEFT: Keysym = 0xff96;
const XK_KP_UP: Keysym = 0xff97;
const XK_KP_RIGHT: Keysym = 0xff98;
const XK_KP_DOWN: Keysym = 0xff99;
const XK_KP_1: Keysym = 0xffb1;
const XK_KP_9: Keysym = 0xffb9;
const XK_ISO_LEFT_TAB: Keysym = 0xfe20;
const XK_SPACE: Keysym = 0x0020;
const XK_1: Keysym = 0x0031;
const XK_9: Keysym = 0x0039;

/// Creates the standard `XC_left_ptr` (glyph 68) cursor from the core `"cursor"` font, or returns
/// `x11rb::NONE` if the font is unavailable.
#[must_use]
pub fn create_left_ptr_cursor(conn: &RustConnection) -> Cursor {
    let Ok(font) = conn.generate_id() else {
        return x11rb::NONE;
    };
    if conn.open_font(font, b"cursor").is_err() {
        return x11rb::NONE;
    }
    let Ok(cursor) = conn.generate_id() else {
        let _ = conn.close_font(font);
        return x11rb::NONE;
    };
    // XC_left_ptr is glyph 68, mask 69
    let _ = conn.create_glyph_cursor(
        cursor, font, font, 68, 69, 0, 0, 0, // black foreground
        0xFFFF, 0xFFFF, 0xFFFF, // white background
    );
    let _ = conn.close_font(font);
    cursor
}

/// Acquires asynchronous pointer and keyboard grabs on `window` (`PLATFORM_INTEGRATION.md` §3.9).
///
/// Retries every 10 ms up to 300 ms if `GrabStatus::ALREADY_GRABBED` is returned (e.g. when
/// summoned from a global hotkey daemon that momentarily holds a passive key grab).
pub fn grab_input(
    conn: &RustConnection,
    window: Window,
    cursor: Cursor,
) -> Result<(), PlatformError> {
    let deadline = Instant::now() + Duration::from_millis(300);
    let ptr_mask = EventMask::POINTER_MOTION | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE;

    loop {
        let ptr_reply = conn
            .grab_pointer(
                false,
                window,
                ptr_mask,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                x11rb::NONE,
                cursor,
                x11rb::CURRENT_TIME,
            )
            .map_err(|e| PlatformError::X11(e.to_string()))?
            .reply()
            .map_err(|e| PlatformError::X11(e.to_string()))?;

        let kbd_reply = conn
            .grab_keyboard(
                false,
                window,
                x11rb::CURRENT_TIME,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .map_err(|e| PlatformError::X11(e.to_string()))?
            .reply()
            .map_err(|e| PlatformError::X11(e.to_string()))?;

        if ptr_reply.status == GrabStatus::SUCCESS && kbd_reply.status == GrabStatus::SUCCESS {
            return Ok(());
        }

        let can_retry = (ptr_reply.status == GrabStatus::ALREADY_GRABBED
            || kbd_reply.status == GrabStatus::ALREADY_GRABBED)
            && Instant::now() < deadline;

        if can_retry {
            // Release any half-acquired grab before retrying
            let _ = conn.ungrab_pointer(x11rb::CURRENT_TIME);
            let _ = conn.ungrab_keyboard(x11rb::CURRENT_TIME);
            let _ = conn.flush();
            thread::sleep(Duration::from_millis(10));
        } else if ptr_reply.status == GrabStatus::SUCCESS || kbd_reply.status == GrabStatus::SUCCESS
        {
            // Keep whichever grab succeeded (`PLATFORM_INTEGRATION.md` §3.9)
            let _ = conn.flush();
            return Ok(());
        } else {
            return Err(PlatformError::X11(format!(
                "input grab failed: pointer={:?}, keyboard={:?}",
                ptr_reply.status, kbd_reply.status
            )));
        }
    }
}

/// Immediately releases any active pointer and keyboard grabs and flushes the connection.
pub fn ungrab_input(conn: &RustConnection) {
    let _ = conn.ungrab_pointer(x11rb::CURRENT_TIME);
    let _ = conn.ungrab_keyboard(x11rb::CURRENT_TIME);
    let _ = conn.flush();
}

/// Cached X11 keycode-to-keysym lookup table (`PLATFORM_INTEGRATION.md` §3.9).
pub struct KeymapTable {
    min_keycode: u8,
    keysyms_per_keycode: u8,
    keysyms: Vec<Keysym>,
}

impl KeymapTable {
    /// Queries the server's keyboard mapping table.
    pub fn query(conn: &RustConnection, setup: &Setup) -> Self {
        let min_keycode = setup.min_keycode;
        let max_keycode = setup.max_keycode;
        let count = max_keycode.saturating_sub(min_keycode).saturating_add(1);
        if let Ok(cookie) = conn.get_keyboard_mapping(min_keycode, count)
            && let Ok(reply) = cookie.reply()
        {
            return Self {
                min_keycode,
                keysyms_per_keycode: reply.keysyms_per_keycode,
                keysyms: reply.keysyms,
            };
        }
        Self {
            min_keycode,
            keysyms_per_keycode: 0,
            keysyms: Vec::new(),
        }
    }

    /// Translates an X11 `(keycode, modifier_mask)` pair into a logical [`Key`].
    #[must_use]
    pub fn translate(&self, keycode: u8, state: KeyButMask) -> Option<Key> {
        if keycode < self.min_keycode || self.keysyms_per_keycode == 0 {
            return None;
        }
        let idx = usize::from(keycode - self.min_keycode) * usize::from(self.keysyms_per_keycode);
        if idx >= self.keysyms.len() {
            return None;
        }

        let shift = state.contains(KeyButMask::SHIFT);
        let sym0 = self.keysyms[idx];
        let sym1 = if self.keysyms_per_keycode > 1 && idx + 1 < self.keysyms.len() {
            self.keysyms[idx + 1]
        } else {
            0
        };
        let active_sym = if shift && sym1 != 0 { sym1 } else { sym0 };

        translate_keysym(active_sym, sym0, shift)
    }
}

/// Translates an X11 keysym (and unshifted primary keysym `sym0`) into a [`Key`].
#[must_use]
pub fn translate_keysym(active_sym: Keysym, sym0: Keysym, shift: bool) -> Option<Key> {
    match active_sym {
        XK_LEFT | XK_KP_LEFT => Some(Key::Left),
        XK_RIGHT | XK_KP_RIGHT => Some(Key::Right),
        XK_UP | XK_KP_UP => Some(Key::Up),
        XK_DOWN | XK_KP_DOWN => Some(Key::Down),
        XK_ISO_LEFT_TAB => Some(Key::ShiftTab),
        XK_TAB => {
            if shift {
                Some(Key::ShiftTab)
            } else {
                Some(Key::Tab)
            }
        }
        XK_RETURN | XK_KP_ENTER => Some(Key::Enter),
        XK_SPACE => Some(Key::Space),
        XK_BACKSPACE => Some(Key::Backspace),
        XK_ESCAPE => Some(Key::Escape),
        s if (XK_1..=XK_9).contains(&s) => Some(Key::Digit((s - XK_1 + 1) as u8)),
        s if (XK_KP_1..=XK_KP_9).contains(&s) => Some(Key::Digit((s - XK_KP_1 + 1) as u8)),
        _ => {
            // Also check unshifted sym0 for number keys if shift was held
            if (XK_1..=XK_9).contains(&sym0) {
                Some(Key::Digit((sym0 - XK_1 + 1) as u8))
            } else {
                None
            }
        }
    }
}
