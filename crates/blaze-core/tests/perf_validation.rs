//! T069: memory/startup validation harness (SC-002 memory discipline,
//! SC-003 cold start). Measures the Rust core's contribution with a
//! 10-page workload; renderer memory is WebKit's (out of process) and is
//! validated manually per quickstart.md. Run with `--nocapture` to see the
//! measured numbers; generous ceilings catch order-of-magnitude regressions
//! without flaking on CI noise.

use std::time::Instant;

use blaze_core::BlazeCore;
use blaze_core::events::{Event, EventSink};
use blaze_core::tabs::Rect;

struct NullSink;
impl EventSink for NullSink {
    fn on_events(&self, _: Vec<Event>) {}
}

/// Resident set size of this process in bytes (macOS/Linux `ps`).
fn rss_bytes() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        * 1024
}

const WORKLOAD: [&str; 10] = [
    "https://news.example/",
    "https://video.example/watch?v=1",
    "https://shop.example/cart",
    "https://mail.example/inbox",
    "https://docs.example/edit",
    "https://social.example/feed",
    "https://wiki.example/article",
    "https://code.example/repo",
    "https://maps.example/route",
    "https://music.example/playlist",
];

/// SC-003: the core must not be the reason startup feels slow. Cold-open of
/// a populated profile (history + bookmarks + snapshot) stays under 500ms —
/// the 1.5s user-facing budget belongs almost entirely to the shell/WebKit.
#[test]
fn cold_start_of_populated_profile_under_budget() {
    let dir = tempfile::tempdir().unwrap();

    // Populate: a 10-tab session, bookmarks, and history.
    {
        let core = BlazeCore::new(dir.path(), Box::new(NullSink)).unwrap();
        let w = core.create_window(Rect::default());
        for url in WORKLOAD {
            core.create_tab(&w, Some(url.into())).unwrap();
        }
        for (i, url) in WORKLOAD.iter().enumerate() {
            core.add_bookmark(None, &format!("Page {i}"), url).unwrap();
        }
        core.write_snapshot_now();
        core.shutdown();
    }

    let start = Instant::now();
    let core = BlazeCore::new(dir.path(), Box::new(NullSink)).unwrap();
    let restored = core.restore_previous_session().unwrap();
    let elapsed = start.elapsed();

    eprintln!("core cold start + session restore: {elapsed:?} (budget 500ms)");
    assert!(!restored.is_empty(), "session must restore");
    assert!(
        elapsed.as_millis() < 500,
        "core cold start {elapsed:?} blows the 500ms core budget (SC-003)"
    );
}

/// SC-002: core-side memory for a 10-page workload. The core holds tab state,
/// session snapshots, and download/bookmark mirrors — all small; page memory
/// lives in WebKit's separate web-content processes.
#[test]
fn ten_page_workload_core_memory_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let before = rss_bytes();

    let core = BlazeCore::new(dir.path(), Box::new(NullSink)).unwrap();
    let w = core.create_window(Rect::default());
    for url in WORKLOAD {
        let tab = core.create_tab(&w, Some(url.into())).unwrap();
        core.notify_committed(&tab, url).unwrap();
        core.notify_loaded(&tab, Some("loaded page title"), true)
            .unwrap();
    }
    core.write_snapshot_now();
    core.profile().flush();

    let after = rss_bytes();
    let growth = after.saturating_sub(before);
    eprintln!(
        "core RSS growth for 10-tab workload: {:.1} MiB (ceiling 64 MiB)",
        growth as f64 / (1024.0 * 1024.0)
    );
    // Order-of-magnitude gate: the whole core (SQLite pages, tab state,
    // writer queues) must stay tiny next to any renderer.
    assert!(
        growth < 64 * 1024 * 1024,
        "core grew {growth} bytes for 10 tabs — memory discipline regression (SC-002)"
    );
}
