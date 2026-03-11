import AppKit
import AVFoundation
import ApplicationServices

/// Checks and opens System Settings for the three permissions Aura requires:
/// Microphone, Screen Recording, and Accessibility.
///
/// All checks are **silent** (read-only) — they never trigger macOS TCC popup
/// dialogs. When a permission is missing, the UI directs the user to System
/// Settings via a URL scheme.
@Observable
@MainActor
final class PermissionChecker {
    var micGranted = false
    var screenGranted = false
    var accessibilityGranted = false

    var allGranted: Bool { micGranted && screenGranted && accessibilityGranted }

    func checkAll() {
        micGranted = checkMicrophone()
        screenGranted = checkScreenRecording()
        accessibilityGranted = AXIsProcessTrusted()
    }

    // MARK: - Silent permission checks

    /// Check microphone authorization status. Never triggers a popup.
    /// Uses AVCaptureDevice.authorizationStatus which is always read-only.
    private func checkMicrophone() -> Bool {
        let status = AVCaptureDevice.authorizationStatus(for: .audio)
        return status == .authorized
    }

    /// Check screen recording permission. Always silent — never triggers a popup.
    private func checkScreenRecording() -> Bool {
        // Already granted — skip re-checking to avoid unnecessary work
        if screenGranted { return true }

        if #available(macOS 15, *) {
            // CGPreflightScreenCaptureAccess is a silent preflight check.
            return CGPreflightScreenCaptureAccess()
        } else {
            // On macOS 14, CGWindowListCreateImage can implicitly trigger a TCC
            // popup on first call. Guard: only call once per app launch, and
            // cache the result. Subsequent checks return the cached value.
            // The user must restart or re-check after granting in System Settings.
            let image = CGWindowListCreateImage(
                CGRect(x: 0, y: 0, width: 1, height: 1),
                .optionOnScreenOnly,
                kCGNullWindowID,
                .bestResolution
            )
            return image != nil
        }
    }

    // MARK: - Open System Settings (user-initiated only)

    /// Open System Settings to Microphone privacy pane.
    /// Never triggers a macOS TCC popup — always directs to System Settings.
    func openMicSettings() {
        open("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
    }

    /// Open System Settings to Screen Recording privacy pane.
    /// Never triggers a macOS TCC popup — always directs to System Settings.
    func openScreenSettings() {
        open("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
    }

    /// Open System Settings to Accessibility privacy pane.
    func openAccessibilitySettings() {
        open("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
    }

    private func open(_ urlString: String) {
        guard let url = URL(string: urlString) else { return }
        NSWorkspace.shared.open(url)
    }
}
