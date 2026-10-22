//! XFixes `XdndSelection` drag arming and XI2 raw modifier trigger detection
//! (`PLATFORM_INTEGRATION.md` §3.6, `ARCHITECTURE.md` §5, §10).

use std::time::{Duration, Instant};

use smallvec::SmallVec;
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xinput::{self, ConnectionExt as _, XIEventMask};
use x11rb::protocol::xproto::{ConnectionExt as _, KeyButMask, Keycode, Window};
use x11rb::rust_connection::RustConnection;

use crate::{PhysPoint, PlatformError, SourceInfo, TriggerModifier};

/// Watchdog delay to disarm when the mouse button is released and no session is open (`200 ms`).
pub const BUTTON_RELEASE_DISARM_DELAY: Duration = Duration::from_millis(200);

/// Watchdog delay when the mouse button is released during an active drag session without `XdndDrop` (`1 s`).
pub const SESSION_BUTTON_RELEASE_WATCHDOG: Duration = Duration::from_secs(1);

/// Maximum duration a drag may remain armed (`120 s`).
pub const MAX_ARMED_DURATION: Duration = Duration::from_secs(120);

/// Polling interval when `TriggerDetectionMode::Poll` is active (`8 ms`).
pub const POLL_TRIGGER_INTERVAL: Duration = Duration::from_millis(8);

/// State transition produced by [`ArmingTracker::on_selection_owner_changed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArmingTransition {
    /// Drag is now armed with `source`.
    Armed {
        /// Identified drag source application.
        source: SourceInfo,
    },
    /// Drag was disarmed.
    Disarmed,
}

/// Edge event produced by [`ArmingTracker::on_raw_key_press`], [`ArmingTracker::on_raw_key_release`],
/// or [`ArmingTracker::on_poll_sample`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerEdge {
    /// Fresh press of the trigger modifier (0 keys down → 1 key down, not pre-held at arm).
    FreshPress,
    /// All trigger modifier keys were released after a fresh press.
    Released,
}

/// Result of checking [`ArmingTracker`] deadlines at `now`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArmingDeadlineCheck {
    /// Whether the tracker exceeded the 120 s max duration and disarmed immediately.
    pub max_duration_expired: bool,
    /// Whether the button-release watchdog deadline elapsed (caller should check `QueryPointer`).
    pub verify_button_release: bool,
    /// Whether the 8 ms poll timer is due.
    pub poll_due: bool,
}

/// Pure, deterministic state machine for drag arming and fresh-modifier edge detection
/// (`PLATFORM_INTEGRATION.md` §3.6).
#[derive(Clone, Debug)]
pub struct ArmingTracker {
    /// 256-bit mask of X11 keycodes mapped to the configured [`TriggerModifier`].
    trigger_keycodes: [u64; 4],
    /// 256-bit mask of trigger keycodes currently pressed.
    down_keys: [u64; 4],
    /// Whether a foreign drag is currently armed.
    armed: bool,
    /// Current `XdndSelection` owner XID.
    owner: Window,
    /// X server timestamp from `XFixesSelectionNotify`.
    selection_timestamp: u32,
    /// Identified drag source metadata.
    source: SourceInfo,
    /// Whether the trigger modifier was already held at the moment the drag was armed.
    held_at_arm: bool,
    /// Previous modifier state observed in `Poll` mode.
    poll_modifier_was_down: bool,
    /// Whether a Radivine drag wheel session is currently open.
    session_open: bool,
    /// Timestamp when the drag was armed.
    armed_at: Option<Instant>,
    /// Deadline to verify whether the mouse button is still released and disarm.
    button_release_deadline: Option<Instant>,
    /// Next 8 ms poll deadline when running in `Poll` mode.
    poll_deadline: Option<Instant>,
}

impl Default for ArmingTracker {
    fn default() -> Self {
        Self::new(&[])
    }
}

impl ArmingTracker {
    /// Creates a new disarmed [`ArmingTracker`] with the given trigger keycodes.
    #[must_use]
    pub fn new(keycodes: &[Keycode]) -> Self {
        let mut tracker = Self {
            trigger_keycodes: [0; 4],
            down_keys: [0; 4],
            armed: false,
            owner: x11rb::NONE,
            selection_timestamp: x11rb::CURRENT_TIME,
            source: SourceInfo::default(),
            held_at_arm: false,
            poll_modifier_was_down: false,
            session_open: false,
            armed_at: None,
            button_release_deadline: None,
            poll_deadline: None,
        };
        tracker.set_trigger_keycodes(keycodes);
        tracker
    }

    /// Updates the set of X11 keycodes mapped to the configured trigger modifier.
    pub fn set_trigger_keycodes(&mut self, keycodes: &[Keycode]) {
        self.trigger_keycodes = [0; 4];
        for &kc in keycodes {
            if kc != 0 {
                set_bit(&mut self.trigger_keycodes, kc);
            }
        }
    }

    /// Returns `true` if `kc` is one of the configured trigger keycodes.
    #[inline]
    #[must_use]
    pub fn is_trigger_keycode(&self, kc: Keycode) -> bool {
        test_bit(&self.trigger_keycodes, kc)
    }

    /// Returns `true` if a drag is currently armed.
    #[inline]
    #[must_use]
    pub const fn is_armed(&self) -> bool {
        self.armed
    }

    /// Returns the X11 window ID of the current `XdndSelection` owner.
    #[inline]
    #[must_use]
    pub const fn owner(&self) -> Window {
        self.owner
    }

    /// Returns the X server timestamp recorded when `XdndSelection` was acquired.
    #[inline]
    #[must_use]
    pub const fn selection_timestamp(&self) -> u32 {
        self.selection_timestamp
    }

    /// Returns the [`SourceInfo`] of the currently armed drag source.
    #[inline]
    #[must_use]
    pub fn source(&self) -> &SourceInfo {
        &self.source
    }

    /// Notifies the tracker whether the drag-mode radial wheel session is currently open.
    pub fn set_session_open(&mut self, open: bool) {
        self.session_open = open;
        if !open {
            // Clear any 1s session watchdog if the session closed normally
            self.button_release_deadline = None;
        }
    }

    /// Disarms the tracker and clears all per-drag state.
    pub fn disarm(&mut self) -> bool {
        let was_armed = self.armed;
        self.armed = false;
        self.owner = x11rb::NONE;
        self.selection_timestamp = x11rb::CURRENT_TIME;
        self.source = SourceInfo::default();
        self.held_at_arm = false;
        self.poll_modifier_was_down = false;
        self.session_open = false;
        self.down_keys = [0; 4];
        self.armed_at = None;
        self.button_release_deadline = None;
        self.poll_deadline = None;
        was_armed
    }

    /// Handles an `XFixesSelectionNotify` event on `XdndSelection`.
    #[allow(clippy::too_many_arguments)]
    pub fn on_selection_owner_changed(
        &mut self,
        new_owner: Window,
        our_windows: (Window, Window),
        button_held: bool,
        modifier_held: bool,
        selection_ts: u32,
        source: SourceInfo,
        poll_mode: bool,
        now: Instant,
    ) -> Option<ArmingTransition> {
        let is_foreign =
            new_owner != x11rb::NONE && new_owner != our_windows.0 && new_owner != our_windows.1;

        if is_foreign {
            if !button_held {
                // Selection changed without a mouse button held; ignore or disarm if previously armed
                if self.disarm() {
                    return Some(ArmingTransition::Disarmed);
                }
                return None;
            }

            self.armed = true;
            self.owner = new_owner;
            self.selection_timestamp = selection_ts;
            self.source = source.clone();
            self.held_at_arm = modifier_held;
            self.poll_modifier_was_down = modifier_held;
            self.down_keys = [0; 4];
            self.session_open = false;
            self.armed_at = Some(now);
            self.button_release_deadline = None;
            self.poll_deadline = if poll_mode {
                Some(now + POLL_TRIGGER_INTERVAL)
            } else {
                None
            };
            Some(ArmingTransition::Armed { source })
        } else if self.disarm() {
            Some(ArmingTransition::Disarmed)
        } else {
            None
        }
    }

    /// Processes an XI2 `RawKeyPress` event while armed.
    pub fn on_raw_key_press(&mut self, keycode: Keycode) -> Option<TriggerEdge> {
        if !self.armed || !self.is_trigger_keycode(keycode) {
            return None;
        }
        // Ignore X11 auto-repeat if this keycode is already recorded as down
        if test_bit(&self.down_keys, keycode) {
            return None;
        }
        set_bit(&mut self.down_keys, keycode);

        if self.held_at_arm {
            return None;
        }

        if count_bits(&self.down_keys) == 1 {
            Some(TriggerEdge::FreshPress)
        } else {
            None
        }
    }

    /// Processes an XI2 `RawKeyRelease` event while armed.
    pub fn on_raw_key_release(&mut self, keycode: Keycode) -> Option<TriggerEdge> {
        if !self.armed || !self.is_trigger_keycode(keycode) {
            return None;
        }
        let was_down = test_bit(&self.down_keys, keycode);
        clear_bit(&mut self.down_keys, keycode);

        if count_bits(&self.down_keys) == 0 {
            if self.held_at_arm {
                // Pre-held modifier has now been fully released; subsequent press will be fresh
                self.held_at_arm = false;
                None
            } else if was_down {
                Some(TriggerEdge::Released)
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Processes an XI2 `RawButtonRelease` event while armed.
    pub fn on_raw_button_release(&mut self, button: u8, now: Instant) {
        if !self.armed || !(1..=3).contains(&button) {
            return;
        }
        let delay = if self.session_open {
            SESSION_BUTTON_RELEASE_WATCHDOG
        } else {
            BUTTON_RELEASE_DISARM_DELAY
        };
        self.button_release_deadline = Some(now + delay);
    }

    /// Processes a `QueryPointer` sample in `Poll` mode (`8 ms` interval).
    pub fn on_poll_sample(
        &mut self,
        button_held: bool,
        modifier_held: bool,
        now: Instant,
    ) -> SmallVec<[TriggerEdge; 2]> {
        let mut edges = SmallVec::new();
        if !self.armed {
            return edges;
        }
        self.poll_deadline = Some(now + POLL_TRIGGER_INTERVAL);

        if !button_held && self.button_release_deadline.is_none() {
            let delay = if self.session_open {
                SESSION_BUTTON_RELEASE_WATCHDOG
            } else {
                BUTTON_RELEASE_DISARM_DELAY
            };
            self.button_release_deadline = Some(now + delay);
        } else if button_held {
            self.button_release_deadline = None;
        }

        if !modifier_held && self.held_at_arm {
            self.held_at_arm = false;
        } else if modifier_held && !self.poll_modifier_was_down && !self.held_at_arm {
            edges.push(TriggerEdge::FreshPress);
        } else if !modifier_held && self.poll_modifier_was_down && !self.held_at_arm {
            edges.push(TriggerEdge::Released);
        }

        self.poll_modifier_was_down = modifier_held;
        edges
    }

    /// Returns the earliest timer deadline required by this tracker.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        if !self.armed {
            return None;
        }
        let mut earliest = self.armed_at.map(|t| t + MAX_ARMED_DURATION);
        for candidate in [self.button_release_deadline, self.poll_deadline]
            .into_iter()
            .flatten()
        {
            earliest = Some(match earliest {
                Some(cur) => cur.min(candidate),
                None => candidate,
            });
        }
        earliest
    }

    /// Inspects deadlines at `now` and returns which actions the caller must take.
    pub fn check_deadlines(&mut self, now: Instant) -> ArmingDeadlineCheck {
        if !self.armed {
            return ArmingDeadlineCheck::default();
        }

        if let Some(armed_at) = self.armed_at
            && now >= armed_at + MAX_ARMED_DURATION
        {
            self.disarm();
            return ArmingDeadlineCheck {
                max_duration_expired: true,
                verify_button_release: false,
                poll_due: false,
            };
        }

        let mut verify_button_release = false;
        if let Some(dl) = self.button_release_deadline
            && now >= dl
        {
            self.button_release_deadline = None;
            verify_button_release = true;
        }

        let mut poll_due = false;
        if let Some(dl) = self.poll_deadline
            && now >= dl
        {
            self.poll_deadline = None;
            poll_due = true;
        }

        ArmingDeadlineCheck {
            max_duration_expired: false,
            verify_button_release,
            poll_due,
        }
    }
}

#[inline]
fn set_bit(bits: &mut [u64; 4], kc: u8) {
    let idx = (kc >> 6) as usize;
    let shift = kc & 63;
    bits[idx] |= 1u64 << shift;
}

#[inline]
fn clear_bit(bits: &mut [u64; 4], kc: u8) {
    let idx = (kc >> 6) as usize;
    let shift = kc & 63;
    bits[idx] &= !(1u64 << shift);
}

#[inline]
fn test_bit(bits: &[u64; 4], kc: u8) -> bool {
    let idx = (kc >> 6) as usize;
    let shift = kc & 63;
    (bits[idx] & (1u64 << shift)) != 0
}

#[inline]
fn count_bits(bits: &[u64; 4]) -> u32 {
    bits[0].count_ones() + bits[1].count_ones() + bits[2].count_ones() + bits[3].count_ones()
}

/// Subscribes `helper_window` to `XFixesSelectionNotify` events for `xdnd_selection`.
pub fn select_xdnd_selection_notify(
    conn: &RustConnection,
    helper_window: Window,
    xdnd_selection: u32,
) -> Result<(), PlatformError> {
    let mask = SelectionEventMask::SET_SELECTION_OWNER
        | SelectionEventMask::SELECTION_WINDOW_DESTROY
        | SelectionEventMask::SELECTION_CLIENT_CLOSE;
    conn.xfixes_select_selection_input(helper_window, xdnd_selection, mask)
        .map_err(|e| PlatformError::X11(e.to_string()))?;
    Ok(())
}

/// Enables or disables XI2 `RawKeyPress | RawKeyRelease | RawButtonRelease` on `root`
/// (`PLATFORM_INTEGRATION.md` §3.6, `ARCHITECTURE.md` §10).
pub fn select_xi2_raw_events(
    conn: &RustConnection,
    root: Window,
    enable: bool,
) -> Result<(), PlatformError> {
    let mask = if enable {
        XIEventMask::RAW_KEY_PRESS | XIEventMask::RAW_KEY_RELEASE | XIEventMask::RAW_BUTTON_RELEASE
    } else {
        XIEventMask::default()
    };
    let event_mask = xinput::EventMask {
        deviceid: u16::from(xinput::Device::ALL_MASTER),
        mask: vec![mask],
    };
    conn.xinput_xi_select_events(root, &[event_mask])
        .map_err(|e| PlatformError::X11(e.to_string()))?;
    let _ = conn.flush();
    Ok(())
}

/// Queries the X11 modifier map and returns all non-zero keycodes for `modifier`.
#[must_use]
pub fn query_modifier_keycodes(conn: &RustConnection, modifier: TriggerModifier) -> Vec<Keycode> {
    let Some(row_index) = modifier_row_index(modifier) else {
        return Vec::new();
    };
    let Ok(cookie) = conn.get_modifier_mapping() else {
        return Vec::new();
    };
    let Ok(reply) = cookie.reply() else {
        return Vec::new();
    };
    let kpm = reply.keycodes_per_modifier() as usize;
    let start = row_index.saturating_mul(kpm);
    let end = start.saturating_add(kpm).min(reply.keycodes.len());
    reply.keycodes[start..end]
        .iter()
        .copied()
        .filter(|&kc| kc != 0)
        .collect()
}

const fn modifier_row_index(modifier: TriggerModifier) -> Option<usize> {
    // Standard X11 modifier map rows:
    // 0 = Shift, 1 = Lock, 2 = Control, 3 = Mod1 (Alt), 4 = Mod2, 5 = Mod3, 6 = Mod4 (Super), 7 = Mod5
    match modifier {
        TriggerModifier::Shift => Some(0),
        TriggerModifier::Ctrl => Some(2),
        TriggerModifier::Alt => Some(3),
        TriggerModifier::Super => Some(6),
        TriggerModifier::Disabled => None,
    }
}

/// Returns the `KeyButMask` corresponding to `modifier`.
#[must_use]
pub fn modifier_keybut_mask(modifier: TriggerModifier) -> KeyButMask {
    match modifier {
        TriggerModifier::Shift => KeyButMask::SHIFT,
        TriggerModifier::Ctrl => KeyButMask::CONTROL,
        TriggerModifier::Alt => KeyButMask::MOD1,
        TriggerModifier::Super => KeyButMask::MOD4,
        TriggerModifier::Disabled => KeyButMask::default(),
    }
}

/// Queries root pointer position, whether any primary/middle/secondary mouse button is held,
/// and whether `modifier` is currently held.
pub fn query_pointer_state(
    conn: &RustConnection,
    root: Window,
    modifier: TriggerModifier,
) -> Result<(PhysPoint, bool, bool), PlatformError> {
    let reply = conn
        .query_pointer(root)
        .map_err(|e| PlatformError::X11(e.to_string()))?
        .reply()
        .map_err(|e| PlatformError::X11(e.to_string()))?;
    let pt = PhysPoint::new(i32::from(reply.root_x), i32::from(reply.root_y));
    let button_held = reply
        .mask
        .intersects(KeyButMask::BUTTON1 | KeyButMask::BUTTON2 | KeyButMask::BUTTON3);
    let mod_mask = modifier_keybut_mask(modifier);
    let modifier_held = u16::from(mod_mask) != 0 && reply.mask.contains(mod_mask);
    Ok((pt, button_held, modifier_held))
}
