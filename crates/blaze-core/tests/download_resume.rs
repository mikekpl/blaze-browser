//! T052: end-to-end resume-after-interrupt against a local HTTP Range server.
//! Exercises start -> interrupt (early EOF) -> launch-time detection ->
//! ranged resume with If-Range validation -> completed file on disk.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use blaze_core::BlazeCore;
use blaze_core::events::{Event, EventSink};

const ETAG: &str = "\"blaze-test-v1\"";
const PAYLOAD_LEN: usize = 256 * 1024;

fn payload() -> Vec<u8> {
    (0..PAYLOAD_LEN).map(|i| (i % 251) as u8).collect()
}

struct RangeServer {
    port: u16,
    /// Bytes to send for the next response before dropping the connection.
    truncate_next: Arc<AtomicUsize>,
    /// Range header values seen, one entry per request ("" when absent).
    ranges_seen: Arc<Mutex<Vec<String>>>,
}

impl RangeServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let port = listener.local_addr().unwrap().port();
        let truncate_next = Arc::new(AtomicUsize::new(usize::MAX));
        let ranges_seen = Arc::new(Mutex::new(Vec::new()));
        let (truncate, seen) = (Arc::clone(&truncate_next), Arc::clone(&ranges_seen));

        std::thread::spawn(move || {
            let body = payload();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut range = String::new();
                let mut if_range = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    let lower = line.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("range:") {
                        range = v.trim().to_string();
                    }
                    if lower.starts_with("if-range:") {
                        if_range = line["if-range:".len()..].trim().to_string();
                    }
                }
                seen.lock().unwrap().push(range.clone());

                let from = range
                    .strip_prefix("bytes=")
                    .and_then(|r| r.strip_suffix('-'))
                    .and_then(|n| n.parse::<usize>().ok())
                    .filter(|_| if_range.is_empty() || if_range == ETAG)
                    .unwrap_or(0);
                let slice = &body[from.min(body.len())..];

                let header = if from > 0 {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nETag: {ETAG}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {from}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len() - 1,
                        body.len(),
                        slice.len()
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nETag: {ETAG}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        slice.len()
                    )
                };

                let cap = truncate.swap(usize::MAX, Ordering::SeqCst);
                let n = slice.len().min(cap);
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&slice[..n]);
                let _ = stream.flush();
                // Dropping the stream early simulates a network interruption.
            }
        });

        Self {
            port,
            truncate_next,
            ranges_seen,
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/file.bin", self.port)
    }
}

struct Recorder(Arc<Mutex<Vec<Event>>>);
impl EventSink for Recorder {
    fn on_events(&self, events: Vec<Event>) {
        self.0.lock().unwrap().extend(events);
    }
}

#[test]
fn download_resumes_after_interrupt_with_range_validation() {
    let server = RangeServer::start();
    let profile_dir = tempfile::tempdir().unwrap();
    let download_dir = tempfile::tempdir().unwrap();

    let events = Arc::new(Mutex::new(Vec::new()));
    let core = BlazeCore::new(profile_dir.path(), Box::new(Recorder(Arc::clone(&events)))).unwrap();
    core.update_settings(|s| {
        s.download_dir = download_dir.path().to_string_lossy().into_owned();
    })
    .unwrap();

    // 1. First attempt: server drops the connection after 64 KiB.
    server.truncate_next.store(64 * 1024, Ordering::SeqCst);
    let id = core
        .start_download(&server.url(), Some("file.bin"))
        .unwrap();
    core.wait_for_download(&id);

    let row = core
        .list_downloads()
        .unwrap()
        .into_iter()
        .find(|r| r.id == id)
        .unwrap();
    assert_eq!(row.state, "interrupted");
    assert_eq!(row.etag.as_deref(), Some(ETAG));
    assert!(row.received_bytes > 0 && row.received_bytes < PAYLOAD_LEN as u64);
    let part = std::path::PathBuf::from(format!("{}.part", row.dest_path));
    assert!(part.exists(), "partial file kept after interruption");

    // 2. Launch-time recovery resumes it automatically (T048).
    let resumed = core.resume_interrupted_downloads().unwrap();
    assert_eq!(resumed, vec![id.clone()]);
    core.wait_for_download(&id);

    let row = core
        .list_downloads()
        .unwrap()
        .into_iter()
        .find(|r| r.id == id)
        .unwrap();
    assert_eq!(row.state, "completed");
    assert_eq!(row.received_bytes, PAYLOAD_LEN as u64);
    assert!(row.completed_at.is_some());

    // 3. Bytes on disk are exact and the .part staging file is gone.
    let got = std::fs::read(&row.dest_path).unwrap();
    assert_eq!(got, payload());
    assert!(!part.exists());

    // 4. The second request was a ranged continuation, not a restart.
    let ranges = server.ranges_seen.lock().unwrap();
    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[0], "");
    assert!(ranges[1].starts_with("bytes="), "resume used Range header");

    // 5. Events surfaced a terminal `completed` update for the UI.
    let final_state = events
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::DownloadUpdated {
                download_id, state, ..
            } if *download_id == id => Some(state.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(final_state, "completed");

    core.shutdown();
}

#[test]
fn cancel_deletes_partial_and_is_terminal() {
    let server = RangeServer::start();
    let profile_dir = tempfile::tempdir().unwrap();
    let download_dir = tempfile::tempdir().unwrap();

    let core = BlazeCore::new(
        profile_dir.path(),
        Box::new(Recorder(Arc::new(Mutex::new(Vec::new())))),
    )
    .unwrap();
    core.update_settings(|s| {
        s.download_dir = download_dir.path().to_string_lossy().into_owned();
    })
    .unwrap();

    // Interrupt so the download lands in a resumable, non-running state.
    server.truncate_next.store(16 * 1024, Ordering::SeqCst);
    let id = core
        .start_download(&server.url(), Some("file.bin"))
        .unwrap();
    core.wait_for_download(&id);

    core.cancel_download(&id).unwrap();
    let row = core
        .list_downloads()
        .unwrap()
        .into_iter()
        .find(|r| r.id == id)
        .unwrap();
    assert_eq!(row.state, "cancelled");
    assert!(!std::path::Path::new(&format!("{}.part", row.dest_path)).exists());

    // Terminal: neither resume nor a second cancel is accepted.
    assert!(core.resume_download(&id).is_err());
    assert!(core.cancel_download(&id).is_err());
    core.shutdown();
}
