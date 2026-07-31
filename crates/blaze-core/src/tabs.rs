//! Window/Tab domain model and tab lifecycle state machine
//! (data-model.md: Tab, Window; FR-013..016).

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use blaze_engine::AudioState;

use crate::CoreError;

pub type TabId = String;
pub type WindowId = String;

const CLOSED_TAB_RING: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabState {
    Active,
    Loading,
    Suspended,
    Crashed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
}

/// Per-tab back/forward stack with a cursor (FR-004).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TabHistory {
    pub entries: Vec<HistoryEntry>,
    pub cursor: usize,
}

impl TabHistory {
    pub fn push(&mut self, url: String, title: String) {
        self.entries
            .truncate(self.cursor.saturating_add(1).min(self.entries.len()));
        self.entries.push(HistoryEntry { url, title });
        self.cursor = self.entries.len() - 1;
    }

    pub fn can_go_back(&self) -> bool {
        self.cursor > 0 && !self.entries.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.entries.is_empty() && self.cursor + 1 < self.entries.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    pub window_id: WindowId,
    pub url: String,
    pub title: String,
    pub history: TabHistory,
    pub state: TabState,
    pub pinned: bool,
    pub audio_state: AudioState,
    pub ads_blocked: u32,
    pub trackers_blocked: u32,
    /// Monotonic activation counter for LRU suspension (FR-016).
    #[serde(default)]
    pub last_activated: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub id: WindowId,
    pub tab_ids: Vec<TabId>,
    pub active_tab_id: TabId,
    pub frame: Rect,
}

/// Snapshot of a closed tab for "reopen closed tab" (FR-015).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClosedTab {
    pub window_id: WindowId,
    pub url: String,
    pub title: String,
    pub pinned: bool,
    pub history: TabHistory,
}

/// In-memory ownership of all windows/tabs. Invariant: every window has ≥1 tab
/// and `active_tab_id` always references a member of `tab_ids`.
#[derive(Debug, Default)]
pub struct TabManager {
    windows: HashMap<WindowId, Window>,
    tabs: HashMap<TabId, Tab>,
    window_order: Vec<WindowId>,
    closed: VecDeque<ClosedTab>,
    activation_clock: u64,
}

impl TabManager {
    pub fn create_window(&mut self, frame: Rect) -> (WindowId, TabId) {
        let window_id: WindowId = Uuid::new_v4().to_string();
        let tab = self.new_tab_struct(&window_id, None);
        let tab_id = tab.id.clone();
        self.tabs.insert(tab_id.clone(), tab);
        self.windows.insert(
            window_id.clone(),
            Window {
                id: window_id.clone(),
                tab_ids: vec![tab_id.clone()],
                active_tab_id: tab_id.clone(),
                frame,
            },
        );
        self.window_order.push(window_id.clone());
        (window_id, tab_id)
    }

    pub fn close_window(&mut self, window_id: &WindowId) -> Result<Vec<TabId>, CoreError> {
        let window = self
            .windows
            .remove(window_id)
            .ok_or_else(|| CoreError::NotFound("window", window_id.clone()))?;
        self.window_order.retain(|w| w != window_id);
        for tab_id in &window.tab_ids {
            if let Some(tab) = self.tabs.remove(tab_id) {
                self.remember_closed(tab);
            }
        }
        Ok(window.tab_ids)
    }

    pub fn create_tab(
        &mut self,
        window_id: &WindowId,
        url: Option<String>,
    ) -> Result<(TabId, u32), CoreError> {
        if !self.windows.contains_key(window_id) {
            return Err(CoreError::NotFound("window", window_id.clone()));
        }
        let tab = self.new_tab_struct(window_id, url);
        let tab_id = tab.id.clone();
        self.tabs.insert(tab_id.clone(), tab);
        let window = self.windows.get_mut(window_id).expect("checked above");
        window.tab_ids.push(tab_id.clone());
        window.active_tab_id = tab_id.clone(); // new tab receives focus (US2-AC1)
        Ok((tab_id, (window.tab_ids.len() - 1) as u32))
    }

    /// Close a tab; focus moves to the adjacent tab (US2-AC2). Closing the last
    /// tab of a window closes the window. Returns (window_id, window_now_empty).
    pub fn close_tab(&mut self, tab_id: &TabId) -> Result<(WindowId, bool), CoreError> {
        let tab = self
            .tabs
            .remove(tab_id)
            .ok_or_else(|| CoreError::NotFound("tab", tab_id.clone()))?;
        let window_id = tab.window_id.clone();
        self.remember_closed(tab);

        let window = self
            .windows
            .get_mut(&window_id)
            .ok_or_else(|| CoreError::NotFound("window", window_id.clone()))?;
        let idx = window.tab_ids.iter().position(|t| t == tab_id).unwrap_or(0);
        window.tab_ids.retain(|t| t != tab_id);

        if window.tab_ids.is_empty() {
            self.windows.remove(&window_id);
            self.window_order.retain(|w| w != &window_id);
            return Ok((window_id, true));
        }
        if window.active_tab_id == *tab_id {
            let new_idx = idx.min(window.tab_ids.len() - 1);
            window.active_tab_id = window.tab_ids[new_idx].clone();
        }
        Ok((window_id, false))
    }

    pub fn activate_tab(&mut self, tab_id: &TabId) -> Result<WindowId, CoreError> {
        let window_id = self.tab(tab_id)?.window_id.clone();
        let window = self
            .windows
            .get_mut(&window_id)
            .expect("tab.window_id always valid");
        window.active_tab_id = tab_id.clone();
        self.touch(tab_id);
        Ok(window_id)
    }

    /// Move a tab to another window at `position` (drag-out/attach, US2-AC4).
    pub fn move_tab(
        &mut self,
        tab_id: &TabId,
        to_window: &WindowId,
        position: u32,
    ) -> Result<(), CoreError> {
        if !self.windows.contains_key(to_window) {
            return Err(CoreError::NotFound("window", to_window.clone()));
        }
        let from_window = self.tab(tab_id)?.window_id.clone();
        if from_window == *to_window {
            return self.reorder_tab(tab_id, position);
        }

        // Detach from source (may close an emptied source window).
        let source = self.windows.get_mut(&from_window).expect("valid window");
        let idx = source
            .tab_ids
            .iter()
            .position(|t| t == tab_id)
            .expect("tab in its window");
        source.tab_ids.remove(idx);
        if source.tab_ids.is_empty() {
            self.windows.remove(&from_window);
            self.window_order.retain(|w| w != &from_window);
        } else if source.active_tab_id == *tab_id {
            let new_idx = idx.min(source.tab_ids.len() - 1);
            source.active_tab_id = source.tab_ids[new_idx].clone();
        }

        let dest = self.windows.get_mut(to_window).expect("checked above");
        let pos = (position as usize).min(dest.tab_ids.len());
        dest.tab_ids.insert(pos, tab_id.clone());
        dest.active_tab_id = tab_id.clone();
        self.tabs.get_mut(tab_id).expect("tab exists").window_id = to_window.clone();
        Ok(())
    }

    pub fn reorder_tab(&mut self, tab_id: &TabId, position: u32) -> Result<(), CoreError> {
        let window_id = self.tab(tab_id)?.window_id.clone();
        let window = self.windows.get_mut(&window_id).expect("valid window");
        let idx = window
            .tab_ids
            .iter()
            .position(|t| t == tab_id)
            .expect("tab in its window");
        window.tab_ids.remove(idx);
        let pos = (position as usize).min(window.tab_ids.len());
        window.tab_ids.insert(pos, tab_id.clone());
        Ok(())
    }

    pub fn set_pinned(&mut self, tab_id: &TabId, pinned: bool) -> Result<(), CoreError> {
        self.tab_mut(tab_id)?.pinned = pinned;
        Ok(())
    }

    pub fn set_audio(&mut self, tab_id: &TabId, audio: AudioState) -> Result<(), CoreError> {
        self.tab_mut(tab_id)?.audio_state = audio;
        Ok(())
    }

    /// Tab lifecycle transitions (data-model.md). Invalid transitions error —
    /// this is the state machine the property tests exercise.
    pub fn transition(&mut self, tab_id: &TabId, to: TabState) -> Result<TabState, CoreError> {
        let tab = self.tab_mut(tab_id)?;
        let from = tab.state;
        let ok = matches!(
            (from, to),
            (TabState::Active, TabState::Loading)
                | (TabState::Loading, TabState::Active)
                | (TabState::Loading, TabState::Loading)
                | (TabState::Active, TabState::Suspended)
                | (TabState::Suspended, TabState::Loading)
                | (_, TabState::Crashed)
                | (TabState::Crashed, TabState::Loading)
        );
        if !ok {
            return Err(CoreError::InvalidArgument(format!(
                "invalid tab transition {from:?} → {to:?}"
            )));
        }
        tab.state = to;
        Ok(to)
    }

    pub fn reopen_closed_tab(&mut self, window_id: &WindowId) -> Option<ClosedTab> {
        // Prefer the most recent tab closed in this window, else any.
        if let Some(pos) = self.closed.iter().rposition(|c| c.window_id == *window_id) {
            return self.closed.remove(pos);
        }
        self.closed.pop_back()
    }

    /// Reload the closed-tab ring from persistence (startup, T035).
    pub fn load_closed_ring(&mut self, closed: Vec<ClosedTab>) {
        self.closed = closed.into_iter().collect();
        while self.closed.len() > CLOSED_TAB_RING {
            self.closed.pop_front();
        }
    }

    /// The `n` most recently closed tabs, newest first.
    pub fn recently_closed(&self, n: usize) -> Vec<&ClosedTab> {
        self.closed.iter().rev().take(n).collect()
    }

    /// Recreate a window from a session snapshot (T036). `tabs` must be
    /// non-empty; returns the new window id plus per-tab ids in order.
    pub fn restore_window(
        &mut self,
        frame: Rect,
        tabs: Vec<(String, String, bool, TabHistory)>,
        active_index: usize,
    ) -> Result<(WindowId, Vec<TabId>), CoreError> {
        if tabs.is_empty() {
            return Err(CoreError::InvalidArgument(
                "restored window has no tabs".into(),
            ));
        }
        let (window_id, first_tab) = self.create_window(frame);
        let mut tab_ids = Vec::with_capacity(tabs.len());
        for (i, (url, title, pinned, history)) in tabs.into_iter().enumerate() {
            let tab_id = if i == 0 {
                first_tab.clone()
            } else {
                self.create_tab(&window_id, None)?.0
            };
            let tab = self.tab_mut(&tab_id)?;
            tab.url = url;
            tab.title = title;
            tab.pinned = pinned;
            tab.history = history;
            // Restored tabs start suspended; the active one loads on focus (FR-016).
            tab.state = TabState::Suspended;
            tab_ids.push(tab_id);
        }
        let active = tab_ids.get(active_index).unwrap_or(&tab_ids[0]).clone();
        self.activate_tab(&active)?;
        Ok((window_id, tab_ids))
    }

    /// Tabs to suspend under LRU pressure: `Active`-state, unpinned,
    /// not window-active, least recently used first, beyond `keep`.
    pub fn lru_suspension_candidates(&self, keep: usize) -> Vec<TabId> {
        let mut eligible: Vec<&Tab> = self
            .tabs
            .values()
            .filter(|t| t.state == TabState::Active && !t.pinned)
            .filter(|t| {
                self.windows
                    .get(&t.window_id)
                    .is_none_or(|w| w.active_tab_id != t.id)
            })
            .collect();
        if eligible.len() <= keep {
            return Vec::new();
        }
        eligible.sort_by_key(|t| t.last_activated);
        eligible[..eligible.len() - keep]
            .iter()
            .map(|t| t.id.clone())
            .collect()
    }

    fn touch(&mut self, tab_id: &TabId) {
        self.activation_clock += 1;
        let clock = self.activation_clock;
        if let Some(tab) = self.tabs.get_mut(tab_id) {
            tab.last_activated = clock;
        }
    }

    pub fn tab(&self, tab_id: &TabId) -> Result<&Tab, CoreError> {
        self.tabs
            .get(tab_id)
            .ok_or_else(|| CoreError::NotFound("tab", tab_id.clone()))
    }

    pub fn tab_mut(&mut self, tab_id: &TabId) -> Result<&mut Tab, CoreError> {
        self.tabs
            .get_mut(tab_id)
            .ok_or_else(|| CoreError::NotFound("tab", tab_id.clone()))
    }

    pub fn window(&self, window_id: &WindowId) -> Result<&Window, CoreError> {
        self.windows
            .get(window_id)
            .ok_or_else(|| CoreError::NotFound("window", window_id.clone()))
    }

    pub fn windows(&self) -> impl Iterator<Item = &Window> {
        self.window_order
            .iter()
            .filter_map(|id| self.windows.get(id))
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    fn new_tab_struct(&mut self, window_id: &WindowId, url: Option<String>) -> Tab {
        self.activation_clock += 1;
        Tab {
            id: Uuid::new_v4().to_string(),
            window_id: window_id.clone(),
            url: url.unwrap_or_else(|| "about:newtab".to_owned()),
            title: String::new(),
            history: TabHistory::default(),
            state: TabState::Active,
            pinned: false,
            audio_state: AudioState::Silent,
            ads_blocked: 0,
            trackers_blocked: 0,
            last_activated: self.activation_clock,
        }
    }

    fn remember_closed(&mut self, tab: Tab) {
        self.closed.push_back(ClosedTab {
            window_id: tab.window_id,
            url: tab.url,
            title: tab.title,
            pinned: tab.pinned,
            history: tab.history,
        });
        while self.closed.len() > CLOSED_TAB_RING {
            self.closed.pop_front();
        }
    }

    /// Test/debug invariant check: every window non-empty with a valid active tab,
    /// every tab's window_id backlink correct.
    pub fn check_invariants(&self) -> Result<(), String> {
        for w in self.windows.values() {
            if w.tab_ids.is_empty() {
                return Err(format!("window {} has no tabs", w.id));
            }
            if !w.tab_ids.contains(&w.active_tab_id) {
                return Err(format!("window {} active tab not a member", w.id));
            }
            for t in &w.tab_ids {
                match self.tabs.get(t) {
                    None => return Err(format!("window {} references missing tab {t}", w.id)),
                    Some(tab) if tab.window_id != w.id => {
                        return Err(format!("tab {t} backlink mismatch"));
                    }
                    _ => {}
                }
            }
        }
        if self.tabs.len()
            != self
                .windows
                .values()
                .map(|w| w.tab_ids.len())
                .sum::<usize>()
        {
            return Err("orphaned tabs exist".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            w: 1280.0,
            h: 800.0,
        }
    }

    #[test]
    fn window_starts_with_one_focused_tab() {
        let mut m = TabManager::default();
        let (w, t) = m.create_window(frame());
        assert_eq!(m.window(&w).unwrap().active_tab_id, t);
        m.check_invariants().unwrap();
    }

    #[test]
    fn closing_active_tab_focuses_adjacent() {
        let mut m = TabManager::default();
        let (w, t0) = m.create_window(frame());
        let (t1, _) = m.create_tab(&w, None).unwrap();
        let (t2, _) = m.create_tab(&w, None).unwrap();
        m.activate_tab(&t1).unwrap();
        m.close_tab(&t1).unwrap();
        // adjacent = the tab that took index 1 (t2)
        assert_eq!(m.window(&w).unwrap().active_tab_id, t2);
        m.check_invariants().unwrap();
        let _ = t0;
    }

    #[test]
    fn closing_last_tab_closes_window() {
        let mut m = TabManager::default();
        let (w, t) = m.create_window(frame());
        let (_, empty) = m.close_tab(&t).unwrap();
        assert!(empty);
        assert!(m.window(&w).is_err());
    }

    #[test]
    fn move_tab_across_windows_updates_backlink_and_focus() {
        let mut m = TabManager::default();
        let (w1, _) = m.create_window(frame());
        let (w2, _) = m.create_window(frame());
        let (t, _) = m.create_tab(&w1, None).unwrap();
        m.move_tab(&t, &w2, 0).unwrap();
        assert_eq!(m.tab(&t).unwrap().window_id, w2);
        assert_eq!(m.window(&w2).unwrap().active_tab_id, t);
        assert_eq!(m.window(&w2).unwrap().tab_ids[0], t);
        m.check_invariants().unwrap();
    }

    #[test]
    fn reopen_closed_tab_restores_history() {
        let mut m = TabManager::default();
        let (w, _) = m.create_window(frame());
        let (t, _) = m
            .create_tab(&w, Some("https://example.com".into()))
            .unwrap();
        m.tab_mut(&t)
            .unwrap()
            .history
            .push("https://example.com".into(), "Example".into());
        m.close_tab(&t).unwrap();
        let restored = m.reopen_closed_tab(&w).expect("closed tab remembered");
        assert_eq!(restored.url, "https://example.com");
        assert_eq!(restored.history.entries.len(), 1);
    }

    #[test]
    fn invalid_transition_rejected() {
        let mut m = TabManager::default();
        let (_, t) = m.create_window(frame());
        m.transition(&t, TabState::Suspended).unwrap();
        // Suspended → Active without Loading is invalid
        assert!(m.transition(&t, TabState::Active).is_err());
        // Suspended → Loading → Active is the legal path
        m.transition(&t, TabState::Loading).unwrap();
        m.transition(&t, TabState::Active).unwrap();
    }

    #[test]
    fn crashed_tab_can_reload() {
        let mut m = TabManager::default();
        let (_, t) = m.create_window(frame());
        m.transition(&t, TabState::Crashed).unwrap();
        m.transition(&t, TabState::Loading).unwrap();
        m.transition(&t, TabState::Active).unwrap();
    }

    #[test]
    fn history_push_truncates_forward_entries() {
        let mut h = TabHistory::default();
        h.push("a".into(), "".into());
        h.push("b".into(), "".into());
        h.push("c".into(), "".into());
        h.cursor = 1; // went back to b
        h.push("d".into(), "".into());
        assert_eq!(
            h.entries.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "d"]
        );
        assert!(!h.can_go_forward());
        assert!(h.can_go_back());
    }
}
