//! Codable mirrors of `fancore::protocol` / `fancore::types` / `fancore::config`.
//!
//! The wire format is newline-delimited JSON, so every field here has to match
//! the Rust `serde` names exactly. Keep the two sides in sync; `fanctl status
//! --json` is the quickest way to spot a drift.

import Foundation

// MARK: - enums

enum ControlMode: String, Codable, CaseIterable, Identifiable {
    case auto, fixed, curve

    var id: String { rawValue }

    var label: String {
        switch self {
        case .auto: return "系统自动"
        case .fixed: return "固定转速"
        case .curve: return "温度曲线"
        }
    }

    var blurb: String {
        switch self {
        case .auto: return "系统自动 — 由 macOS 决定转速"
        case .fixed: return "固定转速 — 始终跑同一个转速"
        case .curve: return "温度曲线 — 按温度线性调速"
        }
    }
}

enum FanMode: String, Codable {
    case auto, manual, unknown

    var label: String {
        switch self {
        case .auto: return "自动"
        case .manual: return "手动"
        case .unknown: return "未知"
        }
    }
}

enum SensorGroup: String, Codable, CaseIterable, Identifiable {
    case cpu
    case pCore = "p_core"
    case eCore = "e_core"
    case gpu
    case soc
    case storage
    case ambient

    var id: String { rawValue }

    var label: String {
        switch self {
        case .cpu: return "CPU 热点"
        case .pCore: return "性能核"
        case .eCore: return "能效核"
        case .gpu: return "GPU"
        case .soc: return "SoC"
        case .storage: return "存储"
        case .ambient: return "环境"
        }
    }

    var detail: String {
        switch self {
        case .cpu: return "封装内最高温，响应最快，最接近 Apple 自己的调速依据"
        case .pCore: return "P 核集群温度，重负载时升温明显"
        case .eCore: return "E 核集群温度，轻负载时更灵敏"
        case .gpu: return "GPU 集群温度，渲染/视频编码时升高"
        case .soc: return "SoC 供电/封装复合温度"
        case .storage: return "SSD / NAND 温度"
        case .ambient: return "机箱进风与内存等环境传感器"
        }
    }
}

// MARK: - curve

struct CurvePoint: Codable, Equatable, Identifiable {
    var temp: Double
    var rpm: Double

    var id: String { "\(temp)-\(rpm)" }
}

struct Curve: Codable, Equatable {
    var points: [CurvePoint]
    var max_temp: Double
    var spin_up_rpm: Double
    var smoothing_secs: Double
    var rpm_deadband: Double
    var max_rpm_per_sec: Double

    static let `default` = Curve(
        points: [
            CurvePoint(temp: 45, rpm: 1000),
            CurvePoint(temp: 60, rpm: 1400),
            CurvePoint(temp: 70, rpm: 2200),
            CurvePoint(temp: 80, rpm: 3200),
            CurvePoint(temp: 90, rpm: 4300),
            CurvePoint(temp: 100, rpm: 4900),
        ],
        max_temp: 100,
        spin_up_rpm: 0,
        smoothing_secs: 3,
        rpm_deadband: 60,
        max_rpm_per_sec: 600
    )

    /// Piecewise-linear interpolation, clamped to the hardware range.
    /// Mirrors `fancore::types::Curve::evaluate` so the preview matches what the
    /// daemon will actually write.
    func evaluate(temp: Double, minRPM: Double, maxRPM: Double) -> Double {
        guard temp.isFinite else { return maxRPM }
        if temp >= max_temp { return maxRPM }
        guard let first = points.first, let last = points.last else { return minRPM }

        var rpm: Double
        if temp <= first.temp {
            rpm = first.rpm
        } else if temp >= last.temp {
            rpm = last.rpm
        } else {
            rpm = last.rpm
            for i in 1..<points.count {
                let a = points[i - 1], b = points[i]
                if temp >= a.temp, temp <= b.temp {
                    let span = max(b.temp - a.temp, 0.01)
                    let t = (temp - a.temp) / span
                    rpm = a.rpm + t * (b.rpm - a.rpm)
                    break
                }
            }
        }
        return min(max(rpm, minRPM), maxRPM)
    }

    /// Sort, dedupe and clamp. Same rules as the Rust side so a round trip is stable.
    mutating func normalize() {
        max_temp = min(max(max_temp, 70), 110)
        smoothing_secs = min(max(smoothing_secs, 0), 30)
        rpm_deadband = min(max(rpm_deadband, 0), 500)
        max_rpm_per_sec = min(max(max_rpm_per_sec, 50), 4900)
        points = points.filter { $0.temp.isFinite && $0.rpm.isFinite }
        points.sort { $0.temp < $1.temp }
        var deduped: [CurvePoint] = []
        for p in points {
            if let last = deduped.last, abs(last.temp - p.temp) < 0.01 { continue }
            deduped.append(CurvePoint(temp: min(max(p.temp, 0), 120), rpm: max(p.rpm, 0)))
        }
        points = deduped
        if points.count < 2 { points = Curve.default.points }
        if points.count > 16 { points = Array(points.prefix(16)) }
    }
}

// MARK: - config

struct Config: Codable, Equatable {
    var mode: ControlMode
    var fan: Int
    var sensor: SensorGroup
    var curve: Curve
    var fixed_rpm: Double
    var emergency_temp: Double
    var poll_ms: Int
    var restore_auto_on_exit: Bool
    var log_flush_secs: Int
    var log_max_kb: Int
    var log_keep: Int
    var log_coalesce: Bool

    static let `default` = Config(
        mode: .auto,
        fan: 0,
        sensor: .cpu,
        curve: .default,
        fixed_rpm: 1800,
        emergency_temp: 98,
        poll_ms: 2000,
        restore_auto_on_exit: true,
        log_flush_secs: 1800,
        log_max_kb: 2048,
        log_keep: 3,
        log_coalesce: true
    )
}

// MARK: - snapshot

struct FanReading: Codable, Identifiable, Equatable {
    var index: Int
    var actual_rpm: Double
    var target_rpm: Double
    var min_rpm: Double
    var max_rpm: Double
    var mode: FanMode

    var id: Int { index }
}

struct SensorReading: Codable, Identifiable, Equatable {
    var group: SensorGroup
    var name: String
    var temp: Double?
    var peak: Double?
    var keys: [String]

    var id: String { group.rawValue }
}

struct Sample: Codable, Equatable {
    var t: Double
    var control_temp: Double?
    var fan_rpm: Double?
    var target_rpm: Double?
    var mode: ControlMode
}

struct Snapshot: Codable, Equatable {
    var version: Int
    var pid: Int
    var uptime_secs: Int
    var config: Config
    var fans: [FanReading]
    var sensors: [SensorReading]
    var control_temp: Double?
    var target_rpm: Double?
    var status: String
    var warnings: [String]
    var privileged: Bool
    var dry_run: Bool
    var mode: ControlMode

    var fan: FanReading? { fans.first }
}

struct Response: Codable {
    var ok: Bool
    var error: String?
    var snapshot: Snapshot?
    var history: [Sample]?
    var message: String?
}
