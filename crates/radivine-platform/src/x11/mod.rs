//! X11 platform backend (`PLATFORM_INTEGRATION.md` §3).

pub mod arming;
pub mod atoms;
pub mod clipboard;
pub mod doctor;
pub mod invoke;
pub mod monitors;
pub mod overlay;
pub mod selection;
pub mod shape;
pub mod shm;
pub mod source;
pub mod xdnd;

use std::os::fd::{AsRawFd, RawFd};
use std::time::Instant;

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt as _, Cursor, EventMask};
use x11rb::rust_connection::RustConnection;

use crate::x11::arming::{
    ArmingTracker, ArmingTransition, TriggerEdge, query_modifier_keycodes, query_pointer_state,
    select_xdnd_selection_notify, select_xi2_raw_events,
};
use crate::x11::atoms::Atoms;
use crate::x11::clipboard::ClipboardService;
use crate::x11::doctor::{DoctorReport, probe_with_connection};
use crate::x11::invoke::{KeymapTable, create_left_ptr_cursor, grab_input, ungrab_input};
use crate::x11::monitors::{
    pick_monitor_for_point, query_monitors, query_xft_scale, select_randr_notifications,
};
use crate::x11::overlay::OverlayWindow;
use crate::x11::selection::{
    SelectionReader, handle_property_notify_wire, handle_selection_notify_wire,
    request_selection_wire,
};
use crate::x11::shape::{apply_bounding_region, apply_input_region};
use crate::x11::shm::{ChannelShuffle, ShmPresenter};
use crate::x11::source::identify_window_source;
use crate::x11::xdnd::{
    DndMessageEvent, XdndAtoms, XdndTarget, resolve_enter_types, send_client_message,
};
use crate::{
    Capabilities, ClipboardContent, FrameRef, Monitor, OverlayInfo, PayloadError, PhysPoint,
    Platform, PlatformError, PlatformEvent, RectPx, ShapePx, Support, TriggerDetectionMode,
    X11Config,
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
    arming: ArmingTracker,
    xdnd: XdndTarget,
    selection: SelectionReader,
    clipboard: ClipboardService,
    pending_payload_events: Vec<PlatformEvent>,
    shape_supported: bool,
    randr_supported: bool,
    xi2_supported: bool,
    res_supported: bool,
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
        let xfixes_supported = doctor
            .extensions
            .iter()
            .any(|e| e.name == "XFixes" && e.available);
        let xi2_supported = doctor
            .extensions
            .iter()
            .any(|e| e.name == "XInputExtension" && e.available);
        let res_supported = doctor
            .extensions
            .iter()
            .any(|e| e.name == "X-Resource" && e.available);

        let overlay = OverlayWindow::create(&conn, screen, &atoms, cfg.window_type)?;
        if xfixes_supported {
            let _ = select_xdnd_selection_notify(&conn, overlay.helper_window, atoms.XdndSelection);
        }

        let shuffle = ChannelShuffle::from_visual(
            setup.image_byte_order,
            overlay.visual.red_mask,
            overlay.visual.green_mask,
            overlay.visual.blue_mask,
        );
        let shm = ShmPresenter::new(shm_supported, overlay.visual.depth, shuffle);
        let keymap = KeymapTable::query(&conn, setup);
        let cursor = create_left_ptr_cursor(&conn);

        let trigger_kcs = query_modifier_keycodes(&conn, cfg.trigger_modifier);
        let arming = ArmingTracker::new(&trigger_kcs);
        let xdnd = XdndTarget::new(overlay.window, XdndAtoms::from_atoms(&atoms));
        let selection = SelectionReader::new(overlay.window, atoms.RADIVINE_DND, atoms.INCR);
        let clipboard = ClipboardService::new(overlay.helper_window);

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
            arming,
            xdnd,
            selection,
            clipboard,
            pending_payload_events: Vec::new(),
            shape_supported,
            randr_supported,
            xi2_supported,
            res_supported,
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

    fn is_poll_trigger_mode(&self) -> bool {
        self.cfg.trigger_detection == TriggerDetectionMode::Poll || !self.xi2_supported
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
        if !self.pending_payload_events.is_empty() {
            out.append(&mut self.pending_payload_events);
        }
        let root = self.conn.setup().roots[self.screen_num].root;
        loop {
            match self.conn.poll_for_event() {
                Ok(Some(event)) => match event {
                    Event::XfixesSelectionNotify(se)
                        if se.selection == self.atoms.XdndSelection =>
                    {
                        let (button_held, modifier_held) =
                            query_pointer_state(&self.conn, root, self.cfg.trigger_modifier)
                                .map_or((false, false), |(_, b, m)| (b, m));
                        let source = if se.owner != x11rb::NONE {
                            identify_window_source(
                                &self.conn,
                                &self.atoms,
                                se.owner,
                                self.res_supported,
                            )
                        } else {
                            crate::SourceInfo::default()
                        };
                        let poll_mode = self.is_poll_trigger_mode();
                        if let Some(transition) = self.arming.on_selection_owner_changed(
                            se.owner,
                            (self.overlay.window, self.overlay.helper_window),
                            button_held,
                            modifier_held,
                            se.selection_timestamp,
                            source,
                            poll_mode,
                            Instant::now(),
                        ) {
                            match transition {
                                ArmingTransition::Armed { source } => {
                                    if !poll_mode {
                                        let _ = select_xi2_raw_events(&self.conn, root, true);
                                    }
                                    out.push(PlatformEvent::DragArmed { source });
                                }
                                ArmingTransition::Disarmed => {
                                    if !poll_mode {
                                        let _ = select_xi2_raw_events(&self.conn, root, false);
                                    }
                                    self.xdnd.reset();
                                    self.selection.cancel();
                                    out.push(PlatformEvent::DragDisarmed);
                                }
                            }
                        }
                    }
                    Event::XinputRawKeyPress(rkp) => {
                        if let Ok(kc) = u8::try_from(rkp.detail)
                            && self.arming.on_raw_key_press(kc) == Some(TriggerEdge::FreshPress)
                            && let Ok(at) = self.pointer_position()
                        {
                            out.push(PlatformEvent::Trigger { at });
                        }
                    }
                    Event::XinputRawKeyRelease(rkr) => {
                        if let Ok(kc) = u8::try_from(rkr.detail)
                            && self.arming.on_raw_key_release(kc) == Some(TriggerEdge::Released)
                        {
                            out.push(PlatformEvent::TriggerReleased);
                        }
                    }
                    Event::XinputRawButtonRelease(rbr) => {
                        if let Ok(btn) = u8::try_from(rbr.detail) {
                            self.arming.on_raw_button_release(btn, Instant::now());
                        }
                    }
                    Event::ClientMessage(cm)
                        if cm.window == self.overlay.window && cm.format == 32 =>
                    {
                        if let Some(dnd_ev) = self.xdnd.on_message(cm.type_, cm.data.as_data32()) {
                            match dnd_ev {
                                DndMessageEvent::Enter {
                                    source,
                                    inline_types,
                                    more_types,
                                    ..
                                } => {
                                    let types = resolve_enter_types(
                                        &self.conn,
                                        &self.atoms,
                                        source,
                                        &inline_types,
                                        more_types,
                                    );
                                    out.push(PlatformEvent::DndEnter { types });
                                }
                                DndMessageEvent::Position { at, .. } => {
                                    out.push(PlatformEvent::DndPosition { at });
                                }
                                DndMessageEvent::Leave { .. } => {
                                    let button_held = query_pointer_state(
                                        &self.conn,
                                        root,
                                        self.cfg.trigger_modifier,
                                    )
                                    .is_ok_and(|(_, b, _)| b);
                                    let owner = self
                                        .conn
                                        .get_selection_owner(self.atoms.XdndSelection)
                                        .ok()
                                        .and_then(|c| c.reply().ok())
                                        .map_or(x11rb::NONE, |r| r.owner);
                                    let drag_ended = !button_held || owner == x11rb::NONE;
                                    out.push(PlatformEvent::DndLeave { drag_ended });
                                }
                                DndMessageEvent::Drop { .. } => {
                                    out.push(PlatformEvent::DndDrop);
                                }
                            }
                        }
                    }
                    Event::SelectionNotify(sn) if sn.requestor == self.overlay.window => {
                        if let Some((mime, result)) =
                            handle_selection_notify_wire(&self.conn, &mut self.selection, sn)
                        {
                            out.push(PlatformEvent::Payload { mime, result });
                        }
                    }
                    Event::PropertyNotify(pn)
                        if pn.window == self.overlay.window
                            && pn.atom == self.atoms.RADIVINE_DND =>
                    {
                        if let Some((mime, result)) =
                            handle_property_notify_wire(&self.conn, &mut self.selection, pn)
                        {
                            out.push(PlatformEvent::Payload { mime, result });
                        }
                    }
                    Event::Expose(ex)
                        if ex.window == self.overlay.window
                            && ex.count == 0
                            && self.overlay.is_mapped =>
                    {
                        let _ = self.shm.flush_pending(
                            &self.conn,
                            self.overlay.window,
                            self.overlay.gc,
                        );
                        let _ = self.conn.flush();
                    }
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
                        let trigger_kcs =
                            query_modifier_keycodes(&self.conn, self.cfg.trigger_modifier);
                        self.arming.set_trigger_keycodes(&trigger_kcs);
                    }
                    Event::SelectionRequest(req) if req.owner == self.overlay.helper_window => {
                        self.clipboard
                            .on_selection_request(&self.conn, &self.atoms, req);
                    }
                    Event::SelectionClear(ev) if ev.owner == self.overlay.helper_window => {
                        self.clipboard.on_selection_clear(ev, self.atoms.CLIPBOARD);
                    }
                    Event::PropertyNotify(ev)
                        if ev.window == root && ev.atom == self.atoms.RESOURCE_MANAGER =>
                    {
                        let (dpi, scale) = query_xft_scale(&self.conn, root, &self.atoms);
                        self.doctor.xft_dpi = dpi;
                        self.doctor.scale = scale;
                        out.push(PlatformEvent::SettingsChanged);
                    }
                    Event::PropertyNotify(ev) => {
                        self.clipboard.on_property_notify(&self.conn, ev);
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
        self.doctor.reduced_motion
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
        self.arming.set_session_open(true);
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
        )?;
        if matches!(shapes, Some([])) && self.arming.is_armed() && self.cfg.pointer_nudge {
            let _ = self
                .conn
                .warp_pointer(x11rb::NONE, x11rb::NONE, 0, 0, 0, 0, 0, 0);
            let _ = self.conn.flush();
        }
        Ok(())
    }

    fn set_bounding_region(&mut self, shapes: Option<&[ShapePx]>) -> Result<(), PlatformError> {
        if !self.shape_supported {
            return Ok(());
        }
        apply_bounding_region(
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
        self.arming.set_session_open(false);
        self.xdnd.reset();
        self.selection.cancel();
        let nudge = self.arming.is_armed() && self.cfg.pointer_nudge;
        self.overlay.hide(&self.conn, self.shape_supported, nudge);
    }

    fn release_buffers(&mut self) {
        self.shm.release(&self.conn);
        let _ = self.conn.flush();
    }

    fn dnd_reply(&mut self, accept: bool) {
        if let Some(msg) = self.xdnd.status(accept) {
            let _ = send_client_message(&self.conn, msg);
        }
    }

    fn request_payload(&mut self, mime: &str) {
        let ts = self
            .xdnd
            .effective_selection_timestamp(self.arming.selection_timestamp());
        if let Err(err) = request_selection_wire(
            &self.conn,
            &self.atoms,
            &mut self.selection,
            mime,
            ts,
            Instant::now(),
        ) {
            self.pending_payload_events.push(PlatformEvent::Payload {
                mime: mime.to_string(),
                result: Err(PayloadError::Protocol(err.to_string())),
            });
        }
    }

    fn dnd_finish(&mut self, success: bool) {
        if let Some(msg) = self.xdnd.finished(success) {
            let _ = send_client_message(&self.conn, msg);
        }
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

    fn set_clipboard(&mut self, content: ClipboardContent) -> Result<(), PlatformError> {
        self.clipboard
            .set_clipboard(&self.conn, &self.atoms, content)
    }

    fn flush(&mut self) -> Result<(), PlatformError> {
        self.conn
            .flush()
            .map_err(|e| PlatformError::X11(e.to_string()))
    }

    fn next_deadline(&self) -> Option<Instant> {
        match (self.arming.next_deadline(), self.selection.next_deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    fn on_deadline(&mut self, now: Instant, out: &mut Vec<PlatformEvent>) {
        if let Some((mime, result)) = self.selection.check_timeout(now) {
            out.push(PlatformEvent::Payload { mime, result });
        }

        let check = self.arming.check_deadlines(now);
        let root = self.conn.setup().roots[self.screen_num].root;
        let poll_mode = self.is_poll_trigger_mode();

        if check.max_duration_expired {
            if !poll_mode {
                let _ = select_xi2_raw_events(&self.conn, root, false);
            }
            self.xdnd.reset();
            self.selection.cancel();
            out.push(PlatformEvent::DragDisarmed);
            return;
        }

        if check.verify_button_release {
            let button_held = query_pointer_state(&self.conn, root, self.cfg.trigger_modifier)
                .is_ok_and(|(_, b, _)| b);
            if !button_held && self.arming.disarm() {
                if !poll_mode {
                    let _ = select_xi2_raw_events(&self.conn, root, false);
                }
                self.xdnd.reset();
                self.selection.cancel();
                out.push(PlatformEvent::DragDisarmed);
                return;
            }
        }

        if check.poll_due
            && let Ok((pt, button_held, modifier_held)) =
                query_pointer_state(&self.conn, root, self.cfg.trigger_modifier)
        {
            for edge in self.arming.on_poll_sample(button_held, modifier_held, now) {
                match edge {
                    TriggerEdge::FreshPress => out.push(PlatformEvent::Trigger { at: pt }),
                    TriggerEdge::Released => out.push(PlatformEvent::TriggerReleased),
                }
            }
        }
    }
}

impl Drop for X11Platform {
    fn drop(&mut self) {
        if self.arming.is_armed() && !self.is_poll_trigger_mode() {
            let root = self.conn.setup().roots[self.screen_num].root;
            let _ = select_xi2_raw_events(&self.conn, root, false);
        }
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
