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
///
/// Spec §5.2 invariant 3 — "Pause mic. Frames buffered in a 2s ring (64 KB).
/// Replay at resume is gated on a VAD-active flag _and_ a wake-state flag —
/// never replay if we just woke from sleep."
///
/// The 2s ring is `rotationRing` below. While `isPaused == true`, resampled
/// f32 samples flow into the ring instead of the wire. On `resumeSending(replay:)`
/// the caller decides whether to replay (false on wake-from-sleep). The audio
/// engine is NOT stopped during pause — frames keep arriving so the ring fills
/// continuously; stop/start during a 9-minute scheduled rotation would also
/// reset Apple's voice-processing IO state and lose the calibration.
public final class AudioCapture {
    public let engine = AVAudioEngine()
    private var resampler: LinearResampler
    private let onChunk: @Sendable (Data) -> Void
    private let onSamples: (@Sendable ([Float]) -> Void)?
    private var hwFormat: AVAudioFormat
    private var enabledVoiceProcessing = false

    /// Spec §5.2 invariant 3: 2 seconds × 16,000 Hz mono = 32,000 Float samples.
    /// (Spec footnotes "64 KB" — that's 32K Int16; we keep f32 here so the
    /// resample step stays out of the audio thread's hot path.)
    public let rotationRing: RingBuffer<Float>

    /// When true, resampled samples are written to `rotationRing` instead of
    /// being PCM16-encoded and pushed to `onChunk`. Toggled by
    /// `pauseSending()` / `resumeSending(replay:)`.
    private var isPaused: Bool = false

    public init(onChunk: @escaping @Sendable (Data) -> Void,
                onSamples: (@Sendable ([Float]) -> Void)? = nil) {
        self.onChunk = onChunk
        self.onSamples = onSamples
        self.hwFormat = AVAudioFormat(standardFormatWithSampleRate: 48000, channels: 1)!
        self.resampler = LinearResampler(sourceRate: 48000, targetRate: 16000)
        self.rotationRing = RingBuffer<Float>(capacity: 32_000)
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
        rotationRing.clear()
        isPaused = false
        Loggers.voice.info("audio capture stopped")
    }

    /// Spec §5.2 step 2: redirect new resampled samples into `rotationRing`
    /// instead of `onChunk`. The audio engine keeps running (so Apple's
    /// voice-processing IO calibration is preserved). The ring is cleared on
    /// each pause so a stale rotation doesn't taint the next one.
    public func pauseSending() {
        rotationRing.clear()
        isPaused = true
        Loggers.voice.info("audio capture paused (rotation buffer engaged)")
    }

    /// Spec §5.2 step 8: drain the ring (if `replay`) then resume normal flow.
    /// Pass `replay: false` when resuming after a wake-from-sleep — replaying
    /// would invent speech the user never made.
    public func resumeSending(replay: Bool) {
        if replay {
            let buffered = rotationRing.readUpTo(rotationRing.currentCount)
            if !buffered.isEmpty {
                let pcm = pcm16Encode(buffered)
                onChunk(pcm)
                Loggers.voice.info("rotation ring replayed: \(buffered.count, privacy: .public) samples")
            }
        } else if rotationRing.currentCount > 0 {
            Loggers.voice.info("rotation ring discarded (post-wake): \(self.rotationRing.currentCount, privacy: .public) samples")
        }
        rotationRing.clear()
        isPaused = false
    }

    private func handleBuffer(_ buf: AVAudioPCMBuffer) {
        guard let channelData = buf.floatChannelData?[0] else { return }
        let frames = Int(buf.frameLength)
        let mono = Array(UnsafeBufferPointer(start: channelData, count: frames))

        // Surface raw mono samples for downstream RMS / barge-in (always —
        // barge-in needs to fire even during rotation if the user starts
        // speaking, though Plan 1 currently only acts on it when not paused).
        onSamples?(mono)

        let resampled = resampler.resample(mono)
        guard !resampled.isEmpty else { return }

        if isPaused {
            // Spec §5.2 invariant 3: divert to ring buffer; consumer drains on resume.
            _ = rotationRing.write(resampled)
        } else {
            let pcm = pcm16Encode(resampled)
            onChunk(pcm)
        }
    }
}
