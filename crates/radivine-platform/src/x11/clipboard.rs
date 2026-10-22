//! X11 `CLIPBOARD` selection ownership service (`PLATFORM_INTEGRATION.md` §3.12).
//!
//! Serves file copy payloads (`x-special/gnome-copied-files`, `text/uri-list`, `UTF8_STRING`,
//! `text/plain;charset=utf-8`, `text/plain`, `STRING`, `TEXT`, `TARGETS`, `TIMESTAMP`,
//! `SAVE_TARGETS`, `MULTIPLE`, and `INCR` chunked transfers) from the persistent hidden helper
//! window so copied files can be pasted into Nemo or text editors at any later time.

use std::collections::HashMap;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask, PropMode, Property,
    PropertyNotifyEvent, SELECTION_NOTIFY_EVENT, SelectionClearEvent, SelectionNotifyEvent,
    SelectionRequestEvent, Window,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::x11::atoms::Atoms;
use crate::{ClipOp, ClipboardContent, PlatformError};

/// Payloads larger than `256 KiB` are transferred via ICCCM `INCR` chunking (`PLATFORM_INTEGRATION.md` §3.12).
pub const INCR_CHUNK_THRESHOLD: usize = 256 * 1024;

#[derive(Clone, Debug)]
struct IncrTransfer {
    target: Atom,
    data: Vec<u8>,
    offset: usize,
    completed_final_zero: bool,
}

/// Manages X11 `CLIPBOARD` ownership on `helper_window` (`PLATFORM_INTEGRATION.md` §3.12).
#[derive(Debug)]
pub struct ClipboardService {
    owner_window: Window,
    content: Option<ClipboardContent>,
    owned_since: u32,
    incr_transfers: HashMap<(Window, Atom), IncrTransfer>,
}

impl ClipboardService {
    /// Creates a new [`ClipboardService`] bound to `owner_window` (`overlay.helper_window`).
    #[must_use]
    pub fn new(owner_window: Window) -> Self {
        Self {
            owner_window,
            content: None,
            owned_since: x11rb::CURRENT_TIME,
            incr_transfers: HashMap::new(),
        }
    }

    /// Returns `true` if Radivine currently holds clipboard content to serve.
    #[must_use]
    pub fn has_content(&self) -> bool {
        self.content.is_some()
    }

    /// Acquires ownership of the X11 `CLIPBOARD` selection and stores `content` to serve (`PLATFORM_INTEGRATION.md` §3.12).
    pub fn set_clipboard(
        &mut self,
        conn: &RustConnection,
        atoms: &Atoms,
        content: ClipboardContent,
    ) -> Result<(), PlatformError> {
        conn.set_selection_owner(self.owner_window, atoms.CLIPBOARD, x11rb::CURRENT_TIME)
            .map_err(|e| PlatformError::X11(e.to_string()))?;
        conn.flush()
            .map_err(|e| PlatformError::X11(e.to_string()))?;

        let owner = conn
            .get_selection_owner(atoms.CLIPBOARD)
            .map_err(|e| PlatformError::X11(e.to_string()))?
            .reply()
            .map_err(|e| PlatformError::X11(e.to_string()))?
            .owner;

        if owner != self.owner_window {
            return Err(PlatformError::X11(
                "X server did not grant CLIPBOARD selection ownership".to_string(),
            ));
        }

        self.content = Some(content);
        self.owned_since = x11rb::CURRENT_TIME;
        self.incr_transfers.clear();
        Ok(())
    }

    /// Handles `SelectionClear` on `owner_window`: drops stored clipboard data and any in-flight `INCR` transfers.
    pub fn on_selection_clear(&mut self, ev: SelectionClearEvent, clipboard_atom: Atom) {
        if ev.owner == self.owner_window && ev.selection == clipboard_atom {
            self.content = None;
            self.incr_transfers.clear();
        }
    }

    /// Handles `SelectionRequest` directed at `owner_window` for `CLIPBOARD`.
    pub fn on_selection_request(
        &mut self,
        conn: &RustConnection,
        atoms: &Atoms,
        req: SelectionRequestEvent,
    ) {
        if req.owner != self.owner_window || req.selection != atoms.CLIPBOARD {
            send_selection_notify(conn, &req, x11rb::NONE);
            return;
        }

        let Some(ref content) = self.content.clone() else {
            send_selection_notify(conn, &req, x11rb::NONE);
            return;
        };

        // Per ICCCM §2.2, obsolete clients may pass `property == NONE`; fallback to `target`.
        let dest_prop = if req.property == x11rb::NONE {
            req.target
        } else {
            req.property
        };

        let ok = self.fulfill_target(conn, atoms, content, req.requestor, req.target, dest_prop);
        let reply_prop = if ok { dest_prop } else { x11rb::NONE };
        send_selection_notify(conn, &req, reply_prop);
    }

    /// Handles `PropertyNotify(DELETE)` on a requestor window during an active `INCR` transfer.
    pub fn on_property_notify(&mut self, conn: &RustConnection, ev: PropertyNotifyEvent) {
        if ev.state != Property::DELETE {
            return;
        }
        let key = (ev.window, ev.atom);
        let Some(mut transfer) = self.incr_transfers.remove(&key) else {
            return;
        };

        if transfer.completed_final_zero {
            let _ = conn.flush();
            return;
        }

        if transfer.offset >= transfer.data.len() {
            // Write the final zero-length chunk to signal EOF per ICCCM INCR protocol.
            let _ =
                conn.change_property8(PropMode::REPLACE, ev.window, ev.atom, transfer.target, &[]);
            let _ = conn.flush();
            return;
        }

        let end = (transfer.offset + INCR_CHUNK_THRESHOLD).min(transfer.data.len());
        let chunk = &transfer.data[transfer.offset..end];
        let _ = conn.change_property8(
            PropMode::REPLACE,
            ev.window,
            ev.atom,
            transfer.target,
            chunk,
        );
        let _ = conn.flush();

        transfer.offset = end;
        self.incr_transfers.insert(key, transfer);
    }

    fn fulfill_target(
        &mut self,
        conn: &RustConnection,
        atoms: &Atoms,
        content: &ClipboardContent,
        requestor: Window,
        target: Atom,
        property: Atom,
    ) -> bool {
        if property == x11rb::NONE {
            return false;
        }

        if target == atoms.TARGETS {
            let targets = supported_targets(atoms);
            return conn
                .change_property32(
                    PropMode::REPLACE,
                    requestor,
                    property,
                    AtomEnum::ATOM,
                    &targets,
                )
                .is_ok();
        }

        if target == atoms.TIMESTAMP {
            return conn
                .change_property32(
                    PropMode::REPLACE,
                    requestor,
                    property,
                    AtomEnum::INTEGER,
                    &[self.owned_since],
                )
                .is_ok();
        }

        if target == atoms.SAVE_TARGETS {
            return conn
                .change_property32(PropMode::REPLACE, requestor, property, AtomEnum::ATOM, &[])
                .is_ok();
        }

        if target == atoms.MULTIPLE {
            return self.fulfill_multiple(conn, atoms, content, requestor, property);
        }

        if let Some((prop_type, bytes)) = encode_target_payload(atoms, content, target) {
            return self
                .write_bytes_or_start_incr(conn, atoms, requestor, property, prop_type, bytes);
        }

        false
    }

    fn fulfill_multiple(
        &mut self,
        conn: &RustConnection,
        atoms: &Atoms,
        content: &ClipboardContent,
        requestor: Window,
        property: Atom,
    ) -> bool {
        let Ok(cookie) = conn.get_property(false, requestor, property, AtomEnum::ANY, 0, 4096)
        else {
            return false;
        };
        let Ok(reply) = cookie.reply() else {
            return false;
        };
        let Some(mut pairs): Option<Vec<Atom>> = reply.value32().map(Iterator::collect) else {
            return false;
        };
        if pairs.len() % 2 != 0 {
            return false;
        }

        let mut modified = false;
        for chunk in pairs.chunks_exact_mut(2) {
            let sub_target = chunk[0];
            let sub_prop = chunk[1];
            if sub_prop == x11rb::NONE
                || !self.fulfill_target(conn, atoms, content, requestor, sub_target, sub_prop)
            {
                chunk[1] = x11rb::NONE;
                modified = true;
            }
        }

        if modified {
            let _ = conn.change_property32(
                PropMode::REPLACE,
                requestor,
                property,
                AtomEnum::ATOM,
                &pairs,
            );
        }
        true
    }

    fn write_bytes_or_start_incr(
        &mut self,
        conn: &RustConnection,
        atoms: &Atoms,
        requestor: Window,
        property: Atom,
        prop_type: Atom,
        bytes: Vec<u8>,
    ) -> bool {
        if bytes.len() <= INCR_CHUNK_THRESHOLD {
            return conn
                .change_property8(PropMode::REPLACE, requestor, property, prop_type, &bytes)
                .is_ok();
        }

        // Start ICCCM INCR transfer for payloads > 256 KiB.
        let _ = conn.change_window_attributes(
            requestor,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        );
        let total_len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        if conn
            .change_property32(
                PropMode::REPLACE,
                requestor,
                property,
                atoms.INCR,
                &[total_len],
            )
            .is_err()
        {
            return false;
        }

        self.incr_transfers.insert(
            (requestor, property),
            IncrTransfer {
                target: prop_type,
                data: bytes,
                offset: 0,
                completed_final_zero: false,
            },
        );
        true
    }
}

/// Returns the list of target atoms advertised in `TARGETS`.
#[must_use]
pub fn supported_targets(atoms: &Atoms) -> Vec<Atom> {
    vec![
        atoms.TARGETS,
        atoms.TIMESTAMP,
        atoms.MULTIPLE,
        atoms.SAVE_TARGETS,
        atoms.GNOME_COPIED_FILES,
        atoms.URI_LIST,
        atoms.UTF8_STRING,
        atoms.PLAIN_UTF8,
        atoms.PLAIN_TEXT,
        Atom::from(AtomEnum::STRING),
        atoms.TEXT,
    ]
}

/// Formats `ClipboardContent` for `x-special/gnome-copied-files` (`"copy\n" + uris joined by "\n"`).
#[must_use]
pub fn format_gnome_copied_files(content: &ClipboardContent) -> Vec<u8> {
    let op_str = match content.operation {
        ClipOp::Copy => "copy",
    };
    let mut out = String::from(op_str);
    for uri in &content.uris {
        out.push('\n');
        out.push_str(uri);
    }
    out.into_bytes()
}

/// Formats `ClipboardContent` for `text/uri-list` (RFC 2483 CRLF-terminated lines).
#[must_use]
pub fn format_uri_list(content: &ClipboardContent) -> Vec<u8> {
    let mut out = String::new();
    for uri in &content.uris {
        out.push_str(uri);
        out.push_str("\r\n");
    }
    out.into_bytes()
}

/// Formats `ClipboardContent` for plain-text targets (`UTF8_STRING`, `text/plain;charset=utf-8`, `STRING`),
/// preferring local filesystem paths joined by `"\n"` and falling back to URIs if no local paths exist.
#[must_use]
pub fn format_plain_paths(content: &ClipboardContent) -> Vec<u8> {
    if !content.local_paths.is_empty() {
        let joined = content
            .local_paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        joined.into_bytes()
    } else {
        content.uris.join("\n").into_bytes()
    }
}

fn encode_target_payload(
    atoms: &Atoms,
    content: &ClipboardContent,
    target: Atom,
) -> Option<(Atom, Vec<u8>)> {
    if target == atoms.GNOME_COPIED_FILES {
        return Some((atoms.GNOME_COPIED_FILES, format_gnome_copied_files(content)));
    }
    if target == atoms.URI_LIST {
        return Some((atoms.URI_LIST, format_uri_list(content)));
    }
    if target == atoms.UTF8_STRING {
        return Some((atoms.UTF8_STRING, format_plain_paths(content)));
    }
    if target == atoms.PLAIN_UTF8 {
        return Some((atoms.PLAIN_UTF8, format_plain_paths(content)));
    }
    if target == atoms.PLAIN_TEXT {
        return Some((atoms.PLAIN_TEXT, format_plain_paths(content)));
    }
    let string_atom = Atom::from(AtomEnum::STRING);
    if target == string_atom || target == atoms.TEXT {
        return Some((string_atom, format_plain_paths(content)));
    }
    None
}

fn send_selection_notify(conn: &RustConnection, req: &SelectionRequestEvent, property: Atom) {
    let ev = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: req.time,
        requestor: req.requestor,
        selection: req.selection,
        target: req.target,
        property,
    };
    let _ = conn.send_event(false, req.requestor, EventMask::NO_EVENT, ev);
    let _ = conn.flush();
}
