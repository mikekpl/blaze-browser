import SwiftUI
import AppKit

struct TabStrip: View {
    @EnvironmentObject private var bridge: CoreBridge
    let windowId: String
    @State private var plusHovering = false
    @State private var draggingTabId: String?
    @State private var stripView: NSView?
    @State private var keyMonitor: Any?

    private var window: WindowInfo? { bridge.browserState.window(windowId) }
    private var tabs: [TabInfo] { window?.tabs ?? [] }
    private static let reservedChromeWidth: CGFloat = 72 + 22 + 8 + 4 * 3
    
    private var tabsContentWidth: CGFloat {
        tabs.reduce(CGFloat(8)) { $0 + ($1.pinned ? 36 : 220) }
            + CGFloat(max(0, tabs.count - 1)) * 4
    }

    var body: some View {
        GeometryReader { geo in
        HStack(spacing: 4) {
            // Traffic light clearance — this area also moves the window on drag.
            WindowMoveArea().frame(width: 72)

            ScrollViewReader { proxy in
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 4) {
                        ForEach(tabs) { tab in
                            TabItem(
                                tab: tab,
                                isActive: window?.activeTabId == tab.id,
                                windowId: windowId,
                                draggingTabId: $draggingTabId)
                                .id(tab.id)
                        }
                    }
                    .animation(.interactiveSpring(response: 0.28, dampingFraction: 0.72), value: tabs.map(\.id))
                    .padding(.horizontal, 4)
                    .padding(.vertical, 3)
                    .coordinateSpace(name: "TabStrip")
                }
                .onChange(of: window?.activeTabId) { active in
                    if let active {
                        withAnimation(.spring(response: 0.3)) { proxy.scrollTo(active) }
                    }
                }
            }
            // hug the tabs so leftover chrome stays a window-drag area
            .frame(width: min(tabsContentWidth, max(0, geo.size.width - Self.reservedChromeWidth)))

            Button {
                bridge.createTab(windowId: windowId)
            } label: {
                Image(systemName: "plus")
                    .font(.system(size: 11, weight: .semibold))
                    .frame(width: 22, height: 22)
                    .background(
                        Circle().fill(plusHovering
                            ? Color.primary.opacity(0.09)
                            : Color.clear))
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .onHover { hovering in
                withAnimation(.easeOut(duration: 0.12)) { plusHovering = hovering }
            }
            .help("New Tab (⌘T)")
            .padding(.trailing, 8)

            WindowMoveArea().frame(maxWidth: .infinity)
        }
        .frame(width: geo.size.width, height: geo.size.height)
        }
        .frame(height: 38)
        .background(
            LinearGradient(
                colors: [
                    Color(nsColor: .windowBackgroundColor),
                    Color(nsColor: .underPageBackgroundColor).opacity(0.6),
                ],
                startPoint: .top, endPoint: .bottom))
        .background(StripFrameReader { stripView = $0 })
        // double-tap anywhere in the strip zooms the window (like macOS title bar)
        .simultaneousGesture(TapGesture(count: 2).onEnded { _ in NSApp.keyWindow?.zoom(nil) })
        .onAppear(perform: installKeyMonitor)
        .onDisappear {
            if let keyMonitor { NSEvent.removeMonitor(keyMonitor) }
            keyMonitor = nil
        }
    }

    /// ⌘←/⌘→ re-arranges the active tab; skipped while editing text so the
    /// address bar keeps its line-start/line-end behavior.
    private func installKeyMonitor() {
        guard keyMonitor == nil else { return }
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            guard event.keyCode == 123 || event.keyCode == 124,  // ← / →
                  event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command,
                  let view = stripView, view.window === NSApp.keyWindow,
                  !(view.window?.firstResponder is NSTextView)
            else { return event }
            moveActiveTab(by: event.keyCode == 123 ? -1 : 1)
            return nil
        }
    }

    private func moveActiveTab(by delta: Int) {
        guard let window else { return }
        let activeId = window.activeTabId
        guard let index = window.tabs.firstIndex(where: { $0.id == activeId }),
              window.tabs.indices.contains(index + delta)
        else { return }
        withAnimation(.interactiveSpring(response: 0.28, dampingFraction: 0.72)) {
            bridge.reorderTab(activeId, to: index + delta)
        }
    }
}

private struct TabItem: View {
    @EnvironmentObject private var bridge: CoreBridge
    @Environment(\.openWindow) private var openWindow
    let tab: TabInfo
    let isActive: Bool
    let windowId: String
    @Binding var draggingTabId: String?
    @State private var isHovering = false
    @State private var grabOffsetX: CGFloat = 0
    @State private var dragOffsetX: CGFloat = 0
    @State private var isDetaching = false

    private static let detachDistance: CGFloat = 48

    private var isDragging: Bool { draggingTabId == tab.id }
    private var window: WindowInfo? { bridge.browserState.window(windowId) }
    private var tabIndex: Int? { window?.tabs.firstIndex { $0.id == tab.id } }
    private var tabCount: Int { window?.tabs.count ?? 0 }
    // fixed widths — pinned tabs show only favicon
    private var tabWidth: CGFloat { tab.pinned ? 36 : 220 }

    var body: some View {
        HStack(spacing: 5) {
            if tab.pinned {
                favicon.frame(maxWidth: .infinity)
            } else {
                favicon
            }
            if !tab.pinned, tab.audioState == "audible" || tab.audioState == "muted" {
                Button {
                    bridge.setMuted(tab.id, muted: tab.audioState != "muted")
                } label: {
                    Image(systemName: tab.audioState == "muted"
                          ? "speaker.slash.fill" : "speaker.wave.2.fill")
                        .font(.system(size: 9))
                        .foregroundStyle(Color.accentColor)
                }
                .buttonStyle(.borderless)
                .help(tab.audioState == "muted" ? "Unmute Tab" : "Mute Tab")
            }
            if !tab.pinned {
                Text(displayTitle)
                    .font(.callout.weight(isActive ? .medium : .regular))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .foregroundStyle(isActive ? .primary : .secondary)
                    .opacity(tab.state == "suspended" ? 0.5 : 1)
                Spacer(minLength: 0)
                Button {
                    bridge.closeTab(tab.id)
                } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 8, weight: .bold))
                        .frame(width: 16, height: 16)
                        .background(RoundedRectangle(cornerRadius: 4).fill(
                            Color.primary.opacity(isHovering || isActive ? 0.08 : 0)))
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Close Tab (⌘W)")
            }
        }
        .padding(.horizontal, tab.pinned ? 0 : 8)
        .frame(width: tabWidth, height: 30)
        .background(
            RoundedRectangle(cornerRadius: 6, style: .continuous)
                .fill(isActive
                      ? AnyShapeStyle(Color(nsColor: .controlBackgroundColor))
                      : isHovering
                          ? AnyShapeStyle(Color.primary.opacity(0.07))
                          : AnyShapeStyle(Color.clear))
                .shadow(color: isActive ? .black.opacity(0.15) : .clear,
                        radius: 2, y: 1))
        .contentShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
        .background(WindowDragBlocker())
        .offset(x: isDragging ? dragOffsetX : 0)
        .scaleEffect(isDragging ? 1.02 : 1)
        .shadow(color: isDragging ? .black.opacity(0.25) : .clear, radius: 8, y: 3)
        .opacity(isDetaching ? 0.6 : 1)
        .zIndex(isDragging ? 1 : 0)
        .animation(.easeOut(duration: 0.15), value: isActive)
        .animation(.easeOut(duration: 0.12), value: isHovering)
        .onHover { hovering in isHovering = hovering }
        .onTapGesture { bridge.activateTab(tab.id) }
        .gesture(dragGesture)
        .contextMenu {
            Button(tab.pinned ? "Unpin Tab" : "Pin Tab") {
                bridge.setPinned(tab.id, pinned: !tab.pinned)
            }
            Button(tab.audioState == "muted" ? "Unmute Tab" : "Mute Tab") {
                bridge.setMuted(tab.id, muted: tab.audioState != "muted")
            }
            Divider()
            Button("Move Tab to New Window") { popOut() }
            .disabled(tabCount < 2)
            Divider()
            Button("Reopen Closed Tab") {
                bridge.reopenClosedTab(windowId: windowId)
            }
            Button("Close Tab") { bridge.closeTab(tab.id) }
            Button("Close All Other Tabs") {
                for other in window?.tabs ?? [] where other.id != tab.id {
                    bridge.closeTab(other.id)
                }
            }
            .disabled(tabCount < 2)
        }
    }

    /// Live drag: the tab follows the pointer, reorders as it crosses slot
    /// boundaries, and releasing ≥ detachDistance above/below the strip pops
    /// it into a new window.
    private var dragGesture: some Gesture {
        DragGesture(minimumDistance: 4, coordinateSpace: .named("TabStrip"))
            .onChanged { value in
                guard let window else { return }
                if draggingTabId != tab.id {
                    draggingTabId = tab.id
                    bridge.activateTab(tab.id)
                    grabOffsetX = value.startLocation.x
                        - minX(at: tabIndex ?? 0, in: window.tabs)
                }
                let desiredMinX = value.location.x - grabOffsetX
                let target = slotIndex(forCenter: desiredMinX + tabWidth / 2,
                                       in: window.tabs)
                if let index = tabIndex, target != index {
                    withAnimation(.interactiveSpring(response: 0.28, dampingFraction: 0.72)) {
                        bridge.reorderTab(tab.id, to: target)
                    }
                }
                // recompute against post-reorder state so the tab keeps tracking the pointer
                if let tabs = self.window?.tabs, let index = tabIndex {
                    dragOffsetX = desiredMinX - minX(at: index, in: tabs)
                }
                isDetaching = abs(value.translation.height) > Self.detachDistance
            }
            .onEnded { value in
                let shouldDetach = abs(value.translation.height) > Self.detachDistance
                    && tabCount > 1
                withAnimation(.interactiveSpring(response: 0.28, dampingFraction: 0.72)) {
                    dragOffsetX = 0
                    isDetaching = false
                }
                draggingTabId = nil
                if shouldDetach { popOut() }
            }
    }

    /// Leading x of the tab slot at `index` in strip coordinates.
    private func minX(at index: Int, in tabs: [TabInfo]) -> CGFloat {
        var x: CGFloat = 4  // strip leading padding
        for i in 0..<min(index, tabs.count) {
            x += (tabs[i].pinned ? CGFloat(36) : CGFloat(220)) + 4
        }
        return x
    }

    /// Slot whose bounds contain `centerX`.
    private func slotIndex(forCenter centerX: CGFloat, in tabs: [TabInfo]) -> Int {
        var x: CGFloat = 4
        for (i, t) in tabs.enumerated() {
            let width = (t.pinned ? CGFloat(36) : CGFloat(220)) + 4
            if centerX < x + width { return i }
            x += width
        }
        return max(0, tabs.count - 1)
    }

    private func popOut() {
        guard let newWindowId = WindowManager.moveTabToNewWindow(tab.id, bridge: bridge)
        else { return }
        openWindow(value: newWindowId)
    }

    private var displayTitle: String {
        if !tab.title.isEmpty { return tab.title }
        if tab.url == "about:newtab" || tab.url.isEmpty { return "New Tab" }
        return URL(string: tab.url)?.host ?? tab.url
    }

    /// Site favicon fetched from /favicon.ico (same-origin, no third parties).
    @ViewBuilder private var favicon: some View {
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
        guard let url = URL(string: tab.url),
              let scheme = url.scheme, scheme.hasPrefix("http"),
              let host = url.host
        else { return nil }
        return URL(string: "\(scheme)://\(host)/favicon.ico")
    }
}

/// Reports the strip's backing NSView so key events can be scoped to its window.
private struct StripFrameReader: NSViewRepresentable {
    let onCapture: (NSView) -> Void
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async { onCapture(view) }
        return view
    }
    func updateNSView(_ nsView: NSView, context: Context) {}
}

/// AppKit blocker under each tab: stops titlebar window-dragging there so
/// clicks on a tab never move the window. Events still bubble to SwiftUI gestures.
private struct WindowDragBlocker: NSViewRepresentable {
    func makeNSView(context: Context) -> _View { _View() }
    func updateNSView(_ nsView: _View, context: Context) {}
    final class _View: NSView {
        override var mouseDownCanMoveWindow: Bool { false }
    }
}

private struct WindowMoveArea: NSViewRepresentable {
    func makeNSView(context: Context) -> _View { _View() }
    func updateNSView(_ nsView: _View, context: Context) {}
    final class _View: NSView {
        override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
        override func mouseDown(with event: NSEvent) {
            if event.clickCount == 2 {
                window?.zoom(nil)
            } else {
                window?.performDrag(with: event)
            }
        }
    }
}
