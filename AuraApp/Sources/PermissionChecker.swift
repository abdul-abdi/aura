import AppKit
import AVFoundation
import ApplicationServices

/// Checks and requests the three permissions Aura requires:
/// Microphone, Screen Recording, and Accessibility.
///
/// Silent checks (polling) never trigger popups. The `request*` methods
/// are called when the user taps "Grant" — these trigger the native macOS
/// prompt, which is expected since the user explicitly initiated it.
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

    // MARK: - Silent permission checks (polling — never trigger popups)

    private func checkMicrophone() -> Bool {
        AVCaptureDevice.authorizationStatus(for: .audio) == .authorized
    }

    private func checkScreenRecording() -> Bool {
        if #available(macOS 15, *) {
            return CGPreflightScreenCaptureAccess()
        } else {
            return false
        }
    }

    // MARK: - User-initiated permission requests (called from "Grant" buttons)

    /// Triggers the native macOS microphone prompt.
    func requestMicAccess() {
        AVCaptureDevice.requestAccess(for: .audio) { [weak self] granted in
            DispatchQueue.main.async {
                self?.micGranted = granted
            }
        }
    }

    /// Triggers the native macOS Screen Recording prompt (macOS 15+)
    /// or opens System Settings on older versions.
    func requestScreenAccess() {
        if #available(macOS 15, *) {
            // CGRequestScreenCaptureAccess shows the native system prompt
            // and returns the current state. The 2s poll picks up changes.
            CGRequestScreenCaptureAccess()
            screenGranted = CGPreflightScreenCaptureAccess()
        } else {
            open("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        }
    }

    /// Triggers the native macOS Accessibility prompt.
    /// AXIsProcessTrustedWithOptions with the prompt key shows the system
    /// dialog asking to grant Accessibility access.
    func requestAccessibilityAccess() {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue(): true] as CFDictionary
        let trusted = AXIsProcessTrustedWithOptions(options)
        accessibilityGranted = trusted
    }

    // MARK: - Helpers

    private func open(_ urlString: String) {
        guard let url = URL(string: urlString) else { return }
        NSWorkspace.shared.open(url)
    }
}
