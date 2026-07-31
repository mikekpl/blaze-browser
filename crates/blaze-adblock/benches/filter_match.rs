//! Criterion benchmarks (T064, SC-004): filter matching must stay well under
//! the 100µs p99 gate, and the DAT-cache engine load path is tracked so
//! startup regressions surface. Run: `cargo bench -p blaze-adblock`.
//! Set `BLAZE_PERF_GATE=1` to hard-fail when the measured p99 exceeds 100µs.

use std::time::{Duration, Instant};

use blaze_adblock::engine::AdblockEngine;
use blaze_engine::ResourceKind;
use criterion::{Criterion, black_box, criterion_group, criterion_main};

/// Synthetic but realistically shaped list: network rules with anchors,
/// exceptions, and cosmetic rules — ~60k lines total.
fn synthetic_list() -> String {
    let mut list = String::with_capacity(2 << 20);
    for i in 0..40_000 {
        list.push_str(&format!("||ad{i}.tracker.example^\n"));
    }
    for i in 0..10_000 {
        list.push_str(&format!("/banner{i}/*$image\n"));
    }
    for i in 0..5_000 {
        list.push_str(&format!("@@||cdn{i}.safe.example^\n"));
    }
    for i in 0..5_000 {
        list.push_str(&format!("example.com##.promo-{i}\n"));
    }
    list
}

fn requests() -> Vec<(&'static str, &'static str, ResourceKind)> {
    vec![
        // hits
        (
            "https://ad1337.tracker.example/pixel.gif",
            "https://news.example/",
            ResourceKind::Image,
        ),
        (
            "https://cdn.example/banner42/ad.png",
            "https://news.example/",
            ResourceKind::Image,
        ),
        // exception
        (
            "https://cdn7.safe.example/lib.js",
            "https://news.example/",
            ResourceKind::Script,
        ),
        // misses
        (
            "https://static.news.example/app.js",
            "https://news.example/",
            ResourceKind::Script,
        ),
        (
            "https://video.example/stream.m3u8",
            "https://video.example/watch",
            ResourceKind::Media,
        ),
    ]
}

fn p99(samples: &mut [Duration]) -> Duration {
    samples.sort();
    samples[samples.len() * 99 / 100]
}

fn bench_filter_match(c: &mut Criterion) {
    let engine = AdblockEngine::from_lists([synthetic_list().as_str()]);
    let reqs = requests();

    c.bench_function("filter_match_mixed", |b| {
        b.iter(|| {
            for (url, source, kind) in &reqs {
                let _ = black_box(engine.check(url, source, *kind));
            }
        })
    });

    // Explicit p99 measurement for the SC-004 gate (per single request).
    let mut samples: Vec<Duration> = Vec::with_capacity(10_000);
    for i in 0..10_000 {
        let (url, source, kind) = &reqs[i % reqs.len()];
        let start = Instant::now();
        let _ = black_box(engine.check(url, source, *kind));
        samples.push(start.elapsed());
    }
    let p99 = p99(&mut samples);
    eprintln!("filter match p99: {p99:?} (gate: 100µs)");
    if std::env::var("BLAZE_PERF_GATE").as_deref() == Ok("1") {
        assert!(
            p99 < Duration::from_micros(100),
            "p99 {p99:?} exceeds the 100µs gate (SC-004)"
        );
    }
}

fn bench_engine_load(c: &mut Criterion) {
    let engine = AdblockEngine::from_lists([synthetic_list().as_str()]);
    let dat = engine.to_cache();

    // Startup path (B1/B2): loading the serialized DAT, not re-parsing text.
    c.bench_function("engine_load_from_cache", |b| {
        b.iter(|| {
            let restored = AdblockEngine::from_cache(black_box(&dat)).unwrap();
            black_box(restored);
        })
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(20);
    targets = bench_filter_match, bench_engine_load
}
criterion_main!(benches);
