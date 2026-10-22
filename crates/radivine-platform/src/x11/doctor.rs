//! `radivine doctor` diagnostic probe and formatter (`PLATFORM_INTEGRATION.md` §3.13, `AC-2.9`).

use std::fmt;
use std::fs;

use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::res::ConnectionExt as _;
use x11rb::protocol::shape::ConnectionExt as _;
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xinput::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

use crate::x11::atoms::Atoms;
use crate::x11::monitors::{is_compositor_active, query_monitors, query_xft_scale};
use crate::x11::overlay::{VisualSpec, find_argb_visual};
use crate::{Capabilities, Monitor, PlatformError, Support};

/// Probed status of an X11 extension (`PLATFORM_INTEGRATION.md` §3.2, §3.13).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionStatus {
    /// Extension name.
    pub name: &'static str,
    /// Whether the extension is present and meets the minimum version.
    pub available: bool,
    /// Probed major.minor version if available.
    pub version: Option<(u32, u32)>,
    /// Extra detail (e.g. `shared_pixmaps=true, fd_passing=true`).
    pub detail: String,
}

/// Complete diagnostic report produced by [`probe_x11_doctor`] (`PLATFORM_INTEGRATION.md` §3.13).
#[derive(Clone, Debug)]
pub struct DoctorReport {
    /// `XDG_SESSION_TYPE` environment variable.
    pub session_type: String,
    /// `DISPLAY` environment variable.
    pub display: String,
    /// X11 server vendor string.
    pub server_vendor: String,
    /// Root window XID.
    pub root_window: u32,
    /// Root window pixel dimensions `(width, height)`.
    pub root_size: (u16, u16),
    /// Whether a compositing manager owns `_NET_WM_CM_S0`.
    pub compositor_active: bool,
    /// `_NET_WM_CM_S0` owner window XID.
    pub compositor_owner: u32,
    /// Compositor process ID if known.
    pub compositor_pid: Option<u32>,
    /// Compositor process `comm` name if known.
    pub compositor_comm: Option<String>,
    /// Selected visual specification.
    pub visual: VisualSpec,
    /// Probed X11 extension statuses.
    pub extensions: Vec<ExtensionStatus>,
    /// Active RandR monitors.
    pub monitors: Vec<Monitor>,
    /// Raw `Xft.dpi` value if present on `RESOURCE_MANAGER`.
    pub xft_dpi: Option<f32>,
    /// Effective scale factor (`Xft.dpi / 96.0`).
    pub scale: f32,
    /// Computed platform [`Capabilities`].
    pub capabilities: Capabilities,
}

impl DoctorReport {
    /// Returns `true` if `invoke_at_pointer` is usable (`Support::Yes` or `Support::Degraded`).
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        !matches!(self.capabilities.invoke_at_pointer, Support::No(_))
    }
}

impl fmt::Display for DoctorReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Session type       : {}", self.session_type)?;
        writeln!(f, "DISPLAY            : {}", self.display)?;
        writeln!(f, "Server vendor      : {}", self.server_vendor)?;
        writeln!(
            f,
            "Root window        : 0x{:08x} ({} x {})",
            self.root_window, self.root_size.0, self.root_size.1
        )?;
        if self.compositor_active {
            let pid_str = self
                .compositor_pid
                .map_or_else(|| "unknown".to_string(), |p| p.to_string());
            let comm_str = self.compositor_comm.as_deref().unwrap_or("unknown");
            writeln!(
                f,
                "Compositor         : active (_NET_WM_CM_S0 = 0x{:08x}, pid {}, comm \"{}\")",
                self.compositor_owner, pid_str, comm_str
            )?;
        } else {
            writeln!(f, "Compositor         : inactive (_NET_WM_CM_S0 unowned)")?;
        }
        writeln!(
            f,
            "32-bit visual      : 0x{:02x} TrueColor, depth {}, R=0x{:08x} G=0x{:08x} B=0x{:08x}",
            self.visual.visual_id,
            self.visual.depth,
            self.visual.red_mask,
            self.visual.green_mask,
            self.visual.blue_mask
        )?;
        writeln!(f, "Extensions:")?;
        for ext in &self.extensions {
            let ver = ext
                .version
                .map_or_else(|| "-".to_string(), |(maj, min)| format!("{maj}.{min}"));
            let status = if ext.available { "ok" } else { "missing" };
            if ext.detail.is_empty() {
                writeln!(f, "  {:<17}: {:<6} ({})", ext.name, ver, status)?;
            } else {
                writeln!(
                    f,
                    "  {:<17}: {}, {} ({})",
                    ext.name, ver, ext.detail, status
                )?;
            }
        }
        writeln!(f, "Monitors:")?;
        for m in &self.monitors {
            let hz = f64::from(m.refresh_mhz) / 1000.0;
            let primary = if m.primary { " (primary)" } else { "" };
            writeln!(
                f,
                "  {:<17}: {}x{}+{}+{} @ {:.2} Hz{}",
                m.name, m.rect.w, m.rect.h, m.rect.x, m.rect.y, hz, primary
            )?;
        }
        match self.xft_dpi {
            Some(dpi) => writeln!(
                f,
                "DPI / scale        : Xft.dpi = {:.0} -> scale = {:.2}",
                dpi, self.scale
            )?,
            None => writeln!(
                f,
                "DPI / scale        : Xft.dpi unset -> scale = {:.2}",
                self.scale
            )?,
        }
        writeln!(f, "Capabilities:")?;
        writeln!(
            f,
            "  drag_session      = {}",
            self.capabilities.drag_session
        )?;
        writeln!(
            f,
            "  invoke_at_pointer = {}",
            self.capabilities.invoke_at_pointer
        )?;
        writeln!(
            f,
            "  translucency      = {}",
            self.capabilities.translucency
        )?;
        writeln!(
            f,
            "  input_passthrough = {}",
            self.capabilities.input_passthrough
        )?;
        writeln!(
            f,
            "  keyboard_in_drag  = {}",
            self.capabilities.keyboard_in_drag
        )?;
        writeln!(
            f,
            "  clipboard_files   = {}",
            self.capabilities.clipboard_files
        )?;
        Ok(())
    }
}

/// Connects to the X11 server and runs the full `radivine doctor` diagnostic probe.
pub fn probe_x11_doctor() -> Result<DoctorReport, PlatformError> {
    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| "(unset)".to_string());

    let (conn, screen_num) =
        RustConnection::connect(None).map_err(|e| PlatformError::Connect(e.to_string()))?;
    let atoms = Atoms::new(&conn)
        .map_err(|e| PlatformError::X11(e.to_string()))?
        .reply()
        .map_err(|e| PlatformError::X11(e.to_string()))?;

    Ok(probe_with_connection(
        &conn,
        screen_num,
        &atoms,
        session_type,
        display,
    ))
}

/// Probes [`DoctorReport`] using an existing [`RustConnection`].
#[must_use]
pub fn probe_with_connection(
    conn: &RustConnection,
    screen_num: usize,
    atoms: &Atoms,
    session_type: String,
    display: String,
) -> DoctorReport {
    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    let server_vendor = String::from_utf8_lossy(&setup.vendor).into_owned();
    let (compositor_active, compositor_owner) = is_compositor_active(conn, atoms);
    let (compositor_pid, compositor_comm) = if compositor_active {
        query_window_pid_and_comm(conn, atoms, compositor_owner)
    } else {
        (None, None)
    };
    let visual = find_argb_visual(screen);

    // Probe extensions
    let xfixes = probe_xfixes(conn);
    let xinput = probe_xinput(conn);
    let shape = probe_shape(conn);
    let shm = probe_shm(conn);
    let randr = probe_randr(conn);
    let xres = probe_xres(conn);

    let monitors = query_monitors(conn, screen, randr.available);
    let (xft_dpi, scale) = query_xft_scale(conn, screen.root, atoms);

    let drag_session = if session_type.eq_ignore_ascii_case("wayland") {
        Support::No("Wayland session does not expose global DND interception")
    } else if xfixes.available && xinput.available {
        Support::Yes
    } else if xfixes.available {
        Support::Degraded("XI2 unavailable; falling back to 8 ms QueryPointer polling")
    } else {
        Support::No("XFixes 5.0+ required for XdndSelection observation")
    };

    let invoke_at_pointer = if session_type.eq_ignore_ascii_case("wayland") {
        Support::Degraded("XWayland pointer coordinates may be restricted outside X11 windows")
    } else {
        Support::Yes
    };

    let translucency = if visual.is_argb && compositor_active {
        Support::Yes
    } else if !compositor_active {
        Support::Degraded("no compositor owns _NET_WM_CM_S0; using opaque card fallback")
    } else {
        Support::Degraded("no 32-bit TrueColor visual; using 24-bit root visual")
    };

    let input_passthrough = if shape.available {
        Support::Yes
    } else {
        Support::Degraded("XShape 1.1 unavailable; full overlay captures input")
    };

    let capabilities = Capabilities {
        drag_session,
        invoke_at_pointer,
        translucency,
        input_passthrough,
        keyboard_in_drag: Support::Degraded("best-effort during foreign active pointer grab"),
        clipboard_files: Support::Yes,
    };

    let extensions = vec![xfixes, xinput, shape, shm, randr, xres];

    DoctorReport {
        session_type,
        display,
        server_vendor,
        root_window: screen.root,
        root_size: (screen.width_in_pixels, screen.height_in_pixels),
        compositor_active,
        compositor_owner,
        compositor_pid,
        compositor_comm,
        visual,
        extensions,
        monitors,
        xft_dpi,
        scale,
        capabilities,
    }
}

fn query_window_pid_and_comm(
    conn: &RustConnection,
    atoms: &Atoms,
    win: Window,
) -> (Option<u32>, Option<String>) {
    let mut pid = conn
        .get_property(false, win, atoms._NET_WM_PID, AtomEnum::CARDINAL, 0, 1)
        .ok()
        .and_then(|c| c.reply().ok())
        .and_then(|r| r.value32()?.next());

    if pid.is_none()
        && let Ok(cookie) = conn.res_query_client_ids(&[x11rb::protocol::res::ClientIdSpec {
            client: win,
            mask: x11rb::protocol::res::ClientIdMask::LOCAL_CLIENT_PID,
        }])
        && let Ok(reply) = cookie.reply()
    {
        for id_val in reply.ids {
            if let Some(&p) = id_val.value.first()
                && p != 0
            {
                pid = Some(p);
                break;
            }
        }
    }

    let comm = pid.and_then(|p| {
        fs::read_to_string(format!("/proc/{p}/comm"))
            .ok()
            .map(|s| s.trim().to_string())
    });
    (pid, comm)
}

fn probe_xfixes(conn: &RustConnection) -> ExtensionStatus {
    if let Ok(c) = conn.xfixes_query_version(5, 0)
        && let Ok(r) = c.reply()
    {
        let ok = r.major_version >= 5;
        return ExtensionStatus {
            name: "XFixes",
            available: ok,
            version: Some((r.major_version, r.minor_version)),
            detail: String::new(),
        };
    }
    ExtensionStatus {
        name: "XFixes",
        available: false,
        version: None,
        detail: String::new(),
    }
}

fn probe_xinput(conn: &RustConnection) -> ExtensionStatus {
    if let Ok(c) = conn.xinput_xi_query_version(2, 2)
        && let Ok(r) = c.reply()
    {
        let ok = r.major_version >= 2;
        return ExtensionStatus {
            name: "XInputExtension",
            available: ok,
            version: Some((u32::from(r.major_version), u32::from(r.minor_version))),
            detail: String::new(),
        };
    }
    ExtensionStatus {
        name: "XInputExtension",
        available: false,
        version: None,
        detail: String::new(),
    }
}

fn probe_shape(conn: &RustConnection) -> ExtensionStatus {
    if let Ok(c) = conn.shape_query_version()
        && let Ok(r) = c.reply()
    {
        let ok = (r.major_version, r.minor_version) >= (1, 1);
        return ExtensionStatus {
            name: "SHAPE",
            available: ok,
            version: Some((u32::from(r.major_version), u32::from(r.minor_version))),
            detail: String::new(),
        };
    }
    ExtensionStatus {
        name: "SHAPE",
        available: false,
        version: None,
        detail: String::new(),
    }
}

fn probe_shm(conn: &RustConnection) -> ExtensionStatus {
    if let Ok(c) = conn.shm_query_version()
        && let Ok(r) = c.reply()
    {
        let ver = (u32::from(r.major_version), u32::from(r.minor_version));
        let fd_passing = ver >= (1, 2);
        return ExtensionStatus {
            name: "MIT-SHM",
            available: fd_passing,
            version: Some(ver),
            detail: format!(
                "shared_pixmaps={}, fd_passing={}",
                r.shared_pixmaps, fd_passing
            ),
        };
    }
    ExtensionStatus {
        name: "MIT-SHM",
        available: false,
        version: None,
        detail: String::new(),
    }
}

fn probe_randr(conn: &RustConnection) -> ExtensionStatus {
    if let Ok(c) = conn.randr_query_version(1, 5)
        && let Ok(r) = c.reply()
    {
        let ok = (r.major_version, r.minor_version) >= (1, 5);
        return ExtensionStatus {
            name: "RANDR",
            available: ok,
            version: Some((r.major_version, r.minor_version)),
            detail: String::new(),
        };
    }
    ExtensionStatus {
        name: "RANDR",
        available: false,
        version: None,
        detail: String::new(),
    }
}

fn probe_xres(conn: &RustConnection) -> ExtensionStatus {
    if let Ok(c) = conn.res_query_version(1, 2)
        && let Ok(r) = c.reply()
    {
        let ok = (r.server_major, r.server_minor) >= (1, 2);
        return ExtensionStatus {
            name: "X-Resource",
            available: ok,
            version: Some((u32::from(r.server_major), u32::from(r.server_minor))),
            detail: String::new(),
        };
    }
    ExtensionStatus {
        name: "X-Resource",
        available: false,
        version: None,
        detail: String::new(),
    }
}
