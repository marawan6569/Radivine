//! Unit and X11 integration tests for `radivine-platform` (`TESTING_STRATEGY.md` §4).

use smallvec::smallvec;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder, MapState};

use radivine_platform::x11::X11Platform;
use radivine_platform::x11::doctor::probe_x11_doctor;
use radivine_platform::x11::invoke::translate_keysym;
use radivine_platform::x11::monitors::{parse_xft_dpi, pick_monitor_for_point};
use radivine_platform::x11::shape::rasterize_shapes_to_rectangles;
use radivine_platform::x11::shm::{ChannelShuffle, clip_damage_rect, swizzle_damage_rect};
use radivine_platform::{
    FrameRef, Key, Monitor, PhysPoint, Platform, RectPx, ShapePx, Support, X11Config,
};

#[test]
fn channel_shuffle_swizzles_rgba_to_bgra() {
    let shuffle =
        ChannelShuffle::from_visual(ImageOrder::LSB_FIRST, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF);
    assert_eq!(
        shuffle,
        ChannelShuffle {
            r_idx: 2,
            g_idx: 1,
            b_idx: 0,
            a_idx: 3
        }
    );

    // 2x2 RGBA8 buffer: pixel (0,0) = R=0x11, G=0x22, B=0x33, A=0xFF
    let src = [
        0x11, 0x22, 0x33, 0xFF, 0x44, 0x55, 0x66, 0x80, // row 0
        0x77, 0x88, 0x99, 0x40, 0xAA, 0xBB, 0xCC, 0x00, // row 1
    ];
    let mut dst = [0u8; 16];
    let rect = clip_damage_rect(RectPx::new(0, 0, 2, 2), 2, 2).expect("valid clip");
    swizzle_damage_rect(&src, 8, &mut dst, 8, rect, shuffle);

    // Expect BGRA in little-endian memory: [B, G, R, A]
    assert_eq!(&dst[0..4], &[0x33, 0x22, 0x11, 0xFF]);
    assert_eq!(&dst[4..8], &[0x66, 0x55, 0x44, 0x80]);
    assert_eq!(&dst[8..12], &[0x99, 0x88, 0x77, 0x40]);
    assert_eq!(&dst[12..16], &[0xCC, 0xBB, 0xAA, 0x00]);
}

#[test]
fn clip_damage_rect_handles_negative_and_out_of_bounds() {
    assert_eq!(
        clip_damage_rect(RectPx::new(-10, -5, 30, 20), 100, 100),
        Some(RectPx::new(0, 0, 20, 15))
    );
    assert_eq!(
        clip_damage_rect(RectPx::new(90, 95, 30, 20), 100, 100),
        Some(RectPx::new(90, 95, 10, 5))
    );
    assert_eq!(
        clip_damage_rect(RectPx::new(100, 50, 10, 10), 100, 100),
        None
    );
}

#[test]
fn rasterize_circle_and_polygon_to_x11_rectangles() {
    let shapes = [
        ShapePx::Circle {
            cx: 100.0,
            cy: 100.0,
            r: 50.0,
        },
        ShapePx::Polygon(smallvec![
            (140.0, 90.0),
            (200.0, 70.0),
            (200.0, 130.0),
            (140.0, 110.0),
        ]),
    ];
    let rects = rasterize_shapes_to_rectangles(&shapes, 500, 500);
    assert!(!rects.is_empty());
    assert!(rects.len() <= 200);
    // Verify center (100, 100) and corridor midpoint (170, 100) are covered
    let covers = |px: i16, py: i16| {
        rects.iter().any(|r| {
            px >= r.x && py >= r.y && px < r.x + r.width as i16 && py < r.y + r.height as i16
        })
    };
    assert!(covers(100, 100));
    assert!(covers(170, 100));
    assert!(!covers(10, 10));
}

#[test]
fn parse_xft_dpi_and_monitor_selection() {
    let rm = "*customization: -color\nXft.dpi:\t144\nXft.antialias:\t1\n";
    assert_eq!(parse_xft_dpi(rm), Some(144.0));
    assert_eq!(parse_xft_dpi("Xft.antialias: 1"), None);

    let monitors = vec![
        Monitor {
            id: 0,
            name: "HDMI-0".into(),
            rect: RectPx::new(0, 0, 1920, 1080),
            refresh_mhz: 60_000,
            primary: true,
        },
        Monitor {
            id: 1,
            name: "DP-0".into(),
            rect: RectPx::new(1920, 0, 1920, 1080),
            refresh_mhz: 60_000,
            primary: false,
        },
    ];
    let m = pick_monitor_for_point(&monitors, PhysPoint::new(2500, 500));
    assert_eq!(m.name, "DP-0");
    let fallback = pick_monitor_for_point(&monitors, PhysPoint::new(-500, -500));
    assert_eq!(fallback.name, "HDMI-0");
}

#[test]
fn translate_keysyms_cover_all_navigation_keys() {
    assert_eq!(translate_keysym(0xff51, 0xff51, false), Some(Key::Left));
    assert_eq!(translate_keysym(0xff53, 0xff53, false), Some(Key::Right));
    assert_eq!(translate_keysym(0xff52, 0xff52, false), Some(Key::Up));
    assert_eq!(translate_keysym(0xff54, 0xff54, false), Some(Key::Down));
    assert_eq!(translate_keysym(0xff09, 0xff09, false), Some(Key::Tab));
    assert_eq!(translate_keysym(0xff09, 0xff09, true), Some(Key::ShiftTab));
    assert_eq!(translate_keysym(0xfe20, 0xff09, true), Some(Key::ShiftTab));
    assert_eq!(translate_keysym(0xff0d, 0xff0d, false), Some(Key::Enter));
    assert_eq!(translate_keysym(0x0020, 0x0020, false), Some(Key::Space));
    assert_eq!(
        translate_keysym(0xff08, 0xff08, false),
        Some(Key::Backspace)
    );
    assert_eq!(translate_keysym(0xff1b, 0xff1b, false), Some(Key::Escape));
    assert_eq!(translate_keysym(0x0031, 0x0031, false), Some(Key::Digit(1)));
    assert_eq!(translate_keysym(0x0039, 0x0039, false), Some(Key::Digit(9)));
}

#[test]
fn live_x11_doctor_and_overlay_shm_lifecycle() {
    if std::env::var("DISPLAY").is_err() {
        return;
    }
    let report = match probe_x11_doctor() {
        Ok(r) => r,
        Err(_) => return,
    };
    assert!(report.is_healthy());
    assert_eq!(report.capabilities.invoke_at_pointer, Support::Yes);
    assert_eq!(report.visual.depth, 32);

    let mut plat = X11Platform::connect(X11Config::default()).expect("connect X11Platform");
    let ptr = plat.pointer_position().expect("pointer_position");
    let info = plat.prepare_overlay(ptr).expect("prepare_overlay");
    assert!(info.size.0 > 0 && info.size.1 > 0);

    // Prepare a small 64x64 patch inside the overlay and present it via MIT-SHM
    let w = info.size.0;
    let h = info.size.1;
    let mut rgba = vec![0u8; (w as usize) * (h as usize) * 4];
    // Write a known premultiplied pixel at (10, 10): R=0x20, G=0x40, B=0x80, A=0xFF
    let idx = (10 * (w as usize) + 10) * 4;
    rgba[idx] = 0x20;
    rgba[idx + 1] = 0x40;
    rgba[idx + 2] = 0x80;
    rgba[idx + 3] = 0xFF;

    let frame = FrameRef {
        data: &rgba,
        width: w,
        height: h,
        stride: (w as usize) * 4,
    };
    plat.set_input_region(Some(&[ShapePx::Circle {
        cx: 100.0,
        cy: 100.0,
        r: 80.0,
    }]))
    .expect("set_input_region");
    plat.present(frame, &[RectPx::new(0, 0, 32, 32)])
        .expect("SHM present");
    plat.map_overlay().expect("map_overlay");
    plat.grab_input().expect("grab_input");

    // Read back pixel (10, 10) via GetImage to verify MIT-SHM upload
    let img = plat
        .connection()
        .get_image(
            ImageFormat::Z_PIXMAP,
            plat.overlay_window_id(),
            10,
            10,
            1,
            1,
            !0,
        )
        .expect("get_image request")
        .reply()
        .expect("get_image reply");
    assert_eq!(&img.data[0..4], &[0x80, 0x40, 0x20, 0xFF]);

    plat.ungrab_input();
    plat.hide_overlay();

    let attrs = plat
        .connection()
        .get_window_attributes(plat.overlay_window_id())
        .expect("get_window_attributes")
        .reply()
        .expect("get_window_attributes reply");
    assert_eq!(attrs.map_state, MapState::UNMAPPED);
    plat.release_buffers();
}
