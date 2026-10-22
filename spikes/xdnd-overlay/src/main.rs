use std::collections::HashSet;
use std::env;
use std::fs;
use std::time::{Duration, Instant};

use rustix::fs::{MemfdFlags, ftruncate, memfd_create};
use rustix::mm::{MapFlags, ProtFlags, mmap, munmap};
use x11rb::atom_manager;
use x11rb::connection::Connection;
use x11rb::errors::ReplyOrIdError;
use x11rb::protocol::Event;
use x11rb::protocol::composite::ConnectionExt as _;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::res::{ClientIdMask, ClientIdSpec, ConnectionExt as _};
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::shm::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xinput::{self, ConnectionExt as _, XIEventMask};
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageData, ClientMessageEvent, ClipOrdering, ColormapAlloc,
    ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, Gcontext,
    ImageFormat, KeyButMask, Keycode, Pixmap, PropMode, Rectangle, Screen, StackMode, VisualClass,
    Visualid, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

atom_manager! {
    pub Atoms: AtomsCookie {
        XdndAware,
        XdndEnter,
        XdndPosition,
        XdndStatus,
        XdndLeave,
        XdndDrop,
        XdndFinished,
        XdndSelection,
        XdndTypeList,
        XdndActionCopy,
        XdndActionMove,
        XdndActionLink,
        XdndActionAsk,
        XdndActionPrivate,
        TARGETS,
        MULTIPLE,
        INCR,
        DELETE,
        UTF8_STRING,
        TEXT,
        URI_LIST: b"text/uri-list",
        GNOME_ICON_LIST: b"x-special/gnome-icon-list",
        GNOME_COPIED_FILES: b"x-special/gnome-copied-files",
        RADIVINE_DND: b"_RADIVINE_DND",
        WM_STATE,
        WM_CLASS,
        _NET_WM_NAME,
        _NET_WM_PID,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DND,
        _NET_WM_WINDOW_TYPE_POPUP_MENU,
        _NET_WM_WINDOW_TYPE_NOTIFICATION,
        _NET_WM_WINDOW_TYPE_UTILITY,
        _NET_WM_BYPASS_COMPOSITOR,
        _NET_SUPPORTING_WM_CHECK,
        _NET_CLIENT_LIST,
        _NET_ACTIVE_WINDOW,
        _NET_WM_CM_S0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TriggerMode {
    Raw,
    Poll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrefetchMode {
    XFixesTs,
    CurrentTime,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShapeMode {
    Disc,
    ExpandChild,
    CaptureAll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NudgeMode {
    None,
    Zero,
    OnePx,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StationaryMode {
    None,
    RejectToAccept,
    AcceptToReject,
    SubmenuDwell,
}

#[derive(Clone, Debug)]
struct ListenConfig {
    trigger_mode: TriggerMode,
    prefetch_mode: PrefetchMode,
    shape_mode: ShapeMode,
    nudge_mode: NudgeMode,
    stationary_mode: StationaryMode,
    stationary_delay_ms: u64,
    stationary_nudge: bool,
    precreate_overlay: bool,
    overlay_size: (u16, u16),
    max_sessions: usize,
    timeout_s: u64,
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self {
            trigger_mode: TriggerMode::Raw,
            prefetch_mode: PrefetchMode::XFixesTs,
            shape_mode: ShapeMode::Disc,
            nudge_mode: NudgeMode::None,
            stationary_mode: StationaryMode::None,
            stationary_delay_ms: 150,
            stationary_nudge: false,
            precreate_overlay: true,
            overlay_size: (400, 400),
            max_sessions: 1,
            timeout_s: 30,
        }
    }
}

struct ArgbVisual {
    visual_id: Visualid,
    depth: u8,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
}

fn find_argb_visual(screen: &Screen) -> Option<ArgbVisual> {
    for depth in &screen.allowed_depths {
        if depth.depth == 32 {
            for vis in &depth.visuals {
                if vis.class == VisualClass::TRUE_COLOR
                    && vis.red_mask == 0x00FF_0000
                    && vis.green_mask == 0x0000_FF00
                    && vis.blue_mask == 0x0000_00FF
                {
                    return Some(ArgbVisual {
                        visual_id: vis.visual_id,
                        depth: 32,
                        red_mask: vis.red_mask,
                        green_mask: vis.green_mask,
                        blue_mask: vis.blue_mask,
                    });
                }
            }
        }
    }
    None
}

fn lookup_client_info(conn: &RustConnection, window: Window) -> (Option<u32>, Option<String>) {
    let spec = ClientIdSpec {
        client: window,
        mask: ClientIdMask::LOCAL_CLIENT_PID,
    };
    if let Ok(cookie) = conn.res_query_client_ids(&[spec])
        && let Ok(reply) = cookie.reply()
    {
        for id in reply.ids {
            if let Some(&pid) = id.value.first()
                && pid > 0
            {
                let comm_path = format!("/proc/{pid}/comm");
                let comm = fs::read_to_string(comm_path)
                    .ok()
                    .map(|s| s.trim().to_string());
                return (Some(pid), comm);
            }
        }
    }
    (None, None)
}

fn get_shift_keycodes(conn: &RustConnection) -> HashSet<Keycode> {
    let mut set = HashSet::new();
    if let Ok(cookie) = conn.get_modifier_mapping()
        && let Ok(reply) = cookie.reply()
    {
        let kpm = reply.keycodes_per_modifier() as usize;
        // Row 0 is Shift
        for &kc in reply.keycodes.iter().take(kpm) {
            if kc != 0 {
                set.insert(kc);
            }
        }
    }
    set
}

fn parse_uri_list(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .map(|l| l.trim_end_matches('\r').trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect()
}

fn select_xi2_raw(conn: &RustConnection, root: Window, enable: bool) -> Result<(), ReplyOrIdError> {
    let mask = if enable {
        XIEventMask::RAW_KEY_PRESS | XIEventMask::RAW_KEY_RELEASE | XIEventMask::RAW_BUTTON_RELEASE
    } else {
        XIEventMask::default()
    };
    let event_mask = xinput::EventMask {
        deviceid: u16::from(xinput::Device::ALL_MASTER),
        mask: vec![mask],
    };
    conn.xinput_xi_select_events(root, &[event_mask])?.check()?;
    Ok(())
}

fn set_disc_input_shape(
    conn: &RustConnection,
    win: Window,
    w: u16,
    h: u16,
    discs: &[(i32, i32, i32)],
    corridor: Option<(i32, i32, i32, i32)>,
) -> Result<(), ReplyOrIdError> {
    let pixmap: Pixmap = conn.generate_id()?;
    conn.create_pixmap(1, pixmap, win, w, h)?.check()?;
    let gc_clear: Gcontext = conn.generate_id()?;
    let gc_set: Gcontext = conn.generate_id()?;
    conn.create_gc(gc_clear, pixmap, &CreateGCAux::new().foreground(0))?;
    conn.create_gc(gc_set, pixmap, &CreateGCAux::new().foreground(1))?;

    conn.poly_fill_rectangle(
        pixmap,
        gc_clear,
        &[Rectangle {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }],
    )?;

    // Build horizontal scanline spans for discs and optional corridor rect
    let mut rects = Vec::new();
    for y in 0..(h as i32) {
        for &(cx, cy, r) in discs {
            let dy = y - cy;
            if dy.abs() <= r {
                let dx = ((r * r - dy * dy) as f64).sqrt().round() as i32;
                let x0 = (cx - dx).max(0);
                let x1 = (cx + dx).min(w as i32 - 1);
                if x1 >= x0 {
                    rects.push(Rectangle {
                        x: x0 as i16,
                        y: y as i16,
                        width: (x1 - x0 + 1) as u16,
                        height: 1,
                    });
                }
            }
        }
    }
    if let Some((x0, y0, x1, y1)) = corridor {
        let rx = x0.max(0) as i16;
        let ry = y0.max(0) as i16;
        let rw = (x1 - x0).max(1) as u16;
        let rh = (y1 - y0).max(1) as u16;
        rects.push(Rectangle {
            x: rx,
            y: ry,
            width: rw,
            height: rh,
        });
    }
    // Send in chunks of 1000 rectangles
    for chunk in rects.chunks(1000) {
        conn.poly_fill_rectangle(pixmap, gc_set, chunk)?;
    }

    conn.shape_mask(SO::SET, SK::INPUT, win, 0, 0, pixmap)?;
    conn.free_gc(gc_clear)?;
    conn.free_gc(gc_set)?;
    conn.free_pixmap(pixmap)?;
    conn.flush()?;
    Ok(())
}

fn clear_input_shape_to_empty(conn: &RustConnection, win: Window) -> Result<(), ReplyOrIdError> {
    conn.shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, win, 0, 0, &[])?;
    conn.flush()?;
    Ok(())
}

fn reset_input_shape_to_full(conn: &RustConnection, win: Window) -> Result<(), ReplyOrIdError> {
    conn.shape_mask(SO::SET, SK::INPUT, win, 0, 0, x11rb::NONE)?;
    conn.flush()?;
    Ok(())
}

fn draw_wheel_visual(
    buf: &mut [u8],
    w: usize,
    h: usize,
    discs: &[(i32, i32, i32, i32)], // (cx, cy, r_in, r_out)
) {
    buf.fill(0);
    for y in 0..(h as i32) {
        for x in 0..(w as i32) {
            let mut px = [0u8; 4]; // B, G, R, A premultiplied
            for &(cx, cy, r_in, r_out) in discs {
                let dx = x - cx;
                let dy = y - cy;
                let d2 = dx * dx + dy * dy;
                if d2 <= r_in * r_in {
                    // Hub: #044C3C @ 92% alpha (235) -> R=4, G=76, B=60
                    let a = 235u16;
                    let r = (4 * a / 255) as u8;
                    let g = (76 * a / 255) as u8;
                    let b = (60 * a / 255) as u8;
                    px = [b, g, r, a as u8];
                } else if d2 <= r_out * r_out {
                    // Ring: #00A87C @ 88% alpha (224) -> R=0, G=168, B=124
                    let a = 224u16;
                    let r = 0u8;
                    let g = (168 * a / 255) as u8;
                    let b = (124 * a / 255) as u8;
                    px = [b, g, r, a as u8];
                }
            }
            if px[3] != 0 {
                let idx = (y as usize * w + x as usize) * 4;
                buf[idx..idx + 4].copy_from_slice(&px);
            }
        }
    }
}

fn run_probe() -> Result<(), Box<dyn std::error::Error>> {
    let t_start = Instant::now();
    let (conn, screen_num) = x11rb::connect(None)?;
    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    let atoms = Atoms::new(&conn)?.reply()?;

    println!("=== RADIVINE M0 S-5 & ENVIRONMENT PROBE ===");
    println!(
        "X11 Server Vendor: {} (release {})",
        String::from_utf8_lossy(&setup.vendor),
        setup.release_number
    );
    println!(
        "Screen {}: {}x{} px ({}x{} mm), root={:#x}, image_byte_order={:?}",
        screen_num,
        screen.width_in_pixels,
        screen.height_in_pixels,
        screen.width_in_millimeters,
        screen.height_in_millimeters,
        screen.root,
        setup.image_byte_order
    );

    // Check extensions
    let xfixes_ver = conn.xfixes_query_version(5, 0)?.reply()?;
    let xi_ver = conn.xinput_xi_query_version(2, 2)?.reply()?;
    let shape_ver = conn.shape_query_version()?.reply()?;
    let shm_ver = conn.shm_query_version()?.reply()?;
    let randr_ver = conn.randr_query_version(1, 5)?.reply()?;
    let res_ver = conn.res_query_version(1, 2)?.reply()?;
    let comp_ver = conn.composite_query_version(0, 4)?.reply()?;

    println!(
        "Extensions: XFixes={}.{}, XI2={}.{}, SHAPE={}.{}, MIT-SHM={}.{} (shared_pixmaps={}), RANDR={}.{}, X-Res={}.{}, Composite={}.{}",
        xfixes_ver.major_version,
        xfixes_ver.minor_version,
        xi_ver.major_version,
        xi_ver.minor_version,
        shape_ver.major_version,
        shape_ver.minor_version,
        shm_ver.major_version,
        shm_ver.minor_version,
        shm_ver.shared_pixmaps,
        randr_ver.major_version,
        randr_ver.minor_version,
        res_ver.server_major,
        res_ver.server_minor,
        comp_ver.major_version,
        comp_ver.minor_version,
    );

    // Check compositor via _NET_WM_CM_S0 selection owner
    let cm_owner = conn
        .get_selection_owner(atoms._NET_WM_CM_S0)?
        .reply()?
        .owner;
    let (cm_pid, cm_comm) = if cm_owner != x11rb::NONE {
        lookup_client_info(&conn, cm_owner)
    } else {
        (None, None)
    };
    println!(
        "Compositor (_NET_WM_CM_S0 owner): window={:#x}, pid={:?}, comm={:?}",
        cm_owner, cm_pid, cm_comm
    );

    // Check WM via _NET_SUPPORTING_WM_CHECK
    let wm_check = conn
        .get_property(
            false,
            screen.root,
            atoms._NET_SUPPORTING_WM_CHECK,
            AtomEnum::WINDOW,
            0,
            1,
        )?
        .reply()?;
    if let Some(wm_win) = wm_check.value32().and_then(|mut it| it.next()) {
        let wm_name = conn
            .get_property(false, wm_win, atoms._NET_WM_NAME, atoms.UTF8_STRING, 0, 64)?
            .reply()?;
        let (wm_pid, wm_comm) = lookup_client_info(&conn, wm_win);
        println!(
            "Window Manager (_NET_SUPPORTING_WM_CHECK): window={:#x}, name={:?}, pid={:?}, comm={:?}",
            wm_win,
            String::from_utf8_lossy(&wm_name.value),
            wm_pid,
            wm_comm
        );
    }

    // Check RandR monitors
    let monitors = conn.randr_get_monitors(screen.root, true)?.reply()?;
    for m in &monitors.monitors {
        let name = conn.get_atom_name(m.name)?.reply()?;
        println!(
            "Monitor {:?}: primary={}, {}x{}+{}+{} px",
            String::from_utf8_lossy(&name.name),
            m.primary,
            m.width,
            m.height,
            m.x,
            m.y
        );
    }

    // Find 32-bit ARGB visual
    let argb = find_argb_visual(screen).expect("32-bit ARGB TrueColor visual required");
    println!(
        "32-bit ARGB Visual: id={:#x}, depth={}, R={:#010x}, G={:#010x}, B={:#010x}",
        argb.visual_id, argb.depth, argb.red_mask, argb.green_mask, argb.blue_mask
    );

    // Test MIT-SHM memfd + AttachFd + PutImage & byte-order verification
    let w = screen.width_in_pixels as usize;
    let h = screen.height_in_pixels as usize;
    let byte_len = w * h * 4;

    let fd = memfd_create(c"radivine-m0-shm", MemfdFlags::CLOEXEC)?;
    ftruncate(&fd, byte_len as u64)?;
    let ptr = unsafe {
        mmap(
            std::ptr::null_mut(),
            byte_len,
            ProtFlags::READ | ProtFlags::WRITE,
            MapFlags::SHARED,
            &fd,
            0,
        )?
    };
    let shm_slice = unsafe { std::slice::from_raw_parts_mut(ptr.cast::<u8>(), byte_len) };

    let shm_seg: shm::Seg = conn.generate_id()?;
    let dup_fd = rustix::io::dup(&fd)?;
    conn.shm_attach_fd(shm_seg, dup_fd, true)?.check()?;

    let colormap = conn.generate_id()?;
    conn.create_colormap(ColormapAlloc::NONE, colormap, screen.root, argb.visual_id)?
        .check()?;

    let win = conn.generate_id()?;
    conn.create_window(
        32,
        win,
        screen.root,
        0,
        0,
        w as u16,
        h as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        argb.visual_id,
        &CreateWindowAux::new()
            .background_pixel(0)
            .border_pixel(0)
            .override_redirect(1)
            .colormap(colormap)
            .event_mask(
                EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE,
            ),
    )?
    .check()?;

    let gc: Gcontext = conn.generate_id()?;
    conn.create_gc(gc, win, &CreateGCAux::new())?.check()?;

    // Empty input shape during probe so we never intercept user clicks
    clear_input_shape_to_empty(&conn, win)?;

    let bypass_val: u32 = 2;
    conn.change_property32(
        PropMode::REPLACE,
        win,
        atoms._NET_WM_BYPASS_COMPOSITOR,
        AtomEnum::CARDINAL,
        &[bypass_val],
    )?;

    // Test all 4 window types
    let window_types = [
        ("DND", atoms._NET_WM_WINDOW_TYPE_DND),
        ("POPUP_MENU", atoms._NET_WM_WINDOW_TYPE_POPUP_MENU),
        ("NOTIFICATION", atoms._NET_WM_WINDOW_TYPE_NOTIFICATION),
        ("UTILITY", atoms._NET_WM_WINDOW_TYPE_UTILITY),
    ];

    for (wt_name, wt_atom) in window_types {
        let focus_before = conn.get_input_focus()?.reply()?.focus;
        let active_before = conn
            .get_property(
                false,
                screen.root,
                atoms._NET_ACTIVE_WINDOW,
                AtomEnum::WINDOW,
                0,
                1,
            )?
            .reply()?
            .value32()
            .and_then(|mut it| it.next())
            .unwrap_or(0);

        conn.change_property32(
            PropMode::REPLACE,
            win,
            atoms._NET_WM_WINDOW_TYPE,
            AtomEnum::ATOM,
            &[wt_atom],
        )?;
        let t_map = Instant::now();
        conn.configure_window(win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
        conn.map_window(win)?;
        conn.flush()?;

        // Wait for MapNotify and Expose
        let mut map_notify_us = None;
        let mut expose_us = None;
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline && (map_notify_us.is_none() || expose_us.is_none()) {
            if let Some(ev) = conn.poll_for_event()? {
                match ev {
                    Event::MapNotify(e) if e.window == win => {
                        map_notify_us = Some(t_map.elapsed().as_micros());
                    }
                    Event::Expose(e) if e.window == win => {
                        expose_us = Some(t_map.elapsed().as_micros());
                    }
                    _ => {}
                }
            } else {
                std::thread::sleep(Duration::from_micros(50));
            }
        }

        let focus_during = conn.get_input_focus()?.reply()?.focus;
        let active_during = conn
            .get_property(
                false,
                screen.root,
                atoms._NET_ACTIVE_WINDOW,
                AtomEnum::WINDOW,
                0,
                1,
            )?
            .reply()?
            .value32()
            .and_then(|mut it| it.next())
            .unwrap_or(0);
        let client_list = conn
            .get_property(
                false,
                screen.root,
                atoms._NET_CLIENT_LIST,
                AtomEnum::WINDOW,
                0,
                1024,
            )?
            .reply()?;
        let in_client_list = client_list
            .value32()
            .map(|mut it| it.any(|w_id| w_id == win))
            .unwrap_or(false);

        let t_unmap = Instant::now();
        conn.unmap_window(win)?;
        conn.flush()?;
        let mut unmap_notify_us = None;
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline && unmap_notify_us.is_none() {
            if let Some(ev) = conn.poll_for_event()? {
                if let Event::UnmapNotify(e) = ev
                    && e.window == win
                {
                    unmap_notify_us = Some(t_unmap.elapsed().as_micros());
                }
            } else {
                std::thread::sleep(Duration::from_micros(50));
            }
        }

        println!(
            "WindowType {:12}: map_notify={:?}µs, expose={:?}µs, unmap_notify={:?}µs, focus_changed={}, active_changed={}, in_client_list={}",
            wt_name,
            map_notify_us,
            expose_us,
            unmap_notify_us,
            focus_before != focus_during,
            active_before != active_during,
            in_client_list
        );
    }

    // Map once with _NET_WM_WINDOW_TYPE_DND to test pixel byte-order and benchmark MIT-SHM PutImage
    conn.change_property32(
        PropMode::REPLACE,
        win,
        atoms._NET_WM_WINDOW_TYPE,
        AtomEnum::ATOM,
        &[atoms._NET_WM_WINDOW_TYPE_DND],
    )?;
    conn.map_window(win)?;
    conn.flush()?;
    // Wait for Expose
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if let Some(Event::Expose(e)) = conn.poll_for_event()?
            && e.window == win
        {
            break;
        }
        std::thread::sleep(Duration::from_micros(50));
    }

    // Pixel byte-order verification: write known premultiplied RGBA -> BGRA pixels at (0,0)..(3,0)
    // Pixel 0: R=4, G=76, B=60, A=235 -> in LSBFirst BGRA bytes: [60, 76, 4, 235] = 0xEB044C3C
    // Pixel 1: R=0, G=168, B=124, A=255 -> in LSBFirst BGRA bytes: [124, 168, 0, 255] = 0xFF00A87C
    // Pixel 2: R=0, G=84, B=62, A=128 -> in LSBFirst BGRA bytes: [62, 84, 0, 128] = 0x8000543E
    // Pixel 3: transparent [0, 0, 0, 0]
    shm_slice[0..16].copy_from_slice(&[
        60, 76, 4, 235, // px 0
        124, 168, 0, 255, // px 1
        62, 84, 0, 128, // px 2
        0, 0, 0, 0, // px 3
    ]);
    conn.shm_put_image(
        win,
        gc,
        w as u16,
        h as u16,
        0,
        0,
        4,
        1,
        0,
        0,
        32,
        ImageFormat::Z_PIXMAP.into(),
        true,
        shm_seg,
        0,
    )?;
    conn.flush()?;
    // Wait for ShmCompletion
    loop {
        if let Some(Event::ShmCompletion(_)) = conn.poll_for_event()? {
            break;
        }
    }
    let img_reply = conn
        .get_image(ImageFormat::Z_PIXMAP, win, 0, 0, 4, 1, !0)?
        .reply()?;
    println!(
        "Byte-order GetImage readback (expected [60,76,4,235, 124,168,0,255, 62,84,0,128, 0,0,0,0]): {:?}",
        &img_reply.data[0..16]
    );
    assert_eq!(
        &img_reply.data[0..16],
        &[60, 76, 4, 235, 124, 168, 0, 255, 62, 84, 0, 128, 0, 0, 0, 0],
        "Premultiplied BGRA byte order mismatch!"
    );

    // Clear buffer back to transparent before benchmarking
    shm_slice.fill(0);

    // Benchmark RGBA->BGRA swizzle + shm::PutImage + ShmCompletion for 3 sizes:
    let src_rgba = vec![0u8; byte_len];
    let bench_rects = [
        ("full_3840x1080", w as u16, h as u16),
        ("cluster_800x600", 800u16, 600u16),
        ("wheel_300x300", 300u16, 300u16),
    ];

    for (label, bw, bh) in bench_rects {
        let mut swizzle_times = Vec::with_capacity(50);
        let mut put_times = Vec::with_capacity(50);
        let mut total_times = Vec::with_capacity(50);

        for _ in 0..50 {
            let t0 = Instant::now();
            // Swizzle damaged rows (RGBA -> BGRA)
            for row in 0..(bh as usize) {
                let row_off = row * w * 4;
                let row_len = (bw as usize) * 4;
                let src_row = &src_rgba[row_off..row_off + row_len];
                let dst_row = &mut shm_slice[row_off..row_off + row_len];
                for (s, d) in src_row.chunks_exact(4).zip(dst_row.chunks_exact_mut(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = s[3];
                }
            }
            let t_swiz = t0.elapsed().as_micros();

            let t1 = Instant::now();
            conn.shm_put_image(
                win,
                gc,
                w as u16,
                h as u16,
                0,
                0,
                bw,
                bh,
                0,
                0,
                32,
                ImageFormat::Z_PIXMAP.into(),
                true,
                shm_seg,
                0,
            )?;
            conn.flush()?;
            loop {
                if let Event::ShmCompletion(_) = conn.wait_for_event()? {
                    break;
                }
            }
            let t_put = t1.elapsed().as_micros();
            swizzle_times.push(t_swiz);
            put_times.push(t_put);
            total_times.push(t0.elapsed().as_micros());
        }
        swizzle_times.sort_unstable();
        put_times.sort_unstable();
        total_times.sort_unstable();
        println!(
            "SHM Bench {:16} ({}x{}): swizzle p50={}µs p95={}µs | put+completion p50={}µs p95={}µs | total min={}µs p50={}µs p95={}µs max={}µs",
            label,
            bw,
            bh,
            swizzle_times[25],
            swizzle_times[47],
            put_times[25],
            put_times[47],
            total_times[0],
            total_times[25],
            total_times[47],
            total_times[49]
        );
    }

    conn.unmap_window(win)?;
    conn.destroy_window(win)?;
    conn.shm_detach(shm_seg)?;
    conn.flush()?;
    unsafe {
        munmap(ptr, byte_len)?;
    }
    println!("=== PROBE COMPLETE in {:?} ===", t_start.elapsed());
    Ok(())
}

fn run_listen(cfg: ListenConfig) -> Result<(), Box<dyn std::error::Error>> {
    let t_epoch = Instant::now();
    let (conn, screen_num) = x11rb::connect(None)?;
    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    let root = screen.root;
    let atoms = Atoms::new(&conn)?.reply()?;

    // Initialize extensions
    conn.xfixes_query_version(5, 0)?.reply()?;
    conn.xinput_xi_query_version(2, 2)?.reply()?;
    conn.shape_query_version()?.reply()?;
    conn.shm_query_version()?.reply()?;
    conn.res_query_version(1, 2)?.reply()?;

    let argb = find_argb_visual(screen).expect("32-bit ARGB visual required");
    let colormap = conn.generate_id()?;
    conn.create_colormap(ColormapAlloc::NONE, colormap, root, argb.visual_id)?
        .check()?;

    // Hidden 1x1 window for XFixes selection notifications
    let hidden_win = conn.generate_id()?;
    conn.create_window(
        0,
        hidden_win,
        root,
        -100,
        -100,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new().override_redirect(1),
    )?
    .check()?;

    conn.xfixes_select_selection_input(
        hidden_win,
        atoms.XdndSelection,
        SelectionEventMask::SET_SELECTION_OWNER
            | SelectionEventMask::SELECTION_WINDOW_DESTROY
            | SelectionEventMask::SELECTION_CLIENT_CLOSE,
    )?
    .check()?;

    let (ow, oh) = cfg.overlay_size;
    let byte_len = (ow as usize) * (oh as usize) * 4;
    let fd = memfd_create(c"radivine-spike-shm", MemfdFlags::CLOEXEC)?;
    ftruncate(&fd, byte_len as u64)?;
    let ptr = unsafe {
        mmap(
            std::ptr::null_mut(),
            byte_len,
            ProtFlags::READ | ProtFlags::WRITE,
            MapFlags::SHARED,
            &fd,
            0,
        )?
    };
    let shm_slice = unsafe { std::slice::from_raw_parts_mut(ptr.cast::<u8>(), byte_len) };
    let shm_seg: shm::Seg = conn.generate_id()?;
    let dup_fd = rustix::io::dup(&fd)?;
    conn.shm_attach_fd(shm_seg, dup_fd, true)?.check()?;

    let create_overlay_window =
        |conn: &RustConnection| -> Result<(Window, Gcontext), Box<dyn std::error::Error>> {
            let win = conn.generate_id()?;
            conn.create_window(
                32,
                win,
                root,
                0,
                0,
                ow,
                oh,
                0,
                WindowClass::INPUT_OUTPUT,
                argb.visual_id,
                &CreateWindowAux::new()
                    .background_pixel(0)
                    .border_pixel(0)
                    .override_redirect(1)
                    .colormap(colormap)
                    .event_mask(
                        EventMask::EXPOSURE
                            | EventMask::STRUCTURE_NOTIFY
                            | EventMask::PROPERTY_CHANGE,
                    ),
            )?
            .check()?;
            let xdnd_version: u32 = 5;
            conn.change_property32(
                PropMode::REPLACE,
                win,
                atoms.XdndAware,
                AtomEnum::ATOM,
                &[xdnd_version],
            )?;
            conn.change_property32(
                PropMode::REPLACE,
                win,
                atoms._NET_WM_WINDOW_TYPE,
                AtomEnum::ATOM,
                &[atoms._NET_WM_WINDOW_TYPE_DND],
            )?;
            conn.change_property32(
                PropMode::REPLACE,
                win,
                atoms._NET_WM_BYPASS_COMPOSITOR,
                AtomEnum::CARDINAL,
                &[2u32],
            )?;
            let gc: Gcontext = conn.generate_id()?;
            conn.create_gc(gc, win, &CreateGCAux::new())?.check()?;
            Ok((win, gc))
        };

    let mut overlay = if cfg.precreate_overlay {
        Some(create_overlay_window(&conn)?)
    } else {
        None
    };

    let mut shift_keycodes = get_shift_keycodes(&conn);
    println!(
        "[{:>8}µs] READY: cfg={:?}, shift_keycodes={:?}, precreated_overlay={:?}",
        t_epoch.elapsed().as_micros(),
        cfg,
        shift_keycodes,
        overlay.map(|(w, _)| w)
    );

    // State variables
    let mut armed = false;
    let mut selection_ts: u32 = 0;
    let mut held_at_arm = false;
    let mut down_shift_keys: HashSet<Keycode> = HashSet::new();
    let mut poll_shift_was_down = false;

    let mut session_active = false;
    let mut overlay_origin = (0i32, 0i32);
    let mut summon_center = (200i32, 200i32); // overlay-local
    let mut child_expanded = false;
    let mut t_trigger: Option<Instant> = None;
    let mut t_mapped: Option<Instant> = None;
    let mut t_prefetch_sent: Option<Instant> = None;
    let mut t_enter: Option<Instant> = None;
    let mut t_drop: Option<Instant> = None;
    let mut position_count: usize = 0;
    let mut current_source: Window = x11rb::NONE;
    let mut last_accept: bool = false;
    let mut last_pos_local: (i32, i32) = (0, 0);
    let mut stationary_deadline: Option<Instant> = None;
    let mut stationary_fired = false;
    let mut child_expand_deadline: Option<Instant> = None;
    let mut incr_active = false;
    let mut incr_buf: Vec<u8> = Vec::new();
    let mut waiting_drop_payload = false;
    let mut completed_sessions = 0usize;

    let global_deadline = Instant::now() + Duration::from_secs(cfg.timeout_s);

    while Instant::now() < global_deadline && completed_sessions < cfg.max_sessions {
        while let Some(ev) = conn.poll_for_event()? {
            let now_us = t_epoch.elapsed().as_micros();
            match ev {
                Event::MappingNotify(_) => {
                    shift_keycodes = get_shift_keycodes(&conn);
                }
                Event::XfixesSelectionNotify(se) if se.selection == atoms.XdndSelection => {
                    if se.owner != x11rb::NONE {
                        let qp = conn.query_pointer(root)?.reply()?;
                        let button_held = qp.mask.intersects(
                            KeyButMask::BUTTON1 | KeyButMask::BUTTON2 | KeyButMask::BUTTON3,
                        );
                        let shift_held = qp.mask.contains(KeyButMask::SHIFT);
                        let (pid, comm) = lookup_client_info(&conn, se.owner);
                        if button_held {
                            armed = true;
                            selection_ts = se.selection_timestamp;
                            held_at_arm = shift_held;
                            down_shift_keys.clear();
                            poll_shift_was_down = shift_held;
                            if cfg.trigger_mode == TriggerMode::Raw {
                                select_xi2_raw(&conn, root, true)?;
                            }
                            println!(
                                "[{:>8}µs] ARMED: owner={:#x} (pid={:?}, app={:?}), sel_ts={}, held_at_arm={}, ptr=({},{})",
                                now_us,
                                se.owner,
                                pid,
                                comm,
                                selection_ts,
                                held_at_arm,
                                qp.root_x,
                                qp.root_y
                            );
                        } else {
                            println!(
                                "[{:>8}µs] IGNORED_SELECTION: owner={:#x} but no button held (mask={:?})",
                                now_us, se.owner, qp.mask
                            );
                        }
                    } else {
                        if armed && cfg.trigger_mode == TriggerMode::Raw {
                            let _ = select_xi2_raw(&conn, root, false);
                        }
                        println!(
                            "[{:>8}µs] DISARMED: XdndSelection owner became None (session_active={})",
                            now_us, session_active
                        );
                        armed = false;
                        if session_active && !waiting_drop_payload {
                            if let Some((win, _)) = overlay {
                                clear_input_shape_to_empty(&conn, win)?;
                                conn.unmap_window(win)?;
                                conn.flush()?;
                            }
                            session_active = false;
                            completed_sessions += 1;
                        }
                    }
                }
                Event::XinputRawKeyPress(rkp) if armed && cfg.trigger_mode == TriggerMode::Raw => {
                    let kc = rkp.detail as u8;
                    if shift_keycodes.contains(&kc) {
                        let is_new = down_shift_keys.insert(kc);
                        if !is_new {
                            // auto-repeat ignored
                        } else if held_at_arm {
                            println!("[{:>8}µs] SHIFT_IGNORED_PREHELD: kc={}", now_us, kc);
                        } else if down_shift_keys.len() == 1 {
                            // Fresh edge trigger!
                            if !session_active {
                                trigger_overlay(
                                    &conn,
                                    &atoms,
                                    &cfg,
                                    root,
                                    &mut overlay,
                                    &create_overlay_window,
                                    shm_slice,
                                    shm_seg,
                                    selection_ts,
                                    t_epoch,
                                    &mut session_active,
                                    &mut overlay_origin,
                                    &mut summon_center,
                                    &mut child_expanded,
                                    &mut t_trigger,
                                    &mut t_mapped,
                                    &mut t_prefetch_sent,
                                    &mut t_enter,
                                    &mut t_drop,
                                    &mut position_count,
                                    &mut stationary_fired,
                                    &mut child_expand_deadline,
                                )?;
                            } else {
                                // Second press toggles off!
                                println!(
                                    "[{:>8}µs] TRIGGER_TOGGLE_OFF: hiding overlay, drag continues",
                                    now_us
                                );
                                if let Some((win, _)) = overlay {
                                    clear_input_shape_to_empty(&conn, win)?;
                                    conn.unmap_window(win)?;
                                    conn.flush()?;
                                }
                                session_active = false;
                                completed_sessions += 1;
                            }
                        }
                    }
                }
                Event::XinputRawKeyRelease(rkr)
                    if armed && cfg.trigger_mode == TriggerMode::Raw =>
                {
                    let kc = rkr.detail as u8;
                    if shift_keycodes.contains(&kc) {
                        down_shift_keys.remove(&kc);
                        if down_shift_keys.is_empty() && held_at_arm {
                            held_at_arm = false;
                            println!(
                                "[{:>8}µs] PREHELD_SHIFT_RELEASED: subsequent Shift press may now trigger",
                                now_us
                            );
                        }
                    }
                }
                Event::Expose(ex) => {
                    if let Some((win, gc)) = overlay
                        && ex.window == win
                        && session_active
                    {
                        conn.shm_put_image(
                            win,
                            gc,
                            ow,
                            oh,
                            0,
                            0,
                            ow,
                            oh,
                            0,
                            0,
                            32,
                            ImageFormat::Z_PIXMAP.into(),
                            false,
                            shm_seg,
                            0,
                        )?;
                        conn.flush()?;
                    }
                }
                Event::ClientMessage(cm) => {
                    if let Some((win, _)) = overlay
                        && cm.window == win
                    {
                        let d = cm.data.as_data32();
                        if cm.type_ == atoms.XdndEnter {
                            current_source = d[0];
                            let more_types = (d[1] & 1) != 0;
                            let version = (d[1] >> 24) & 0xff;
                            let mut types = Vec::new();
                            if more_types {
                                let prop = conn
                                    .get_property(
                                        false,
                                        current_source,
                                        atoms.XdndTypeList,
                                        AtomEnum::ATOM,
                                        0,
                                        64,
                                    )?
                                    .reply()?;
                                if let Some(it) = prop.value32() {
                                    types.extend(it);
                                }
                            } else {
                                for &a in &d[2..5] {
                                    if a != 0 {
                                        types.push(a);
                                    }
                                }
                            }
                            let mut type_names = Vec::new();
                            for &t in &types {
                                if let Ok(c) = conn.get_atom_name(t)
                                    && let Ok(r) = c.reply()
                                {
                                    type_names.push(String::from_utf8_lossy(&r.name).to_string());
                                }
                            }
                            let enter_lat_us = t_mapped.map(|t| t.elapsed().as_micros());
                            t_enter = Some(Instant::now());
                            println!(
                                "[{:>8}µs] XDND_ENTER: source={:#x}, version={}, map_to_enter_us={:?}, types={:?}",
                                now_us, current_source, version, enter_lat_us, type_names
                            );
                        } else if cm.type_ == atoms.XdndPosition {
                            let src = d[0];
                            let x_root = ((d[2] >> 16) & 0xffff) as i16 as i32;
                            let y_root = (d[2] & 0xffff) as i16 as i32;
                            let ts = d[3];
                            let req_action = d[4];
                            position_count += 1;

                            let lx = x_root - overlay_origin.0;
                            let ly = y_root - overlay_origin.1;
                            last_pos_local = (lx, ly);

                            // Compute hit test
                            let dx0 = lx - summon_center.0;
                            let dy0 = ly - summon_center.1;
                            let dist0_sq = dx0 * dx0 + dy0 * dy0;

                            let mut accept = match cfg.stationary_mode {
                                StationaryMode::RejectToAccept => stationary_fired,
                                StationaryMode::AcceptToReject => !stationary_fired,
                                _ => {
                                    // Accept inside main ring (38 <= r <= 120) or child ring if expanded
                                    let in_main_ring = (38 * 38..=120 * 120).contains(&dist0_sq);
                                    let in_child_ring = if child_expanded {
                                        let cx1 = summon_center.0 + 180;
                                        let cy1 = summon_center.1;
                                        let dx1 = lx - cx1;
                                        let dy1 = ly - cy1;
                                        let d1_sq = dx1 * dx1 + dy1 * dy1;
                                        (24 * 24..=85 * 85).contains(&d1_sq)
                                    } else {
                                        false
                                    };
                                    in_main_ring || in_child_ring
                                }
                            };
                            if !session_active {
                                accept = false;
                            }
                            last_accept = accept;

                            send_xdnd_status(&conn, &atoms, win, src, accept)?;

                            if cfg.stationary_mode != StationaryMode::None && !stationary_fired {
                                stationary_deadline = Some(
                                    Instant::now() + Duration::from_millis(cfg.stationary_delay_ms),
                                );
                            }

                            println!(
                                "[{:>8}µs] XDND_POSITION #{}: root=({},{}), local=({},{}), ts={}, req_action={}, reply_accept={}",
                                now_us,
                                position_count,
                                x_root,
                                y_root,
                                lx,
                                ly,
                                ts,
                                req_action,
                                accept
                            );
                        } else if cm.type_ == atoms.XdndLeave {
                            let src = d[0];
                            let qp = conn.query_pointer(root)?.reply()?;
                            let button_held = qp.mask.intersects(
                                KeyButMask::BUTTON1 | KeyButMask::BUTTON2 | KeyButMask::BUTTON3,
                            );
                            let owner = conn
                                .get_selection_owner(atoms.XdndSelection)?
                                .reply()?
                                .owner;
                            let drag_ended = !button_held || owner == x11rb::NONE;
                            println!(
                                "[{:>8}µs] XDND_LEAVE: source={:#x}, button_held={}, owner={:#x}, drag_ended={}",
                                now_us, src, button_held, owner, drag_ended
                            );
                            if drag_ended {
                                clear_input_shape_to_empty(&conn, win)?;
                                conn.unmap_window(win)?;
                                conn.flush()?;
                                session_active = false;
                                completed_sessions += 1;
                            }
                        } else if cm.type_ == atoms.XdndDrop {
                            let src = d[0];
                            let ts = d[2];
                            t_drop = Some(Instant::now());
                            println!(
                                "[{:>8}µs] XDND_DROP: source={:#x}, ts={}, last_accept={}, pos_local={:?}",
                                now_us, src, ts, last_accept, last_pos_local
                            );
                            if !last_accept {
                                println!(
                                    "[{:>8}µs] DROP_TIME_RECHECK_REJECT: finishing drop with success=0",
                                    now_us
                                );
                                send_xdnd_finished(&conn, &atoms, win, src, false)?;
                                clear_input_shape_to_empty(&conn, win)?;
                                conn.unmap_window(win)?;
                                conn.flush()?;
                                session_active = false;
                                completed_sessions += 1;
                            } else {
                                waiting_drop_payload = true;
                                conn.convert_selection(
                                    win,
                                    atoms.XdndSelection,
                                    atoms.URI_LIST,
                                    atoms.RADIVINE_DND,
                                    ts,
                                )?;
                                conn.flush()?;
                            }
                        }
                    }
                }
                Event::SelectionNotify(sn) => {
                    if let Some((win, _)) = overlay
                        && sn.requestor == win
                    {
                        if sn.property == x11rb::NONE {
                            println!(
                                "[{:>8}µs] SELECTION_REFUSED: target={}, time={}",
                                now_us, sn.target, sn.time
                            );
                            if waiting_drop_payload {
                                waiting_drop_payload = false;
                                send_xdnd_finished(&conn, &atoms, win, current_source, false)?;
                                clear_input_shape_to_empty(&conn, win)?;
                                conn.unmap_window(win)?;
                                conn.flush()?;
                                session_active = false;
                                completed_sessions += 1;
                            }
                        } else {
                            let prop = conn
                                .get_property(
                                    true,
                                    win,
                                    atoms.RADIVINE_DND,
                                    AtomEnum::ANY,
                                    0,
                                    u32::MAX / 4,
                                )?
                                .reply()?;
                            if prop.type_ == atoms.INCR {
                                incr_active = true;
                                incr_buf.clear();
                                let lower_bound =
                                    prop.value32().and_then(|mut it| it.next()).unwrap_or(0);
                                println!(
                                    "[{:>8}µs] SELECTION_INCR_START: lower_bound={} bytes",
                                    now_us, lower_bound
                                );
                            } else {
                                let uris = parse_uri_list(&prop.value);
                                if waiting_drop_payload {
                                    let drop_lat = t_drop.map(|t| t.elapsed().as_micros());
                                    println!(
                                        "[{:>8}µs] DROP_PAYLOAD_OK: lat={:?}µs, bytes={}, uris_count={}, first_uris={:?}",
                                        now_us,
                                        drop_lat,
                                        prop.value.len(),
                                        uris.len(),
                                        &uris[..uris.len().min(5)]
                                    );
                                    waiting_drop_payload = false;
                                    send_xdnd_finished(&conn, &atoms, win, current_source, true)?;
                                    clear_input_shape_to_empty(&conn, win)?;
                                    conn.unmap_window(win)?;
                                    conn.flush()?;
                                    session_active = false;
                                    completed_sessions += 1;
                                } else {
                                    let pref_lat = t_prefetch_sent.map(|t| t.elapsed().as_micros());
                                    println!(
                                        "[{:>8}µs] PREFETCH_OK: lat={:?}µs, bytes={}, uris_count={}, first_uris={:?}",
                                        now_us,
                                        pref_lat,
                                        prop.value.len(),
                                        uris.len(),
                                        &uris[..uris.len().min(5)]
                                    );
                                }
                            }
                        }
                    }
                }
                Event::PropertyNotify(pn) => {
                    if let Some((win, _)) = overlay
                        && pn.window == win
                        && pn.atom == atoms.RADIVINE_DND
                        && pn.state == x11rb::protocol::xproto::Property::NEW_VALUE
                        && incr_active
                    {
                        let prop = conn
                            .get_property(
                                true,
                                win,
                                atoms.RADIVINE_DND,
                                AtomEnum::ANY,
                                0,
                                u32::MAX / 4,
                            )?
                            .reply()?;
                        if prop.value.is_empty() {
                            incr_active = false;
                            let uris = parse_uri_list(&incr_buf);
                            if waiting_drop_payload {
                                let drop_lat = t_drop.map(|t| t.elapsed().as_micros());
                                println!(
                                    "[{:>8}µs] DROP_PAYLOAD_INCR_OK: lat={:?}µs, total_bytes={}, uris_count={}, first_uris={:?}",
                                    now_us,
                                    drop_lat,
                                    incr_buf.len(),
                                    uris.len(),
                                    &uris[..uris.len().min(3)]
                                );
                                waiting_drop_payload = false;
                                send_xdnd_finished(&conn, &atoms, win, current_source, true)?;
                                clear_input_shape_to_empty(&conn, win)?;
                                conn.unmap_window(win)?;
                                conn.flush()?;
                                session_active = false;
                                completed_sessions += 1;
                            } else {
                                let pref_lat = t_prefetch_sent.map(|t| t.elapsed().as_micros());
                                println!(
                                    "[{:>8}µs] PREFETCH_INCR_OK: lat={:?}µs, total_bytes={}, uris_count={}, first_uris={:?}",
                                    now_us,
                                    pref_lat,
                                    incr_buf.len(),
                                    uris.len(),
                                    &uris[..uris.len().min(3)]
                                );
                            }
                        } else {
                            incr_buf.extend_from_slice(&prop.value);
                        }
                    }
                }
                _ => {}
            }
        }

        // Poll-mode trigger check if armed
        if armed && cfg.trigger_mode == TriggerMode::Poll {
            let qp = conn.query_pointer(root)?.reply()?;
            let shift_down = qp.mask.contains(KeyButMask::SHIFT);
            if !shift_down && held_at_arm {
                held_at_arm = false;
                println!(
                    "[{:>8}µs] POLL_PREHELD_SHIFT_RELEASED",
                    t_epoch.elapsed().as_micros()
                );
            } else if shift_down && !poll_shift_was_down && !held_at_arm {
                if !session_active {
                    trigger_overlay(
                        &conn,
                        &atoms,
                        &cfg,
                        root,
                        &mut overlay,
                        &create_overlay_window,
                        shm_slice,
                        shm_seg,
                        selection_ts,
                        t_epoch,
                        &mut session_active,
                        &mut overlay_origin,
                        &mut summon_center,
                        &mut child_expanded,
                        &mut t_trigger,
                        &mut t_mapped,
                        &mut t_prefetch_sent,
                        &mut t_enter,
                        &mut t_drop,
                        &mut position_count,
                        &mut stationary_fired,
                        &mut child_expand_deadline,
                    )?;
                } else {
                    println!(
                        "[{:>8}µs] POLL_TRIGGER_TOGGLE_OFF",
                        t_epoch.elapsed().as_micros()
                    );
                    if let Some((win, _)) = overlay {
                        clear_input_shape_to_empty(&conn, win)?;
                        conn.unmap_window(win)?;
                        conn.flush()?;
                    }
                    session_active = false;
                    completed_sessions += 1;
                }
            }
            poll_shift_was_down = shift_down;
        }

        // Dynamic child shape expansion timer (S-4)
        if session_active
            && !child_expanded
            && let Some(dl) = child_expand_deadline
            && Instant::now() >= dl
        {
            child_expand_deadline = None;
            child_expanded = true;
            if let Some((win, gc)) = overlay {
                let cx0 = summon_center.0;
                let cy0 = summon_center.1;
                let cx1 = cx0 + 180;
                let cy1 = cy0;
                draw_wheel_visual(
                    shm_slice,
                    ow as usize,
                    oh as usize,
                    &[(cx0, cy0, 38, 100), (cx1, cy1, 24, 75)],
                );
                set_disc_input_shape(
                    &conn,
                    win,
                    ow,
                    oh,
                    &[(cx0, cy0, 112), (cx1, cy1, 85)],
                    Some((cx0 + 80, cy0 - 45, cx1 - 40, cy0 + 45)),
                )?;
                conn.shm_put_image(
                    win,
                    gc,
                    ow,
                    oh,
                    0,
                    0,
                    ow,
                    oh,
                    0,
                    0,
                    32,
                    ImageFormat::Z_PIXMAP.into(),
                    false,
                    shm_seg,
                    0,
                )?;
                conn.flush()?;
                println!(
                    "[{:>8}µs] SHAPE_EXPANDED_CHILD: added child disc at local ({},{}) r=85",
                    t_epoch.elapsed().as_micros(),
                    cx1,
                    cy1
                );
            }
        }

        // Stationary pointer timer (unsolicited XdndStatus / submenu dwell investigation)
        if session_active
            && !stationary_fired
            && current_source != x11rb::NONE
            && let Some(dl) = stationary_deadline
            && Instant::now() >= dl
        {
            stationary_deadline = None;
            stationary_fired = true;
            if let Some((win, _)) = overlay {
                match cfg.stationary_mode {
                    StationaryMode::RejectToAccept => {
                        last_accept = true;
                        send_xdnd_status(&conn, &atoms, win, current_source, true)?;
                        if cfg.stationary_nudge {
                            conn.warp_pointer(x11rb::NONE, x11rb::NONE, 0, 0, 0, 0, 1, 0)?;
                            conn.warp_pointer(x11rb::NONE, x11rb::NONE, 0, 0, 0, 0, -1, 0)?;
                            conn.flush()?;
                        }
                        println!(
                            "[{:>8}µs] UNSOLICITED_STATUS_SENT: Reject -> Accept (nudge={})",
                            t_epoch.elapsed().as_micros(),
                            cfg.stationary_nudge
                        );
                    }
                    StationaryMode::AcceptToReject => {
                        last_accept = false;
                        send_xdnd_status(&conn, &atoms, win, current_source, false)?;
                        if cfg.stationary_nudge {
                            conn.warp_pointer(x11rb::NONE, x11rb::NONE, 0, 0, 0, 0, 1, 0)?;
                            conn.warp_pointer(x11rb::NONE, x11rb::NONE, 0, 0, 0, 0, -1, 0)?;
                            conn.flush()?;
                        }
                        println!(
                            "[{:>8}µs] UNSOLICITED_STATUS_SENT: Accept -> Reject (nudge={})",
                            t_epoch.elapsed().as_micros(),
                            cfg.stationary_nudge
                        );
                    }
                    StationaryMode::SubmenuDwell => {
                        last_accept = false;
                        send_xdnd_status(&conn, &atoms, win, current_source, false)?;
                        println!(
                            "[{:>8}µs] SUBMENU_DWELL_FIRED_STATIONARY: sent unsolicited status",
                            t_epoch.elapsed().as_micros()
                        );
                    }
                    StationaryMode::None => {}
                }
            }
        }

        std::thread::sleep(Duration::from_millis(1));
    }

    if armed && cfg.trigger_mode == TriggerMode::Raw {
        let _ = select_xi2_raw(&conn, root, false);
    }
    if let Some((win, _)) = overlay {
        let _ = conn.unmap_window(win);
        let _ = conn.destroy_window(win);
    }
    let _ = conn.destroy_window(hidden_win);
    let _ = conn.shm_detach(shm_seg);
    let _ = conn.flush();
    unsafe {
        let _ = munmap(ptr, byte_len);
    }
    println!(
        "[{:>8}µs] LISTEN_EXIT: completed_sessions={}",
        t_epoch.elapsed().as_micros(),
        completed_sessions
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn trigger_overlay<F>(
    conn: &RustConnection,
    atoms: &Atoms,
    cfg: &ListenConfig,
    root: Window,
    overlay: &mut Option<(Window, Gcontext)>,
    create_overlay_window: &F,
    shm_slice: &mut [u8],
    shm_seg: shm::Seg,
    selection_ts: u32,
    t_epoch: Instant,
    session_active: &mut bool,
    overlay_origin: &mut (i32, i32),
    summon_center: &mut (i32, i32),
    child_expanded: &mut bool,
    t_trigger: &mut Option<Instant>,
    t_mapped: &mut Option<Instant>,
    t_prefetch_sent: &mut Option<Instant>,
    t_enter: &mut Option<Instant>,
    t_drop: &mut Option<Instant>,
    position_count: &mut usize,
    stationary_fired: &mut bool,
    child_expand_deadline: &mut Option<Instant>,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: Fn(&RustConnection) -> Result<(Window, Gcontext), Box<dyn std::error::Error>>,
{
    let t0 = Instant::now();
    *t_trigger = Some(t0);
    *t_enter = None;
    *t_drop = None;
    *position_count = 0;
    *stationary_fired = false;
    *child_expanded = false;

    let qp = conn.query_pointer(root)?.reply()?;
    let ptr_x = qp.root_x as i32;
    let ptr_y = qp.root_y as i32;

    let (ow, oh) = cfg.overlay_size;
    let (ox, oy, cx, cy) = if cfg.shape_mode == ShapeMode::ExpandChild {
        // Leave room to the right for the child disc
        let cx = 140i32;
        let cy = (oh as i32) / 2;
        (ptr_x - cx, ptr_y - cy, cx, cy)
    } else {
        let cx = (ow as i32) / 2;
        let cy = (oh as i32) / 2;
        (ptr_x - cx, ptr_y - cy, cx, cy)
    };
    *overlay_origin = (ox, oy);
    *summon_center = (cx, cy);

    if overlay.is_none() {
        *overlay = Some(create_overlay_window(conn)?);
    }
    let (win, gc) = overlay.unwrap();

    // Draw initial disc into SHM
    let r_out = if cfg.shape_mode == ShapeMode::ExpandChild {
        100
    } else {
        120
    };
    draw_wheel_visual(shm_slice, ow as usize, oh as usize, &[(cx, cy, 38, r_out)]);

    // Configure input shape BEFORE mapping
    match cfg.shape_mode {
        ShapeMode::Disc => {
            set_disc_input_shape(conn, win, ow, oh, &[(cx, cy, 150)], None)?;
        }
        ShapeMode::ExpandChild => {
            set_disc_input_shape(conn, win, ow, oh, &[(cx, cy, 112)], None)?;
            *child_expand_deadline = Some(Instant::now() + Duration::from_millis(120));
        }
        ShapeMode::CaptureAll => {
            reset_input_shape_to_full(conn, win)?;
        }
    }

    // Prefetch selection if configured
    match cfg.prefetch_mode {
        PrefetchMode::XFixesTs => {
            *t_prefetch_sent = Some(Instant::now());
            conn.convert_selection(
                win,
                atoms.XdndSelection,
                atoms.URI_LIST,
                atoms.RADIVINE_DND,
                selection_ts,
            )?;
        }
        PrefetchMode::CurrentTime => {
            *t_prefetch_sent = Some(Instant::now());
            conn.convert_selection(
                win,
                atoms.XdndSelection,
                atoms.URI_LIST,
                atoms.RADIVINE_DND,
                x11rb::CURRENT_TIME,
            )?;
        }
        PrefetchMode::None => {
            *t_prefetch_sent = None;
        }
    }

    // Configure, raise, and map
    conn.configure_window(
        win,
        &ConfigureWindowAux::new()
            .x(ox)
            .y(oy)
            .width(ow as u32)
            .height(oh as u32)
            .stack_mode(StackMode::ABOVE),
    )?;
    conn.map_window(win)?;
    conn.shm_put_image(
        win,
        gc,
        ow,
        oh,
        0,
        0,
        ow,
        oh,
        0,
        0,
        32,
        ImageFormat::Z_PIXMAP.into(),
        false,
        shm_seg,
        0,
    )?;
    conn.flush()?;

    let trig_to_map_us = t0.elapsed().as_micros();
    *t_mapped = Some(Instant::now());
    *session_active = true;

    // Optional pointer nudge right after map
    match cfg.nudge_mode {
        NudgeMode::None => {}
        NudgeMode::Zero => {
            conn.warp_pointer(x11rb::NONE, x11rb::NONE, 0, 0, 0, 0, 0, 0)?;
            conn.flush()?;
        }
        NudgeMode::OnePx => {
            conn.warp_pointer(x11rb::NONE, x11rb::NONE, 0, 0, 0, 0, 1, 0)?;
            conn.warp_pointer(x11rb::NONE, x11rb::NONE, 0, 0, 0, 0, -1, 0)?;
            conn.flush()?;
        }
    }

    println!(
        "[{:>8}µs] TRIGGERED: win={:#x}, ptr=({},{}), origin=({},{}), trigger_to_map_us={}, nudge={:?}",
        t_epoch.elapsed().as_micros(),
        win,
        ptr_x,
        ptr_y,
        ox,
        oy,
        trig_to_map_us,
        cfg.nudge_mode
    );
    Ok(())
}

fn send_xdnd_status(
    conn: &RustConnection,
    atoms: &Atoms,
    overlay_win: Window,
    source_win: Window,
    accept: bool,
) -> Result<(), ReplyOrIdError> {
    let flags: u32 = if accept { 1 | 2 } else { 2 };
    let action: u32 = if accept {
        atoms.XdndActionPrivate
    } else {
        x11rb::NONE
    };
    let ev = ClientMessageEvent {
        response_type: x11rb::protocol::xproto::CLIENT_MESSAGE_EVENT,
        format: 32,
        sequence: 0,
        window: source_win,
        type_: atoms.XdndStatus,
        data: ClientMessageData::from([overlay_win, flags, 0, 0, action]),
    };
    conn.send_event(false, source_win, EventMask::NO_EVENT, ev)?;
    conn.flush()?;
    Ok(())
}

fn send_xdnd_finished(
    conn: &RustConnection,
    atoms: &Atoms,
    overlay_win: Window,
    source_win: Window,
    success: bool,
) -> Result<(), ReplyOrIdError> {
    let flags: u32 = if success { 1 } else { 0 };
    let action: u32 = if success {
        atoms.XdndActionPrivate
    } else {
        x11rb::NONE
    };
    let ev = ClientMessageEvent {
        response_type: x11rb::protocol::xproto::CLIENT_MESSAGE_EVENT,
        format: 32,
        sequence: 0,
        window: source_win,
        type_: atoms.XdndFinished,
        data: ClientMessageData::from([overlay_win, flags, action, 0, 0]),
    };
    conn.send_event(false, source_win, EventMask::NO_EVENT, ev)?;
    conn.flush()?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let subcmd = args.next().unwrap_or_else(|| "probe".to_string());

    if subcmd == "probe" {
        return run_probe();
    }

    if subcmd == "listen" {
        let mut cfg = ListenConfig::default();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--trigger-mode" => {
                    cfg.trigger_mode = match args.next().as_deref() {
                        Some("poll") => TriggerMode::Poll,
                        _ => TriggerMode::Raw,
                    };
                }
                "--prefetch" => {
                    cfg.prefetch_mode = match args.next().as_deref() {
                        Some("current-time") => PrefetchMode::CurrentTime,
                        Some("none") => PrefetchMode::None,
                        _ => PrefetchMode::XFixesTs,
                    };
                }
                "--shape-mode" => {
                    cfg.shape_mode = match args.next().as_deref() {
                        Some("expand-child") => ShapeMode::ExpandChild,
                        Some("capture-all") => ShapeMode::CaptureAll,
                        _ => ShapeMode::Disc,
                    };
                }
                "--nudge" => {
                    cfg.nudge_mode = match args.next().as_deref() {
                        Some("zero") => NudgeMode::Zero,
                        Some("one-px") => NudgeMode::OnePx,
                        _ => NudgeMode::None,
                    };
                }
                "--stationary-mode" => {
                    cfg.stationary_mode = match args.next().as_deref() {
                        Some("reject-to-accept") => StationaryMode::RejectToAccept,
                        Some("accept-to-reject") => StationaryMode::AcceptToReject,
                        Some("submenu-dwell") => StationaryMode::SubmenuDwell,
                        _ => StationaryMode::None,
                    };
                }
                "--stationary-delay-ms" => {
                    if let Some(v) = args.next() {
                        cfg.stationary_delay_ms = v.parse().unwrap_or(150);
                    }
                }
                "--stationary-nudge" => {
                    cfg.stationary_nudge = true;
                }
                "--no-precreate" => {
                    cfg.precreate_overlay = false;
                }
                "--size" => {
                    if let (Some(w), Some(h)) = (args.next(), args.next()) {
                        cfg.overlay_size = (w.parse().unwrap_or(400), h.parse().unwrap_or(400));
                    }
                }
                "--max-sessions" => {
                    if let Some(v) = args.next() {
                        cfg.max_sessions = v.parse().unwrap_or(1);
                    }
                }
                "--timeout-s" => {
                    if let Some(v) = args.next() {
                        cfg.timeout_s = v.parse().unwrap_or(30);
                    }
                }
                other => {
                    eprintln!("Unknown arg: {other}");
                }
            }
        }
        return run_listen(cfg);
    }

    eprintln!("Usage: xdnd-overlay-spike [probe | listen [options]]");
    Ok(())
}
