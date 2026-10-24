//! App state: polls the daemon, holds the snapshot/history, and forwards edits.
//!
//! All socket work happens on a private serial queue and results are handed
//! back to the main queue. The UI never blocks on the daemon.

import Foundation
import SwiftUI

final class FanStore: ObservableObject {
    enum Tab: String, CaseIterable, Identifiable {
        case overview, curve, settings
        var id: String { rawValue }
        var label: String {
            switch self {
            case .overview: return "总览"
            case .curve: return "曲线"
            case .settings: return "设置"
            }
        }
        var symbol: String {
            switch self {
            case .overview: return "gauge.medium"
            case .curve: return "chart.xyaxis.line"
            case .settings: return "gearshape"
            }
        }
    }

    struct Toast: Identifiable {
        let id = UUID()
        let text: String
        let isError: Bool
    }

    // MARK: published state

    @Published private(set) var snapshot: Snapshot?
    /// Full series, appended to on every snapshot (not observed by the chart).
    private(set) var samples: [Sample] = []
    /// Chart-facing copy, republished at most every `chartInterval` seconds.
    /// Re-rendering a `Canvas` of this size costs a graphics-buffer round trip,
    /// so it must not follow the 2 s snapshot cadence.
    @Published private(set) var chartData: [Sample] = []
    @Published private(set) var connected = false
    @Published private(set) var lastError: String?
    @Published var toasts: [Toast] = []

    /// Local edits, applied with 保存并应用.
    @Published var draft: Config = .default
    @Published private(set) var saved: Config = .default

    @Published var tab: Tab = .overview
    @Published var chartRange: ChartRange = .min5

    @Published var menuBarShowsText = true
    @Published var menuBarOnly = false

    // MARK: plumbing

    let socketPath: String
    private let work = DispatchQueue(label: "local.fanctl.ui.net", qos: .utility)
    private var timer: DispatchSourceTimer?
    private var pollTick = 0
    private var historyTick = 0
    private var lastChartPublish = Date.distantPast
    private let chartInterval: TimeInterval = 2

    init(socketPath: String = Client.defaultSocket) {
        self.socketPath = socketPath
        let prefs = UiPrefs.load()
        menuBarShowsText = prefs.menu_bar_text
        menuBarOnly = prefs.menu_bar_only
    }

    deinit { timer?.cancel() }

    func start() {
        guard timer == nil else { return }
        let timer = DispatchSource.makeTimerSource(queue: work)
        timer.schedule(deadline: .now(), repeating: .milliseconds(250), leeway: .milliseconds(50))
        timer.setEventHandler { [weak self] in self?.tick() }
        timer.resume()
        self.timer = timer
        // Fill the chart on the first frame instead of waiting for the 2 s tick.
        refreshSnapshot()
        refreshHistory()
    }

    /// 4 Hz timer; the socket polls are throttled inside `tick()`.
    ///
    /// The daemon samples the SMC every `poll_ms` (2 s by default), so polling
    /// it faster only re-renders identical data — and every re-render of a
    /// large `Chart` costs a graphics-buffer round trip. Staying at 2 s keeps
    /// the process around 40 MB instead of ~115 MB.
    private func tick() {
        pollTick += 1
        if pollTick % (windowVisible ? 8 : 12) == 0 { refreshSnapshot() }
        historyTick += 1
        if historyTick % 240 == 0 { refreshHistory() }
    }

    var windowVisible = true

    // MARK: polling

    private func refreshSnapshot() {
        let client = Client(socketPath: socketPath)
        work.async { [weak self] in
            let result = Result { try client.snapshot() }
            DispatchQueue.main.async { self?.apply(result) }
        }
    }

    private func refreshHistory() {
        let client = Client(socketPath: socketPath)
        work.async { [weak self] in
            let result = Result { try client.history(seconds: 1800) }
            DispatchQueue.main.async {
                guard let self, case .success(let samples) = result else { return }
                self.samples = samples
                self.publishChartIfDue(force: true)
            }
        }
    }

    /// Copy the series into the published chart buffer, throttled.
    private func publishChartIfDue(force: Bool = false) {
        guard force || Date().timeIntervalSince(lastChartPublish) >= chartInterval else { return }
        lastChartPublish = Date()
        let cutoff = (samples.last?.t ?? 0) - 1900
        chartData = samples.filter { $0.t >= cutoff }
    }

    private func apply(_ result: Result<Snapshot, Error>) {
        switch result {
        case .success(let snapshot):
            connected = true
            lastError = nil
            adopt(snapshot)
            // Already on the main queue here, and throttled inside.
            publishChartIfDue()
        case .failure(let error):
            connected = false
            self.snapshot = nil
            lastError = (error as? ClientError)?.errorDescription ?? error.localizedDescription
        }
    }

    private func adopt(_ next: Snapshot) {
        let first = snapshot == nil
        if first || snapshot?.config != next.config {
            saved = next.config
            if first || !draftDirty { draft = next.config }
        }
        appendSample(from: next)
        snapshot = next
    }

    /// Extend the local series from the snapshot instead of re-downloading the
    /// whole history every tick (that alone was ~30 MB of JSON churn).
    private func appendSample(from snapshot: Snapshot) {
        guard let temp = snapshot.control_temp, let fan = snapshot.fan else { return }
        let t = Date().timeIntervalSince1970
        if let last = samples.last, t - last.t < 1 { return }
        samples.append(
            Sample(
                t: t, control_temp: temp, fan_rpm: fan.actual_rpm,
                target_rpm: snapshot.target_rpm ?? fan.target_rpm, mode: snapshot.mode))
        let cutoff = t - 1900
        if let index = samples.firstIndex(where: { $0.t >= cutoff }), index > 0 {
            samples.removeFirst(index)
        }
    }

    // MARK: actions

    private func send<T: Encodable>(_ request: T, describe: String? = nil) {
        let client = Client(socketPath: socketPath)
        work.async { [weak self] in
            let result = Result { try client.call(request) }
            DispatchQueue.main.async {
                guard let self else { return }
                switch result {
                case .success(let response):
                    if !response.ok {
                        self.toast(response.error ?? "请求失败", isError: true)
                    } else if let message = response.message ?? describe {
                        self.toast(message, isError: false)
                    }
                    if let snapshot = response.snapshot { self.adopt(snapshot) }
                    if let history = response.history {
                    self.samples = history
                    self.publishChartIfDue(force: true)
                }
                case .failure(let error):
                    let text = (error as? ClientError)?.errorDescription ?? error.localizedDescription
                    self.toast(text, isError: true)
                }
            }
        }
    }

    func setMode(_ mode: ControlMode) { send(SetModeRequest(mode: mode)) }
    func setAuto() { send(AutoRequest()) }
    func reloadConfig() { send(ReloadRequest()) }
    func setFixed(_ rpm: Double) { send(SetFixedRequest(rpm: rpm, apply: true, fan: nil)) }

    func applyDraft() {
        var config = draft
        config.curve.normalize()
        draft = config
        send(SetConfigRequest(config: config), describe: "配置已保存")
    }

    func revertDraft() { draft = saved }
    func resetCurve() { draft.curve = .default }

    var draftDirty: Bool { draft != saved }

    // MARK: derived values

    var hardwareRange: ClosedRange<Double> {
        guard let fan = snapshot?.fan else { return 1000...4900 }
        return fan.min_rpm...max(fan.max_rpm, fan.min_rpm + 1)
    }

    var menuBarTitle: String {
        guard menuBarShowsText else { return "" }
        guard let snapshot else { return "—" }
        let temp = snapshot.control_temp.map { String(format: "%.0f°", $0) } ?? "—"
        guard let rpm = snapshot.fan?.actual_rpm else { return temp }
        return "\(temp) \(String(format: "%.0f", rpm))"
    }

    var menuBarDetail: String {
        guard let snapshot else { return "后台服务未运行" }
        let rpm = snapshot.fan.map { String(format: "%.0f rpm", $0.actual_rpm) } ?? ""
        return "\(snapshot.mode.label) · \(rpm)"
    }

    func historyWindow() -> [Sample] {
        guard let last = chartData.last else { return [] }
        let cutoff = last.t - chartRange.seconds
        return chartData.filter { $0.t >= cutoff }
    }

    /// Downsample to at most `limit` points for drawing.
    ///
    /// Swift Charts allocates a graphics buffer per rendered point; a full
    /// 1 550-sample window costs ~50 MB of GPU memory for a 900 pt wide plot
    /// where only ~450 columns are distinguishable anyway. Bucketing keeps the
    /// envelope (min/max within each bucket) so spikes stay visible.
    func chartSamples(limit: Int = 220) -> [Sample] {
        let window = historyWindow()
        guard window.count > limit else { return window }
        let bucket = Double(window.count) / Double(limit)
        var output: [Sample] = []
        output.reserveCapacity(limit)
        var index = 0
        while index < window.count {
            let end = min(window.count, index + Int(bucket.rounded(.up)))
            let slice = window[index..<end]
            // Keep the extremum so curves do not flatten out.
            let peak = slice.max {
                (($0.control_temp ?? 0) + $0.fan_rpm.map { $0 / 40 } .orZero)
                    < (($1.control_temp ?? 0) + $1.fan_rpm.map { $0 / 40 } .orZero)
            }
            output.append(peak ?? slice[slice.startIndex])
            index = end
        }
        return output
    }

    // MARK: prefs

    func savePrefs() {
        UiPrefs(menu_bar_only: menuBarOnly, menu_bar_text: menuBarShowsText).save()
    }

    func toast(_ text: String, isError: Bool) {
        let toast = Toast(text: text, isError: isError)
        toasts.append(toast)
        if toasts.count > 4 { toasts.removeFirst() }
        DispatchQueue.main.asyncAfter(deadline: .now() + 4) { [weak self] in
            self?.toasts.removeAll { $0.id == toast.id }
        }
    }
}

// MARK: - chart ranges

enum ChartRange: String, CaseIterable, Identifiable {
    case min1, min5, min15, hour1

    var id: String { rawValue }

    var label: String {
        switch self {
        case .min1: return "1 分钟"
        case .min5: return "5 分钟"
        case .min15: return "15 分钟"
        case .hour1: return "1 小时"
        }
    }

    var seconds: Double {
        switch self {
        case .min1: return 60
        case .min5: return 300
        case .min15: return 900
        case .hour1: return 3600
        }
    }
}

// MARK: - UI preferences (mirrors fanui's ui.json)

struct UiPrefs: Codable {
    var menu_bar_only: Bool
    var menu_bar_text: Bool

    static let `default` = UiPrefs(menu_bar_only: false, menu_bar_text: true)

    static func url() -> URL? {
        guard let home = ProcessInfo.processInfo.environment["HOME"] else { return nil }
        return URL(fileURLWithPath: home)
            .appendingPathComponent("Library/Application Support/FanCtl/ui.json")
    }

    static func load() -> UiPrefs {
        guard let url = url(), let data = try? Data(contentsOf: url),
              let prefs = try? JSONDecoder().decode(UiPrefs.self, from: data)
        else { return .default }
        return prefs
    }

    func save() {
        guard let url = Self.url() else { return }
        try? FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        guard let data = try? JSONEncoder().encode(self) else { return }
        try? data.write(to: url, options: .atomic)
    }
}

// MARK: - rendering fixture

extension FanStore {
    /// Populate a deterministic snapshot + history so `--render` can draw every
    /// page without the daemon (or a display) being present.
    @MainActor
    func seedForRendering() {
        let temps: [SensorGroup: Double] = [
            .cpu: 57.2, .pCore: 42.0, .eCore: 42.2, .gpu: 61.8,
            .soc: 32.5, .storage: 38.4, .ambient: 32.2,
        ]
        let sensors = SensorGroup.allCases.map { group in
            SensorReading(
                group: group, name: group.label, temp: temps[group],
                peak: (temps[group] ?? 0) + 6, keys: ["TCMz", "Tp01", "Te05"])
        }
        let curve = Curve(
            points: [
                CurvePoint(temp: 45, rpm: 1000), CurvePoint(temp: 70, rpm: 1000),
                CurvePoint(temp: 80, rpm: 1650), CurvePoint(temp: 86, rpm: 2300),
                CurvePoint(temp: 90, rpm: 3000), CurvePoint(temp: 95, rpm: 4020),
                CurvePoint(temp: 105, rpm: 4900),
            ],
            max_temp: 97, spin_up_rpm: 0, smoothing_secs: 2,
            rpm_deadband: 40, max_rpm_per_sec: 600)
        var config = Config.default
        config.mode = .curve
        config.sensor = .cpu
        config.curve = curve
        config.fixed_rpm = 1000
        config.poll_ms = 1000

        let snapshot = Snapshot(
            version: 1, pid: 42063, uptime_secs: 172_809, config: config,
            fans: [
                FanReading(
                    index: 0, actual_rpm: 1000, target_rpm: 1000,
                    min_rpm: 1000, max_rpm: 4900, mode: .manual)
            ],
            sensors: sensors, control_temp: 57.183586, target_rpm: 1000,
            status: "CPU 热点 · 57.2°C → 1000 rpm", warnings: [],
            privileged: true, dry_run: false, mode: .curve)

        self.snapshot = snapshot
        self.saved = config
        self.draft = config
        self.connected = true

        let now = Date().timeIntervalSince1970
        self.samples = (0..<150).map { index in
            let t = now - Double(150 - index) * 2
            let phase = Double(index) / 12
            return Sample(
                t: t,
                control_temp: 55 + 9 * sin(phase),
                fan_rpm: 1000 + 700 * cos(phase / 1.7) + 40 * sin(phase * 3),
                target_rpm: 1000 + 650 * cos(phase / 1.7),
                mode: .curve)
        }
    }
}


extension Optional where Wrapped == Double {
    var orZero: Double { self ?? 0 }
}
