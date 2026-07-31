import SwiftUI
import UniformTypeIdentifiers

/// Bookmarks manager window (T056, FR-025/026): browse the folder tree,
/// search by title/URL, rename/edit/delete, and drag to organize.
struct BookmarksManager: View {
    @EnvironmentObject private var bridge: CoreBridge
    @State private var query = ""
    @State private var editing: BookmarkItem?

    private var searching: Bool {
        !query.trimmingCharacters(in: .whitespaces).isEmpty
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                TextField("Search bookmarks", text: $query)
                    .textFieldStyle(.plain)
                Spacer()
                Button("New Folder") {
                    bridge.createBookmarkFolder(title: "New Folder")
                }
            }
            .padding(10)
            Divider()

            if searching {
                searchResults
            } else {
                treeList
            }
        }
        .frame(minWidth: 480, minHeight: 360)
        .onAppear { bridge.refreshBookmarks() }
        .sheet(item: $editing) { item in
            BookmarkEditSheet(item: item)
                .environmentObject(bridge)
        }
    }

    private var treeList: some View {
        List {
            ForEach(bridge.bookmarkTree) { item in
                BookmarkRow(item: item, depth: 0, onEdit: { editing = $0 })
            }
            // Drop zone: drag here to move an item to the top level.
            Color.clear
                .frame(height: 24)
                .onDrop(of: [.text], delegate: BookmarkDropDelegate(
                    bridge: bridge, targetParent: nil, position: .max))
        }
        .listStyle(.inset)
    }

    private var searchResults: some View {
        List(bridge.searchBookmarks(query)) { hit in
            HStack {
                Image(systemName: hit.isFolder ? "folder" : "bookmark")
                    .foregroundStyle(hit.isFolder ? Color.accentColor : .secondary)
                VStack(alignment: .leading) {
                    Text(hit.title)
                    if let url = hit.url {
                        Text(url).font(.caption).foregroundStyle(.secondary)
                    }
                }
                Spacer()
            }
            .contentShape(Rectangle())
            .contextMenu {
                Button("Edit…") { editing = hit }
                Button("Delete", role: .destructive) { bridge.deleteBookmark(id: hit.id) }
            }
        }
        .listStyle(.inset)
    }
}

/// One row (recursing into folders) with drag-organize support.
private struct BookmarkRow: View {
    @EnvironmentObject private var bridge: CoreBridge
    let item: BookmarkItem
    let depth: Int
    let onEdit: (BookmarkItem) -> Void
    @State private var expanded = true

    var body: some View {
        Group {
            HStack(spacing: 6) {
                if item.isFolder {
                    Button {
                        expanded.toggle()
                    } label: {
                        Image(systemName: expanded ? "chevron.down" : "chevron.right")
                            .font(.caption2)
                    }
                    .buttonStyle(.borderless)
                    Image(systemName: "folder")
                        .foregroundStyle(Color.accentColor)
                } else {
                    Image(systemName: "bookmark")
                        .foregroundStyle(.secondary)
                        .padding(.leading, 14)
                }
                VStack(alignment: .leading, spacing: 1) {
                    Text(item.title.isEmpty ? (item.url ?? "untitled") : item.title)
                    if let url = item.url {
                        Text(url).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                }
                Spacer()
            }
            .padding(.leading, CGFloat(depth) * 18)
            .contentShape(Rectangle())
            .onDrag { NSItemProvider(object: String(item.id) as NSString) }
            .onDrop(of: [.text], delegate: BookmarkDropDelegate(
                bridge: bridge,
                targetParent: item.isFolder ? item.id : nil,
                position: item.isFolder ? .max : 0))
            .contextMenu {
                Button("Edit…") { onEdit(item) }
                if item.isFolder {
                    Button("New Folder Inside") {
                        bridge.createBookmarkFolder(title: "New Folder", parentId: item.id)
                    }
                }
                Button("Delete", role: .destructive) { bridge.deleteBookmark(id: item.id) }
            }

            if item.isFolder && expanded {
                ForEach(item.children) { child in
                    BookmarkRow(item: child, depth: depth + 1, onEdit: onEdit)
                }
            }
        }
    }
}

/// Dropping a dragged bookmark id moves it into `targetParent` (T056).
private struct BookmarkDropDelegate: DropDelegate {
    let bridge: CoreBridge
    let targetParent: Int64?
    let position: Int

    func performDrop(info: DropInfo) -> Bool {
        guard let provider = info.itemProviders(for: [.text]).first else { return false }
        provider.loadObject(ofClass: NSString.self) { object, _ in
            guard let idString = object as? String, let id = Int64(idString) else { return }
            DispatchQueue.main.async {
                bridge.moveBookmark(id: id, parentId: targetParent, position: position)
            }
        }
        return true
    }
}

/// Title/URL edit sheet.
private struct BookmarkEditSheet: View {
    @EnvironmentObject private var bridge: CoreBridge
    @Environment(\.dismiss) private var dismiss
    let item: BookmarkItem
    @State private var title = ""
    @State private var url = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(item.isFolder ? "Edit Folder" : "Edit Bookmark")
                .font(.headline)
            TextField("Title", text: $title)
            if !item.isFolder {
                TextField("URL", text: $url)
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Save") {
                    bridge.editBookmark(
                        id: item.id,
                        title: title,
                        url: item.isFolder ? nil : url)
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(16)
        .frame(width: 380)
        .onAppear {
            title = item.title
            url = item.url ?? ""
        }
    }
}
