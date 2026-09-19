import SwiftUI
import AppKit

struct TabStrip: View {
    @EnvironmentObject private var bridge: CoreBridge
    @ObservedObject private var drag = TabDragSession.shared
    let windowId: String
    @State private var plusHovering = false
    @State private var draggingTabId: String?
    @State private var stripView: NSView?
    @State private var keyMonitor: Any?

    private var window: WindowInfo? { bridge.browserState.window(windowId) }
    private var tabs: [TabInfo] { window?.tabs ?? [] }
    private static let reservedChromeWidth: CGFloat = 72 + 22 + 8 + 4 * 3
    
    /// Tabs as laid out right now: a tab being dragged out of the window has
    /// already given up its slot, a tab hovering in from another has one open.
    private var tabsContentWidth: CGFloat {
        let staying = tabs.filter { !drag.isLeaving($0.id) }
        var width = staying.reduce(CGFloat(8)) { $0 + TabStripLayout.width(of: $1) }
            + CGFloat(max(0, staying.count - 1)) * TabStripLayout.spacing
        if let incoming { width += incoming.width + TabStripLayout.spacing }
        return width
    }

    /// Drop target when a tab from another window hovers over this strip.
    private var incoming: TabDragSession.DropTarget? {
        drag.dropTarget.flatMap { $0.windowId == windowId ? $0 : nil }
    }

    var body: some View {
        GeometryReader { geo in
        HStack(spacing: 4) {
            // Traffic light clearance — this area also moves the window on drag.
            WindowMoveArea().frame(width: 72)

            ScrollViewReader { proxy in
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 4) {
                        ForEach(Array(tabs.enumerated()), id: \.element.id) { index, tab in
                            TabItem(
                                tab: tab,
                                isActive: window?.activeTabId == tab.id,
                                windowId: windowId,
                                isLeaving: drag.isLeaving(tab.id),
                                draggingTabId: $draggingTabId)
                                // make room for a tab hovering in from another window
                                .offset(x: incoming.map { index >= $0.slot
                                    ? $0.width + TabStripLayout.spacing : 0 } ?? 0)
                                .id(tab.id)
                        }
                    }
                    .animation(.interactiveSpring(response: 0.28, dampingFraction: 0.72), value: tabs.map(\.id))
                    .padding(.horizontal, 4)
                    .padding(.vertical, 3)
                    .coordinateSpace(name: "TabStrip")
                    .overlay(alignment: .leading) { incomingSlot }
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

    /// The opened gap where a tab dragged in from another window will land.
    @ViewBuilder private var incomingSlot: some View {
        if let incoming {
            RoundedRectangle(cornerRadius: 6, style: .continuous)
                .fill(Color.accentColor.opacity(0.12))
                .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .strokeBorder(Color.accentColor.opacity(0.7),
                                  style: StrokeStyle(lineWidth: 1.5, dash: [5, 4])))
                .frame(width: incoming.width, height: TabStripLayout.tabHeight)
                .offset(x: TabStripLayout.minX(at: incoming.slot, in: tabs))
                .allowsHitTesting(false)
                .transition(.opacity)
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
    /// Mid-drag and releasing now would move the tab out of this window:
    /// it gives up its slot so the strip shows what's about to happen.
    let isLeaving: Bool
    @Binding var draggingTabId: String?
    @State private var isHovering = false
    @State private var grabOffsetX: CGFloat = 0

    private var drag: TabDragSession { .shared }
    private var isDragging: Bool { draggingTabId == tab.id }
    private var window: WindowInfo? { bridge.browserState.window(windowId) }
    private var tabIndex: Int? { window?.tabs.firstIndex { $0.id == tab.id } }
    private var tabCount: Int { window?.tabs.count ?? 0 }
    private var tabWidth: CGFloat { TabStripLayout.width(of: tab) }
    /// Like DuckDuckGo: pinned tabs only reorder, and a window's only tab
    /// can join another window but can't be torn off into a new one.
    private var canLeaveWindow: Bool { !tab.pinned }
    private var canOpenNewWindow: Bool { !tab.pinned && tabCount > 1 }

    var body: some View {
        HStack(spacing: 5) {
            if tab.pinned {
                TabFavicon(pageURL: tab.url).frame(maxWidth: .infinity)
            } else {
                TabFavicon(pageURL: tab.url)
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
        // the floating snapshot carries the drag; the real tab holds its slot
        // until it is headed out of the window, then folds away (the view
        // stays mounted — it owns the gesture)
        .opacity(isLeaving ? 0 : isDragging ? 0.6 : 1)
        .frame(width: isLeaving ? 0 : tabWidth, alignment: .leading)
        .clipped()
        .padding(.trailing, isLeaving ? -TabStripLayout.spacing : 0)
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
            Button("Move Tab to New Window") { popOut(at: nil) }
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

    private var dragGesture: some Gesture {
        DragGesture(minimumDistance: 4, coordinateSpace: .named("TabStrip"))
            .onChanged { value in
                guard let window else { return }
                if draggingTabId != tab.id {
                    draggingTabId = tab.id
                    bridge.activateTab(tab.id)
                    grabOffsetX = value.startLocation.x
                        - TabStripLayout.minX(at: tabIndex ?? 0, in: window.tabs)
                    let grabY = value.startLocation.y - TabStripLayout.verticalPadding
                    drag.begin(tab: tab, title: displayTitle, grabOffset: CGSize(
                        width: min(max(grabOffsetX, 0), tabWidth),
                        height: min(max(grabY, 0), TabStripLayout.tabHeight)))
                }
                guard !drag.isCancelled else { return }
                let mouse = NSEvent.mouseLocation
                let mergeTarget = canLeaveWindow ? mergeTarget(at: mouse) : nil
                // live reorder only while the pointer rides this window's strip
                if mergeTarget == nil,
                   WindowManager.isOverStrip(ofWindow: windowId, atScreenPoint: mouse) {
                    let center = value.location.x - grabOffsetX + tabWidth / 2
                    let target = TabStripLayout.slotIndex(forCenter: center, in: window.tabs)
                    if let index = tabIndex, target != index {
                        withAnimation(.interactiveSpring(response: 0.28, dampingFraction: 0.72)) {
                            bridge.reorderTab(tab.id, to: target)
                        }
                    }
                }
                if let mergeTarget {
                    drag.update(mouse: mouse, phase: .merging(.init(
                        windowId: mergeTarget.windowId, slot: mergeTarget.slot, width: tabWidth)))
                } else {
                    drag.update(mouse: mouse, phase: isTearOff(at: mouse) ? .tearingOff : .reordering)
                }
            }
            .onEnded { _ in
                let mouse = NSEvent.mouseLocation
                let cancelled = drag.isCancelled
                // decide with the session's geometry before it is torn down
                let mergeTarget = canLeaveWindow ? mergeTarget(at: mouse) : nil
                let newWindowTopLeft = mergeTarget == nil && isTearOff(at: mouse)
                    ? drag.windowTopLeft(forMouse: mouse) : nil
                drag.end()
                draggingTabId = nil
                guard !cancelled else { return }
                if let mergeTarget {
                    merge(into: mergeTarget)
                } else if let newWindowTopLeft {
                    popOut(at: newWindowTopLeft)
                }
            }
    }

    private func mergeTarget(at mouse: NSPoint)
        -> (windowId: String, nsWindow: NSWindow, slot: Int)? {
        WindowManager.mergeTarget(
            atScreenPoint: mouse, tabCenterX: drag.tabCenterX(forMouse: mouse),
            excluding: windowId, bridge: bridge)
    }

    /// Releasing at `mouse` would open this tab in a new window.
    private func isTearOff(at mouse: NSPoint) -> Bool {
        canOpenNewWindow
            && !WindowManager.isOverStrip(ofWindow: windowId, atScreenPoint: mouse)
            && WindowManager.isTearOffPoint(mouse, fromWindow: windowId)
    }

    /// Move the tab into another window's strip at the drop slot and bring
    /// that window to front. The core closes a source window left empty and
    /// the tab's live web view follows it (see `WebViewStore`).
    private func merge(into target: (windowId: String, nsWindow: NSWindow, slot: Int)) {
        withAnimation(.interactiveSpring(response: 0.28, dampingFraction: 0.72)) {
            bridge.moveTab(tab.id, toWindow: target.windowId, position: target.slot)
        }
        bridge.activateTab(tab.id)
        target.nsWindow.makeKeyAndOrderFront(nil)
    }

    /// Tear the tab off into its own window — where the drag preview showed
    /// it, otherwise wherever the system places new windows.
    private func popOut(at topLeft: NSPoint?) {
        guard let newWindowId = WindowManager.moveTabToNewWindow(
            tab.id, bridge: bridge, topLeft: topLeft, sourceWindowId: windowId)
        else { return }
        openWindow(value: newWindowId)
    }

    private var displayTitle: String {
        if !tab.title.isEmpty { return tab.title }
        if tab.url == "about:newtab" || tab.url.isEmpty { return "New Tab" }
        return URL(string: tab.url)?.host ?? tab.url
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
