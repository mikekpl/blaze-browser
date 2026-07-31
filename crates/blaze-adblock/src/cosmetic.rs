//! Per-site cosmetic filtering and scriptlet selection (T023, T044).
//!
//! Cosmetic hiding is delivered as CSS injected at document start; scriptlets
//! are small JS snippets (uBO-style) selected per site from
//! `assets/scriptlets/manifest.json`.

use crate::engine::AdblockEngine;
use blaze_engine::UserScript;
use serde::Deserialize;

/// Max selectors per generated CSS rule block (keeps rules parseable fast).
const SELECTORS_PER_BLOCK: usize = 1_000;

/// One entry in `assets/scriptlets/manifest.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct ScriptletEntry {
    pub name: String,
    /// Host suffixes this scriptlet applies to (e.g. "youtube.com");
    /// `"*"` applies everywhere.
    pub hosts: Vec<String>,
    /// JS source, injected at document start in the page world.
    #[serde(default)]
    pub source: String,
    /// Alternative to `source`: a .js file next to the manifest (T044).
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub main_frame_only: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ScriptletManifest {
    #[serde(default)]
    pub scriptlets: Vec<ScriptletEntry>,
}

impl ScriptletManifest {
    /// Parse the manifest; malformed manifests degrade to "no scriptlets"
    /// (never a startup failure — crash-resilience point 4).
    pub fn from_json_lossy(json: &str) -> Self {
        serde_json::from_str(json).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "scriptlet manifest malformed; ignoring");
            Self::default()
        })
    }

    /// Load from disk, resolving `file` entries relative to the manifest
    /// directory. Unreadable files drop just that scriptlet.
    pub fn load_from_path(manifest_path: &std::path::Path) -> Self {
        let Ok(json) = std::fs::read_to_string(manifest_path) else {
            return Self::default();
        };
        let mut manifest = Self::from_json_lossy(&json);
        let dir = manifest_path.parent().unwrap_or(std::path::Path::new("."));
        manifest.scriptlets.retain_mut(|entry| {
            let Some(file) = &entry.file else { return true };
            match std::fs::read_to_string(dir.join(file)) {
                Ok(source) => {
                    entry.source = source;
                    true
                }
                Err(e) => {
                    tracing::warn!(file, error = %e, "scriptlet file unreadable; skipping");
                    false
                }
            }
        });
        manifest
    }

    /// Scriptlets applicable to `host` (suffix match on registrable domain labels).
    pub fn for_host(&self, host: &str) -> Vec<UserScript> {
        self.scriptlets
            .iter()
            .filter(|s| s.hosts.iter().any(|h| host_matches(host, h)))
            .map(|s| UserScript {
                name: s.name.clone(),
                source: s.source.clone(),
                main_frame_only: s.main_frame_only,
            })
            .collect()
    }
}

/// `host` equals `pattern`, is a subdomain of it, or `pattern` is `"*"`.
fn host_matches(host: &str, pattern: &str) -> bool {
    pattern == "*"
        || host == pattern
        || host.strip_suffix(pattern).is_some_and(|p| p.ends_with('.'))
}

/// Generate the page CSS hiding ad elements for `url`, chunked into blocks.
pub fn cosmetic_css(engine: &AdblockEngine, url: &str) -> String {
    let payload = engine.cosmetics_for(url);
    let mut css = String::new();
    for chunk in payload.hide_selectors.chunks(SELECTORS_PER_BLOCK) {
        css.push_str(&chunk.join(","));
        css.push_str("{display:none !important;}\n");
    }
    css
}

/// Engine-provided scriptlet JS (from `+js(...)` filter rules) for `url`.
pub fn engine_scriptlets(engine: &AdblockEngine, url: &str) -> Option<UserScript> {
    let payload = engine.cosmetics_for(url);
    if payload.injected_script.is_empty() {
        return None;
    }
    Some(UserScript {
        name: "engine-scriptlets".into(),
        source: payload.injected_script,
        main_frame_only: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_selects_by_host_suffix() {
        let manifest = ScriptletManifest::from_json_lossy(
            r#"{"scriptlets":[{"name":"yt","hosts":["youtube.com"],"source":"/*js*/"}]}"#,
        );
        assert_eq!(manifest.for_host("www.youtube.com").len(), 1);
        assert_eq!(manifest.for_host("youtube.com").len(), 1);
        assert!(manifest.for_host("notyoutube.com").is_empty());
        assert!(manifest.for_host("example.org").is_empty());
    }

    #[test]
    fn malformed_manifest_degrades_to_empty() {
        let manifest = ScriptletManifest::from_json_lossy("{nope");
        assert!(manifest.scriptlets.is_empty());
    }

    #[test]
    fn file_entries_resolve_relative_to_manifest_and_wildcard_matches() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("s.js"), "/*stealth*/").expect("js");
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{"scriptlets":[
                {"name":"stealth","hosts":["*"],"file":"s.js"},
                {"name":"gone","hosts":["*"],"file":"missing.js"}
            ]}"#,
        )
        .expect("manifest");
        let manifest = ScriptletManifest::load_from_path(&dir.path().join("manifest.json"));
        assert_eq!(manifest.scriptlets.len(), 1, "unreadable file dropped");
        let scripts = manifest.for_host("anything.example");
        assert_eq!(scripts.len(), 1, "wildcard applies everywhere");
        assert_eq!(scripts[0].source, "/*stealth*/");
    }

    #[test]
    fn css_generated_for_matching_site() {
        let engine =
            AdblockEngine::from_lists(["news.site##.banner-ad\nnews.site##.tracking-pixel\n"]);
        let css = cosmetic_css(&engine, "https://news.site/");
        assert!(css.contains(".banner-ad"));
        assert!(css.ends_with("{display:none !important;}\n"));
    }
}
