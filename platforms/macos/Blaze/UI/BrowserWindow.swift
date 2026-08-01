import SwiftUI
import AppKit

/// Browser window: tab strip (T033) + toolbar + per-tab web views (T038).
/// Binds to one core window — a restored one at launch (T037), a specific
/// id for drag-out windows (T034), or a fresh window (⌘N).
struct BrowserWindow: View {
    @EnvironmentObject private var bridge: CoreBridge
    @Environment(\.openWindow) private var openWindow
    /// Specific core window to bind (drag-out / restored extras); nil = claim any.
    let requestedWindowId: String?
    @StateObject private var store: WebViewStore
    @State private var windowId: String?
    @State private var activeBackend: WebKitBackend?
    @State private var addressText: String = ""
    @State private var hostWindow: NSWindow?
    @FocusState private var addressFocused: Bool

    init(requestedWindowId: String? = nil) {
        self.requestedWindowId = requestedWindowId
        _store = StateObject(wrappedValue: WebViewStore(bridge: CoreBridge.shared))
    }

    private var window: WindowInfo? { bridge.browserState.window(windowId) }
    private var activeTab: TabInfo? {
        window.flatMap { w in w.tabs.first { $0.id == w.activeTabId } }
    }

    var body: some View {
        VStack(spacing: 0) {
            if let windowId {
                TabStrip(windowId: windowId)
                Divider()
            }
            if let backend = activeBackend {
                Toolbar(
                    backend: backend,
                    addressText: $addressText,
                    addressFocused: $addressFocused,
                    onSubmit: submitAddress)
                Divider()
                if bridge.bookmarksBarVisible {
                    BookmarksBar { url in openBookmark(url, backend: backend) }
                    Divider()
                }
                ZStack {
                    WebViewContainer(backend: backend)
                        .id(backend.tabId) // swap NSView when the active tab changes
                    if let model = backend.errorPage {
                        ErrorPageView(model: model) {
                            backend.errorPage = nil
                            backend.reload()
                        }
                    }
                }
                .overlay(alignment: .top) {
                    if let notice = backend.drmNotice {
                        DRMNoticeView(message: notice) { backend.drmNotice = nil }
                            .padding(.top, 12)
                            .transition(.move(edge: .top).combined(with: .opacity))
                    }
                }
            } else {
                ZStack {
                    Color(nsColor: .underPageBackgroundColor)
                    Text("New Tab")
                        .font(.title2)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .ignoresSafeArea(.all, edges: .top)  // fill under transparent titlebar (fullSizeContentView)
        .frame(minWidth: 600, minHeight: 400)
        .overlay(alignment: .bottom) { popupNotice }
        .background(WindowAccessor(window: $hostWindow))
        .onAppear(perform: bindWindow)
        .onChange(of: bridge.browserState) { _ in syncWithState() }
        .onChange(of: activeBackend?.currentURL) { url in
            if let url, !addressFocused { addressText = url }
        }
        .onChange(of: bridge.blockedNavigation?.url) { _ in
            if let blocked = bridge.blockedNavigation, let backend = activeBackend {
                backend.errorPage = ErrorPageModel(
                    url: blocked.url,
                    message: "Navigation blocked: \(blocked.reason)")
                bridge.blockedNavigation = nil
            }
        }
        .onReceive(NotificationCenter.default.publisher(
            for: NSWindow.didBecomeKeyNotification)) { note in
            if let hostWindow, (note.object as? NSWindow) === hostWindow,
               let windowId {
                bridge.setFrontWindow(windowId)
            }
        }
        .onDisappear {
            if let windowId, !bridge.isTerminating {
                bridge.closeWindow(windowId)
            }
        }
    }

    /// Claim a core window and, from the primary launch window, open extra
    /// SwiftUI windows for any remaining restored session windows (T037).
    private func bindWindow() {
        guard windowId == nil else { return }
        windowId = bridge.claimWindow(
            requested: requestedWindowId,
            frame: CGRect(x: 0, y: 0, width: 1280, height: 800))
        if requestedWindowId == nil {
            for extra in bridge.unclaimedWindows {
                openWindow(value: extra)
            }
        }
        syncWithState()
        addressFocused = true
    }

    /// Reconcile web views with core state: resume the active tab if it was
    /// suspended, drop web views for suspended/closed tabs (FR-016).
    private func syncWithState() {
        // last tab closed → core window is gone; close the AppKit window too
        if let windowId, bridge.browserState.window(windowId) == nil {
            hostWindow?.close()
            return
        }
        store.sync(with: window)
        guard let tab = activeTab else {
            activeBackend = nil
            return
        }
        let backend = store.backend(for: tab)
        if activeBackend !== backend {
            activeBackend = backend
            addressText = tab.url == "about:newtab" ? "" : tab.url
            if tab.url == "about:newtab" || tab.url.isEmpty {
                // async: the new tab's toolbar must mount before focus can land
                DispatchQueue.main.async { addressFocused = true }
            }
        }
    }

    /// Transient popup-blocked notice (T030), auto-dismissing.
    @ViewBuilder private var popupNotice: some View {
        if let notice = bridge.popupNotice {
            Text(notice)
                .font(.callout)
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
                .background(.regularMaterial, in: Capsule())
                .padding(.bottom, 16)
                .transition(.move(edge: .bottom).combined(with: .opacity))
                .task {
                    try? await Task.sleep(nanoseconds: 3_000_000_000)
                    bridge.popupNotice = nil
                }
        }
    }

    private func submitAddress() {
        guard let backend = activeBackend,
              let url = bridge.navigate(tabId: backend.tabId, input: addressText)
        else { return }
        addressFocused = false
        backend.navigate(to: url)
    }

    /// One-click open from the bookmarks bar (T057).
    private func openBookmark(_ url: String, backend: WebKitBackend) {
        guard let resolved = bridge.navigate(tabId: backend.tabId, input: url) else { return }
        backend.navigate(to: resolved)
    }
}

/// Captures the hosting NSWindow and configures it for a Chrome-style full-height tab strip.
private struct WindowAccessor: NSViewRepresentable {
    @Binding var window: NSWindow?

    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async { configure(view.window) }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        if window == nil { DispatchQueue.main.async { configure(nsView.window) } }
    }

    private func configure(_ win: NSWindow?) {
        guard let win else { return }
        // fullSizeContentView extends our SwiftUI content into the titlebar region.
        win.styleMask.insert(.fullSizeContentView)
        win.titlebarAppearsTransparent = true
        win.titleVisibility = .hidden
        window = win
    }
}
