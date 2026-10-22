//! Pure XDND v5 target state machine and X11 `ClientMessage` helpers
//! (`PLATFORM_INTEGRATION.md` §3.7, `FEASIBILITY_STUDY.md` §6).
//!
//! Safety invariants:
//! - Always replies with `XdndActionPrivate` when accepting and `x11rb::NONE` (`0`) when rejecting.
//! - **Never** replies with `XdndActionMove` and **never** requests the `DELETE` selection target.

use smallvec::SmallVec;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageData, ClientMessageEvent, ConnectionExt as _, EventMask, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::x11::atoms::Atoms;
use crate::{PhysPoint, PlatformError};

/// Maximum XDND protocol version supported by Radivine.
pub const XDND_MAX_VERSION: u8 = 5;

/// Relevant XDND atom IDs passed into the pure [`XdndTarget`] state machine so it can be
/// unit-tested without an X11 server connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XdndAtoms {
    /// `XdndEnter` atom.
    pub enter: Atom,
    /// `XdndPosition` atom.
    pub position: Atom,
    /// `XdndStatus` atom.
    pub status: Atom,
    /// `XdndLeave` atom.
    pub leave: Atom,
    /// `XdndDrop` atom.
    pub drop: Atom,
    /// `XdndFinished` atom.
    pub finished: Atom,
    /// `XdndActionPrivate` atom.
    pub action_private: Atom,
}

impl XdndAtoms {
    /// Extracts [`XdndAtoms`] from the interned [`Atoms`] table.
    #[must_use]
    pub const fn from_atoms(atoms: &Atoms) -> Self {
        Self {
            enter: atoms.XdndEnter,
            position: atoms.XdndPosition,
            status: atoms.XdndStatus,
            leave: atoms.XdndLeave,
            drop: atoms.XdndDrop,
            finished: atoms.XdndFinished,
            action_private: atoms.XdndActionPrivate,
        }
    }
}

/// Parsed event emitted by [`XdndTarget::on_message`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DndMessageEvent {
    /// `XdndEnter` from `source`.
    Enter {
        /// Drag source window ID.
        source: Window,
        /// Negotiated XDND protocol version (`<= 5`).
        version: u8,
        /// Up to 3 inline type atoms from `l[2..5]`.
        inline_types: SmallVec<[Atom; 3]>,
        /// `true` if bit 0 of `l[1]` is set (caller should read `XdndTypeList` property on `source`).
        more_types: bool,
    },
    /// `XdndPosition` from the active `source`.
    Position {
        /// Drag source window ID.
        source: Window,
        /// Root physical pointer position (`(l2 >> 16) as i16`, `(l2 & 0xFFFF) as i16`).
        at: PhysPoint,
        /// X server timestamp (`l[3]`).
        timestamp: u32,
        /// Source-requested action atom (`l[4]`).
        requested_action: Atom,
    },
    /// `XdndLeave` from the active `source`.
    Leave {
        /// Drag source window ID.
        source: Window,
    },
    /// `XdndDrop` from the active `source`.
    Drop {
        /// Drag source window ID.
        source: Window,
        /// Drop timestamp (`l[2]`).
        timestamp: u32,
    },
}

/// Outgoing `ClientMessage` payload built by [`XdndTarget`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutgoingClientMessage {
    /// Destination window (`source`).
    pub destination: Window,
    /// Message type atom (`XdndStatus` or `XdndFinished`).
    pub msg_type: Atom,
    /// Five 32-bit `data32` words (`l[0..5]`).
    pub data: [u32; 5],
}

/// Pure XDND v5 target state machine (`PLATFORM_INTEGRATION.md` §3.7).
#[derive(Clone, Debug)]
pub struct XdndTarget {
    overlay_window: Window,
    atoms: XdndAtoms,
    current_source: Window,
    drop_source: Window,
    version: u8,
    last_position: Option<PhysPoint>,
    last_position_timestamp: Option<u32>,
    drop_timestamp: Option<u32>,
}

impl XdndTarget {
    /// Creates a new idle [`XdndTarget`] for `overlay_window`.
    #[must_use]
    pub const fn new(overlay_window: Window, atoms: XdndAtoms) -> Self {
        Self {
            overlay_window,
            atoms,
            current_source: x11rb::NONE,
            drop_source: x11rb::NONE,
            version: XDND_MAX_VERSION,
            last_position: None,
            last_position_timestamp: None,
            drop_timestamp: None,
        }
    }

    /// Resets all active XDND session state.
    pub fn reset(&mut self) {
        self.current_source = x11rb::NONE;
        self.drop_source = x11rb::NONE;
        self.version = XDND_MAX_VERSION;
        self.last_position = None;
        self.last_position_timestamp = None;
        self.drop_timestamp = None;
    }

    /// Returns the currently entered drag source window ID, or `x11rb::NONE` (`0`).
    #[inline]
    #[must_use]
    pub const fn current_source(&self) -> Window {
        self.current_source
    }

    /// Returns `true` if a drag source is currently entered on the overlay.
    #[inline]
    #[must_use]
    pub const fn is_entered(&self) -> bool {
        self.current_source != x11rb::NONE
    }

    /// Returns the most recent root pointer coordinates reported by `XdndPosition`.
    #[inline]
    #[must_use]
    pub const fn last_position(&self) -> Option<PhysPoint> {
        self.last_position
    }

    /// Returns the best X server timestamp to use for `ConvertSelection(XdndSelection)`
    /// (`PLATFORM_INTEGRATION.md` §3.8):
    /// 1. Drop timestamp if dropping,
    /// 2. Last `XdndPosition` timestamp,
    /// 3. XFixes `selection_timestamp` from arming,
    /// 4. `CURRENT_TIME` (`0`).
    #[must_use]
    pub fn effective_selection_timestamp(&self, xfixes_selection_ts: u32) -> u32 {
        if let Some(ts) = self.drop_timestamp
            && ts != x11rb::CURRENT_TIME
        {
            return ts;
        }
        if let Some(ts) = self.last_position_timestamp
            && ts != x11rb::CURRENT_TIME
        {
            return ts;
        }
        if xfixes_selection_ts != x11rb::CURRENT_TIME {
            return xfixes_selection_ts;
        }
        x11rb::CURRENT_TIME
    }

    /// Processes an incoming format-32 `ClientMessage` (`msg_type`, `data32`).
    ///
    /// Stale messages whose `l[0]` does not match `current_source` are ignored.
    pub fn on_message(&mut self, msg_type: Atom, d: [u32; 5]) -> Option<DndMessageEvent> {
        if msg_type == self.atoms.enter {
            let source = d[0];
            if source == x11rb::NONE {
                return None;
            }
            let more_types = (d[1] & 1) != 0;
            let raw_version = ((d[1] >> 24) & 0xFF) as u8;
            let version = raw_version.min(XDND_MAX_VERSION);

            let mut inline_types = SmallVec::new();
            for &atom in &d[2..5] {
                if atom != x11rb::NONE {
                    inline_types.push(atom);
                }
            }

            self.current_source = source;
            self.drop_source = x11rb::NONE;
            self.version = version;
            self.drop_timestamp = None;

            Some(DndMessageEvent::Enter {
                source,
                version,
                inline_types,
                more_types,
            })
        } else if msg_type == self.atoms.position {
            let source = d[0];
            if source == x11rb::NONE || source != self.current_source {
                return None;
            }
            let x = ((d[2] >> 16) & 0xFFFF) as i16 as i32;
            let y = (d[2] & 0xFFFF) as i16 as i32;
            let at = PhysPoint::new(x, y);
            let timestamp = d[3];
            let requested_action = d[4];

            self.last_position = Some(at);
            self.last_position_timestamp = Some(timestamp);

            Some(DndMessageEvent::Position {
                source,
                at,
                timestamp,
                requested_action,
            })
        } else if msg_type == self.atoms.leave {
            let source = d[0];
            if source == x11rb::NONE || source != self.current_source {
                return None;
            }
            self.current_source = x11rb::NONE;
            self.drop_source = x11rb::NONE;
            self.drop_timestamp = None;
            Some(DndMessageEvent::Leave { source })
        } else if msg_type == self.atoms.drop {
            let source = d[0];
            if source == x11rb::NONE || source != self.current_source {
                return None;
            }
            let timestamp = d[2];
            self.drop_source = source;
            self.drop_timestamp = Some(timestamp);
            Some(DndMessageEvent::Drop { source, timestamp })
        } else {
            None
        }
    }

    /// Builds an outgoing `XdndStatus` `ClientMessage` for the currently entered drag source
    /// (`PLATFORM_INTEGRATION.md` §3.7).
    ///
    /// Uses `XdndActionPrivate` when `accept == true` and `x11rb::NONE` (`0`) when `accept == false`.
    #[must_use]
    pub const fn status(&self, accept: bool) -> Option<OutgoingClientMessage> {
        if self.current_source == x11rb::NONE {
            return None;
        }
        let flags = if accept { 1u32 | 2u32 } else { 2u32 };
        let action = if accept {
            self.atoms.action_private
        } else {
            x11rb::NONE
        };
        Some(OutgoingClientMessage {
            destination: self.current_source,
            msg_type: self.atoms.status,
            data: [self.overlay_window, flags, 0, 0, action],
        })
    }

    /// Builds an outgoing `XdndFinished` `ClientMessage` for the dropping (or currently entered)
    /// drag source and resets the target state (`PLATFORM_INTEGRATION.md` §3.7).
    pub fn finished(&mut self, success: bool) -> Option<OutgoingClientMessage> {
        let dest = if self.drop_source != x11rb::NONE {
            self.drop_source
        } else if self.current_source != x11rb::NONE {
            self.current_source
        } else {
            return None;
        };

        let accepted_bit = u32::from(success);
        let action = if success && self.version >= 5 {
            self.atoms.action_private
        } else {
            x11rb::NONE
        };

        self.reset();

        Some(OutgoingClientMessage {
            destination: dest,
            msg_type: self.atoms.finished,
            data: [self.overlay_window, accepted_bit, action, 0, 0],
        })
    }
}

/// Resolves the list of offered MIME type names on `XdndEnter`, reading `XdndTypeList` from
/// `source` when `more_types` is `true` (`PLATFORM_INTEGRATION.md` §3.7).
#[must_use]
pub fn resolve_enter_types(
    conn: &RustConnection,
    atoms: &Atoms,
    source: Window,
    inline_types: &[Atom],
    more_types: bool,
) -> Vec<String> {
    let mut type_atoms: Vec<Atom> = Vec::new();
    if more_types
        && let Ok(cookie) =
            conn.get_property(false, source, atoms.XdndTypeList, AtomEnum::ATOM, 0, 128)
        && let Ok(reply) = cookie.reply()
        && let Some(iter) = reply.value32()
    {
        type_atoms.extend(iter.filter(|&a| a != x11rb::NONE));
    }
    if type_atoms.is_empty() {
        type_atoms.extend(inline_types.iter().copied().filter(|&a| a != x11rb::NONE));
    }

    let mut names = Vec::with_capacity(type_atoms.len());
    for atom in type_atoms {
        if atom == atoms.URI_LIST {
            names.push("text/uri-list".to_string());
        } else if atom == atoms.UTF8_STRING {
            names.push("UTF8_STRING".to_string());
        } else if atom == atoms.TEXT {
            names.push("TEXT".to_string());
        } else if atom == atoms.PLAIN_UTF8 {
            names.push("text/plain;charset=utf-8".to_string());
        } else if atom == atoms.GNOME_ICON_LIST {
            names.push("x-special/gnome-icon-list".to_string());
        } else if atom == atoms.GNOME_COPIED_FILES {
            names.push("x-special/gnome-copied-files".to_string());
        } else if let Ok(cookie) = conn.get_atom_name(atom)
            && let Ok(reply) = cookie.reply()
        {
            names.push(String::from_utf8_lossy(&reply.name).into_owned());
        }
    }
    names
}

/// Sends a format-32 `ClientMessage` to `msg.destination` with `propagate = false` and
/// `event_mask = NO_EVENT` (`PLATFORM_INTEGRATION.md` §3.7).
pub fn send_client_message(
    conn: &RustConnection,
    msg: OutgoingClientMessage,
) -> Result<(), PlatformError> {
    let event = ClientMessageEvent {
        response_type: x11rb::protocol::xproto::CLIENT_MESSAGE_EVENT,
        format: 32,
        sequence: 0,
        window: msg.destination,
        type_: msg.msg_type,
        data: ClientMessageData::from(msg.data),
    };
    conn.send_event(false, msg.destination, EventMask::NO_EVENT, event)
        .map_err(|e| PlatformError::X11(e.to_string()))?;
    conn.flush()
        .map_err(|e| PlatformError::X11(e.to_string()))?;
    Ok(())
}
