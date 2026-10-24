#![no_main]

use libfuzzer_sys::fuzz_target;
use radivine_convert::{format_from_extension, format_from_mime, sniff_format};

fuzz_target!(|data: &[u8]| {
    let _ = sniff_format(data);
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = format_from_extension(s);
        let _ = format_from_mime(s);
    }
});
