//! Servo backend for the `WebEngine` trait (research.md R1, promotion policy in
//! contracts/web-engine-trait.md). T070 scaffold: the full state machine —
//! navigation staging, history, blocking hooks, suspension, event queue — is
//! implemented against a pluggable `PageSource`. The `servo` feature will swap
//! the simulated source for the real libservo embedding without touching the
//! trait surface; golden parity tests (T071) pin the event contract either way.

use std::collections::VecDeque;

use blaze_engine::{
    AudioState, BlockingArtifacts, EngineEvent, ResourceKind, UserScript, WebEngine,
};
use url::Url;

/// What a page load produces, independent of the real engine.
#[derive(Debug, Clone, Default)]
pub struct SimulatedPage {
    pub title: String,
    /// Subresources fetched during load; each is offered to the blocker.
    pub subresources: Vec<(Url, ResourceKind)>,
    /// `window.open` attempts; always denied (US1-AC5) and reported.
    pub popups: Vec<Url>,
}

/// Supplies page content for a URL. The libservo embedding replaces this;
/// tests use a fixed corpus.
pub trait PageSource: Send {
    fn load(&self, url: &Url) -> Option<SimulatedPage>;
}

/// Network-blocking hook: `true` means the request must not reach the network.
/// The real embedding calls the shared `blaze-adblock` engine here.
pub type Blocker = Box<dyn Fn(&Url, ResourceKind) -> bool + Send>;

enum LoadStage {
    Started,
    Committed,
}

struct PendingLoad {
    url: Url,
    stage: LoadStage,
    /// History navigation must not clear the forward stack.
    from_history: bool,
}

/// One instance per tab view (contract). Loads advance one stage per `pump`
/// call so `stop` has a real window to cancel in — mirroring async engines.
pub struct ServoEngine {
    source: Box<dyn PageSource>,
    blocker: Blocker,
    events: VecDeque<EngineEvent>,
    pending: Option<PendingLoad>,
    current: Option<Url>,
    back_stack: Vec<Url>,
    forward_stack: Vec<Url>,
    title: Option<String>,
    artifacts: BlockingArtifacts,
    muted: bool,
    suspended: bool,
}

impl ServoEngine {
    pub fn new(source: Box<dyn PageSource>, blocker: Blocker) -> Self {
        Self {
            source,
            blocker,
            events: VecDeque::new(),
            pending: None,
            current: None,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            title: None,
            artifacts: BlockingArtifacts::default(),
            muted: false,
            suspended: false,
        }
    }

    /// Drain queued events (the embedding delivers these on its emitter).
    pub fn drain_events(&mut self) -> Vec<EngineEvent> {
        self.events.drain(..).collect()
    }

    /// Advance the in-flight load by one stage. Returns `true` while loading.
    pub fn pump(&mut self) -> bool {
        let Some(load) = self.pending.take() else {
            return false;
        };
        match load.stage {
            LoadStage::Started => {
                let Some(page) = self.source.load(&load.url) else {
                    // Unreachable page: finish unsuccessfully (invariant 1).
                    self.events.push_back(EngineEvent::NavigationFinished {
                        url: load.url,
                        success: false,
                    });
                    return false;
                };
                // Blocking hook runs before commit — nothing blocked ever loads.
                if self.artifacts.network_rules.native_matcher {
                    for (sub, kind) in &page.subresources {
                        if (self.blocker)(sub, *kind) {
                            self.events.push_back(EngineEvent::RequestBlocked {
                                url: sub.clone(),
                                kind: *kind,
                            });
                        }
                    }
                }
                for popup in &page.popups {
                    self.events
                        .push_back(EngineEvent::PopupBlocked { url: popup.clone() });
                }
                self.events.push_back(EngineEvent::NavigationCommitted {
                    url: load.url.clone(),
                });
                self.title = Some(page.title.clone());
                self.events.push_back(EngineEvent::TitleChanged(page.title));
                self.pending = Some(PendingLoad {
                    stage: LoadStage::Committed,
                    ..load
                });
                true
            }
            LoadStage::Committed => {
                if let Some(prev) = self.current.take() {
                    self.back_stack.push(prev);
                }
                if !load.from_history {
                    self.forward_stack.clear();
                }
                self.current = Some(load.url.clone());
                self.events.push_back(EngineEvent::NavigationFinished {
                    url: load.url,
                    success: true,
                });
                false
            }
        }
    }

    /// Run the pending load to completion (convenience for synchronous callers).
    pub fn pump_to_idle(&mut self) {
        while self.pump() {}
    }

    fn begin_load(&mut self, url: Url, from_history: bool) {
        self.suspended = false;
        self.events
            .push_back(EngineEvent::NavigationStarted { url: url.clone() });
        self.pending = Some(PendingLoad {
            url,
            stage: LoadStage::Started,
            from_history,
        });
    }

    pub fn current_url(&self) -> Option<&Url> {
        self.current.as_ref()
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    pub fn scriptlets(&self) -> &[UserScript] {
        &self.artifacts.scriptlets
    }
}

impl WebEngine for ServoEngine {
    fn navigate(&mut self, url: &Url) {
        self.begin_load(url.clone(), false);
    }

    fn go_back(&mut self) {
        // History moves are applied to the stacks up front; the load replays
        // the page (real engine restores from its session history).
        let Some(target) = self.back_stack.pop() else {
            return;
        };
        if let Some(cur) = self.current.take() {
            self.forward_stack.push(cur);
        }
        // begin_load pushes current back — it is already None here.
        self.begin_load(target, true);
    }

    fn go_forward(&mut self) {
        let Some(target) = self.forward_stack.pop() else {
            return;
        };
        if let Some(cur) = self.current.take() {
            self.back_stack.push(cur);
        }
        self.begin_load(target, true);
    }

    fn reload(&mut self) {
        if let Some(url) = self.current.clone() {
            // A reload replaces the current entry, not the history stacks.
            self.current = None;
            self.begin_load(url, true);
        }
    }

    fn stop(&mut self) {
        // Cancels the in-flight load: no NavigationFinished (invariant 1 carve-out).
        self.pending = None;
    }

    fn apply_blocking(&mut self, artifacts: BlockingArtifacts) {
        self.artifacts = artifacts;
    }

    fn set_muted(&mut self, muted: bool) {
        if self.muted != muted {
            self.muted = muted;
            self.events
                .push_back(EngineEvent::AudioStateChanged(if muted {
                    AudioState::Muted
                } else {
                    AudioState::Silent
                }));
        }
    }

    fn suspend(&mut self) {
        // Drop the live view; core retains url/title/history (FR-016).
        self.pending = None;
        self.title = None;
        self.suspended = true;
    }

    fn resume(&mut self, url: &Url) {
        self.begin_load(url.clone(), true);
    }

    fn poll_title(&self) -> Option<String> {
        self.title.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Corpus(HashMap<String, SimulatedPage>);

    impl PageSource for Corpus {
        fn load(&self, url: &Url) -> Option<SimulatedPage> {
            self.0.get(url.as_str()).cloned()
        }
    }

    fn engine() -> ServoEngine {
        let mut pages = HashMap::new();
        pages.insert(
            "https://a.example/".to_owned(),
            SimulatedPage {
                title: "Page A".into(),
                subresources: vec![(
                    Url::parse("https://ads.example/banner.js").unwrap(),
                    ResourceKind::Script,
                )],
                popups: vec![],
            },
        );
        pages.insert(
            "https://b.example/".to_owned(),
            SimulatedPage {
                title: "Page B".into(),
                ..Default::default()
            },
        );
        let mut e = ServoEngine::new(
            Box::new(Corpus(pages)),
            Box::new(|url, _| url.host_str() == Some("ads.example")),
        );
        e.apply_blocking(BlockingArtifacts {
            network_rules: blaze_engine::NetworkRules {
                webkit_json: None,
                native_matcher: true,
            },
            ..Default::default()
        });
        e
    }

    fn nav(e: &mut ServoEngine, url: &str) {
        e.navigate(&Url::parse(url).unwrap());
        e.pump_to_idle();
    }

    #[test]
    fn navigate_blocks_matched_requests_and_finishes_once() {
        let mut e = engine();
        nav(&mut e, "https://a.example/");
        let events = e.drain_events();
        let finished = events
            .iter()
            .filter(|ev| matches!(ev, EngineEvent::NavigationFinished { .. }))
            .count();
        assert_eq!(finished, 1);
        assert!(
            events
                .iter()
                .any(|ev| matches!(ev, EngineEvent::RequestBlocked { .. }))
        );
        assert_eq!(e.poll_title().as_deref(), Some("Page A"));
    }

    #[test]
    fn stop_cancels_without_finished_event() {
        let mut e = engine();
        e.navigate(&Url::parse("https://a.example/").unwrap());
        e.pump(); // committed but not finished
        e.stop();
        e.pump_to_idle();
        let events = e.drain_events();
        assert!(
            !events
                .iter()
                .any(|ev| matches!(ev, EngineEvent::NavigationFinished { .. }))
        );
    }

    #[test]
    fn history_and_suspend_resume() {
        let mut e = engine();
        nav(&mut e, "https://a.example/");
        nav(&mut e, "https://b.example/");
        e.go_back();
        e.pump_to_idle();
        assert_eq!(e.current_url().unwrap().as_str(), "https://a.example/");
        e.go_forward();
        e.pump_to_idle();
        assert_eq!(e.current_url().unwrap().as_str(), "https://b.example/");

        e.suspend();
        assert!(e.is_suspended());
        assert_eq!(e.poll_title(), None);
        e.resume(&Url::parse("https://b.example/").unwrap());
        e.pump_to_idle();
        assert!(!e.is_suspended());
        assert_eq!(e.poll_title().as_deref(), Some("Page B"));
    }
}
