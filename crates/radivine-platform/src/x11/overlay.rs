//! X11 32-bit ARGB override-redirect overlay window and 1×1 helper window (`PLATFORM_INTEGRATION.md` §3.4).

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, Colormap, ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateGCAux,
    CreateWindowAux, EventMask, Gcontext, PropMode, Screen, StackMode, VisualClass, Visualid,
    Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::x11::atoms::Atoms;
use crate::x11::shape::{clear_input_region_empty, reset_bounding_region};
use crate::{OverlayWindowType, PlatformError, RectPx};

/// Selected X11 visual specification (`PLATFORM_INTEGRATION.md` §3.4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisualSpec {
    /// Visual ID.
    pub visual_id: Visualid,
    /// Bit depth (`32` for ARGB, `24` for opaque fallback).
    pub depth: u8,
    /// Red channel bitmask.
    pub red_mask: u32,
    /// Green channel bitmask.
    pub green_mask: u32,
    /// Blue channel bitmask.
    pub blue_mask: u32,
    /// Whether this visual is a 32-bit TrueColor visual supporting per-pixel alpha.
    pub is_argb: bool,
}

/// Finds a 32-bit `TrueColor` visual on `screen`, falling back to `screen.root_visual` if unavailable.
#[must_use]
pub fn find_argb_visual(screen: &Screen) -> VisualSpec {
    for depth_info in &screen.allowed_depths {
        if depth_info.depth != 32 {
            continue;
        }
        for vt in &depth_info.visuals {
            if vt.class == VisualClass::TRUE_COLOR
                && vt.red_mask != 0
                && vt.green_mask != 0
                && vt.blue_mask != 0
            {
                return VisualSpec {
                    visual_id: vt.visual_id,
                    depth: 32,
                    red_mask: vt.red_mask,
                    green_mask: vt.green_mask,
                    blue_mask: vt.blue_mask,
                    is_argb: true,
                };
            }
        }
    }

    // Fallback to root visual
    for depth_info in &screen.allowed_depths {
        if depth_info.depth != screen.root_depth {
            continue;
        }
        for vt in &depth_info.visuals {
            if vt.visual_id == screen.root_visual {
                return VisualSpec {
                    visual_id: vt.visual_id,
                    depth: screen.root_depth,
                    red_mask: vt.red_mask,
                    green_mask: vt.green_mask,
                    blue_mask: vt.blue_mask,
                    is_argb: false,
                };
            }
        }
    }

    VisualSpec {
        visual_id: screen.root_visual,
        depth: screen.root_depth,
        red_mask: 0x00FF_0000,
        green_mask: 0x0000_FF00,
        blue_mask: 0x0000_00FF,
        is_argb: false,
    }
}

/// Persistent pre-created X11 overlay window and 1×1 helper window (`PLATFORM_INTEGRATION.md` §3.4.2).
pub struct OverlayWindow {
    /// Overlay X11 `Window` ID (created once at daemon startup, reused across sessions).
    pub window: Window,
    /// 1×1 `InputOnly` helper window for selection/clipboard ownership.
    pub helper_window: Window,
    /// Graphics context for `ShmPutImage` / `PutImage`.
    pub gc: Gcontext,
    /// Private colormap for the 32-bit ARGB visual.
    pub colormap: Colormap,
    /// Chosen visual metadata.
    pub visual: VisualSpec,
    /// Whether the overlay window is currently mapped.
    pub is_mapped: bool,
    /// Current configured geometry in root physical pixels.
    pub current_rect: RectPx,
}

impl OverlayWindow {
    /// Creates the unmapped persistent overlay window, GC, colormap, and 1×1 helper window.
    pub fn create(
        conn: &RustConnection,
        screen: &Screen,
        atoms: &Atoms,
        window_type: OverlayWindowType,
    ) -> Result<Self, PlatformError> {
        let visual = find_argb_visual(screen);

        let colormap = conn
            .generate_id()
            .map_err(|e| PlatformError::X11(e.to_string()))?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual.visual_id)
            .map_err(|e| PlatformError::X11(e.to_string()))?;

        let window = conn
            .generate_id()
            .map_err(|e| PlatformError::X11(e.to_string()))?;

        let initial_rect = RectPx::new(
            0,
            0,
            u32::from(screen.width_in_pixels).max(1),
            u32::from(screen.height_in_pixels).max(1),
        );

        let win_aux = CreateWindowAux::new()
            .background_pixel(0)
            .border_pixel(0)
            .override_redirect(1)
            .colormap(colormap)
            .event_mask(
                EventMask::EXPOSURE
                    | EventMask::STRUCTURE_NOTIFY
                    | EventMask::PROPERTY_CHANGE
                    | EventMask::POINTER_MOTION
                    | EventMask::BUTTON_PRESS
                    | EventMask::BUTTON_RELEASE
                    | EventMask::KEY_PRESS
                    | EventMask::KEY_RELEASE,
            );

        conn.create_window(
            visual.depth,
            window,
            screen.root,
            initial_rect.x as i16,
            initial_rect.y as i16,
            initial_rect.w as u16,
            initial_rect.h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            visual.visual_id,
            &win_aux,
        )
        .map_err(|e| PlatformError::X11(e.to_string()))?;

        let gc = conn
            .generate_id()
            .map_err(|e| PlatformError::X11(e.to_string()))?;
        conn.create_gc(gc, window, &CreateGCAux::new().graphics_exposures(0))
            .map_err(|e| PlatformError::X11(e.to_string()))?;

        // Set standard properties (`PLATFORM_INTEGRATION.md` §3.4.2)
        let xdnd_version: u32 = 5;
        conn.change_property32(
            PropMode::REPLACE,
            window,
            atoms.XdndAware,
            AtomEnum::ATOM,
            &[xdnd_version],
        )
        .map_err(|e| PlatformError::X11(e.to_string()))?;

        let wm_type_atom: Atom = match window_type {
            OverlayWindowType::Dnd => atoms._NET_WM_WINDOW_TYPE_DND,
            OverlayWindowType::PopupMenu => atoms._NET_WM_WINDOW_TYPE_POPUP_MENU,
            OverlayWindowType::Notification => atoms._NET_WM_WINDOW_TYPE_NOTIFICATION,
            OverlayWindowType::Utility => atoms._NET_WM_WINDOW_TYPE_UTILITY,
        };
        conn.change_property32(
            PropMode::REPLACE,
            window,
            atoms._NET_WM_WINDOW_TYPE,
            AtomEnum::ATOM,
            &[wm_type_atom],
        )
        .map_err(|e| PlatformError::X11(e.to_string()))?;

        // Never bypass compositor (value 2) so 32-bit ARGB alpha compositing stays active
        conn.change_property32(
            PropMode::REPLACE,
            window,
            atoms._NET_WM_BYPASS_COMPOSITOR,
            AtomEnum::CARDINAL,
            &[2u32],
        )
        .map_err(|e| PlatformError::X11(e.to_string()))?;

        conn.change_property8(
            PropMode::REPLACE,
            window,
            atoms.WM_CLASS,
            AtomEnum::STRING,
            b"radivine\0Radivine\0",
        )
        .map_err(|e| PlatformError::X11(e.to_string()))?;

        conn.change_property8(
            PropMode::REPLACE,
            window,
            atoms._NET_WM_NAME,
            atoms.UTF8_STRING,
            b"Radivine",
        )
        .map_err(|e| PlatformError::X11(e.to_string()))?;

        let pid = std::process::id();
        conn.change_property32(
            PropMode::REPLACE,
            window,
            atoms._NET_WM_PID,
            AtomEnum::CARDINAL,
            &[pid],
        )
        .map_err(|e| PlatformError::X11(e.to_string()))?;

        // Create 1x1 InputOnly helper window for selection/clipboard ownership
        let helper_window = conn
            .generate_id()
            .map_err(|e| PlatformError::X11(e.to_string()))?;
        let helper_aux = CreateWindowAux::new()
            .override_redirect(1)
            .event_mask(EventMask::PROPERTY_CHANGE);
        conn.create_window(
            0,
            helper_window,
            screen.root,
            -100,
            -100,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            x11rb::COPY_FROM_PARENT,
            &helper_aux,
        )
        .map_err(|e| PlatformError::X11(e.to_string()))?;

        Ok(Self {
            window,
            helper_window,
            gc,
            colormap,
            visual,
            is_mapped: false,
            current_rect: initial_rect,
        })
    }

    /// Reconfigures the unmapped overlay window to cover `rect`.
    pub fn prepare(&mut self, conn: &RustConnection, rect: RectPx) -> Result<(), PlatformError> {
        let aux = ConfigureWindowAux::new()
            .x(rect.x)
            .y(rect.y)
            .width(rect.w.max(1))
            .height(rect.h.max(1))
            .stack_mode(StackMode::ABOVE);
        conn.configure_window(self.window, &aux)
            .map_err(|e| PlatformError::X11(e.to_string()))?;
        self.current_rect = rect;
        Ok(())
    }

    /// Raises and maps the overlay window, flushing `shm` pending damage in the same X11 batch.
    pub fn map(
        &mut self,
        conn: &RustConnection,
        shm: &mut crate::x11::shm::ShmPresenter,
        pointer_nudge: bool,
    ) -> Result<(), PlatformError> {
        conn.configure_window(
            self.window,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )
        .map_err(|e| PlatformError::X11(e.to_string()))?;
        conn.map_window(self.window)
            .map_err(|e| PlatformError::X11(e.to_string()))?;
        shm.flush_pending(conn, self.window, self.gc)?;
        if pointer_nudge {
            let _ = conn.warp_pointer(x11rb::NONE, x11rb::NONE, 0, 0, 0, 0, 0, 0);
        }
        conn.flush()
            .map_err(|e| PlatformError::X11(e.to_string()))?;
        self.is_mapped = true;
        Ok(())
    }

    /// Clears the input shape region, resets any bounding shape, unmaps the overlay window, and flushes immediately.
    pub fn hide(&mut self, conn: &RustConnection, shape_supported: bool, pointer_nudge: bool) {
        if shape_supported {
            let _ = clear_input_region_empty(conn, self.window);
            let _ = reset_bounding_region(conn, self.window);
        }
        let _ = conn.unmap_window(self.window);
        if pointer_nudge {
            let _ = conn.warp_pointer(x11rb::NONE, x11rb::NONE, 0, 0, 0, 0, 0, 0);
        }
        let _ = conn.flush();
        self.is_mapped = false;
    }

    /// Destroys the X11 windows, GC, and colormap on shutdown.
    pub fn destroy(&mut self, conn: &RustConnection) {
        let _ = conn.destroy_window(self.window);
        let _ = conn.destroy_window(self.helper_window);
        let _ = conn.free_gc(self.gc);
        let _ = conn.free_colormap(self.colormap);
        let _ = conn.flush();
    }
}
