#![no_main]

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use libfuzzer_sys::fuzz_target;
use radivine_jobs::WirePath;

fuzz_target!(|data: &[u8]| {
    if let Ok(wp) = serde_json::from_slice::<WirePath>(data) {
        let _ = wp.decode_bytes();
        let _ = wp.to_path_buf();
    }

    let path = Path::new(OsStr::from_bytes(data));
    if let Ok(wp) = WirePath::from_absolute_path(path) {
        let json = serde_json::to_vec(&wp).expect("serialize WirePath");
        let decoded: WirePath = serde_json::from_slice(&json).expect("deserialize WirePath");
        let roundtripped = decoded.to_path_buf().expect("roundtrip to PathBuf");
        assert_eq!(roundtripped.as_os_str().as_bytes(), data);
    }
});
