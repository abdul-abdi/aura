import Foundation
import ScreenCaptureKit
import CoreImage
import VideoToolbox
import OSLog
import os

/// 2 FPS screen capture via ScreenCaptureKit. Each frame is JPEG-encoded
/// (≤ 1920px wide) and put into a single-slot FrameSlot if the perceptual
/// hash changed. Unchanged frames are dropped.
///
/// Amendment T: hash compare and slot.put() happen synchronously on the
/// SCK output queue. Hopping to MainActor would introduce ordering races
/// (SCK delivers serially; the actor hop reorders).
public final class ScreenStream: NSObject, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    public let slot = FrameSlot<Data>()

    private var stream: SCStream?
    /// Updated only on `queue`. Not shared across threads — no atomic needed.
    private var lastHash: UInt64 = 0
    private let queue = DispatchQueue(label: "dev.abdullahi.AuraV3.screen")

    @MainActor
    public func start() async throws {
        let content = try await SCShareableContent.current
        guard let display = content.displays.first else {
            throw NSError(domain: "Aura.Screen", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "no displays"])
        }
        let filter = SCContentFilter(display: display, excludingApplications: [], exceptingWindows: [])
        let config = SCStreamConfiguration()
        config.width = min(1920, display.width)
        config.height = Int(Double(display.height) * Double(config.width) / Double(display.width))
        config.minimumFrameInterval = CMTime(value: 1, timescale: 2)  // 2 FPS
        config.pixelFormat = kCVPixelFormatType_32BGRA
        config.queueDepth = 2

        let stream = SCStream(filter: filter, configuration: config, delegate: self)
        try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
        try await stream.startCapture()
        self.stream = stream
        Loggers.vision.info("screen stream started at \(config.width, privacy: .public)x\(config.height, privacy: .public) @ 2fps")
    }

    @MainActor
    public func stop() async {
        try? await stream?.stopCapture()
        stream = nil
    }

    /// Amendment T: synchronous on `queue`. NO MainActor hop.
    public func stream(_ stream: SCStream,
                        didOutputSampleBuffer sampleBuffer: CMSampleBuffer,
                        of type: SCStreamOutputType) {
        guard type == .screen,
              let image = SampleBufferEncoder.jpeg(from: sampleBuffer, quality: 0.8) else {
            return
        }
        let h = PerceptualHash.hash(image)
        if h == lastHash { return }
        lastHash = h
        slot.put(image)
    }

    public func stream(_ stream: SCStream, didStopWithError error: Error) {
        Loggers.vision.error("screen stream stopped: \(error.localizedDescription)")
    }
}

enum SampleBufferEncoder {
    static func jpeg(from sample: CMSampleBuffer, quality: CGFloat) -> Data? {
        guard let pixel = CMSampleBufferGetImageBuffer(sample) else { return nil }
        let ci = CIImage(cvPixelBuffer: pixel)
        let context = CIContext(options: nil)
        let cs = CGColorSpace(name: CGColorSpace.sRGB)!
        return context.jpegRepresentation(of: ci, colorSpace: cs, options: [
            kCGImageDestinationLossyCompressionQuality as CIImageRepresentationOption: quality
        ])
    }
}
