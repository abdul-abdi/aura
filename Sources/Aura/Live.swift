import Foundation

/// Gemini Live API client over a WebSocket.
///
/// Never gives up: on any drop it reconnects with backoff and resumes the
/// conversation from the server's session-resumption handle. Callbacks run on
/// a private serial queue.
final class Live: NSObject, URLSessionWebSocketDelegate {
    enum State { case connecting, ready, failed(String) }

    var onMessage: (([String: Any]) -> Void)?
    var onState: ((State) -> Void)?

    private let apiKey: String
    private let models: [String]
    private let makeSetup: (_ model: String, _ resumeHandle: String?) -> [String: Any]
    private let queue: OperationQueue = {
        let q = OperationQueue()
        q.maxConcurrentOperationCount = 1
        return q
    }()
    private lazy var urlSession = URLSession(configuration: .default, delegate: self, delegateQueue: queue)

    // Touched from the audio thread too, so guarded by a lock.
    private let lock = NSLock()
    private var _task: URLSessionWebSocketTask?
    private var _ready = false

    // Only touched on `queue`.
    private var resumeHandle: String?
    private var modelIndex = 0
    private var readyAt = Date.distantPast
    private var failures = 0
    private var closeReason: String?
    private var connectToken = 0

    /// `models` are tried in order: if the server rejects one during setup (e.g. it
    /// isn't available on a free-tier key), the next one is used.
    init(apiKey: String, models: [String], setup: @escaping (_ model: String, _ resumeHandle: String?) -> [String: Any]) {
        self.apiKey = apiKey
        self.models = models
        self.makeSetup = setup
    }

    var isReady: Bool { locked { _ready } }
    private var task: URLSessionWebSocketTask? {
        get { locked { _task } }
        set { locked { _task = newValue } }
    }

    func start() { queue.addOperation { self.connect() } }

    /// Drop the current connection (if any) and reconnect right away.
    func reconnect() {
        queue.addOperation {
            self.failures = 0
            if let task = self.task { task.cancel(with: .goingAway, reason: nil) } else { self.scheduleConnect(after: 0) }
        }
    }

    // MARK: Sending

    func sendAudio(_ pcm16k: Data) {
        sendRealtime(["audio": ["mimeType": "audio/pcm;rate=16000", "data": pcm16k.base64EncodedString()]])
    }
    func sendAudioEnd() { sendRealtime(["audioStreamEnd": true]) }
    func sendImage(_ jpeg: Data) {
        sendRealtime(["video": ["mimeType": "image/jpeg", "data": jpeg.base64EncodedString()]])
    }
    /// A user turn as text (typed messages, and context such as the greeting cue).
    func sendText(_ text: String) {
        guard isReady else { return }
        send(["clientContent": ["turns": [["role": "user", "parts": [["text": text]]]], "turnComplete": true]])
    }

    func sendToolResponse(id: String, name: String, response: [String: Any]) {
        send(["toolResponse": ["functionResponses": [["id": id, "name": name, "response": response]]]])
    }

    private func sendRealtime(_ input: [String: Any]) {
        guard isReady else { return }
        send(["realtimeInput": input])
    }

    private func send(_ message: [String: Any], on task: URLSessionWebSocketTask? = nil) {
        guard let task = task ?? self.task,
              let data = try? JSONSerialization.data(withJSONObject: message),
              let text = String(data: data, encoding: .utf8)
        else { return }
        task.send(.string(text)) { _ in }
    }

    // MARK: Connection (runs on `queue`)

    private func connect() {
        guard task == nil else { return }
        locked { _ready = false }
        onState?(.connecting)
        let base = "wss://generativelanguage.googleapis.com/ws/"
            + "google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent"
        guard let url = URL(string: "\(base)?key=\(apiKey)") else {
            onState?(.failed("The API key contains invalid characters"))
            return
        }
        let task = urlSession.webSocketTask(with: url)
        task.maximumMessageSize = 32 << 20
        self.task = task
        task.resume()
        send(makeSetup(models[modelIndex], resumeHandle), on: task)
        receive(task)
    }

    private func receive(_ task: URLSessionWebSocketTask) {
        task.receive { [weak self] result in
            guard let self, task === self.task else { return }
            switch result {
            case .success(let message):
                var data: Data?
                switch message {
                case .string(let s): data = s.data(using: .utf8)
                case .data(let d): data = d
                @unknown default: break
                }
                if let data, let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
                    self.handle(json, task)
                }
                self.receive(task)
            case .failure(let error):
                self.dropped(task, error.localizedDescription)
            }
        }
    }

    private func handle(_ json: [String: Any], _ task: URLSessionWebSocketTask) {
        if json["setupComplete"] != nil {
            log.info("Connected to Gemini with \(self.models[self.modelIndex], privacy: .public)")
            readyAt = Date()
            locked { _ready = true }
            onState?(.ready)
        }
        if let update = json["sessionResumptionUpdate"] as? [String: Any],
           update["resumable"] as? Bool != false,
           let handle = update["newHandle"] as? String, !handle.isEmpty {
            resumeHandle = handle
        }
        if json["goAway"] != nil {
            // Server is about to close this connection; move to a new one now.
            task.cancel(with: .normalClosure, reason: nil)
        }
        onMessage?(json)
    }

    private func dropped(_ task: URLSessionWebSocketTask, _ error: String) {
        guard task === self.task else { return }
        // A connection that lived a while dropped normally (e.g. the server's
        // ~10-minute rotation): resume right away. Anything else backs off.
        let healthy = isReady && Date().timeIntervalSince(readyAt) > 10
        self.task = nil
        locked { _ready = false }
        if healthy {
            failures = 0
            onState?(.connecting)
        } else {
            failures += 1
            // A stale resumption handle can make every setup fail; start fresh.
            if failures >= 2 { resumeHandle = nil }
            onState?(.failed(closeReason ?? error))
            // The server rejected this model (not just a network error): try the next one.
            if let reason = closeReason, modelIndex + 1 < models.count {
                log.error("\(self.models[self.modelIndex], privacy: .public) rejected (\(reason, privacy: .public)); trying \(self.models[self.modelIndex + 1], privacy: .public)")
                modelIndex += 1
                resumeHandle = nil
                failures = 0
            }
        }
        closeReason = nil
        scheduleConnect(after: healthy ? 0.2 : min(30, 0.5 * pow(2, Double(failures))))
    }

    private func scheduleConnect(after delay: Double) {
        connectToken += 1
        let token = connectToken
        DispatchQueue.global().asyncAfter(deadline: .now() + delay) { [weak self] in
            self?.queue.addOperation {
                guard let self, token == self.connectToken else { return }
                self.connect()
            }
        }
    }

    func urlSession(
        _ session: URLSession, webSocketTask: URLSessionWebSocketTask,
        didCloseWith closeCode: URLSessionWebSocketTask.CloseCode, reason: Data?
    ) {
        if let reason, let text = String(data: reason, encoding: .utf8), !text.isEmpty { closeReason = text }
    }

    private func locked<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }

    // MARK: Setup message

    static func setupMessage(
        model: String, voice: String, instructions: String, tools: [[String: Any]], resumeHandle: String?
    ) -> [String: Any] {
        var resumption: [String: Any] = [:]
        if let resumeHandle { resumption["handle"] = resumeHandle }
        let empty: [String: Any] = [:]
        return [
            "setup": [
                "model": model.hasPrefix("models/") ? model : "models/\(model)",
                "generationConfig": [
                    "responseModalities": ["AUDIO"],
                    "speechConfig": ["voiceConfig": ["prebuiltVoiceConfig": ["voiceName": voice]]],
                ] as [String: Any],
                "systemInstruction": ["parts": [["text": instructions]]],
                "tools": [["functionDeclarations": tools], ["googleSearch": empty]],
                "sessionResumption": resumption,
                // Audio+video sessions are cut off after ~2 minutes without compression.
                "contextWindowCompression": ["slidingWindow": empty],
                "inputAudioTranscription": empty,
                "outputAudioTranscription": empty,
            ] as [String: Any]
        ]
    }
}
