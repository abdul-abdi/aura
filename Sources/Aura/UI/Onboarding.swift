import SwiftUI
import AppKit
import AVFoundation
import ScreenCaptureKit

public enum OnboardingStep: String, Sendable, CaseIterable {
    case welcome, apiKey, mic, screen, accessibility, sayHi, done
}

@MainActor
public final class OnboardingState: ObservableObject {
    @Published public var step: OnboardingStep = .welcome
    @Published public var apiKeyInput: String = ""

    private let keychainService: String

    public init(keychainService: String = Config.keychainService) {
        self.keychainService = keychainService
    }

    public func advance() {
        let all = OnboardingStep.allCases
        guard let i = all.firstIndex(of: step), i + 1 < all.count else { return }
        step = all[i + 1]
    }

    public func commitAPIKey() {
        let trimmed = apiKeyInput.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return }
        try? Keychain.set(trimmed, service: keychainService, account: "api_key")
        apiKeyInput = ""
    }
}

public struct OnboardingView: View {
    @StateObject private var state = OnboardingState()
    public var onComplete: (() -> Void)?

    public init(onComplete: (() -> Void)? = nil) {
        self.onComplete = onComplete
    }

    public var body: some View {
        VStack(spacing: 24) {
            switch state.step {
            case .welcome:       WelcomeStep(state: state)
            case .apiKey:        APIKeyStep(state: state)
            case .mic:           MicPermissionStep(state: state)
            case .screen:        ScreenPermissionStep(state: state)
            case .accessibility: AccessibilityStep(state: state)
            case .sayHi:         SayHiStep(state: state)
            case .done:
                VStack(spacing: 16) {
                    Text("All set.").font(.title)
                    Button("Launch Aura") {
                        onComplete?()
                    }
                    .keyboardShortcut(.defaultAction)
                }
            }
        }
        .padding(40)
        .frame(width: 520, height: 360)
    }
}

private struct WelcomeStep: View {
    @ObservedObject var state: OnboardingState
    var body: some View {
        VStack(spacing: 16) {
            Text("Aura").font(.system(size: 48, weight: .light))
            Text("Local-only voice assistant for macOS.").foregroundStyle(.secondary)
            Button("Continue") { state.advance() }.keyboardShortcut(.defaultAction)
        }
    }
}

private struct APIKeyStep: View {
    @ObservedObject var state: OnboardingState
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Gemini API key").font(.headline)
            Text("Stored in your login Keychain. Never written to disk.")
                .foregroundStyle(.secondary).font(.callout)
            SecureField("AIza…", text: $state.apiKeyInput)
                .textFieldStyle(.roundedBorder)
            HStack {
                Spacer()
                Button("Continue") {
                    state.commitAPIKey()
                    state.advance()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(state.apiKeyInput.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
    }
}

private struct MicPermissionStep: View {
    @ObservedObject var state: OnboardingState
    var body: some View {
        VStack(spacing: 12) {
            Text("Microphone permission").font(.headline)
            Text("Aura needs to hear you. macOS will ask once.")
                .foregroundStyle(.secondary)
            Button("Request access") {
                AVCaptureDevice.requestAccess(for: .audio) { _ in
                    DispatchQueue.main.async { state.advance() }
                }
            }
        }
    }
}

private struct ScreenPermissionStep: View {
    @ObservedObject var state: OnboardingState
    var body: some View {
        VStack(spacing: 12) {
            Text("Screen recording permission").font(.headline)
            Text("Aura sees your screen so Gemini can help.")
                .foregroundStyle(.secondary)
            Button("Request access") {
                Task { @MainActor in
                    await ScreenPermissionStep.requestAccess()
                    state.advance()
                }
            }
            Button("Open System Settings (manual)") {
                if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture") {
                    NSWorkspace.shared.open(url)
                }
            }
            .buttonStyle(.link)
        }
    }

    /// Amendment K: trigger the modern screen-recording permission prompt via
    /// SCShareableContent.current. On macOS 15+, the system surfaces a picker.
    /// On 14, the standard TCC prompt appears. If the call throws (user denied
    /// or pre-permission state), the user can fall back to the settings link.
    @MainActor
    static func requestAccess() async {
        do {
            _ = try await SCShareableContent.current
        } catch {
            // Permission denied or transient — user can use the manual link.
        }
    }
}

private struct AccessibilityStep: View {
    @ObservedObject var state: OnboardingState
    var body: some View {
        VStack(spacing: 12) {
            Text("Accessibility permission").font(.headline)
            Text("Aura uses Accessibility to read UI elements and synthesize clicks/keys.")
                .foregroundStyle(.secondary)
            Button("Open System Settings") {
                if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
                    NSWorkspace.shared.open(url)
                }
            }
            Button("I granted it — continue") { state.advance() }.keyboardShortcut(.defaultAction)
        }
    }
}

private struct SayHiStep: View {
    @ObservedObject var state: OnboardingState
    var body: some View {
        VStack(spacing: 12) {
            Text("Say hi to Aura").font(.headline)
            Text("Press the menu-bar dot, then speak.").foregroundStyle(.secondary)
            Button("Done") { state.advance() }.keyboardShortcut(.defaultAction)
        }
    }
}
