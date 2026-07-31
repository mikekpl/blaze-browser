import SwiftUI
import WebKit

/// WKWebView fallback backend implementing the WebEngine contract from the
/// Swift side (contracts/web-engine-trait.md). Blocking enforcement (US1):
/// 1. Declarative: compiled WKContentRuleList installed at view creation.
/// 2. Cosmetic: per-site CSS + scriptlets injected at document start.
/// 3. Fallback: `decidePolicyFor` consults the native matcher for frames.
final class WebKitBackend: NSObject, ObservableObject {
    let webView: WKWebView
    /// Core tab this view renders; set before first navigation.
    var tabId: String = ""
    @Published var currentURL: String = ""
    @Published var title: String = ""
    @Published var isLoading: Bool = false
    @Published var errorPage: ErrorPageModel?
    /// Set when the page requests a DRM key system WebKit can't provide (T045).
    @Published var drmNotice: String?

    private weak var bridge: CoreBridge?
    private static var compiledRules: WKContentRuleList?

    init(bridge: CoreBridge) {
        let config = WKWebViewConfiguration()
        config.defaultWebpagePreferences.allowsContentJavaScript = true
        // Popup blocking baseline (US1-AC5): windows must be user-created.
        config.preferences.javaScriptCanOpenWindowsAutomatically = false
        // Fullscreen video (T043, US3-AC3).
        config.preferences.isElementFullscreenEnabled = true
        webView = WKWebView(frame: .zero, configuration: config)
        self.bridge = bridge
        super.init()
        webView.navigationDelegate = self
        webView.uiDelegate = self
        webView.allowsBackForwardNavigationGestures = true
        let controller = webView.configuration.userContentController
        controller.add(WeakMessageHandler(self), name: "blazeMedia")
        controller.add(WeakMessageHandler(self), name: "blazeDRM")
        installBuiltinScripts(into: controller)
        installContentRules()
        NotificationCenter.default.addObserver(
            self, selector: #selector(adblockToggled(_:)),
            name: .blazeAdblockChanged, object: nil)
    }

    /// Global Shields toggle (T060): add/remove declarative rules live.
    @objc private func adblockToggled(_ note: Notification) {
        let enabled = note.userInfo?["enabled"] as? Bool ?? true
        let controller = webView.configuration.userContentController
        if enabled {
            installContentRules()
        } else {
            controller.removeAllContentRuleLists()
        }
    }

    deinit {
        let controller = webView.configuration.userContentController
        controller.removeScriptMessageHandler(forName: "blazeMedia")
        controller.removeScriptMessageHandler(forName: "blazeDRM")
    }

    // MARK: - Blocking artifacts (T026)

    /// Compile-once, reuse-everywhere: the rule list is identity-cached by
    /// WKContentRuleListStore under "blaze-shields".
    private func installContentRules() {
        guard bridge?.settings.adblockEnabled != false else { return }
        if let rules = Self.compiledRules {
            webView.configuration.userContentController.add(rules)
            return
        }
        guard let json = bridge?.webkitRulesJSON() else {
            // Adblock still initializing — retry when ready.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
                self?.installContentRules()
            }
            return
        }
        WKContentRuleListStore.default().compileContentRuleList(
            forIdentifier: "blaze-shields", encodedContentRuleList: json
        ) { [weak self] rules, error in
            if let rules {
                Self.compiledRules = rules
                self?.webView.configuration.userContentController.add(rules)
            } else if let error {
                NSLog("Blaze: content rule compilation failed: %@", "\(error)")
            }
        }
    }

    /// Inject per-site cosmetic CSS and scriptlets at document start.
    private func applyCosmetics(for url: URL) {
        guard let bridge, let payload = bridge.cosmetics(for: url.absoluteString) else { return }
        let controller = webView.configuration.userContentController
        controller.removeAllUserScripts()
        installBuiltinScripts(into: controller)
        if !payload.css.isEmpty {
            let escaped = payload.css
                .replacingOccurrences(of: "\\", with: "\\\\")
                .replacingOccurrences(of: "`", with: "\\`")
            let source = """
            (function() {
              const s = document.createElement('style');
              s.textContent = `\(escaped)`;
              document.documentElement.appendChild(s);
            })();
            """
            controller.addUserScript(WKUserScript(
                source: source, injectionTime: .atDocumentStart, forMainFrameOnly: false))
        }
        for scriptlet in payload.scriptlets {
            controller.addUserScript(WKUserScript(
                source: scriptlet.source, injectionTime: .atDocumentStart, forMainFrameOnly: false))
        }
    }

    // MARK: - WebEngine commands

    func navigate(to url: URL) {
        errorPage = nil
        drmNotice = nil
        applyCosmetics(for: url)
        webView.load(URLRequest(url: url))
    }

    func goBack() { webView.goBack() }
    func goForward() { webView.goForward() }
    func reload() { webView.reload() }
    func stop() { webView.stopLoading() }

    /// Mute/unmute the page's media elements (T042; no public WKWebView mute API).
    func setPageMuted(_ muted: Bool) {
        let js = "document.querySelectorAll('video,audio').forEach(m => { m.muted = \(muted); });"
            + " window.__blazePageMuted = \(muted);"
        webView.evaluateJavaScript(js)
    }

    // MARK: - Built-in detection scripts (T041, T045)

    /// Media playback + DRM detection; re-added after every cosmetics reset.
    private func installBuiltinScripts(into controller: WKUserContentController) {
        controller.addUserScript(WKUserScript(
            source: Self.mediaDetectionJS, injectionTime: .atDocumentStart, forMainFrameOnly: false))
        controller.addUserScript(WKUserScript(
            source: Self.drmDetectionJS, injectionTime: .atDocumentStart, forMainFrameOnly: false))
    }

    /// Counts playing media elements and reports audible-state transitions.
    private static let mediaDetectionJS = """
    (function () {
      if (window.__blazeMediaHook) { return; }
      window.__blazeMediaHook = true;
      let playing = 0;
      const post = (isPlaying) => {
        try { webkit.messageHandlers.blazeMedia.postMessage({ playing: isPlaying }); } catch (_) {}
      };
      const update = (delta) => {
        const was = playing > 0;
        playing = Math.max(0, playing + delta);
        const now = playing > 0;
        if (was !== now) { post(now); }
      };
      document.addEventListener('play', (e) => {
        if (window.__blazePageMuted && e.target && 'muted' in e.target) { e.target.muted = true; }
        update(1);
      }, true);
      document.addEventListener('pause', () => update(-1), true);
      document.addEventListener('ended', () => update(-1), true);
      window.addEventListener('pagehide', () => { if (playing > 0) { playing = 0; post(false); } });
    })();
    """

    /// Reports EME key-system requests that WebKit rejects (Widevine etc.).
    private static let drmDetectionJS = """
    (function () {
      if (window.__blazeDrmHook || !navigator.requestMediaKeySystemAccess) { return; }
      window.__blazeDrmHook = true;
      const orig = navigator.requestMediaKeySystemAccess.bind(navigator);
      navigator.requestMediaKeySystemAccess = function (keySystem, configs) {
        return orig(keySystem, configs).catch((err) => {
          try { webkit.messageHandlers.blazeDRM.postMessage({ keySystem: String(keySystem) }); } catch (_) {}
          throw err;
        });
      };
    })();
    """
}

/// Breaks the WKUserContentController → handler retain cycle.
private final class WeakMessageHandler: NSObject, WKScriptMessageHandler {
    private weak var target: WebKitBackend?
    init(_ target: WebKitBackend) { self.target = target }
    func userContentController(_ controller: WKUserContentController,
                               didReceive message: WKScriptMessage) {
        target?.handleScriptMessage(message)
    }
}

extension WebKitBackend {
    func handleScriptMessage(_ message: WKScriptMessage) {
        switch message.name {
        case "blazeMedia":
            let playing = (message.body as? [String: Any])?["playing"] as? Bool ?? false
            bridgeNotifyPlayback(playing)
        case "blazeDRM":
            let keySystem = (message.body as? [String: Any])?["keySystem"] as? String ?? "DRM"
            drmNotice = "This page uses protected content (\(keySystem)) that Blaze can't play yet."
        default:
            break
        }
    }

    private func bridgeNotifyPlayback(_ playing: Bool) {
        guard let bridge, !tabId.isEmpty else { return }
        bridge.notifyMediaPlayback(tabId: tabId, playing: playing)
    }
}

// MARK: - Engine bridging hooks (core-api.md table)

extension WebKitBackend: WKNavigationDelegate {
    func webView(_ webView: WKWebView,
                 decidePolicyFor navigationAction: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard let bridge, let url = navigationAction.request.url else {
            decisionHandler(.allow)
            return
        }
        if navigationAction.shouldPerformDownload {
            // e.g. <a download>; materialize a WKDownload, then hand off to Rust.
            decisionHandler(.download)
            return
        }
        let isMainFrame = navigationAction.targetFrame?.isMainFrame ?? true
        if !isMainFrame {
            // Native-matcher fallback for subframe documents (T026).
            let source = webView.url?.absoluteString ?? ""
            if bridge.shouldBlock(
                tabId: tabId, url: url.absoluteString, sourceURL: source, kind: "document") {
                decisionHandler(.cancel)
                return
            }
        }
        decisionHandler(.allow)
    }

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        isLoading = true
    }

    /// Route non-renderable responses (attachments, binaries) into the
    /// download engine instead of a blank page (T050, FR-020).
    func webView(_ webView: WKWebView,
                 decidePolicyFor navigationResponse: WKNavigationResponse,
                 decisionHandler: @escaping (WKNavigationResponsePolicy) -> Void) {
        let disposition = (navigationResponse.response as? HTTPURLResponse)?
            .value(forHTTPHeaderField: "Content-Disposition")?.lowercased() ?? ""
        if !navigationResponse.canShowMIMEType || disposition.hasPrefix("attachment") {
            decisionHandler(.download)
            return
        }
        decisionHandler(.allow)
    }

    func webView(_ webView: WKWebView,
                 navigationAction: WKNavigationAction,
                 didBecome download: WKDownload) {
        download.delegate = self
    }

    func webView(_ webView: WKWebView,
                 navigationResponse: WKNavigationResponse,
                 didBecome download: WKDownload) {
        download.delegate = self
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        if let url = webView.url?.absoluteString {
            currentURL = url
            bridge?.notifyCommitted(tabId: tabId, url: url)
        }
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        isLoading = false
        title = webView.title ?? ""
        currentURL = webView.url?.absoluteString ?? currentURL
        bridge?.notifyLoaded(tabId: tabId, title: webView.title, success: true)
    }

    func webView(_ webView: WKWebView,
                 didFailProvisionalNavigation navigation: WKNavigation!,
                 withError error: Error) {
        showError(error)
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        showError(error)
    }

    /// Friendly error page (T029, FR-005). Cancellations are not errors.
    private func showError(_ error: Error) {
        isLoading = false
        bridge?.notifyLoaded(tabId: tabId, title: nil, success: false)
        let nsError = error as NSError
        if nsError.code == NSURLErrorCancelled { return }
        errorPage = ErrorPageModel(
            url: currentURL,
            message: friendlyMessage(for: nsError))
    }

    private func friendlyMessage(for error: NSError) -> String {
        switch error.code {
        case NSURLErrorNotConnectedToInternet:
            return "You appear to be offline. Check your connection and try again."
        case NSURLErrorCannotFindHost, NSURLErrorDNSLookupFailed:
            return "This site can't be found. Check the address for typos."
        case NSURLErrorTimedOut:
            return "The site took too long to respond."
        case NSURLErrorServerCertificateUntrusted, NSURLErrorSecureConnectionFailed:
            return "A secure connection couldn't be established."
        default:
            return "The page couldn't be loaded. (\(error.localizedDescription))"
        }
    }
}

extension WebKitBackend: WKDownloadDelegate {
    /// Handoff point (T050): the Rust engine owns the transfer (ranged
    /// resume, crash recovery), so the WKDownload itself is cancelled.
    func download(_ download: WKDownload,
                  decideDestinationUsing response: URLResponse,
                  suggestedFilename: String,
                  completionHandler: @escaping (URL?) -> Void) {
        let url = download.originalRequest?.url ?? response.url
        if let url, url.scheme == "http" || url.scheme == "https" {
            bridge?.startDownload(url: url.absoluteString, suggestedName: suggestedFilename)
        }
        isLoading = false
        completionHandler(nil) // nil destination cancels the WebKit transfer
    }
}

extension WebKitBackend: WKUIDelegate {
    /// File uploads via <input type="file"> (T051, FR-024): standard
    /// open panel honoring the page's multi-file/directory options.
    func webView(_ webView: WKWebView,
                 runOpenPanelWith parameters: WKOpenPanelParameters,
                 initiatedByFrame frame: WKFrameInfo,
                 completionHandler: @escaping ([URL]?) -> Void) {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = parameters.allowsDirectories
        panel.allowsMultipleSelection = parameters.allowsMultipleSelection
        if let window = webView.window {
            panel.beginSheetModal(for: window) { response in
                completionHandler(response == .OK ? panel.urls : nil)
            }
        } else {
            completionHandler(panel.runModal() == .OK ? panel.urls : nil)
        }
    }

    /// Popup blocking (T030, US1-AC5): never create implicit web views.
    func webView(_ webView: WKWebView,
                 createWebViewWith configuration: WKWebViewConfiguration,
                 for navigationAction: WKNavigationAction,
                 windowFeatures: WKWindowFeatures) -> WKWebView? {
        if let url = navigationAction.request.url {
            if navigationAction.targetFrame == nil {
                // Open user-initiated link targets in the same view instead of a popup.
                webView.load(URLRequest(url: url))
            }
            DispatchQueue.main.async { [weak self] in
                self?.bridge?.popupNotice = "Popup blocked: \(url.host ?? url.absoluteString)"
            }
        }
        return nil
    }
}

/// SwiftUI wrapper for the backing WKWebView.
struct WebViewContainer: NSViewRepresentable {
    @ObservedObject var backend: WebKitBackend

    func makeNSView(context: Context) -> WKWebView { backend.webView }
    func updateNSView(_ nsView: WKWebView, context: Context) {}
}
