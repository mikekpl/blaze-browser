//! Navigation orchestration (T028/T029): address-bar input → resolved URL or
//! blocked navigation; commit/finish notifications from the engine backend;
//! global history recording (FR-004) via the storage writer.

use rusqlite::params;

use crate::events::Event;
use crate::tabs::{TabId, TabState};
use crate::{BlazeCore, CoreError};

impl BlazeCore {
    /// Resolve address-bar `input` for `tab_id` and mark the tab loading.
    ///
    /// Returns the URL the shell should load. Dangerous input emits
    /// `NavigationBlocked` (→ friendly error page, FR-005) and errors.
    pub fn navigate(&self, tab_id: &TabId, input: &str) -> Result<String, CoreError> {
        let settings = self.get_settings();
        let template = settings.search_engine.template();
        let url = match blaze_net::url::resolve_input(input, template) {
            Ok(url) => url.to_string(),
            Err(e) => {
                self.dispatcher.emit(Event::NavigationBlocked {
                    tab: tab_id.clone(),
                    url: input.to_string(),
                    reason: e.to_string(),
                });
                self.dispatcher.flush();
                return Err(CoreError::InvalidArgument(e.to_string()));
            }
        };

        let mut tabs = self.lock_tabs_pub();
        tabs.transition(tab_id, TabState::Loading)?;
        self.dispatcher.emit(Event::TabStateChanged {
            tab: tab_id.clone(),
            state: TabState::Loading,
        });
        drop(tabs);
        self.dispatcher.flush();
        Ok(url)
    }

    /// Engine committed a main-frame navigation: update tab meta and record
    /// the visit in global history (upsert with visit_count, FR-004).
    pub fn notify_committed(&self, tab_id: &TabId, url: &str) -> Result<(), CoreError> {
        {
            let mut tabs = self.lock_tabs_pub();
            let tab = tabs.tab_mut(tab_id)?;
            tab.url = url.to_string();
            tab.history.push(url.to_string(), String::new());
        }
        record_visit(self, url, None);
        self.dispatcher.emit(Event::TabMetaChanged {
            tab: tab_id.clone(),
            url: Some(url.to_string()),
            title: None,
        });
        self.dispatcher.flush();
        self.note_session_changed();
        Ok(())
    }

    /// Engine finished loading (or failed). Updates state/title.
    pub fn notify_loaded(
        &self,
        tab_id: &TabId,
        title: Option<&str>,
        success: bool,
    ) -> Result<(), CoreError> {
        let url = {
            let mut tabs = self.lock_tabs_pub();
            if tabs.tab(tab_id)?.state != TabState::Active {
                tabs.transition(tab_id, TabState::Active)?;
            }
            let tab = tabs.tab_mut(tab_id)?;
            if let Some(t) = title {
                tab.title = t.to_string();
            }
            tab.url.clone()
        };
        if success && let Some(t) = title {
            record_visit(self, &url, Some(t));
        }
        self.dispatcher.emit(Event::TabStateChanged {
            tab: tab_id.clone(),
            state: TabState::Active,
        });
        self.dispatcher.emit(Event::TabMetaChanged {
            tab: tab_id.clone(),
            url: Some(url),
            title: title.map(str::to_string),
        });
        self.dispatcher.flush();
        Ok(())
    }
}

/// Upsert a visit into global history via the writer thread (never blocks UI).
fn record_visit(core: &BlazeCore, url: &str, title: Option<&str>) {
    // Skip non-web schemes (error pages, about:blank).
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return;
    }
    let url = url.to_string();
    let title = title.map(str::to_string);
    core.profile().writer().submit(move |conn| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let result = conn.execute(
            "INSERT INTO history (url, title, visit_count, last_visit)
             VALUES (?1, COALESCE(?2, ''), 1, ?3)
             ON CONFLICT(url) DO UPDATE SET
               visit_count = visit_count + (CASE WHEN excluded.title = '' THEN 1 ELSE 0 END),
               title = CASE WHEN excluded.title != '' THEN excluded.title ELSE title END,
               last_visit = excluded.last_visit",
            params![url, title, now],
        );
        if let Err(e) = result {
            tracing::error!(error = %e, "failed to record history visit");
        }
    });
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
    fn navigate_resolves_and_marks_loading() {
        let (core, _d) = core();
        let window = core.create_window(crate::tabs::Rect::default());
        let tab = core.create_tab(&window, None).expect("tab");
        let url = core.navigate(&tab, "example.com").expect("resolves");
        assert_eq!(url, "https://example.com/");
        core.with_tabs(|t| {
            assert_eq!(t.tab(&tab).expect("tab").state, TabState::Loading);
        });
    }

    #[test]
    fn dangerous_input_is_blocked() {
        let (core, _d) = core();
        let window = core.create_window(crate::tabs::Rect::default());
        let tab = core.create_tab(&window, None).expect("tab");
        assert!(core.navigate(&tab, "javascript:alert(1)").is_err());
    }

    #[test]
    fn committed_visit_lands_in_history_with_upsert() {
        let (core, _d) = core();
        let window = core.create_window(crate::tabs::Rect::default());
        let tab = core.create_tab(&window, None).expect("tab");
        core.notify_committed(&tab, "https://example.com/")
            .expect("commit");
        core.notify_committed(&tab, "https://example.com/")
            .expect("commit");
        core.notify_loaded(&tab, Some("Example"), true)
            .expect("loaded");
        core.profile().flush();

        let conn = core.profile().read_conn().expect("read conn");
        let (count, title): (i64, String) = conn
            .query_row(
                "SELECT visit_count, title FROM history WHERE url = ?1",
                params!["https://example.com/"],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("history row");
        assert_eq!(count, 2);
        assert_eq!(title, "Example");
    }
}
