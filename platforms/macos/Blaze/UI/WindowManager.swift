import AppKit
import SwiftUI
import Combine

/// T034: multi-window coordination — moving tabs between windows (the
/// drag-out / attach flows) on top of the core's `move_tab`/`reorder_tab`.
@MainActor
enum WindowManager {
    /// Detach `tabId` into a brand-new window. Creates the core window,
    /// moves the tab across, drops the placeholder tab the new window was
    /// born with, and returns the new window id for `openWindow`.
    /// With a `topLeft` (screen corner promised by the tear-off preview) the
    /// window opens right there, sized like the window the tab left.
    static func moveTabToNewWindow(_ tabId: String, bridge: CoreBridge,
                                   topLeft: NSPoint? = nil,
                                   sourceWindowId: String? = nil) -> String? {
        var frame = CGRect(x: 80, y: 80, width: 1100, height: 760)
        if let topLeft {
            let size = sourceWindowId.flatMap { hostWindow(for: $0) }?.frame.size ?? frame.size
            frame = windowFrame(topLeft: topLeft, size: size)
        }
        guard let newWindowId = bridge.registerWindow(frame: frame) else { return nil }
        if topLeft != nil { pendingFrames[newWindowId] = frame }
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

    // MARK: - Host windows

    /// Live AppKit window per core window id, weakly held.
    private static let hostWindows = NSMapTable<NSString, NSWindow>(
        keyOptions: .copyIn, valueOptions: .weakMemory)
    /// Frames for drag-out windows, applied once their AppKit window exists.
    private static var pendingFrames: [String: NSRect] = [:]

    static func registerHost(_ window: NSWindow, for windowId: String) {
        hostWindows.setObject(window, forKey: windowId as NSString)
        if let frame = pendingFrames.removeValue(forKey: windowId) {
            window.setFrame(frame, display: true)
        }
    }

    static func hostWindow(for windowId: String) -> NSWindow? {
        hostWindows.object(forKey: windowId as NSString)
    }

    /// Place a freshly created drag-out window before it first draws, so it
    /// doesn't flash at SwiftUI's default position. The frame stays pending
    /// until `registerHost` re-applies it after SwiftUI's own placement.
    static func applyPendingFrame(to window: NSWindow, windowId: String) {
        if let frame = pendingFrames[windowId] { window.setFrame(frame, display: false) }
    }

    // MARK: - Tab drag geometry

    /// Release farther than this from the strip (sideways/below) tears the tab off…
    private static let dropToOpenDistance: CGFloat = 100
    /// …while just this much above it is enough.
    private static let dropToOpenDistanceAbove: CGFloat = 10

    /// The tab strip's frame in screen coordinates (top of the window).
    static func stripFrame(of window: NSWindow) -> NSRect {
        let frame = window.frame
        return NSRect(x: frame.minX, y: frame.maxY - TabStripLayout.height,
                      width: frame.width, height: TabStripLayout.height)
    }

    /// Browser window whose tab strip is under `point`, honouring stacking:
    /// a strip covered by another browser window is not a target.
    private static func stripHit(atScreenPoint point: NSPoint)
        -> (windowId: String, nsWindow: NSWindow)? {
        var idsByWindow: [ObjectIdentifier: String] = [:]
        for case let key as NSString in hostWindows.keyEnumerator() {
            if let win = hostWindows.object(forKey: key) {
                idsByWindow[ObjectIdentifier(win)] = key as String
            }
        }
        for win in NSApp.orderedWindows where win.isVisible {
            guard let id = idsByWindow[ObjectIdentifier(win)] else { continue }
            // pad a little for a forgiving drop zone
            if stripFrame(of: win).insetBy(dx: 0, dy: -6).contains(point) { return (id, win) }
            if win.frame.contains(point) { return nil }
        }
        return nil
    }

    /// True while `point` is over the source window's own strip (live reorder zone).
    static func isOverStrip(ofWindow windowId: String, atScreenPoint point: NSPoint) -> Bool {
        stripHit(atScreenPoint: point)?.windowId == windowId
    }

    /// Another window's strip under `point` and the slot the tab would take,
    /// judged by the floating tab's centre (`tabCenterX`, screen coordinates).
    /// Returns nil over the source window's own strip (that's a plain
    /// reorder) or over no strip at all.
    static func mergeTarget(atScreenPoint point: NSPoint, tabCenterX: CGFloat,
                            excluding sourceWindowId: String, bridge: CoreBridge)
        -> (windowId: String, nsWindow: NSWindow, slot: Int)? {
        guard let hit = stripHit(atScreenPoint: point), hit.windowId != sourceWindowId
        else { return nil }
        let tabs = bridge.browserState.window(hit.windowId)?.tabs ?? []
        // 76 = traffic-light move area (72) + strip HStack spacing (4)
        let xInStrip = tabCenterX - hit.nsWindow.frame.minX - 76
        return (hit.windowId, hit.nsWindow,
                TabStripLayout.insertionIndex(forX: xInStrip, in: tabs))
    }

    /// Whether releasing at `point` is far enough from the source strip to
    /// open a new window: barely above it, or well clear of it otherwise.
    static func isTearOffPoint(_ point: NSPoint, fromWindow windowId: String) -> Bool {
        guard let window = hostWindow(for: windowId) else { return false }
        let strip = stripFrame(of: window)
        if point.y > strip.maxY + dropToOpenDistanceAbove { return true }
        return !strip.insetBy(dx: -dropToOpenDistance, dy: -dropToOpenDistance).contains(point)
    }

    /// Frame for a torn-off window whose top-left corner should sit at
    /// `topLeft`, nudged only as far as needed to stay reachable on screen.
    private static func windowFrame(topLeft: NSPoint, size: NSSize) -> NSRect {
        let probe = NSPoint(x: topLeft.x + TabDragSession.firstTabOrigin.x, y: topLeft.y - 1)
        let screen = NSScreen.screens.first { $0.frame.contains(probe) } ?? NSScreen.main
        guard let visible = screen?.visibleFrame else {
            return NSRect(x: topLeft.x, y: topLeft.y - size.height,
                          width: size.width, height: size.height)
        }
        let width = min(size.width, visible.width)
        let height = min(size.height, visible.height)
        // the strip must stay grabbable: top edge on screen, ≥120pt visible sideways
        let top = min(max(topLeft.y, visible.minY + TabStripLayout.height), visible.maxY)
        let x = min(max(topLeft.x, visible.minX - width + 120), visible.maxX - 120)
        return NSRect(x: x, y: top - height, width: width, height: height)
    }
}

/// T038: one live `WebKitBackend` per non-suspended tab. Suspending a tab
/// drops its WKWebView entirely (the real memory win); resuming recreates
/// it and reloads the tab's last URL.
///
/// App-wide rather than per-window: a tab dragged into another window keeps
/// its live web view (page state, scroll, media) instead of reloading.
@MainActor
final class WebViewStore: ObservableObject {
    static let shared = WebViewStore(bridge: .shared)

    @Published private(set) var backends: [String: WebKitBackend] = [:]
    private let bridge: CoreBridge
    private var stateObserver: AnyCancellable?

    init(bridge: CoreBridge) {
        self.bridge = bridge
        // Reconcile off the core state directly so web views are reaped even
        // when the window that showed them is already gone.
        stateObserver = bridge.$browserState
            .receive(on: DispatchQueue.main)
            .sink { [weak self] state in self?.sync(with: state) }
    }

    /// Live backend for a tab, creating one on first use.
    func backend(for tab: TabInfo) -> WebKitBackend {
        if let existing = backends[tab.id] { return existing }
        let backend = WebKitBackend(bridge: bridge)
        backend.tabId = tab.id
        backends[tab.id] = backend
        // Resume content for restored/suspended/reopened tabs.
        if !tab.isEmpty,
           let url = bridge.navigate(tabId: tab.id, input: tab.url) {
            backend.navigate(to: url)
        }
        return backend
    }

    /// Reconcile with the core state: drop web views for tabs that were
    /// closed or suspended in any window (FR-016) and apply mute state (T042).
    func sync(with state: BrowserState) {
        let tabs = state.windows.flatMap(\.tabs)
        let keep = Set(tabs.filter { $0.state != "suspended" }.map(\.id))
        for tabId in backends.keys.filter({ !keep.contains($0) }) {
            backends[tabId]?.teardown()
            backends.removeValue(forKey: tabId)
        }
        for tab in tabs {
            backends[tab.id]?.setPageMuted(tab.audioState == "muted")
        }
    }
}
