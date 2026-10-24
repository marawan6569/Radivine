#![no_main]

use libfuzzer_sys::fuzz_target;
use radivine_convert_image::riff::{inspect_webp_container, mux_webp_metadata};

fuzz_target!(|data: &[u8]| {
    let _ = inspect_webp_container(data);
    let mid = data.len() / 2;
    let (container, meta) = data.split_at(mid);
    if let Ok(muxed) = mux_webp_metadata(container, 64, 48, Some(meta), Some(meta)) {
        let _ = inspect_webp_container(&muxed);
    }
});
