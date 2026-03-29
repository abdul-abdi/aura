// AuraApp/Sources/AudioEngine.swift
import AVFoundation
import Foundation
import os

/// Manages audio capture (with voice processing) and playback (separate engine).
/// Two AVAudioEngine instances: capture engine has voice processing enabled for
/// echo cancellation; playback engine is clean — no voice processing interference.
/// This matches VibeCat's proven architecture for Gemini Live audio.
@MainActor
final class AudioEngine {
    // Capture engine — voice processing enabled for echo cancellation
    private var captureEngine = AVAudioEngine()
    private var isTapInstalled = false
    private let tapFireCount = OSAllocatedUnfairLock(initialState: UInt64(0))
    private var healthCheckTask: Task<Void, Never>?

    // Playback engine — separate, no voice processing
    private var playbackEngine = AVAudioEngine()
    private var playerNode = AVAudioPlayerNode()

    /// Called with base64-encoded 16kHz PCM audio frames from the mic.
    var onAudioFrame: ((String) -> Void)?

    /// Called when playback needs to pause/resume (backpressure signal to daemon).
    var onPlaybackPause: ((Bool) -> Void)?

    // Silence gate
    private let silentFrameCount = OSAllocatedUnfairLock(initialState: 0)
    private let silenceThreshold: Float = 0.001
    private let silentFramesBeforeGate: Int = 3

    // Playback backpressure
    private var playbackPaused = false

    /// Whether the engine is currently capturing.
    private(set) var isCapturing = false

    /// Timestamp of the last audio frame for latency measurement
    private(set) var lastCaptureTime: CFAbsoluteTime = 0

    /// Whether audio is currently playing (for UI state).
    private(set) var isPlaying = false

    // Playback format: 24kHz, 16-bit mono (Gemini's native audio output format)
    private let playbackFormat = AVAudioFormat(
        commonFormat: .pcmFormatInt16,
        sampleRate: 24000,
        channels: 1,
        interleaved: true
    )!

    // Coalescing buffer for playback
    private var playbackBuffer = Data()
    private let minPlaybackBytes = 960   // ~20ms at 24kHz 16-bit mono
    private let maxPlaybackBytes = 96_000 // ~2 seconds
    private var isFirstPlaybackChunk = true

    init() {
        setupPlaybackEngine()
    }

    deinit {
        healthCheckTask?.cancel()
    }

    // MARK: - Playback Engine Setup (separate from capture)

    private func setupPlaybackEngine() {
        playbackEngine.attach(playerNode)
        playbackEngine.connect(playerNode, to: playbackEngine.mainMixerNode, format: playbackFormat)
        do {
            try playbackEngine.start()
            NSLog("[AudioEngine] Playback engine started")
        } catch {
            NSLog("[AudioEngine] Playback engine start failed: %@", error.localizedDescription)
        }
    }

    // MARK: - Capture

    func startCapture() throws {
        let inputNode = captureEngine.inputNode

        // Enable voice processing for echo cancellation (non-fatal if unavailable).
        // VibeCat proves this works with separate capture/playback engines.
        do {
            try inputNode.setVoiceProcessingEnabled(true)
            NSLog("[AudioEngine] Voice processing enabled")
        } catch {
            NSLog("[AudioEngine] Voice processing unavailable: %@", error.localizedDescription)
        }

        // Read hardware format AFTER enabling VP — VP changes the output format.
        // With VP: typically 48kHz, 3 channels (ch0=processed, ch1=ambient, ch2=reserved).
        // Without VP: hardware native (e.g. 48kHz mono/stereo).
        let hwFormat = inputNode.outputFormat(forBus: 0)
        NSLog("[AudioEngine] Hardware format: %.0fHz, %d channels", hwFormat.sampleRate, hwFormat.channelCount)

        guard hwFormat.sampleRate > 0, hwFormat.channelCount > 0 else {
            NSLog("[AudioEngine] Invalid hardware format — cannot capture")
            throw NSError(domain: "AudioEngine", code: -1, userInfo: [
                NSLocalizedDescriptionKey: "Audio input format unavailable"
            ])
        }

        let sourceSampleRate = hwFormat.sampleRate

        // Install tap with hardware format (NOT 16kHz).
        // Requesting a non-native format silently fails without voice processing.
        inputNode.installTap(onBus: 0, bufferSize: 2048, format: hwFormat) { [weak self] buffer, _ in
            guard let self else { return }
            self.tapFireCount.withLock { $0 += 1 }

            let captureTime = CFAbsoluteTimeGetCurrent()

            // Extract channel 0 into a mono view for RMS + conversion
            guard let floatData = buffer.floatChannelData else { return }
            let frameCount = Int(buffer.frameLength)
            guard frameCount > 0 else { return }

            // Compute RMS energy on channel 0 for silence gate
            let samples = floatData[0]
            var sumSquares: Float = 0
            for i in 0..<frameCount {
                let s = samples[i]
                sumSquares += s * s
            }
            let rms = sqrtf(sumSquares / Float(frameCount))

            if rms < self.silenceThreshold {
                let count = self.silentFrameCount.withLock { val -> Int in
                    val += 1
                    return val
                }
                if count >= self.silentFramesBeforeGate {
                    return  // Skip sending silence
                }
            } else {
                self.silentFrameCount.withLock { $0 = 0 }
            }

            // Create a mono buffer from channel 0 for PCMConverter
            let monoFormat = AVAudioFormat(
                commonFormat: .pcmFormatFloat32,
                sampleRate: sourceSampleRate,
                channels: 1,
                interleaved: false
            )!
            guard let monoBuffer = AVAudioPCMBuffer(
                pcmFormat: monoFormat,
                frameCapacity: buffer.frameLength
            ) else { return }
            monoBuffer.frameLength = buffer.frameLength
            monoBuffer.floatChannelData![0].update(from: samples, count: frameCount)

            // Resample to 16kHz + PCM16 encode
            guard let pcmData = PCMConverter.convertToGeminiFormat(
                buffer: monoBuffer,
                sourceSampleRate: sourceSampleRate
            ) else { return }

            let base64 = PCMConverter.encodeBase64(pcmData)

            Task { @MainActor [weak self] in
                self?.lastCaptureTime = captureTime
                self?.onAudioFrame?(base64)
            }
        }
        isTapInstalled = true

        captureEngine.prepare()
        try captureEngine.start()
        isCapturing = true
        NSLog("[AudioEngine] Capture started: %.0fHz/%dch → 16kHz mono", hwFormat.sampleRate, hwFormat.channelCount)

        // Start tap health check
        startHealthCheck()

        // Listen for audio device changes (AirPods connect, etc.)
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handleConfigChange),
            name: .AVAudioEngineConfigurationChange,
            object: captureEngine
        )
    }

    func stopCapture() {
        healthCheckTask?.cancel()
        healthCheckTask = nil
        NotificationCenter.default.removeObserver(self, name: .AVAudioEngineConfigurationChange, object: captureEngine)

        if isTapInstalled {
            captureEngine.inputNode.removeTap(onBus: 0)
            isTapInstalled = false
        }
        captureEngine.stop()
        isCapturing = false
    }

    // MARK: - Playback

    /// Append incoming PCM audio from Gemini (base64-encoded 24kHz 16-bit LE mono).
    func appendPlaybackAudio(_ base64: String) {
        guard let data = Data(base64Encoded: base64) else { return }
        playbackBuffer.append(data)

        // Discard stale audio if buffer grows too large
        if playbackBuffer.count > maxPlaybackBytes {
            playbackBuffer = Data()
        }

        // Backpressure: signal daemon to pause if buffer is too large
        if playbackBuffer.count > 48_000 && !playbackPaused {
            playbackPaused = true
            onPlaybackPause?(true)
        }

        // Schedule first chunk immediately for lowest perceived latency
        if isFirstPlaybackChunk {
            let chunk = playbackBuffer
            playbackBuffer = Data()
            isFirstPlaybackChunk = false
            if playbackPaused {
                playbackPaused = false
                onPlaybackPause?(false)
            }
            schedulePlayback(pcm16Data: chunk)
            return
        }

        guard playbackBuffer.count >= minPlaybackBytes else { return }

        let chunk = playbackBuffer
        playbackBuffer = Data()
        if playbackPaused {
            playbackPaused = false
            onPlaybackPause?(false)
        }
        schedulePlayback(pcm16Data: chunk)
    }

    /// Start a new playback stream (resets buffer).
    func startPlaybackStream() {
        playbackBuffer = Data()
        isFirstPlaybackChunk = true
        if !playerNode.isPlaying {
            playerNode.play()
        }
        isPlaying = true
    }

    /// Stop playback immediately (barge-in).
    func stopPlayback() {
        playerNode.stop()
        playbackBuffer = Data()
        isFirstPlaybackChunk = true
        isPlaying = false
        if playbackPaused {
            playbackPaused = false
            onPlaybackPause?(false)
        }
    }

    private func schedulePlayback(pcm16Data: Data) {
        let frameCount = AVAudioFrameCount(pcm16Data.count / 2)
        guard frameCount > 0 else { return }

        guard let buffer = AVAudioPCMBuffer(
            pcmFormat: playbackFormat,
            frameCapacity: frameCount
        ) else { return }

        buffer.frameLength = frameCount
        pcm16Data.withUnsafeBytes { rawPtr in
            guard let int16Ptr = rawPtr.bindMemory(to: Int16.self).baseAddress,
                  let channelData = buffer.int16ChannelData?[0] else { return }
            channelData.update(from: int16Ptr, count: Int(frameCount))
        }

        // Restart playback engine if it stopped
        if !playbackEngine.isRunning {
            do {
                try playbackEngine.start()
            } catch {
                NSLog("[AudioEngine] playback engine restart failed: %@", error.localizedDescription)
                return
            }
        }

        playerNode.scheduleBuffer(buffer)
        if !playerNode.isPlaying {
            playerNode.play()
        }
    }

    // MARK: - Health Check

    private func startHealthCheck() {
        healthCheckTask = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 5_000_000_000)
                guard let self, self.isCapturing else { continue }

                let previousCount = self.tapFireCount.withLock { $0 }
                try? await Task.sleep(nanoseconds: 1_000_000_000)
                let currentCount = self.tapFireCount.withLock { $0 }
                if currentCount == previousCount && self.isCapturing {
                    self.restartCapture()
                }
            }
        }
    }

    private func restartCapture() {
        stopCapture()
        do {
            try startCapture()
        } catch {
            NSLog("[AudioEngine] restart failed: %@", error.localizedDescription)
        }
    }

    // MARK: - Device Change

    @objc private func handleConfigChange(_ notification: Notification) {
        Task { @MainActor [weak self] in
            self?.restartCapture()
            // Rebuild playback engine too (device may have changed)
            self?.playerNode.stop()
            self?.playbackEngine.stop()
            self?.playbackEngine.detach(self!.playerNode)
            self?.playbackEngine = AVAudioEngine()
            self?.playerNode = AVAudioPlayerNode()
            self?.setupPlaybackEngine()
        }
    }
}
