import AppKit
import Combine
import os

let log = Logger(subsystem: "com.aura.desktop", category: "aura")

/// Wires Gemini Live to the microphone, speakers, screen and tools, and
/// publishes what's happening for the menu bar panel.
///
/// Thread confinement instead of an actor: published state is only touched on
/// the main thread, and session state (`speaking`, `tools`) only on Live's queue.
final class Aura: ObservableObject, @unchecked Sendable {
    enum Dot { case off, connecting, listening, speaking, muted, problem }

    struct Line: Identifiable {
        enum Kind { case user, aura, action }
        let id = UUID()
        let kind: Kind
        var text: String
        var failed = false
    }

    @Published private(set) var dot = Dot.off
    @Published private(set) var status = "Set up Aura"
    @Published private(set) var lines: [Line] = []
    @Published private(set) var muted = false
    @Published private(set) var running = false

    private let audio = Audio()
    private var live: Live?
    private var screenTimer: Timer?
    private var greeted = false
    private var openLine: Line.Kind?  // main thread: the line streaming text is appended to
    private var capturing = false  // main thread
    private var lastFrame = Data()
    private var speaking = false  // Live queue
    private var tools: Task<Void, Never>?  // Live queue: tools run one at a time

    init() {
        NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didWakeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            guard let self, self.running else { return }
            self.audio.restart()
            self.live?.reconnect()
        }
        HotKey.register { [weak self] in self?.toggleMute() }
        if Config.apiKey != nil, Permissions.microphone {
            start()
        } else {
            DispatchQueue.main.async {
                NSApp.activate()
                let alert = NSAlert()
                alert.messageText = "Aura lives in your menu bar"
                alert.informativeText = "Click the dot at the top right of your screen to finish setting up Aura."
                alert.runModal()
            }
        }
    }

    func start() {
        guard !running, let key = Config.apiKey else { return }
        let voice = Config.voice
        // Newest model first; the 2.5 native-audio model also works on free-tier keys.
        let models = [Config.model, "gemini-2.5-flash-native-audio-preview-12-2025"]
            .reduce(into: [String]()) { if !$0.contains($1) { $0.append($1) } }
        let live = Live(apiKey: key, models: models) { model, handle in
            Live.setupMessage(
                model: model, voice: voice, instructions: Prompt.instructions(facts: Memory.load()),
                tools: Tools.declarations, resumeHandle: handle)
        }
        live.onState = { [weak self] state in DispatchQueue.main.async { self?.update(state) } }
        live.onMessage = { [weak self] message in self?.handle(message) }
        self.live = live
        audio.onMicrophone = { pcm in live.sendAudio(pcm) }
        do {
            try audio.start()
        } catch {
            self.live = nil
            show(.problem, error.localizedDescription)
            return
        }
        running = true
        live.start()
        screenTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.captureScreen() }
    }

    func toggleMute() {
        guard running else { return }
        muted.toggle()
        audio.muted = muted
        if muted { live?.sendAudioEnd() }
        show(muted ? .muted : .listening, muted ? "Muted — ⌘⇧A to unmute" : "Listening")
    }

    /// A typed message from the panel.
    func send(_ text: String) {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let live else { return }
        lines.append(Line(kind: .user, text: text))
        openLine = nil
        live.sendText(text)
    }

    // MARK: Gemini events

    private func update(_ state: Live.State) {
        switch state {
        case .connecting:
            show(.connecting, "Connecting…")
        case .ready:
            show(muted ? .muted : .listening, muted ? "Muted — ⌘⇧A to unmute" : "Listening")
            captureScreen(force: true)
            if !greeted {
                greeted = true
                live?.sendText("[Aura just started. Greet the user in a few words.]")
            }
        case .failed(let reason):
            log.error("Gemini connection: \(reason, privacy: .public)")
            show(.problem, "\(reason.prefix(160)) — retrying…")
        }
    }

    /// Runs on the Live queue; audio is played straight from here.
    private func handle(_ message: [String: Any]) {
        if let content = message["serverContent"] as? [String: Any] {
            if content["interrupted"] as? Bool == true {
                audio.stopPlayback()
                setSpeaking(false)
            }
            let parts = (content["modelTurn"] as? [String: Any])?["parts"] as? [[String: Any]] ?? []
            for part in parts {
                guard let blob = part["inlineData"] as? [String: Any],
                      (blob["mimeType"] as? String)?.hasPrefix("audio/") == true,
                      let base64 = blob["data"] as? String, let pcm = Data(base64Encoded: base64)
                else { continue }
                audio.play(pcm)
                setSpeaking(true)
            }
            if let text = (content["inputTranscription"] as? [String: Any])?["text"] as? String {
                DispatchQueue.main.async { self.append(.user, text) }
            }
            if let text = (content["outputTranscription"] as? [String: Any])?["text"] as? String {
                DispatchQueue.main.async { self.append(.aura, text) }
            }
            if content["turnComplete"] as? Bool == true {
                setSpeaking(false)
                DispatchQueue.main.async { self.openLine = nil }
            }
        }
        let calls = (message["toolCall"] as? [String: Any])?["functionCalls"] as? [[String: Any]] ?? []
        for call in calls { runTool(call) }
    }

    private func runTool(_ call: [String: Any]) {
        let id = call["id"] as? String ?? ""
        let name = call["name"] as? String ?? ""
        let args = call["args"] as? [String: Any] ?? [:]
        let previous = tools
        tools = Task { [weak self] in
            await previous?.value
            guard let self, let live = self.live else { return }
            DispatchQueue.main.async {
                self.lines.append(Line(kind: .action, text: Tools.describe(name, args)))
                self.openLine = .action
            }
            if name == "quit_aura" {
                live.sendToolResponse(id: id, name: name, response: ["ok": true])
                try? await Task.sleep(nanoseconds: 3_000_000_000)  // let the goodbye play
                await MainActor.run { NSApp.terminate(nil) }
                return
            }
            let response = await Tools.run(name, args)
            if let error = response["error"] as? String {
                log.error("\(name, privacy: .public) failed: \(error, privacy: .public)")
                DispatchQueue.main.async { self.markLastActionFailed() }
            }
            if Tools.changesScreen.contains(name) {
                try? await Task.sleep(nanoseconds: 500_000_000)  // let the UI settle
                await self.sendFrame(force: true)
            }
            live.sendToolResponse(id: id, name: name, response: response)
        }
    }

    private func setSpeaking(_ value: Bool) {
        guard value != speaking else { return }
        speaking = value
        DispatchQueue.main.async {
            guard self.dot == .listening || self.dot == .speaking else { return }
            self.show(value ? .speaking : .listening, value ? "Speaking" : "Listening")
        }
    }

    // MARK: Screen

    private func captureScreen(force: Bool = false) {
        guard running, !capturing, live?.isReady == true, Permissions.screen else { return }
        capturing = true
        Task {
            await sendFrame(force: force)
            DispatchQueue.main.async { self.capturing = false }
        }
    }

    private func sendFrame(force: Bool) async {
        guard let jpeg = try? await Screen.capture() else { return }
        await MainActor.run {
            guard let live = self.live, live.isReady, force || jpeg != self.lastFrame else { return }
            self.lastFrame = jpeg
            live.sendImage(jpeg)
        }
    }

    // MARK: UI state (main thread)

    private func show(_ dot: Dot, _ status: String) {
        self.dot = dot
        self.status = status
    }

    private func append(_ kind: Line.Kind, _ text: String) {
        if openLine == kind, let last = lines.indices.last, lines[last].kind == kind {
            lines[last].text += text
        } else {
            let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !text.isEmpty else { return }
            lines.append(Line(kind: kind, text: text))
            openLine = kind
        }
        if lines.count > 200 { lines.removeFirst(lines.count - 200) }
    }

    private func markLastActionFailed() {
        if let i = lines.lastIndex(where: { $0.kind == .action }) { lines[i].failed = true }
    }
}
