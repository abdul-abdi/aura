// AuraApp/Sources/AudioEngine.swift
import AVFoundation
import Foundation
import os

/// Manages audio capture and playback using AVAudioEngine with voice processing.
/// Voice processing provides native echo cancellation, noise suppression, and AGC.
@MainActor
final class AudioEngine {
    private var engine = AVAudioEngine()
    private var playerNode = AVAudioPlayerNode()
    private var isTapInstalled = false
    private let tapFireCount = OSAllocatedUnfairLock(initialState: UInt64(0))
    private var healthCheckTask: Task<Void, Never>?

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
        commonFormat: .pcmFormatFloat32,
        sampleRate: 24000,
        channels: 1,
        interleaved: false
    )!

    // Coalescing buffer for playback: accumulate >= 960 bytes (~20ms at 24kHz) before scheduling
    private var playbackBuffer = Data()
    private let minPlaybackBytes = 960
    private let maxPlaybackBytes = 96_000  // ~2 seconds at 24kHz 16-bit mono
    private var isFirstPlaybackChunk = true

    deinit {
        healthCheckTask?.cancel()
    }

    // MARK: - Capture

    func startCapture() throws {
        let inputNode = engine.inputNode

        // Enable voice processing — provides echo cancellation, noise suppression, AGC
        try inputNode.setVoiceProcessingEnabled(true)

        let captureFormat = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: 16000,
            channels: 1,
            interleaved: false
        )!

        inputNode.installTap(onBus: 0, bufferSize: 2048, format: captureFormat) { [weak self] buffer, _ in
            guard let self else { return }
            self.tapFireCount.withLock { $0 += 1 }

            let captureTime = CFAbsoluteTimeGetCurrent()

            // Compute RMS energy for silence gate
            let rms = PCMConverter.rmsEnergy(buffer: buffer)
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

            // AVAudioEngine delivers at 16kHz mono — just convert to PCM16
            guard let pcmData = PCMConverter.convertToGeminiFormat(
                buffer: buffer
            ) else { return }

            let base64 = PCMConverter.encodeBase64(pcmData)

            Task { @MainActor [weak self] in
                self?.lastCaptureTime = captureTime
                self?.onAudioFrame?(base64)
            }
        }
        isTapInstalled = true

        // Attach player node for playback
        engine.attach(playerNode)
        engine.connect(playerNode, to: engine.mainMixerNode, format: playbackFormat)

        try engine.start()
        isCapturing = true

        // Start tap health check
        startHealthCheck()

        // Listen for audio device changes (AirPods connect, etc.)
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handleConfigChange),
            name: .AVAudioEngineConfigurationChange,
            object: engine
        )
    }

    func stopCapture() {
        healthCheckTask?.cancel()
        healthCheckTask = nil
        NotificationCenter.default.removeObserver(self, name: .AVAudioEngineConfigurationChange, object: engine)

        if isTapInstalled {
            engine.inputNode.removeTap(onBus: 0)
            isTapInstalled = false
        }
        engine.detach(playerNode)
        engine.stop()
        isCapturing = false
    }

    // MARK: - Playback

    /// Append incoming PCM audio from Gemini (base64-encoded 24kHz 16-bit LE mono).
    func appendPlaybackAudio(_ base64: String) {
        guard let data = Data(base64Encoded: base64) else { return }
        playbackBuffer.append(data)

        // Discard stale audio if buffer grows too large (hard cap)
        if playbackBuffer.count > maxPlaybackBytes {
            playbackBuffer = Data()
        }

        // Backpressure: signal daemon to pause if buffer is too large
        if playbackBuffer.count > 48_000 && !playbackPaused {
            playbackPaused = true
            onPlaybackPause?(true)
        }

        // Schedule first chunk immediately for lowest perceived latency;
        // coalesce subsequent chunks for smooth playback.
        if isFirstPlaybackChunk {
            let chunk = playbackBuffer
            playbackBuffer = Data()
            isFirstPlaybackChunk = false
            // Signal resume when buffer drains
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
        // Signal resume when buffer drains
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
        // Reset backpressure state — if paused, send resume so daemon unblocks
        if playbackPaused {
            playbackPaused = false
            onPlaybackPause?(false)
        }
    }

    private func schedulePlayback(pcm16Data: Data) {
        // Convert 16-bit LE PCM to Float32 for AVAudioPlayerNode
        let sampleCount = pcm16Data.count / 2
        guard sampleCount > 0 else { return }

        guard let buffer = AVAudioPCMBuffer(
            pcmFormat: playbackFormat,
            frameCapacity: AVAudioFrameCount(sampleCount)
        ) else { return }

        buffer.frameLength = AVAudioFrameCount(sampleCount)
        guard let floatChannelData = buffer.floatChannelData else { return }
        let floatPtr = floatChannelData[0]

        pcm16Data.withUnsafeBytes { rawBuffer in
            let int16Ptr = rawBuffer.bindMemory(to: Int16.self)
            for i in 0..<sampleCount {
                floatPtr[i] = Float(int16Ptr[i]) / 32768.0
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
                try? await Task.sleep(nanoseconds: 5_000_000_000) // 5 seconds
                guard let self, self.isCapturing else { continue }

                let previousCount = self.tapFireCount.withLock { $0 }
                try? await Task.sleep(nanoseconds: 1_000_000_000) // 1 more second
                let currentCount = self.tapFireCount.withLock { $0 }
                if currentCount == previousCount && self.isCapturing {
                    // Tap stalled — restart
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
            print("[AudioEngine] restart failed: \(error.localizedDescription)")
        }
    }

    // MARK: - Device Change

    @objc private func handleConfigChange(_ notification: Notification) {
        // Audio device changed (AirPods connected, external mic plugged in, etc.)
        // Restart the engine with the new device configuration
        Task { @MainActor [weak self] in
            self?.restartCapture()
        }
    }
}
