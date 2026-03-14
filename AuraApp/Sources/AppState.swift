import Foundation
import SwiftUI

/// Central observable state for the Aura app.
/// Processes daemon events and drives all UI updates.
@Observable
@MainActor
final class AppState {
    // MARK: - Onboarding

    enum OnboardingStep: Equatable {
        case welcome
        case permissions
        case done
    }

    var onboardingStep: OnboardingStep = .done
    var permissionChecker = PermissionChecker()

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

    // MARK: - Activity Stream

    var events: [ActivityEvent] = []
    var isThinking: Bool = false

    // MARK: - Recent Sessions

    var recentSessions: [RecentSession] = []

    // MARK: - Status

    var statusMessage: String = "Disconnected"

    // MARK: - Connection

    private var connection: DaemonConnection?

    private static let maxEvents = 200

    /// Whether the last completed assistant turn should be followed by a separator.
    private var needsTurnSeparator = false

    // MARK: - Init

    init() {
        let onboardingDone = UserDefaults.standard.bool(forKey: "aura.onboardingComplete")
        if onboardingDone && Self.configFileHasKey() {
            permissionChecker.checkAll()
            if permissionChecker.allGranted {
                onboardingStep = .done
            } else {
                onboardingStep = .permissions
            }
        } else if onboardingDone && !Self.configFileHasKey() {
            UserDefaults.standard.removeObject(forKey: "aura.onboardingComplete")
            onboardingStep = .welcome
        } else if Self.configFileHasKey() {
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

        case .recentSessions(let update):
            withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                recentSessions = update.sessions
            }

        case .shutdown:
            statusMessage = "Daemon shutting down"
            dotColor = .gray
            isPulsing = false
        }
    }

    private func handleTranscript(_ update: TranscriptUpdate) {
        switch update.role {
        case .user:
            insertTurnSeparatorIfNeeded()
            let kind: EventKind = update.source == .text ? .userText : .userSpeech
            let event = ActivityEvent(kind: kind, text: update.text)
            withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                events.append(event)
            }

        case .assistant:
            isThinking = false
            // Merge consecutive assistant speech (streaming)
            if let lastIndex = events.indices.last,
               case .assistantSpeech = events[lastIndex].kind {
                events[lastIndex].text += update.text
            } else if !update.text.isEmpty {
                let event = ActivityEvent(kind: .assistantSpeech, text: update.text)
                withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                    events.append(event)
                }
            }

            if update.done {
                needsTurnSeparator = true
            }
        }

        trimEvents()
    }

    private func displayName(for toolName: String) -> String {
        switch toolName {
        case "run_applescript": return "Running script"
        case "get_screen_context": return "Reading screen"
        case "move_mouse": return "Moving mouse"
        case "click": return "Clicking"
        case "click_element": return "Clicking element"
        case "click_menu_item": return "Clicking menu"
        case "activate_app": return "Activating app"
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

        switch update.status {
        case .running:
            isThinking = true
            let event = ActivityEvent(kind: .toolCall(.running), text: name)
            withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                events.append(event)
            }

        case .completed, .failed:
            isThinking = false
            let resultLine = update.summary ?? update.output ?? ""
            let displayText = resultLine.isEmpty ? name : "\(name)\n\(resultLine)"

            // Find the most recent .running tool event and update it in-place
            if let idx = events.lastIndex(where: {
                if case .toolCall(.running) = $0.kind { return true }
                return false
            }) {
                withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                    events[idx] = ActivityEvent(kind: .toolCall(update.status), text: displayText)
                }
            } else {
                // Fallback: no .running event found — append
                let event = ActivityEvent(kind: .toolCall(update.status), text: displayText)
                withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                    events.append(event)
                }
            }
        }

        trimEvents()
    }

    private func insertTurnSeparatorIfNeeded() {
        guard needsTurnSeparator else { return }
        needsTurnSeparator = false
        let separator = ActivityEvent(kind: .turnSeparator, text: "")
        events.append(separator)
    }

    private func handleDisconnect() {
        connectionState = .disconnected
        statusMessage = "Connection lost"
        dotColor = .amber
        isPulsing = true
    }

    private func trimEvents() {
        if events.count > Self.maxEvents {
            events.removeFirst(events.count - Self.maxEvents)
        }
    }

    // MARK: - User actions

    func sendText(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }

        insertTurnSeparatorIfNeeded()
        let event = ActivityEvent(kind: .userText, text: trimmed)
        withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
            events.append(event)
        }

        isThinking = true
        connection?.send(.sendText(trimmed))
        trimEvents()
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
