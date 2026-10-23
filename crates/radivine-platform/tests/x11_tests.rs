//! Unit and X11 integration tests for `radivine-platform` (`TESTING_STRATEGY.md` §4).

use std::time::{Duration, Instant};

use smallvec::smallvec;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder, MapState};

use radivine_platform::x11::X11Platform;
use radivine_platform::x11::arming::{
    ArmingTracker, ArmingTransition, BUTTON_RELEASE_DISARM_DELAY, MAX_ARMED_DURATION,
    SESSION_BUTTON_RELEASE_WATCHDOG, TriggerEdge, query_modifier_keycodes,
};
use radivine_platform::x11::doctor::probe_x11_doctor;
use radivine_platform::x11::invoke::translate_keysym;
use radivine_platform::x11::monitors::{parse_xft_dpi, pick_monitor_for_point};
use radivine_platform::x11::selection::{
    MAX_PAYLOAD_BYTES, PAYLOAD_TIMEOUT, SelectionReader, resolve_mime_atom,
};
use radivine_platform::x11::shape::rasterize_shapes_to_rectangles;
use radivine_platform::x11::shm::{ChannelShuffle, clip_damage_rect, swizzle_damage_rect};
use radivine_platform::x11::source::{identify_window_source, parse_proc_comm, read_proc_comm};
use radivine_platform::x11::xdnd::{DndMessageEvent, OutgoingClientMessage, XdndAtoms, XdndTarget};
use radivine_platform::{
    FrameRef, Key, Monitor, PayloadError, PhysPoint, Platform, RectPx, ShapePx, SourceInfo,
    Support, TriggerModifier, X11Config,
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
    if !radivine_platform::is_isolated_test_display() {
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
    let _ = plat.grab_input();

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

#[test]
fn arming_tracker_fresh_press_autorepeat_and_second_press() {
    let t0 = Instant::now();
    // Standard X11 Shift_L = 50, Shift_R = 62
    let mut tracker = ArmingTracker::new(&[50, 62]);
    assert!(!tracker.is_armed());

    // Selection change without a mouse button held must not arm
    assert_eq!(
        tracker.on_selection_owner_changed(
            0x2000,
            (0x100, 0x101),
            false,
            false,
            12345,
            SourceInfo {
                app: Some("nemo".into()),
                pid: Some(42),
            },
            false,
            t0,
        ),
        None
    );
    assert!(!tracker.is_armed());

    // Selection change with button held arms the tracker
    let transition = tracker.on_selection_owner_changed(
        0x2000,
        (0x100, 0x101),
        true,
        false,
        12345,
        SourceInfo {
            app: Some("nemo".into()),
            pid: Some(42),
        },
        false,
        t0,
    );
    assert_eq!(
        transition,
        Some(ArmingTransition::Armed {
            source: SourceInfo {
                app: Some("nemo".into()),
                pid: Some(42),
            }
        })
    );
    assert!(tracker.is_armed());
    assert_eq!(tracker.selection_timestamp(), 12345);

    // Non-Shift keycode is ignored
    assert_eq!(tracker.on_raw_key_press(36), None);

    // Fresh Shift_L press -> FreshPress
    assert_eq!(tracker.on_raw_key_press(50), Some(TriggerEdge::FreshPress));
    // Auto-repeat on Shift_L -> None
    assert_eq!(tracker.on_raw_key_press(50), None);
    // Pressing Shift_R while Shift_L is still down -> None
    assert_eq!(tracker.on_raw_key_press(62), None);
    // Releasing Shift_L while Shift_R is still down -> None
    assert_eq!(tracker.on_raw_key_release(50), None);
    // Releasing Shift_R (now 0 down) -> Released
    assert_eq!(tracker.on_raw_key_release(62), Some(TriggerEdge::Released));

    // Second fresh Shift press while still armed -> FreshPress (for toggle-off)
    assert_eq!(tracker.on_raw_key_press(50), Some(TriggerEdge::FreshPress));
    assert_eq!(tracker.on_raw_key_release(50), Some(TriggerEdge::Released));

    // Selection owner becomes None -> Disarmed
    assert_eq!(
        tracker.on_selection_owner_changed(
            0,
            (0x100, 0x101),
            false,
            false,
            12350,
            SourceInfo::default(),
            false,
            t0,
        ),
        Some(ArmingTransition::Disarmed)
    );
    assert!(!tracker.is_armed());
}

#[test]
fn arming_tracker_ignores_preheld_shift_until_released() {
    let t0 = Instant::now();
    let mut tracker = ArmingTracker::new(&[50, 62]);

    // Armed while Shift is already held (e.g. Shift+drag in Nemo or text editor)
    tracker.on_selection_owner_changed(
        0x2000,
        (0x100, 0x101),
        true,
        true,
        9000,
        SourceInfo {
            app: Some("nemo".into()),
            pid: Some(100),
        },
        false,
        t0,
    );
    assert!(tracker.is_armed());

    // Auto-repeat or second Shift key press while held_at_arm -> None
    assert_eq!(tracker.on_raw_key_press(50), None);
    assert_eq!(tracker.on_raw_key_press(62), None);
    assert_eq!(tracker.on_raw_key_release(62), None);

    // Releasing all Shift keys clears held_at_arm without emitting Released
    assert_eq!(tracker.on_raw_key_release(50), None);

    // Subsequent fresh press now triggers!
    assert_eq!(tracker.on_raw_key_press(50), Some(TriggerEdge::FreshPress));
}

#[test]
fn arming_tracker_poll_mode_and_watchdogs() {
    let t0 = Instant::now();
    let mut tracker = ArmingTracker::new(&[50]);

    // Arm in poll mode with Shift pre-held
    tracker.on_selection_owner_changed(
        0x2000,
        (0x100, 0x101),
        true,
        true,
        100,
        SourceInfo::default(),
        true,
        t0,
    );
    assert!(tracker.on_poll_sample(true, true, t0).is_empty());
    // Release pre-held Shift in poll mode
    assert!(tracker.on_poll_sample(true, false, t0).is_empty());
    // Fresh press in poll mode
    assert_eq!(
        tracker.on_poll_sample(true, true, t0).as_slice(),
        &[TriggerEdge::FreshPress]
    );
    assert_eq!(
        tracker.on_poll_sample(true, false, t0).as_slice(),
        &[TriggerEdge::Released]
    );

    // Raw button release when no session is open sets 200 ms disarm deadline
    tracker.on_raw_button_release(1, t0);
    let check_early = tracker.check_deadlines(t0 + Duration::from_millis(100));
    assert!(!check_early.verify_button_release);
    let check_due = tracker.check_deadlines(t0 + BUTTON_RELEASE_DISARM_DELAY);
    assert!(check_due.verify_button_release);

    // When a session IS open, button release watchdog is 1 s
    tracker.set_session_open(true);
    tracker.on_raw_button_release(1, t0);
    assert!(
        !tracker
            .check_deadlines(t0 + Duration::from_millis(500))
            .verify_button_release
    );
    assert!(
        tracker
            .check_deadlines(t0 + SESSION_BUTTON_RELEASE_WATCHDOG)
            .verify_button_release
    );

    // 120 s max duration timeout disarms immediately
    let check_max = tracker.check_deadlines(t0 + MAX_ARMED_DURATION);
    assert!(check_max.max_duration_expired);
    assert!(!tracker.is_armed());
}

#[test]
fn source_identification_parses_comm_and_resolves_live_window_pid() {
    assert_eq!(parse_proc_comm("nemo\n"), Some("nemo".to_string()));
    assert_eq!(parse_proc_comm("   \n"), None);
    assert!(read_proc_comm(std::process::id()).is_some());

    if !radivine_platform::is_isolated_test_display() {
        return;
    }
    let Ok(plat) = X11Platform::connect(X11Config::default()) else {
        return;
    };
    let atoms = radivine_platform::x11::atoms::Atoms::new(plat.connection())
        .expect("atoms cookie")
        .reply()
        .expect("atoms reply");
    let info = identify_window_source(plat.connection(), &atoms, plat.overlay_window_id(), true);
    assert_eq!(info.pid, Some(std::process::id()));
    assert!(info.app.is_some());

    let shift_kcs = query_modifier_keycodes(plat.connection(), TriggerModifier::Shift);
    assert!(!shift_kcs.is_empty());
}

fn sample_xdnd_atoms() -> XdndAtoms {
    XdndAtoms {
        enter: 10,
        position: 11,
        status: 12,
        leave: 13,
        drop: 14,
        finished: 15,
        action_private: 99,
    }
}

#[test]
fn xdnd_target_enter_position_status_and_drop_lifecycle() {
    let atoms = sample_xdnd_atoms();
    let overlay_win = 0x0500_0001;
    let src_win = 0x0300_0042;
    let mut target = XdndTarget::new(overlay_win, atoms);
    assert!(!target.is_entered());
    assert_eq!(target.effective_selection_timestamp(4000), 4000);

    // XdndEnter: version 5 in bits 24..31, inline types [500, 501, 0], more_types = 0
    let enter_ev = target.on_message(atoms.enter, [src_win, 5 << 24, 500, 501, 0]);
    assert_eq!(
        enter_ev,
        Some(DndMessageEvent::Enter {
            source: src_win,
            version: 5,
            inline_types: smallvec![500, 501],
            more_types: false,
        })
    );
    assert!(target.is_entered());

    // Stale XdndPosition from a different window is ignored
    let packed_pos = ((1920u32 & 0xFFFF) << 16) | (540u32 & 0xFFFF);
    assert_eq!(
        target.on_message(atoms.position, [0x9999, 0, packed_pos, 5000, 77]),
        None
    );

    // Valid XdndPosition (including signed negative coordinate -120, 300)
    let neg_x = (-120i16 as u16) as u32;
    let pos_y = (300i16 as u16) as u32;
    let packed_neg = (neg_x << 16) | pos_y;
    let pos_ev = target.on_message(atoms.position, [src_win, 0, packed_neg, 5500, 88]);
    assert_eq!(
        pos_ev,
        Some(DndMessageEvent::Position {
            source: src_win,
            at: PhysPoint::new(-120, 300),
            timestamp: 5500,
            requested_action: 88,
        })
    );
    assert_eq!(target.effective_selection_timestamp(4000), 5500);

    // Status(accept=true) MUST reply with XdndActionPrivate (99), never requested_action (88)
    assert_eq!(
        target.status(true),
        Some(OutgoingClientMessage {
            destination: src_win,
            msg_type: atoms.status,
            data: [overlay_win, 3, 0, 0, atoms.action_private],
        })
    );
    // Status(accept=false) MUST reply with flags=2 (want positions) and action=NONE (0)
    assert_eq!(
        target.status(false),
        Some(OutgoingClientMessage {
            destination: src_win,
            msg_type: atoms.status,
            data: [overlay_win, 2, 0, 0, 0],
        })
    );

    // XdndDrop with timestamp 6600
    let drop_ev = target.on_message(atoms.drop, [src_win, 0, 6600, 0, 0]);
    assert_eq!(
        drop_ev,
        Some(DndMessageEvent::Drop {
            source: src_win,
            timestamp: 6600,
        })
    );
    assert_eq!(target.effective_selection_timestamp(4000), 6600);

    // Finished(success=true) replies with accepted=1 and XdndActionPrivate, then resets
    assert_eq!(
        target.finished(true),
        Some(OutgoingClientMessage {
            destination: src_win,
            msg_type: atoms.finished,
            data: [overlay_win, 1, atoms.action_private, 0, 0],
        })
    );
    assert!(!target.is_entered());
    assert_eq!(target.status(true), None);
}

#[test]
fn xdnd_target_leave_and_rejected_drop() {
    let atoms = sample_xdnd_atoms();
    let overlay_win = 0x0500_0001;
    let src_win = 0x0300_0042;
    let mut target = XdndTarget::new(overlay_win, atoms);

    // Enter with more_types = true (bit 0 = 1) and version 6 (clamped to 5)
    let enter_ev = target.on_message(atoms.enter, [src_win, (6 << 24) | 1, 500, 501, 502]);
    assert_eq!(
        enter_ev,
        Some(DndMessageEvent::Enter {
            source: src_win,
            version: 5,
            inline_types: smallvec![500, 501, 502],
            more_types: true,
        })
    );

    // Leave clears source
    assert_eq!(
        target.on_message(atoms.leave, [src_win, 0, 0, 0, 0]),
        Some(DndMessageEvent::Leave { source: src_win })
    );
    assert!(!target.is_entered());
    assert_eq!(target.status(true), None);

    // Re-enter and reject drop
    target.on_message(atoms.enter, [src_win, 5 << 24, 500, 0, 0]);
    target.on_message(atoms.drop, [src_win, 0, 7000, 0, 0]);
    assert_eq!(
        target.finished(false),
        Some(OutgoingClientMessage {
            destination: src_win,
            msg_type: atoms.finished,
            data: [overlay_win, 0, 0, 0, 0],
        })
    );
}

#[test]
fn selection_reader_single_shot_incr_refused_timeout_and_size_cap() {
    let t0 = Instant::now();
    let prop_atom = 700;
    let incr_atom = 701;
    let uri_list_atom = 702;
    let mut reader = SelectionReader::new(0x0500_0001, prop_atom, incr_atom);

    // 1. Single-shot transfer
    reader.begin("text/uri-list", t0);
    assert!(reader.is_active());
    let res = reader.on_selection_notify_property(
        prop_atom,
        uri_list_atom,
        b"file:///tmp/a.txt\r\n".to_vec(),
        0,
    );
    assert_eq!(
        res,
        Some((
            "text/uri-list".to_string(),
            Ok(b"file:///tmp/a.txt\r\n".to_vec())
        ))
    );
    assert!(!reader.is_active());

    // 2. Refused transfer (property == NONE)
    reader.begin("text/uri-list", t0);
    assert_eq!(
        reader.on_selection_notify_property(0, 0, Vec::new(), 0),
        Some(("text/uri-list".to_string(), Err(PayloadError::Refused)))
    );

    // 3. Multi-chunk INCR transfer
    reader.begin("text/uri-list", t0);
    assert_eq!(
        reader.on_selection_notify_property(prop_atom, incr_atom, vec![0, 1, 0, 0], 256),
        None
    );
    assert!(reader.is_incr());
    assert_eq!(reader.on_incr_chunk(b"file:///tmp/one.txt\r\n"), None);
    assert_eq!(reader.on_incr_chunk(b"file:///tmp/two.txt\r\n"), None);
    assert_eq!(
        reader.on_incr_chunk(b""),
        Some((
            "text/uri-list".to_string(),
            Ok(b"file:///tmp/one.txt\r\nfile:///tmp/two.txt\r\n".to_vec())
        ))
    );
    assert!(!reader.is_active());

    // 4. Timeout after 2 s
    reader.begin("text/uri-list", t0);
    assert_eq!(reader.check_timeout(t0 + Duration::from_secs(1)), None);
    assert_eq!(
        reader.check_timeout(t0 + PAYLOAD_TIMEOUT),
        Some(("text/uri-list".to_string(), Err(PayloadError::Timeout)))
    );

    // 5. 16 MiB cap in INCR lower_bound_hint
    reader.begin("text/uri-list", t0);
    assert_eq!(
        reader.on_selection_notify_property(
            prop_atom,
            incr_atom,
            Vec::new(),
            MAX_PAYLOAD_BYTES + 1
        ),
        Some(("text/uri-list".to_string(), Err(PayloadError::TooLarge)))
    );

    // 6. Live X11 check: DELETE target is strictly forbidden
    if radivine_platform::is_isolated_test_display()
        && let Ok(plat) = X11Platform::connect(X11Config::default())
    {
        let atoms = radivine_platform::x11::atoms::Atoms::new(plat.connection())
            .expect("atoms cookie")
            .reply()
            .expect("atoms reply");
        assert!(resolve_mime_atom(plat.connection(), &atoms, "DELETE").is_err());
        assert_eq!(
            resolve_mime_atom(plat.connection(), &atoms, "text/uri-list").expect("uri-list atom"),
            atoms.URI_LIST
        );
    }
}

#[test]
fn clipboard_formatting_and_live_x11_selection_serving() {
    use std::path::PathBuf;
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{
        AtomEnum, ConnectionExt as _, CreateWindowAux, PropMode, WindowClass,
    };
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    use radivine_platform::x11::clipboard::{
        format_gnome_copied_files, format_plain_paths, format_uri_list,
    };
    use radivine_platform::{ClipOp, ClipboardContent};

    let sample = ClipboardContent {
        uris: vec![
            "file:///tmp/alpha.txt".to_string(),
            "file:///tmp/dir%20with%20space/beta.rs".to_string(),
        ],
        local_paths: vec![
            PathBuf::from("/tmp/alpha.txt"),
            PathBuf::from("/tmp/dir with space/beta.rs"),
        ],
        operation: ClipOp::Copy,
    };

    assert_eq!(
        format_gnome_copied_files(&sample),
        b"copy\nfile:///tmp/alpha.txt\nfile:///tmp/dir%20with%20space/beta.rs"
    );
    assert_eq!(
        format_uri_list(&sample),
        b"file:///tmp/alpha.txt\r\nfile:///tmp/dir%20with%20space/beta.rs\r\n"
    );
    assert_eq!(
        format_plain_paths(&sample),
        b"/tmp/alpha.txt\n/tmp/dir with space/beta.rs"
    );

    if !radivine_platform::is_isolated_test_display() {
        return;
    }
    let Ok(mut plat) = X11Platform::connect(X11Config::default()) else {
        return;
    };

    plat.set_clipboard(sample.clone())
        .expect("set_clipboard should succeed on live X11");

    // Open a second independent X11 client connection to act as Nemo / text editor requesting CLIPBOARD
    let (client, screen_num) = RustConnection::connect(None).expect("second X11 client");
    let atoms = radivine_platform::x11::atoms::Atoms::new(&client)
        .unwrap()
        .reply()
        .unwrap();
    let root = client.setup().roots[screen_num].root;
    let req_win = client.generate_id().unwrap();
    client
        .create_window(
            0,
            req_win,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new(),
        )
        .unwrap();
    client.flush().unwrap();

    let request_and_read = |plat: &mut X11Platform, target: u32| -> Vec<u8> {
        let dest_prop = atoms.RADIVINE_CLIP;
        let _ = client.delete_property(req_win, dest_prop);
        client
            .convert_selection(
                req_win,
                atoms.CLIPBOARD,
                target,
                dest_prop,
                x11rb::CURRENT_TIME,
            )
            .unwrap();
        client.flush().unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut events = Vec::new();
        while Instant::now() < deadline {
            plat.dispatch_pending(&mut events).unwrap();
            if let Ok(Some(Event::SelectionNotify(sn))) = client.poll_for_event()
                && sn.requestor == req_win
            {
                assert_eq!(sn.property, dest_prop, "conversion should succeed");
                let reply = client
                    .get_property(true, req_win, dest_prop, AtomEnum::ANY, 0, 65536)
                    .unwrap()
                    .reply()
                    .unwrap();
                return reply.value;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for SelectionNotify for target {target}");
    };

    // 1. x-special/gnome-copied-files
    let gnome_bytes = request_and_read(&mut plat, atoms.GNOME_COPIED_FILES);
    assert_eq!(
        gnome_bytes,
        b"copy\nfile:///tmp/alpha.txt\nfile:///tmp/dir%20with%20space/beta.rs"
    );

    // 2. text/uri-list
    let uri_bytes = request_and_read(&mut plat, atoms.URI_LIST);
    assert_eq!(
        uri_bytes,
        b"file:///tmp/alpha.txt\r\nfile:///tmp/dir%20with%20space/beta.rs\r\n"
    );

    // 3. UTF8_STRING
    let utf8_bytes = request_and_read(&mut plat, atoms.UTF8_STRING);
    assert_eq!(utf8_bytes, b"/tmp/alpha.txt\n/tmp/dir with space/beta.rs");

    // 4. TARGETS
    let _ = client.delete_property(req_win, atoms.RADIVINE_CLIP);
    client
        .convert_selection(
            req_win,
            atoms.CLIPBOARD,
            atoms.TARGETS,
            atoms.RADIVINE_CLIP,
            x11rb::CURRENT_TIME,
        )
        .unwrap();
    client.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut got_targets = false;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        plat.dispatch_pending(&mut events).unwrap();
        if let Ok(Some(Event::SelectionNotify(sn))) = client.poll_for_event()
            && sn.requestor == req_win
        {
            let reply = client
                .get_property(true, req_win, atoms.RADIVINE_CLIP, AtomEnum::ATOM, 0, 64)
                .unwrap()
                .reply()
                .unwrap();
            let target_list: Vec<u32> = reply.value32().unwrap().collect();
            assert!(target_list.contains(&atoms.GNOME_COPIED_FILES));
            assert!(target_list.contains(&atoms.URI_LIST));
            assert!(target_list.contains(&atoms.UTF8_STRING));
            assert!(target_list.contains(&atoms.MULTIPLE));
            got_targets = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(got_targets);

    // 5. MULTIPLE
    let prop_a = client
        .intern_atom(false, b"_RADIVINE_TEST_MULT_A")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let prop_b = client
        .intern_atom(false, b"_RADIVINE_TEST_MULT_B")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let pair_prop = atoms.RADIVINE_CLIP;
    client
        .change_property32(
            PropMode::REPLACE,
            req_win,
            pair_prop,
            AtomEnum::ATOM,
            &[atoms.GNOME_COPIED_FILES, prop_a, atoms.UTF8_STRING, prop_b],
        )
        .unwrap();
    client
        .convert_selection(
            req_win,
            atoms.CLIPBOARD,
            atoms.MULTIPLE,
            pair_prop,
            x11rb::CURRENT_TIME,
        )
        .unwrap();
    client.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut got_multiple = false;
    while Instant::now() < deadline {
        plat.dispatch_pending(&mut events).unwrap();
        if let Ok(Some(Event::SelectionNotify(sn))) = client.poll_for_event()
            && sn.requestor == req_win
        {
            assert_eq!(sn.property, pair_prop);
            let rep_a = client
                .get_property(true, req_win, prop_a, AtomEnum::ANY, 0, 4096)
                .unwrap()
                .reply()
                .unwrap();
            let rep_b = client
                .get_property(true, req_win, prop_b, AtomEnum::ANY, 0, 4096)
                .unwrap()
                .reply()
                .unwrap();
            assert_eq!(rep_a.value, format_gnome_copied_files(&sample));
            assert_eq!(rep_b.value, format_plain_paths(&sample));
            got_multiple = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(got_multiple);

    // 6. SelectionClear when another client claims CLIPBOARD
    client
        .set_selection_owner(req_win, atoms.CLIPBOARD, x11rb::CURRENT_TIME)
        .unwrap();
    client.flush().unwrap();
    std::thread::sleep(Duration::from_millis(15));
    plat.dispatch_pending(&mut events).unwrap();

    let _ = client.destroy_window(req_win);
    let _ = client.flush();
}
