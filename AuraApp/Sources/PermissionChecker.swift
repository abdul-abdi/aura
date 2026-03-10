import AppKit
import AVFoundation
import ApplicationServices

/// Checks and opens System Settings for the three permissions Aura requires:
/// Microphone, Screen Recording, and Accessibility.
@Observable
@MainActor
final class PermissionChecker {
    var micGranted = false
    var screenGranted = false
    var accessibilityGranted = false

    var allGranted: Bool { micGranted && screenGranted && accessibilityGranted }

    func checkAll() {
        micGranted = AVCaptureDevice.authorizationStatus(for: .audio) == .authorized
        screenGranted = checkScreenRecording()
        accessibilityGranted = AXIsProcessTrusted()
    }

    private func checkScreenRecording() -> Bool {
        if #available(macOS 15, *) {
            return CGPreflightScreenCaptureAccess()
        } else {
            // On macOS 14, attempt to capture a 1×1 pixel region.
            // If Screen Recording is denied the result is a solid black image
            // rather than nil, so we use CGWindowListCreateImage instead — a nil
            // return reliably indicates denial on 14.
            let image = CGWindowListCreateImage(
                CGRect(x: 0, y: 0, width: 1, height: 1),
                .optionOnScreenOnly,
                kCGNullWindowID,
                .bestResolution
            )
            return image != nil
        }
    }

    // MARK: - Open System Settings

    func openMicSettings() {
        open("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
    }

    func openScreenSettings() {
        open("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
    }

    func openAccessibilitySettings() {
        open("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
    }

    private func open(_ urlString: String) {
        guard let url = URL(string: urlString) else { return }
        NSWorkspace.shared.open(url)
    }
}
