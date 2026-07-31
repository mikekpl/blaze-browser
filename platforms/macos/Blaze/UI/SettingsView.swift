import AppKit
import SwiftUI

/// Settings screen (T060, US6/FR-027): every control writes a partial patch;
/// the core merges it, persists, and broadcasts `SettingsChanged` so all
/// windows update instantly.
struct SettingsView: View {
    @EnvironmentObject private var bridge: CoreBridge
    @State private var customTemplate = ""

    var body: some View {
        Form {
            Section("Appearance") {
                Picker("Theme", selection: themeBinding) {
                    ForEach(AppTheme.allCases) { theme in
                        Text(theme.label).tag(theme)
                    }
                }
                .pickerStyle(.segmented)

                Toggle("Show bookmarks bar", isOn: bookmarksBarBinding)
            }

            Section("Search") {
                Picker("Search engine", selection: searchEngineBinding) {
                    Text("Google").tag("google")
                    Text("DuckDuckGo").tag("duckduckgo")
                    Text("Brave Search").tag("brave")
                    Text("Custom").tag("custom")
                }
                if bridge.settings.searchEngine == "custom" {
                    TextField("URL template (%s = query)", text: $customTemplate)
                        .onSubmit(commitCustomTemplate)
                        .help("Example: https://example.com/search?q=%s")
                }
            }

            Section("Downloads") {
                LabeledContent("Save files to") {
                    HStack {
                        Text(bridge.settings.downloadDir)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .foregroundStyle(.secondary)
                        Button("Choose…", action: chooseDownloadFolder)
                    }
                }
            }

            Section("General") {
                Toggle("Restore previous session at launch", isOn: sessionRestoreBinding)
                Toggle("Block ads and trackers (Shields)", isOn: adblockBinding)
            }
        }
        .formStyle(.grouped)
        .frame(width: 440)
        .fixedSize()
        .onAppear { customTemplate = bridge.settings.customSearchTemplate }
        .blazeThemed()
    }

    // MARK: - Bindings (read the mirror, write a patch)

    private var themeBinding: Binding<AppTheme> {
        Binding(
            get: { AppTheme(rawValue: bridge.settings.theme) ?? .system },
            set: { bridge.updateSettings(["theme": $0.rawValue]) })
    }

    private var bookmarksBarBinding: Binding<Bool> {
        Binding(
            get: { bridge.bookmarksBarVisible },
            set: { bridge.setBookmarksBarVisible($0) })
    }

    private var searchEngineBinding: Binding<String> {
        Binding(
            get: { bridge.settings.searchEngine },
            set: { choice in
                if choice == "custom" {
                    let template = customTemplate.isEmpty
                        ? "https://www.google.com/search?q=%s" : customTemplate
                    customTemplate = template
                    bridge.updateSettings(["search_engine": ["custom": template]])
                } else {
                    bridge.updateSettings(["search_engine": choice])
                }
            })
    }

    private var sessionRestoreBinding: Binding<Bool> {
        Binding(
            get: { bridge.settings.restoreSession },
            set: { bridge.updateSettings(["session_restore": $0 ? "restore" : "fresh"]) })
    }

    private var adblockBinding: Binding<Bool> {
        Binding(
            get: { bridge.settings.adblockEnabled },
            set: { bridge.updateSettings(["adblock_enabled": $0]) })
    }

    // MARK: - Actions

    private func commitCustomTemplate() {
        guard customTemplate.contains("%s"),
              customTemplate.hasPrefix("https://") || customTemplate.hasPrefix("http://")
        else { return }
        bridge.updateSettings(["search_engine": ["custom": customTemplate]])
    }

    private func chooseDownloadFolder() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.prompt = "Choose"
        if panel.runModal() == .OK, let url = panel.url {
            bridge.updateSettings(["download_dir": url.path])
        }
    }
}
