//! Wrapper around Brave's adblock-rust engine (research.md R2).
//!
//! Startup-path discipline (B1/B2): the parsed engine is expensive to build
//! from raw filter text (~150k rules), so a serialized DAT cache is used —
//! build once, then `serialize()` and reload the DAT on subsequent launches.

use adblock::Engine;
use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request;
use blaze_engine::ResourceKind;

#[derive(Debug, thiserror::Error)]
pub enum AdblockError {
    #[error("engine cache is stale or corrupt: {0}")]
    Cache(String),
    #[error("invalid request: {0}")]
    Request(String),
}

/// Verdict for one network request.
#[derive(Debug, Clone, Default)]
pub struct BlockDecision {
    pub block: bool,
    /// Matched an exception rule (informational).
    pub exception: bool,
    /// Redirect resource (uBO-style) instead of a plain block, if any.
    pub redirect: Option<String>,
}

/// Cosmetic payload for one page load.
#[derive(Debug, Clone, Default)]
pub struct CosmeticPayload {
    /// CSS selectors to hide (`display: none !important`).
    pub hide_selectors: Vec<String>,
    /// Scriptlet JavaScript to inject at document start.
    pub injected_script: String,
}

/// Thread-safe (all `&self`) network + cosmetic filter engine.
pub struct AdblockEngine {
    inner: Engine,
}

impl AdblockEngine {
    /// Build from raw filter-list texts (slow path — run off the UI thread).
    pub fn from_lists<'a>(lists: impl IntoIterator<Item = &'a str>) -> Self {
        let mut set = FilterSet::new(false);
        for text in lists {
            set.add_filter_list(text.to_string(), ParseOptions::default());
        }
        Self {
            inner: Engine::new_with_filter_set(set),
        }
    }

    /// Fast path: restore a previously serialized engine (DAT cache).
    pub fn from_cache(dat: &[u8]) -> Result<Self, AdblockError> {
        let mut inner = Engine::default();
        inner
            .deserialize(dat)
            .map_err(|e| AdblockError::Cache(format!("{e:?}")))?;
        Ok(Self { inner })
    }

    /// Serialize for the DAT cache (write via the storage writer thread).
    pub fn to_cache(&self) -> Vec<u8> {
        self.inner.serialize()
    }

    /// Classify one network request (Servo backend + WebKit fallback hook).
    pub fn check(
        &self,
        url: &str,
        source_url: &str,
        kind: ResourceKind,
    ) -> Result<BlockDecision, AdblockError> {
        let request = Request::new(url, source_url, request_type(kind), "GET")
            .map_err(|e| AdblockError::Request(format!("{e:?}")))?;
        let result = self.inner.check_network_request(&request);
        Ok(BlockDecision {
            block: result.should_block(),
            exception: result.exception.is_some(),
            redirect: result.redirect,
        })
    }

    /// Cosmetic resources (hide selectors + scriptlets) for a page URL.
    pub fn cosmetics_for(&self, url: &str) -> CosmeticPayload {
        let resources = self.inner.url_cosmetic_resources(url);
        let mut hide_selectors: Vec<String> = resources.hide_selectors.into_iter().collect();
        hide_selectors.sort();
        CosmeticPayload {
            hide_selectors,
            injected_script: resources.injected_script,
        }
    }

    /// Generic class/id selectors for in-page mutation observation.
    pub fn hidden_class_id_selectors(
        &self,
        classes: &[String],
        ids: &[String],
        exceptions: &std::collections::HashSet<String>,
    ) -> Vec<String> {
        self.inner
            .hidden_class_id_selectors(classes, ids, exceptions)
    }
}

/// Map the engine-neutral resource kind onto adblock-rust request types.
fn request_type(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Document => "document",
        ResourceKind::Script => "script",
        ResourceKind::Image => "image",
        ResourceKind::Stylesheet => "stylesheet",
        ResourceKind::Font => "font",
        ResourceKind::Media => "media",
        ResourceKind::Xhr => "xmlhttprequest",
        ResourceKind::Websocket => "websocket",
        ResourceKind::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "||ads.example.com^\nnews.site##.banner-ad\n@@||ads.example.com/allowed^\n";

    fn engine() -> AdblockEngine {
        AdblockEngine::from_lists([LIST])
    }

    #[test]
    fn blocks_matching_request() {
        let d = engine()
            .check(
                "https://ads.example.com/pixel.js",
                "https://news.site/",
                ResourceKind::Script,
            )
            .expect("valid request");
        assert!(d.block);
    }

    #[test]
    fn allows_exception_rule() {
        let d = engine()
            .check(
                "https://ads.example.com/allowed/x.js",
                "https://news.site/",
                ResourceKind::Script,
            )
            .expect("valid request");
        assert!(!d.block);
        assert!(d.exception);
    }

    #[test]
    fn allows_unmatched_request() {
        let d = engine()
            .check(
                "https://cdn.example.com/app.js",
                "https://news.site/",
                ResourceKind::Script,
            )
            .expect("valid request");
        assert!(!d.block);
    }

    #[test]
    fn cache_round_trip_preserves_verdicts() {
        let original = engine();
        let restored = AdblockEngine::from_cache(&original.to_cache()).expect("cache loads");
        let d = restored
            .check(
                "https://ads.example.com/pixel.js",
                "https://news.site/",
                ResourceKind::Script,
            )
            .expect("valid request");
        assert!(d.block);
    }

    #[test]
    fn corrupt_cache_is_rejected_not_panicking() {
        assert!(AdblockEngine::from_cache(b"not a dat file").is_err());
    }

    #[test]
    fn cosmetic_selectors_present() {
        let payload = engine().cosmetics_for("https://news.site/article");
        assert!(payload.hide_selectors.iter().any(|s| s == ".banner-ad"));
    }
}
