import Foundation
import SwiftUI

/// T034: multi-window coordination — moving tabs between windows (the
/// drag-out / attach flows) on top of the core's `move_tab`/`reorder_tab`.
@MainActor
enum WindowManager {
    /// Detach `tabId` into a brand-new window. Creates the core window,
    /// moves the tab across, drops the placeholder tab the new window was
    /// born with, and returns the new window id for `openWindow`.
    static func moveTabToNewWindow(_ tabId: String, bridge: CoreBridge) -> String? {
        guard let newWindowId = bridge.registerWindow(
            frame: CGRect(x: 80, y: 80, width: 1100, height: 760))
        else { return nil }
        let placeholder = bridge.browserState.window(newWindowId)?.tabs.first?.id
        bridge.moveTab(tabId, toWindow: newWindowId, position: 0)
        if let placeholder, placeholder != tabId {
            bridge.closeTab(placeholder)
        }
        bridge.activateTab(tabId)
        return newWindowId
    }

    /// Attach `tabId` at the end of an existing window's strip.
    static func moveTab(_ tabId: String, toWindow windowId: String, bridge: CoreBridge) {
        let count = bridge.browserState.window(windowId)?.tabs.count ?? 0
        bridge.moveTab(tabId, toWindow: windowId, position: count)
    }
}

/// T038: one live `WebKitBackend` per non-suspended tab. Suspending a tab
/// drops its WKWebView entirely (the real memory win); resuming recreates
/// it and reloads the tab's last URL.
@MainActor
final class WebViewStore: ObservableObject {
    @Published private(set) var backends: [String: WebKitBackend] = [:]
    private let bridge: CoreBridge

    init(bridge: CoreBridge) {
        self.bridge = bridge
    }

    /// Live backend for a tab, creating one on first use.
    func backend(for tab: TabInfo) -> WebKitBackend {
        if let existing = backends[tab.id] { return existing }
        let backend = WebKitBackend(bridge: bridge)
        backend.tabId = tab.id
        backends[tab.id] = backend
        // Resume content for restored/suspended/reopened tabs.
        if tab.url != "about:newtab", !tab.url.isEmpty,
           let url = bridge.navigate(tabId: tab.id, input: tab.url) {
            backend.navigate(to: url)
        }
        return backend
    }

    /// Reconcile with the core state: drop web views for tabs that were
    /// closed or suspended (FR-016) and apply mute state (T042).
    func sync(with window: WindowInfo?) {
        guard let window else {
            backends.removeAll()
            return
        }
        let keep = Set(window.tabs.filter { $0.state != "suspended" }.map(\.id))
        for tabId in backends.keys where !keep.contains(tabId) {
            backends.removeValue(forKey: tabId)
        }
        for tab in window.tabs {
            backends[tab.id]?.setPageMuted(tab.audioState == "muted")
        }
    }
}
