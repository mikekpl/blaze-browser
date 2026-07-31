//! T071: golden event-sequence contract tests (web-engine-trait.md invariant 5,
//! "the swap guarantee"). The golden sequences below ARE the contract: any
//! backend — this Servo scaffold today, the libservo embedding tomorrow, and
//! the Swift WKWebView backend (checked manually against the same corpus in
//! BlazeTests) — must produce exactly these normalized sequences for the
//! shared page corpus.

use std::collections::HashMap;

use blaze_engine::{BlockingArtifacts, EngineEvent, NetworkRules, ResourceKind, WebEngine};
use blaze_engine_servo::{PageSource, ServoEngine, SimulatedPage};
use url::Url;

/// Shared test-page corpus (identical across backends).
fn corpus() -> HashMap<String, SimulatedPage> {
    let mut pages = HashMap::new();
    pages.insert(
        "https://news.example/".to_owned(),
        SimulatedPage {
            title: "Daily News".into(),
            subresources: vec![
                (
                    Url::parse("https://ads.example/banner.js").unwrap(),
                    ResourceKind::Script,
                ),
                (
                    Url::parse("https://static.news.example/app.js").unwrap(),
                    ResourceKind::Script,
                ),
                (
                    Url::parse("https://tracker.example/pixel.gif").unwrap(),
                    ResourceKind::Image,
                ),
            ],
            popups: vec![],
        },
    );
    pages.insert(
        "https://popup.example/".to_owned(),
        SimulatedPage {
            title: "Popup Farm".into(),
            subresources: vec![],
            popups: vec![Url::parse("https://popup.example/win").unwrap()],
        },
    );
    pages.insert(
        "https://clean.example/".to_owned(),
        SimulatedPage {
            title: "Clean Page".into(),
            ..Default::default()
        },
    );
    pages
}

struct Source(HashMap<String, SimulatedPage>);

impl PageSource for Source {
    fn load(&self, url: &Url) -> Option<SimulatedPage> {
        self.0.get(url.as_str()).cloned()
    }
}

fn backend() -> ServoEngine {
    let mut engine = ServoEngine::new(
        Box::new(Source(corpus())),
        Box::new(|url, _| {
            matches!(
                url.host_str(),
                Some("ads.example") | Some("tracker.example")
            )
        }),
    );
    engine.apply_blocking(BlockingArtifacts {
        network_rules: NetworkRules {
            webkit_json: None,
            native_matcher: true,
        },
        ..Default::default()
    });
    engine
}

/// Normalize events into stable, backend-agnostic strings.
fn normalize(events: &[EngineEvent]) -> Vec<String> {
    events
        .iter()
        .map(|e| match e {
            EngineEvent::NavigationStarted { url } => format!("started {url}"),
            EngineEvent::NavigationCommitted { url } => format!("committed {url}"),
            EngineEvent::NavigationFinished { url, success } => {
                format!("finished {url} success={success}")
            }
            EngineEvent::TitleChanged(t) => format!("title {t}"),
            EngineEvent::RequestBlocked { url, .. } => format!("blocked {url}"),
            EngineEvent::PopupBlocked { url } => format!("popup_blocked {url}"),
            other => format!("{other:?}"),
        })
        .collect()
}

fn run(engine: &mut ServoEngine, url: &str) -> Vec<String> {
    engine.navigate(&Url::parse(url).unwrap());
    engine.pump_to_idle();
    normalize(&engine.drain_events())
}

#[test]
fn golden_ad_heavy_page() {
    let mut engine = backend();
    assert_eq!(
        run(&mut engine, "https://news.example/"),
        vec![
            "started https://news.example/",
            "blocked https://ads.example/banner.js",
            "blocked https://tracker.example/pixel.gif",
            "committed https://news.example/",
            "title Daily News",
            "finished https://news.example/ success=true",
        ]
    );
}

#[test]
fn golden_popup_page() {
    let mut engine = backend();
    assert_eq!(
        run(&mut engine, "https://popup.example/"),
        vec![
            "started https://popup.example/",
            "popup_blocked https://popup.example/win",
            "committed https://popup.example/",
            "title Popup Farm",
            "finished https://popup.example/ success=true",
        ]
    );
}

#[test]
fn golden_clean_page_no_blocking_noise() {
    let mut engine = backend();
    assert_eq!(
        run(&mut engine, "https://clean.example/"),
        vec![
            "started https://clean.example/",
            "committed https://clean.example/",
            "title Clean Page",
            "finished https://clean.example/ success=true",
        ]
    );
}

#[test]
fn golden_unreachable_page() {
    let mut engine = backend();
    assert_eq!(
        run(&mut engine, "https://nowhere.example/"),
        vec![
            "started https://nowhere.example/",
            "finished https://nowhere.example/ success=false",
        ]
    );
}

/// Invariant 3: with the native matcher disabled (WebKit declarative path),
/// the backend must not double-report blocks.
#[test]
fn no_native_blocks_when_declarative_rules_own_blocking() {
    let mut engine = backend();
    engine.apply_blocking(BlockingArtifacts {
        network_rules: NetworkRules {
            webkit_json: Some("[]".into()),
            native_matcher: false,
        },
        ..Default::default()
    });
    let events = run(&mut engine, "https://news.example/");
    assert!(!events.iter().any(|e| e.starts_with("blocked ")));
}
