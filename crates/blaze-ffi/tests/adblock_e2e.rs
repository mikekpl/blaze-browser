//! Integration test (T032, quickstart scenario 1): the blocking invariant —
//! rule-matched requests never reach the network. Drives the real FFI surface
//! (engine init from disk, DAT cache, classify gate) against a local server
//! that counts every request it receives.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use blaze_ffi::{BlazeCoreHandle, EventListener, Rect};

struct NullListener;
impl EventListener for NullListener {
    fn on_events(&self, _events_json: String) {}
}

/// Local HTTP server recording each request path it serves.
fn spawn_counting_server() -> (String, Arc<AtomicUsize>, Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let paths = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (hits2, paths2) = (hits.clone(), paths.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            if let Some(path) = request.split_whitespace().nth(1) {
                hits2.fetch_add(1, Ordering::SeqCst);
                paths2.lock().expect("paths lock").push(path.to_string());
            }
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        }
    });
    (format!("http://{addr}"), hits, paths)
}

/// Fetch `url` only if the shield classifier allows it — exactly what the
/// engine backend does in `decidePolicyFor`.
fn fetch_if_allowed(handle: &BlazeCoreHandle, tab: &str, url: &str, source: &str) -> bool {
    let blocked = handle
        .classify_request(tab.into(), url.into(), source.into(), "script".into())
        .expect("classify");
    if blocked {
        return false;
    }
    // Plain HTTP GET (std only).
    let authority = url.strip_prefix("http://").expect("http url");
    let (host, path) = authority.split_once('/').expect("path");
    let mut stream = TcpStream::connect(host).expect("connect");
    let request = format!("GET /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("send");
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    assert!(response.starts_with("HTTP/1.1 200"));
    true
}

#[test]
fn rule_matched_requests_never_hit_the_network() {
    let dir = tempfile::tempdir().expect("tempdir");
    let filters = dir.path().join("filters");
    std::fs::create_dir_all(&filters).expect("mkdir");
    std::fs::write(filters.join("list.txt"), "/ads/*\n/tracking/*\n").expect("write list");

    let handle = BlazeCoreHandle::new(
        dir.path().to_string_lossy().into_owned(),
        Box::new(NullListener),
    )
    .expect("core");
    handle
        .init_adblock(filters.to_string_lossy().into_owned(), String::new())
        .expect("adblock init");

    let window = handle
        .create_window(Rect {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
        })
        .expect("window");
    let tab = handle.create_tab(window, None).expect("tab");

    let (base, hits, paths) = spawn_counting_server();
    let page = format!("{base}/index.html");

    // Blocked resources: classifier must gate them off the network entirely.
    assert!(!fetch_if_allowed(
        &handle,
        &tab,
        &format!("{base}/ads/banner.js"),
        &page
    ));
    assert!(!fetch_if_allowed(
        &handle,
        &tab,
        &format!("{base}/tracking/pixel.js"),
        &page
    ));
    // Legitimate resource loads normally (layout intact).
    assert!(fetch_if_allowed(
        &handle,
        &tab,
        &format!("{base}/app.js"),
        &page
    ));

    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "exactly one request may reach the server"
    );
    assert_eq!(*paths.lock().expect("paths"), vec!["/app.js".to_string()]);

    // Shield counters reflect the two blocks.
    let stats = handle.get_shield_stats(tab).expect("stats");
    let v: serde_json::Value = serde_json::from_str(&stats).expect("json");
    assert_eq!(
        v["ads_blocked"].as_u64().unwrap_or(0) + v["trackers_blocked"].as_u64().unwrap_or(0),
        2
    );

    handle.shutdown();
}
