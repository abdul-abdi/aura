import AppKit
import AVFoundation
import ApplicationServices

/// Checks and requests the three permissions Aura requires:
/// Microphone, Screen Recording, and Accessibility.
///
/// - Microphone: inline grant/deny prompt via AVCaptureDevice.
/// - Screen Recording (macOS 15+): CGRequestScreenCaptureAccess registers Aura
///   in System Settings and shows a one-time popup directing the user there.
/// - Accessibility: AXIsProcessTrustedWithOptions registers Aura in System
///   Settings and shows a one-time popup directing the user there.
/// - Polling (checkAll) is always silent and never triggers popups.
@Observable
@MainActor
final class PermissionChecker {
    var micGranted = false
    var screenGranted = false
    var accessibilityGranted = false

    var allGranted: Bool { micGranted && screenGranted && accessibilityGranted }

    func checkAll() {
        micGranted = AVCaptureDevice.authorizationStatus(for: .audio) == .authorized
        accessibilityGranted = Self.checkAccessibility()
        if #available(macOS 15, *) {
            screenGranted = CGPreflightScreenCaptureAccess()
        }
    }

    /// Check accessibility using AXIsProcessTrustedWithOptions (prompt: false).
    /// This is less aggressively cached than the bare AXIsProcessTrusted() on
    /// macOS Ventura+ and re-evaluates TCC state on each call.
    private static func checkAccessibility() -> Bool {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue(): false] as CFDictionary
        return AXIsProcessTrustedWithOptions(options)
    }

    // MARK: - Grant actions (called from "Grant" buttons)

    /// Microphone — triggers a real native inline prompt (grant/deny).
    func requestMicAccess() {
        AVCaptureDevice.requestAccess(for: .audio) { [weak self] granted in
            DispatchQueue.main.async {
                self?.micGranted = granted
            }
        }
    }

    /// Screen Recording — calls the native API which registers Aura in
    /// System Settings and shows a one-time popup directing the user there.
    func requestScreenAccess() {
        if #available(macOS 15, *) {
            CGRequestScreenCaptureAccess()
            screenGranted = CGPreflightScreenCaptureAccess()
        }
    }

    /// Accessibility — calls the native API which registers Aura in
    /// System Settings and shows a one-time popup directing the user there.
    func requestAccessibilityAccess() {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue(): true] as CFDictionary
        accessibilityGranted = AXIsProcessTrustedWithOptions(options)
    }
}
