import AVFoundation

/// Microphone → 16 kHz PCM for Gemini, Gemini's 24 kHz PCM → speakers.
///
/// Uses Apple's voice processing (the echo canceller FaceTime uses), so Aura
/// never hears itself and the user can talk over it at any time.
final class Audio {
    /// Called on the audio thread with 16-bit little-endian mono PCM at 16 kHz.
    var onMicrophone: ((Data) -> Void)?
    var muted = false

    private var engine = AVAudioEngine()
    private var player = AVAudioPlayerNode()
    private var observer: NSObjectProtocol?
    private let speakerFormat = AVAudioFormat(
        commonFormat: .pcmFormatFloat32, sampleRate: 24_000, channels: 1, interleaved: false)!
    private let micFormat = AVAudioFormat(
        commonFormat: .pcmFormatInt16, sampleRate: 16_000, channels: 1, interleaved: true)!

    func start() throws {
        engine = AVAudioEngine()
        player = AVAudioPlayerNode()

        let input = engine.inputNode
        try input.setVoiceProcessingEnabled(true)
        // Don't turn other apps' audio way down while Aura listens.
        input.voiceProcessingOtherAudioDuckingConfiguration = .init(enableAdvancedDucking: false, duckingLevel: .min)

        let inputFormat = input.outputFormat(forBus: 0)
        guard inputFormat.channelCount > 0, let converter = AVAudioConverter(from: inputFormat, to: micFormat) else {
            throw AuraError("No usable microphone (check the Microphone permission)")
        }
        input.installTap(onBus: 0, bufferSize: 4096, format: inputFormat) { [weak self] buffer, _ in
            self?.forward(buffer, converter)
        }

        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: speakerFormat)
        engine.prepare()
        try engine.start()
        player.play()

        // Headphones plugged in, AirPods connected, etc.
        observer = NotificationCenter.default.addObserver(
            forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main
        ) { [weak self] _ in self?.restart() }
    }

    func stop() {
        if let observer { NotificationCenter.default.removeObserver(observer) }
        observer = nil
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
    }

    func restart() {
        stop()
        try? start()
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

    private func forward(_ buffer: AVAudioPCMBuffer, _ converter: AVAudioConverter) {
        guard !muted else { return }
        let capacity = AVAudioFrameCount(Double(buffer.frameLength) * 16_000 / buffer.format.sampleRate) + 64
        guard let out = AVAudioPCMBuffer(pcmFormat: micFormat, frameCapacity: capacity) else { return }
        var consumed = false
        var error: NSError?
        converter.convert(to: out, error: &error) { _, status in
            if consumed {
                status.pointee = .noDataNow
                return nil
            }
            consumed = true
            status.pointee = .haveData
            return buffer
        }
        guard error == nil, out.frameLength > 0, let samples = out.int16ChannelData else { return }
        onMicrophone?(Data(bytes: samples[0], count: Int(out.frameLength) * 2))
    }
}
