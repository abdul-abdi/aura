import Foundation

/// An ordered sequence of `Turn`s. Spec §5: a conversation is a value; the
/// session is the WebSocket the next turn happens to be travelling on.
/// Rotation is "opening a new transport for the next turn." The conversation
/// persists in the event log; the transport is incidental.
///
/// `Conversation` is a thin value-type wrapper around `[Turn]`. The
/// authoritative store of conversation history is the event log; this type
/// is a derived view, typically materialized from a `query(kind: "turn.*")`
/// scan over the log.
public struct Conversation: Sendable, Equatable {
    public var turns: [Turn]

    public init(turns: [Turn] = []) {
        self.turns = turns
    }

    public mutating func append(_ turn: Turn) {
        turns.append(turn)
    }

    public var isEmpty: Bool { turns.isEmpty }
    public var count: Int { turns.count }
}
