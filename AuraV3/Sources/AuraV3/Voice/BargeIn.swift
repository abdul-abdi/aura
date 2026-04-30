import Foundation

/// Energy-gate barge-in detector with explicit calibration phase.
///
/// Amendment V: the `calibrate(durationMs:)` method is REQUIRED before
/// `shouldBargeIn` will fire. The original accumulating-mean was vulnerable
/// to training on user voice if the user spoke during the first second.
public final class BargeInDetector {
    public private(set) var threshold: Float = 0.02
    public private(set) var isCalibrated: Bool = false
    private var consecutiveLoud = 0
    private var reverbGuardUntil: Date = .distantPast
    private var ambientSamples: [Float] = []

    public init() {}

    /// Observe an ambient frame during calibration. Idempotent if called
    /// after calibration is committed (treated as drift sample, optional).
    public func observeAmbient(frame: [Float]) {
        let r = Self.rms(frame)
        ambientSamples.append(r)
        if ambientSamples.count >= 50 {
            let mean = ambientSamples.reduce(0, +) / Float(ambientSamples.count)
            threshold = max(mean * 1.3, 0.02)
            if ambientSamples.count > 100 {
                ambientSamples.removeFirst(ambientSamples.count - 100)
            }
        }
    }

    /// Commit calibration. Call after observing enough ambient frames.
    /// `shouldBargeIn` returns false until this is called.
    public func commitCalibration() {
        isCalibrated = true
    }

    /// Convenience: simulated calibration for testing or post-startup. Records
    /// `frameCount` synthetic-ambient observations at the given rms, then commits.
    public func calibrateSynthetic(frameCount: Int = 50, rms: Float = 0.005) {
        for _ in 0..<frameCount {
            ambientSamples.append(rms)
        }
        let mean = ambientSamples.reduce(0, +) / Float(max(ambientSamples.count, 1))
        threshold = max(mean * 1.3, 0.02)
        isCalibrated = true
    }

    public func startReverbGuard(durationMs: Int = 300) {
        reverbGuardUntil = Date().addingTimeInterval(Double(durationMs) / 1000.0)
    }

    public func shouldBargeIn(frame: [Float]) -> Bool {
        // Amendment V: never fire before calibration is committed.
        guard isCalibrated else { return false }
        guard Date() >= reverbGuardUntil else {
            consecutiveLoud = 0
            return false
        }
        let r = Self.rms(frame)
        if r > threshold {
            consecutiveLoud += 1
            if consecutiveLoud >= 2 {
                consecutiveLoud = 0
                return true
            }
        } else {
            consecutiveLoud = 0
        }
        return false
    }

    private static func rms(_ samples: [Float]) -> Float {
        guard !samples.isEmpty else { return 0 }
        var sum: Float = 0
        for s in samples { sum += s * s }
        return (sum / Float(samples.count)).squareRoot()
    }
}
