//! Menu-bar dropdown: live reading, quick mode switching, window toggle, quit.

import SwiftUI
import AppKit

struct MenuContent: View {
    @ObservedObject var store: FanStore
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Group {
            if let snapshot = store.snapshot {
                Button {
                    openWindow(id: WindowID.main)
                    NSApp.activate(ignoringOtherApps: true)
                } label: {
                    Text("\(snapshot.status)")
                }

                Divider()

                ForEach(ControlMode.allCases) { mode in
                    Button {
                        store.setMode(mode)
                    } label: {
                        Text((snapshot.mode == mode ? "✓ " : "　") + mode.label)
                    }
                }

                Divider()

                Button("显示主窗口") {
                    openWindow(id: WindowID.main)
                    NSApp.activate(ignoringOtherApps: true)
                }
                Button("打开日志") {
                    NSWorkspace.shared.open(URL(fileURLWithPath: "/Library/Logs/FanCtl/daemon.log"))
                }
            } else {
                Text("后台服务未运行")
                Text(store.lastError ?? "")
                Button("显示主窗口") {
                    openWindow(id: WindowID.main)
                    NSApp.activate(ignoringOtherApps: true)
                }
            }

            Divider()
            Button("退出 FanCtl") { NSApp.terminate(nil) }
        }
    }
}
