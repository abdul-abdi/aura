import Foundation
import AppKit
import AVFoundation

/// The app's runtime root: holds all subsystem instances, wires their callbacks,
/// supervises the watchdogs, and orchestrates the voice round-trip.
///
/// Amendment N: handleSleep does a hard tear-down (capture + playback + session).
///   Radios are dying on willSleep; rotating is wrong there. handleWake reconnects
///   from scratch — there's no session to rotate from.
/// Amendment O: AudioConfigWatchdog is actually instantiated and wired to
///   capture.engine. On device change, capture is stopped and restarted.
/// Amendment P: WatchdogSupervisor wraps NetworkPathWatchdog. NWPathMonitor.start
///   is synchronous so the supervisor sees a normal return; infrastructure is in
///   place for future watchdogs with actual run loops.
@MainActor
public final class AppCoordinator {
    private let state = AppState.shared
    private var session: GeminiLiveSession?
    private var capture: AudioCapture?
    private let playback = AudioPlayback()
    private var screenStream: ScreenStream?
    private var bargeIn = BargeInDetector()

    private var sleepWake: SleepWakeWatchdog!
    private var network: NetworkPathWatchdog!
    private var audioConfig: AudioConfigWatchdog?
    private var permission: PermissionWatchdog!
    private var supervisor: WatchdogSupervisor!

    private var rotation: RotationOrchestrator!
    private var effects: EffectLog!
    private var tools: ToolRegistry!
    private var memoryTools: MemoryTools!

    private var sessionTimer: Task<Void, Never>?
    private var receiveLoopTask: Task<Void, Never>?
    private var screenPumpTask: Task<Void, Never>?
    private var rotationRateLimit = RateLimit(minIntervalSeconds: 5.0)
    private var bargeInCalibrationFrames = 0
    /// Prevents concurrent reconnect attempts — both `.closed` events and network
    /// path-up transitions can both call reconnect; without this guard, two loops
    /// race and may produce two live sessions.
    private var reconnectInProgress = false
    /// Spec §5.2 invariant 3: timestamp of the last `didWake`. Used to gate
    /// rotation-ring replay — never replay if we just woke from sleep.
    private var lastWakeAt: Date?
    /// Replay suppression window after wake. Beyond this, rotations resume
    /// normal replay behavior.
    private let wakeReplaySuppressSeconds: Double = 5.0

    public init() {
        self.effects = EffectLog(log: state.log)
        self.rotation = RotationOrchestrator(log: state.log, effects: effects)
        self.permission = PermissionWatchdog(log: state.log)
        self.supervisor = WatchdogSupervisor(log: state.log)
        self.tools = ToolRegistry(effects: effects)
        self.memoryTools = MemoryTools(log: state.log)
        // Tool registration moved to startSession() — registering from a
        // detached Task in init() raced the WS handshake; if Gemini connected
        // before the four register awaits landed on the actor, the first
        // toolCall would return unknown_tool. startSession runs registration
        // synchronously-on-actor before connect(), eliminating the race.

        self.sleepWake = SleepWakeWatchdog(
            onSleep: { [weak self] in self?.handleSleep() },
            onWake:  { [weak self] in self?.handleWake() }
        )
        self.network = NetworkPathWatchdog()
    }

    public func start() {
        sleepWake.start()

        // Amendment P: supervise the network watchdog. The closure captures `net` (a
        // non-optional actor ref, Sendable). NWPathMonitor.start is synchronous under
        // the hood — the async actor method returns quickly; supervisor sees normal
        // completion and will not restart. Infrastructure is in place for Plan 2/3
        // watchdogs that have actual run loops.
        let net = self.network!
        let sup = self.supervisor!
        // `self` is @MainActor-isolated; passing it into a @Sendable closure requires it
        // to be Sendable. AppCoordinator is final and @MainActor, so all access through
        // the Task { @MainActor in } hop is safe. We use `unowned(unsafe)` to avoid the
        // captured-var warning while keeping the dispatch back on @MainActor.
        //
        // Note: the NWPathMonitor callback fires on a private dispatch queue (not MainActor).
        // The Task { @MainActor in ... } hop is the correct isolation re-entry point.
        // We intentionally do NOT use [weak self] here because (a) the outer Task already
        // prevents the closure from outliving the app's lifetime, and (b) [weak self]
        // would require capturing a 'var' (weak var) which is a Swift 6 error across
        // concurrency domains. The supervisor Task is cancelled in deinit (not wired yet
        // in Plan 1) so this is acceptable.
        let coord: AppCoordinator = self
        Task {
            await sup.supervise(name: "network") {
                await net.start { @Sendable status in
                    Task { @MainActor in coord.handleNetworkChange(status) }
                }
            }
        }

        Task { @MainActor [weak self] in await self?.startSession() }
    }

    // MARK: - Session lifecycle

    private func startSession() async {
        guard let key = Config.apiKey() else {
            state.setDotState(.offline)
            Loggers.app.warning("No API key in Keychain; staying offline.")
            return
        }

        // Register the MVP tools BEFORE connect(). ToolRegistry actor
        // serializes these; awaiting all four guarantees the first toolCall
        // hitting the receive loop finds them registered. Re-registration
        // is idempotent (dictionary write) — startSession is called on
        // every connect/reconnect/wake path, so we just write through.
        let registry = self.tools!
        let mt = self.memoryTools!
        let ct = ClipboardTools()
        await registry.register("remember",       mt.rememberToolImpl())
        await registry.register("recall",         mt.recallToolImpl())
        await registry.register("forget",         mt.forgetToolImpl())
        await registry.register("clipboard_read", ct.readToolImpl())

        // Plan 3 MVP: assemble the system prompt via Context.swift — the one
        // place model-context comes together. Reads the user-profile lens
        // from the log; pure function from inputs.
        let profile = (try? await Lenses.userProfile(log: state.log)) ?? []
        let inputs = Context.Inputs(mode: "default", now: Date(), userProfile: profile)
        let assembled = Context.sessionStartContext(inputs: inputs)
        await state.log.append(kind: EventKind.contextAssembled, payload: .object([
            "kind":             .string("sessionStart"),
            "approx_tokens":    .number(Double(assembled.approxTokens)),
            "dropped_sections": .array(assembled.droppedSections.map { .string($0) }),
            "persona_hash":     .string(assembled.personaHash),
            "facts_count":      .number(Double(profile.count)),
        ]))

        var cfg = LiveSessionConfig(apiKey: key)
        cfg.toolDeclarations = ToolDeclarations.mvp
        cfg.systemPrompt = assembled.text
        let session = GeminiLiveSession(config: cfg)
        do {
            try await session.connect()
        } catch {
            state.setDotState(.offline)
            Loggers.app.error("connect failed: \(error.localizedDescription)")
            return
        }
        self.session = session
        state.setDotState(.listening)

        receiveLoopTask?.cancel()
        receiveLoopTask = Task { @MainActor [weak self] in
            await self?.runReceiveLoop(session)
        }

        // Stop any previous capture before reassigning. ARC alone won't
        // deterministically tear down the old AVAudioEngine — the tap closure
        // captures self weakly, so the engine could outlive the assignment
        // until the next ARC pass. Explicit stop() removes the tap and stops
        // the engine, freeing the audio HW immediately.
        capture?.stop()

        // Audio capture → send.
        let cap = AudioCapture(
            onChunk: { [weak self] pcm in
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    guard !self.state.isMicMuted else { return }
                    try? await self.session?.send(.audio(pcm))
                }
            },
            onSamples: { [weak self] samples in
                // Hop to MainActor — bargeIn + playback are @MainActor-bound.
                // ~50 Hz hop frequency is acceptable.
                Task { @MainActor [weak self] in
                    self?.handleBargeInSamples(samples)
                }
            }
        )
        do { try cap.start() } catch {
            Loggers.app.error("capture start failed: \(error.localizedDescription)")
            Task { [weak self] in await self?.permission.reportFailure(.microphone) }
        }
        self.capture = cap

        // Amendment O: AudioConfigWatchdog actually wired.
        // Stop any previous watchdog first — its observer is registered for the OLD
        // engine which we just stopped. Without this, NotificationCenter leaks an
        // observer per session restart.
        audioConfig?.stop()
        let configWatchdog = AudioConfigWatchdog(engine: cap.engine) { [weak self] in
            self?.handleAudioConfigChange()
        }
        configWatchdog.start()
        self.audioConfig = configWatchdog

        // Playback engine must be started before the receive loop enqueues audio.
        do { try playback.start() } catch {
            Loggers.app.warning("playback start failed: \(error.localizedDescription)")
        }

        // Screen stream — stop any prior instance before creating a new one.
        // startSession() is called on every connect/reconnect/wake path; without this
        // guard, each reconnect leaks an extra SCK stream.
        if let existing = self.screenStream {
            await existing.stop()
            self.screenStream = nil
        }
        let screen = ScreenStream()
        do { try await screen.start() } catch {
            Loggers.app.warning("screen stream start failed: \(error.localizedDescription)")
            Task { [weak self] in await self?.permission.reportFailure(.screen) }
        }
        self.screenStream = screen

        // Pump screen frames from the slot to Gemini at up to 10 Hz (frames arrive
        // at 2 Hz; oversampling guarantees we don't miss new frames).
        startScreenPump()

        // Schedule rotation at T = 9 minutes (before the 10-minute server limit).
        scheduleRotation()
    }

    /// Polls the ScreenStream's FrameSlot at 10 Hz and sends each new JPEG frame
    /// over the current session as `LiveFrame.video`. Idempotent — cancels any
    /// existing pump task before installing the new one.
    private func startScreenPump() {
        screenPumpTask?.cancel()
        screenPumpTask = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 100_000_000)  // 100ms
                guard let self else { return }
                guard let frame = self.screenStream?.slot.take() else { continue }
                try? await self.session?.send(.video(frame))
            }
        }
    }

    private func scheduleRotation() {
        sessionTimer?.cancel()
        sessionTimer = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 9 * 60 * 1_000_000_000)
            guard !Task.isCancelled else { return }
            await self?.rotateNow(reason: .scheduled)
        }
    }

    // MARK: - Receive loop

    private func runReceiveLoop(_ session: GeminiLiveSession) async {
        let stream = session.receive()
        for await event in stream {
            switch event {
            case .setupComplete:
                Loggers.transport.info("setupComplete")

            case .audioOut(let data):
                playback.enqueue(data)
                state.setDotState(.speaking)

            case .userTranscript(let text):
                let log = state.log
                await log.append(kind: EventKind.turnUser,
                                 payload: .object(["text": .string(text)]))

            case .assistantTranscript(let text):
                let log = state.log
                await log.append(kind: EventKind.turnAssistant,
                                 payload: .object(["text": .string(text)]))

            case .toolCall(let id, let name, let args):
                // Amendment AH: thread geminiCallID through to EffectLog.
                // Persist the request before dispatch so the log reflects what
                // Gemini asked even if dispatch fails or the session is torn
                // down mid-call.
                let argsForLog = args
                let log = state.log
                await log.append(
                    kind: EventKind.turnToolCall,
                    geminiCallID: id,
                    payload: .object([
                        "name": .string(name),
                        "args": argsForLog,
                    ])
                )
                let registry = self.tools!
                let result = await registry.dispatch(name: name, args: args, geminiCallID: id)
                let geminiResponse = result.toGeminiResponse()
                try? await session.send(.toolResult(callID: id, output: geminiResponse))
                await log.append(
                    kind: EventKind.turnToolResult,
                    geminiCallID: id,
                    payload: geminiResponse
                )
                Loggers.action.info("tool \(name, privacy: .public) → \(result.status.rawValue, privacy: .public)")

            case .interrupted:
                playback.flush()
                bargeIn.startReverbGuard()
                state.setDotState(.listening)

            case .resumptionHandle:
                break

            case .closed(let reason):
                // Surface why the session closed — this is the only signal we
                // get back when the WS handshake fails (auth, model rejection,
                // network) since `connect()` returns optimistically before the
                // server ack.
                Loggers.transport.error("session closed: \(reason ?? "no reason given", privacy: .public)")
                state.setDotState(.offline)
                Task { @MainActor [weak self] in await self?.reconnect() }
            }
        }
    }

    // MARK: - Rotation

    private func rotateNow(reason: RotationOrchestrator.Reason) async {
        guard let cur = session else { return }
        state.setDotState(.rotating)
        let key = Config.apiKey() ?? ""
        let log = state.log

        // Plan 3 MVP: build a real digest via Context.rotationDigest. Inputs
        // are derived from the log (user profile lens). Goal stack is empty
        // for now — Plan 3 follow-up adds goal events.
        let profile = (try? await Lenses.userProfile(log: log)) ?? []
        let digestInputs = Context.Inputs(mode: "default", now: Date(), userProfile: profile)
        let digestText = Context.rotationDigest(inputs: digestInputs)
        await log.append(kind: EventKind.contextAssembled, payload: .object([
            "kind":         .string("rotationDigest"),
            "approx_tokens": .number(Double(Context.approxTokens(digestText))),
            "facts_count":   .number(Double(profile.count)),
        ]))

        do {
            let new = try await rotation.rotate(
                reason: reason,
                currentSession: cur,
                newSessionFactory: {
                    var newCfg = LiveSessionConfig(apiKey: key)
                    newCfg.toolDeclarations = ToolDeclarations.mvp
                    newCfg.systemPrompt = Persona.systemPrompt(mode: "default")
                    let s = GeminiLiveSession(config: newCfg)
                    try await s.connect()
                    return s
                },
                pendingEffectCorrelations: {
                    (try? await EffectLog.findOrphans(log: log)) ?? []
                },
                digest: digestText,
                playback: self.playback,
                captureMicPaused: { [weak self] in self?.capture?.pauseSending() },
                captureMicResumed: { [weak self] in
                    // Spec §5.2 invariant 3: replay the rotation ring iff this
                    // resume isn't within the post-wake suppression window.
                    guard let self = self else { return }
                    let recentWake = self.lastWakeAt.map {
                        Date().timeIntervalSince($0) < self.wakeReplaySuppressSeconds
                    } ?? false
                    self.capture?.resumeSending(replay: !recentWake)
                }
            )
            self.session = new
            receiveLoopTask?.cancel()
            receiveLoopTask = Task { @MainActor [weak self] in
                await self?.runReceiveLoop(new)
            }
            // Restart the screen pump so it sends to the fresh session.
            startScreenPump()
            state.setDotState(.listening)
            scheduleRotation()
        } catch {
            Loggers.rotation.error("rotation failed: \(error.localizedDescription)")
            state.setDotState(.offline)
        }
    }

    // MARK: - Reconnect

    private func reconnect() async {
        // Concurrent-reconnect guard — both `.closed` and network path-up can both
        // call reconnect; without this, two loops race.
        guard !reconnectInProgress else {
            Loggers.app.info("reconnect already in progress; ignoring duplicate trigger")
            return
        }
        reconnectInProgress = true
        defer { reconnectInProgress = false }

        var bo = Backoff()
        for _ in 0..<5 {
            try? await Task.sleep(for: bo.next())
            await startSession()
            if state.dotState != .offline { return }
        }
        Loggers.app.error("reconnect exhausted 5 attempts; staying offline")
    }

    // MARK: - Sleep / Wake  (Amendment N)

    /// Amendment N: hard tear-down on willSleep. Radios are dying; rotating is wrong.
    private func handleSleep() {
        Loggers.app.info("sleep — hard tear-down")
        capture?.stop()
        playback.stop()
        // Stop the audio-config observer too — its target engine just stopped.
        // startSession recreates both the capture engine and a fresh watchdog.
        audioConfig?.stop()
        audioConfig = nil
        state.setDotState(.offline)
        sessionTimer?.cancel()
        receiveLoopTask?.cancel()
        screenPumpTask?.cancel()
        // Re-calibrate barge-in on wake — acoustics may have changed.
        bargeIn = BargeInDetector()
        bargeInCalibrationFrames = 0
        let s = session
        session = nil
        // Stop the screen stream (SCK capture should not run while the screen is off).
        // startSession will create a fresh one on wake.
        let screen = screenStream
        screenStream = nil
        Task {
            await screen?.stop()
            await s?.close()   // best-effort; OS will yank the socket anyway
        }
    }

    /// Amendment N: since handleSleep tore everything down, reconnect from scratch
    /// (there is no current session to rotate FROM). Stamps `lastWakeAt` so the
    /// next `captureMicResumed` (after the new session establishes and the first
    /// scheduled / triggered rotation completes) suppresses ring-replay per
    /// spec §5.2 invariant 3.
    private func handleWake() {
        lastWakeAt = Date()
        let log = state.log
        Task { @MainActor [weak self] in
            await log.append(kind: EventKind.stateWake, payload: .null)
            await self?.reconnect()
        }
    }

    // MARK: - Network change

    private func handleNetworkChange(_ status: NetworkPathWatchdog.Status) {
        Loggers.app.info("network: \(String(describing: status), privacy: .public)")
        // Only rotate when the network becomes available and we have a live session.
        // Amendment S: rate-limit to prevent WiFi-handoff flap from burning the
        // attempt budget (5+ triggers in 2s is common on network path changes).
        if status == .satisfied, session != nil {
            if rotationRateLimit.tryAcquire() {
                Task { @MainActor [weak self] in await self?.rotateNow(reason: .pathChange) }
            } else {
                Loggers.app.info("network rotation rate-limited (5s window)")
            }
        }
    }

    // MARK: - Barge-in calibration

    private func handleBargeInSamples(_ samples: [Float]) {
        if !bargeIn.isCalibrated {
            bargeIn.observeAmbient(frame: samples)
            bargeInCalibrationFrames += 1
            if bargeInCalibrationFrames >= 50 {
                bargeIn.commitCalibration()
                Loggers.voice.info("barge-in calibrated; threshold=\(self.bargeIn.threshold, privacy: .public)")
            }
            return
        }
        if state.dotState == .speaking, bargeIn.shouldBargeIn(frame: samples) {
            playback.flush()
            bargeIn.startReverbGuard()
            state.setDotState(.listening)
            Task { [weak self] in
                await self?.state.log.append(
                    kind: "state.barge_in",
                    payload: .object(["client_side": .bool(true)])
                )
            }
        }
    }

    // MARK: - Audio config change  (Amendment O)

    private func handleAudioConfigChange() {
        // AVAudioEngine fires this notification BOTH on real hot-plug events
        // AND synchronously while we ourselves call setVoiceProcessingEnabled
        // / installTap inside AudioCapture.start(). If we react to the latter
        // by stop()+start(), we trigger the same notification chain again —
        // infinite loop, observed in the field as repeated "audio config
        // change — restarting capture" log lines and the connection never
        // settling. Skip notifications during the settle window.
        if let cap = capture, cap.isInStartSettleWindow {
            Loggers.app.debug("audio config change ignored — within capture start settle window")
            return
        }
        Loggers.app.info("audio config change — restarting capture")
        capture?.stop()
        do { try capture?.start() } catch {
            Loggers.app.error("capture restart failed: \(error.localizedDescription)")
        }
    }
}
