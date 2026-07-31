import SwiftUI

/// Shield popover (T031, FR-009/FR-010): per-page counters + per-site toggle.
struct ShieldPopover: View {
    @EnvironmentObject private var bridge: CoreBridge
    let tabId: String
    let host: String
    @State private var blockingEnabled = true

    private var stats: ShieldStats {
        bridge.shieldStats[tabId] ?? ShieldStats()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Image(systemName: "shield.lefthalf.filled")
                    .foregroundStyle(blockingEnabled ? Color.orange : Color.secondary)
                Text(host.isEmpty ? "Shields" : host)
                    .font(.headline)
                    .lineLimit(1)
            }
            Divider()
            HStack {
                statBlock(count: stats.adsBlocked, label: "Ads blocked")
                Divider().frame(height: 32)
                statBlock(count: stats.trackersBlocked, label: "Trackers blocked")
            }
            Divider()
            Toggle("Block ads & trackers on this site", isOn: $blockingEnabled)
                .toggleStyle(.switch)
                .disabled(host.isEmpty)
                .onChange(of: blockingEnabled) { enabled in
                    guard !host.isEmpty else { return }
                    bridge.setSiteException(host: host, blockingEnabled: enabled)
                }
            Text("Changes apply on next page load and persist for this site.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding(16)
        .frame(width: 300)
        .onAppear {
            blockingEnabled = host.isEmpty || bridge.isBlockingEnabled(host: host)
        }
    }

    private func statBlock(count: Int, label: String) -> some View {
        VStack(alignment: .leading) {
            Text("\(count)")
                .font(.title2.weight(.bold).monospacedDigit())
            Text(label)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
