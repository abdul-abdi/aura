import Darwin
import Foundation

/// Manages the Unix domain socket connection to the Rust daemon.
/// Reads JSONL events from the daemon and sends JSONL commands back.
@MainActor
final class DaemonConnection {
    private let socketPath: String
    private var fileDescriptor: Int32 = -1
    private var readTask: Task<Void, Never>?
    private var reconnectTask: Task<Void, Never>?
    private var buffer = Data()

    var onEvent: ((DaemonEvent) -> Void)?
    var onDisconnect: (() -> Void)?
    var onReconnect: (() -> Void)?

    private(set) var isConnected = false

    init(socketPath: String? = nil) {
        self.socketPath = socketPath ?? Self.defaultSocketPath()
    }

    static func defaultSocketPath() -> String {
        let appSupport = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first!
        return appSupport
            .appendingPathComponent("aura")
            .appendingPathComponent("daemon.sock")
            .path
    }

    /// Attempt to connect to the daemon socket, retrying for up to 10 seconds.
    func connect() async {
        let maxAttempts = 20
        let retryInterval: UInt64 = 500_000_000 // 500ms in nanoseconds

        for attempt in 1...maxAttempts {
            if tryConnect() {
                isConnected = true
                startReading()
                return
            }

            if attempt < maxAttempts {
                try? await Task.sleep(nanoseconds: retryInterval)
            }
        }

        onDisconnect?()
    }

    /// Reconnect with exponential backoff (500ms → 15s, up to 20 attempts).
    func reconnect() {
        disconnect()
        reconnectTask = Task { [weak self] in
            guard let self else { return }
            var delay: UInt64 = 500_000_000  // 500ms
            let maxDelay: UInt64 = 15_000_000_000  // 15s
            let maxAttempts = 20

            for _ in 1...maxAttempts {
                if Task.isCancelled { return }
                try? await Task.sleep(nanoseconds: delay)
                if Task.isCancelled { return }

                if tryConnect() {
                    await MainActor.run {
                        self.isConnected = true
                        self.startReading()
                        self.onReconnect?()
                    }
                    return
                }

                delay = min(delay * 2, maxDelay)
            }

            // All attempts exhausted
            await MainActor.run {
                self.onDisconnect?()
            }
        }
    }

    private func tryConnect() -> Bool {
        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return false }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)

        // Copy socket path into sun_path tuple
        let pathCString = socketPath.utf8CString
        let maxLen = MemoryLayout.size(ofValue: addr.sun_path)
        guard pathCString.count <= maxLen else {
            Darwin.close(fd)
            return false
        }

        withUnsafeMutablePointer(to: &addr.sun_path) { sunPathPtr in
            sunPathPtr.withMemoryRebound(to: CChar.self, capacity: maxLen) { dest in
                for i in 0..<pathCString.count {
                    dest[i] = pathCString[i]
                }
            }
        }

        let addrLen = socklen_t(MemoryLayout<sockaddr_un>.size)

        let connectResult = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPtr in
                Darwin.connect(fd, sockaddrPtr, addrLen)
            }
        }

        guard connectResult == 0 else {
            Darwin.close(fd)
            return false
        }

        // Set non-blocking for async reads
        let flags = Darwin.fcntl(fd, F_GETFL)
        _ = Darwin.fcntl(fd, F_SETFL, flags | O_NONBLOCK)

        self.fileDescriptor = fd
        return true
    }

    private func startReading() {
        let fd = self.fileDescriptor

        // Use Task.detached to avoid MainActor isolation for the read loop.
        // Data is dispatched back to MainActor via await self?.appendData().
        readTask = Task.detached { [weak self] in
            let bufferSize = 4096
            var readBuffer = [UInt8](repeating: 0, count: bufferSize)

            while !Task.isCancelled {
                let bytesRead = Darwin.read(fd, &readBuffer, bufferSize)

                if bytesRead > 0 {
                    let data = Data(readBuffer[..<bytesRead])
                    await self?.appendData(data)
                } else if bytesRead == 0 {
                    // EOF
                    await self?.handleDisconnect()
                    break
                } else {
                    // EAGAIN/EWOULDBLOCK means no data available yet (non-blocking)
                    if errno == EAGAIN || errno == EWOULDBLOCK {
                        try? await Task.sleep(nanoseconds: 10_000_000) // 10ms
                        continue
                    }
                    // Actual error
                    await self?.handleDisconnect()
                    break
                }
            }
        }
    }

    private func appendData(_ data: Data) {
        buffer.append(data)
        processBuffer()
    }

    private func processBuffer() {
        let newline = UInt8(ascii: "\n")
        while let newlineIndex = buffer.firstIndex(of: newline) {
            let lineData = buffer[buffer.startIndex..<newlineIndex]
            buffer = Data(buffer[(newlineIndex + 1)...])

            guard !lineData.isEmpty else { continue }

            let decoder = JSONDecoder()
            do {
                let event = try decoder.decode(DaemonEvent.self, from: Data(lineData))
                onEvent?(event)
            } catch {
                // Skip malformed lines silently
            }
        }
    }

    /// Send a command to the daemon as a JSONL line.
    func send(_ command: UICommand) {
        guard fileDescriptor >= 0, isConnected else { return }

        let encoder = JSONEncoder()
        encoder.outputFormatting = []
        guard var data = try? encoder.encode(command) else { return }
        data.append(UInt8(ascii: "\n"))

        let fd = fileDescriptor
        let result = data.withUnsafeBytes { ptr -> Int in
            guard let baseAddress = ptr.baseAddress else { return -1 }
            return Darwin.write(fd, baseAddress, data.count)
        }

        if result < 0 {
            // Write failed — connection is broken
            handleDisconnect()
        }
    }

    private func handleDisconnect() {
        let wasConnected = isConnected
        disconnect()
        onDisconnect?()

        // Auto-reconnect if we were previously connected
        if wasConnected {
            reconnect()
        }
    }

    func disconnect() {
        reconnectTask?.cancel()
        reconnectTask = nil
        readTask?.cancel()
        readTask = nil
        if fileDescriptor >= 0 {
            Darwin.close(fileDescriptor)
            fileDescriptor = -1
        }
        isConnected = false
        buffer = Data()
    }
}
