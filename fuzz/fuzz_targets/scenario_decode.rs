#![no_main]

use glade_discover_sim::Scenario;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(json) = std::str::from_utf8(data) {
        let _ = Scenario::from_json(json);
    }
});
