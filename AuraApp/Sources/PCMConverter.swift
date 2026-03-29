// AuraApp/Sources/PCMConverter.swift
import AVFoundation
import Foundation

/// Converts audio buffers from hardware sample rate to 16kHz mono PCM for Gemini.
enum PCMConverter {
    /// Extract channel 0, resample to 16kHz if needed, and convert to PCM16.
    /// The tap delivers at hardware sample rate — we resample here.
    static func convertToGeminiFormat(
        buffer: AVAudioPCMBuffer,
        sourceSampleRate: Double = 16000
    ) -> Data? {
        guard let floatData = buffer.floatChannelData else { return nil }

        let frameCount = Int(buffer.frameLength)
        guard frameCount > 0 else { return nil }

        // Extract channel 0 (processed voice when VP is active)
        var channel0 = Array(UnsafeBufferPointer(start: floatData[0], count: frameCount))

        // Resample to 16kHz if hardware rate differs
        if sourceSampleRate != 16000 {
            channel0 = resample(channel0, fromRate: sourceSampleRate, toRate: 16000)
        }

        guard !channel0.isEmpty else { return nil }

        // Convert Float32 [-1.0, 1.0] to 16-bit LE PCM bytes
        return floatToPCM16(channel0)
    }

    /// Linear-interpolation resampling. Adequate for speech downsampling (e.g. 48kHz → 16kHz).
    static func resample(_ samples: [Float], fromRate: Double, toRate: Double) -> [Float] {
        guard fromRate != toRate, !samples.isEmpty else { return samples }
        let ratio = fromRate / toRate
        let outputCount = Int(Double(samples.count) / ratio)
        guard outputCount > 0 else { return [] }

        var output = [Float](repeating: 0, count: outputCount)
        for i in 0..<outputCount {
            let srcPos = Double(i) * ratio
            let srcIdx = Int(srcPos)
            let frac = Float(srcPos - Double(srcIdx))
            if srcIdx + 1 < samples.count {
                output[i] = samples[srcIdx] * (1 - frac) + samples[srcIdx + 1] * frac
            } else if srcIdx < samples.count {
                output[i] = samples[srcIdx]
            }
        }
        return output
    }

    /// Convert f32 samples to 16-bit little-endian PCM Data.
    static func floatToPCM16(_ samples: [Float]) -> Data {
        var data = Data(capacity: samples.count * 2)
        for sample in samples {
            let clamped = max(-1.0, min(1.0, sample))
            let int16 = Int16(clamped * 32767.0)
            withUnsafeBytes(of: int16.littleEndian) { data.append(contentsOf: $0) }
        }
        return data
    }

    /// Encode PCM Data to base64 string for JSONL transport.
    static func encodeBase64(_ pcmData: Data) -> String {
        pcmData.base64EncodedString()
    }

    /// Compute RMS energy of channel 0 for silence detection.
    static func rmsEnergy(buffer: AVAudioPCMBuffer) -> Float {
        guard let floatData = buffer.floatChannelData else { return 0 }
        let frameCount = Int(buffer.frameLength)
        guard frameCount > 0 else { return 0 }

        let channel0 = floatData[0]
        var sumSquares: Float = 0
        for i in 0..<frameCount {
            let sample = channel0[i]
            sumSquares += sample * sample
        }
        return sqrtf(sumSquares / Float(frameCount))
    }
}
