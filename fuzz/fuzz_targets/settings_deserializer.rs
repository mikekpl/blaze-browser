//! T063: settings.json is user-editable on disk; the lossy deserializer must
//! never panic and always yield usable settings (per-key fallback contract).
//! Run: `cargo +nightly fuzz run settings_deserializer`.

#![no_main]

use blaze_storage::settings::Settings;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let settings = Settings::from_json_lossy(text);
        // Round-trip must also hold for whatever came out.
        let json = serde_json::to_string(&settings).expect("settings always serialize");
        let again = Settings::from_json_lossy(&json);
        assert_eq!(settings, again, "lossy parse must be idempotent");
    }
});
