//! Live-daemon smoke test for the Swift client.
//!
//! Build and run against an installed `fand`:
//!
//!     ./test.sh
//!
//! Reads are always safe; the writes it performs are the same ones the UI
//! issues (mode switch + config round trip) and it restores the original mode.

import Foundation

// Exercises the request path against the live daemon: reads, a mode switch,
// and a config round trip. Prints PASS/FAIL per step.

@main
struct ReqTest {
    static func main() {
        let client = Client(socketPath: Client.defaultSocket)
        
        func step(_ name: String, _ body: () throws -> Bool) {
            do {
                let ok = try body()
                print(ok ? "PASS  \(name)" : "FAIL  \(name)")
            } catch {
                print("FAIL  \(name): \(error)")
            }
        }
        
        step("snapshot") {
            let snap = try client.snapshot()
            print("      mode=\(snap.mode.rawValue) temp=\(snap.control_temp ?? -1) rpm=\(snap.fan?.actual_rpm ?? -1)")
            return snap.version == 1
        }
        
        step("history") {
            let h = try client.history(seconds: 600)
            print("      \(h.count) samples")
            return h.count > 0
        }
        
        var original = Config.default
        step("capture config") {
            original = try client.snapshot().config
            return true
        }
        
        step("set_mode curve") {
            let r = try client.call(SetModeRequest(mode: .curve))
            print("      \(r.message ?? "-")")
            return r.ok
        }
        
        step("set_config roundtrip") {
            var config = original
            config.sensor = original.sensor
            let r = try client.call(SetConfigRequest(config: config))
            print("      \(r.message ?? r.error ?? "-")")
            return r.ok
        }
        
        step("restore mode") {
            let r = try client.call(SetModeRequest(mode: original.mode))
            print("      restored \(original.mode.rawValue): \(r.message ?? "-")")
            return r.ok
        }
        
        step("set_fixed apply=false (no write)") {
            let r = try client.call(SetFixedRequest(rpm: original.fixed_rpm, apply: false, fan: nil))
            return r.ok
        }
    }
}
