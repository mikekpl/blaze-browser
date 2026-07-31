//! WebEngine abstraction: the swap seam between Servo (target engine) and
//! WKWebView (v1 fallback). `blaze-core` depends only on this contract.
//! See specs/001-lightweight-adblock-browser/contracts/web-engine-trait.md.

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioState {
    Silent,
    Audible,
    Muted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Document,
    Script,
    Image,
    Stylesheet,
    Font,
    Media,
    Xhr,
    Websocket,
    Other,
}

/// A JavaScript snippet injected at document start (cosmetic filtering / scriptlets).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserScript {
    pub name: String,
    pub source: String,
    pub main_frame_only: bool,
}

/// Engine-specific network blocking payloads compiled from one filter source.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkRules {
    /// WKContentRuleList JSON for the WebKit backend.
    pub webkit_json: Option<String>,
    /// Whether the native (in-process adblock engine) matcher is active — Servo backend.
    pub native_matcher: bool,
}

/// Everything a backend needs to enforce blocking for one page load.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlockingArtifacts {
    pub network_rules: NetworkRules,
    pub cosmetic_css: String,
    pub scriptlets: Vec<UserScript>,
}

/// Events emitted by a backend for one tab view. Implementations must be
/// panic-isolated: internal failure surfaces as `Crashed`, never a process abort.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    NavigationStarted {
        url: Url,
    },
    NavigationCommitted {
        url: Url,
    },
    NavigationFinished {
        url: Url,
        success: bool,
    },
    TitleChanged(String),
    FaviconChanged(Vec<u8>),
    AudioStateChanged(AudioState),
    RequestBlocked {
        url: Url,
        kind: ResourceKind,
    },
    DownloadRequested {
        url: Url,
        suggested_name: Option<String>,
    },
    FullscreenRequested(bool),
    PopupBlocked {
        url: Url,
    },
    Crashed {
        reason: String,
    },
}

/// One instance per tab view.
pub trait WebEngine: Send {
    fn navigate(&mut self, url: &Url);
    fn go_back(&mut self);
    fn go_forward(&mut self);
    fn reload(&mut self);
    fn stop(&mut self);

    /// Inject ad-block artifacts before/at document start.
    fn apply_blocking(&mut self, artifacts: BlockingArtifacts);

    fn set_muted(&mut self, muted: bool);

    /// Drop the live view, retaining nothing but what core already holds
    /// (url/title/history). Used for inactive-tab suspension (FR-016).
    fn suspend(&mut self);
    fn resume(&mut self, url: &Url);

    fn poll_title(&self) -> Option<String>;
}
