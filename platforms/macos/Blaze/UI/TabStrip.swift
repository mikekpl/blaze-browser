import SwiftUI
import AppKit

struct TabStrip: View {
    @EnvironmentObject private var bridge: CoreBridge
    let windowId: String
    @State private var plusHovering = false
    @State private var stripView: NSView?
    @State private var keyMonitor: Any?

    private var window: WindowInfo? { bridge.browserState.window(windowId) }
    private var tabs: [TabInfo] { window?.tabs ?? [] }

    var body: some View {
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
                                windowId: windowId)
                                .id(tab.id)
                        }
                    }
                    .animation(.interactiveSpring(response: 0.28, dampingFraction: 0.72), value: tabs.map(\.id))
                    .padding(.horizontal, 4)
                    .padding(.vertical, 2)
                }
                .onChange(of: window?.activeTabId) { active in
                    if let active {
                        withAnimation(.spring(response: 0.3)) { proxy.scrollTo(active) }
                    }
                }
            }

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
        }
        .frame(height: 34)
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
    @State private var isHovering = false

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
                controlButton("chevron.left", help: "Move Tab Left") { move(by: -1) }
                    .disabled(tabIndex == 0)
                controlButton("chevron.right", help: "Move Tab Right") { move(by: 1) }
                    .disabled(tabIndex == tabCount - 1)
                controlButton("arrow.up.forward.app", help: "Pop Tab into New Window") { popOut() }
                    .disabled(tabCount < 2)
                Button {
                    bridge.closeTab(tab.id)
                } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 8, weight: .bold))
                        .frame(width: 16, height: 16)
                        .background(Circle().fill(Color.primary.opacity(
                            isHovering || isActive ? 0.08 : 0)))
                        .contentShape(Circle())
                }
                .buttonStyle(.plain)
                .opacity(isHovering || isActive ? 1 : 0)
                .help("Close Tab (⌘W)")
            }
        }
        .padding(.horizontal, tab.pinned ? 0 : 8)
        .padding(.vertical, 4)
        .frame(width: tabWidth)
        .background(
            Capsule()
                .fill(isActive
                      ? AnyShapeStyle(Color(nsColor: .controlBackgroundColor))
                      : isHovering
                          ? AnyShapeStyle(Color.primary.opacity(0.06))
                          : AnyShapeStyle(Color.clear))
                .shadow(color: isActive ? .black.opacity(0.18) : .clear,
                        radius: 3, y: 1))
        .overlay(
            Capsule()
                .strokeBorder(
                    isActive
                        ? LinearGradient(
                            colors: [Color.accentColor.opacity(0.55),
                                     Color.accentColor.opacity(0.2)],
                            startPoint: .topLeading, endPoint: .bottomTrailing)
                        : LinearGradient(colors: [.clear], startPoint: .top, endPoint: .bottom),
                    lineWidth: 1))
        .contentShape(Capsule())
        .background(WindowDragBlocker())
        .animation(.easeOut(duration: 0.15), value: isActive)
        .animation(.easeOut(duration: 0.12), value: isHovering)
        .onHover { hovering in isHovering = hovering }
        .onTapGesture { bridge.activateTab(tab.id) }
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

    /// Hover-revealed tab control (move left/right, pop out).
    private func controlButton(
        _ icon: String, help: String, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: 9, weight: .semibold))
                .frame(width: 16, height: 16)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
        .opacity(isHovering || isActive ? 1 : 0)
        .help(help)
    }

    private func move(by delta: Int) {
        guard let index = tabIndex, let window,
              window.tabs.indices.contains(index + delta)
        else { return }
        withAnimation(.interactiveSpring(response: 0.28, dampingFraction: 0.72)) {
            bridge.reorderTab(tab.id, to: index + delta)
        }
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

/// NSView that calls performDrag to move the window — used for the dead zone beside traffic lights.
private struct WindowMoveArea: NSViewRepresentable {
    func makeNSView(context: Context) -> _View { _View() }
    func updateNSView(_ nsView: _View, context: Context) {}
    final class _View: NSView {
        override func mouseDown(with event: NSEvent) {
            window?.performDrag(with: event)
        }
    }
}
