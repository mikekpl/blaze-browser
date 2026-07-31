//! Platform data-directory resolution — the only platform-conditional code in core (FR-031).

use std::path::PathBuf;

/// Default profile directory:
/// macOS `~/Library/Application Support/Blaze`, Linux XDG data dir, Windows `%APPDATA%`.
pub fn default_profile_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("Blaze"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_dir_resolves_and_ends_with_blaze() {
        let dir = default_profile_dir().expect("data dir must exist on supported platforms");
        assert!(dir.ends_with("Blaze"));
    }
}
