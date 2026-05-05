import Foundation
import AVFoundation
import OSLog

/// Mic capture using AVAudioEngine with voice-processing IO enabled.
/// Implements the VibeCat pattern v2 painfully discovered:
///   1. Enable voice processing (echo cancel) on the input node BEFORE reading the format
///   2. Read the *hardware* format AFTER VP is on (VP changes the input format)
///   3. Install the tap at hardware format (NOT 16kHz — Apple's tap can't convert there)
///   4. Extract channel 0 (the processed voice channel) into mono
///   5. Resample manually to 16kHz f32 → PCM16 → base64 for the wire
///
/// Amendment I: uses `inputFormat(forBus:)` NOT `outputFormat(forBus:)` — after
/// `setVoiceProcessingEnabled(true)`, the inputFormat is the bus-delivered format
/// that the tap actually receives.
///
/// The `engine` property is exposed publicly so AudioConfigWatchdog (Task 7.3 /
/// amendment O) can observe it without needing a separate accessor.
public final class AudioCapture {
    public let engine = AVAudioEngine()
    private var resampler: LinearResampler
    private let onChunk: @Sendable (Data) -> Void
    private let onSamples: (@Sendable ([Float]) -> Void)?
    private var hwFormat: AVAudioFormat
    private var enabledVoiceProcessing = false

    public init(onChunk: @escaping @Sendable (Data) -> Void,
                onSamples: (@Sendable ([Float]) -> Void)? = nil) {
        self.onChunk = onChunk
        self.onSamples = onSamples
        self.hwFormat = AVAudioFormat(standardFormatWithSampleRate: 48000, channels: 1)!
        self.resampler = LinearResampler(sourceRate: 48000, targetRate: 16000)
    }

    public func start() throws {
        let input = engine.inputNode
        do {
            try input.setVoiceProcessingEnabled(true)
            enabledVoiceProcessing = true
        } catch {
            Loggers.voice.warning("voice-processing IO unavailable; continuing without echo cancel: \(error.localizedDescription)")
        }
        // Amendment I: inputFormat (NOT outputFormat) is what the tap actually receives.
        hwFormat = input.inputFormat(forBus: 0)
        Loggers.voice.info("capture HW format: \(self.hwFormat.sampleRate, privacy: .public)Hz × \(self.hwFormat.channelCount, privacy: .public)ch")
        resampler = LinearResampler(sourceRate: hwFormat.sampleRate, targetRate: 16000)

        input.installTap(onBus: 0, bufferSize: 512, format: hwFormat) { [weak self] buf, _ in
            self?.handleBuffer(buf)
        }

        engine.prepare()
        try engine.start()
        Loggers.voice.info("audio capture started")
    }

    public func stop() {
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        Loggers.voice.info("audio capture stopped")
    }

    private func handleBuffer(_ buf: AVAudioPCMBuffer) {
        guard let channelData = buf.floatChannelData?[0] else { return }
        let frames = Int(buf.frameLength)
        let mono = Array(UnsafeBufferPointer(start: channelData, count: frames))

        // Surface raw mono samples for downstream RMS / barge-in.
        onSamples?(mono)

        let resampled = resampler.resample(mono)
        guard !resampled.isEmpty else { return }
        let pcm = pcm16Encode(resampled)
        onChunk(pcm)
    }
}
