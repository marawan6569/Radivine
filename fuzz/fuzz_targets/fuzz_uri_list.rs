#![no_main]

use libfuzzer_sys::fuzz_target;
use radivine_core::{BasicContext, InvocationMode, SourceInfo, parse_uri_list, sniff_bytes};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let uris = parse_uri_list(&text);
    let ctx = BasicContext::from_uris(&uris, InvocationMode::Drag, SourceInfo::default());
    for item in &ctx.items {
        if let Some(ref p) = item.local_path {
            assert!(!p.as_os_str().as_encoded_bytes().contains(&0));
        }
    }
    let _ = sniff_bytes(data);
});
