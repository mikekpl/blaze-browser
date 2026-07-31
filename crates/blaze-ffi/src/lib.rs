//! UniFFI boundary between the Rust core and platform shells
//! (contracts/core-api.md). Every export is panic-isolated: a caught panic
//! becomes `BlazeError::Internal`, never a process abort (plan: crash-resilience §1).
//!
//! Events cross the boundary as the JSON encoding of `Vec<Event>` so shells
//! can ignore unknown variants (forward compatibility guarantee).

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, OnceLock};

use blaze_adblock::cosmetic::ScriptletManifest;
use blaze_adblock::{AdblockEngine, ShieldCounters};
use blaze_core::events::{Event, EventSink};
use blaze_core::tabs::Rect as CoreRect;
use blaze_core::{BlazeCore, CoreError};

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum BlazeError {
    #[error("invalid argument: {msg}")]
    InvalidArgument { msg: String },
    #[error("{kind} not found: {id}")]
    NotFound { kind: String, id: String },
    #[error("storage error: {msg}")]
    Storage { msg: String },
    #[error("network error: {msg}")]
    Network { msg: String },
    #[error("internal error: {msg}")]
    Internal { msg: String },
}

impl From<CoreError> for BlazeError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::InvalidArgument(msg) => Self::InvalidArgument { msg },
            CoreError::NotFound(kind, id) => Self::NotFound {
                kind: kind.to_owned(),
                id,
            },
            CoreError::Storage(err) => Self::Storage {
                msg: err.to_string(),
            },
            CoreError::Network(msg) => Self::Network { msg },
            CoreError::Internal(msg) => Self::Internal { msg },
        }
    }
}

#[derive(uniffi::Record)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Shell-implemented listener; receives JSON-encoded `Vec<Event>` batches.
#[uniffi::export(callback_interface)]
pub trait EventListener: Send + Sync {
    fn on_events(&self, events_json: String);
}

struct ListenerSink(Box<dyn EventListener>);

impl EventSink for ListenerSink {
    fn on_events(&self, events: Vec<Event>) {
        match serde_json::to_string(&events) {
            Ok(json) => self.0.on_events(json),
            Err(e) => {
                // Serialization of our own types failing is a bug; drop the batch, never panic.
                eprintln!("blaze-ffi: event serialization failed: {e}");
            }
        }
    }
}

/// Panic isolation for every FFI entry point.
fn guard<T>(f: impl FnOnce() -> Result<T, BlazeError>) -> Result<T, BlazeError> {
    match std::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(res) => res,
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_owned());
            Err(BlazeError::Internal { msg })
        }
    }
}

#[derive(uniffi::Object)]
pub struct BlazeCoreHandle {
    inner: BlazeCore,
    adblock: OnceLock<AdblockEngine>,
    webkit_rules: OnceLock<String>,
    scriptlets: OnceLock<ScriptletManifest>,
    shields: ShieldCounters,
}

#[uniffi::export]
impl BlazeCoreHandle {
    /// Open (creating if needed) the profile at `profile_dir`; empty string
    /// selects the platform default directory.
    #[uniffi::constructor]
    pub fn new(
        profile_dir: String,
        listener: Box<dyn EventListener>,
    ) -> Result<Arc<Self>, BlazeError> {
        guard(|| {
            let dir = if profile_dir.is_empty() {
                blaze_storage::paths::default_profile_dir().ok_or(BlazeError::Storage {
                    msg: "no platform data directory".into(),
                })?
            } else {
                std::path::PathBuf::from(profile_dir)
            };
            let core = BlazeCore::new(&dir, Box::new(ListenerSink(listener)))?;
            let _ = core.load_persisted_closed_tabs(); // best-effort ring reload
            Ok(Arc::new(Self {
                inner: core,
                adblock: OnceLock::new(),
                webkit_rules: OnceLock::new(),
                scriptlets: OnceLock::new(),
                shields: ShieldCounters::default(),
            }))
        })
    }

    /// Flush all state; call before process exit.
    pub fn shutdown(&self) {
        let _ = guard(|| {
            self.inner.shutdown();
            Ok(())
        });
    }

    // ---- Windows & tabs ----

    pub fn create_window(&self, frame: Rect) -> Result<String, BlazeError> {
        guard(|| {
            Ok(self.inner.create_window(CoreRect {
                x: frame.x,
                y: frame.y,
                w: frame.w,
                h: frame.h,
            }))
        })
    }

    pub fn close_window(&self, window_id: String) -> Result<(), BlazeError> {
        guard(|| Ok(self.inner.close_window(&window_id)?))
    }

    pub fn create_tab(&self, window_id: String, url: Option<String>) -> Result<String, BlazeError> {
        guard(|| Ok(self.inner.create_tab(&window_id, url)?))
    }

    pub fn close_tab(&self, tab_id: String) -> Result<(), BlazeError> {
        guard(|| Ok(self.inner.close_tab(&tab_id)?))
    }

    pub fn activate_tab(&self, tab_id: String) -> Result<(), BlazeError> {
        guard(|| Ok(self.inner.activate_tab(&tab_id)?))
    }

    pub fn move_tab(
        &self,
        tab_id: String,
        to_window: String,
        position: u32,
    ) -> Result<(), BlazeError> {
        guard(|| Ok(self.inner.move_tab(&tab_id, &to_window, position)?))
    }

    pub fn reorder_tab(&self, tab_id: String, position: u32) -> Result<(), BlazeError> {
        guard(|| Ok(self.inner.reorder_tab(&tab_id, position)?))
    }

    pub fn set_pinned(&self, tab_id: String, pinned: bool) -> Result<(), BlazeError> {
        guard(|| Ok(self.inner.set_pinned(&tab_id, pinned)?))
    }

    pub fn set_muted(&self, tab_id: String, muted: bool) -> Result<(), BlazeError> {
        guard(|| Ok(self.inner.set_muted(&tab_id, muted)?))
    }

    /// Engine reported media playback started/stopped (T041, FR-021).
    /// Emits `TabAudioChanged` only when the effective state changes.
    pub fn notify_media_playback(&self, tab_id: String, playing: bool) -> Result<(), BlazeError> {
        guard(|| Ok(self.inner.notify_media_playback(&tab_id, playing)?))
    }

    // ---- Session (US2: T035/T036/T038) ----

    /// Full window/tab tree as JSON for shell rendering (tab strips).
    pub fn get_state_json(&self) -> Result<String, BlazeError> {
        guard(|| {
            let value = self.inner.with_tabs(|tabs| {
                let windows: Vec<serde_json::Value> = tabs
                    .windows()
                    .map(|w| {
                        let tab_list: Vec<serde_json::Value> = w
                            .tab_ids
                            .iter()
                            .filter_map(|id| tabs.tab(id).ok())
                            .map(|t| {
                                serde_json::json!({
                                    "id": t.id,
                                    "url": t.url,
                                    "title": t.title,
                                    "pinned": t.pinned,
                                    "state": t.state,
                                    "audio_state": t.audio_state,
                                    "can_go_back": t.history.can_go_back(),
                                    "can_go_forward": t.history.can_go_forward(),
                                })
                            })
                            .collect();
                        serde_json::json!({
                            "id": w.id,
                            "active_tab_id": w.active_tab_id,
                            "tabs": tab_list,
                        })
                    })
                    .collect();
                serde_json::json!({ "windows": windows })
            });
            Ok(value.to_string())
        })
    }

    /// Restore the previous session; returns restored window ids (FR-018).
    pub fn restore_previous_session(&self) -> Result<Vec<String>, BlazeError> {
        guard(|| Ok(self.inner.restore_previous_session()?))
    }

    /// Reopen the most recently closed tab, preferring `window_id` (FR-015).
    /// Returns the new tab id, or None when the ring is empty.
    pub fn reopen_closed_tab(&self, window_id: String) -> Result<Option<String>, BlazeError> {
        guard(|| Ok(self.inner.reopen_closed_tab(&window_id)?))
    }

    /// Suspend LRU background tabs beyond `tab_suspend.max_active` (FR-016).
    /// Returns the suspended tab ids so the shell can drop their web views.
    pub fn suspend_lru_tabs(&self) -> Result<Vec<String>, BlazeError> {
        guard(|| Ok(self.inner.suspend_lru_tabs()?))
    }

    // ---- Downloads (US4, FR-020..023) ----

    /// Start downloading `url` into the configured download directory.
    /// Returns the download id; progress arrives as throttled (>=250ms)
    /// `DownloadUpdated` events.
    pub fn start_download(
        &self,
        url: String,
        suggested_name: Option<String>,
    ) -> Result<String, BlazeError> {
        guard(|| Ok(self.inner.start_download(&url, suggested_name.as_deref())?))
    }

    /// Pause an active download (FR-022).
    pub fn pause_download(&self, download_id: String) -> Result<(), BlazeError> {
        guard(|| Ok(self.inner.pause_download(&download_id)?))
    }

    /// Resume a paused or interrupted download with `If-Range` validation (FR-023).
    pub fn resume_download(&self, download_id: String) -> Result<(), BlazeError> {
        guard(|| Ok(self.inner.resume_download(&download_id)?))
    }

    /// Cancel a non-terminal download; the partial file is deleted.
    pub fn cancel_download(&self, download_id: String) -> Result<(), BlazeError> {
        guard(|| Ok(self.inner.cancel_download(&download_id)?))
    }

    /// All downloads (newest first) as a JSON array of download rows.
    pub fn list_downloads_json(&self) -> Result<String, BlazeError> {
        guard(|| {
            let rows = self.inner.list_downloads()?;
            serde_json::to_string(&rows).map_err(|e| BlazeError::Internal { msg: e.to_string() })
        })
    }

    /// Launch-time recovery (T048): mark downloads orphaned by a crash/quit
    /// as interrupted and auto-resume validatable ones. Returns resumed ids.
    pub fn resume_interrupted_downloads(&self) -> Result<Vec<String>, BlazeError> {
        guard(|| Ok(self.inner.resume_interrupted_downloads()?))
    }

    // ---- Bookmarks (US5, FR-024..026) ----

    /// Add a bookmark leaf; returns the new bookmark id.
    pub fn add_bookmark(
        &self,
        parent_id: Option<i64>,
        title: String,
        url: String,
    ) -> Result<i64, BlazeError> {
        guard(|| Ok(self.inner.add_bookmark(parent_id, &title, &url)?))
    }

    /// Create a bookmark folder; returns the new folder id.
    pub fn create_folder(&self, parent_id: Option<i64>, title: String) -> Result<i64, BlazeError> {
        guard(|| Ok(self.inner.create_bookmark_folder(parent_id, &title)?))
    }

    /// Edit title and/or URL of a bookmark or folder.
    pub fn edit_bookmark(
        &self,
        id: i64,
        title: Option<String>,
        url: Option<String>,
    ) -> Result<(), BlazeError> {
        guard(|| {
            Ok(self
                .inner
                .edit_bookmark(id, title.as_deref(), url.as_deref())?)
        })
    }

    /// Delete a bookmark, or a folder with all its descendants.
    pub fn delete_bookmark(&self, id: i64) -> Result<(), BlazeError> {
        guard(|| Ok(self.inner.delete_bookmark(id)?))
    }

    /// Move/reorder a node; cycle-creating moves are rejected.
    pub fn move_bookmark(
        &self,
        id: i64,
        new_parent: Option<i64>,
        position: i64,
    ) -> Result<(), BlazeError> {
        guard(|| Ok(self.inner.move_bookmark(id, new_parent, position)?))
    }

    /// Nested bookmarks tree as JSON (`[{id,is_folder,title,url,children}]`).
    pub fn bookmarks_tree_json(&self) -> Result<String, BlazeError> {
        guard(|| {
            let tree = self.inner.bookmarks_tree()?;
            serde_json::to_string(&tree).map_err(|e| BlazeError::Internal { msg: e.to_string() })
        })
    }

    /// Title/URL search results as a JSON array of flat bookmark rows.
    pub fn search_bookmarks_json(&self, query: String) -> Result<String, BlazeError> {
        guard(|| {
            let hits = self.inner.search_bookmarks(&query)?;
            serde_json::to_string(&hits).map_err(|e| BlazeError::Internal { msg: e.to_string() })
        })
    }

    // ---- Navigation (resolution done in core; engine drive lands in Phase 3) ----

    /// Resolve address-bar input for `tab_id`, mark the tab loading, and reset
    /// its shield counters. Dangerous input emits `NavigationBlocked` + errors.
    pub fn navigate(&self, tab_id: String, input: String) -> Result<String, BlazeError> {
        guard(|| {
            let url = self.inner.navigate(&tab_id, &input)?;
            self.shields.reset(&tab_id);
            Ok(url)
        })
    }

    /// Engine committed a main-frame navigation (history recording, FR-004).
    pub fn notify_committed(&self, tab_id: String, url: String) -> Result<(), BlazeError> {
        guard(|| Ok(self.inner.notify_committed(&tab_id, &url)?))
    }

    /// Engine finished (or failed) a load.
    pub fn notify_loaded(
        &self,
        tab_id: String,
        title: Option<String>,
        success: bool,
    ) -> Result<(), BlazeError> {
        guard(|| {
            Ok(self
                .inner
                .notify_loaded(&tab_id, title.as_deref(), success)?)
        })
    }

    /// Resolve address-bar input (URL vs search per current settings) and
    /// return the URL the shell's engine backend should load.
    pub fn resolve_navigation(&self, input: String) -> Result<String, BlazeError> {
        guard(|| {
            let template = self
                .inner
                .get_settings()
                .search_engine
                .template()
                .to_owned();
            let url = blaze_net::url::resolve_input(&input, &template)
                .map_err(|e| BlazeError::InvalidArgument { msg: e.to_string() })?;
            Ok(url.to_string())
        })
    }

    // ---- Ad blocking (US1) ----

    /// Load (or build) the filter engine. `filters_dir` holds bundled `*.txt`
    /// lists; `scriptlets_manifest_path` may be empty. Uses a serialized DAT
    /// cache in the profile dir so warm launches skip the parse (B1/B2).
    /// Call from a background queue — the cold build parses ~150k rules.
    pub fn init_adblock(
        &self,
        filters_dir: String,
        scriptlets_manifest_path: String,
    ) -> Result<u32, BlazeError> {
        guard(|| {
            let lists = read_filter_lists(std::path::Path::new(&filters_dir))?;
            let count = lists.len() as u32;
            let fingerprint = fingerprint(&lists);
            let cache = self.inner.profile().dir().join("adblock.dat");
            let meta = self.inner.profile().dir().join("adblock.dat.fp");

            let cached_ok =
                std::fs::read_to_string(&meta).is_ok_and(|m| m.trim() == fingerprint.to_string());
            let engine = if cached_ok {
                std::fs::read(&cache)
                    .ok()
                    .and_then(|dat| AdblockEngine::from_cache(&dat).ok())
            } else {
                None
            };
            let engine = match engine {
                Some(e) => e,
                None => {
                    let e = AdblockEngine::from_lists(lists.iter().map(String::as_str));
                    // Persist the DAT off the critical path (best effort).
                    let _ = std::fs::write(&cache, e.to_cache());
                    let _ = std::fs::write(&meta, fingerprint.to_string());
                    e
                }
            };
            let _ = self.adblock.set(engine);

            if !scriptlets_manifest_path.is_empty() {
                let _ =
                    self.scriptlets
                        .set(ScriptletManifest::load_from_path(std::path::Path::new(
                            &scriptlets_manifest_path,
                        )));
            }

            // Compile (or reuse) the WebKit content-blocker JSON.
            let rules_cache = self.inner.profile().dir().join("webkit_rules.json");
            let rules_meta = self.inner.profile().dir().join("webkit_rules.fp");
            let rules_ok = std::fs::read_to_string(&rules_meta)
                .is_ok_and(|m| m.trim() == fingerprint.to_string());
            let json = if rules_ok {
                std::fs::read_to_string(&rules_cache).ok()
            } else {
                None
            };
            let json = match json {
                Some(j) => j,
                None => {
                    let (j, skipped) = blaze_adblock::webkit_rules::compile_webkit_json(
                        lists.iter().map(String::as_str),
                    )
                    .map_err(|e| BlazeError::Internal { msg: e.to_string() })?;
                    tracing::info!(skipped, "compiled WebKit content-blocker rules");
                    let _ = std::fs::write(&rules_cache, &j);
                    let _ = std::fs::write(&rules_meta, fingerprint.to_string());
                    j
                }
            };
            let _ = self.webkit_rules.set(json);

            self.inner.dispatcher().emit(Event::FilterListsUpdated {
                lists: vec!["bundled".into()],
            });
            self.inner.dispatcher().flush();
            Ok(count)
        })
    }

    /// WKContentRuleList JSON compiled from the loaded lists.
    pub fn compiled_rules_for_webkit(&self) -> Result<String, BlazeError> {
        guard(|| {
            self.webkit_rules
                .get()
                .cloned()
                .ok_or(BlazeError::Internal {
                    msg: "adblock not initialized".into(),
                })
        })
    }

    /// Cosmetic payload for a page: `{"css": "...", "scriptlets": [{...}]}`.
    /// Empty payload when blocking is disabled for the site (FR-010).
    pub fn cosmetics_for(&self, url: String) -> Result<String, BlazeError> {
        guard(|| {
            let host = host_of(&url);
            if !self.blocking_enabled(&host)? {
                return Ok(r#"{"css":"","scriptlets":[]}"#.into());
            }
            let engine = self.adblock.get().ok_or(BlazeError::Internal {
                msg: "adblock not initialized".into(),
            })?;
            let css = blaze_adblock::cosmetic::cosmetic_css(engine, &url);
            let mut scriptlets: Vec<blaze_engine::UserScript> = self
                .scriptlets
                .get()
                .map(|m| m.for_host(&host))
                .unwrap_or_default();
            if let Some(s) = blaze_adblock::cosmetic::engine_scriptlets(engine, &url) {
                scriptlets.push(s);
            }
            serde_json::to_string(&serde_json::json!({ "css": css, "scriptlets": scriptlets }))
                .map_err(|e| BlazeError::Internal { msg: e.to_string() })
        })
    }

    /// Should this request be blocked? Updates shield counters and emits
    /// `ShieldStatsChanged` when blocked. `kind` is a resource-type string
    /// ("document", "script", "image", "stylesheet", "font", "media",
    /// "xhr", "websocket", "other").
    pub fn classify_request(
        &self,
        tab_id: String,
        url: String,
        source_url: String,
        kind: String,
    ) -> Result<bool, BlazeError> {
        guard(|| {
            let source_host = host_of(&source_url);
            if !self.blocking_enabled(&source_host)? {
                return Ok(false);
            }
            let engine = self.adblock.get().ok_or(BlazeError::Internal {
                msg: "adblock not initialized".into(),
            })?;
            let decision = engine
                .check(&url, &source_url, resource_kind(&kind))
                .map_err(|e| BlazeError::InvalidArgument { msg: e.to_string() })?;
            if decision.block {
                let stats = self
                    .shields
                    .record(&tab_id, blaze_adblock::shields::classify_block(&url));
                self.inner.dispatcher().emit(Event::ShieldStatsChanged {
                    tab: tab_id,
                    ads_blocked: stats.ads_blocked as u32,
                    trackers_blocked: stats.trackers_blocked as u32,
                    enabled: true,
                });
                self.inner.dispatcher().flush();
            }
            Ok(decision.block)
        })
    }

    /// Per-site blocking toggle (persists across restarts, FR-010).
    pub fn set_site_exception(
        &self,
        host: String,
        blocking_enabled: bool,
    ) -> Result<(), BlazeError> {
        guard(|| {
            blaze_storage::exceptions::set_site_exception(
                self.inner.profile(),
                &host,
                blocking_enabled,
            );
            Ok(())
        })
    }

    /// Is blocking enabled for `host` (default true)?
    pub fn is_blocking_enabled(&self, host: String) -> Result<bool, BlazeError> {
        guard(|| self.blocking_enabled(&host))
    }

    /// Current shield counters for a tab as JSON.
    pub fn get_shield_stats(&self, tab_id: String) -> Result<String, BlazeError> {
        guard(|| {
            serde_json::to_string(&self.shields.get(&tab_id))
                .map_err(|e| BlazeError::Internal { msg: e.to_string() })
        })
    }

    // ---- Settings ----

    /// Current settings as JSON (contracts/storage-schema.md settings document).
    pub fn get_settings_json(&self) -> Result<String, BlazeError> {
        guard(|| {
            serde_json::to_string(&self.inner.get_settings())
                .map_err(|e| BlazeError::Internal { msg: e.to_string() })
        })
    }

    /// Apply a partial settings document (unknown keys ignored, invalid values
    /// keep their previous value); emits `SettingsChanged`.
    pub fn update_settings_json(&self, patch_json: String) -> Result<String, BlazeError> {
        guard(|| {
            let updated = self.inner.update_settings(|s| {
                let merged = merge_settings_patch(s, &patch_json);
                *s = merged;
            })?;
            serde_json::to_string(&updated).map_err(|e| BlazeError::Internal { msg: e.to_string() })
        })
    }
}

impl BlazeCoreHandle {
    /// Site-exception lookup on a read connection (WAL: safe concurrent read).
    fn blocking_enabled(&self, host: &str) -> Result<bool, BlazeError> {
        if host.is_empty() {
            return Ok(true);
        }
        let conn = self
            .inner
            .profile()
            .read_conn()
            .map_err(|e| BlazeError::Storage { msg: e.to_string() })?;
        blaze_storage::exceptions::blocking_enabled_for(&conn, host)
            .map_err(|e| BlazeError::Storage { msg: e.to_string() })
    }
}

/// All `*.txt` filter lists in `dir`, sorted for stable fingerprints.
fn read_filter_lists(dir: &std::path::Path) -> Result<Vec<String>, BlazeError> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| BlazeError::Storage {
            msg: format!("filters dir: {e}"),
        })?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "txt"))
        .collect();
    paths.sort();
    let mut lists = Vec::with_capacity(paths.len());
    for p in paths {
        lists.push(
            std::fs::read_to_string(&p).map_err(|e| BlazeError::Storage {
                msg: format!("{}: {e}", p.display()),
            })?,
        );
    }
    Ok(lists)
}

/// Deterministic content fingerprint for the DAT / rules caches.
fn fingerprint(lists: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    for l in lists {
        l.hash(&mut h);
    }
    h.finish()
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default()
}

fn resource_kind(kind: &str) -> blaze_engine::ResourceKind {
    use blaze_engine::ResourceKind as K;
    match kind {
        "document" => K::Document,
        "script" => K::Script,
        "image" => K::Image,
        "stylesheet" => K::Stylesheet,
        "font" => K::Font,
        "media" => K::Media,
        "xhr" => K::Xhr,
        "websocket" => K::Websocket,
        _ => K::Other,
    }
}

/// Merge a JSON patch onto existing settings key-by-key: a key is applied only
/// if the resulting document still deserializes, so invalid values keep the
/// previous value (not the default) and unknown keys are ignored.
fn merge_settings_patch(
    current: &blaze_storage::settings::Settings,
    patch_json: &str,
) -> blaze_storage::settings::Settings {
    let Ok(serde_json::Value::Object(patch)) = serde_json::from_str(patch_json) else {
        return current.clone();
    };
    let mut result = current.clone();
    for (key, value) in patch {
        let Ok(mut doc) = serde_json::to_value(&result) else {
            continue;
        };
        if let Some(map) = doc.as_object_mut() {
            map.insert(key, value);
            if let Ok(candidate) = serde_json::from_value::<blaze_storage::settings::Settings>(doc)
            {
                result = candidate;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NullListener;
    impl EventListener for NullListener {
        fn on_events(&self, _events_json: String) {}
    }

    #[test]
    fn construct_and_drive_core_through_ffi_surface() {
        let dir = tempfile::tempdir().unwrap();
        let handle = BlazeCoreHandle::new(
            dir.path().to_string_lossy().into_owned(),
            Box::new(NullListener),
        )
        .unwrap();
        let w = handle
            .create_window(Rect {
                x: 0.0,
                y: 0.0,
                w: 800.0,
                h: 600.0,
            })
            .unwrap();
        let t = handle
            .create_tab(w.clone(), Some("https://example.com".into()))
            .unwrap();
        handle.set_pinned(t.clone(), true).unwrap();
        handle.close_tab(t).unwrap();
        handle.shutdown();
    }

    #[test]
    fn resolve_navigation_uses_configured_search_engine() {
        let dir = tempfile::tempdir().unwrap();
        let handle = BlazeCoreHandle::new(
            dir.path().to_string_lossy().into_owned(),
            Box::new(NullListener),
        )
        .unwrap();
        let url = handle.resolve_navigation("hello world".into()).unwrap();
        assert!(
            url.contains("google.com/search"),
            "default engine is Google: {url}"
        );
        let direct = handle.resolve_navigation("example.com".into()).unwrap();
        assert_eq!(direct, "https://example.com/");
        assert!(
            handle
                .resolve_navigation("javascript:alert(1)".into())
                .is_err()
        );
    }

    #[test]
    fn adblock_pipeline_through_ffi() {
        let dir = tempfile::tempdir().unwrap();
        let filters = dir.path().join("filters");
        std::fs::create_dir_all(&filters).unwrap();
        std::fs::write(filters.join("test.txt"), "||ads.example.com^\n").unwrap();

        let handle = BlazeCoreHandle::new(
            dir.path().to_string_lossy().into_owned(),
            Box::new(NullListener),
        )
        .unwrap();
        let loaded = handle
            .init_adblock(filters.to_string_lossy().into_owned(), String::new())
            .unwrap();
        assert_eq!(loaded, 1);

        // webkit rules compiled
        let json = handle.compiled_rules_for_webkit().unwrap();
        assert!(json.starts_with('['));

        // blocked request counts into shields
        let w = handle
            .create_window(Rect {
                x: 0.0,
                y: 0.0,
                w: 800.0,
                h: 600.0,
            })
            .unwrap();
        let t = handle.create_tab(w, None).unwrap();
        assert!(
            handle
                .classify_request(
                    t.clone(),
                    "https://ads.example.com/x.js".into(),
                    "https://news.site/".into(),
                    "script".into()
                )
                .unwrap()
        );
        let stats = handle.get_shield_stats(t.clone()).unwrap();
        assert!(stats.contains(r#""ads_blocked":1"#));

        // site exception disables blocking (flush writer, then re-check)
        handle
            .set_site_exception("news.site".into(), false)
            .unwrap();
        handle.inner.profile().flush();
        assert!(!handle.is_blocking_enabled("news.site".into()).unwrap());
        assert!(
            !handle
                .classify_request(
                    t,
                    "https://ads.example.com/x.js".into(),
                    "https://news.site/".into(),
                    "script".into()
                )
                .unwrap()
        );
        handle.shutdown();

        // warm start: engine restored from DAT cache
        let handle2 = BlazeCoreHandle::new(
            dir.path().to_string_lossy().into_owned(),
            Box::new(NullListener),
        )
        .unwrap();
        handle2
            .init_adblock(filters.to_string_lossy().into_owned(), String::new())
            .unwrap();
        assert!(
            handle2
                .compiled_rules_for_webkit()
                .unwrap()
                .starts_with('[')
        );
    }

    #[test]
    fn settings_patch_merges_and_survives_invalid_values() {
        let dir = tempfile::tempdir().unwrap();
        let handle = BlazeCoreHandle::new(
            dir.path().to_string_lossy().into_owned(),
            Box::new(NullListener),
        )
        .unwrap();
        let updated = handle
            .update_settings_json(r#"{"theme":"dark","bogus_key":1}"#.into())
            .unwrap();
        assert!(updated.contains(r#""theme": "dark""#) || updated.contains(r#""theme":"dark""#));
        // invalid value keeps previous (dark), not default
        let updated2 = handle
            .update_settings_json(r#"{"theme":"neon"}"#.into())
            .unwrap();
        assert!(updated2.contains("dark"));
    }
}
