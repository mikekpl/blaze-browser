//! Boots the real libservo embedding end to end: load a page from a local
//! server, observe the navigation events, the title, request blocking and a
//! rendered frame. This is the check that a servo upgrade still *runs*, not
//! just compiles. Needs no network.
//!
//!   cargo test -p blaze-engine-servo --features servo --test embedded_smoke
#![cfg(feature = "servo")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blaze_engine::{BlockingArtifacts, EngineEvent, NetworkRules, WebEngine};
use blaze_engine_servo::EmbeddedServoEngine;
use url::Url;

const PAGE: &str = "<!doctype html><title>blaze-smoke</title>\
    <body style='background:#0a84ff'><h1>hello</h1><script src='/blocked.js'></script>";

/// Minimal HTTP server; records every requested path.
fn serve(paths: Arc<Mutex<Vec<String>>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]);
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
            paths.lock().expect("paths").push(path.clone());
            let (kind, body) = if path == "/" {
                ("text/html", PAGE)
            } else {
                ("application/javascript", "document.title = 'not-blocked';")
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    port
}

#[test]
fn loads_blocks_and_renders_a_page() {
    let paths = Arc::new(Mutex::new(Vec::new()));
    let port = serve(paths.clone());
    let url = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("url");

    let mut engine = EmbeddedServoEngine::new(Box::new(|url, _| url.path() == "/blocked.js"));
    engine.apply_blocking(BlockingArtifacts {
        network_rules: NetworkRules {
            native_matcher: true,
            ..NetworkRules::default()
        },
        ..BlockingArtifacts::default()
    });
    // first navigation of a fresh engine: the case a load-after-build race loses
    engine.navigate(&url);

    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    let target = url.clone();
    let finished = |events: &[EngineEvent]| {
        events.iter().any(|e| {
            matches!(e, EngineEvent::NavigationFinished { url, success: true } if *url == target)
        })
    };
    while Instant::now() < deadline && !finished(&events) {
        events.extend(engine.drain_events());
        std::thread::sleep(Duration::from_millis(50));
    }
    // the title is reported on its own schedule; give it a moment
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && engine.poll_title().is_none() {
        std::thread::sleep(Duration::from_millis(50));
    }
    events.extend(engine.drain_events());

    assert!(finished(&events), "page never finished loading: {events:?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, EngineEvent::NavigationCommitted { url: committed } if *committed == url)),
        "no commit event: {events:?}"
    );
    assert_eq!(
        engine.poll_title().as_deref(),
        Some("blaze-smoke"),
        "events: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, EngineEvent::RequestBlocked { .. })),
        "blocker was not consulted: {events:?}"
    );
    assert!(
        !paths
            .lock()
            .expect("paths")
            .iter()
            .any(|p| p == "/blocked.js"),
        "blocked script still hit the network"
    );

    let (width, height, rgba) = engine.frame_rgba().expect("no rendered frame");
    assert_eq!(rgba.len(), (width * height * 4) as usize);
    assert!(
        rgba.as_chunks::<4>()
            .0
            .iter()
            .any(|px| px[..3] != [0, 0, 0]),
        "frame is entirely black"
    );
}
