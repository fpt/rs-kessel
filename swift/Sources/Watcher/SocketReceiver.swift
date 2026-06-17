import Foundation

/// An event received from Claude Code hooks via Unix domain socket
public struct HookEvent: @unchecked Sendable {
    public let event: String             // "PostToolUse", "Stop", etc.
    public let toolName: String?
    public let toolInput: [String: Any]?
    public let filePath: String?
    public let cwd: String?              // working directory (session identifier)
    public let raw: [String: Any]        // full parsed JSON

    init(json: [String: Any]) {
        self.event = json["event"] as? String ?? "unknown"
        self.toolName = json["tool"] as? String
            ?? (json["tool_input"] as? [String: Any])?["name"] as? String
        self.toolInput = json["tool_input"] as? [String: Any]
        self.filePath = json["file"] as? String
            ?? (json["tool_input"] as? [String: Any])?["file_path"] as? String
        self.cwd = json["cwd"] as? String
        self.raw = json
    }

    /// Convert to JSON string for the Rust EventRouter.
    public func toRouterJSON() -> String? {
        var dict: [String: Any] = ["source": "hook", "event": event]
        if let tool = toolName { dict["tool_name"] = tool }
        if let path = filePath { dict["file_path"] = path }
        if let cwd = cwd { dict["session_id"] = cwd }
        guard let data = try? JSONSerialization.data(withJSONObject: dict) else { return nil }
        return String(data: data, encoding: .utf8)
    }
}

/// Listens on a Unix domain socket for ndjson messages from Claude Code hooks.
public class SocketReceiver {
    private let socketPath: String
    private var listenFD: Int32 = -1
    private var listenSource: DispatchSourceRead?
    private var clientSources: [Int32: DispatchSourceRead] = [:]
    private var clientBuffers: [Int32: Data] = [:]
    private var continuation: AsyncStream<HookEvent>.Continuation?
    private let queue = DispatchQueue(label: "kessel-cli.socket-receiver", qos: .utility)

    public init(socketPath: String? = nil) {
        self.socketPath = socketPath ?? "/tmp/kessel-cli-\(getuid()).sock"
    }

    /// The path the socket is listening on
    public var path: String { socketPath }

    /// Returns an AsyncStream of hook events.
    public func events() -> AsyncStream<HookEvent> {
        AsyncStream { continuation in
            self.continuation = continuation
            continuation.onTermination = { @Sendable _ in
                self.stop()
            }
        }
    }

    /// Start listening on the Unix domain socket.
    public func start() throws {
        // Remove stale socket
        unlink(socketPath)

        // Create socket
        listenFD = socket(AF_UNIX, SOCK_STREAM, 0)
        guard listenFD >= 0 else {
            throw SocketError.createFailed(errno: errno)
        }

        // Bind
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let pathBytes = socketPath.utf8CString
        guard pathBytes.count <= MemoryLayout.size(ofValue: addr.sun_path) else {
            close(listenFD)
            throw SocketError.pathTooLong
        }
        withUnsafeMutablePointer(to: &addr.sun_path) { ptr in
            ptr.withMemoryRebound(to: CChar.self, capacity: pathBytes.count) { dest in
                pathBytes.withUnsafeBufferPointer { src in
                    _ = memcpy(dest, src.baseAddress!, pathBytes.count)
                }
            }
        }

        let bindResult = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
                bind(listenFD, sockPtr, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bindResult == 0 else {
            close(listenFD)
            throw SocketError.bindFailed(errno: errno)
        }

        // Listen
        guard listen(listenFD, 5) == 0 else {
            close(listenFD)
            unlink(socketPath)
            throw SocketError.listenFailed(errno: errno)
        }

        // Accept connections via GCD
        let source = DispatchSource.makeReadSource(fileDescriptor: listenFD, queue: queue)
        source.setEventHandler { [weak self] in
            self?.acceptConnection()
        }
        source.setCancelHandler { [weak self] in
            guard let self = self else { return }
            close(self.listenFD)
            self.listenFD = -1
        }
        listenSource = source
        source.resume()
    }

    private func acceptConnection() {
        var clientAddr = sockaddr_un()
        var addrLen = socklen_t(MemoryLayout<sockaddr_un>.size)

        let clientFD = withUnsafeMutablePointer(to: &clientAddr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
                accept(listenFD, sockPtr, &addrLen)
            }
        }
        guard clientFD >= 0 else { return }

        clientBuffers[clientFD] = Data()

        let readSource = DispatchSource.makeReadSource(fileDescriptor: clientFD, queue: queue)
        readSource.setEventHandler { [weak self] in
            self?.readFromClient(fd: clientFD)
        }
        readSource.setCancelHandler { [weak self] in
            close(clientFD)
            self?.clientBuffers.removeValue(forKey: clientFD)
            self?.clientSources.removeValue(forKey: clientFD)
        }
        clientSources[clientFD] = readSource
        readSource.resume()
    }

    private func readFromClient(fd: Int32) {
        var buf = [UInt8](repeating: 0, count: 4096)
        let n = read(fd, &buf, buf.count)

        if n <= 0 {
            // EOF or error — process any remaining buffer, then close
            if let buffer = clientBuffers[fd], !buffer.isEmpty {
                processBuffer(buffer)
            }
            clientSources[fd]?.cancel()
            return
        }

        clientBuffers[fd]?.append(contentsOf: buf[0..<n])

        // Process complete lines
        guard var buffer = clientBuffers[fd] else { return }
        while let newlineIdx = buffer.firstIndex(of: UInt8(ascii: "\n")) {
            let lineData = buffer[buffer.startIndex..<newlineIdx]
            buffer = buffer[(newlineIdx + 1)...]
            processBuffer(Data(lineData))
        }
        clientBuffers[fd] = buffer
    }

    private func processBuffer(_ data: Data) {
        guard !data.isEmpty,
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return
        }
        let event = HookEvent(json: json)
        continuation?.yield(event)
    }

    public func stop() {
        listenSource?.cancel()
        listenSource = nil
        for (_, source) in clientSources {
            source.cancel()
        }
        clientSources.removeAll()
        clientBuffers.removeAll()
        unlink(socketPath)
        continuation?.finish()
        continuation = nil
    }

    deinit {
        stop()
    }

    public enum SocketError: Error, CustomStringConvertible {
        case createFailed(errno: Int32)
        case bindFailed(errno: Int32)
        case listenFailed(errno: Int32)
        case pathTooLong

        public var description: String {
            switch self {
            case .createFailed(let e): return "socket() failed: \(String(cString: strerror(e)))"
            case .bindFailed(let e): return "bind() failed: \(String(cString: strerror(e)))"
            case .listenFailed(let e): return "listen() failed: \(String(cString: strerror(e)))"
            case .pathTooLong: return "Socket path exceeds maximum length"
            }
        }
    }
}
