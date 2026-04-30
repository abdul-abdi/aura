import Foundation
import AppKit

// MARK: - AppState

/// Central observable state for the Aura v3 app.
///
/// Constructed synchronously on the main actor (amendment J). Opens the event log
/// via `Log.open(path:)` — which is now a synchronous call — then fires
/// `session.start` and kicks off the periodic WAL checkpoint loop.
@MainActor
public final class AppState: ObservableObject {

    // MARK: - DotState

    public enum DotState: String, Sendable {
        case idle, listening, speaking, rotating, offline
    }

    // MARK: - Published state

    @Published public private(set) var dotState: DotState = .offline
    @Published public private(set) var sessionID: String = UUID().uuidString
    @Published public private(set) var isMicMuted: Bool = false
    @Published public private(set) var isProactiveMuted: Bool = false

    // MARK: - Log

    public let log: Log

    // MARK: - Shared singleton

    public static let shared: AppState = {
        let path = AppState.databasePath()
        do {
            let log = try Log.open(path: path)  // synchronous (amendment J)
            return AppState(log: log)
        } catch {
            // Without the log the app cannot record any events — treat as fatal.
            fatalError("could not open event log at \(path): \(error)")
        }
    }()

    // MARK: - Init

    private init(log: Log) {
        self.log = log
        let sid = sessionID
        // Fire-and-forget: record session.start into the log.
        Task { await log.append(kind: EventKind.sessionStart, sessionID: sid, payload: .null) }
        // Kick off the 5-minute periodic WAL checkpoint loop.
        Task { await self.log.startPeriodicCheckpoint() }
    }

    // MARK: - Mutators

    public func setDotState(_ s: DotState) { dotState = s }
    public func toggleMic() { isMicMuted.toggle() }
    public func toggleProactive() { isProactiveMuted.toggle() }

    // MARK: - Helpers

    private static func databasePath() -> String {
        let support = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)
            .first!
            .appendingPathComponent("aura", isDirectory: true)
        try? FileManager.default.createDirectory(at: support, withIntermediateDirectories: true)
        return support.appendingPathComponent("aura.sqlite").path
    }
}
