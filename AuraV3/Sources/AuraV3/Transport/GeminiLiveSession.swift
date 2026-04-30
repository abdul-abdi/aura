import Foundation

public actor GeminiLiveSession: LiveSession {
    private let config: LiveSessionConfig
    private var task: URLSessionWebSocketTask?
    private let session: URLSession
    private var receiveLoop: Task<Void, Never>?
    private var continuation: AsyncStream<LiveEvent>.Continuation?

    public init(config: LiveSessionConfig) {
        self.config = config
        let scfg = URLSessionConfiguration.ephemeral
        scfg.waitsForConnectivity = false
        scfg.timeoutIntervalForRequest = 10
        self.session = URLSession(configuration: scfg)
    }

    public func connect() async throws {
        let url = appendKey(to: config.endpoint, key: config.apiKey)
        var req = URLRequest(url: url)
        req.timeoutInterval = 10
        let task = session.webSocketTask(with: req)
        self.task = task
        task.resume()

        let setup = GeminiProtocol.setupMessage(config: config)
        let data = try JSONSerialization.data(withJSONObject: setup)
        try await task.send(.data(data))

        startReceiveLoop()
    }

    public func send(_ frame: LiveFrame) async throws {
        guard let task else { throw URLError(.notConnectedToInternet) }
        let json: [String: Any]
        switch frame {
        case .audio(let raw):
            json = GeminiProtocol.realtimeInputAudio(raw.base64EncodedString())
        case .video(let jpeg):
            json = GeminiProtocol.realtimeInputVideoJPEG(jpeg.base64EncodedString())
        case .clientText(let text):
            json = GeminiProtocol.clientContentText(text)
        case .toolResult(let id, let output):
            json = GeminiProtocol.toolResponse(callID: id, output: output)
        }
        let data = try JSONSerialization.data(withJSONObject: json)
        try await task.send(.data(data))
    }

    public nonisolated func receive() -> AsyncStream<LiveEvent> {
        AsyncStream { cont in
            Task { await self.installContinuation(cont) }
        }
    }

    private func installContinuation(_ cont: AsyncStream<LiveEvent>.Continuation) {
        self.continuation = cont
    }

    public func close() async {
        receiveLoop?.cancel()
        // Best-effort: notify server we're done. Failures are fine here.
        try? await task?.send(.string("{\"clientContent\":{\"turnComplete\":true}}"))
        task?.cancel(with: .normalClosure, reason: nil)
        continuation?.yield(.closed(reason: "client_close"))
        continuation?.finish()
        task = nil
    }

    private func startReceiveLoop() {
        receiveLoop = Task { [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                guard let task = await self.task else { break }
                do {
                    let msg = try await task.receive()
                    await self.handleIncoming(msg)
                } catch {
                    await self.yieldEvent(.closed(reason: error.localizedDescription))
                    return
                }
            }
        }
    }

    private func handleIncoming(_ msg: URLSessionWebSocketTask.Message) async {
        let data: Data
        switch msg {
        case .data(let d): data = d
        case .string(let s): data = Data(s.utf8)
        @unknown default: return
        }
        guard let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }

        if obj["setupComplete"] != nil {
            yieldEvent(.setupComplete)
            return
        }
        if let resumption = (obj["sessionResumptionUpdate"] as? [String: Any]),
           let handle = resumption["newHandle"] as? String {
            yieldEvent(.resumptionHandle(handle))
        }
        if let server = obj["serverContent"] as? [String: Any] {
            handleServerContent(server)
        }
        if let toolCall = obj["toolCall"] as? [String: Any],
           let calls = toolCall["functionCalls"] as? [[String: Any]] {
            for call in calls {
                guard let id = call["id"] as? String,
                      let name = call["name"] as? String else { continue }
                let argsAny = call["args"] ?? NSNull()
                let argsData = (try? JSONSerialization.data(withJSONObject: argsAny)) ?? Data("null".utf8)
                let args = (try? JSONDecoder().decode(JSONValue.self, from: argsData)) ?? .null
                yieldEvent(.toolCall(callID: id, name: name, args: args))
            }
        }
        if (obj["interrupted"] as? Bool) == true {
            yieldEvent(.interrupted)
        }
    }

    private func handleServerContent(_ server: [String: Any]) {
        if let turn = server["modelTurn"] as? [String: Any],
           let parts = turn["parts"] as? [[String: Any]] {
            for part in parts {
                if let inline = part["inlineData"] as? [String: Any],
                   let b64 = inline["data"] as? String,
                   let raw = Data(base64Encoded: b64) {
                    yieldEvent(.audioOut(raw))
                }
            }
        }
        if let outputTr = server["outputTranscription"] as? [String: Any],
           let text = outputTr["text"] as? String {
            yieldEvent(.assistantTranscript(text))
        }
        if let inputTr = server["inputTranscription"] as? [String: Any],
           let text = inputTr["text"] as? String {
            yieldEvent(.userTranscript(text))
        }
    }

    private func yieldEvent(_ ev: LiveEvent) {
        continuation?.yield(ev)
    }

    private func appendKey(to url: URL, key: String) -> URL {
        var comps = URLComponents(url: url, resolvingAgainstBaseURL: false)!
        var q = comps.queryItems ?? []
        q.append(URLQueryItem(name: "key", value: key))
        comps.queryItems = q
        return comps.url!
    }
}
