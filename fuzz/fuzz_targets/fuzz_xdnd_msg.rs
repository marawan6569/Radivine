#![no_main]

use libfuzzer_sys::fuzz_target;
use radivine_platform::x11::selection::SelectionReader;
use radivine_platform::x11::xdnd::{XdndAtoms, XdndTarget};
use std::time::Instant;

fuzz_target!(|data: &[u8]| {
    let atoms = XdndAtoms {
        enter: 10,
        position: 11,
        status: 12,
        leave: 13,
        drop: 14,
        finished: 15,
        action_private: 16,
    };
    let mut target = XdndTarget::new(0x0100_0001, atoms);
    let mut sel = SelectionReader::new(0x0100_0001, 20, 21);
    let now = Instant::now();

    for chunk in data.chunks(24) {
        if chunk.len() < 24 {
            break;
        }
        let msg_selector = chunk[0] % 6;
        let msg_type = match msg_selector {
            0 => atoms.enter,
            1 => atoms.position,
            2 => atoms.leave,
            3 => atoms.drop,
            4 => 999,
            _ => 0,
        };
        let mut d = [0u32; 5];
        for i in 0..5 {
            let base = 4 + i * 4;
            d[i] = u32::from_le_bytes([
                chunk[base],
                chunk[base + 1],
                chunk[base + 2],
                chunk[base + 3],
            ]);
        }
        let _ = target.on_message(msg_type, d);
        let _ = target.status(chunk[1] & 1 != 0);
        let _ = target.finished(chunk[2] & 1 != 0);
        if chunk[3] & 1 != 0 {
            sel.begin("text/uri-list", now);
            let _ = sel.on_selection_notify_property(
                u32::from(chunk[1]),
                u32::from(chunk[2]),
                chunk.to_vec(),
                chunk.len(),
            );
        } else {
            let _ = sel.on_incr_chunk(chunk);
        }
    }
});
