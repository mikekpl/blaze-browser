import SwiftUI

/// Browser toolbar (T027): back/forward/reload, address bar with HTTPS
/// indicator, shield button with live counters.
struct Toolbar: View {
    @EnvironmentObject private var bridge: CoreBridge
    @ObservedObject var backend: WebKitBackend
    @Binding var addressText: String
    var addressFocused: FocusState<Bool>.Binding
    var onSubmit: () -> Void
    @State private var showShields = false
    @State private var showDownloads = false
    @State private var bookmarkConfirmation: String?
    @State private var toolbarWidth: CGFloat = 800

    /// Collapse secondary controls into an overflow menu at narrow widths (T061).
    private var isCompact: Bool { toolbarWidth < 560 }

    private var currentBookmark: BookmarkItem? {
        bridge.bookmark(for: backend.currentURL)
    }

    private var hasActiveDownload: Bool {
        bridge.downloads.contains { $0.state == "active" }
    }

    private var host: String {
        URL(string: backend.currentURL)?.host ?? ""
    }

    private var isSecure: Bool {
        backend.currentURL.hasPrefix("https://")
    }

    private var shieldCount: Int {
        let stats = bridge.shieldStats[backend.tabId] ?? ShieldStats()
        return stats.adsBlocked + stats.trackersBlocked
    }

    var body: some View {
        HStack(spacing: 8) {
            navigationButtons
            addressField
            if isCompact {
                overflowMenu
            } else {
                bookmarkButton
                shieldButton
                downloadButton
            }
            if backend.isLoading {
                ProgressView()
                    .controlSize(.small)
            }
        }
        .buttonStyle(.borderless)
        .padding(8)
        .background(
            GeometryReader { proxy in
                Color.clear
                    .onAppear { toolbarWidth = proxy.size.width }
                    .onChange(of: proxy.size.width) { toolbarWidth = $0 }
            })
    }

    private var navigationButtons: some View {
        Group {
            Button(action: backend.goBack) {
                Image(systemName: "chevron.left")
            }
            .help("Back")
            Button(action: backend.goForward) {
                Image(systemName: "chevron.right")
            }
            .help("Forward")
            Button(action: backend.isLoading ? backend.stop : backend.reload) {
                Image(systemName: backend.isLoading ? "xmark" : "arrow.clockwise")
            }
            .help(backend.isLoading ? "Stop" : "Reload")
        }
    }

    private var addressField: some View {
        HStack(spacing: 6) {
            if !backend.currentURL.isEmpty {
                Image(systemName: isSecure ? "lock.fill" : "lock.open")
                    .font(.caption)
                    .foregroundStyle(isSecure ? Color.secondary : Color.orange)
                    .help(isSecure ? "Connection is secure" : "Connection is not secure")
            }
            TextField("Search or enter address", text: $addressText)
                .textFieldStyle(.plain)
                .focused(addressFocused)
                .onSubmit(onSubmit)
                .accessibilityIdentifier("addressField")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .background(
            RoundedRectangle(cornerRadius: 7)
                .fill(Color(nsColor: .controlBackgroundColor)))
        .frame(minWidth: 120)
    }

    private var bookmarkButton: some View {
        Button(action: toggleBookmark) {
            Image(systemName: currentBookmark != nil ? "star.fill" : "star")
                .foregroundStyle(currentBookmark != nil ? Color.yellow : Color.secondary)
        }
        .help(currentBookmark != nil ? "Remove Bookmark" : "Bookmark This Page")
        .disabled(backend.currentURL.isEmpty)
        .accessibilityIdentifier("bookmarkButton")
        .popover(isPresented: confirmationBinding, arrowEdge: .bottom) {
            confirmationLabel
        }
    }

    private var shieldButton: some View {
        Button {
            showShields.toggle()
        } label: {
            HStack(spacing: 3) {
                Image(systemName: "shield.lefthalf.filled")
                if shieldCount > 0 {
                    Text("\(shieldCount)")
                        .font(.caption.monospacedDigit())
                }
            }
            .foregroundStyle(Color.orange)
        }
        .help("Shields: blocked ads & trackers")
        .popover(isPresented: $showShields, arrowEdge: .bottom) {
            shieldPopover
        }
    }

    private var downloadButton: some View {
        Button {
            showDownloads.toggle()
        } label: {
            Image(systemName: hasActiveDownload
                ? "arrow.down.circle.fill" : "arrow.down.circle")
                .foregroundStyle(hasActiveDownload ? Color.accentColor : Color.secondary)
        }
        .help("Downloads")
        .popover(isPresented: $showDownloads, arrowEdge: .bottom) {
            downloadsPopover
        }
    }

    /// Same actions, one anchor: popovers attach to the menu button so
    /// nothing is clipped at narrow widths (T061).
    private var overflowMenu: some View {
        Menu {
            Button(currentBookmark != nil ? "Remove Bookmark" : "Bookmark This Page",
                   action: toggleBookmark)
                .disabled(backend.currentURL.isEmpty)
            Button("Shields (\(shieldCount) blocked)…") { showShields = true }
            Button("Downloads…") { showDownloads = true }
        } label: {
            Image(systemName: "ellipsis.circle")
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .popover(isPresented: $showShields, arrowEdge: .bottom) { shieldPopover }
        .popover(isPresented: $showDownloads, arrowEdge: .bottom) { downloadsPopover }
        .popover(isPresented: confirmationBinding, arrowEdge: .bottom) { confirmationLabel }
    }

    // MARK: - Shared popover content

    private var shieldPopover: some View {
        ShieldPopover(tabId: backend.tabId, host: host)
            .environmentObject(bridge)
    }

    private var downloadsPopover: some View {
        DownloadsView()
            .environmentObject(bridge)
    }

    private var confirmationBinding: Binding<Bool> {
        Binding(
            get: { bookmarkConfirmation != nil },
            set: { if !$0 { bookmarkConfirmation = nil } })
    }

    private var confirmationLabel: some View {
        Label(bookmarkConfirmation ?? "", systemImage: "star.fill")
            .padding(10)
            .onAppear {
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) {
                    bookmarkConfirmation = nil
                }
            }
    }

    /// Star toggles bookmark for the current page with a brief confirmation (T055).
    private func toggleBookmark() {
        if let existing = currentBookmark {
            bridge.deleteBookmark(id: existing.id)
            bookmarkConfirmation = "Bookmark removed"
        } else {
            let title = backend.title.isEmpty ? host : backend.title
            bridge.addBookmark(title: title, url: backend.currentURL)
            bookmarkConfirmation = "Bookmarked"
        }
    }
}
