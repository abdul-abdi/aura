// AuraApp/Sources/PCMConverter.swift
import AVFoundation
import Foundation

/// Converts audio buffers from hardware sample rate to 16kHz mono PCM for Gemini.
enum PCMConverter {
    /// Extract channel 0 from a 16kHz mono buffer and convert to PCM16.
    /// AVAudioEngine delivers at 16kHz via installTap format parameter — no resampling needed.
    static func convertToGeminiFormat(
        buffer: AVAudioPCMBuffer
    ) -> Data? {
        guard let floatData = buffer.floatChannelData else { return nil }

        let frameCount = Int(buffer.frameLength)
        guard frameCount > 0 else { return nil }

        // Extract channel 0 (processed voice)
        let channel0 = Array(UnsafeBufferPointer(start: floatData[0], count: frameCount))

        // Convert Float32 [-1.0, 1.0] to 16-bit LE PCM bytes
        return floatToPCM16(channel0)
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
