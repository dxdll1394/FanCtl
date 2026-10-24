//! Settings page: sensor source, safety limits, appearance and diagnostics.

import SwiftUI

struct SettingsPage: View {
    @ObservedObject var store: FanStore
    var scrollable = true

    private var availableGroups: [SensorGroup] {
        guard let snapshot = store.snapshot else { return SensorGroup.allCases }
        let live = snapshot.sensors.filter { $0.temp != nil }.map(\.group)
        return live.isEmpty ? SensorGroup.allCases : live
    }

    var body: some View {
        PageScaffold(scrollable: scrollable) {
            VStack(alignment: .leading, spacing: 12) {
                sensorCard
                safetyCard
                appearanceCard
                diagnosticsCard
            }
            .padding(16)
        }
    }

    // MARK: sensor source

    private var sensorCard: some View {
        Card {
            CardTitle(text: "温度源")
            Text("曲线用哪一组传感器作为输入。Apple 的调速以封装热点为主，一般保持默认即可。")
                .font(.system(size: 11))
                .foregroundStyle(Palette.muted)

            VStack(alignment: .leading, spacing: 3) {
                ForEach(availableGroups) { group in
                    let selected = store.draft.sensor == group
                    Button {
                        guard !selected else { return }
                        store.draft.sensor = group
                        store.applyDraft()
                    } label: {
                        HStack(spacing: 8) {
                            Image(systemName: selected ? "largecircle.fill.circle" : "circle")
                                .font(.system(size: 11))
                                .foregroundStyle(selected ? Palette.accent : Palette.muted)
                            Text(group.label).font(.system(size: 13))
                            if let temp = tempFor(group) {
                                Text(String(format: "%.1f°C", temp))
                                    .font(.system(size: 11))
                                    .monospacedDigit()
                                    .foregroundStyle(Palette.forTemp(temp))
                            }
                            Text(group.detail)
                                .font(.system(size: 11))
                                .foregroundStyle(Palette.muted)
                            Spacer(minLength: 0)
                        }
                        .padding(.horizontal, 6)
                        .padding(.vertical, 4)
                        .background(
                            RoundedRectangle(cornerRadius: 5)
                                .fill(selected ? Palette.accent.opacity(0.12) : .clear))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }

    private func tempFor(_ group: SensorGroup) -> Double? {
        store.snapshot?.sensors.first { $0.group == group }?.temp
    }

    // MARK: safety

    private var safetyCard: some View {
        Card {
            CardTitle(text: "安全")
            Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 10) {
                GridRow {
                    Text("紧急全速温度").font(.system(size: 12))
                    HStack(spacing: 8) {
                        Slider(
                            value: Binding(
                                get: { store.draft.emergency_temp },
                                set: { store.draft.emergency_temp = $0 }),
                            in: 70...110, step: 1)
                        .frame(width: 180)
                        Text(String(format: "%.0f °C", store.draft.emergency_temp))
                            .font(.system(size: 12)).monospacedDigit()
                    }
                    .help("达到这个温度立即全速，不管曲线怎么设置")
                }
                GridRow {
                    Text("退出时交还系统").font(.system(size: 12))
                    Toggle(
                        "守护进程停止或退出时恢复自动",
                        isOn: Binding(
                            get: { store.draft.restore_auto_on_exit },
                            set: { store.draft.restore_auto_on_exit = $0 }))
                    .toggleStyle(.checkbox)
                    .font(.system(size: 12))
                }
                GridRow {
                    Text("采样间隔").font(.system(size: 12))
                    HStack(spacing: 8) {
                        Slider(
                            value: Binding(
                                get: { Double(store.draft.poll_ms) },
                                set: { store.draft.poll_ms = Int($0) }),
                            in: 500...5000, step: 100)
                        .frame(width: 180)
                        Text("\(store.draft.poll_ms) ms")
                            .font(.system(size: 12)).monospacedDigit()
                    }
                    .help("越小响应越快，CPU 占用越高（默认 1000 ms）")
                }
            }
            Text("保护逻辑：温度传感器连续 5 秒读不到时自动交还 macOS；写入失败会记录到日志并保持风扇在安全转速。")
                .font(.system(size: 11))
                .foregroundStyle(Palette.muted)
            HStack(spacing: 8) {
                Button("保存并应用") { store.applyDraft() }
                    .controlSize(.small)
                if store.draftDirty {
                    Button("放弃修改") { store.revertDraft() }.controlSize(.small)
                }
            }
        }
    }

    // MARK: appearance

    private var appearanceCard: some View {
        Card {
            CardTitle(text: "界面")
            Toggle(
                "关闭窗口后留在菜单栏（否则直接退出界面）",
                isOn: Binding(
                    get: { store.menuBarOnly },
                    set: {
                        store.menuBarOnly = $0
                        store.savePrefs()
                    }))
            .toggleStyle(.checkbox)
            .font(.system(size: 12))

            Text("无论选哪个，后台守护进程都会继续按曲线调速；这里只决定界面窗口的去留。")
                .font(.system(size: 11))
                .foregroundStyle(Palette.muted)

            Toggle(
                "菜单栏显示温度与转速",
                isOn: Binding(
                    get: { store.menuBarShowsText },
                    set: {
                        store.menuBarShowsText = $0
                        store.savePrefs()
                    }))
            .toggleStyle(.checkbox)
            .font(.system(size: 12))

            Button("立即交还系统自动控制") { store.setAuto() }
                .controlSize(.small)

            Text("守护进程配置：/Library/Application Support/FanCtl/config.json")
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(Palette.muted)
            Text("界面偏好：~/Library/Application Support/FanCtl/ui.json")
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(Palette.muted)
        }
    }

    // MARK: diagnostics

    private var diagnosticsCard: some View {
        Card {
            CardTitle(text: "诊断")
            if let snapshot = store.snapshot {
                Label(
                    "已连接 · PID \(snapshot.pid) · 协议 v\(snapshot.version) · "
                        + (snapshot.dry_run ? "只读预览" : "可写"),
                    systemImage: "checkmark.circle.fill"
                )
                .font(.system(size: 12))
                .foregroundStyle(Palette.ok)
            } else {
                Label("未连接后台服务", systemImage: "xmark.circle.fill")
                    .font(.system(size: 12))
                    .foregroundStyle(Palette.danger)
            }
            Text("socket：\(store.socketPath)")
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(Palette.muted)

            HStack(spacing: 8) {
                Button("打开日志") {
                    NSWorkspace.shared.open(URL(fileURLWithPath: "/Library/Logs/FanCtl/daemon.log"))
                }
                Button("打开配置目录") {
                    NSWorkspace.shared.open(
                        URL(fileURLWithPath: "/Library/Application Support/FanCtl"))
                }
                Button("复制自检命令") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString("fanctl doctor", forType: .string)
                    store.toast("已复制：fanctl doctor", isError: false)
                }
                Button("重新载入配置") { store.reloadConfig() }
            }
            .controlSize(.small)

            if let error = store.lastError {
                Text(error)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(Palette.warn)
                    .textSelection(.enabled)
            }
        }
    }
}
