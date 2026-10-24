//! Visual language shared by every page.
//!
//! Colours are lifted from `fanui/src/theme.rs` so the two UIs are
//! indistinguishable in screenshots.

import SwiftUI

enum Palette {
    static let accent = Color(red: 0x2E / 255, green: 0x8B / 255, blue: 0xF7 / 255)
    static let temp = Color(red: 0xFF / 255, green: 0x8A / 255, blue: 0x3D / 255)
    static let rpm = Color(red: 0x35 / 255, green: 0xC7 / 255, blue: 0x8F / 255)
    static let ok = Color(red: 0x3F / 255, green: 0xC9 / 255, blue: 0x6A / 255)
    static let warn = Color(red: 0xF2 / 255, green: 0xB1 / 255, blue: 0x35 / 255)
    static let danger = Color(red: 0xEF / 255, green: 0x53 / 255, blue: 0x50 / 255)
    static let muted = Color(red: 0x8A / 255, green: 0x8F / 255, blue: 0x98 / 255)
    static let cool = Color(red: 0x5A / 255, green: 0xC8 / 255, blue: 0xE8 / 255)
    static let orange = Color(red: 0xF5 / 255, green: 0x7C / 255, blue: 0x36 / 255)

    static let panel = Color(red: 0x16 / 255, green: 0x18 / 255, blue: 0x1C / 255)
    static let card = Color(red: 0x20 / 255, green: 0x24 / 255, blue: 0x2A / 255)
    static let stroke = Color(red: 0x2C / 255, green: 0x31 / 255, blue: 0x39 / 255)
    static let well = Color(red: 0x0F / 255, green: 0x11 / 255, blue: 0x14 / 255)
    static let raised = Color(red: 0x22 / 255, green: 0x26 / 255, blue: 0x2C / 255)

    /// Same ramp as `theme::temp_colour`.
    static func forTemp(_ temp: Double) -> Color {
        switch temp {
        case ..<50: return cool
        case ..<65: return ok
        case ..<78: return warn
        case ..<90: return orange
        default: return danger
        }
    }

    static func forMode(_ mode: ControlMode) -> Color {
        switch mode {
        case .auto: return muted
        case .fixed: return accent
        case .curve: return ok
        }
    }
}

// MARK: - building blocks

/// Rounded card that every page block sits in.
struct Card<Content: View>: View {
    var spacing: CGFloat = 10
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: spacing) { content }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(Palette.card)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(Palette.stroke, lineWidth: 1)
            )
    }
}

struct CardTitle: View {
    let text: String
    var trailing: String?

    var body: some View {
        HStack(spacing: 6) {
            Text(text).font(.system(size: 13, weight: .semibold))
            Spacer(minLength: 8)
            if let trailing {
                Text(trailing).font(.system(size: 11)).foregroundStyle(Palette.muted)
            }
        }
    }
}

/// Big number + unit + caption.
struct Stat: View {
    let value: String
    let unit: String
    let caption: String
    var colour: Color = .primary

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(alignment: .firstTextBaseline, spacing: 3) {
                Text(value)
                    .font(.system(size: 26, weight: .semibold))
                    .monospacedDigit()
                    .foregroundStyle(colour)
                Text(unit).font(.system(size: 11)).foregroundStyle(Palette.muted)
            }
            Text(caption).font(.system(size: 11)).foregroundStyle(Palette.muted)
        }
        .frame(minWidth: 90, alignment: .leading)
    }
}

/// Pill badge, e.g. the active control mode.
struct Badge: View {
    let text: String
    let colour: Color

    var body: some View {
        Text(text)
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(colour)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(Capsule().fill(colour.opacity(0.16)))
    }
}

/// Thin horizontal bar.
struct Meter: View {
    let fraction: Double
    var colour: Color
    var height: CGFloat = 6

    var body: some View {
        GeometryReader { proxy in
            ZStack(alignment: .leading) {
                Capsule().fill(Palette.stroke)
                Capsule()
                    .fill(colour)
                    .frame(width: proxy.size.width * min(max(fraction, 0), 1))
            }
        }
        .frame(height: height)
    }
}

/// Sensor row: name, bar, reading. Highlights the group the curve reads.
struct SensorRow: View {
    let sensor: SensorReading
    let selected: Bool

    var body: some View {
        let temp = sensor.temp
        let colour = temp.map(Palette.forTemp) ?? Palette.muted
        HStack(spacing: 10) {
            Text(sensor.name)
                .font(.system(size: 13))
                .foregroundStyle(selected ? Color.white : Color(white: 0.83))
                .frame(width: 78, alignment: .leading)

            GeometryReader { proxy in
                ZStack(alignment: .leading) {
                    Capsule().fill(Palette.stroke)
                    if let temp {
                        Capsule()
                            .fill(colour)
                            .frame(width: proxy.size.width * min(max(temp / 110, 0), 1))
                    }
                }
            }
            .frame(height: 8)

            Text(temp.map { String(format: "%.1f°C", $0) } ?? "—")
                .font(.system(size: 12))
                .monospacedDigit()
                .foregroundStyle(colour)
                .frame(width: 62, alignment: .trailing)
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 3)
        .background(
            RoundedRectangle(cornerRadius: 4)
                .fill(selected ? Palette.accent.opacity(0.12) : .clear)
        )
        .help("\(sensor.name)\n\(sensor.group.detail)\n传感器：\(keysPreview(sensor.keys))")
    }
}

func keysPreview(_ keys: [String], max limit: Int = 12) -> String {
    if keys.isEmpty { return "无" }
    if keys.count <= limit { return keys.joined(separator: " ") }
    return keys.prefix(limit).joined(separator: " ") + " … 共 \(keys.count) 个"
}

func uptimeText(_ seconds: Int) -> String {
    if seconds < 60 { return "\(seconds) 秒" }
    if seconds < 3600 { return "\(seconds / 60) 分钟" }
    if seconds < 86_400 { return "\(seconds / 3600) 小时 \((seconds % 3600) / 60) 分" }
    return "\(seconds / 86_400) 天"
}

/// Local view state for controls; `@State` needs the SwiftUI macro plugin that
/// the command-line toolchain does not ship, so views use a tiny reference box.
final class Box<Value>: ObservableObject {
    @Published var value: Value
    init(_ value: Value) { self.value = value }
}
