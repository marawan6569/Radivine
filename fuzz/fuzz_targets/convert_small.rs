#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use radivine_convert::{
    CapabilityCatalog, ConversionEngine, ConversionOptions, FormatId, NoopControl,
    ResourceLimitsConfig, SameFormatPolicy, SourceSpec,
};
use radivine_convert_image::{ImageBackend, descriptor};

fuzz_target!(|data: &[u8]| {
    if data.len() > 65_536 {
        return;
    }
    let limits = ResourceLimitsConfig {
        max_source_bytes: 65_536,
        max_dimension: 256,
        max_pixels: 65_536,
        max_alloc_bytes: 262_144,
        per_item_timeout_secs: 5,
        ..ResourceLimitsConfig::default()
    };
    let backend = Arc::new(ImageBackend::with_limits(limits.clone()));
    let catalog = CapabilityCatalog::builtin_only(descriptor());
    let engine = ConversionEngine::new(catalog, vec![backend], limits);

    let Ok(tmp) = tempfile::tempdir() else {
        return;
    };
    let src_path = tmp.path().join("fuzz_in.bin");
    if std::fs::write(&src_path, data).is_err() {
        return;
    }

    let opts = ConversionOptions {
        target: FormatId::WebP,
        same_format: SameFormatPolicy::Reencode,
        ..ConversionOptions::default()
    };
    let Ok((validated, _)) = opts.validate(engine.catalog()) else {
        return;
    };
    let plan = engine.plan(&[SourceSpec::from_path(&src_path)], &validated);
    if let Some(item) = plan.items.first() {
        let _ = engine.convert_item(item, &validated, &mut NoopControl);
    }
});
