//! Entry point: a menu-bar extra plus one main window.
//!
//! `LSUIElement` keeps it out of the Dock (the app is a status-bar utility), so
//! the window has to be summoned explicitly from the menu.

import SwiftUI
import AppKit

struct FanCtlUI: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @StateObject private var store = FanStore()

    var body: some Scene {
        Window("FanCtl", id: WindowID.main) {
            RootView(store: store)
                .frame(minWidth: 900, minHeight: 700)
                .preferredColorScheme(.dark)
                .onAppear {
                    if let requested = AppDelegate.requestedTab,
                       let tab = FanStore.Tab.allCases[safe: requested] {
                        store.tab = tab
                    }
                    store.start()
                    store.windowVisible = true
                    delegate.store = store
                }
                .onDisappear { store.windowVisible = false }
        }
        .windowToolbarStyle(.unified(showsTitle: false))
        .defaultSize(width: 1000, height: 820)
        .commands {
            CommandGroup(replacing: .appTermination) {
                Button("退出 FanCtl") { NSApp.terminate(nil) }
                    .keyboardShortcut("q")
            }
            CommandMenu("控制") {
                Button("系统自动") { store.setMode(.auto) }
                Button("固定转速") { store.setMode(.fixed) }
                Button("温度曲线") { store.setMode(.curve) }
                Divider()
                Button("重新载入配置") { store.reloadConfig() }
            }
        }

        MenuBarExtra {
            MenuContent(store: store)
        } label: {
            // A `°`-suffixed temperature is more legible than an icon in the
            // menu bar, and matches the egui build's format exactly.
            Text(store.menuBarTitle.isEmpty ? " " : store.menuBarTitle)
                .monospacedDigit()
        }
        .menuBarExtraStyle(.menu)
    }
}

enum WindowID {
    static let main = "main"
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    var store: FanStore?
    static var requestedTab: Int?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Menu-bar-only utility: no Dock icon, no app switcher entry.
        if ProcessInfo.processInfo.environment["FANCTL_NO_WINDOW"] != nil {
            NSApp.setActivationPolicy(.accessory)
            return
        }
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)
        NSApp.windows.first?.makeKeyAndOrderFront(nil)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        // Closing the window must not kill the UI when the user asked it to
        // stay in the menu bar; the daemon keeps running either way.
        guard let store else { return true }
        return !store.menuBarOnly
    }
}
