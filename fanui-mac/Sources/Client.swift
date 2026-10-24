//! Unix-socket client for the `fand` root daemon.
//!
//! Same wire protocol as `fancore::client`: one JSON object per line in, one
//! per line out. Requests are modelled as small `Encodable` structs because the
//! Rust side is an internally-tagged enum (`"cmd": "set_mode"`, ...).

import Foundation

// MARK: - requests

struct SnapshotRequest: Encodable { let cmd = "snapshot" }
struct PingRequest: Encodable { let cmd = "ping" }
struct AutoRequest: Encodable { let cmd = "set_auto" }
struct ReloadRequest: Encodable { let cmd = "reload" }
struct FlushLogRequest: Encodable { let cmd = "flush_log" }

struct HistoryRequest: Encodable {
    let cmd = "history"
    let seconds: UInt64
}

struct SetModeRequest: Encodable {
    let cmd = "set_mode"
    let mode: ControlMode
}

struct SetConfigRequest: Encodable {
    let cmd = "set_config"
    let config: Config
}

struct SetFixedRequest: Encodable {
    let cmd = "set_fixed"
    let rpm: Double
    let apply: Bool
    let fan: Int?
}

// MARK: - errors

enum ClientError: LocalizedError {
    case notRunning(String)
    case io(String)
    case protocolError(String)

    var errorDescription: String? {
        switch self {
        case .notRunning(let path):
            return "后台服务未运行（找不到 \(path)）。先执行 `sudo fanctl daemon install` 或 `sudo fanctl daemon run`"
        case .io(let message):
            return "与后台服务通信失败：\(message)"
        case .protocolError(let message):
            return "后台服务返回了无法解析的数据：\(message)"
        }
    }
}

// MARK: - client

struct Client {
    let socketPath: String
    var timeout: TimeInterval = 3

    static var defaultSocket: String {
        if let override = ProcessInfo.processInfo.environment["FANCTL_SOCKET"], !override.isEmpty {
            return override
        }
        return "/var/run/fanctl.sock"
    }

    func call<T: Encodable>(_ request: T) throws -> Response {
        let payload = try JSONEncoder().encode(request)
        let reply = try transmit(payload)
        do {
            return try JSONDecoder().decode(Response.self, from: reply)
        } catch {
            throw ClientError.protocolError(String(describing: error))
        }
    }

    func snapshot() throws -> Snapshot {
        let response = try call(SnapshotRequest())
        guard let snapshot = response.snapshot else {
            throw ClientError.protocolError(response.error ?? "响应里没有快照")
        }
        return snapshot
    }

    func history(seconds: UInt64) throws -> [Sample] {
        try call(HistoryRequest(seconds: seconds)).history ?? []
    }

    // MARK: transport

    private func transmit(_ payload: Data) throws -> Data {
        guard FileManager.default.fileExists(atPath: socketPath) else {
            throw ClientError.notRunning(socketPath)
        }

        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw ClientError.io("socket() 失败") }
        defer { close(fd) }

        var timeoutValue = timeval(
            tv_sec: Int(timeout),
            tv_usec: suseconds_t((timeout - floor(timeout)) * 1_000_000)
        )
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeoutValue, socklen_t(MemoryLayout<timeval>.size))
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeoutValue, socklen_t(MemoryLayout<timeval>.size))

        guard connectUnix(fd: fd, path: socketPath) else {
            throw ClientError.io("连接 \(socketPath) 失败（errno \(errno)）")
        }

        var out = payload
        out.append(0x0A) // newline terminates the request
        guard writeAll(fd: fd, bytes: out) else {
            throw ClientError.io("写入请求失败")
        }

        guard let line = readLine(fd: fd) else {
            throw ClientError.protocolError("空响应")
        }
        return line
    }

    private func connectUnix(fd: Int32, path: String) -> Bool {
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        addr.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        let maxPath = MemoryLayout.size(ofValue: addr.sun_path)
        guard path.utf8.count < maxPath else { return false }
        withUnsafeMutablePointer(to: &addr.sun_path) { pointer in
            pointer.withMemoryRebound(to: CChar.self, capacity: maxPath) { destination in
                _ = path.withCString { strcpy(destination, $0) }
            }
        }
        return withUnsafePointer(to: &addr) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPointer in
                connect(fd, sockaddrPointer, socklen_t(MemoryLayout<sockaddr_un>.size)) == 0
            }
        }
    }

    private func writeAll(fd: Int32, bytes: Data) -> Bool {
        var remaining = bytes[...]
        while !remaining.isEmpty {
            let written = remaining.withUnsafeBytes { buffer -> Int in
                guard let base = buffer.baseAddress else { return -1 }
                return write(fd, base, buffer.count)
            }
            if written <= 0 {
                if errno == EINTR { continue }
                return false
            }
            remaining = remaining.dropFirst(written)
        }
        return true
    }

    private func readLine(fd: Int32) -> Data? {
        var buffer = Data()
        var chunk = [UInt8](repeating: 0, count: 16 * 1024)
        while true {
            let count = read(fd, &chunk, chunk.count)
            if count > 0 {
                buffer.append(contentsOf: chunk[0..<count])
                if buffer.last == 0x0A { break }
                continue
            }
            if count < 0, errno == EINTR { continue }
            break
        }
        guard !buffer.isEmpty else { return nil }
        if buffer.last == 0x0A { buffer.removeLast() }
        return buffer
    }
}
