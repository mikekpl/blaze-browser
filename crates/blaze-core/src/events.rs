//! Batched core→shell event channel (contracts/core-api.md). Events serialize
//! to JSON so shells can ignore unknown variants (forward compatibility).

use serde::Serialize;

use blaze_engine::AudioState;
use blaze_storage::settings::Settings;

use crate::tabs::{TabId, TabState, WindowId};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    TabCreated {
        tab: TabId,
        window: WindowId,
        index: u32,
    },
    TabClosed {
        tab: TabId,
        window: WindowId,
    },
    WindowClosed {
        window: WindowId,
    },
    TabStateChanged {
        tab: TabId,
        state: TabState,
    },
    TabMetaChanged {
        tab: TabId,
        url: Option<String>,
        title: Option<String>,
    },
    TabAudioChanged {
        tab: TabId,
        audio_state: AudioState,
    },
    NavigationBlocked {
        tab: TabId,
        url: String,
        reason: String,
    },
    ShieldStatsChanged {
        tab: TabId,
        ads_blocked: u32,
        trackers_blocked: u32,
        enabled: bool,
    },
    DownloadUpdated {
        download_id: String,
        state: String,
        received_bytes: u64,
        total_bytes: Option<u64>,
    },
    SettingsChanged {
        settings: Settings,
    },
    FilterListsUpdated {
        lists: Vec<String>,
    },
    SessionSnapshotWritten {
        snapshot_id: i64,
    },
}

/// Receives batched events; implemented by the FFI layer / test doubles.
pub trait EventSink: Send + Sync {
    fn on_events(&self, events: Vec<Event>);
}

/// Collects events during one command and flushes them as a single batch (B4).
pub struct Dispatcher {
    sink: Box<dyn EventSink>,
    queue: std::sync::Mutex<Vec<Event>>,
}

impl Dispatcher {
    pub fn new(sink: Box<dyn EventSink>) -> Self {
        Self {
            sink,
            queue: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn emit(&self, event: Event) {
        self.queue.lock().expect("event queue poisoned").push(event);
    }

    pub fn flush(&self) {
        let batch: Vec<Event> =
            std::mem::take(&mut *self.queue.lock().expect("event queue poisoned"));
        if !batch.is_empty() {
            self.sink.on_events(batch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Capture(Arc<Mutex<Vec<usize>>>);
    impl EventSink for Capture {
        fn on_events(&self, events: Vec<Event>) {
            self.0.lock().unwrap().push(events.len());
        }
    }

    #[test]
    fn events_batch_per_flush() {
        let sizes = Arc::new(Mutex::new(Vec::new()));
        let d = Dispatcher::new(Box::new(Capture(sizes.clone())));
        d.emit(Event::WindowClosed { window: "w".into() });
        d.emit(Event::TabClosed {
            tab: "t".into(),
            window: "w".into(),
        });
        d.flush();
        d.flush(); // empty flush emits nothing
        assert_eq!(*sizes.lock().unwrap(), vec![2]);
    }

    #[test]
    fn events_serialize_with_type_tag() {
        let json = serde_json::to_string(&Event::TabStateChanged {
            tab: "t1".into(),
            state: TabState::Loading,
        })
        .unwrap();
        assert!(json.contains(r#""type":"tab_state_changed""#));
        assert!(json.contains(r#""state":"loading""#));
    }
}
