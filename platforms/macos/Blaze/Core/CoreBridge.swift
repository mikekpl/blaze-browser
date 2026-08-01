import Foundation
import Combine

/// One decoded core event (JSON payload from the batched event stream).
/// Unknown event types are ignored by design — forward-compatibility contract.
struct CoreEvent {
    let type: String
    let payload: [String: Any]
}

/// Per-tab shield counters mirrored from `ShieldStatsChanged` events.
struct ShieldStats: Equatable {
    var adsBlocked: Int = 0
    var trackersBlocked: Int = 0
    var enabled: Bool = true
}

/// One tab as mirrored from the core's `get_state_json` (source of truth).
struct TabInfo: Identifiable, Equatable {
    let id: String
    var url: String
    var title: String
    var pinned: Bool
    var state: String       // active | loading | suspended | crashed
    var audioState: String  // silent | audible | muted
}

/// One window (ordered tabs + focus) mirrored from the core.
struct WindowInfo: Identifiable, Equatable {
    let id: String
    var activeTabId: String
    var tabs: [TabInfo]
}

struct BrowserState: Equatable {
    var windows: [WindowInfo] = []

    func window(_ id: String?) -> WindowInfo? {
        windows.first { $0.id == id }
    }
}

/// One download row mirrored from the core (T050, FR-020..023).
struct DownloadInfo: Identifiable, Equatable {
    let id: String
    var sourceUrl: String
    var destPath: String
    var totalBytes: Int64?
    var receivedBytes: Int64
    var state: String  // active | paused | completed | interrupted | cancelled
    var createdAt: Int64
    var completedAt: Int64?

    var fileName: String { (destPath as NSString).lastPathComponent }
    var fractionComplete: Double? {
        guard let total = totalBytes, total > 0 else { return nil }
        return Double(receivedBytes) / Double(total)
    }
}

/// One node of the nested bookmarks tree (T054, FR-024..026).
struct BookmarkItem: Identifiable, Equatable {
    let id: Int64
    var isFolder: Bool
    var title: String
    var url: String?
    var children: [BookmarkItem]

    static func parse(_ obj: [String: Any]) -> BookmarkItem {
        BookmarkItem(
            id: (obj["id"] as? NSNumber)?.int64Value ?? 0,
            isFolder: obj["is_folder"] as? Bool ?? false,
            title: obj["title"] as? String ?? "",
            url: obj["url"] as? String,
            children: (obj["children"] as? [[String: Any]] ?? []).map(parse))
    }
}

/// Bootstraps the Rust core and relays batched events onto the main actor.
/// The single Swift-side owner of `BlazeCoreHandle` (contracts/core-api.md).
final class CoreBridge: ObservableObject {
    static let shared = CoreBridge()

    private(set) var core: BlazeCoreHandle?
    @Published private(set) var lastError: String?
    /// Window id currently in front (single-window in Phase 3; WindowManager expands this in US2).
    @Published private(set) var frontWindowId: String?
    @Published private(set) var activeTabId: String?
    @Published private(set) var shieldStats: [String: ShieldStats] = [:]
    /// Last blocked navigation (tab, url, reason) → error page (FR-005).
    @Published var blockedNavigation: (url: String, reason: String)?
    /// Transient popup-blocked notice (US1-AC5).
    @Published var popupNotice: String?
    @Published private(set) var adblockReady = false
    /// Mirror of the core's full window/tab tree (T033/T036).
    @Published private(set) var browserState = BrowserState()
    /// Downloads newest-first, mirrored from the core (T050).
    @Published private(set) var downloads: [DownloadInfo] = []
    /// Smoothed transfer speed in bytes/sec per active download id.
    @Published private(set) var downloadSpeeds: [String: Double] = [:]
    private var speedSamples: [String: (bytes: Int64, at: Date)] = [:]
    /// Nested bookmarks tree mirrored from the core (T054..057).
    @Published private(set) var bookmarkTree: [BookmarkItem] = []
    /// Bookmarks bar visibility mirrored from settings (T057).
    @Published private(set) var bookmarksBarVisible = true
    /// UI-relevant settings mirrored from the core (T058/T060).
    @Published private(set) var settings = UISettings()
    /// Core windows restored from the last session not yet bound to a
    /// SwiftUI window (claimed by BrowserWindow on appear, T037).
    private(set) var unclaimedWindows: [String] = []

    private let listener = BatchListener()

    private init() {
        listener.onBatch = { [weak self] events in
            DispatchQueue.main.async { self?.handle(events: events) }
        }
        do {
            // Empty profile dir selects the platform default (~/Library/Application Support/Blaze).
            core = try BlazeCoreHandle(profileDir: "", listener: listener)
            initAdblock()
            restoreSessionIfEnabled()
            resumeInterruptedDownloads()
            refreshBookmarks()
            syncUISettings()
        } catch {
            lastError = "Core failed to start: \(error)"
        }
    }

    /// Restore the previous session at launch when settings allow (FR-018).
    private func restoreSessionIfEnabled() {
        guard let core else { return }
        let wantsRestore: Bool = {
            guard let json = try? core.getSettingsJson(),
                  let data = json.data(using: .utf8),
                  let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            else { return true }
            return (obj["session_restore"] as? String ?? "restore") == "restore"
        }()
        guard wantsRestore else { return }
        unclaimedWindows = (try? core.restorePreviousSession()) ?? []
        refreshState()
    }

    /// Build/load the filter engine off the main thread (cold build parses ~150k rules).
    private func initAdblock() {
        guard let core,
              let assets = Bundle.main.url(forResource: "assets", withExtension: nil)
        else { return }
        let filters = assets.appendingPathComponent("filters").path
        let manifest = assets.appendingPathComponent("scriptlets/manifest.json").path
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            do {
                let count = try core.initAdblock(
                    filtersDir: filters, scriptletsManifestPath: manifest)
                DispatchQueue.main.async { self?.adblockReady = true }
                NSLog("Blaze: adblock ready (%d lists)", count)
            } catch {
                DispatchQueue.main.async { self?.lastError = "Adblock init failed: \(error)" }
            }
        }
    }

    // MARK: - Commands

    /// Bind a SwiftUI window to a core window: a specific restored id, any
    /// unclaimed restored window, or a brand-new one (T034/T037).
    func claimWindow(requested: String? = nil, frame: CGRect) -> String? {
        if let requested {
            unclaimedWindows.removeAll { $0 == requested }
            frontWindowId = requested
            return requested
        }
        if let restored = unclaimedWindows.first {
            unclaimedWindows.removeFirst()
            frontWindowId = restored
            return restored
        }
        return registerWindow(frame: frame)
    }

    /// Register the SwiftUI window with the core.
    func registerWindow(frame: CGRect) -> String? {
        guard let core else { return nil }
        do {
            let windowId = try core.createWindow(
                frame: Rect(x: frame.origin.x, y: frame.origin.y,
                            w: frame.width, h: frame.height))
            frontWindowId = windowId
            refreshState()
            return windowId
        } catch {
            report(error)
            return nil
        }
    }

    /// Re-pull the full window/tab tree from the core (source of truth).
    func refreshState() {
        guard let core, let json = try? core.getStateJson(),
              let data = json.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let windows = obj["windows"] as? [[String: Any]]
        else { return }
        browserState = BrowserState(windows: windows.map { w in
            WindowInfo(
                id: w["id"] as? String ?? "",
                activeTabId: w["active_tab_id"] as? String ?? "",
                tabs: (w["tabs"] as? [[String: Any]] ?? []).map { t in
                    TabInfo(
                        id: t["id"] as? String ?? "",
                        url: t["url"] as? String ?? "",
                        title: t["title"] as? String ?? "",
                        pinned: t["pinned"] as? Bool ?? false,
                        state: t["state"] as? String ?? "active",
                        audioState: t["audio_state"] as? String ?? "silent")
                })
        })
    }

    // MARK: Tab commands (T033/T034)

    @discardableResult
    func createTab(windowId: String, url: String? = nil) -> String? {
        guard let core else { return nil }
        do {
            let tabId = try core.createTab(windowId: windowId, url: url)
            activeTabId = tabId
            refreshState()
            enforceTabSuspension()
            return tabId
        } catch {
            report(error)
            return nil
        }
    }

    func closeTab(_ tabId: String) {
        // Closing a window's last tab tears down the core window; the SwiftUI
        // window notices the dead id in syncWithState and closes itself.
        try? core?.closeTab(tabId: tabId)
        shieldStats.removeValue(forKey: tabId)
        refreshState()
    }

    func activateTab(_ tabId: String) {
        try? core?.activateTab(tabId: tabId)
        activeTabId = tabId
        refreshState()
        enforceTabSuspension()
    }

    func reorderTab(_ tabId: String, to position: Int) {
        try? core?.reorderTab(tabId: tabId, position: UInt32(max(0, position)))
        refreshState()
    }

    func moveTab(_ tabId: String, toWindow windowId: String, position: Int) {
        try? core?.moveTab(tabId: tabId, toWindow: windowId, position: UInt32(max(0, position)))
        refreshState()
    }

    func setPinned(_ tabId: String, pinned: Bool) {
        try? core?.setPinned(tabId: tabId, pinned: pinned)
        refreshState()
    }

    func setMuted(_ tabId: String, muted: Bool) {
        try? core?.setMuted(tabId: tabId, muted: muted)
        refreshState()
    }

    /// Reopen the most recently closed tab (Cmd+Shift+T, FR-015).
    @discardableResult
    func reopenClosedTab(windowId: String) -> String? {
        guard let core else { return nil }
        let tabId = (try? core.reopenClosedTab(windowId: windowId)) ?? nil
        if let tabId {
            try? core.activateTab(tabId: tabId)
            activeTabId = tabId
        }
        refreshState()
        return tabId
    }

    /// Ask the core which background tabs to suspend (FR-016, T038);
    /// suspended ids flow back via state refresh and web views are dropped.
    func enforceTabSuspension() {
        guard let core else { return }
        if let suspended = try? core.suspendLruTabs(), !suspended.isEmpty {
            refreshState()
        }
    }

    /// Resolve address-bar input for a tab, marking it loading in the core.
    /// Returns nil (and records the block) for dangerous input.
    func navigate(tabId: String, input: String) -> URL? {
        guard let core else { return nil }
        do {
            return URL(string: try core.navigate(tabId: tabId, input: input))
        } catch {
            // NavigationBlocked event carries the reason; error surfaced via event.
            return nil
        }
    }

    /// Resolve address-bar input without tab side effects (used by tests).
    func resolveNavigation(_ input: String) -> URL? {
        guard let core else { return nil }
        return (try? core.resolveNavigation(input: input)).flatMap(URL.init(string:))
    }

    func notifyCommitted(tabId: String, url: String) {
        try? core?.notifyCommitted(tabId: tabId, url: url)
    }

    func notifyLoaded(tabId: String, title: String?, success: Bool) {
        try? core?.notifyLoaded(tabId: tabId, title: title, success: success)
    }

    /// Engine reported playback started/stopped (T041); audible indicator
    /// updates arrive back via `TabAudioChanged`.
    func notifyMediaPlayback(tabId: String, playing: Bool) {
        try? core?.notifyMediaPlayback(tabId: tabId, playing: playing)
    }

    /// Compiled WKContentRuleList JSON (empty until adblock is ready).
    func webkitRulesJSON() -> String? {
        try? core?.compiledRulesForWebkit()
    }

    /// Cosmetic payload `{css, scriptlets}` for a page URL.
    func cosmetics(for url: String) -> (css: String, scriptlets: [(name: String, source: String)])? {
        guard settings.adblockEnabled,
              let json = try? core?.cosmeticsFor(url: url),
              let data = json.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else { return nil }
        let css = obj["css"] as? String ?? ""
        let scripts = (obj["scriptlets"] as? [[String: Any]] ?? []).compactMap { s -> (String, String)? in
            guard let name = s["name"] as? String, let source = s["source"] as? String else { return nil }
            return (name, source)
        }
        return (css, scripts)
    }

    /// Blocking fallback for requests WebKit's declarative rules can't cover.
    func shouldBlock(tabId: String, url: String, sourceURL: String, kind: String) -> Bool {
        guard settings.adblockEnabled else { return false }
        return (try? core?.classifyRequest(tabId: tabId, url: url, sourceUrl: sourceURL, kind: kind)) ?? false
    }

    func setSiteException(host: String, blockingEnabled: Bool) {
        try? core?.setSiteException(host: host, blockingEnabled: blockingEnabled)
    }

    func isBlockingEnabled(host: String) -> Bool {
        (try? core?.isBlockingEnabled(host: host)) ?? true
    }

    func newTabInFrontWindow() {
        guard let windowId = frontWindowId else { return }
        createTab(windowId: windowId)
    }

    /// Close the active tab in the front window (⌘W).
    func closeActiveTabInFrontWindow() {
        guard let window = browserState.window(frontWindowId) else { return }
        closeTab(window.activeTabId)
    }

    /// Reopen the last closed tab into the front window (⌘⇧T).
    func reopenClosedTabInFrontWindow() {
        guard let windowId = frontWindowId else { return }
        reopenClosedTab(windowId: windowId)
    }

    func setFrontWindow(_ windowId: String) {
        frontWindowId = windowId
    }

    // MARK: Downloads (T050, FR-020..023)

    /// Hand a URL to the Rust download engine (WKDownload handoff or direct).
    @discardableResult
    func startDownload(url: String, suggestedName: String? = nil) -> String? {
        guard let core else { return nil }
        do {
            let id = try core.startDownload(url: url, suggestedName: suggestedName)
            refreshDownloads()
            return id
        } catch {
            report(error)
            return nil
        }
    }

    func pauseDownload(_ id: String) {
        try? core?.pauseDownload(downloadId: id)
    }

    func resumeDownload(_ id: String) {
        try? core?.resumeDownload(downloadId: id)
        refreshDownloads()
    }

    func cancelDownload(_ id: String) {
        try? core?.cancelDownload(downloadId: id)
        refreshDownloads()
    }

    /// Crash/quit recovery: interrupted downloads resume automatically (T048).
    private func resumeInterruptedDownloads() {
        guard let core else { return }
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let resumed = (try? core.resumeInterruptedDownloads()) ?? []
            DispatchQueue.main.async {
                self?.refreshDownloads()
                if !resumed.isEmpty {
                    NSLog("Blaze: resumed %d interrupted download(s)", resumed.count)
                }
            }
        }
    }

    /// Re-pull the download list from the core (source of truth).
    func refreshDownloads() {
        guard let core, let json = try? core.listDownloadsJson(),
              let data = json.data(using: .utf8),
              let rows = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]]
        else { return }
        downloads = rows.map { r in
            DownloadInfo(
                id: r["id"] as? String ?? "",
                sourceUrl: r["source_url"] as? String ?? "",
                destPath: r["dest_path"] as? String ?? "",
                totalBytes: (r["total_bytes"] as? NSNumber)?.int64Value,
                receivedBytes: (r["received_bytes"] as? NSNumber)?.int64Value ?? 0,
                state: r["state"] as? String ?? "active",
                createdAt: (r["created_at"] as? NSNumber)?.int64Value ?? 0,
                completedAt: (r["completed_at"] as? NSNumber)?.int64Value)
        }
    }

    /// Update the rolling speed estimate from one `DownloadUpdated` event.
    private func noteDownloadProgress(id: String, state: String, receivedBytes: Int64) {
        guard state == "active" else {
            speedSamples.removeValue(forKey: id)
            downloadSpeeds.removeValue(forKey: id)
            return
        }
        let now = Date()
        if let last = speedSamples[id] {
            let dt = now.timeIntervalSince(last.at)
            if dt > 0.1 {
                let instant = Double(receivedBytes - last.bytes) / dt
                let previous = downloadSpeeds[id] ?? instant
                downloadSpeeds[id] = previous * 0.7 + instant * 0.3  // smooth ETA jitter
                speedSamples[id] = (receivedBytes, now)
            }
        } else {
            speedSamples[id] = (receivedBytes, now)
        }
    }

    // MARK: Bookmarks (T054..T057, FR-024..026)

    /// Re-pull the nested bookmarks tree from the core (source of truth).
    func refreshBookmarks() {
        guard let core, let json = try? core.bookmarksTreeJson(),
              let data = json.data(using: .utf8),
              let nodes = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]]
        else { return }
        bookmarkTree = nodes.map(BookmarkItem.parse)
    }

    @discardableResult
    func addBookmark(title: String, url: String, parentId: Int64? = nil) -> Int64? {
        guard let core else { return nil }
        let id = try? core.addBookmark(parentId: parentId, title: title, url: url)
        refreshBookmarks()
        return id
    }

    @discardableResult
    func createBookmarkFolder(title: String, parentId: Int64? = nil) -> Int64? {
        guard let core else { return nil }
        let id = try? core.createFolder(parentId: parentId, title: title)
        refreshBookmarks()
        return id
    }

    func editBookmark(id: Int64, title: String? = nil, url: String? = nil) {
        try? core?.editBookmark(id: id, title: title, url: url)
        refreshBookmarks()
    }

    func deleteBookmark(id: Int64) {
        try? core?.deleteBookmark(id: id)
        refreshBookmarks()
    }

    func moveBookmark(id: Int64, parentId: Int64?, position: Int) {
        try? core?.moveBookmark(id: id, newParent: parentId, position: Int64(position))
        refreshBookmarks()
    }

    /// Flat search over titles and URLs (folders first).
    func searchBookmarks(_ query: String) -> [BookmarkItem] {
        guard let core, let json = try? core.searchBookmarksJson(query: query),
              let data = json.data(using: .utf8),
              let rows = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]]
        else { return [] }
        return rows.map(BookmarkItem.parse)
    }

    /// The bookmark leaf matching `url` anywhere in the tree, if any (T055).
    func bookmark(for url: String) -> BookmarkItem? {
        func find(in nodes: [BookmarkItem]) -> BookmarkItem? {
            for node in nodes {
                if !node.isFolder, node.url == url { return node }
                if let hit = find(in: node.children) { return hit }
            }
            return nil
        }
        return find(in: bookmarkTree)
    }

    func setBookmarksBarVisible(_ visible: Bool) {
        _ = try? core?.updateSettingsJson(
            patchJson: "{\"bookmarks_bar_visible\": \(visible)}")
    }

    /// Apply a partial settings document; the resulting `SettingsChanged`
    /// event refreshes the mirror in every window (T060).
    func updateSettings(_ patch: [String: Any]) {
        guard let core,
              let data = try? JSONSerialization.data(withJSONObject: patch),
              let json = String(data: data, encoding: .utf8) else { return }
        _ = try? core.updateSettingsJson(patchJson: json)
    }

    /// Mirror UI-relevant settings into published state.
    private func syncUISettings() {
        guard let core, let json = try? core.getSettingsJson(),
              let data = json.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else { return }
        applySettings(obj)
    }

    private func applySettings(_ obj: [String: Any]) {
        let updated = UISettings.parse(obj)
        if updated.adblockEnabled != settings.adblockEnabled {
            NotificationCenter.default.post(
                name: .blazeAdblockChanged, object: nil,
                userInfo: ["enabled": updated.adblockEnabled])
        }
        settings = updated
        bookmarksBarVisible = obj["bookmarks_bar_visible"] as? Bool ?? true
    }

    /// Close a core window when its SwiftUI window is dismissed mid-session.
    /// Skipped during termination so the final snapshot keeps all windows.
    func closeWindow(_ windowId: String) {
        guard !isTerminating else { return }
        try? core?.closeWindow(windowId: windowId)
        refreshState()
    }

    private(set) var isTerminating = false

    /// Flush session + storage before windows tear down at quit (T036/T037).
    func prepareForTermination() {
        isTerminating = true
        core?.shutdown()
    }

    func shutdown() {
        core?.shutdown()
    }

    // MARK: - Events

    private func handle(events: [CoreEvent]) {
        var stateDirty = false
        var downloadsDirty = false
        for event in events {
            switch event.type {
            case "tab_created":
                if let tab = event.payload["tab"] as? String {
                    activeTabId = tab
                }
                stateDirty = true
            case "tab_closed", "window_closed", "tab_state_changed",
                 "tab_meta_changed", "tab_audio_changed":
                stateDirty = true
            case "download_updated":
                if let id = event.payload["download_id"] as? String {
                    noteDownloadProgress(
                        id: id,
                        state: event.payload["state"] as? String ?? "active",
                        receivedBytes: (event.payload["received_bytes"] as? NSNumber)?.int64Value ?? 0)
                }
                downloadsDirty = true
            case "settings_changed":
                if let settings = event.payload["settings"] as? [String: Any] {
                    applySettings(settings)
                }
            case "shield_stats_changed":
                if let tab = event.payload["tab"] as? String {
                    shieldStats[tab] = ShieldStats(
                        adsBlocked: event.payload["ads_blocked"] as? Int ?? 0,
                        trackersBlocked: event.payload["trackers_blocked"] as? Int ?? 0,
                        enabled: event.payload["enabled"] as? Bool ?? true)
                }
            case "navigation_blocked":
                blockedNavigation = (
                    url: event.payload["url"] as? String ?? "",
                    reason: event.payload["reason"] as? String ?? "blocked")
            default:
                break // unknown events intentionally ignored
            }
        }
        if stateDirty { refreshState() }
        if downloadsDirty { refreshDownloads() }
        objectWillChange.send()
    }

    private func report(_ error: Error) {
        lastError = "\(error)"
    }
}

/// Receives JSON batches from Rust on an arbitrary thread.
/// `@unchecked Sendable`: `onBatch` is set once during init, before the
/// listener is handed to Rust, and never mutated afterwards.
private final class BatchListener: EventListener, @unchecked Sendable {
    var onBatch: (([CoreEvent]) -> Void)?

    func onEvents(eventsJson: String) {
        guard
            let data = eventsJson.data(using: .utf8),
            let array = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]]
        else { return }
        let events = array.compactMap { obj -> CoreEvent? in
            guard let type = obj["type"] as? String else { return nil }
            return CoreEvent(type: type, payload: obj)
        }
        onBatch?(events)
    }
}

/// UI-relevant subset of the core settings document (T058/T060).
struct UISettings: Equatable {
    var theme = "system"
    /// Preset id ("google", "duckduckgo", "brave") or "custom".
    var searchEngine = "google"
    var customSearchTemplate = ""
    var downloadDir = "~/Downloads"
    var restoreSession = true
    var adblockEnabled = true

    static func parse(_ obj: [String: Any]) -> UISettings {
        var s = UISettings()
        s.theme = obj["theme"] as? String ?? "system"
        if let preset = obj["search_engine"] as? String {
            s.searchEngine = preset
        } else if let custom = obj["search_engine"] as? [String: Any],
                  let template = custom["custom"] as? String {
            s.searchEngine = "custom"
            s.customSearchTemplate = template
        }
        s.downloadDir = obj["download_dir"] as? String ?? "~/Downloads"
        s.restoreSession = (obj["session_restore"] as? String ?? "restore") == "restore"
        s.adblockEnabled = obj["adblock_enabled"] as? Bool ?? true
        return s
    }
}

extension Notification.Name {
    /// Posted when the global adblock toggle flips; web views re-apply rules.
    static let blazeAdblockChanged = Notification.Name("BlazeAdblockChanged")
}
