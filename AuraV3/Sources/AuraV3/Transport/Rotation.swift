import Foundation

public protocol LogView: Sendable {
    func recentTurns(limit: Int) async -> [LogEvent]
}

public struct PendingToolIntent: Sendable, Equatable {
    public let operation: String
    public let args: JSONValue
    public init(operation: String, args: JSONValue) {
        self.operation = operation
        self.args = args
    }
}

public enum RotationDigest {
    /// Pure function. Deterministic given inputs. Used by Context.swift in Plan 3
    /// for the full version; this Plan 1 version is byte-controlled and minimal.
    public static func build(
        log: LogView,
        activeGoalStack: [String],
        pendingTool: PendingToolIntent?,
        rollingSummary: String,
        inferredSubgoal: String?
    ) -> String {
        var lines: [String] = []
        lines.append("[Aura session continuation]")
        if !activeGoalStack.isEmpty {
            lines.append("Active goal stack: \(activeGoalStack.joined(separator: " \u{25b8} "))")
        }
        if let sg = inferredSubgoal { lines.append("Inferred subgoal: \(sg)") }
        if !rollingSummary.isEmpty { lines.append("Recent context: \(rollingSummary)") }
        if let p = pendingTool {
            let argsJSON = (try? JSONEncoder().encode(p.args))
                .flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
            lines.append("Pending tool intent: \(p.operation) \(argsJSON)")
        }
        return lines.joined(separator: "\n")
    }
}

@MainActor
public final class RotationOrchestrator {
    public enum Reason: Sendable { case scheduled, disconnect, wake, pathChange }

    public enum RotationError: Error {
        case alreadyInProgress
        case setupCompleteTimeout
    }

    private let log: Log
    private var rotationInProgress = false
    private let setupCompleteTimeoutSeconds: Double = 8.0

    public init(log: Log) { self.log = log }

    /// Returns the new session iff rotation completed (all invariants held).
    /// Amendment S: declare success only after setupComplete arrives.
    public func rotate(
        reason: Reason,
        currentSession: GeminiLiveSession,
        newSessionFactory: @escaping @Sendable () async throws -> GeminiLiveSession,
        pendingEffectCorrelations: [String],
        digest: String,
        playback: AudioPlayback,
        captureMicPaused: () -> Void,
        captureMicResumed: () -> Void
    ) async throws -> GeminiLiveSession {
        guard !rotationInProgress else { throw RotationError.alreadyInProgress }
        rotationInProgress = true
        defer { rotationInProgress = false }

        Loggers.rotation.info("rotation start: reason=\(String(describing: reason), privacy: .public), pending_effects=\(pendingEffectCorrelations.count, privacy: .public)")

        // Invariant 4: pause mic before we close.
        captureMicPaused()

        // Invariant 3: drain playback.
        playback.stop()

        // Close old transport.
        await currentSession.close()

        // Build new session, wait for setupComplete (amendment S).
        let newSession = try await newSessionFactory()
        try await waitForSetupComplete(on: newSession)

        // Send the digest on the new socket.
        try await newSession.send(.clientText(digest))

        // Play the rotation cue via the new playback engine.
        try? playback.start()
        playback.enqueue(AudioCue.rotationCuePCM16())

        // Resume mic.
        captureMicResumed()
        Loggers.rotation.info("rotation complete")
        return newSession
    }

    /// Amendment S: wait for setupComplete from the new session before declaring success.
    /// Without this, "WS connected" was treated as "session ready" — wrong; the server
    /// hasn't accepted the setup until setupComplete arrives.
    private func waitForSetupComplete(on session: GeminiLiveSession) async throws {
        let stream = session.receive()
        let timeoutSecs = setupCompleteTimeoutSeconds
        try await withThrowingTaskGroup(of: Void.self) { group in
            group.addTask {
                for await event in stream {
                    if case .setupComplete = event { return }
                }
                // Stream finished without setupComplete.
                throw RotationError.setupCompleteTimeout
            }
            group.addTask {
                try await Task.sleep(nanoseconds: UInt64(timeoutSecs * 1_000_000_000))
                throw RotationError.setupCompleteTimeout
            }
            try await group.next()
            group.cancelAll()
        }
    }
}
