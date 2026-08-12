#![no_main]

use formatwright_engine_sdk::EngineManifest;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(manifest) = serde_json::from_slice::<EngineManifest>(data) {
        let _ = manifest.validate(formatwright_core::ENGINE_PROTOCOL_VERSION);
    }
});
