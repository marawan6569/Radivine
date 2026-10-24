#![no_main]

use libfuzzer_sys::fuzz_target;
use radivine_convert_image::exif::{
    normalize_exif_tiff_slice, parse_orientation, reset_orientation_in_place,
    sanitize_exif_keep_safe,
};

fuzz_target!(|data: &[u8]| {
    let _ = normalize_exif_tiff_slice(data);
    let _ = parse_orientation(data);
    let mut owned = data.to_vec();
    if reset_orientation_in_place(&mut owned).is_some() {
        assert_eq!(parse_orientation(&owned), Some(1));
    }
    let _ = sanitize_exif_keep_safe(data, false);
    let _ = sanitize_exif_keep_safe(data, true);
});
