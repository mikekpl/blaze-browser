//! Shields: per-tab blocking counters and per-site enable/disable (T024).
//!
//! Counters are in-memory per tab (reset on navigation); the per-site
//! exception list persists via `blaze-storage::exceptions`.

use std::collections::HashMap;
use std::sync::Mutex;

/// Live blocking counters for one page load (FR-009).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct ShieldStats {
    pub ads_blocked: u64,
    pub trackers_blocked: u64,
}

/// What was blocked, for counter attribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Ad,
    Tracker,
}

/// Per-tab shield counters. All methods take `&self` (internal locking) so
/// this can sit behind the core facade without extra synchronization.
#[derive(Debug, Default)]
pub struct ShieldCounters {
    per_tab: Mutex<HashMap<String, ShieldStats>>,
}

impl ShieldCounters {
    /// Record one blocked request; returns the updated stats for the tab.
    pub fn record(&self, tab_id: &str, kind: BlockKind) -> ShieldStats {
        let mut map = self.per_tab.lock().expect("shield lock");
        let stats = map.entry(tab_id.to_string()).or_default();
        match kind {
            BlockKind::Ad => stats.ads_blocked += 1,
            BlockKind::Tracker => stats.trackers_blocked += 1,
        }
        *stats
    }

    /// Reset on main-frame navigation (counters are per page load).
    pub fn reset(&self, tab_id: &str) {
        self.per_tab
            .lock()
            .expect("shield lock")
            .insert(tab_id.to_string(), ShieldStats::default());
    }

    pub fn get(&self, tab_id: &str) -> ShieldStats {
        self.per_tab
            .lock()
            .expect("shield lock")
            .get(tab_id)
            .copied()
            .unwrap_or_default()
    }

    pub fn drop_tab(&self, tab_id: &str) {
        self.per_tab.lock().expect("shield lock").remove(tab_id);
    }
}

/// Classify a blocked URL as ad vs tracker (heuristic: EasyPrivacy-style
/// tracker hosts vs everything else). Good enough for counter display.
pub fn classify_block(url: &str) -> BlockKind {
    const TRACKER_HINTS: &[&str] = &[
        "analytics",
        "telemetry",
        "tracking",
        "tracker",
        "metrics",
        "pixel",
        "beacon",
        "stats",
        "collect",
    ];
    let lower = url.to_ascii_lowercase();
    if TRACKER_HINTS.iter().any(|h| lower.contains(h)) {
        BlockKind::Tracker
    } else {
        BlockKind::Ad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_accumulate_and_reset() {
        let counters = ShieldCounters::default();
        counters.record("t1", BlockKind::Ad);
        counters.record("t1", BlockKind::Ad);
        counters.record("t1", BlockKind::Tracker);
        assert_eq!(
            counters.get("t1"),
            ShieldStats {
                ads_blocked: 2,
                trackers_blocked: 1
            }
        );
        counters.reset("t1");
        assert_eq!(counters.get("t1"), ShieldStats::default());
    }

    #[test]
    fn tabs_are_independent() {
        let counters = ShieldCounters::default();
        counters.record("t1", BlockKind::Ad);
        assert_eq!(counters.get("t2"), ShieldStats::default());
    }

    #[test]
    fn classification_heuristic() {
        assert_eq!(
            classify_block("https://www.google-analytics.com/collect"),
            BlockKind::Tracker
        );
        assert_eq!(
            classify_block("https://ads.example.com/banner.js"),
            BlockKind::Ad
        );
    }
}
