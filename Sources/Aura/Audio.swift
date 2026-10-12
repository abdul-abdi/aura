import AVFoundation

/// Microphone → 16 kHz PCM for Gemini, Gemini's 24 kHz PCM → speakers.
///
/// Uses Apple's voice processing (the echo canceller FaceTime uses). Playback
/// runs through the same engine so the canceller knows what Aura is saying:
/// Aura never hears itself and the user can talk over it at any time.
final class Audio {
    /// Called on the audio thread with 16-bit little-endian mono PCM at 16 kHz.
    var onMicrophone: ((Data) -> Void)?
    var muted = false

    private var engine = AVAudioEngine()
    private var player = AVAudioPlayerNode()
    private var observer: NSObjectProtocol?
    private var startedAt = Date.distantPast
    private var resampler = Resampler(from: 48_000)
    private let speakerFormat = AVAudioFormat(
        commonFormat: .pcmFormatFloat32, sampleRate: 24_000, channels: 1, interleaved: false)!

    func start() throws {
        startedAt = Date()
        engine = AVAudioEngine()
        player = AVAudioPlayerNode()

        let input = engine.inputNode
        do {
            try input.setVoiceProcessingEnabled(true)
            // Don't turn other apps' audio way down while Aura listens.
            input.voiceProcessingOtherAudioDuckingConfiguration = .init(enableAdvancedDucking: false, duckingLevel: .min)
        } catch {
            log.warning("Echo cancellation unavailable: \(error.localizedDescription, privacy: .public)")
        }

        // With voice processing on, tap at the input-side hardware format and read
        // channel 0 (the processed voice); the output-side format is wrong here.
        let format = input.inputFormat(forBus: 0)
        guard format.channelCount > 0, format.sampleRate > 0 else {
            throw AuraError("No usable microphone (check the Microphone permission)")
        }
        resampler = Resampler(from: format.sampleRate)
        input.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self] buffer, _ in
            self?.forward(buffer)
        }

        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: speakerFormat)
        engine.prepare()
        try engine.start()
        player.play()
        startedAt = Date()

        observer = NotificationCenter.default.addObserver(
            forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main
        ) { [weak self] _ in
            // Turning on voice processing posts this itself; restarting on that loops forever.
            // Only react to real changes (headphones, AirPods) once things have settled.
            guard let self, Date().timeIntervalSince(self.startedAt) > 2 else { return }
            self.restart()
        }
    }

    func stop() {
        if let observer { NotificationCenter.default.removeObserver(observer) }
        observer = nil
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
    }

    func restart() {
        stop()
        do { try start() } catch { log.error("Audio restart failed: \(error.localizedDescription, privacy: .public)") }
    }

    /// Queue 16-bit little-endian mono PCM at 24 kHz for playback.
    func play(_ pcm: Data) {
        let frames = pcm.count / 2
        guard frames > 0,
              let buffer = AVAudioPCMBuffer(pcmFormat: speakerFormat, frameCapacity: AVAudioFrameCount(frames))
        else { return }
        buffer.frameLength = AVAudioFrameCount(frames)
        let out = buffer.floatChannelData![0]
        pcm.withUnsafeBytes { raw in
            for i in 0..<frames {
                out[i] = Float(Int16(littleEndian: raw.loadUnaligned(fromByteOffset: i * 2, as: Int16.self))) / 32768
            }
        }
        player.scheduleBuffer(buffer)
        if !player.isPlaying { player.play() }
    }

    /// Drop everything queued (the user interrupted).
    func stopPlayback() {
        player.stop()
        player.play()
    }

    private func forward(_ buffer: AVAudioPCMBuffer) {
        guard !muted, let channel = buffer.floatChannelData?[0], buffer.frameLength > 0 else { return }
        let samples = resampler.process(UnsafeBufferPointer(start: channel, count: Int(buffer.frameLength)))
        guard !samples.isEmpty else { return }
        var pcm = Data(capacity: samples.count * 2)
        for sample in samples {
            withUnsafeBytes(of: Int16(max(-1, min(1, sample)) * 32767).littleEndian) { pcm.append(contentsOf: $0) }
        }
        onMicrophone?(pcm)
    }
}

/// Linear-interpolation resampler to 16 kHz that carries its position across buffers.
struct Resampler {
    private let step: Double  // input samples per output sample
    private var position = 0.0  // in the current buffer; -1 means the previous buffer's last sample
    private var last: Float = 0

    init(from sampleRate: Double) { step = sampleRate / 16_000 }

    mutating func process(_ input: UnsafeBufferPointer<Float>) -> [Float] {
        guard !input.isEmpty else { return [] }
        var out: [Float] = []
        out.reserveCapacity(Int(Double(input.count) / step) + 1)
        while position < Double(input.count - 1) {
            let i = Int(position.rounded(.down))
            let a = i < 0 ? last : input[i]
            let b = input[i + 1]
            out.append(a + (b - a) * Float(position - Double(i)))
            position += step
        }
        position -= Double(input.count)
        last = input[input.count - 1]
        return out
    }
}
