//! X11 drag source application identification via `X-Resource` (`LOCAL_CLIENT_PID`) and `_NET_WM_PID`
//! (`PLATFORM_INTEGRATION.md` §3.1, §3.6).

use std::fs;

use x11rb::protocol::res::{ClientIdMask, ClientIdSpec, ConnectionExt as _};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

use crate::SourceInfo;
use crate::x11::atoms::Atoms;

/// Identifies the process ID and executable name (`comm`) owning `window`.
///
/// Queries the X-Resource 1.2 extension (`res_query_client_ids` with `LOCAL_CLIENT_PID`) first
/// when `res_supported` is `true`, falling back to the window's `_NET_WM_PID` property.
/// If a local PID is resolved, reads `/proc/<pid>/comm` to populate [`SourceInfo::app`].
#[must_use]
pub fn identify_window_source(
    conn: &RustConnection,
    atoms: &Atoms,
    window: Window,
    res_supported: bool,
) -> SourceInfo {
    if window == x11rb::NONE {
        return SourceInfo::default();
    }

    let pid = query_pid_via_xres(conn, window, res_supported)
        .or_else(|| query_pid_via_ewmh(conn, atoms, window));

    let app = pid.and_then(read_proc_comm);
    SourceInfo { app, pid }
}

fn query_pid_via_xres(conn: &RustConnection, window: Window, res_supported: bool) -> Option<u32> {
    if !res_supported {
        return None;
    }
    let spec = ClientIdSpec {
        client: window,
        mask: ClientIdMask::LOCAL_CLIENT_PID,
    };
    let reply = conn.res_query_client_ids(&[spec]).ok()?.reply().ok()?;
    for id in reply.ids {
        if let Some(&pid) = id.value.first()
            && pid > 0
        {
            return Some(pid);
        }
    }
    None
}

fn query_pid_via_ewmh(conn: &RustConnection, atoms: &Atoms, window: Window) -> Option<u32> {
    let reply = conn
        .get_property(false, window, atoms._NET_WM_PID, AtomEnum::CARDINAL, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    reply.value32()?.next().filter(|&pid| pid > 0)
}

/// Reads `/proc/<pid>/comm` and normalizes the process name.
#[must_use]
pub fn read_proc_comm(pid: u32) -> Option<String> {
    let path = format!("/proc/{pid}/comm");
    let raw = fs::read_to_string(path).ok()?;
    parse_proc_comm(&raw)
}

/// Parses the contents of `/proc/<pid>/comm` into a clean lowercase process name.
#[must_use]
pub fn parse_proc_comm(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
