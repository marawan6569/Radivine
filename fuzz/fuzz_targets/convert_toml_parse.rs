#![no_main]

use libfuzzer_sys::fuzz_target;
use radivine_convert::{ConvertSettings, PresetRef};

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data)
        && let Ok((settings, _warnings)) = ConvertSettings::from_toml_str(text)
    {
        let _ = settings.resolve(&PresetRef::QuickConvert, None);
        let _ = settings.resolve(&PresetRef::DefaultPreset, None);
        let _ = settings.save_preserving_comments(text);
    }
});
