#![no_main]

use std::io::BufReader;

use libfuzzer_sys::fuzz_target;
use radivine_jobs::{JobUpdate, WorkerRequest, WorkerResponse, read_worker_request};

fuzz_target!(|data: &[u8]| {
    let mut reader = BufReader::new(data);
    let _ = read_worker_request(&mut reader);

    if let Ok(req) = serde_json::from_slice::<WorkerRequest>(data) {
        let _ = req.version();
    }
    let _ = serde_json::from_slice::<WorkerResponse>(data);
    let _ = serde_json::from_slice::<JobUpdate>(data);
});
