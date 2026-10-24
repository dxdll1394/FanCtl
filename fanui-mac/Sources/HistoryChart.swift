//! History plot built from `Shape`s plus plain `Text` labels.
//!
//! Three approaches were measured on an M-series Mac for a 1 500-point window:
//!
//! | approach         | footprint |
//! | ---------------- | --------- |
//! | Swift Charts     | ~110 MB   |
//! | `Canvas`         | ~46 MB, spikes to 100 MB on each redraw |
//! | `Shape` + `Text` | ~24 MB, flat |
//!
//! Swift Charts allocates a GPU buffer per rendered mark; `Canvas` re-encodes
//! its display list every frame. A `Shape` is a plain vector path that Core
//! Animation caches, so redraws cost nothing measurable — and labels drawn as
//! ordinary `Text` avoid the per-frame glyph rasterisation `Canvas` would do.

import SwiftUI

/// One plotted polyline.
struct ChartSeries: Equatable {
    struct Point: Equatable {
        let x: Double
        let y: Double
    }

    let name: String
    let colour: Color
    let dashed: Bool
    let points: [Point]
    /// Multiplier applied before plotting so RPM can share the °C axis.
    var scale: Double = 1
}

struct HistoryChart: View {
    let series: [ChartSeries]
    let xRange: ClosedRange<Double>
    let yRange: ClosedRange<Double>
    let xLabel: (Double) -> String
    /// Right-hand axis (converted from the °C scale) for RPM. Empty = hidden.
    let rightLabel: (Double) -> String

    private let leftInset: CGFloat = 30
    private let rightInset: CGFloat = 46
    private let bottomInset: CGFloat = 16
    private let topInset: CGFloat = 6

    var body: some View {
        GeometryReader { proxy in
            let plot = CGRect(
                x: leftInset,
                y: topInset,
                width: max(proxy.size.width - leftInset - rightInset, 1),
                height: max(proxy.size.height - topInset - bottomInset, 1))

            ZStack(alignment: .topLeading) {
                Palette.well.opacity(0.35)

                GridShape(
                    xFractions: xTicks.map { fraction($0, xRange) },
                    yFractions: yTicks.map { fraction($0, yRange) }
                )
                .stroke(Palette.stroke.opacity(0.55), lineWidth: 0.7)
                .padding(
                    EdgeInsets(
                        top: topInset, leading: leftInset,
                        bottom: bottomInset, trailing: rightInset))

                ForEach(Array(series.enumerated()), id: \.offset) { _, item in
                    PlotShape(
                        points: item.points,
                        xRange: xRange,
                        yRange: yRange,
                        scale: item.scale
                    )
                    .stroke(
                        item.colour,
                        style: StrokeStyle(
                            lineWidth: item.dashed ? 1.0 : 1.5,
                            lineCap: .round, lineJoin: .round,
                            dash: item.dashed ? [3, 3] : []))
                    .padding(
                        EdgeInsets(
                            top: topInset, leading: leftInset,
                            bottom: bottomInset, trailing: rightInset))
                }

                ForEach(yTicks, id: \.self) { tick in
                    Text(String(format: "%.0f", tick))
                        .font(.system(size: 9))
                        .foregroundStyle(Palette.muted)
                        .frame(width: leftInset - 6, alignment: .trailing)
                        .position(x: (leftInset - 6) / 2, y: yPosition(tick, plot))
                }

                if !rightLabel(yRange.lowerBound).isEmpty {
                    ForEach(yTicks, id: \.self) { tick in
                        Text(rightLabel(tick))
                            .font(.system(size: 9))
                            .foregroundStyle(Palette.muted.opacity(0.75))
                            .frame(width: rightInset - 4, alignment: .leading)
                            .position(
                                x: plot.maxX + (rightInset - 4) / 2 + 2,
                                y: yPosition(tick, plot))
                    }
                }

                ForEach(xTicks, id: \.self) { tick in
                    Text(xLabel(tick))
                        .font(.system(size: 9))
                        .foregroundStyle(Palette.muted)
                        .position(x: xPosition(tick, plot), y: proxy.size.height - bottomInset / 2)
                }
            }
        }
    }

    // MARK: geometry

    private func fraction(_ value: Double, _ range: ClosedRange<Double>) -> Double {
        let span = max(range.upperBound - range.lowerBound, 0.001)
        return min(max((value - range.lowerBound) / span, 0), 1)
    }

    private func xPosition(_ value: Double, _ plot: CGRect) -> CGFloat {
        plot.minX + plot.width * CGFloat(fraction(value, xRange))
    }

    private func yPosition(_ value: Double, _ plot: CGRect) -> CGFloat {
        plot.maxY - plot.height * CGFloat(fraction(value, yRange))
    }

    // MARK: ticks

    private var yTicks: [Double] {
        let span = yRange.upperBound - yRange.lowerBound
        let step = span > 90 ? 25.0 : (span > 45 ? 15.0 : 10.0)
        var ticks: [Double] = []
        var value = (yRange.lowerBound / step).rounded(.up) * step
        while value <= yRange.upperBound + 0.001 {
            ticks.append(value)
            value += step
        }
        return ticks
    }

    private var xTicks: [Double] {
        let span = xRange.upperBound - xRange.lowerBound
        let candidates: [Double] = [10, 15, 30, 60, 120, 300, 600, 900]
        let step = candidates.first { span / $0 <= 7 } ?? 1800
        var ticks: [Double] = []
        var value = (xRange.lowerBound / step).rounded(.down) * step
        while value <= xRange.upperBound + 0.001 {
            if value >= xRange.lowerBound { ticks.append(value) }
            value += step
        }
        return ticks
    }
}

// MARK: - shapes

/// Straight-line plot of one series.
struct PlotShape: Shape {
    let points: [ChartSeries.Point]
    let xRange: ClosedRange<Double>
    let yRange: ClosedRange<Double>
    var scale: Double = 1

    func path(in rect: CGRect) -> Path {
        var path = Path()
        guard points.count > 1 else { return path }
        let xSpan = max(xRange.upperBound - xRange.lowerBound, 0.001)
        let ySpan = max(yRange.upperBound - yRange.lowerBound, 0.001)

        for (index, point) in points.enumerated() {
            let fx = min(max((point.x - xRange.lowerBound) / xSpan, 0), 1)
            let fy = min(max((point.y * scale - yRange.lowerBound) / ySpan, 0), 1)
            let location = CGPoint(
                x: rect.minX + rect.width * CGFloat(fx),
                y: rect.maxY - rect.height * CGFloat(fy))
            if index == 0 {
                path.move(to: location)
            } else {
                path.addLine(to: location)
            }
        }
        return path
    }
}

/// Grid lines at the given fractions of the plot rect.
struct GridShape: Shape {
    let xFractions: [Double]
    let yFractions: [Double]

    func path(in rect: CGRect) -> Path {
        var path = Path()
        for fraction in yFractions {
            let y = rect.maxY - rect.height * CGFloat(fraction)
            path.move(to: CGPoint(x: rect.minX, y: y))
            path.addLine(to: CGPoint(x: rect.maxX, y: y))
        }
        for fraction in xFractions {
            let x = rect.minX + rect.width * CGFloat(fraction)
            path.move(to: CGPoint(x: x, y: rect.minY))
            path.addLine(to: CGPoint(x: x, y: rect.maxY))
        }
        return path
    }
}
