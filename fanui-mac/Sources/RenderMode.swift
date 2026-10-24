//! Offscreen rendering for visual review: `FanCtl --render <dir>`.
//!
//! Draws each page into a PNG with `ImageRenderer` so layout regressions can be
//! checked without a window, a display, or screen-recording permission. The
//! data comes from the live daemon when it is reachable, otherwise from a
//! synthetic snapshot that exercises every state the UI can show.

import SwiftUI
import AppKit

enum RenderMode {
    @MainActor
    static func run(outputDirectory: String) -> Never {
        let directory = URL(fileURLWithPath: outputDirectory)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        let store = FanStore()
        store.seedForRendering()

        let pages: [(String, AnyView)] = [
            ("overview", AnyView(OverviewPage(store: store, scrollable: false))),
            ("curve", AnyView(CurvePage(store: store, scrollable: false))),
            ("settings", AnyView(SettingsPage(store: store, scrollable: false))),
            ("window", AnyView(RootView(store: store, scrollable: false))),
        ]

        for (name, view) in pages {
            render(view, size: CGSize(width: 1000, height: 820), to: directory, name: name)
        }

        // The offline state is a different branch of the same views, so it
        // needs its own store rather than a flag on the seeded one.
        let offline = FanStore()
        offline.seedOfflineForRendering()
        render(
            AnyView(RootView(store: offline, scrollable: false)),
            size: CGSize(width: 1000, height: 820), to: directory, name: "offline")

        print("已输出到 \(directory.path)")
        exit(0)
    }

    @MainActor
    private static func render(_ view: AnyView, size: CGSize, to directory: URL, name: String) {
        let renderer = ImageRenderer(
            content: view
                .frame(width: size.width, height: size.height)
                .background(Palette.panel)
                .environment(\.colorScheme, .dark)
        )
        renderer.scale = 2

        guard let image = renderer.nsImage,
              let tiff = image.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiff),
              let png = bitmap.representation(using: .png, properties: [:])
        else {
            print("渲染 \(name) 失败")
            return
        }
        let url = directory.appendingPathComponent("\(name).png")
        try? png.write(to: url)
        print("  \(name).png  \(Int(size.width))x\(Int(size.height))")
    }
}
