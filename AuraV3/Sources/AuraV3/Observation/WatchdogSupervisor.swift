import Foundation

/// Wraps a watchdog start function in a supervised Task: catches errors,
/// appends a state.watchdog_crashed event, and restarts with exponential
/// backoff capped at 60s.
///
/// Amendment P: without this, the spec §5.5 promise that watchdogs "fail
/// independently / restart on crash" is performative.
public actor WatchdogSupervisor {
    private let log: Log
    private var supervised: [String: Task<Void, Never>] = [:]

    public init(log: Log) { self.log = log }

    public func supervise(name: String,
                            start: @escaping @Sendable () async throws -> Void) {
        supervised[name]?.cancel()
        let task = Task { [weak self] in
            guard let self else { return }
            var attempt = 0
            while !Task.isCancelled {
                do {
                    try await start()
                    return  // normal return — done
                } catch {
                    if Task.isCancelled { return }
                    let backoffSeconds = min(60, 1 << min(attempt, 6))
                    await self.recordCrash(name: name, error: error, backoffSeconds: backoffSeconds)
                    attempt += 1
                    try? await Task.sleep(nanoseconds: UInt64(backoffSeconds) * 1_000_000_000)
                }
            }
        }
        supervised[name] = task
    }

    public func stop(name: String) {
        supervised[name]?.cancel()
        supervised.removeValue(forKey: name)
    }

    public func stopAll() {
        for (_, t) in supervised { t.cancel() }
        supervised.removeAll()
    }

    private func recordCrash(name: String, error: Error, backoffSeconds: Int) async {
        await log.append(
            kind: "state.watchdog_crashed",
            payload: .object([
                "name": .string(name),
                "error": .string(String(describing: error)),
                "backoff_s": .number(Double(backoffSeconds)),
            ])
        )
        Loggers.observation.error("watchdog \(name, privacy: .public) crashed: \(error.localizedDescription); restart in \(backoffSeconds, privacy: .public)s")
    }
}
