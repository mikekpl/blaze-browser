//! Fault-tolerant `settings.json` (contracts/storage-schema.md): unknown keys
//! ignored, invalid values fall back per-key, missing file ⇒ all defaults.
//! Never fails hard (fuzz target).

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    #[default]
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SearchEngine {
    // Google per spec clarification (user-changeable).
    #[default]
    Google,
    Duckduckgo,
    Brave,
    #[serde(untagged)]
    Custom {
        custom: String,
    },
}

impl SearchEngine {
    /// `%s` is replaced with the percent-encoded query.
    pub fn template(&self) -> &str {
        match self {
            SearchEngine::Google => "https://www.google.com/search?q=%s",
            SearchEngine::Duckduckgo => "https://duckduckgo.com/?q=%s",
            SearchEngine::Brave => "https://search.brave.com/search?q=%s",
            SearchEngine::Custom { custom } => custom,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionRestore {
    #[default]
    Restore,
    Fresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EngineChoice {
    #[default]
    Webkit,
    Servo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabSuspend {
    pub max_active: u32,
    pub idle_minutes: u32,
}

impl Default for TabSuspend {
    fn default() -> Self {
        Self {
            max_active: 20,
            idle_minutes: 30,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub theme: Theme,
    pub search_engine: SearchEngine,
    pub download_dir: String,
    pub session_restore: SessionRestore,
    pub bookmarks_bar_visible: bool,
    pub tab_suspend: TabSuspend,
    pub adblock_enabled: bool,
    pub engine: EngineChoice,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            theme: Theme::default(),
            search_engine: SearchEngine::default(),
            download_dir: "~/Downloads".to_owned(),
            session_restore: SessionRestore::default(),
            bookmarks_bar_visible: true,
            tab_suspend: TabSuspend::default(),
            adblock_enabled: true,
            engine: EngineChoice::default(),
        }
    }
}

impl Settings {
    /// Parse with per-key fallback: each invalid value reverts to its default
    /// while valid siblings are kept.
    pub fn from_json_lossy(raw: &str) -> Self {
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw) else {
            return Self::default();
        };
        let d = Self::default();
        fn get<T: for<'de> Deserialize<'de>>(
            map: &serde_json::Map<String, Value>,
            key: &str,
            default: T,
        ) -> T {
            map.get(key)
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(default)
        }
        Self {
            version: get(&map, "version", d.version),
            theme: get(&map, "theme", d.theme),
            search_engine: get(&map, "search_engine", d.search_engine),
            download_dir: get(&map, "download_dir", d.download_dir),
            session_restore: get(&map, "session_restore", d.session_restore),
            bookmarks_bar_visible: get(&map, "bookmarks_bar_visible", d.bookmarks_bar_visible),
            tab_suspend: get(&map, "tab_suspend", d.tab_suspend),
            adblock_enabled: get(&map, "adblock_enabled", d.adblock_enabled),
            engine: get(&map, "engine", d.engine),
        }
    }

    /// Missing/corrupt file ⇒ defaults; never errors.
    pub fn load_lossy(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(raw) => Self::from_json_lossy(&raw),
            Err(_) => Self::default(),
        }
    }

    /// Atomic save: temp file + rename.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_contract() {
        let s = Settings::default();
        assert_eq!(s.theme, Theme::System);
        assert_eq!(s.search_engine, SearchEngine::Google);
        assert_eq!(s.session_restore, SessionRestore::Restore);
        assert_eq!(s.engine, EngineChoice::Webkit);
        assert!(s.adblock_enabled);
        assert!(s.bookmarks_bar_visible);
    }

    #[test]
    fn garbage_input_yields_defaults() {
        assert_eq!(
            Settings::from_json_lossy("not json at all"),
            Settings::default()
        );
        assert_eq!(Settings::from_json_lossy("[1,2,3]"), Settings::default());
        assert_eq!(Settings::from_json_lossy(""), Settings::default());
    }

    #[test]
    fn invalid_value_falls_back_per_key_keeping_valid_siblings() {
        let s = Settings::from_json_lossy(r#"{"theme":"neon","adblock_enabled":false}"#);
        assert_eq!(s.theme, Theme::System); // invalid → default
        assert!(!s.adblock_enabled); // valid sibling kept
    }

    #[test]
    fn unknown_keys_ignored() {
        let s = Settings::from_json_lossy(r#"{"theme":"dark","future_key":42}"#);
        assert_eq!(s.theme, Theme::Dark);
    }

    #[test]
    fn custom_search_engine_round_trips() {
        let s = Settings {
            search_engine: SearchEngine::Custom {
                custom: "https://s.example/?q=%s".into(),
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(Settings::from_json_lossy(&json), s);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let s = Settings {
            theme: Theme::Dark,
            ..Default::default()
        };
        s.save(&path).unwrap();
        assert_eq!(Settings::load_lossy(&path), s);
    }

    #[test]
    fn missing_file_is_defaults() {
        assert_eq!(
            Settings::load_lossy(Path::new("/nonexistent/settings.json")),
            Settings::default()
        );
    }
}
