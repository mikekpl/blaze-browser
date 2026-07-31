//! T063: filter-list text is third-party input (downloaded lists); parsing
//! arbitrary bytes must never panic. Run: `cargo +nightly fuzz run filter_list_parser`.

#![no_main]

use blaze_adblock::engine::AdblockEngine;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let engine = AdblockEngine::from_lists([text]);
        let _ = engine.check(
            "https://ads.example/banner.js",
            "https://news.example/",
            blaze_engine::ResourceKind::Script,
        );
        let _ = engine.cosmetics_for("https://news.example/article");
    }
});
