import Foundation
import SwiftUI

/// Central observable state for the Aura app.
/// Processes daemon events and drives all UI updates.
@Observable
@MainActor
final class AppState {
    // MARK: - Onboarding

    enum OnboardingStep: Equatable {
        case welcome       // API key setup (first run only)
        case permissions   // macOS permission grants
        case done          // normal app UI
    }

    var onboardingStep: OnboardingStep = .done
    var permissionChecker = PermissionChecker()

    // Legacy shim — kept so nothing else breaks
    var showOnboarding: Bool { onboardingStep != .done }

    // MARK: - Connection state

    enum ConnectionState {
        case disconnected
        case connecting
        case connected
    }

    var connectionState: ConnectionState = .disconnected

    // MARK: - Dot state

    var dotColor: DotColorName = .gray
    var isPulsing: Bool = false

    // MARK: - Conversation

    var messages: [ChatMessage] = []
    var isThinking: Bool = false

    // MARK: - Status

    var statusMessage: String = "Disconnected"

    // MARK: - Connection

    private var connection: DaemonConnection?

    private static let maxMessages = 200

    // MARK: - Init

    init() {
        let onboardingDone = UserDefaults.standard.bool(forKey: "aura.onboardingComplete")
        if onboardingDone && Self.configFileHasKey() {
            // Always verify permissions — TCC grants are invalidated when
            // a rebuild changes the binary's CDHash (ad-hoc signing).
            permissionChecker.checkAll()
            if permissionChecker.allGranted {
                onboardingStep = .done
            } else {
                onboardingStep = .permissions
            }
        } else if onboardingDone && !Self.configFileHasKey() {
            // Config was deleted (e.g. uninstall/reinstall) — restart onboarding
            UserDefaults.standard.removeObject(forKey: "aura.onboardingComplete")
            onboardingStep = .welcome
        } else if Self.configFileHasKey() {
            // Key already saved (e.g. re-install) — check if permissions are also granted
            permissionChecker.checkAll()
            if permissionChecker.allGranted {
                onboardingStep = .done
                UserDefaults.standard.set(true, forKey: "aura.onboardingComplete")
            } else {
                onboardingStep = .permissions
            }
        } else {
            onboardingStep = .welcome
        }
    }

    // MARK: - Onboarding

    func completeWelcome() {
        // If permissions were already granted via native macOS dialogs, skip straight to done
        permissionChecker.checkAll()
        if permissionChecker.allGranted {
            completeOnboarding()
        } else {
            onboardingStep = .permissions
        }
    }

    func completeOnboarding() {
        onboardingStep = .done
        UserDefaults.standard.set(true, forKey: "aura.onboardingComplete")
    }

    private static func configFileHasKey() -> Bool {
        let path = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/aura/config.toml")
        guard let contents = try? String(contentsOf: path, encoding: .utf8) else { return false }
        return contents.contains("api_key")
    }

    // MARK: - Setup

    func configure(connection: DaemonConnection) {
        self.connection = connection
        connectionState = .connecting
        statusMessage = "Connecting..."
        dotColor = .amber

        connection.onEvent = { [weak self] event in
            self?.handleEvent(event)
        }

        connection.onDisconnect = { [weak self] in
            self?.handleDisconnect()
        }

        connection.onReconnect = { [weak self] in
            self?.markConnected()
        }
    }

    func markConnected() {
        connectionState = .connected
        statusMessage = "Connected"
        dotColor = .green
    }

    // MARK: - Event handling

    func handleEvent(_ event: DaemonEvent) {
        switch event {
        case .dotColor(let update):
            withAnimation(.spring(response: 0.3, dampingFraction: 0.7)) {
                dotColor = update.color
                isPulsing = update.pulsing
            }

        case .transcript(let update):
            handleTranscript(update)

        case .toolStatus(let update):
            handleToolStatus(update)

        case .status(let update):
            statusMessage = update.message

        case .shutdown:
            statusMessage = "Daemon shutting down"
            dotColor = .gray
            isPulsing = false
        }
    }

    private func handleTranscript(_ update: TranscriptUpdate) {
        switch update.role {
        case .user:
            // User transcripts from voice — add as new message
            let message = ChatMessage(role: .user, text: update.text)
            withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                messages.append(message)
            }

        case .assistant:
            isThinking = false
            // Merge consecutive assistant transcripts (streaming)
            if let lastIndex = messages.indices.last,
               case .assistant = messages[lastIndex].role,
               !update.done {
                // Append to existing assistant message
                messages[lastIndex].text += update.text
            } else if !update.text.isEmpty {
                let message = ChatMessage(role: .assistant, text: update.text)
                withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                    messages.append(message)
                }
            }
        }

        trimMessages()
    }

    private func displayName(for toolName: String) -> String {
        switch toolName {
        case "run_applescript": return "Running script"
        case "get_screen_context": return "Reading screen"
        case "move_mouse": return "Moving mouse"
        case "click": return "Clicking"
        case "type_text": return "Typing"
        case "press_key": return "Pressing key"
        case "scroll": return "Scrolling"
        case "drag": return "Dragging"
        case "shutdown_aura": return "Shutting down"
        default: return toolName.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }

    private func handleToolStatus(_ update: ToolStatusUpdate) {
        let name = displayName(for: update.name)
        let displayText: String
        switch update.status {
        case .running:
            isThinking = true
            displayText = name
        case .completed:
            isThinking = false
            let output = update.output.map { "\n\($0)" } ?? ""
            displayText = "\(name)\(output)"
        case .failed:
            isThinking = false
            let output = update.output.map { "\n\($0)" } ?? ""
            displayText = "\(name)\(output)"
        }

        let message = ChatMessage(role: .tool(update.status), text: displayText)
        withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
            messages.append(message)
        }

        trimMessages()
    }

    private func handleDisconnect() {
        connectionState = .connecting  // auto-reconnect is in progress
        statusMessage = "Reconnecting..."
        dotColor = .amber
        isPulsing = true
    }

    private func trimMessages() {
        if messages.count > Self.maxMessages {
            messages.removeFirst(messages.count - Self.maxMessages)
        }
    }

    // MARK: - User actions

    func sendText(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }

        let message = ChatMessage(role: .user, text: trimmed)
        withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
            messages.append(message)
        }

        isThinking = true
        connection?.send(.sendText(trimmed))
        trimMessages()
    }

    func requestReconnect() {
        connectionState = .connecting
        statusMessage = "Reconnecting..."
        dotColor = .amber
        connection?.send(.reconnect)
    }

    func requestShutdown() {
        connection?.send(.shutdown)
    }
}
