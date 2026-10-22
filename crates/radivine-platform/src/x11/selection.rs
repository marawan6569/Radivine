//! XDND selection conversion (`ConvertSelection`), property retrieval, and `INCR` multi-chunk
//! receiver (`PLATFORM_INTEGRATION.md` §3.8).
//!
//! Safety invariants:
//! - Enforces a `2 s` transfer timeout ([`PayloadError::Timeout`]) and `16 MiB` size cap ([`PayloadError::TooLarge`]).
//! - **Never** requests the `DELETE` selection target (`PLATFORM_INTEGRATION.md` §3.7, `ARCHITECTURE.md` §10).

use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, Property, PropertyNotifyEvent, SelectionNotifyEvent, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::x11::atoms::Atoms;
use crate::{PayloadError, PlatformError};

/// Maximum time allowed for a selection payload transfer (`2 s`).
pub const PAYLOAD_TIMEOUT: Duration = Duration::from_secs(2);

/// Maximum payload size accepted from a drag source (`16 MiB`).
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

/// Pure state machine tracking an in-flight `XdndSelection` conversion (single-shot or `INCR`)
/// (`PLATFORM_INTEGRATION.md` §3.8).
#[derive(Clone, Debug)]
pub struct SelectionReader {
    overlay_window: Window,
    property_atom: Atom,
    incr_atom: Atom,
    active_mime: Option<String>,
    incr_active: bool,
    incr_buf: Vec<u8>,
    deadline: Option<Instant>,
}

impl SelectionReader {
    /// Creates a new idle [`SelectionReader`].
    #[must_use]
    pub const fn new(overlay_window: Window, property_atom: Atom, incr_atom: Atom) -> Self {
        Self {
            overlay_window,
            property_atom,
            incr_atom,
            active_mime: None,
            incr_active: false,
            incr_buf: Vec::new(),
            deadline: None,
        }
    }

    /// Starts tracking a new selection request for `mime` initiated at `now`.
    pub fn begin(&mut self, mime: &str, now: Instant) {
        self.active_mime = Some(mime.to_string());
        self.incr_active = false;
        self.incr_buf.clear();
        self.deadline = Some(now + PAYLOAD_TIMEOUT);
    }

    /// Cancels any in-flight selection transfer.
    pub fn cancel(&mut self) {
        self.active_mime = None;
        self.incr_active = false;
        self.incr_buf.clear();
        self.deadline = None;
    }

    /// Returns `true` if a selection transfer is currently in flight.
    #[inline]
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active_mime.is_some()
    }

    /// Returns `true` if an `INCR` multi-chunk transfer is currently active.
    #[inline]
    #[must_use]
    pub const fn is_incr(&self) -> bool {
        self.incr_active
    }

    /// Returns the timeout deadline for the current transfer, if active.
    #[inline]
    #[must_use]
    pub const fn next_deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Checks whether the `2 s` transfer timeout has elapsed at `now`.
    pub fn check_timeout(
        &mut self,
        now: Instant,
    ) -> Option<(String, Result<Vec<u8>, PayloadError>)> {
        let dl = self.deadline?;
        if now >= dl {
            let mime = self.active_mime.take()?;
            self.cancel();
            Some((mime, Err(PayloadError::Timeout)))
        } else {
            None
        }
    }

    /// Processes the result of `SelectionNotify` and the initial `GetProperty(delete=true)` read.
    ///
    /// - If `notify_property == x11rb::NONE`, returns `Err(PayloadError::Refused)`.
    /// - If `prop_type == INCR`, transitions into incremental chunk assembly and returns `None`.
    /// - Otherwise validates size (`<= 16 MiB`) and returns `Ok(prop_bytes)`.
    pub fn on_selection_notify_property(
        &mut self,
        notify_property: Atom,
        prop_type: Atom,
        prop_bytes: Vec<u8>,
        lower_bound_hint: usize,
    ) -> Option<(String, Result<Vec<u8>, PayloadError>)> {
        if !self.is_active() {
            return None;
        }

        if notify_property == x11rb::NONE {
            let mime = self.active_mime.take()?;
            self.cancel();
            return Some((mime, Err(PayloadError::Refused)));
        }

        if prop_type == self.incr_atom {
            if lower_bound_hint > MAX_PAYLOAD_BYTES {
                let mime = self.active_mime.take()?;
                self.cancel();
                return Some((mime, Err(PayloadError::TooLarge)));
            }
            self.incr_active = true;
            self.incr_buf.clear();
            self.incr_buf.reserve(lower_bound_hint.min(1024 * 1024));
            return None;
        }

        if prop_bytes.len() > MAX_PAYLOAD_BYTES {
            let mime = self.active_mime.take()?;
            self.cancel();
            return Some((mime, Err(PayloadError::TooLarge)));
        }

        let mime = self.active_mime.take()?;
        self.cancel();
        Some((mime, Ok(prop_bytes)))
    }

    /// Appends a chunk read on `PropertyNotify(NEW_VALUE)` during an `INCR` transfer.
    ///
    /// An empty `chunk` (`0` bytes) signals the end of the `INCR` stream and completes the transfer.
    pub fn on_incr_chunk(
        &mut self,
        chunk: &[u8],
    ) -> Option<(String, Result<Vec<u8>, PayloadError>)> {
        if !self.is_active() || !self.incr_active {
            return None;
        }

        if chunk.is_empty() {
            let mime = self.active_mime.take()?;
            let completed = std::mem::take(&mut self.incr_buf);
            self.cancel();
            return Some((mime, Ok(completed)));
        }

        if self.incr_buf.len().saturating_add(chunk.len()) > MAX_PAYLOAD_BYTES {
            let mime = self.active_mime.take()?;
            self.cancel();
            return Some((mime, Err(PayloadError::TooLarge)));
        }

        self.incr_buf.extend_from_slice(chunk);
        None
    }

    /// Returns the configured destination property atom (`_RADIVINE_DND`).
    #[inline]
    #[must_use]
    pub const fn property_atom(&self) -> Atom {
        self.property_atom
    }

    /// Returns the requestor overlay window ID.
    #[inline]
    #[must_use]
    pub const fn overlay_window(&self) -> Window {
        self.overlay_window
    }
}

/// Resolves a MIME type string into its X11 `Atom`, refusing `DELETE` for file safety.
pub fn resolve_mime_atom(
    conn: &RustConnection,
    atoms: &Atoms,
    mime: &str,
) -> Result<Atom, PlatformError> {
    if mime.trim().eq_ignore_ascii_case("DELETE") {
        return Err(PlatformError::X11(
            "refusing to convert forbidden DELETE target".to_string(),
        ));
    }
    match mime {
        "text/uri-list" => Ok(atoms.URI_LIST),
        "UTF8_STRING" => Ok(atoms.UTF8_STRING),
        "TEXT" => Ok(atoms.TEXT),
        "text/plain;charset=utf-8" => Ok(atoms.PLAIN_UTF8),
        "x-special/gnome-icon-list" => Ok(atoms.GNOME_ICON_LIST),
        "x-special/gnome-copied-files" => Ok(atoms.GNOME_COPIED_FILES),
        _ => {
            let reply = conn
                .intern_atom(false, mime.as_bytes())
                .map_err(|e| PlatformError::X11(e.to_string()))?
                .reply()
                .map_err(|e| PlatformError::X11(e.to_string()))?;
            Ok(reply.atom)
        }
    }
}

/// Initiates an `XdndSelection` conversion into `_RADIVINE_DND` on `reader.overlay_window()`.
pub fn request_selection_wire(
    conn: &RustConnection,
    atoms: &Atoms,
    reader: &mut SelectionReader,
    mime: &str,
    timestamp: u32,
    now: Instant,
) -> Result<(), PlatformError> {
    let target_atom = resolve_mime_atom(conn, atoms, mime)?;
    let win = reader.overlay_window();
    let prop = reader.property_atom();

    // Delete any stale destination property before calling ConvertSelection
    let _ = conn.delete_property(win, prop);
    conn.convert_selection(win, atoms.XdndSelection, target_atom, prop, timestamp)
        .map_err(|e| PlatformError::X11(e.to_string()))?;
    conn.flush()
        .map_err(|e| PlatformError::X11(e.to_string()))?;

    reader.begin(mime, now);
    Ok(())
}

/// Handles an incoming `SelectionNotify` event on the overlay window.
pub fn handle_selection_notify_wire(
    conn: &RustConnection,
    reader: &mut SelectionReader,
    sn: SelectionNotifyEvent,
) -> Option<(String, Result<Vec<u8>, PayloadError>)> {
    if !reader.is_active() || sn.requestor != reader.overlay_window() {
        return None;
    }

    if sn.property == x11rb::NONE {
        return reader.on_selection_notify_property(x11rb::NONE, x11rb::NONE, Vec::new(), 0);
    }

    let prop_atom = reader.property_atom();
    // Read up to MAX_PAYLOAD_BYTES + 16 bytes so payloads exceeding 16 MiB are caught
    let max_longs = ((MAX_PAYLOAD_BYTES / 4) + 4) as u32;
    let reply = match conn
        .get_property(
            true,
            reader.overlay_window(),
            prop_atom,
            AtomEnum::ANY,
            0,
            max_longs,
        )
        .ok()
        .and_then(|c| c.reply().ok())
    {
        Some(r) => r,
        None => {
            let mime = reader.active_mime.take()?;
            reader.cancel();
            return Some((
                mime,
                Err(PayloadError::Protocol(
                    "GetProperty failed on SelectionNotify".to_string(),
                )),
            ));
        }
    };

    let lower_bound = reply
        .value32()
        .and_then(|mut it| it.next())
        .map_or(0, |v| v as usize);
    let _ = conn.flush();

    reader.on_selection_notify_property(sn.property, reply.type_, reply.value, lower_bound)
}

/// Handles a `PropertyNotify(NEW_VALUE)` event on `_RADIVINE_DND` during an active `INCR` transfer.
pub fn handle_property_notify_wire(
    conn: &RustConnection,
    reader: &mut SelectionReader,
    pn: PropertyNotifyEvent,
) -> Option<(String, Result<Vec<u8>, PayloadError>)> {
    if !reader.is_incr()
        || pn.window != reader.overlay_window()
        || pn.atom != reader.property_atom()
        || pn.state != Property::NEW_VALUE
    {
        return None;
    }

    let max_longs = ((MAX_PAYLOAD_BYTES / 4) + 4) as u32;
    let reply = match conn
        .get_property(
            true,
            reader.overlay_window(),
            reader.property_atom(),
            AtomEnum::ANY,
            0,
            max_longs,
        )
        .ok()
        .and_then(|c| c.reply().ok())
    {
        Some(r) => r,
        None => {
            let mime = reader.active_mime.take()?;
            reader.cancel();
            return Some((
                mime,
                Err(PayloadError::Protocol(
                    "GetProperty failed during INCR chunk read".to_string(),
                )),
            ));
        }
    };
    let _ = conn.flush();

    reader.on_incr_chunk(&reply.value)
}
