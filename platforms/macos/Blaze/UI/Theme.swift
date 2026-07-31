import AppKit
import SwiftUI

/// Theme engine (T059, US6): light/dark/system with instant switching.
/// `NSApp.appearance` covers every AppKit-hosted surface (windows, popovers,
/// menus, sheets); `preferredColorScheme` keeps SwiftUI previews honest.
enum AppTheme: String, CaseIterable, Identifiable {
    case light, dark, system

    var id: String { rawValue }

    var label: String {
        switch self {
        case .light: return "Light"
        case .dark: return "Dark"
        case .system: return "System"
        }
    }

    var colorScheme: ColorScheme? {
        switch self {
        case .light: return .light
        case .dark: return .dark
        case .system: return nil
        }
    }

    private var nsAppearance: NSAppearance? {
        switch self {
        case .light: return NSAppearance(named: .aqua)
        case .dark: return NSAppearance(named: .darkAqua)
        case .system: return nil // follow the OS
        }
    }

    /// Apply app-wide, instantly, to all open windows.
    func applyToApp() {
        NSApp.appearance = nsAppearance
    }
}

/// Root modifier keeping a scene in sync with the theme setting.
struct ThemedSurface: ViewModifier {
    @EnvironmentObject private var bridge: CoreBridge

    private var theme: AppTheme {
        AppTheme(rawValue: bridge.settings.theme) ?? .system
    }

    func body(content: Content) -> some View {
        content
            .preferredColorScheme(theme.colorScheme)
            .onAppear { theme.applyToApp() }
            .onChange(of: bridge.settings.theme) { raw in
                (AppTheme(rawValue: raw) ?? .system).applyToApp()
            }
    }
}

extension View {
    func blazeThemed() -> some View { modifier(ThemedSurface()) }
}
