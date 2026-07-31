import SwiftUI
import AppKit

@main
struct BlazeApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @StateObject private var bridge = CoreBridge.shared

    var body: some Scene {
        // Value-based group: nil value = claim any core window (launch, ⌘N);
        // a concrete id binds restored/drag-out windows (T034/T037).
        WindowGroup(for: String.self) { $windowId in
            BrowserWindow(requestedWindowId: windowId)
                .blazeThemed()
                .environmentObject(bridge)
        }
        .windowStyle(.hiddenTitleBar)
        .commands {
            CommandGroup(after: .newItem) {
                Button("New Tab") {
                    bridge.newTabInFrontWindow()
                }
                .keyboardShortcut("t", modifiers: .command)

                Button("Close Tab") {
                    bridge.closeActiveTabInFrontWindow()
                }
                .keyboardShortcut("w", modifiers: .command)

                Button("Reopen Closed Tab") {
                    bridge.reopenClosedTabInFrontWindow()
                }
                .keyboardShortcut("t", modifiers: [.command, .shift])
            }
            CommandMenu("Bookmarks") {
                Button("Show Bookmarks Manager") {
                    openWindow(id: "bookmarks-manager")
                }
                .keyboardShortcut("b", modifiers: [.command, .option])

                Button(bridge.bookmarksBarVisible
                    ? "Hide Bookmarks Bar" : "Show Bookmarks Bar") {
                    bridge.setBookmarksBarVisible(!bridge.bookmarksBarVisible)
                }
                .keyboardShortcut("b", modifiers: [.command, .shift])
            }
        }

        Window("Bookmarks", id: "bookmarks-manager") {
            BookmarksManager()
                .blazeThemed()
                .environmentObject(bridge)
        }
        .defaultSize(width: 560, height: 440)

        Settings {
            SettingsView()
                .environmentObject(bridge)
        }
    }

    @Environment(\.openWindow) private var openWindow
}

/// Flushes the debounced session snapshot and storage writer before the
/// windows tear down, so quit never loses session state (T036/T037).
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        Watchdog.shared.start()
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        Watchdog.shared.stop()
        CoreBridge.shared.prepareForTermination()
        return .terminateNow
    }
}
