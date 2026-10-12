import AppKit
import ScreenCaptureKit

/// Captures the display under the mouse pointer for Gemini to see.
///
/// Tool coordinates are normalized to that captured view: x and y run from
/// 0 to 1000, with (0, 0) at the top-left corner.
enum Screen {
    /// Global bounds (points, top-left origin) of the most recently captured display.
    private(set) static var bounds = CGDisplayBounds(CGMainDisplayID())

    static func capture(maxSide: CGFloat = 1280) async throws -> Data {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        let mouse = CGEvent(source: nil)?.location ?? .zero
        guard let display = content.displays.first(where: { CGDisplayBounds($0.displayID).contains(mouse) })
            ?? content.displays.first
        else { throw AuraError("No display to capture") }

        let rect = CGDisplayBounds(display.displayID)
        let ownApp = content.applications.filter { $0.processID == ProcessInfo.processInfo.processIdentifier }
        let filter = SCContentFilter(display: display, excludingApplications: ownApp, exceptingWindows: [])
        let config = SCStreamConfiguration()
        let scale = min(1, maxSide / max(rect.width, rect.height))
        config.width = Int(rect.width * scale)
        config.height = Int(rect.height * scale)
        config.showsCursor = true

        let image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
        guard let jpeg = NSBitmapImageRep(cgImage: image)
            .representation(using: .jpeg, properties: [.compressionFactor: 0.6])
        else { throw AuraError("Could not encode screenshot") }
        bounds = rect
        return jpeg
    }

    /// Normalized (0–1000) point on the captured view → global screen point.
    static func point(x: Double, y: Double, in rect: CGRect = bounds) -> CGPoint {
        CGPoint(x: rect.minX + rect.width * x / 1000, y: rect.minY + rect.height * y / 1000)
    }

    /// Global screen point → normalized (0–1000) point on the captured view.
    static func normalized(_ p: CGPoint, in rect: CGRect = bounds) -> (x: Int, y: Int) {
        (Int(((p.x - rect.minX) / rect.width * 1000).rounded()), Int(((p.y - rect.minY) / rect.height * 1000).rounded()))
    }
}
