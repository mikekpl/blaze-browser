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
    private let store = WebViewStore.shared
    @State private var windowId: String?
    @State private var activeBackend: WebKitBackend?
    @State private var addressText: String = ""
    @State private var hostWindow: NSWindow?
    @FocusState private var addressFocused: Bool

    init(requestedWindowId: String? = nil) {
        self.requestedWindowId = requestedWindowId
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
                TabContent(backend: backend, showsNewTabPage: activeTab?.isEmpty ?? true)
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
        .background(WindowAccessor(window: $hostWindow, windowId: requestedWindowId))
        .onAppear(perform: bindWindow)
        .onChange(of: bridge.browserState) { _ in syncWithState() }
        .onChange(of: hostWindow) { win in
            // cross-window tab drops hit-test against this registry
            if let win, let windowId { WindowManager.registerHost(win, for: windowId) }
        }
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

    /// Reconcile with core state: show the active tab's web view, resuming it
    /// if it was suspended. `WebViewStore` reaps closed/suspended ones (FR-016).
    private func syncWithState() {
        // last tab closed → core window is gone; close the AppKit window too
        if let windowId, bridge.browserState.window(windowId) == nil {
            hostWindow?.close()
            return
        }
        if let windowId, let hostWindow {
            WindowManager.registerHost(hostWindow, for: windowId)
        }
        guard let tab = activeTab else {
            activeBackend = nil
            return
        }
        let backend = store.backend(for: tab)
        if activeBackend !== backend {
            activeBackend = backend
            addressText = tab.isEmpty ? "" : tab.url
            if tab.isEmpty {
                // async: the new tab's toolbar must mount before focus can land
                DispatchQueue.main.async { addressFocused = true }
            }
        }
    }

    /// Transient blocked-popup/redirect notice (T030), auto-dismissing. Only
    /// the window the attempt came from shows it, and it never takes focus;
    /// "Open" is the one way a blocked popup gets to open — the user's call.
    @ViewBuilder private var popupNotice: some View {
        if let notice = bridge.popupNotice,
           notice.tabId.isEmpty || window?.tabs.contains(where: { $0.id == notice.tabId }) == true {
            HStack(spacing: 10) {
                Image(systemName: "hand.raised.fill")
                    .foregroundStyle(.secondary)
                Text(notice.message)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if let url = notice.url {
                    Button("Open") {
                        bridge.popupNotice = nil
                        bridge.createTab(nextTo: notice.tabId, url: url)
                    }
                    .buttonStyle(.link)
                }
            }
            .font(.callout)
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
            .background(.regularMaterial, in: Capsule())
            .frame(maxWidth: 520)
            .padding(.bottom, 16)
            .transition(.move(edge: .bottom).combined(with: .opacity))
            .task(id: notice) {
                try? await Task.sleep(nanoseconds: 5_000_000_000)
                if bridge.popupNotice == notice { bridge.popupNotice = nil }
            }
        }
    }

    private func submitAddress() {
        // about:blank / about:newtab empty the tab rather than being
        // navigated to (the core rejects the about: scheme as dangerous)
        if let backend = activeBackend, !addressText.isEmpty,
           TabInfo.isEmptyURL(addressText) {
            addressText = ""
            backend.showBlank()
            return
        }
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

/// The active tab's page area: web view, with the new-tab scenery over an
/// empty tab and the friendly error page over a failed load. Observes the
/// backend itself so load/error changes re-render without the whole window.
private struct TabContent: View {
    @ObservedObject var backend: WebKitBackend
    let showsNewTabPage: Bool

    var body: some View {
        ZStack {
            WebViewContainer(backend: backend)
                .id(backend.tabId) // swap NSView when the active tab changes
            if let model = backend.errorPage {
                ErrorPageView(model: model) {
                    backend.errorPage = nil
                    backend.reload()
                }
            } else if showsNewTabPage, !backend.isLoading {
                NewTabPage(seed: backend.tabId)
                    .transition(.opacity)
            }
        }
        .overlay(alignment: .top) {
            if let notice = backend.drmNotice {
                DRMNoticeView(message: notice) { backend.drmNotice = nil }
                    .padding(.top, 12)
                    .transition(.move(edge: .top).combined(with: .opacity))
            }
        }
    }
}

/// Captures the hosting NSWindow and configures it for a Chrome-style full-height tab strip.
private struct WindowAccessor: NSViewRepresentable {
    @Binding var window: NSWindow?
    /// Known up front only for drag-out / restored windows.
    let windowId: String?

    func makeNSView(context: Context) -> NSView {
        let view = _View()
        view.windowId = windowId
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
        win.isMovable = false
        window = win
    }

    final class _View: NSView {
        var windowId: String?
        // a window torn off by a tab drag opens where the tab was dropped
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if let window, let windowId {
                WindowManager.applyPendingFrame(to: window, windowId: windowId)
            }
        }
    }
}
