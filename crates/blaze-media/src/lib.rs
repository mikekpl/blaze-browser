//! T041: media session state and audible-tab tracking (FR-021..023).
//! Tracks per-tab playback + mute and derives the effective `AudioState`;
//! the core emits `TabAudioChanged` when the derived state changes.

use std::collections::HashMap;
use std::sync::Mutex;

use blaze_engine::AudioState;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct MediaSession {
    playing: bool,
    muted: bool,
}

impl MediaSession {
    fn effective(self) -> AudioState {
        match (self.muted, self.playing) {
            (true, _) => AudioState::Muted,
            (false, true) => AudioState::Audible,
            (false, false) => AudioState::Silent,
        }
    }
}

/// Thread-safe per-tab media registry shared by core and FFI.
#[derive(Debug, Default)]
pub struct MediaTracker {
    sessions: Mutex<HashMap<String, MediaSession>>,
}

impl MediaTracker {
    /// Engine reported playback started/stopped. Returns the new effective
    /// state when it changed, None when unchanged (no event needed).
    pub fn set_playing(&self, tab_id: &str, playing: bool) -> Option<AudioState> {
        self.update(tab_id, |s| s.playing = playing)
    }

    /// User toggled tab mute. Same change-detection contract as `set_playing`.
    pub fn set_muted(&self, tab_id: &str, muted: bool) -> Option<AudioState> {
        self.update(tab_id, |s| s.muted = muted)
    }

    pub fn state(&self, tab_id: &str) -> AudioState {
        self.lock()
            .get(tab_id)
            .copied()
            .unwrap_or_default()
            .effective()
    }

    pub fn drop_tab(&self, tab_id: &str) {
        self.lock().remove(tab_id);
    }

    /// Tabs currently audible (for shell indicators / diagnostics).
    pub fn audible_tabs(&self) -> Vec<String> {
        self.lock()
            .iter()
            .filter(|(_, s)| s.effective() == AudioState::Audible)
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn update(&self, tab_id: &str, f: impl FnOnce(&mut MediaSession)) -> Option<AudioState> {
        let mut sessions = self.lock();
        let session = sessions.entry(tab_id.to_owned()).or_default();
        let before = session.effective();
        f(session);
        let after = session.effective();
        (before != after).then_some(after)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, MediaSession>> {
        self.sessions.lock().expect("media tracker lock poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_drives_audible_state() {
        let t = MediaTracker::default();
        assert_eq!(t.set_playing("a", true), Some(AudioState::Audible));
        assert_eq!(t.set_playing("a", true), None, "no change, no event");
        assert_eq!(t.set_playing("a", false), Some(AudioState::Silent));
    }

    #[test]
    fn mute_overrides_playback_and_unmute_restores() {
        let t = MediaTracker::default();
        t.set_playing("a", true);
        assert_eq!(t.set_muted("a", true), Some(AudioState::Muted));
        assert_eq!(
            t.set_muted("a", false),
            Some(AudioState::Audible),
            "unmute while playing returns to audible, not silent"
        );
    }

    #[test]
    fn audible_tabs_lists_only_playing_unmuted() {
        let t = MediaTracker::default();
        t.set_playing("a", true);
        t.set_playing("b", true);
        t.set_muted("b", true);
        t.set_playing("c", false);
        assert_eq!(t.audible_tabs(), vec!["a".to_string()]);
        t.drop_tab("a");
        assert!(t.audible_tabs().is_empty());
    }
}
