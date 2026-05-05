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
    // PermissionWatchdog is wired but reportFailure callers are deferred to Plan 2.
    private var permission: PermissionWatchdog!
    private var supervisor: WatchdogSupervisor!

    private var rotation: RotationOrchestrator!
    private var effects: EffectLog!

    private var sessionTimer: Task<Void, Never>?
    private var receiveLoopTask: Task<Void, Never>?

    public init() {
        self.effects = EffectLog(log: state.log)
        self.rotation = RotationOrchestrator(log: state.log, effects: effects)
        self.permission = PermissionWatchdog(log: state.log)
        self.supervisor = WatchdogSupervisor(log: state.log)

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
        let cfg = LiveSessionConfig(apiKey: key)
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

        // Audio capture → send.
        let cap = AudioCapture { [weak self] pcm in
            Task { @MainActor [weak self] in
                guard let self else { return }
                guard !self.state.isMicMuted else { return }
                try? await self.session?.send(.audio(pcm))
            }
        }
        do { try cap.start() } catch {
            Loggers.app.error("capture start failed: \(error.localizedDescription)")
        }
        self.capture = cap

        // Amendment O: AudioConfigWatchdog actually wired.
        // `cap.engine` is already declared `public` on AudioCapture.
        let configWatchdog = AudioConfigWatchdog(engine: cap.engine) { [weak self] in
            self?.handleAudioConfigChange()
        }
        configWatchdog.start()
        self.audioConfig = configWatchdog

        // Playback engine must be started before the receive loop enqueues audio.
        do { try playback.start() } catch {
            Loggers.app.warning("playback start failed: \(error.localizedDescription)")
        }

        // Screen stream — non-fatal if screen recording permission is missing.
        let screen = ScreenStream()
        do { try await screen.start() } catch {
            Loggers.app.warning("screen stream start failed: \(error.localizedDescription)")
        }
        self.screenStream = screen

        // Schedule rotation at T = 9 minutes (before the 10-minute server limit).
        scheduleRotation()
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

            case .toolCall(let id, let name, _):
                // Plan 2 will dispatch to ToolRegistry. Plan 1: respond with "not implemented".
                try? await session.send(.toolResult(
                    callID: id,
                    output: .object([
                        "status": .string("failed"),
                        "detail": .string("Tools not yet implemented in Plan 1"),
                    ])
                ))
                Loggers.action.warning("toolCall \(name, privacy: .public) ignored (Plan 1)")

            case .interrupted:
                playback.flush()
                bargeIn.startReverbGuard()
                state.setDotState(.listening)

            case .resumptionHandle:
                break

            case .closed:
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
        do {
            let new = try await rotation.rotate(
                reason: reason,
                currentSession: cur,
                newSessionFactory: {
                    let s = GeminiLiveSession(config: LiveSessionConfig(apiKey: key))
                    try await s.connect()
                    return s
                },
                pendingEffectCorrelations: {
                    (try? await EffectLog.findOrphans(log: log)) ?? []
                },
                digest: "(Plan 1: minimal digest — full version in Plan 3 Context.swift)",
                playback: self.playback,
                captureMicPaused: { [weak self] in self?.capture?.stop() },
                captureMicResumed: { [weak self] in try? self?.capture?.start() }
            )
            self.session = new
            receiveLoopTask?.cancel()
            receiveLoopTask = Task { @MainActor [weak self] in
                await self?.runReceiveLoop(new)
            }
            state.setDotState(.listening)
            scheduleRotation()
        } catch {
            Loggers.rotation.error("rotation failed: \(error.localizedDescription)")
            state.setDotState(.offline)
        }
    }

    // MARK: - Reconnect

    private func reconnect() async {
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
        state.setDotState(.offline)
        sessionTimer?.cancel()
        receiveLoopTask?.cancel()
        let s = session
        session = nil
        // Best-effort; the OS will yank the socket anyway.
        Task { await s?.close() }
    }

    /// Amendment N: since handleSleep tore everything down, reconnect from scratch
    /// (there is no current session to rotate FROM).
    private func handleWake() {
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
        if status == .satisfied, session != nil {
            Task { @MainActor [weak self] in await self?.rotateNow(reason: .pathChange) }
        }
    }

    // MARK: - Audio config change  (Amendment O)

    private func handleAudioConfigChange() {
        // Device change — restart capture cleanly. Don't attempt to rotate the
        // Gemini session; the audio config notification is about the local engine.
        Loggers.app.info("audio config change — restarting capture")
        capture?.stop()
        do { try capture?.start() } catch {
            Loggers.app.error("capture restart failed: \(error.localizedDescription)")
        }
    }
}
