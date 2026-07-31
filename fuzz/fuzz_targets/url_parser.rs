//! T063: address-bar input resolution must never panic on arbitrary input
//! (it decides URL vs search for every keystroke-submitted string).
//! Run: `cargo +nightly fuzz run url_parser` (from the repo root).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(input) = std::str::from_utf8(data) {
        let _ = blaze_net::url::resolve_input(input, "https://www.google.com/search?q=%s");
        // Custom template containing the input exercises the %s expansion path.
        let _ = blaze_net::url::resolve_input("query", input);
    }
});
