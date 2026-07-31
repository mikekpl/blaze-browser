//! Session lifecycle (T035/T036): debounced snapshots (≤1s, FR-017), restore
//! on launch, and reopen-closed-tab (FR-015).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::events::Event;
use crate::tabs::{Rect, TabHistory, TabId, WindowId};
use crate::{BlazeCore, CoreError};

const SNAPSHOT_DEBOUNCE: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabSnapshot {
    pub url: String,
    pub title: String,
    pub pinned: bool,
    #[serde(default)]
    pub history: TabHistory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowSnapshot {
    pub frame: Rect,
    pub active_index: usize,
    pub tabs: Vec<TabSnapshot>,
}

/// The complete restorable session (contracts/storage-schema.md payload).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub windows: Vec<WindowSnapshot>,
}

/// Debounce state: at most one snapshot write per second; `dirty` marks
/// changes inside the window, flushed by `shutdown` (kill -9 loses ≤1s).
#[derive(Debug, Default)]
pub struct SnapshotDebounce {
    last_write: Mutex<Option<Instant>>,
    dirty: AtomicBool,
}

impl BlazeCore {
    /// Call after any session-shape mutation; writes at most once per second.
    pub fn note_session_changed(&self) {
        let mut last = self
            .session_debounce()
            .last_write
            .lock()
            .expect("debounce lock");
        let due = last.is_none_or(|t| t.elapsed() >= SNAPSHOT_DEBOUNCE);
        if due {
            *last = Some(Instant::now());
            drop(last);
            self.session_debounce()
                .dirty
                .store(false, Ordering::Relaxed);
            self.write_snapshot_now();
        } else {
            self.session_debounce().dirty.store(true, Ordering::Relaxed);
        }
    }

    /// Flush a pending debounced snapshot (shutdown path).
    pub fn flush_session(&self) {
        if self.session_debounce().dirty.swap(false, Ordering::Relaxed) {
            self.write_snapshot_now();
        }
    }

    pub fn write_snapshot_now(&self) {
        let snapshot = self.current_snapshot();
        match serde_json::to_string(&snapshot) {
            Ok(payload) => {
                blaze_storage::session::write_snapshot(self.profile(), payload);
                self.dispatcher().emit(Event::SessionSnapshotWritten {
                    snapshot_id: chrono_secs(),
                });
                self.dispatcher().flush();
            }
            Err(e) => tracing::error!(error = %e, "session snapshot serialization failed"),
        }
    }

    pub fn current_snapshot(&self) -> SessionSnapshot {
        self.with_tabs(|tabs| SessionSnapshot {
            windows: tabs
                .windows()
                .map(|w| {
                    let active_index = w
                        .tab_ids
                        .iter()
                        .position(|t| *t == w.active_tab_id)
                        .unwrap_or(0);
                    WindowSnapshot {
                        frame: w.frame,
                        active_index,
                        tabs: w
                            .tab_ids
                            .iter()
                            .filter_map(|id| tabs.tab(id).ok())
                            .map(|t| TabSnapshot {
                                url: t.url.clone(),
                                title: t.title.clone(),
                                pinned: t.pinned,
                                history: t.history.clone(),
                            })
                            .collect(),
                    }
                })
                .collect(),
        })
    }

    /// Restore the previous session from the latest snapshot (FR-018).
    /// Returns restored window ids (empty when no snapshot exists).
    pub fn restore_previous_session(&self) -> Result<Vec<WindowId>, CoreError> {
        let conn = self.profile().read_conn()?;
        let Some(payload) = blaze_storage::session::latest_snapshot(&conn)? else {
            return Ok(Vec::new());
        };
        let snapshot: SessionSnapshot = serde_json::from_str(&payload)
            .map_err(|e| CoreError::Internal(format!("corrupt session snapshot: {e}")))?;

        let mut restored = Vec::new();
        for window in snapshot.windows {
            if window.tabs.is_empty() {
                continue;
            }
            let tabs: Vec<_> = window
                .tabs
                .into_iter()
                .map(|t| (t.url, t.title, t.pinned, t.history))
                .collect();
            let (window_id, tab_ids) = {
                let mut mgr = self.lock_tabs_pub();
                mgr.restore_window(window.frame, tabs, window.active_index)?
            };
            for (i, tab_id) in tab_ids.iter().enumerate() {
                self.dispatcher().emit(Event::TabCreated {
                    tab: tab_id.clone(),
                    window: window_id.clone(),
                    index: i as u32,
                });
            }
            restored.push(window_id);
        }
        self.dispatcher().flush();
        Ok(restored)
    }

    /// Reopen the most recently closed tab, preferring `window_id` (FR-015).
    pub fn reopen_closed_tab(&self, window_id: &WindowId) -> Result<Option<TabId>, CoreError> {
        let closed = {
            let mut mgr = self.lock_tabs_pub();
            mgr.reopen_closed_tab(window_id)
        };
        let Some(closed) = closed else {
            return Ok(None);
        };
        blaze_storage::session::pop_closed_tab(self.profile(), closed.window_id.clone());

        // Reopen into the requested window if it still exists, else its own.
        let target = if self.with_tabs(|t| t.window(window_id).is_ok()) {
            window_id.clone()
        } else if self.with_tabs(|t| t.window(&closed.window_id).is_ok()) {
            closed.window_id.clone()
        } else {
            return Ok(None);
        };

        let tab_id = self.create_tab(&target, Some(closed.url.clone()))?;
        {
            let mut mgr = self.lock_tabs_pub();
            let tab = mgr.tab_mut(&tab_id)?;
            tab.title = closed.title;
            tab.pinned = closed.pinned;
            tab.history = closed.history;
        }
        self.dispatcher().emit(Event::TabMetaChanged {
            tab: tab_id.clone(),
            url: Some(closed.url),
            title: None,
        });
        self.dispatcher().flush();
        self.note_session_changed();
        Ok(Some(tab_id))
    }

    /// Persist a closed tab into the durable ring (called by close paths).
    pub(crate) fn persist_closed_tab(&self, closed: &crate::tabs::ClosedTab) {
        if let Ok(payload) = serde_json::to_string(closed) {
            blaze_storage::session::push_closed_tab(
                self.profile(),
                closed.window_id.clone(),
                payload,
            );
        }
    }

    /// Reload the closed-tab ring from persistence (startup).
    pub fn load_persisted_closed_tabs(&self) -> Result<(), CoreError> {
        let conn = self.profile().read_conn()?;
        let rows = blaze_storage::session::load_closed_tabs(&conn)?;
        let closed: Vec<crate::tabs::ClosedTab> = rows
            .into_iter()
            .filter_map(|(_, payload)| serde_json::from_str(&payload).ok())
            .collect();
        self.lock_tabs_pub().load_closed_ring(closed);
        Ok(())
    }

    /// Suspend least-recently-used unpinned background tabs beyond
    /// `max_active` (FR-016); returns the suspended tab ids.
    pub fn suspend_lru_tabs(&self) -> Result<Vec<TabId>, CoreError> {
        let keep = self.get_settings().tab_suspend.max_active as usize;
        let candidates = self.with_tabs(|t| t.lru_suspension_candidates(keep));
        let mut suspended = Vec::new();
        {
            let mut mgr = self.lock_tabs_pub();
            for tab_id in candidates {
                if mgr
                    .transition(&tab_id, crate::tabs::TabState::Suspended)
                    .is_ok()
                {
                    self.dispatcher().emit(Event::TabStateChanged {
                        tab: tab_id.clone(),
                        state: crate::tabs::TabState::Suspended,
                    });
                    suspended.push(tab_id);
                }
            }
        }
        self.dispatcher().flush();
        Ok(suspended)
    }
}

fn chrono_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventSink;

    struct NullSink;
    impl EventSink for NullSink {
        fn on_events(&self, _: Vec<Event>) {}
    }

    fn core() -> (BlazeCore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        (
            BlazeCore::new(dir.path(), Box::new(NullSink)).expect("core"),
            dir,
        )
    }

    #[test]
    fn snapshot_round_trips_through_restore() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let core = BlazeCore::new(dir.path(), Box::new(NullSink)).expect("core");
            let w = core.create_window(Rect::default());
            let t2 = core
                .create_tab(&w, Some("https://example.com/".into()))
                .expect("tab");
            core.set_pinned(&t2, true).expect("pin");
            core.write_snapshot_now();
            core.shutdown();
        }
        let core = BlazeCore::new(dir.path(), Box::new(NullSink)).expect("core");
        let restored = core.restore_previous_session().expect("restore");
        assert_eq!(restored.len(), 1);
        core.with_tabs(|tabs| {
            let w = tabs.window(&restored[0]).expect("window");
            assert_eq!(w.tab_ids.len(), 2);
            let t = tabs.tab(&w.tab_ids[1]).expect("tab");
            assert_eq!(t.url, "https://example.com/");
            assert!(t.pinned);
            assert_eq!(t.state, crate::tabs::TabState::Suspended);
            // active tab: index 1 was active (created last)
            assert_eq!(w.active_tab_id, w.tab_ids[1]);
        });
    }

    #[test]
    fn reopen_closed_tab_restores_content_and_persists() {
        let (core, _d) = core();
        let w = core.create_window(Rect::default());
        let t = core
            .create_tab(&w, Some("https://example.com/".into()))
            .expect("tab");
        core.close_tab(&t).expect("close");
        let reopened = core
            .reopen_closed_tab(&w)
            .expect("reopen")
            .expect("some tab");
        core.with_tabs(|tabs| {
            assert_eq!(
                tabs.tab(&reopened).expect("tab").url,
                "https://example.com/"
            );
        });
        // Ring exhausted for this window's extra tabs beyond what was closed.
        assert!(core.reopen_closed_tab(&w).expect("reopen").is_none());
    }

    #[test]
    fn closed_ring_survives_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let core = BlazeCore::new(dir.path(), Box::new(NullSink)).expect("core");
            let w = core.create_window(Rect::default());
            let t = core
                .create_tab(&w, Some("https://kept.example/".into()))
                .expect("tab");
            core.close_tab(&t).expect("close");
            core.shutdown();
        }
        let core = BlazeCore::new(dir.path(), Box::new(NullSink)).expect("core");
        core.load_persisted_closed_tabs().expect("load ring");
        let w = core.create_window(Rect::default());
        let reopened = core
            .reopen_closed_tab(&w)
            .expect("reopen")
            .expect("tab from disk");
        core.with_tabs(|tabs| {
            assert_eq!(
                tabs.tab(&reopened).expect("tab").url,
                "https://kept.example/"
            );
        });
    }

    #[test]
    fn lru_suspension_respects_pins_and_focus() {
        let (core, _d) = core();
        let w = core.create_window(Rect::default());
        let t1 = core.create_tab(&w, None).expect("t1");
        let _t2 = core.create_tab(&w, None).expect("t2");
        let t3 = core.create_tab(&w, None).expect("t3"); // focused
        core.set_pinned(&t1, true).expect("pin");

        // keep=0 → everything unpinned & unfocused suspends
        let suspended = {
            let updated = core
                .update_settings(|s| s.tab_suspend.max_active = 0)
                .expect("settings");
            assert_eq!(updated.tab_suspend.max_active, 0);
            core.suspend_lru_tabs().expect("suspend")
        };
        assert!(!suspended.contains(&t1), "pinned tab never suspends");
        assert!(!suspended.contains(&t3), "focused tab never suspends");
        assert_eq!(suspended.len(), 2, "initial window tab + t2");
    }
}
