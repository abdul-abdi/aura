import Foundation
import AVFoundation

public final class AudioPlayback {
    private let engine = AVAudioEngine()
    private let playerNode = AVAudioPlayerNode()
    private let format: AVAudioFormat

    public init() {
        self.format = AVAudioFormat(standardFormatWithSampleRate: 24000, channels: 1)!
        engine.attach(playerNode)
        engine.connect(playerNode, to: engine.mainMixerNode, format: format)
    }

    /// Idempotent — calling start() on an already-running engine is a no-op.
    /// AVAudioEngine.start() throws if already running; we treat that as success.
    public func start() throws {
        if engine.isRunning {
            if !playerNode.isPlaying { playerNode.play() }
            return
        }
        engine.prepare()
        try engine.start()
        playerNode.play()
    }

    public func stop() {
        if playerNode.isPlaying { playerNode.stop() }
        if engine.isRunning { engine.stop() }
    }

    /// Schedule a chunk of 24kHz PCM16 (raw bytes, little-endian).
    public func enqueue(_ pcm16: Data) {
        guard let buf = Self.makeFloatBuffer(from: pcm16, format: format) else { return }
        playerNode.scheduleBuffer(buf, at: nil, options: [], completionHandler: nil)
    }

    /// Drop everything pending. Called on barge-in or rotation.
    public func flush() {
        playerNode.stop()
        playerNode.play()
    }

    private static func makeFloatBuffer(from pcm16: Data, format: AVAudioFormat) -> AVAudioPCMBuffer? {
        let sampleCount = pcm16.count / 2
        guard let buf = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(sampleCount)) else {
            return nil
        }
        buf.frameLength = AVAudioFrameCount(sampleCount)
        let dst = buf.floatChannelData![0]
        pcm16.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
            let p = raw.baseAddress!.assumingMemoryBound(to: Int16.self)
            for i in 0..<sampleCount {
                dst[i] = Float(p[i]) / 32767.0
            }
        }
        return buf
    }
}
