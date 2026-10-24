//! `FanCtl --shot <path>`: launch normally, then capture the real window with
//! `NSView.cacheDisplay` and quit. This grabs AppKit-backed controls (text
//! fields, sliders, pickers) that `ImageRenderer` can only stub out, and it
//! needs no screen-recording permission because it reads our own view tree.

import AppKit

enum WindowShot {
    @MainActor
    static var pendingPath: String?
    @MainActor
    private static var attempts = 0

    @MainActor
    static func scheduleIfRequested() {
        guard let flag = CommandLine.arguments.firstIndex(of: "--shot"),
              CommandLine.arguments.count > flag + 1
        else { return }
        pendingPath = CommandLine.arguments[flag + 1]
        if let tabIndex = CommandLine.arguments.firstIndex(of: "--tab"),
           CommandLine.arguments.count > tabIndex + 1,
           let tab = Int(CommandLine.arguments[tabIndex + 1]) {
            AppDelegate.requestedTab = tab
        }
        Timer.scheduledTimer(withTimeInterval: 1.6, repeats: true) { _ in
            Task { @MainActor in tryCapture() }
        }
    }

    @MainActor
    private static func tryCapture() {
        attempts += 1
        guard attempts > 14 else { return capture() }
        print("没有找到可截图的窗口")
        NSApp.terminate(nil)
    }

    @MainActor
    private static func capture() {
        guard let path = pendingPath else { return }
        guard let window = NSApp.windows.first(where: {
            $0.isVisible && $0.contentView != nil && $0.contentView!.bounds.width > 200
        }) else { return }
        guard let view = window.contentView else { return }
        let bounds = view.bounds
        guard bounds.width > 1, bounds.height > 1,
              let rep = view.bitmapImageRepForCachingDisplay(in: bounds)
        else { return }
        view.cacheDisplay(in: bounds, to: rep)
        guard let png = rep.representation(using: .png, properties: [:]) else { return }
        try? png.write(to: URL(fileURLWithPath: path))
        print("已保存 \(path)  \(rep.pixelsWide)x\(rep.pixelsHigh)")
        pendingPath = nil
        NSApp.terminate(nil)
    }
}
