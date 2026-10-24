//! Curve page: drag points on the chart, or type exact numbers below.

import SwiftUI
import Charts

struct CurvePage: View {
    @ObservedObject var store: FanStore
    var scrollable = true
    @ObservedObject private var dragging = Box(Optional<Int>.none)
    @ObservedObject private var hovered = Box(Optional<Int>.none)

    private var curve: Curve { store.draft.curve }
    private var range: ClosedRange<Double> { store.hardwareRange }

    var body: some View {
        PageScaffold(scrollable: scrollable) {
            VStack(alignment: .leading, spacing: 12) {
                if store.snapshot == nil {
                    Card {
                        Text("后台服务未运行，曲线暂时无法下发")
                            .font(.system(size: 12))
                            .foregroundStyle(Palette.warn)
                        Text("你仍可以在这里编辑，配置会在服务启动后保存。")
                            .font(.system(size: 11))
                            .foregroundStyle(Palette.muted)
                    }
                }

                headerCard
                previewCard
                editorCard
                pointsCard
            }
            .padding(16)
        }
    }

    // MARK: header

    private var headerCard: some View {
        Card {
            HStack(spacing: 8) {
                Text("风扇曲线").font(.system(size: 16, weight: .semibold))
                Spacer(minLength: 8)
                if store.draftDirty { Badge(text: "有未保存修改", colour: Palette.warn) }
                Button("恢复默认") { store.resetCurve() }
                    .controlSize(.small)
                Button("保存并应用") { store.applyDraft() }
                    .controlSize(.small)
                    .keyboardShortcut("s", modifiers: .command)
                if store.draftDirty {
                    Button("放弃修改") { store.revertDraft() }.controlSize(.small)
                }
            }
            Text("拖动圆点调整；双击空白处新增点，右键点删除。X 轴温度、Y 轴转速。")
                .font(.system(size: 11))
                .foregroundStyle(Palette.muted)
        }
    }

    // MARK: preview

    private var previewCard: some View {
        Card {
            if let temp = store.snapshot?.control_temp {
                let want = curve.evaluate(
                    temp: temp, minRPM: range.lowerBound, maxRPM: range.upperBound)
                HStack(spacing: 8) {
                    Text(String(format: "当前 %.1f°C", temp))
                        .font(.system(size: 13))
                        .foregroundStyle(Palette.forTemp(temp))
                    Text("→").foregroundStyle(Palette.muted)
                    Text(String(format: "%.0f rpm", want))
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(Palette.rpm)
                    if let actual = store.snapshot?.fan?.actual_rpm {
                        Text(String(format: "（实际 %.0f rpm）", actual))
                            .font(.system(size: 11))
                            .foregroundStyle(Palette.muted)
                    }
                }
            } else {
                Text("等待温度").font(.system(size: 12)).foregroundStyle(Palette.muted)
            }
        }
    }

    // MARK: chart editor

    private var editorCard: some View {
        Card {
            Chart {
                ForEach(Array(curve.points.enumerated()), id: \.offset) { index, point in
                    LineMark(x: .value("温度", point.temp), y: .value("转速", point.rpm))
                        .foregroundStyle(Palette.accent)
                        .lineStyle(StrokeStyle(lineWidth: 2.2))
                        .interpolationMethod(.linear)
                }
                ForEach(Array(curve.points.enumerated()), id: \.offset) { index, point in
                    PointMark(x: .value("温度", point.temp), y: .value("转速", point.rpm))
                        .foregroundStyle(
                            dragging.value == index
                                ? Palette.accent
                                : (hovered.value == index ? Palette.warn : Palette.rpm))
                        .symbolSize(dragging.value == index ? 150 : 90)
                }
                if let temp = store.snapshot?.control_temp {
                    RuleMark(x: .value("当前温度", temp))
                        .foregroundStyle(Palette.forTemp(temp).opacity(0.85))
                        .lineStyle(StrokeStyle(lineWidth: 1, dash: [4, 3]))
                        .annotation(position: .top, alignment: .leading) {
                            Text(String(format: "%.1f°C", temp))
                                .font(.system(size: 10))
                                .foregroundStyle(Palette.forTemp(temp))
                        }
                }
                RuleMark(y: .value("硬件上限", range.upperBound))
                    .foregroundStyle(Palette.muted.opacity(0.35))
                    .lineStyle(StrokeStyle(lineWidth: 1, dash: [2, 4]))
                RuleMark(y: .value("硬件下限", range.lowerBound))
                    .foregroundStyle(Palette.muted.opacity(0.35))
                    .lineStyle(StrokeStyle(lineWidth: 1, dash: [2, 4]))
            }
            .chartXScale(domain: 30...105)
            .chartYScale(domain: (range.lowerBound * 0.9)...(range.upperBound * 1.05))
            .chartXAxisLabel("温度 °C", alignment: .center)
            .chartYAxisLabel("转速 rpm")
            .chartOverlay { proxy in
                GeometryReader { geometry in
                    Rectangle()
                        .fill(.clear)
                        .contentShape(Rectangle())
                        .gesture(dragGesture(proxy: proxy, geometry: geometry))
                        .onContinuousHover { phase in
                            switch phase {
                            case .active(let location):
                                hovered.value = nearestPoint(to: location, proxy: proxy, geometry: geometry)
                            case .ended:
                                hovered.value = nil
                            }
                        }
                        .contextMenu { contextMenu }
                }
            }
            .frame(height: 320)

            Text("提示：拖动圆点时温度只能在左右相邻点之间移动；点密度上限 16 个。")
                .font(.system(size: 11))
                .foregroundStyle(Palette.muted)
        }
    }

    private func dragGesture(proxy: ChartProxy, geometry: GeometryProxy) -> some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { value in
                guard let plotFrame = proxy.plotFrame else { return }
                let anchor = geometry[plotFrame].origin
                let local = CGPoint(
                    x: value.location.x - anchor.x, y: value.location.y - anchor.y)

                if dragging.value == nil {
                    dragging.value = nearestPoint(
                        to: CGPoint(x: value.location.x, y: value.location.y),
                        proxy: proxy, geometry: geometry)
                }
                guard let index = dragging.value else { return }
                guard let temp = proxy.value(atX: local.x, as: Double.self),
                      let rpm = proxy.value(atY: local.y, as: Double.self)
                else { return }
                movePoint(index, temp: temp, rpm: rpm)
            }
            .onEnded { _ in dragging.value = nil }
    }

    private func movePoint(_ index: Int, temp: Double, rpm: Double) {
        guard curve.points.indices.contains(index) else { return }
        let lower = index == 0 ? 30.0 : curve.points[index - 1].temp + 0.5
        let upper = index == curve.points.count - 1
            ? 110.0
            : curve.points[index + 1].temp - 0.5
        var points = curve.points
        points[index].temp = min(max(temp, lower), max(lower, upper))
        points[index].rpm = min(max(rpm, range.lowerBound), range.upperBound)
        store.draft.curve.points = points
    }

    private func nearestPoint(to location: CGPoint, proxy: ChartProxy, geometry: GeometryProxy) -> Int? {
        guard let plotFrame = proxy.plotFrame else { return nil }
        let anchor = geometry[plotFrame].origin
        var best: (index: Int, distance: CGFloat)?
        for (index, point) in curve.points.enumerated() {
            guard let x = proxy.position(forX: point.temp),
                  let y = proxy.position(forY: point.rpm)
            else { continue }
            let dx = location.x - (x + anchor.x)
            let dy = location.y - (y + anchor.y)
            let distance = (dx * dx + dy * dy).squareRoot()
            if distance <= 18, best == nil || distance < best!.distance {
                best = (index, distance)
            }
        }
        return best?.index
    }

    @ViewBuilder
    private var contextMenu: some View {
        if let index = dragging.value ?? hovered.value, curve.points.indices.contains(index) {
            Button("删除这个控制点") {
                var points = curve.points
                guard points.count > 2 else { return }
                points.remove(at: index)
                store.draft.curve.points = points
            }
        }
        Button("新增控制点（在当前温度）") { appendPoint() }
        Divider()
        Button("恢复默认曲线") { store.resetCurve() }
    }

    private func appendPoint() {
        var points = curve.points
        guard points.count < 16 else { return }
        let temp = store.snapshot?.control_temp ?? 70
        guard !points.contains(where: { abs($0.temp - temp) < 0.6 }) else { return }
        let rpm = curve.evaluate(temp: temp, minRPM: range.lowerBound, maxRPM: range.upperBound)
        points.append(CurvePoint(temp: temp, rpm: rpm))
        points.sort { $0.temp < $1.temp }
        store.draft.curve.points = points
    }

    // MARK: point table

    private var pointsCard: some View {
        Card {
            CardTitle(text: "控制点", trailing: "\(curve.points.count) / 16")
            Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 6) {
                GridRow {
                    Text("温度 °C").font(.system(size: 11)).foregroundStyle(Palette.muted)
                    Text("转速 rpm").font(.system(size: 11)).foregroundStyle(Palette.muted)
                    Text("").font(.system(size: 11))
                }
                ForEach(Array(curve.points.enumerated()), id: \.offset) { index, point in
                    GridRow {
                        TextField(
                            "", value: Binding(
                                get: { curve.points[index].temp },
                                set: { store.draft.curve.points[index].temp = $0 }),
                            format: .number.precision(.fractionLength(0...1))
                        )
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 74)

                        TextField(
                            "", value: Binding(
                                get: { curve.points[index].rpm },
                                set: { store.draft.curve.points[index].rpm = $0 }),
                            format: .number.precision(.fractionLength(0...0))
                        )
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 90)

                        Button {
                            guard curve.points.count > 2 else { return }
                            var points = curve.points
                            points.remove(at: index)
                            store.draft.curve.points = points
                        } label: {
                            Image(systemName: "trash").font(.system(size: 11))
                        }
                        .buttonStyle(.borderless)
                        .disabled(curve.points.count <= 2)
                    }
                }
            }
            HStack(spacing: 8) {
                Button("新增控制点") { appendPoint() }
                    .controlSize(.small)
                    .disabled(curve.points.count >= 16)
                Button("按温度排序") {
                    var points = curve.points
                    points.sort { $0.temp < $1.temp }
                    store.draft.curve.points = points
                }
                .controlSize(.small)
            }
            Text("修改后点「保存并应用」才会下发给守护进程。")
                .font(.system(size: 11))
                .foregroundStyle(Palette.muted)
        }
    }
}
