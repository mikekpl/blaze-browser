//! T040: Durability under hard kill (SIGKILL). A child process builds a
//! session, flushes, then hangs; the parent kill -9s it and verifies the
//! snapshot restores from disk (FR-018, Constitution crash-resistance).

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use blaze_core::BlazeCore;
use blaze_core::events::{Event, EventSink};
use blaze_core::tabs::Rect;

struct NullSink;
impl EventSink for NullSink {
    fn on_events(&self, _: Vec<Event>) {}
}

const CHILD_ENV: &str = "BLAZE_RECOVERY_DIR";

/// Child role: build a session, flush to disk, print READY, hang until killed.
#[test]
fn child_session_writer() {
    let Ok(dir) = std::env::var(CHILD_ENV) else {
        return; // not in child mode; nothing to do
    };
    let core = BlazeCore::new(std::path::Path::new(&dir), Box::new(NullSink)).expect("core");
    let w = core.create_window(Rect::default());
    core.create_tab(&w, Some("https://survivor.example/a".into()))
        .expect("tab a");
    let t = core
        .create_tab(&w, Some("https://survivor.example/b".into()))
        .expect("tab b");
    core.set_pinned(&t, true).expect("pin");
    core.write_snapshot_now();
    core.profile().flush(); // durable before READY
    println!("READY");
    // Hang forever; parent sends SIGKILL. No shutdown() runs — that's the point.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

#[test]
fn session_survives_sigkill() {
    if std::env::var(CHILD_ENV).is_ok() {
        return; // we are the child; skip the parent role
    }
    let dir = tempfile::tempdir().expect("tempdir");

    let mut child = Command::new(std::env::current_exe().expect("test exe"))
        .args(["child_session_writer", "--exact", "--nocapture"])
        .env(CHILD_ENV, dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child");

    // Wait for the child to confirm durable state, then SIGKILL it.
    let stdout = child.stdout.take().expect("child stdout");
    let mut ready = false;
    for line in BufReader::new(stdout).lines() {
        if line.expect("read child").contains("READY") {
            ready = true;
            break;
        }
    }
    assert!(ready, "child never reached READY");
    child.kill().expect("SIGKILL child"); // std kill = SIGKILL on unix
    child.wait().expect("reap child");

    // Recovery: a fresh core must restore the exact session from disk.
    let core = BlazeCore::new(dir.path(), Box::new(NullSink)).expect("core reopens after kill");
    let restored = core.restore_previous_session().expect("restore");
    assert_eq!(restored.len(), 1, "one window restored");
    core.with_tabs(|tabs| {
        let w = tabs.window(&restored[0]).expect("window");
        assert_eq!(w.tab_ids.len(), 3, "newtab + two created tabs");
        let urls: Vec<_> = w
            .tab_ids
            .iter()
            .map(|id| tabs.tab(id).expect("tab").url.clone())
            .collect();
        assert!(urls.contains(&"https://survivor.example/a".to_string()));
        assert!(urls.contains(&"https://survivor.example/b".to_string()));
        let pinned = w
            .tab_ids
            .iter()
            .filter(|id| tabs.tab(id).expect("tab").pinned)
            .count();
        assert_eq!(pinned, 1, "pin state survives");
        tabs.check_invariants()
            .expect("invariants hold after recovery");
    });
}
