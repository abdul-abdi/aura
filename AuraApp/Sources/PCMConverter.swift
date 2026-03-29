// AuraApp/Sources/PCMConverter.swift
import AVFoundation
import Foundation

/// Converts audio buffers from hardware sample rate to 16kHz mono PCM for Gemini.
enum PCMConverter {
    /// Extract channel 0 (processed voice) from a multi-channel buffer and resample to 16kHz.
    /// Voice processing may output 3 channels; only channel 0 contains the processed voice.
    static func convertToGeminiFormat(
        buffer: AVAudioPCMBuffer,
        sourceRate: Double
    ) -> Data? {
        guard let floatData = buffer.floatChannelData else { return nil }

        let frameCount = Int(buffer.frameLength)
        guard frameCount > 0 else { return nil }

        // Extract channel 0 (processed voice)
        let channel0 = Array(UnsafeBufferPointer(start: floatData[0], count: frameCount))

        // Resample to 16kHz if needed
        let targetRate: Double = 16000
        let resampled: [Float]

        if abs(sourceRate - targetRate) < 1.0 {
            resampled = channel0
        } else {
            let ratio = targetRate / sourceRate
            let outputCount = Int(Double(frameCount) * ratio)
            resampled = linearResample(channel0, outputCount: outputCount)
        }

        // Convert Float32 [-1.0, 1.0] to 16-bit LE PCM bytes
        return floatToPCM16(resampled)
    }

    /// Linear interpolation resample (good enough for voice at 16kHz target).
    static func linearResample(_ input: [Float], outputCount: Int) -> [Float] {
        guard outputCount > 0, !input.isEmpty else { return [] }
        if outputCount == 1 { return [input[0]] }

        var output = [Float](repeating: 0, count: outputCount)
        let ratio = Double(input.count - 1) / Double(outputCount - 1)

        for i in 0..<outputCount {
            let srcIndex = Double(i) * ratio
            let lower = Int(srcIndex)
            let upper = min(lower + 1, input.count - 1)
            let frac = Float(srcIndex - Double(lower))
            output[i] = input[lower] * (1.0 - frac) + input[upper] * frac
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
}
