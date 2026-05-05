import Foundation

public struct Backoff {
    public let base: Duration
    public let max: Duration
    public let jitter: Double  // 0…1

    private var attempt: Int = 0

    public init(base: Duration = .milliseconds(200),
                 max: Duration = .milliseconds(3200),
                 jitter: Double = 0.25) {
        self.base = base
        self.max = max
        self.jitter = jitter
    }

    public mutating func next() -> Duration {
        let d = currentRaw()
        attempt += 1
        return applyJitter(d)
    }

    public func peek() -> Duration {
        applyJitter(currentRaw())
    }

    public mutating func reset() { attempt = 0 }

    private func currentRaw() -> Duration {
        let mult = (1 << min(attempt, 16))
        let raw = base * mult
        return raw < max ? raw : max
    }

    private func applyJitter(_ d: Duration) -> Duration {
        let dMs = durationToMs(d)
        guard jitter > 0 else { return .milliseconds(dMs) }
        let delta = Double(dMs) * jitter
        let lo = Double(dMs) - delta
        let hi = Double(dMs) + delta
        let pick = Double.random(in: lo...hi)
        return .milliseconds(Int(pick.rounded()))
    }

    /// Convert Duration to integer milliseconds.
    private func durationToMs(_ d: Duration) -> Int {
        let s = d.components.seconds
        let attos = d.components.attoseconds
        // 1 ms = 1e15 attoseconds
        return Int(s) * 1000 + Int(attos / 1_000_000_000_000_000)
    }
}

/// Amendment S: rate-limit reconnect/rotate attempts by wall-clock interval,
/// independent of attempt count. Default: max 1 attempt per 5 seconds.
/// WiFi-handoff flap can fire 5+ rotation triggers in 2 seconds — without this
/// cap, the attempt budget is consumed by background events the user never sees.
public struct RateLimit: Sendable {
    public let minIntervalSeconds: Double
    private var lastAttemptAt: Date = .distantPast

    public init(minIntervalSeconds: Double = 5.0) {
        self.minIntervalSeconds = minIntervalSeconds
    }

    /// Returns true if enough time has passed since the last attempt to allow
    /// another. Records the attempt time as a side effect when it returns true.
    public mutating func tryAcquire() -> Bool {
        let now = Date()
        if now.timeIntervalSince(lastAttemptAt) >= minIntervalSeconds {
            lastAttemptAt = now
            return true
        }
        return false
    }

    public mutating func reset() { lastAttemptAt = .distantPast }
}
