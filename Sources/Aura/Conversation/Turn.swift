import Foundation

/// One turn in a conversation. Spec §5: "A conversation is a value: an ordered
/// sequence of `Turn`s." Each Turn is one of four content variants — what was
/// said by the user, what was said by the assistant, a tool invocation, or a
/// tool result.
///
/// `Turn` is a value: comparing two `Turn`s with `==` compares structure, not
/// identity. Turn order is established by `tsMono` (monotonic ns since boot)
/// rather than wall clock, matching the event log's ordering rule.
public struct Turn: Sendable, Equatable {
    public enum Content: Sendable, Equatable {
        case userText(String)
        case assistantText(String)
        case toolCall(name: String, args: JSONValue, callID: String)
        case toolResult(callID: String, output: JSONValue)
    }

    public let tsMono: UInt64
    public let content: Content

    public init(tsMono: UInt64, content: Content) {
        self.tsMono = tsMono
        self.content = content
    }
}
