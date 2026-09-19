import SwiftUI

/// What an empty tab shows instead of a blank (or error) page: a landscape
/// drawn on the fly. The scene is derived from the tab's id, so every empty
/// tab gets its own — and keeps it across relaunches — with daytime palettes
/// in light mode and night skies in dark mode.
struct NewTabPage: View {
    let seed: String
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        let scene = Scenery(seed: seed, night: colorScheme == .dark)
        ZStack {
            Canvas(rendersAsynchronously: true) { context, size in
                scene.draw(in: &context, size: size)
            }
            VStack(spacing: 6) {
                Text(Self.greeting)
                    .font(.system(size: 34, weight: .semibold, design: .rounded))
                Text(Date.now, format: .dateTime.weekday(.wide).month(.wide).day())
                    .font(.title3.weight(.medium))
                    .opacity(0.85)
            }
            .foregroundStyle(.white)
            .shadow(color: .black.opacity(0.35), radius: 10, y: 2)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .padding(.top, 90)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("New Tab")
    }

    private static var greeting: String {
        switch Calendar.current.component(.hour, from: .now) {
        case 5..<12: return "Good morning"
        case 12..<18: return "Good afternoon"
        default: return "Good evening"
        }
    }
}

/// A seeded landscape: sky, sun or moon, stars, haze-faded mountain ridges
/// and a pine-lined foreground. Pure function of (seed, night, size).
struct Scenery {
    private struct Palette {
        let skyTop, skyBottom, glow, orb, farRidge, nearRidge: Color
        let stars: Bool
    }

    private static let dayPalettes: [Palette] = [
        // dawn
        Palette(skyTop: rgb(0x3b4a7a), skyBottom: rgb(0xf7b79a), glow: rgb(0xffd9b0),
                orb: rgb(0xfff1d6), farRidge: rgb(0xb98aa0), nearRidge: rgb(0x2b2a4c), stars: false),
        // clear day
        Palette(skyTop: rgb(0x2f7fd1), skyBottom: rgb(0xcfeaf7), glow: rgb(0xffffff),
                orb: rgb(0xfffbe6), farRidge: rgb(0x9fc3d8), nearRidge: rgb(0x1f4d45), stars: false),
        // sunset
        Palette(skyTop: rgb(0x4a2c6d), skyBottom: rgb(0xff9a5a), glow: rgb(0xffc56b),
                orb: rgb(0xffe3a3), farRidge: rgb(0xc26a6a), nearRidge: rgb(0x2a1a3a), stars: false),
        // golden hour
        Palette(skyTop: rgb(0x5a7fb5), skyBottom: rgb(0xffd98a), glow: rgb(0xfff0b8),
                orb: rgb(0xfff6d8), farRidge: rgb(0xc9a27a), nearRidge: rgb(0x3a3324), stars: false),
    ]

    private static let nightPalettes: [Palette] = [
        // deep night
        Palette(skyTop: rgb(0x050a1f), skyBottom: rgb(0x1d2f5c), glow: rgb(0x8fa8ff),
                orb: rgb(0xf2f4ff), farRidge: rgb(0x2a3d6b), nearRidge: rgb(0x060b18), stars: true),
        // twilight
        Palette(skyTop: rgb(0x0e1033), skyBottom: rgb(0x6b3a6e), glow: rgb(0xe08aa0),
                orb: rgb(0xffe9f0), farRidge: rgb(0x4a3566), nearRidge: rgb(0x0c0a1c), stars: true),
        // aurora
        Palette(skyTop: rgb(0x03121c), skyBottom: rgb(0x0f4a4a), glow: rgb(0x6fffc8),
                orb: rgb(0xe9fff6), farRidge: rgb(0x16504f), nearRidge: rgb(0x03100f), stars: true),
    ]

    private let seedValue: UInt64
    private let palette: Palette

    init(seed: String, night: Bool) {
        // FNV-1a: stable across launches, unlike `hashValue`
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in seed.utf8 {
            hash = (hash ^ UInt64(byte)) &* 0x0000_0100_0000_01b3
        }
        seedValue = hash
        let palettes = night ? Self.nightPalettes : Self.dayPalettes
        palette = palettes[Int(hash % UInt64(palettes.count))]
    }

    func draw(in context: inout GraphicsContext, size: CGSize) {
        guard size.width > 0, size.height > 0 else { return }
        var rng = SplitMix64(state: seedValue)
        let frame = CGRect(origin: .zero, size: size)

        context.fill(Path(frame), with: .linearGradient(
            Gradient(colors: [palette.skyTop, palette.skyBottom]),
            startPoint: .zero, endPoint: CGPoint(x: 0, y: size.height * 0.8)))

        if palette.stars {
            for _ in 0..<140 {
                let point = CGPoint(x: rng.unit() * size.width, y: pow(rng.unit(), 1.6) * size.height * 0.6)
                let radius = 0.4 + rng.unit() * 1.1
                context.fill(
                    Path(ellipseIn: CGRect(x: point.x, y: point.y, width: radius * 2, height: radius * 2)),
                    with: .color(.white.opacity(0.35 + rng.unit() * 0.6)))
            }
        }

        // sun / moon with a soft halo, kept to one side, clear of the greeting
        let orbSide = rng.unit() < 0.5 ? 0.08 : 0.74
        let orbCenter = CGPoint(x: size.width * (orbSide + rng.unit() * 0.18),
                                y: size.height * (0.18 + rng.unit() * 0.2))
        let orbRadius = min(size.width, size.height) * (0.045 + rng.unit() * 0.03)
        let haloRadius = orbRadius * 7
        context.fill(
            Path(ellipseIn: CGRect(x: orbCenter.x - haloRadius, y: orbCenter.y - haloRadius,
                                   width: haloRadius * 2, height: haloRadius * 2)),
            with: .radialGradient(
                Gradient(colors: [palette.glow.opacity(0.55), palette.glow.opacity(0)]),
                center: orbCenter, startRadius: orbRadius * 0.6, endRadius: haloRadius))
        context.fill(
            Path(ellipseIn: CGRect(x: orbCenter.x - orbRadius, y: orbCenter.y - orbRadius,
                                   width: orbRadius * 2, height: orbRadius * 2)),
            with: .color(palette.orb))

        // ridges back to front: higher, hazier and smoother in the distance
        let ridgeCount = 5
        for layer in 0..<ridgeCount {
            let depth = Double(layer) / Double(ridgeCount - 1)  // 0 far … 1 near
            let baseline = size.height * (0.5 + depth * 0.36)
            let amplitude = size.height * (0.2 - depth * 0.09)
            let ridge = Ridge(rng: &rng, roughness: 1.6 + depth * 2.2)
            var path = Path()
            path.move(to: CGPoint(x: 0, y: size.height))
            let step: CGFloat = 4
            for x in stride(from: CGFloat(0), through: size.width + step, by: step) {
                path.addLine(to: CGPoint(
                    x: x, y: baseline - ridge.height(at: x / size.width) * amplitude))
            }
            path.addLine(to: CGPoint(x: size.width, y: size.height))
            path.closeSubpath()
            let color = Self.mix(palette.farRidge, palette.nearRidge, depth)
            context.fill(path, with: .linearGradient(
                Gradient(colors: [color, Self.mix(color, palette.nearRidge, 0.35)]),
                startPoint: CGPoint(x: 0, y: baseline - amplitude),
                endPoint: CGPoint(x: 0, y: size.height)))

            // pines along the two nearest ridgelines, placed in normalised x
            // so the forest stays put while the window resizes
            if layer >= ridgeCount - 2 {
                let scale = layer == ridgeCount - 1 ? 1.0 : 0.6
                var pines = Path()
                for _ in 0..<110 {
                    let x = rng.unit()
                    let height = (14 + rng.unit() * 22) * scale
                    let foot = CGPoint(x: x * size.width,
                                       y: baseline - ridge.height(at: x) * amplitude + 2)
                    pines.move(to: CGPoint(x: foot.x, y: foot.y - height))
                    pines.addLine(to: CGPoint(x: foot.x - height * 0.28, y: foot.y))
                    pines.addLine(to: CGPoint(x: foot.x + height * 0.28, y: foot.y))
                    pines.closeSubpath()
                }
                context.fill(pines, with: .color(Self.mix(color, palette.nearRidge, 0.5)))
            }
        }
    }

    /// A ridgeline as a few octaves of seeded sines, normalised to 0…1.
    private struct Ridge {
        private var octaves: [(frequency: Double, phase: Double, weight: Double)] = []

        init(rng: inout SplitMix64, roughness: Double) {
            var frequency = roughness
            var weight = 1.0
            for _ in 0..<5 {
                octaves.append((frequency * (0.8 + rng.unit() * 0.4), rng.unit() * .pi * 2, weight))
                frequency *= 2.1
                weight *= 0.48
            }
        }

        func height(at x: Double) -> Double {
            var sum = 0.0
            var total = 0.0
            for octave in octaves {
                sum += sin(x * octave.frequency * .pi * 2 + octave.phase) * octave.weight
                total += octave.weight
            }
            return (sum / total + 1) / 2
        }
    }

    private struct SplitMix64 {
        var state: UInt64

        mutating func next() -> UInt64 {
            state &+= 0x9e37_79b9_7f4a_7c15
            var z = state
            z = (z ^ (z >> 30)) &* 0xbf58_476d_1ce4_e5b9
            z = (z ^ (z >> 27)) &* 0x94d0_49bb_1331_11eb
            return z ^ (z >> 31)
        }

        /// Uniform in 0..<1.
        mutating func unit() -> Double { Double(next() >> 11) / Double(1 << 53) }
    }

    private static func rgb(_ hex: UInt32) -> Color {
        Color(red: Double((hex >> 16) & 0xff) / 255,
              green: Double((hex >> 8) & 0xff) / 255,
              blue: Double(hex & 0xff) / 255)
    }

    private static func mix(_ a: Color, _ b: Color, _ t: Double) -> Color {
        let from = NSColor(a).usingColorSpace(.sRGB) ?? .black
        let to = NSColor(b).usingColorSpace(.sRGB) ?? .black
        return Color(red: from.redComponent + (to.redComponent - from.redComponent) * t,
                     green: from.greenComponent + (to.greenComponent - from.greenComponent) * t,
                     blue: from.blueComponent + (to.blueComponent - from.blueComponent) * t)
    }
}
