//! Real libservo (servo v0.4.0) embedding behind the `servo` feature.
//!
//! `WebEngine` requires `Send`, but libservo's `Servo`/`WebView` handles are
//! `Rc`-based and must stay on one thread. So each `EmbeddedServoEngine` owns a
//! dedicated engine thread that runs the Servo event loop; the handle talks to
//! it over a channel and reads engine state (title/url/events) from shared
//! memory. Rendering is headless (`SoftwareRenderingContext`); frames can be
//! copied out with [`EmbeddedServoEngine::frame_rgba`] for display layers.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use blaze_engine::{AudioState, BlockingArtifacts, EngineEvent, ResourceKind, WebEngine};
use libservo::{
    LoadStatus, RenderingContext, Servo, ServoBuilder, SoftwareRenderingContext,
    UserContentManager, WebResourceLoad, WebView, WebViewBuilder, WebViewDelegate,
};
use url::Url;

use crate::Blocker;

/// Headless viewport; display layers resize via [`EmbeddedServoEngine::resize`].
const DEFAULT_VIEWPORT: (u32, u32) = (1280, 800);

enum Command {
    Navigate(Url),
    GoBack,
    GoForward,
    Reload,
    ApplyBlocking(BlockingArtifacts),
    SetMuted(bool),
    Suspend,
    Resume(Url),
    Resize(u32, u32),
    /// Copy the current frame out as (width, height, RGBA bytes).
    Frame(Sender<Option<(u32, u32, Vec<u8>)>>),
    /// Servo asked for an event-loop spin (via `EventLoopWaker`).
    Wake,
    Shutdown,
}

#[derive(Default)]
struct Shared {
    events: VecDeque<EngineEvent>,
    title: Option<String>,
    url: Option<Url>,
    muted: bool,
}

type SharedState = Arc<Mutex<Shared>>;

impl Shared {
    fn push(&mut self, event: EngineEvent) {
        self.events.push_back(event);
    }
}

/// Wakes the engine thread's command loop when Servo needs a spin.
#[derive(Clone)]
struct Waker(Sender<Command>);

impl libservo::EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn libservo::EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        let _ = self.0.send(Command::Wake);
    }
}

/// Maps Servo callbacks onto the `EngineEvent` contract and enforces blocking.
struct Delegate {
    shared: SharedState,
    blocker: Blocker,
    artifacts: RefCell<BlockingArtifacts>,
}

impl Delegate {
    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().expect("engine state poisoned")
    }
}

/// The request destination lives in a servo-internal crate; classify via its
/// stable Debug name instead of version-locking that crate.
fn resource_kind(destination: &impl std::fmt::Debug) -> ResourceKind {
    match format!("{destination:?}").to_ascii_lowercase().as_str() {
        "document" | "iframe" | "frame" | "embed" | "object" => ResourceKind::Document,
        "script" | "serviceworker" | "sharedworker" | "worker" | "audioworklet"
        | "paintworklet" | "json" => ResourceKind::Script,
        "image" => ResourceKind::Image,
        "style" | "xslt" => ResourceKind::Stylesheet,
        "font" => ResourceKind::Font,
        "audio" | "video" | "track" => ResourceKind::Media,
        "websocket" => ResourceKind::Websocket,
        "" | "none" => ResourceKind::Xhr,
        _ => ResourceKind::Other,
    }
}

impl WebViewDelegate for Delegate {
    fn notify_load_status_changed(&self, webview: WebView, status: LoadStatus) {
        let url = webview
            .url()
            .unwrap_or_else(|| Url::parse("about:blank").expect("static URL"));
        let event = match status {
            LoadStatus::Started => EngineEvent::NavigationStarted { url },
            LoadStatus::HeadParsed => EngineEvent::NavigationCommitted { url },
            LoadStatus::Complete => EngineEvent::NavigationFinished { url, success: true },
        };
        self.shared().push(event);
    }

    fn notify_url_changed(&self, _webview: WebView, url: Url) {
        self.shared().url = Some(url);
    }

    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        let mut shared = self.shared();
        shared.title = title.clone();
        if let Some(title) = title {
            shared.push(EngineEvent::TitleChanged(title));
        }
    }

    fn notify_crashed(&self, _webview: WebView, reason: String, _backtrace: Option<String>) {
        self.shared().push(EngineEvent::Crashed { reason });
    }

    fn notify_fullscreen_state_changed(&self, _webview: WebView, is_fullscreen: bool) {
        self.shared()
            .push(EngineEvent::FullscreenRequested(is_fullscreen));
    }

    fn request_create_new(
        &self,
        parent_webview: WebView,
        _request: libservo::CreateNewWebViewRequest,
    ) {
        // Popups are always denied (US1-AC5): dropping the request without
        // building a WebView cancels it. The target URL is not exposed here,
        // so report the opener's URL.
        if let Some(url) = parent_webview.url() {
            self.shared().push(EngineEvent::PopupBlocked { url });
        }
    }

    fn load_web_resource(&self, _webview: WebView, load: WebResourceLoad) {
        if !self.artifacts.borrow().network_rules.native_matcher {
            return;
        }
        let request = load.request();
        let kind = resource_kind(&request.destination);
        let url = request.url.clone();
        if (self.blocker)(&url, kind) {
            self.shared().push(EngineEvent::RequestBlocked {
                url: url.clone(),
                kind,
            });
            // Intercept and cancel so the request never reaches the network.
            load.intercept(libservo::WebResourceResponse::new(url)).cancel();
        }
    }
}

/// One instance per tab view, backed by a real libservo `WebView`.
pub struct EmbeddedServoEngine {
    commands: Sender<Command>,
    shared: SharedState,
    thread: Option<JoinHandle<()>>,
    suspended: bool,
}

impl EmbeddedServoEngine {
    /// Spawn the engine thread and load nothing (navigate to begin).
    /// `blocker` is consulted for every subresource; `true` blocks it.
    pub fn new(blocker: Blocker) -> Self {
        let shared: SharedState = Arc::default();
        let (tx, rx) = channel();
        let thread_shared = shared.clone();
        let waker_tx = tx.clone();
        let thread = std::thread::Builder::new()
            .name("blaze-servo".into())
            .spawn(move || run_engine_thread(thread_shared, blocker, rx, waker_tx))
            .expect("failed to spawn servo engine thread");
        Self {
            commands: tx,
            shared,
            thread: Some(thread),
            suspended: false,
        }
    }

    fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Drain queued engine events (same contract as the simulated backend).
    pub fn drain_events(&mut self) -> Vec<EngineEvent> {
        self.shared
            .lock()
            .expect("engine state poisoned")
            .events
            .drain(..)
            .collect()
    }

    pub fn current_url(&self) -> Option<Url> {
        self.shared.lock().expect("engine state poisoned").url.clone()
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    /// Resize the headless viewport (device pixels).
    pub fn resize(&self, width: u32, height: u32) {
        self.send(Command::Resize(width, height));
    }

    /// Synchronously copy the last rendered frame as RGBA bytes.
    pub fn frame_rgba(&self) -> Option<(u32, u32, Vec<u8>)> {
        let (tx, rx) = channel();
        self.send(Command::Frame(tx));
        rx.recv_timeout(Duration::from_secs(2)).ok().flatten()
    }
}

impl WebEngine for EmbeddedServoEngine {
    fn navigate(&mut self, url: &Url) {
        self.suspended = false;
        self.send(Command::Navigate(url.clone()));
    }

    fn go_back(&mut self) {
        self.send(Command::GoBack);
    }

    fn go_forward(&mut self) {
        self.send(Command::GoForward);
    }

    fn reload(&mut self) {
        self.send(Command::Reload);
    }

    fn stop(&mut self) {
        // libservo 0.4 exposes no cancel-load API; the closest safe behaviour
        // is a no-op (the load finishes into a suspended-equivalent state).
    }

    fn apply_blocking(&mut self, artifacts: BlockingArtifacts) {
        self.send(Command::ApplyBlocking(artifacts));
    }

    fn set_muted(&mut self, muted: bool) {
        let changed = {
            let mut shared = self.shared.lock().expect("engine state poisoned");
            let changed = shared.muted != muted;
            shared.muted = muted;
            changed
        };
        if changed {
            self.send(Command::SetMuted(muted));
            self.shared
                .lock()
                .expect("engine state poisoned")
                .push(EngineEvent::AudioStateChanged(if muted {
                    AudioState::Muted
                } else {
                    AudioState::Silent
                }));
        }
    }

    fn suspend(&mut self) {
        self.suspended = true;
        self.send(Command::Suspend);
    }

    fn resume(&mut self, url: &Url) {
        self.suspended = false;
        self.send(Command::Resume(url.clone()));
    }

    fn poll_title(&self) -> Option<String> {
        self.shared.lock().expect("engine state poisoned").title.clone()
    }
}

impl Drop for EmbeddedServoEngine {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// State owned by the engine thread (everything here is `!Send`).
struct EngineThread {
    servo: Servo,
    rendering_context: Rc<SoftwareRenderingContext>,
    delegate: Rc<Delegate>,
    webview: Option<WebView>,
}

impl EngineThread {
    fn ensure_webview(&mut self) -> &WebView {
        if self.webview.is_none() {
            let user_content_manager = UserContentManager::new(&self.servo);
            {
                let artifacts = self.delegate.artifacts.borrow();
                for scriptlet in &artifacts.scriptlets {
                    user_content_manager
                        .add_script(libservo::UserScript::new(scriptlet.source.clone(), None).into());
                }
                if !artifacts.cosmetic_css.is_empty() {
                    user_content_manager.add_script(
                        libservo::UserScript::new(
                            inject_css_script(&artifacts.cosmetic_css),
                            None,
                        )
                        .into(),
                    );
                }
            }
            let webview = WebViewBuilder::new(&self.servo, self.rendering_context.clone())
                .delegate(self.delegate.clone())
                .user_content_manager(Rc::new(user_content_manager))
                .build();
            webview.focus();
            self.webview = Some(webview);
        }
        self.webview.as_ref().expect("just created")
    }

    fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::Wake => {},
            Command::Navigate(url) => self.ensure_webview().load(url),
            Command::GoBack => {
                if let Some(webview) = &self.webview {
                    webview.go_back(1);
                }
            },
            Command::GoForward => {
                if let Some(webview) = &self.webview {
                    webview.go_forward(1);
                }
            },
            Command::Reload => {
                if let Some(webview) = &self.webview {
                    webview.reload();
                }
            },
            Command::ApplyBlocking(artifacts) => {
                // Network gating applies immediately; scriptlets/CSS are baked
                // into the UserContentManager of the next webview (suspend →
                // resume or first navigate), matching "next load" semantics.
                *self.delegate.artifacts.borrow_mut() = artifacts;
            },
            Command::SetMuted(muted) => {
                if let Some(webview) = &self.webview {
                    webview.evaluate_javascript(mute_script(muted), |_| {});
                }
            },
            Command::Suspend => {
                // Dropping the last handle tears the webview down (FR-016).
                self.webview = None;
            },
            Command::Resume(url) => {
                self.webview = None;
                self.ensure_webview().load(url);
            },
            Command::Resize(width, height) => {
                if let Some(webview) = &self.webview {
                    webview.resize(dpi::PhysicalSize::new(width, height));
                }
            },
            Command::Frame(reply) => {
                let frame = self.webview.as_ref().and_then(|webview| {
                    webview.paint();
                    let size = self.rendering_context.size2d();
                    self.rendering_context
                        .read_to_image(libservo::DeviceIntRect::from_size(size.to_i32()))
                        .map(|image| (image.width(), image.height(), image.into_raw()))
                });
                let _ = reply.send(frame);
            },
            Command::Shutdown => return false,
        }
        true
    }
}

fn run_engine_thread(
    shared: SharedState,
    blocker: Blocker,
    commands: Receiver<Command>,
    waker: Sender<Command>,
) {
    // Servo's network stack needs a process-wide rustls crypto provider.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let rendering_context = match SoftwareRenderingContext::new(dpi::PhysicalSize::new(
        DEFAULT_VIEWPORT.0,
        DEFAULT_VIEWPORT.1,
    )) {
        Ok(context) => Rc::new(context),
        Err(error) => {
            shared.lock().expect("engine state poisoned").push(EngineEvent::Crashed {
                reason: format!("failed to create rendering context: {error:?}"),
            });
            return;
        },
    };

    let servo = ServoBuilder::default()
        .event_loop_waker(Box::new(Waker(waker)))
        .build();

    let delegate = Rc::new(Delegate {
        shared: shared.clone(),
        blocker,
        artifacts: RefCell::new(BlockingArtifacts::default()),
    });

    let mut thread = EngineThread {
        servo,
        rendering_context,
        delegate,
        webview: None,
    };

    loop {
        // Bounded wait so animations/timers keep advancing even without wakes.
        match commands.recv_timeout(Duration::from_millis(16)) {
            Ok(command) => {
                if !thread.handle(command) {
                    break;
                }
            },
            Err(RecvTimeoutError::Timeout) => {},
            Err(RecvTimeoutError::Disconnected) => break,
        }
        thread.servo.spin_event_loop();
    }

    // Dropping the webview then the last Servo handle shuts the engine down
    // (Servo drop spins the loop until the constellation exits).
    thread.webview = None;
}

fn mute_script(muted: bool) -> String {
    format!(
        "document.querySelectorAll('audio,video').forEach(m => m.muted = {muted});"
    )
}

fn inject_css_script(css: &str) -> String {
    // serde_json escaping keeps arbitrary filter CSS safe inside the literal.
    let literal = serde_json::to_string(css).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        "(() => {{ const s = document.createElement('style'); s.textContent = {literal}; \
         (document.head || document.documentElement).appendChild(s); }})();"
    )
}
