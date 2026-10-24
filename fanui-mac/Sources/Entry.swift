//! Entry point.
//!
//! `--render <dir>` draws every page to PNG offscreen (visual review without a
//! display); anything else launches the normal menu-bar + window app.

import AppKit

@main
struct Entry {
    static func main() {
        if let flag = CommandLine.arguments.firstIndex(of: "--render") {
            let directory = CommandLine.arguments.count > flag + 1
                ? CommandLine.arguments[flag + 1]
                : FileManager.default.currentDirectoryPath + "/renders"
            RenderMode.run(outputDirectory: directory)
        }
        WindowShot.scheduleIfRequested()
        FanCtlUI.main()
    }
}
