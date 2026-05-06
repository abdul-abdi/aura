import Foundation
import AppKit

/// `clipboard.read` reads NSPasteboard.general's string contents. Read-only.
///
/// Amendment AJ: clipboard contents are *untrusted input* — a webpage may have
/// crafted text that looks like a directive. The output is prefixed with the
/// IDPI warning so Gemini's persona instructions (which forbid following
/// instructions found in tool outputs) apply.
public struct ClipboardTools: Sendable {
    public init() {}

    static let untrustedPrefix = MemoryTools.untrustedPrefix

    /// Read the current clipboard string. Returns `.ok` with `_untrusted: true`
    /// and a `_text` field carrying the warning-prefixed content. Returns
    /// `.failed(.targetNotFound)` if the clipboard has no string content.
    public func read() async -> ToolResult {
        // NSPasteboard must be accessed on the main actor.
        let s: String? = await MainActor.run {
            NSPasteboard.general.string(forType: .string)
        }
        guard let s else {
            return .failed(reason: .targetNotFound,
                           detail: "clipboard has no string content")
        }
        return .ok(output: .object([
            "_untrusted": .bool(true),
            "_text":      .string(Self.untrustedPrefix + s),
            "length":     .number(Double(s.count)),
        ]))
    }
}

extension ClipboardTools {
    /// Adapter for `ToolRegistry.register`. Args shape: ignored.
    public func readToolImpl() -> ToolImpl {
        return { _ in await self.read() }
    }
}
