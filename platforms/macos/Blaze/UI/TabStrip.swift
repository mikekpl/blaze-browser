import SwiftUI
import AppKit
import UniformTypeIdentifiers

struct TabStrip: View {
    @EnvironmentObject private var bridge: CoreBridge
    let windowId: String
    @State private var draggingTabId: String?
    @State private var plusHovering = false

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
                                windowId: windowId,
                                draggingTabId: $draggingTabId)
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
        // double-tap anywhere in the strip zooms the window (like macOS title bar)
        .simultaneousGesture(TapGesture(count: 2).onEnded { _ in NSApp.keyWindow?.zoom(nil) })
    }
}

private struct TabItem: View {
    @EnvironmentObject private var bridge: CoreBridge
    let tab: TabInfo
    let isActive: Bool
    let windowId: String
    @Binding var draggingTabId: String?
    @State private var isHovering = false

    private var isDragging: Bool { draggingTabId == tab.id }
    private var window: WindowInfo? { bridge.browserState.window(windowId) }
    // fixed widths — pinned tabs show only favicon
    private var tabWidth: CGFloat { tab.pinned ? 36 : 160 }

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
        .scaleEffect(isDragging ? 1.05 : 1)
        .shadow(color: isDragging ? .black.opacity(0.30) : .clear, radius: 12, y: 6)
        .opacity(isDragging ? 0.88 : 1)
        .zIndex(isDragging ? 1 : 0)
        .animation(.interactiveSpring(response: 0.28, dampingFraction: 0.72), value: isDragging)
        .animation(.easeOut(duration: 0.15), value: isActive)
        .animation(.easeOut(duration: 0.12), value: isHovering)
        .onHover { hovering in isHovering = hovering }
        .onTapGesture { bridge.activateTab(tab.id) }
        .onDrag {
            draggingTabId = tab.id
            return NSItemProvider(object: tab.id as NSString)
        }
        .onDrop(of: [UTType.text], delegate: TabReorderDropDelegate(
            targetTab: tab, windowId: windowId,
            draggingTabId: $draggingTabId, bridge: bridge))
        .contextMenu {
            Button(tab.pinned ? "Unpin Tab" : "Pin Tab") {
                bridge.setPinned(tab.id, pinned: !tab.pinned)
            }
            Button(tab.audioState == "muted" ? "Unmute Tab" : "Mute Tab") {
                bridge.setMuted(tab.id, muted: tab.audioState != "muted")
            }
            Divider()
            Button("Move Tab to New Window") {
                _ = WindowManager.moveTabToNewWindow(tab.id, bridge: bridge)
            }
            .disabled((window?.tabs.count ?? 0) < 2)
            Divider()
            Button("Reopen Closed Tab") {
                bridge.reopenClosedTab(windowId: windowId)
            }
            Button("Close Tab") { bridge.closeTab(tab.id) }
        }
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

/// Drag-to-reorder: entering another tab's frame mid-drag reorders live.
struct TabReorderDropDelegate: DropDelegate {
    let targetTab: TabInfo
    let windowId: String
    @Binding var draggingTabId: String?
    let bridge: CoreBridge

    func dropEntered(info: DropInfo) {
        guard let dragging = draggingTabId, dragging != targetTab.id,
              let window = bridge.browserState.window(windowId),
              let to = window.tabs.firstIndex(where: { $0.id == targetTab.id })
        else { return }
        withAnimation(.interactiveSpring(response: 0.28, dampingFraction: 0.72)) {
            bridge.reorderTab(dragging, to: to)
        }
    }

    func dropUpdated(info: DropInfo) -> DropProposal? {
        DropProposal(operation: .move)
    }

    func performDrop(info: DropInfo) -> Bool {
        draggingTabId = nil
        return true
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
