//! Overview page: live stats, the seven sensor groups and the history chart.

import SwiftUI

struct OverviewPage: View {
    @ObservedObject var store: FanStore
    /// `ImageRenderer` cannot lay out a `ScrollView`, so `--render` draws the
    /// plain stack instead.
    var scrollable = true

    var body: some View {
        PageScaffold(scrollable: scrollable) {
            VStack(alignment: .leading, spacing: 12) {
                if let snapshot = store.snapshot {
                    statusCard(snapshot)
                    HStack(alignment: .top, spacing: 12) {
                        sensorCard(snapshot)
                        policyCard(snapshot)
                    }
                    chartCard()
                    if !snapshot.warnings.isEmpty {
                        Card {
                            ForEach(snapshot.warnings, id: \.self) { warning in
                                Label(warning, systemImage: "exclamationmark.triangle.fill")
                                    .font(.system(size: 12))
                                    .foregroundStyle(Palette.warn)
                            }
                        }
                    }
                } else {
                    offlineCard
                }
            }
            .padding(16)
        }
    }

    // MARK: status

    private func statusCard(_ snapshot: Snapshot) -> some View {
        let fan = snapshot.fan
        let fraction: Double = {
            guard let fan, fan.max_rpm > fan.min_rpm else { return 0 }
            return (fan.actual_rpm - fan.min_rpm) / (fan.max_rpm - fan.min_rpm)
        }()

        return Card(spacing: 12) {
            HStack(alignment: .top, spacing: 22) {
                Stat(
                    value: snapshot.control_temp.map { String(format: "%.1f", $0) } ?? "--",
                    unit: "°C",
                    caption: "\(snapshot.config.sensor.label)（曲线输入）",
                    colour: snapshot.control_temp.map(Palette.forTemp) ?? Palette.muted)

                Stat(
                    value: String(format: "%.0f", fan?.actual_rpm ?? 0),
                    unit: "rpm", caption: "实际转速", colour: Palette.rpm)

                Stat(
                    value: (snapshot.target_rpm ?? fan?.target_rpm)
                        .map { String(format: "%.0f", $0) } ?? "--",
                    unit: "rpm", caption: "目标转速", colour: Palette.accent)

                Stat(
                    value: String(format: "%.0f–%.0f", fan?.min_rpm ?? 1000, fan?.max_rpm ?? 4900),
                    unit: "rpm", caption: "硬件范围", colour: Palette.muted)

                Spacer(minLength: 12)

                VStack(alignment: .trailing, spacing: 5) {
                    HStack(spacing: 6) {
                        if snapshot.dry_run { Badge(text: "只读预览", colour: Palette.warn) }
                        Badge(text: snapshot.mode.label, colour: Palette.forMode(snapshot.mode))
                    }
                    Text(snapshot.status)
                        .font(.system(size: 11))
                        .foregroundStyle(Palette.muted)
                    Text("PID \(snapshot.pid) · 已运行 \(uptimeText(snapshot.uptime_secs))")
                        .font(.system(size: 11))
                        .foregroundStyle(Palette.muted)
                }
            }
            Meter(fraction: fraction, colour: Palette.rpm, height: 6)
        }
    }

    // MARK: sensors

    private func sensorCard(_ snapshot: Snapshot) -> some View {
        Card {
            CardTitle(
                text: "温度传感器",
                trailing: "共 \(snapshot.sensors.reduce(0) { $0 + $1.keys.count }) 个键")
            VStack(spacing: 2) {
                ForEach(snapshot.sensors) { sensor in
                    SensorRow(sensor: sensor, selected: sensor.group == snapshot.config.sensor)
                }
            }
            Text("高亮行是当前曲线使用的温度源，可在「设置」里切换")
                .font(.system(size: 11))
                .foregroundStyle(Palette.muted)
        }
        .frame(maxWidth: .infinity)
    }

    // MARK: policy

    private func policyCard(_ snapshot: Snapshot) -> some View {
        Card {
            CardTitle(text: "控制方式")
            VStack(spacing: 4) {
                ForEach(ControlMode.allCases) { mode in
                    ModeButton(
                        mode: mode,
                        selected: snapshot.mode == mode,
                        action: { store.setMode(mode) })
                }
            }
            detail(snapshot)
        }
        .frame(maxWidth: .infinity)
    }

    @ViewBuilder
    private func detail(_ snapshot: Snapshot) -> some View {
        let range = store.hardwareRange
        switch snapshot.mode {
        case .fixed:
            HStack(spacing: 8) {
                Text("目标转速").font(.system(size: 12))
                Slider(
                    value: Binding(
                        get: { store.draft.fixed_rpm },
                        set: { store.draft.fixed_rpm = $0 }),
                    in: range, step: 50
                )
                .frame(width: 160)
                Text(String(format: "%.0f rpm", store.draft.fixed_rpm))
                    .font(.system(size: 12)).monospacedDigit()
                Button("应用") { store.setFixed(store.draft.fixed_rpm) }
                    .controlSize(.small)
            }
        case .curve:
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Text("曲线").font(.system(size: 12)).foregroundStyle(Palette.muted)
                    if let temp = snapshot.control_temp {
                        let rpm = snapshot.config.curve.evaluate(
                            temp: temp, minRPM: range.lowerBound, maxRPM: range.upperBound)
                        Text(String(format: "%.1f°C → %.0f rpm", temp, rpm))
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(Palette.ok)
                    } else {
                        Text("等待温度").font(.system(size: 12)).foregroundStyle(Palette.muted)
                    }
                }
                Text(String(
                    format: "平滑 %.0fs · 回差 %.0f rpm · 限速 %.0f rpm/s · %.0f°C 以上全速",
                    snapshot.config.curve.smoothing_secs,
                    snapshot.config.curve.rpm_deadband,
                    snapshot.config.curve.max_rpm_per_sec,
                    snapshot.config.curve.max_temp))
                    .font(.system(size: 11))
                    .foregroundStyle(Palette.muted)
            }
        case .auto:
            Text(String(
                format: "macOS 会在 %.0f–%.0f rpm 之间自行调节，fanctl 不干预。",
                range.lowerBound, range.upperBound))
                .font(.system(size: 11))
                .foregroundStyle(Palette.muted)
        }

        Button {
            store.tab = .curve
        } label: {
            Label("编辑曲线…", systemImage: "slider.horizontal.3")
                .font(.system(size: 12))
        }
        .buttonStyle(.link)
    }

    // MARK: chart

    private func chartCard() -> some View {
        let samples = store.chartSamples(limit: 400)
        let now = samples.last?.t ?? 0
        let span = store.chartRange.seconds

        let temp = samples.compactMap { sample -> ChartSeries.Point? in
            sample.control_temp.map { .init(x: sample.t - now, y: $0) }
        }
        let rpm = samples.compactMap { sample -> ChartSeries.Point? in
            sample.fan_rpm.map { .init(x: sample.t - now, y: $0) }
        }
        let target = samples.compactMap { sample -> ChartSeries.Point? in
            sample.target_rpm.map { .init(x: sample.t - now, y: $0) }
        }

        return Card {
            HStack(spacing: 8) {
                CardTitle(text: "温度 / 转速")
                Picker("", selection: Binding(
                    get: { store.chartRange }, set: { store.chartRange = $0 })) {
                    ForEach(ChartRange.allCases) { range in
                        Text(range.label).tag(range)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(width: 260)
                Spacer(minLength: 8)
                Text("\(store.chartData.count) 个采样")
                    .font(.system(size: 11))
                    .foregroundStyle(Palette.muted)
            }

            if samples.isEmpty {
                Text("暂无历史数据")
                    .font(.system(size: 12))
                    .foregroundStyle(Palette.muted)
                    .frame(maxWidth: .infinity, minHeight: 220)
            } else {
                HistoryChart(
                    series: [
                        ChartSeries(
                            name: "温度", colour: Palette.temp, dashed: false, points: temp),
                        ChartSeries(
                            name: "实际转速", colour: Palette.rpm, dashed: false,
                            points: rpm, scale: rpmScale),
                        ChartSeries(
                            name: "目标转速", colour: Palette.accent.opacity(0.75),
                            dashed: true, points: target, scale: rpmScale),
                    ],
                    xRange: -span...0,
                    yRange: 0...120,
                    xLabel: relativeLabel,
                    rightLabel: { tick in String(format: "%.0f", tick / rpmScale) })
                .frame(height: 250)
                .padding(.top, 4)

                ChartLegend(items: [
                    ("温度 °C", Palette.temp),
                    ("实际转速 rpm", Palette.rpm),
                    ("目标转速 rpm", Palette.accent.opacity(0.75)),
                ])
            }
        }
    }

    /// 4900 rpm maps to 120 °C so both series fit one axis.
    private var rpmScale: Double {
        let top = max(store.hardwareRange.upperBound, 1000)
        return 120 / top
    }

    private func relativeLabel(_ seconds: Double) -> String {
        if abs(seconds) < 0.5 { return "现在" }
        if abs(seconds) < 60 { return String(format: "%.0fs", seconds) }
        return String(format: "%.0f:%.0f", (seconds / 60).rounded(.towardZero), abs(seconds).truncatingRemainder(dividingBy: 60))
    }

    // MARK: offline

    private var offlineCard: some View {
        Card(spacing: 10) {
            Text("后台服务未运行")
                .font(.system(size: 20, weight: .semibold))
            Text("风扇控制需要 root 权限的守护进程。安装一次即可，它会随系统启动。")
                .font(.system(size: 12))
                .foregroundStyle(Palette.muted)
            if let error = store.lastError {
                Text(error)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(Palette.danger)
                    .textSelection(.enabled)
            }
            HStack(spacing: 8) {
                Button("重试连接") { store.reloadConfig() }
                Button("打开安装说明") {
                    NSWorkspace.shared.open(URL(fileURLWithPath: installScript()))
                }
            }
            Text("命令行替代方案： sudo /usr/local/libexec/fanctl/fand  （或 cargo run -p fand -- --dry-run 只观察）")
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(Palette.muted)
        }
    }

    private func installScript() -> String {
        let bundle = Bundle.main.bundlePath
        var candidates = [
            bundle + "/../../install.sh",
            FileManager.default.currentDirectoryPath + "/install.sh",
        ]
        if let env = ProcessInfo.processInfo.environment["FANCTL_SRC"] {
            candidates.insert(env + "/install.sh", at: 0)
        }
        return candidates.first { FileManager.default.fileExists(atPath: $0) } ?? "/tmp"
    }
}

// MARK: - small components

struct ModeButton: View {
    let mode: ControlMode
    let selected: Bool
    let action: () -> Void
    @ObservedObject private var hover = Box(false)

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: selected ? "largecircle.fill.circle" : "circle")
                    .font(.system(size: 11))
                    .foregroundStyle(selected ? Palette.accent : Palette.muted)
                Text(mode.blurb)
                    .font(.system(size: 13))
                    .foregroundStyle(selected ? Color.white : Color(white: 0.8))
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .background(
                RoundedRectangle(cornerRadius: 6)
                    .fill(selected ? Palette.accent.opacity(0.16) : (hover.value ? Palette.raised : .clear)))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover.value = $0 }
    }
}

struct ChartLegend: View {
    let items: [(String, Color)]

    var body: some View {
        HStack(spacing: 14) {
            ForEach(items, id: \.0) { item in
                HStack(spacing: 5) {
                    Circle().fill(item.1).frame(width: 7, height: 7)
                    Text(item.0).font(.system(size: 10)).foregroundStyle(Palette.muted)
                }
            }
            Spacer(minLength: 0)
        }
    }
}
