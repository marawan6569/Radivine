#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use radivine_convert::{
    ConversionBackend, FormatId, ResourceLimitsConfig, SourceReader, sniff_format,
};
use radivine_convert_image::ImageBackend;

fuzz_target!(|data: &[u8]| {
    let backend = ImageBackend::with_limits(ResourceLimitsConfig {
        max_source_bytes: 1_048_576,
        max_dimension: 256,
        max_pixels: 65_536,
        max_alloc_bytes: 262_144,
        per_item_timeout_secs: 5,
        ..ResourceLimitsConfig::default()
    });
    let fmt = sniff_format(data).unwrap_or(FormatId::Png);
    let mut cursor = Cursor::new(data);
    let mut reader = SourceReader::new(&mut cursor);
    let _ = backend.inspect(&mut reader, fmt);
});
