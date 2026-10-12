import AVFoundation

/// Microphone → 16 kHz PCM for Gemini, Gemini's 24 kHz PCM → speakers.
///
/// Mic and speaker run on separate engines, the layout v2 proved on this Mac:
/// the mic engine uses Apple's voice processing (echo cancellation) so Aura
/// doesn't hear itself; putting playback in that same engine fails to start
/// on some Macs (error -10875).
final class Audio {
    /// Called on the audio thread with 16-bit little-endian mono PCM at 16 kHz.
    var onMicrophone: ((Data) -> Void)?
    var muted = false

    private var mic = AVAudioEngine()
    private var speaker = AVAudioEngine()
    private var player = AVAudioPlayerNode()
    private var observer: NSObjectProtocol?
    private var startedAt = Date.distantPast
    private var resampler = Resampler(from: 48_000)
    private let speakerFormat = AVAudioFormat(
        commonFormat: .pcmFormatFloat32, sampleRate: 24_000, channels: 1, interleaved: false)!

    func start() throws {
        startedAt = Date()
        mic = AVAudioEngine()
        let input = mic.inputNode
        var echoCancellation = true
        do {
            try input.setVoiceProcessingEnabled(true)
            // Don't turn other apps' audio way down while Aura listens.
            input.voiceProcessingOtherAudioDuckingConfiguration = .init(enableAdvancedDucking: false, duckingLevel: .min)
        } catch {
            echoCancellation = false
            log.warning("Echo cancellation unavailable: \(error.localizedDescription, privacy: .public)")
        }

        // Read the format after enabling voice processing (it changes it) and tap at that
        // native format; channel 0 is the processed voice.
        let format = input.outputFormat(forBus: 0)
        log.info("Mic: \(format.sampleRate, privacy: .public) Hz × \(format.channelCount, privacy: .public) ch, echo cancellation \(echoCancellation ? "on" : "off", privacy: .public)")
        guard format.channelCount > 0, format.sampleRate > 0 else {
            throw AuraError("No usable microphone (check the Microphone permission)")
        }
        resampler = Resampler(from: format.sampleRate)
        input.installTap(onBus: 0, bufferSize: 2048, format: format) { [weak self] buffer, _ in
            self?.forward(buffer)
        }
        mic.prepare()
        try mic.start()

        speaker = AVAudioEngine()
        player = AVAudioPlayerNode()
        speaker.attach(player)
        speaker.connect(player, to: speaker.mainMixerNode, format: speakerFormat)
        speaker.prepare()
        try speaker.start()
        player.play()
        startedAt = Date()

        observer = NotificationCenter.default.addObserver(
            forName: .AVAudioEngineConfigurationChange, object: nil, queue: .main
        ) { [weak self] note in
            // Turning on voice processing posts this itself; restarting on that loops forever.
            // Only react to real changes (headphones, AirPods) once things have settled.
            guard let self, let engine = note.object as? AVAudioEngine,
                  engine === self.mic || engine === self.speaker,
                  Date().timeIntervalSince(self.startedAt) > 2
            else { return }
            self.restart()
        }
    }

    func stop() {
        if let observer { NotificationCenter.default.removeObserver(observer) }
        observer = nil
        mic.inputNode.removeTap(onBus: 0)
        mic.stop()
        player.stop()
        speaker.stop()
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
