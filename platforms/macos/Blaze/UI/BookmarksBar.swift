import SwiftUI

/// Bookmarks bar (T057, FR-024): root-level bookmarks one click away,
/// folders as pull-down menus. Visibility follows `bookmarks_bar_visible`.
struct BookmarksBar: View {
    @EnvironmentObject private var bridge: CoreBridge
    /// Opens a URL in the active tab.
    let open: (String) -> Void

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 2) {
                ForEach(bridge.bookmarkTree) { item in
                    BookmarksBarItem(item: item, open: open)
                }
                if bridge.bookmarkTree.isEmpty {
                    Text("Bookmark pages with the ★ button to see them here")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                        .padding(.horizontal, 8)
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
        }
        .frame(height: 26)
        .background(Color(nsColor: .windowBackgroundColor))
    }
}

private struct BookmarksBarItem: View {
    @EnvironmentObject private var bridge: CoreBridge
    let item: BookmarkItem
    let open: (String) -> Void

    var body: some View {
        if item.isFolder {
            Menu {
                folderContents(item)
            } label: {
                Label(item.title, systemImage: "folder")
                    .font(.caption)
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
        } else {
            Button {
                if let url = item.url { open(url) }
            } label: {
                Text(item.title.isEmpty ? (item.url ?? "") : item.title)
                    .font(.caption)
                    .lineLimit(1)
            }
            .buttonStyle(.borderless)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .contextMenu {
                Button("Delete") { bridge.deleteBookmark(id: item.id) }
            }
        }
    }

    /// Recursive menu content; AnyView breaks the opaque-type cycle.
    private func folderContents(_ folder: BookmarkItem) -> AnyView {
        AnyView(
            Group {
                ForEach(folder.children) { child in
                    if child.isFolder {
                        Menu(child.title) {
                            folderContents(child)
                        }
                    } else {
                        Button(child.title.isEmpty ? (child.url ?? "") : child.title) {
                            if let url = child.url { open(url) }
                        }
                    }
                }
                if folder.children.isEmpty {
                    Text("Empty").foregroundStyle(.secondary)
                }
            })
    }
}
