import SwiftUI
import AppKit

/// Downloads popover (T050): live progress with speed/ETA, pause/resume/
/// cancel controls, open / Reveal in Finder, and the persisted history list.
struct DownloadsView: View {
    @EnvironmentObject private var bridge: CoreBridge

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Downloads")
                .font(.headline)
                .padding(12)
            Divider()
            if bridge.downloads.isEmpty {
                Text("No downloads yet")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 80)
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(bridge.downloads) { download in
                            DownloadRowView(
                                download: download,
                                speed: bridge.downloadSpeeds[download.id])
                            Divider().padding(.leading, 12)
                        }
                    }
                }
                .frame(maxHeight: 360)
            }
        }
        .frame(width: 380)
        .onAppear { bridge.refreshDownloads() }
    }
}

private struct DownloadRowView: View {
    @EnvironmentObject private var bridge: CoreBridge
    let download: DownloadInfo
    let speed: Double?

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: iconName)
                .font(.title2)
                .foregroundStyle(iconColor)
                .frame(width: 28)

            VStack(alignment: .leading, spacing: 3) {
                Text(download.fileName)
                    .font(.callout.weight(.medium))
                    .lineLimit(1)
                    .truncationMode(.middle)
                if download.state == "active" {
                    ProgressView(value: download.fractionComplete ?? 0)
                        .progressViewStyle(.linear)
                        .opacity(download.fractionComplete == nil ? 0.4 : 1)
                }
                Text(detailText)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            controls
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    @ViewBuilder private var controls: some View {
        HStack(spacing: 6) {
            switch download.state {
            case "active":
                Button { bridge.pauseDownload(download.id) } label: {
                    Image(systemName: "pause.circle")
                }
                .help("Pause")
                cancelButton
            case "paused", "interrupted":
                Button { bridge.resumeDownload(download.id) } label: {
                    Image(systemName: "arrow.clockwise.circle")
                }
                .help("Resume")
                cancelButton
            case "completed":
                Button { open() } label: {
                    Image(systemName: "arrow.up.forward.square")
                }
                .help("Open")
                Button { revealInFinder() } label: {
                    Image(systemName: "magnifyingglass.circle")
                }
                .help("Reveal in Finder")
            default:
                EmptyView()
            }
        }
        .buttonStyle(.borderless)
        .font(.title3)
    }

    private var cancelButton: some View {
        Button { bridge.cancelDownload(download.id) } label: {
            Image(systemName: "xmark.circle")
        }
        .help("Cancel")
    }

    private var iconName: String {
        switch download.state {
        case "active": return "arrow.down.circle"
        case "paused": return "pause.circle.fill"
        case "interrupted": return "exclamationmark.circle"
        case "completed": return "checkmark.circle.fill"
        default: return "xmark.circle.fill"
        }
    }

    private var iconColor: Color {
        switch download.state {
        case "active": return .accentColor
        case "paused": return .secondary
        case "interrupted": return .orange
        case "completed": return .green
        default: return .secondary
        }
    }

    private var detailText: String {
        let received = Self.bytes(download.receivedBytes)
        switch download.state {
        case "active":
            var parts: [String] = []
            if let total = download.totalBytes {
                parts.append("\(received) of \(Self.bytes(total))")
            } else {
                parts.append(received)
            }
            if let speed, speed > 1 {
                parts.append("\(Self.bytes(Int64(speed)))/s")
                if let total = download.totalBytes, total > download.receivedBytes {
                    let eta = Double(total - download.receivedBytes) / speed
                    parts.append(Self.duration(eta) + " left")
                }
            }
            return parts.joined(separator: " — ")
        case "paused":
            return "Paused — \(received)"
        case "interrupted":
            return "Interrupted — \(received)"
        case "completed":
            if let total = download.totalBytes { return Self.bytes(total) }
            return received
        case "cancelled":
            return "Cancelled"
        default:
            return download.state
        }
    }

    private func open() {
        NSWorkspace.shared.open(URL(fileURLWithPath: download.destPath))
    }

    private func revealInFinder() {
        NSWorkspace.shared.activateFileViewerSelecting(
            [URL(fileURLWithPath: download.destPath)])
    }

    private static func bytes(_ n: Int64) -> String {
        ByteCountFormatter.string(fromByteCount: n, countStyle: .file)
    }

    private static func duration(_ seconds: Double) -> String {
        if seconds < 60 { return "\(Int(seconds))s" }
        if seconds < 3600 { return "\(Int(seconds / 60))m \(Int(seconds) % 60)s" }
        return "\(Int(seconds / 3600))h \(Int(seconds) % 3600 / 60)m"
    }
}
