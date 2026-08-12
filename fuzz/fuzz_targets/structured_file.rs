#![no_main]

use std::io::Write;
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let Some((&selector, payload)) = data.split_first() else {
        return;
    };
    let suffix = match selector % 4 {
        0 => ".json",
        1 => ".yaml",
        2 => ".csv",
        _ => ".xml",
    };
    let Ok(mut fixture) = tempfile::Builder::new().suffix(suffix).tempfile() else {
        return;
    };
    if fixture.write_all(payload).is_err() || fixture.flush().is_err() {
        return;
    }
    let runtime = RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("fuzz runtime")
    });
    let _ = runtime.block_on(formatwright_core::inspect_structured(fixture.path()));
});
