import XCTest
@testable import Blaze

final class CoreBridgeTests: XCTestCase {
    func testCoreBootstraps() {
        XCTAssertNotNil(CoreBridge.shared.core, "Rust core must start")
    }

    func testResolveNavigationDisambiguates() {
        let bridge = CoreBridge.shared
        XCTAssertEqual(bridge.resolveNavigation("example.com")?.absoluteString,
                       "https://example.com/")
        let search = bridge.resolveNavigation("hello world")
        XCTAssertTrue(search?.absoluteString.contains("google.com/search") ?? false,
                      "default engine is Google per clarification")
        XCTAssertNil(bridge.resolveNavigation("javascript:alert(1)"),
                     "dangerous schemes must be rejected")
    }

    func testWindowRegistration() {
        let result = CoreBridge.shared.registerWindow(
            frame: CGRect(x: 0, y: 0, width: 800, height: 600))
        XCTAssertNotNil(result)
    }

    /// US6: settings patches persist in the core and the UI mirror parses
    /// both preset and custom search engines (T058/T060).
    func testSettingsPatchAndMirrorParsing() throws {
        let bridge = CoreBridge.shared
        bridge.updateSettings(["theme": "dark"])
        let core = try XCTUnwrap(bridge.core)
        let json = try core.getSettingsJson()
        XCTAssertTrue(json.contains("\"theme\":\"dark\""))
        bridge.updateSettings(["theme": "system"]) // restore default

        let custom = UISettings.parse([
            "theme": "light",
            "search_engine": ["custom": "https://x.example/?q=%s"],
            "session_restore": "fresh",
            "adblock_enabled": false,
        ])
        XCTAssertEqual(custom.theme, "light")
        XCTAssertEqual(custom.searchEngine, "custom")
        XCTAssertEqual(custom.customSearchTemplate, "https://x.example/?q=%s")
        XCTAssertFalse(custom.restoreSession)
        XCTAssertFalse(custom.adblockEnabled)
    }

    /// US5: bookmark CRUD + tree + search through the bridge (T054..T056).
    func testBookmarkLifecycle() throws {
        let bridge = CoreBridge.shared
        let folder = try XCTUnwrap(bridge.createBookmarkFolder(title: "TestFolder-\(UUID())"))
        let url = "https://bookmark-test-\(UUID().uuidString.prefix(8)).example.com/"
        let id = try XCTUnwrap(bridge.addBookmark(title: "Test Page", url: url, parentId: folder))

        XCTAssertEqual(bridge.bookmark(for: url)?.id, id, "star lookup finds the leaf")
        let hits = bridge.searchBookmarks("bookmark-test")
        XCTAssertTrue(hits.contains { $0.id == id })

        bridge.editBookmark(id: id, title: "Renamed")
        let folderNode = bridge.bookmarkTree.first { $0.id == folder }
        XCTAssertEqual(folderNode?.children.first?.title, "Renamed")

        bridge.deleteBookmark(id: folder) // cascades to the leaf
        XCTAssertNil(bridge.bookmark(for: url))
    }

    /// US2: create/switch/reorder/close/reopen through the bridge with the
    /// state mirror staying consistent with the core.
    func testMultiTabLifecycleAndReopen() throws {
        let bridge = CoreBridge.shared
        let windowId = try XCTUnwrap(bridge.registerWindow(
            frame: CGRect(x: 0, y: 0, width: 800, height: 600)))

        let t2 = try XCTUnwrap(bridge.createTab(
            windowId: windowId, url: "https://example.com/keepme"))
        XCTAssertEqual(bridge.browserState.window(windowId)?.tabs.count, 2)
        XCTAssertEqual(bridge.browserState.window(windowId)?.activeTabId, t2)

        bridge.reorderTab(t2, to: 0)
        XCTAssertEqual(bridge.browserState.window(windowId)?.tabs.first?.id, t2)

        bridge.setPinned(t2, pinned: true)
        XCTAssertEqual(bridge.browserState.window(windowId)?.tabs.first?.pinned, true)
        bridge.setPinned(t2, pinned: false)

        bridge.closeTab(t2)
        XCTAssertEqual(bridge.browserState.window(windowId)?.tabs.count, 1)

        let reopened = try XCTUnwrap(bridge.reopenClosedTab(windowId: windowId))
        let tab = bridge.browserState.window(windowId)?.tabs.first { $0.id == reopened }
        XCTAssertEqual(tab?.url, "https://example.com/keepme")
        XCTAssertEqual(bridge.browserState.window(windowId)?.activeTabId, reopened)
    }
}
