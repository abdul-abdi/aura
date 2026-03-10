import Foundation
import SwiftUI

/// Central observable state for the Aura app.
/// Processes daemon events and drives all UI updates.
@Observable
@MainActor
final class AppState {
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

    // MARK: - Status

    var statusMessage: String = "Disconnected"

    // MARK: - Connection

    private var connection: DaemonConnection?

    private static let maxMessages = 200

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

    private func handleToolStatus(_ update: ToolStatusUpdate) {
        let displayText: String
        switch update.status {
        case .running:
            displayText = "\(update.name)"
        case .completed:
            let output = update.output.map { "\n\($0)" } ?? ""
            displayText = "\(update.name)\(output)"
        case .failed:
            let output = update.output.map { "\n\($0)" } ?? ""
            displayText = "\(update.name)\(output)"
        }

        let message = ChatMessage(role: .tool(update.status), text: displayText)
        withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
            messages.append(message)
        }

        trimMessages()
    }

    private func handleDisconnect() {
        connectionState = .disconnected
        statusMessage = "Disconnected"
        dotColor = .gray
        isPulsing = false
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
