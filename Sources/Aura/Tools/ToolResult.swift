import Foundation

/// A typed result for a tool dispatch. Spec §8.1.
/// Tools never return raw strings — they return `ok` / `needsClarification` /
/// `failed`, with structured failure reasons so Gemini can reason about *why*
/// an action didn't take effect (vs. inventing a hallucinated error).
public struct ToolResult: Sendable, Equatable {
    public enum Status: String, Sendable, Equatable {
        case ok, needsClarification, failed
    }

    public let status: Status
    public let output: JSONValue?
    public let question: String?
    public let failureReason: ToolFailureReason?
    public let detail: String?
    public let confidence: Double?

    private init(status: Status,
                 output: JSONValue? = nil,
                 question: String? = nil,
                 failureReason: ToolFailureReason? = nil,
                 detail: String? = nil,
                 confidence: Double? = nil) {
        self.status = status
        self.output = output
        self.question = question
        self.failureReason = failureReason
        self.detail = detail
        self.confidence = confidence
    }

    public static func ok(output: JSONValue? = nil,
                          detail: String? = nil,
                          confidence: Double? = nil) -> ToolResult {
        ToolResult(status: .ok, output: output, detail: detail, confidence: confidence)
    }

    public static func needsClarification(_ question: String) -> ToolResult {
        ToolResult(status: .needsClarification, question: question)
    }

    public static func failed(reason: ToolFailureReason, detail: String? = nil) -> ToolResult {
        ToolResult(status: .failed, failureReason: reason, detail: detail)
    }

    /// Serializes to the shape Gemini's `toolResponse.functionResponses[].response`
    /// expects. Amendment AJ (IDPI defense): when the result *content* originates
    /// from a read tool over untrusted source (web pages, mail, clipboard, recall),
    /// the caller should prefix the textual content with the IDPI warning before
    /// invoking this — it's the tool's responsibility to mark its own outputs
    /// untrusted, not this serializer's.
    public func toGeminiResponse() -> JSONValue {
        var dict: [String: JSONValue] = ["status": .string(status.rawValue)]
        if let o = output { dict["output"] = o }
        if let q = question { dict["question"] = .string(q) }
        if let r = failureReason { dict["failure_reason"] = .string(r.code) }
        if let d = detail { dict["detail"] = .string(d) }
        if let c = confidence { dict["confidence"] = .number(c) }
        return .object(dict)
    }
}

/// Structured failure reasons for `ToolResult.failed`. Spec §8.1.
/// Open enum kept as a Swift `enum` with associated values where extra context
/// is essential (`preconditionFailed`, `toolError`); `code` flattens to a wire
/// string for Gemini.
public enum ToolFailureReason: Sendable, Equatable {
    case noStateChange
    case targetNotFound
    case timeout
    case permissionDenied
    case preconditionFailed(reason: String)
    case rotationForced
    case rateLimited
    case toolError(code: String)

    public var code: String {
        switch self {
        case .noStateChange:                return "noStateChange"
        case .targetNotFound:               return "targetNotFound"
        case .timeout:                      return "timeout"
        case .permissionDenied:             return "permissionDenied"
        case .preconditionFailed(let r):    return "preconditionFailed(\(r))"
        case .rotationForced:               return "rotationForced"
        case .rateLimited:                  return "rateLimited"
        case .toolError(let c):             return "toolError(\(c))"
        }
    }
}
