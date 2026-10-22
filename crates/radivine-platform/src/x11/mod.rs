//! X11 platform backend (`PLATFORM_INTEGRATION.md` §3).

pub mod arming;
pub mod atoms;
pub mod doctor;
pub mod invoke;
pub mod monitors;
pub mod overlay;
pub mod shape;
pub mod shm;
pub mod source;

use std::os::fd::{AsRawFd, RawFd};
use std::time::Instant;

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt as _, Cursor, EventMask};
use x11rb::rust_connection::RustConnection;

use crate::x11::atoms::Atoms;
use crate::x11::doctor::{DoctorReport, probe_with_connection};
use crate::x11::invoke::{KeymapTable, create_left_ptr_cursor, grab_input, ungrab_input};
use crate::x11::monitors::{
    pick_monitor_for_point, query_monitors, query_xft_scale, select_randr_notifications,
};
use crate::x11::overlay::OverlayWindow;
use crate::x11::shape::apply_input_region;
use crate::x11::shm::{ChannelShuffle, ShmPresenter};
use crate::{
    Capabilities, ClipboardContent, FrameRef, Monitor, OverlayInfo, PhysPoint, Platform,
    PlatformError, PlatformEvent, RectPx, ShapePx, Support, X11Config,
};

/// X11 implementation of [`Platform`] (`PLATFORM_INTEGRATION.md` §3, §7).
pub struct X11Platform {
    conn: RustConnection,
    screen_num: usize,
    atoms: Atoms,
    cfg: X11Config,
    doctor: DoctorReport,
    overlay: OverlayWindow,
    shm: ShmPresenter,
    keymap: KeymapTable,
    cursor: Cursor,
    shape_supported: bool,
    randr_supported: bool,
    grabbed: bool,
}

impl X11Platform {
    /// Connects to the X11 server, probes extensions, and creates the persistent unmapped overlay.
    pub fn connect(cfg: X11Config) -> Result<Self, PlatformError> {
        let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "x11".to_string());
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());

        let (conn, screen_num) =
            RustConnection::connect(None).map_err(|e| PlatformError::Connect(e.to_string()))?;
        let atoms = Atoms::new(&conn)
            .map_err(|e| PlatformError::X11(e.to_string()))?
            .reply()
            .map_err(|e| PlatformError::X11(e.to_string()))?;

        let doctor = probe_with_connection(&conn, screen_num, &atoms, session_type, display);
        let setup = conn.setup();
        let screen = &setup.roots[screen_num];
        let root = screen.root;

        // Observe root window PROPERTY_CHANGE so live Xft.dpi updates emit SettingsChanged
        let _ = conn.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        );

        let randr_supported = doctor
            .extensions
            .iter()
            .any(|e| e.name == "RANDR" && e.available);
        if randr_supported {
            select_randr_notifications(&conn, root);
        }

        let shape_supported = doctor
            .extensions
            .iter()
            .any(|e| e.name == "SHAPE" && e.available);
        let shm_supported = doctor
            .extensions
            .iter()
            .any(|e| e.name == "MIT-SHM" && e.available);

        let overlay = OverlayWindow::create(&conn, screen, &atoms, cfg.window_type)?;
        let shuffle = ChannelShuffle::from_visual(
            setup.image_byte_order,
            overlay.visual.red_mask,
            overlay.visual.green_mask,
            overlay.visual.blue_mask,
        );
        let shm = ShmPresenter::new(shm_supported, overlay.visual.depth, shuffle);
        let keymap = KeymapTable::query(&conn, setup);
        let cursor = create_left_ptr_cursor(&conn);

        conn.flush()
            .map_err(|e| PlatformError::X11(e.to_string()))?;

        Ok(Self {
            conn,
            screen_num,
            atoms,
            cfg,
            doctor,
            overlay,
            shm,
            keymap,
            cursor,
            shape_supported,
            randr_supported,
            grabbed: false,
        })
    }

    /// Returns the diagnostic [`DoctorReport`] gathered when this backend connected.
    #[must_use]
    pub fn doctor_report(&self) -> &DoctorReport {
        &self.doctor
    }

    /// Returns a reference to the underlying [`RustConnection`].
    #[must_use]
    pub fn connection(&self) -> &RustConnection {
        &self.conn
    }
}

impl Platform for X11Platform {
    fn name(&self) -> &'static str {
        "x11"
    }

    fn capabilities(&self) -> &Capabilities {
        &self.doctor.capabilities
    }

    #[cfg(unix)]
    fn poll_fd(&self) -> RawFd {
        self.conn.stream().as_raw_fd()
    }

    fn dispatch_pending(&mut self, out: &mut Vec<PlatformEvent>) -> Result<(), PlatformError> {
        let root = self.conn.setup().roots[self.screen_num].root;
        loop {
            match self.conn.poll_for_event() {
                Ok(Some(event)) => match event {
                    Event::MotionNotify(ev) => {
                        out.push(PlatformEvent::PointerMoved {
                            at: PhysPoint::new(i32::from(ev.root_x), i32::from(ev.root_y)),
                        });
                    }
                    Event::ButtonPress(ev) => {
                        out.push(PlatformEvent::Button {
                            button: ev.detail,
                            pressed: true,
                            at: PhysPoint::new(i32::from(ev.root_x), i32::from(ev.root_y)),
                        });
                    }
                    Event::ButtonRelease(ev) => {
                        out.push(PlatformEvent::Button {
                            button: ev.detail,
                            pressed: false,
                            at: PhysPoint::new(i32::from(ev.root_x), i32::from(ev.root_y)),
                        });
                    }
                    Event::KeyPress(ev) => {
                        if let Some(key) = self.keymap.translate(ev.detail, ev.state) {
                            out.push(PlatformEvent::Key { key, pressed: true });
                        }
                    }
                    Event::KeyRelease(ev) => {
                        if let Some(key) = self.keymap.translate(ev.detail, ev.state) {
                            out.push(PlatformEvent::Key {
                                key,
                                pressed: false,
                            });
                        }
                    }
                    Event::MappingNotify(_) => {
                        self.keymap = KeymapTable::query(&self.conn, self.conn.setup());
                    }
                    Event::PropertyNotify(ev)
                        if ev.window == root && ev.atom == self.atoms.RESOURCE_MANAGER =>
                    {
                        let (dpi, scale) = query_xft_scale(&self.conn, root, &self.atoms);
                        self.doctor.xft_dpi = dpi;
                        self.doctor.scale = scale;
                        out.push(PlatformEvent::SettingsChanged);
                    }
                    Event::RandrScreenChangeNotify(_) | Event::RandrNotify(_) => {
                        let screen = &self.conn.setup().roots[self.screen_num];
                        self.doctor.monitors =
                            query_monitors(&self.conn, screen, self.randr_supported);
                        out.push(PlatformEvent::MonitorsChanged);
                    }
                    Event::ShmCompletion(_) => {}
                    _ => {}
                },
                Ok(None) => break,
                Err(e) => {
                    out.push(PlatformEvent::ConnectionLost);
                    return Err(PlatformError::X11(e.to_string()));
                }
            }
        }
        Ok(())
    }

    fn monitors(&self) -> &[Monitor] {
        &self.doctor.monitors
    }

    fn scale(&self) -> f32 {
        self.doctor.scale
    }

    fn reduced_motion(&self) -> Option<bool> {
        None
    }

    fn pointer_position(&mut self) -> Result<PhysPoint, PlatformError> {
        let root = self.conn.setup().roots[self.screen_num].root;
        let reply = self
            .conn
            .query_pointer(root)
            .map_err(|e| PlatformError::X11(e.to_string()))?
            .reply()
            .map_err(|e| PlatformError::X11(e.to_string()))?;
        Ok(PhysPoint::new(
            i32::from(reply.root_x),
            i32::from(reply.root_y),
        ))
    }

    fn overlay_window_id(&self) -> u32 {
        self.overlay.window
    }

    fn prepare_overlay(&mut self, at: PhysPoint) -> Result<OverlayInfo, PlatformError> {
        let monitor = pick_monitor_for_point(&self.doctor.monitors, at);
        self.overlay.prepare(&self.conn, monitor.rect)?;
        self.shm
            .ensure_size(&self.conn, monitor.rect.w, monitor.rect.h)?;
        let translucent = matches!(self.doctor.capabilities.translucency, Support::Yes);

        Ok(OverlayInfo {
            origin: PhysPoint::new(monitor.rect.x, monitor.rect.y),
            size: (monitor.rect.w, monitor.rect.h),
            scale: self.doctor.scale,
            monitor,
            translucent,
        })
    }

    fn map_overlay(&mut self) -> Result<(), PlatformError> {
        self.overlay
            .map(&self.conn, &mut self.shm, self.cfg.pointer_nudge)
    }

    fn present(&mut self, frame: FrameRef<'_>, damage: &[RectPx]) -> Result<(), PlatformError> {
        self.shm.present(
            &self.conn,
            self.overlay.window,
            self.overlay.gc,
            frame,
            damage,
            self.overlay.is_mapped,
        )
    }

    fn set_input_region(&mut self, shapes: Option<&[ShapePx]>) -> Result<(), PlatformError> {
        if !self.shape_supported {
            return Ok(());
        }
        apply_input_region(
            &self.conn,
            self.overlay.window,
            self.overlay.current_rect.w as u16,
            self.overlay.current_rect.h as u16,
            shapes,
        )
    }

    fn hide_overlay(&mut self) {
        if self.grabbed {
            ungrab_input(&self.conn);
            self.grabbed = false;
        }
        self.overlay
            .hide(&self.conn, self.shape_supported, self.cfg.pointer_nudge);
    }

    fn release_buffers(&mut self) {
        self.shm.release(&self.conn);
        let _ = self.conn.flush();
    }

    fn dnd_reply(&mut self, _accept: bool) {
        // Implemented in M3 (XDND Drag Mode)
    }

    fn request_payload(&mut self, _mime: &str) {
        // Implemented in M3 (XDND Drag Mode)
    }

    fn dnd_finish(&mut self, _success: bool) {
        // Implemented in M3 (XDND Drag Mode)
    }

    fn grab_input(&mut self) -> Result<(), PlatformError> {
        grab_input(&self.conn, self.overlay.window, self.cursor)?;
        self.grabbed = true;
        Ok(())
    }

    fn ungrab_input(&mut self) {
        if self.grabbed {
            ungrab_input(&self.conn);
            self.grabbed = false;
        } else {
            ungrab_input(&self.conn);
        }
    }

    fn set_clipboard(&mut self, _content: ClipboardContent) -> Result<(), PlatformError> {
        // Implemented in M4 (File Actions & Clipboard Ownership)
        Ok(())
    }

    fn flush(&mut self) -> Result<(), PlatformError> {
        self.conn
            .flush()
            .map_err(|e| PlatformError::X11(e.to_string()))
    }

    fn next_deadline(&self) -> Option<Instant> {
        None
    }

    fn on_deadline(&mut self, _now: Instant, _out: &mut Vec<PlatformEvent>) {}
}

impl Drop for X11Platform {
    fn drop(&mut self) {
        if self.grabbed {
            ungrab_input(&self.conn);
            self.grabbed = false;
        }
        self.overlay.hide(&self.conn, self.shape_supported, false);
        self.shm.release(&self.conn);
        if self.cursor != x11rb::NONE {
            let _ = self.conn.free_cursor(self.cursor);
        }
        self.overlay.destroy(&self.conn);
    }
}
