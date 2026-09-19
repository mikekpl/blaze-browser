import SwiftUI
import AppKit
import WebKit

/// Tab-strip geometry shared by reordering, cross-window drops and the
/// incoming-tab gap. X values are in the strip's "TabStrip" space.
enum TabStripLayout {
    static let height: CGFloat = 38
    static let tabHeight: CGFloat = 30
    static let spacing: CGFloat = 4
    static let leadingPadding: CGFloat = 4
    static let verticalPadding: CGFloat = 3

    // fixed widths — pinned tabs show only favicon
    static func width(of tab: TabInfo) -> CGFloat { tab.pinned ? 36 : 220 }

    /// Leading x of the tab slot at `index`.
    static func minX(at index: Int, in tabs: [TabInfo]) -> CGFloat {
        var x = leadingPadding
        for i in 0..<min(index, tabs.count) {
            x += width(of: tabs[i]) + spacing
        }
        return x
    }

    /// Slot whose bounds contain `centerX` (reordering within a strip).
    static func slotIndex(forCenter centerX: CGFloat, in tabs: [TabInfo]) -> Int {
        var x = leadingPadding
        for (i, t) in tabs.enumerated() {
            let slotWidth = width(of: t) + spacing
            if centerX < x + slotWidth { return i }
            x += slotWidth
        }
        return max(0, tabs.count - 1)
    }

    /// Gap nearest to `x`, 0...count (inserting a tab from another window).
    static func insertionIndex(forX x: CGFloat, in tabs: [TabInfo]) -> Int {
        var minX = leadingPadding
        for (i, t) in tabs.enumerated() {
            if x < minX + width(of: t) / 2 { return i }
            minX += width(of: t) + spacing
        }
        return tabs.count
    }
}

/// One app-wide tab drag. A floating snapshot of the tab follows the pointer
/// across windows and screens; where the drag is released decides reorder /
/// merge / new window, and the snapshot always shows which one it will be:
/// a bare tab while reordering or merging, a miniature window — placed
/// exactly where the real one will open — once releasing would tear it off.
@MainActor
final class TabDragSession: ObservableObject {
    static let shared = TabDragSession()

    struct DropTarget: Equatable {
        let windowId: String
        let slot: Int
        /// Width of the gap the receiving strip opens for the tab.
        let width: CGFloat
    }

    enum Phase: Equatable {
        /// In or near the source strip: releasing keeps the tab in its window.
        case reordering
        /// Over another window's strip: releasing drops the tab into `slot`.
        case merging(DropTarget)
        /// Clear of every strip: releasing opens the tab in a new window.
        case tearingOff
    }

    @Published private(set) var phase: Phase = .reordering
    /// Tab being dragged, nil when idle.
    @Published private(set) var tabId: String?
    /// Page snapshot shown inside the window preview.
    @Published private(set) var pageSnapshot: NSImage?
    /// Escape pressed mid-drag: releasing the mouse then does nothing.
    private(set) var isCancelled = false

    var dropTarget: DropTarget? {
        if case .merging(let target) = phase { return target }
        return nil
    }

    /// The dragged tab is on its way out of its window (strip closes the gap).
    func isLeaving(_ id: String) -> Bool { tabId == id && phase != .reordering }

    // Where a strip puts its first tab, measured from the window's top-left:
    // traffic-light area (72) + HStack spacing (4) + strip padding (4), and
    // the 30pt tab centred in the 38pt strip. The preview and the torn-off
    // window share this origin so the tab never jumps under the pointer.
    static let firstTabOrigin = CGPoint(x: 80, y: 4)
    static let previewSize = CGSize(width: 440, height: 290)
    private static let shadowMargin: CGFloat = 24

    private var panel: NSPanel?
    private var monitors: [Any] = []
    private var grabOffset: CGSize = .zero
    private var tabWidth: CGFloat = 0

    /// `grabOffset` is the pointer position inside the tab (top-left origin),
    /// so the snapshot stays pinned under the pointer where it was grabbed.
    func begin(tab: TabInfo, title: String, grabOffset: CGSize) {
        end()
        isCancelled = false
        self.grabOffset = grabOffset
        tabWidth = TabStripLayout.width(of: tab)
        tabId = tab.id

        let margin = Self.shadowMargin
        let size = NSSize(width: Self.previewSize.width + margin * 2,
                          height: Self.previewSize.height + margin * 2)
        let panel = NSPanel(contentRect: NSRect(origin: .zero, size: size),
                            styleMask: [.borderless, .nonactivatingPanel],
                            backing: .buffered, defer: false)
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.level = .popUpMenu
        panel.ignoresMouseEvents = true
        panel.isReleasedWhenClosed = false
        panel.collectionBehavior = [.transient, .ignoresCycle, .fullScreenAuxiliary]
        panel.contentView = NSHostingView(
            rootView: TabDragPreview(session: self, tab: tab, title: title).padding(margin))
        self.panel = panel
        move(to: NSEvent.mouseLocation)
        panel.orderFrontRegardless()
        capturePageSnapshot(of: tab.id)

        if let esc = NSEvent.addLocalMonitorForEvents(matching: .keyDown, handler: { [weak self] event in
            guard event.keyCode == 53 else { return event }  // Escape
            self?.cancel()
            return nil
        }) { monitors.append(esc) }
        // Safety net: never leave the snapshot on screen if the gesture ends
        // without reporting (deferred so the gesture's own handler runs first).
        if let up = NSEvent.addLocalMonitorForEvents(matching: .leftMouseUp, handler: { [weak self] event in
            DispatchQueue.main.async { self?.end() }
            return event
        }) { monitors.append(up) }
    }

    func update(mouse: NSPoint, phase: Phase) {
        guard !isCancelled else { return }
        move(to: mouse)
        if self.phase != phase {
            withAnimation(.spring(response: 0.3, dampingFraction: 0.8)) { self.phase = phase }
        }
    }

    func end() {
        monitors.forEach(NSEvent.removeMonitor)
        monitors.removeAll()
        panel?.orderOut(nil)
        panel = nil
        if pageSnapshot != nil { pageSnapshot = nil }
        if tabId != nil { tabId = nil }
        if phase != .reordering {
            withAnimation(.spring(response: 0.3, dampingFraction: 0.8)) { phase = .reordering }
        }
    }

    /// Screen x of the floating tab's centre — what strips hit-test slots with.
    func tabCenterX(forMouse mouse: NSPoint) -> CGFloat {
        mouse.x - grabOffset.width + tabWidth / 2
    }

    /// Top-left screen corner of the window a tear-off at `mouse` opens: the
    /// one that leaves its first tab under the pointer exactly as grabbed.
    func windowTopLeft(forMouse mouse: NSPoint) -> NSPoint {
        NSPoint(x: mouse.x - grabOffset.width - Self.firstTabOrigin.x,
                y: mouse.y + grabOffset.height + Self.firstTabOrigin.y)
    }

    private func cancel() {
        isCancelled = true
        panel?.orderOut(nil)
        if phase != .reordering {
            withAnimation(.spring(response: 0.3, dampingFraction: 0.8)) { phase = .reordering }
        }
    }

    private func move(to mouse: NSPoint) {
        guard let panel else { return }
        let topLeft = windowTopLeft(forMouse: mouse)
        panel.setFrameOrigin(NSPoint(
            x: topLeft.x - Self.shadowMargin,
            y: topLeft.y + Self.shadowMargin - panel.frame.height))
    }

    private func capturePageSnapshot(of tabId: String) {
        guard let webView = WebViewStore.shared.backends[tabId]?.webView,
              webView.bounds.width > 0 else { return }
        let config = WKSnapshotConfiguration()
        config.snapshotWidth = NSNumber(value: Double(Self.previewSize.width))
        webView.takeSnapshot(with: config) { [weak self] image, _ in
            DispatchQueue.main.async {
                guard let self, self.tabId == tabId else { return }
                self.pageSnapshot = image
            }
        }
    }
}

/// Site favicon fetched from /favicon.ico (same-origin, no third parties).
struct TabFavicon: View {
    let pageURL: String

    var body: some View {
        if let url = faviconURL {
            AsyncImage(url: url) { phase in
                if let image = phase.image {
                    image.resizable().interpolation(.medium)
                } else {
                    Image(systemName: "globe")
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                }
            }
            .frame(width: 14, height: 14)
            .clipShape(RoundedRectangle(cornerRadius: 3))
        }
    }

    private var faviconURL: URL? {
        guard let url = URL(string: pageURL),
              let scheme = url.scheme, scheme.hasPrefix("http"),
              let host = url.host
        else { return nil }
        return URL(string: "\(scheme)://\(host)/favicon.ico")
    }
}

/// The floating snapshot. The tab chip sits where a strip would put a
/// window's first tab; while tearing off, a miniature browser window grows
/// around it so it is unmistakable that releasing opens a new window.
private struct TabDragPreview: View {
    @ObservedObject var session: TabDragSession
    let tab: TabInfo
    let title: String

    private var isWindow: Bool { session.phase == .tearingOff }

    var body: some View {
        ZStack(alignment: .topLeading) {
            windowBody
                .opacity(isWindow ? 1 : 0)
                .scaleEffect(isWindow ? 1 : 0.7, anchor: UnitPoint(
                    x: (TabDragSession.firstTabOrigin.x + TabStripLayout.width(of: tab) / 2)
                        / TabDragSession.previewSize.width,
                    y: 0))
            chip.offset(x: TabDragSession.firstTabOrigin.x, y: TabDragSession.firstTabOrigin.y)
        }
        .frame(width: TabDragSession.previewSize.width,
               height: TabDragSession.previewSize.height, alignment: .topLeading)
    }

    private var chip: some View {
        HStack(spacing: 5) {
            if tab.pinned {
                TabFavicon(pageURL: tab.url).frame(maxWidth: .infinity)
            } else {
                TabFavicon(pageURL: tab.url)
                Text(title)
                    .font(.callout.weight(.medium))
                    .lineLimit(1)
                    .truncationMode(.tail)
                Spacer(minLength: 0)
            }
        }
        .padding(.horizontal, tab.pinned ? 0 : 8)
        .frame(width: TabStripLayout.width(of: tab), height: TabStripLayout.tabHeight)
        .background(
            RoundedRectangle(cornerRadius: 6, style: .continuous)
                .fill(Color(nsColor: .controlBackgroundColor))
                // lifted while loose, seated like an active tab inside the window
                .shadow(color: .black.opacity(isWindow ? 0.15 : 0.3),
                        radius: isWindow ? 2 : 8, y: isWindow ? 1 : 3))
        .opacity(isWindow ? 1 : 0.95)
    }

    // explicit colours throughout: system dividers and materials don't
    // render dependably inside a transparent, non-activating panel
    private var hairline: some View {
        Color.primary.opacity(0.12).frame(height: 1)
    }

    private var windowBody: some View {
        VStack(spacing: 0) {
            // tab strip with traffic lights
            HStack(spacing: 8) {
                ForEach([Color.red, .yellow, .green], id: \.self) { color in
                    Circle().fill(color.opacity(0.85)).frame(width: 12, height: 12)
                }
                Spacer(minLength: 0)
            }
            .padding(.leading, 13)
            .frame(height: TabStripLayout.height)
            .background(LinearGradient(
                colors: [Color(nsColor: .windowBackgroundColor),
                         Color(nsColor: .underPageBackgroundColor).opacity(0.6)],
                startPoint: .top, endPoint: .bottom))
            hairline
            // toolbar with the address
            HStack {
                Text(URL(string: tab.url)?.host ?? "")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .padding(.horizontal, 10)
                    .frame(maxWidth: .infinity, minHeight: 20, alignment: .leading)
                    .background(Capsule().fill(Color.primary.opacity(0.06)))
            }
            .padding(.horizontal, 12)
            .frame(height: 32)
            .background(Color(nsColor: .windowBackgroundColor))
            hairline
            // page
            ZStack(alignment: .bottom) {
                Color(nsColor: .underPageBackgroundColor)
                if let snapshot = session.pageSnapshot {
                    Image(nsImage: snapshot)
                        .resizable()
                        .aspectRatio(contentMode: .fill)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                }
                Label("New Window", systemImage: "macwindow.badge.plus")
                    .font(.callout.weight(.medium))
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                    .background(Capsule().fill(Color(nsColor: .windowBackgroundColor))
                        .shadow(color: .black.opacity(0.2), radius: 3, y: 1))
                    .padding(.bottom, 14)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .clipped()
        }
        .background(Color(nsColor: .windowBackgroundColor))  // opaque behind the hairlines
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous)
            .strokeBorder(Color.primary.opacity(0.18)))
        .compositingGroup()  // one shadow for the window, not one per subview
        .shadow(color: .black.opacity(0.35), radius: 16, y: 8)
    }
}
