import Foundation

/// Generate an 80ms 440Hz sine wave at -24dB as 24kHz mono PCM16.
/// Used as the rotation audio cue so the user knows a session refresh is happening.
public enum AudioCue {
    public static func rotationCuePCM16() -> Data {
        let durationMs = 80
        let frequency: Double = 440
        let sampleRate: Double = 24000
        let amplitude: Float = 0.063  // ≈ -24 dBFS (10^(-24/20))
        let count = Int(Double(durationMs) / 1000.0 * sampleRate)
        var samples: [Float] = []
        samples.reserveCapacity(count)
        let fadeSamples = Int(0.005 * sampleRate)
        for i in 0..<count {
            let t = Double(i) / sampleRate
            var s = amplitude * Float(sin(2 * .pi * frequency * t))
            if i < fadeSamples { s *= Float(i) / Float(fadeSamples) }
            else if i > count - fadeSamples { s *= Float(count - i) / Float(fadeSamples) }
            samples.append(s)
        }
        return pcm16Encode(samples)
    }
}
