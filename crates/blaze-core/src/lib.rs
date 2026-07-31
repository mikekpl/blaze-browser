//! `BlazeCore`: the orchestration facade every platform shell talks to
//! (via blaze-ffi). Contract: specs/001-lightweight-adblock-browser/contracts/core-api.md.

pub mod bookmarks;
pub mod diagnostics;
pub mod downloads;
pub mod events;
pub mod navigation;
pub mod session;
pub mod tabs;

use std::path::Path;

use thiserror::Error;

use blaze_storage::settings::Settings;
use blaze_storage::{Profile, StorageError};

use events::{Dispatcher, Event, EventSink};
use tabs::{Rect, TabId, TabManager, WindowId};

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("{0} not found: {1}")]
    NotFound(&'static str, String),
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    #[error("network: {0}")]
    Network(String),
    #[error("internal: {0}")]
    Internal(String),
}

pub struct BlazeCore {
    profile: Profile,
    tabs: std::sync::Mutex<TabManager>,
    dispatcher: std::sync::Arc<Dispatcher>,
    session_debounce: session::SnapshotDebounce,
    media: blaze_media::MediaTracker,
    downloads: downloads::DownloadsRuntime,
}

impl BlazeCore {
    /// Open the profile at `profile_dir` and wire the event sink.
    pub fn new(profile_dir: &Path, sink: Box<dyn EventSink>) -> Result<Self, CoreError> {
        let profile = Profile::open(profile_dir)?;
        diagnostics::install(&profile.dir().join("crashes"));
        Ok(Self {
            profile,
            tabs: std::sync::Mutex::new(TabManager::default()),
            dispatcher: std::sync::Arc::new(Dispatcher::new(sink)),
            session_debounce: session::SnapshotDebounce::default(),
            media: blaze_media::MediaTracker::default(),
            downloads: downloads::DownloadsRuntime::default(),
        })
    }

    /// Flush session/storage state; call before process exit.
    pub fn shutdown(&self) {
        self.flush_session();
        self.profile.flush();
    }

    // ---- Windows & tabs (FR-013..016) ----

    pub fn create_window(&self, frame: Rect) -> WindowId {
        let mut tabs = self.lock_tabs();
        let (window_id, tab_id) = tabs.create_window(frame);
        self.dispatcher.emit(Event::TabCreated {
            tab: tab_id,
            window: window_id.clone(),
            index: 0,
        });
        drop(tabs);
        self.dispatcher.flush();
        self.note_session_changed();
        window_id
    }

    pub fn close_window(&self, window_id: &WindowId) -> Result<(), CoreError> {
        let mut tabs = self.lock_tabs();
        let closed = tabs.close_window(window_id)?;
        let closed_count = closed.len();
        for tab in closed {
            self.media.drop_tab(&tab);
            self.dispatcher.emit(Event::TabClosed {
                tab,
                window: window_id.clone(),
            });
        }
        self.dispatcher.emit(Event::WindowClosed {
            window: window_id.clone(),
        });
        let to_persist: Vec<_> = tabs
            .recently_closed(closed_count)
            .into_iter()
            .cloned()
            .collect();
        drop(tabs);
        for item in &to_persist {
            self.persist_closed_tab(item);
        }
        self.dispatcher.flush();
        self.note_session_changed();
        Ok(())
    }

    pub fn create_tab(
        &self,
        window_id: &WindowId,
        url: Option<String>,
    ) -> Result<TabId, CoreError> {
        let mut tabs = self.lock_tabs();
        let (tab_id, index) = tabs.create_tab(window_id, url)?;
        self.dispatcher.emit(Event::TabCreated {
            tab: tab_id.clone(),
            window: window_id.clone(),
            index,
        });
        drop(tabs);
        self.dispatcher.flush();
        self.note_session_changed();
        Ok(tab_id)
    }

    pub fn close_tab(&self, tab_id: &TabId) -> Result<(), CoreError> {
        let mut tabs = self.lock_tabs();
        let (window_id, window_emptied) = tabs.close_tab(tab_id)?;
        self.media.drop_tab(tab_id);
        self.dispatcher.emit(Event::TabClosed {
            tab: tab_id.clone(),
            window: window_id.clone(),
        });
        if window_emptied {
            self.dispatcher
                .emit(Event::WindowClosed { window: window_id });
        }
        let to_persist: Vec<_> = tabs.recently_closed(1).into_iter().cloned().collect();
        drop(tabs);
        for item in &to_persist {
            self.persist_closed_tab(item);
        }
        self.dispatcher.flush();
        self.note_session_changed();
        Ok(())
    }

    pub fn activate_tab(&self, tab_id: &TabId) -> Result<(), CoreError> {
        self.lock_tabs().activate_tab(tab_id)?;
        self.note_session_changed();
        Ok(())
    }

    pub fn move_tab(
        &self,
        tab_id: &TabId,
        to_window: &WindowId,
        position: u32,
    ) -> Result<(), CoreError> {
        self.lock_tabs().move_tab(tab_id, to_window, position)?;
        self.note_session_changed();
        Ok(())
    }

    pub fn reorder_tab(&self, tab_id: &TabId, position: u32) -> Result<(), CoreError> {
        self.lock_tabs().reorder_tab(tab_id, position)?;
        self.note_session_changed();
        Ok(())
    }

    pub fn set_pinned(&self, tab_id: &TabId, pinned: bool) -> Result<(), CoreError> {
        self.lock_tabs().set_pinned(tab_id, pinned)?;
        self.note_session_changed();
        Ok(())
    }

    pub fn set_muted(&self, tab_id: &TabId, muted: bool) -> Result<(), CoreError> {
        let Some(state) = self.media.set_muted(tab_id, muted) else {
            return Ok(()); // no effective change
        };
        self.apply_audio_state(tab_id, state)
    }

    /// Engine reported playback started/stopped in a tab (T041, FR-021).
    pub fn notify_media_playback(&self, tab_id: &TabId, playing: bool) -> Result<(), CoreError> {
        let Some(state) = self.media.set_playing(tab_id, playing) else {
            return Ok(());
        };
        self.apply_audio_state(tab_id, state)
    }

    fn apply_audio_state(
        &self,
        tab_id: &TabId,
        state: blaze_engine::AudioState,
    ) -> Result<(), CoreError> {
        let mut tabs = self.lock_tabs();
        tabs.set_audio(tab_id, state)?;
        self.dispatcher.emit(Event::TabAudioChanged {
            tab: tab_id.clone(),
            audio_state: state,
        });
        drop(tabs);
        self.dispatcher.flush();
        Ok(())
    }

    // ---- Settings (FR-027, FR-032) ----

    pub fn get_settings(&self) -> Settings {
        self.profile.settings()
    }

    pub fn update_settings(
        &self,
        mutate: impl FnOnce(&mut Settings),
    ) -> Result<Settings, CoreError> {
        let updated = self.profile.update_settings(mutate)?;
        self.dispatcher.emit(Event::SettingsChanged {
            settings: updated.clone(),
        });
        self.dispatcher.flush();
        Ok(updated)
    }

    // ---- Introspection for shells/tests ----

    pub fn with_tabs<R>(&self, f: impl FnOnce(&TabManager) -> R) -> R {
        f(&self.lock_tabs())
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    pub fn dispatcher(&self) -> &Dispatcher {
        &self.dispatcher
    }

    fn lock_tabs(&self) -> std::sync::MutexGuard<'_, TabManager> {
        self.tabs.lock().expect("tab manager lock poisoned")
    }

    pub(crate) fn lock_tabs_pub(&self) -> std::sync::MutexGuard<'_, TabManager> {
        self.lock_tabs()
    }

    pub(crate) fn session_debounce(&self) -> &session::SnapshotDebounce {
        &self.session_debounce
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct NullSink;
    impl EventSink for NullSink {
        fn on_events(&self, _: Vec<Event>) {}
    }

    struct Recorder(Arc<Mutex<Vec<String>>>);
    impl EventSink for Recorder {
        fn on_events(&self, events: Vec<Event>) {
            let mut log = self.0.lock().unwrap();
            for e in events {
                log.push(serde_json::to_string(&e).unwrap());
            }
        }
    }

    fn core() -> (BlazeCore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (BlazeCore::new(dir.path(), Box::new(NullSink)).unwrap(), dir)
    }

    #[test]
    fn core_bootstraps_profile_on_disk() {
        let (core, dir) = core();
        core.shutdown();
        assert!(dir.path().join("profile.db").exists());
        let _ = core;
    }

    #[test]
    fn window_and_tab_lifecycle_through_facade() {
        let (core, _dir) = core();
        let w = core.create_window(Rect {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
        });
        let t = core
            .create_tab(&w, Some("https://example.com".into()))
            .unwrap();
        core.activate_tab(&t).unwrap();
        core.close_tab(&t).unwrap();
        core.with_tabs(|m| m.check_invariants().unwrap());
    }

    #[test]
    fn settings_update_emits_event_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let core = BlazeCore::new(dir.path(), Box::new(Recorder(log.clone()))).unwrap();
        core.update_settings(|s| s.theme = blaze_storage::settings::Theme::Dark)
            .unwrap();
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|e| e.contains("settings_changed"))
        );
        // persisted
        let reloaded =
            blaze_storage::settings::Settings::load_lossy(&dir.path().join("settings.json"));
        assert_eq!(reloaded.theme, blaze_storage::settings::Theme::Dark);
    }
}
