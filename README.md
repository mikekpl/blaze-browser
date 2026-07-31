<p align="center">
  <img src="platforms/macos/Icon/appicon-1024.png" width="128"/>
</p>

<h1 align="center">Blaze Browser</h1>

<p align="center">
  <strong>Fast. Private. Zero bloat.</strong><br/>
  A Rust-native, ad-free macOS browser built for people who want speed without compromise.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/platform-macOS%2013%2B-blue?style=flat-square"/>
  <img src="https://img.shields.io/badge/language-Rust%20%2B%20Swift-orange?style=flat-square"/>
  <img src="https://img.shields.io/badge/telemetry-zero-brightgreen?style=flat-square"/>
  <img src="https://img.shields.io/badge/ads-blocked-brightgreen?style=flat-square"/>
  <img src="https://img.shields.io/badge/license-MIT-lightgrey?style=flat-square"/>
</p>

---

## Screenshots

<p align="center">
  <img src="assets/blaze-dark.png" width="680" alt="Blaze Browser – dark mode"/>
  <br/><em>Dark mode</em>
</p>

<p align="center">
  <img src="assets/blaze-light.png" width="680" alt="Blaze Browser – light mode"/>
  <br/><em>Light mode</em>
</p>

---

## Why Blaze?

Most browsers are Chromium wrappers — they carry Google's rendering engine, telemetry hooks, and memory overhead whether you want them or not. Blaze takes a different path:

- **No Chromium.** The core is pure Rust. The macOS shell uses Apple's WebKit as the rendering backend today, isolated behind a clean abstraction layer so a fully independent Servo-based engine can be swapped in without touching a single line of UI code.
- **No ads, ever.** A Brave-style Rust adblock engine runs natively in-process. Lists compile to a binary trie at startup — filter matching is sub-microsecond (p99 < 1 µs in benchmarks) with zero network round-trips.
- **No telemetry, period.** Zero data collection, zero crash reporting, zero analytics. What you browse stays on your device.
- **Memory efficient.** Tabs you haven't visited in a while are automatically suspended, freeing their memory. Background tabs cost almost nothing.

---

## Features

| | |
|---|---|
| 🔥 **Sub-microsecond ad blocking** | Rust-native filter engine, no extension overhead |
| 🪶 **Lightweight** | Tab suspension frees memory automatically |
| 🚫 **Zero telemetry** | No data ever leaves your device |
| 🌙 **Dark & light mode** | Follows macOS system appearance instantly |
| 📑 **Full tab management** | Create, close, pin, mute, reorder, move between windows |
| 🔖 **Bookmarks** | Toolbar, bookmarks bar, and full manager |
| ⬇️ **Downloads** | Native download engine with pause/resume/progress |
| 🎬 **Video & audio** | YouTube, streaming sites, and any HTML5 media |
| 🔒 **Per-site ad-block exceptions** | Allowlist individual domains with one click |
| 🗂 **Session restore** | All windows and tabs come back exactly where you left them |
| ♾️ **Multiple windows** | Drag tabs out to new windows, merge them back |
| 🔍 **Smart address bar** | URLs, searches, and bookmarks in one field |

---

## The Web Engine Abstraction

Blaze is not tied to any single rendering engine. The Rust core defines a single `WebEngine` trait:

```rust
pub trait WebEngine: Send + Sync {
    fn navigate(&self, url: &str);
    fn reload(&self);
    fn go_back(&self);
    fn go_forward(&self);
    fn evaluate_script(&self, js: &str);
    fn state(&self) -> EngineState;
}
```

Today, `WebKitBackend` implements this trait using Apple's `WKWebView` — giving full compatibility with every website while the native engine matures. Tomorrow, `ServoEngine` implements the same trait using Mozilla's Servo renderer in pure Rust. **Switching engines is a single line of configuration.** The Swift UI, the tab manager, the ad blocker, and the download engine have no knowledge of which backend is running.

```
┌────────────────────────────────────────────────────┐
│                  macOS SwiftUI Shell               │
│  TabStrip · Toolbar · Bookmarks · Settings         │
└──────────────────────┬─────────────────────────────┘
                       │  FFI (uniffi / Swift)
┌──────────────────────▼─────────────────────────────┐
│              blaze-core  (Rust)                    │
│  Tab manager · Session · Downloads · Events        │
└──────────┬───────────────────────┬─────────────────┘
           │                       │
┌──────────▼──────────┐  ┌────────-▼──────────────────┐
│  WebKitBackend      │  │  ServoEngine (in progress) │
│  WKWebView via FFI  │  │  Rust-native renderer      │
└─────────────────────┘  └───────────────────────────-┘
           │
┌──────────▼──────────────────────────────────────────┐
│  blaze-adblock  (Rust)                              │
│  Brave-compatible filter engine · < 1 µs p99        │
└─────────────────────────────────────────────────────┘
```

---

## Architecture at a Glance

```
blaze-browser/
├── crates/
│   ├── blaze-core        # Tab, window, session, downloads, bookmarks
│   ├── blaze-engine      # WebEngine trait + shared types
│   ├── blaze-engine-servo# Servo renderer stub (engine-parity tests pass)
│   ├── blaze-adblock     # Brave-style filter engine
│   ├── blaze-net         # HTTP, range requests, download worker
│   ├── blaze-storage     # SQLite profile: history, bookmarks, settings
│   ├── blaze-media       # Audio/video state tracking
│   └── blaze-ffi         # uniffi bindings for Swift
├── platforms/
│   └── macos/            # SwiftUI shell + XcodeGen project
├── fuzz/                 # cargo-fuzz harnesses
└── scripts/
    └── build-dmg.sh      # Universal (arm64 + x86_64) DMG builder
```

---

## Performance

| Metric | Result | Gate |
|--------|--------|------|
| Ad filter match (p99) | **917 ns** | < 100 µs |
| Filter engine cache load | **3.5 ms** | — |
| Cold browser start | **< 500 ms** | — |
| 10-page RSS growth | **< 64 MiB** | — |

Benchmarks run with `BLAZE_PERF_GATE=1 cargo bench -p blaze-adblock`.

---

## Install

1. Download the latest `Blaze-x.x.x.dmg` from [Releases](https://github.com/mikekpl/blaze-browser/releases).
2. Open the DMG, drag **Blaze.app** to Applications.
3. **Right-click → Open** on first launch (unsigned build — Gatekeeper will ask once).
4. If the Dock icon looks stale after update, run `killall Dock`.

> Blaze is currently **macOS 13+ only** (Apple Silicon and Intel, universal binary). Linux, Windows, Android, and iOS platform stubs are in the architecture but not yet wired up.

---

## Contributing

Pull requests are welcome. Please run `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` before opening a PR. The CI pipeline also runs AddressSanitizer, ThreadSanitizer, and fuzz smoke tests on every push.

---

## License

MIT — see [LICENSE](LICENSE).