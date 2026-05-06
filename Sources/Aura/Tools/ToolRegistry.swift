import Foundation

public typealias ToolImpl = @Sendable (JSONValue) async -> ToolResult
public typealias MutatingToolImpl = @Sendable (JSONValue, EffectLog, String?) async -> ToolResult

/// The single dispatch point for all 27 (Plan 2 MVP: 4) tools. Spec §8 + §9.
///
/// Read-only tools register via `register`; mutating tools register via
/// `registerMutating` and the registry wraps the body in
/// `EffectLog.executeWithEffect` (amendment AG). Drift impossible by
/// construction — no other site in the codebase calls `executeWithEffect`.
///
/// Concurrency is bounded by an `AsyncSemaphore` (default 8). Spec §9: tool
/// dispatches must not pile up unbounded — the runtime is interactive, not a
/// queue.
public actor ToolRegistry {
    private var readOnlyImpls: [String: ToolImpl] = [:]
    private var mutatingImpls: [String: MutatingToolImpl] = [:]
    /// `intentBuilder` produces the EffectIntent (operation + args) the
    /// EffectLog will record before running the body. Kept separate from the
    /// impl so the body itself doesn't reach into mutable EffectLog state.
    private var mutatingIntents: [String: @Sendable (JSONValue) -> EffectIntent] = [:]
    private let semaphore: AsyncSemaphore
    private let effects: EffectLog

    public init(effects: EffectLog, maxConcurrent: Int = 8) {
        self.effects = effects
        self.semaphore = AsyncSemaphore(value: maxConcurrent)
    }

    public func register(_ name: String, _ impl: @escaping ToolImpl) {
        readOnlyImpls[name] = impl
    }

    public func registerMutating(_ name: String,
                                 intent: @escaping @Sendable (JSONValue) -> EffectIntent,
                                 _ impl: @escaping MutatingToolImpl) {
        mutatingIntents[name] = intent
        mutatingImpls[name] = impl
    }

    public func registered() -> [String] {
        Array(Set(readOnlyImpls.keys).union(mutatingImpls.keys)).sorted()
    }

    /// Dispatch a Gemini-issued tool call. Returns a `ToolResult` ready for
    /// `session.send(.toolResult(...))`. The `geminiCallID` is threaded through
    /// to the EffectLog (amendment AH) so duplicate cross-rotation dispatches
    /// are detectable in the log.
    public func dispatch(name: String,
                         args: JSONValue,
                         geminiCallID: String? = nil) async -> ToolResult {
        await semaphore.wait()
        defer { Task { await semaphore.signal() } }

        if let impl = readOnlyImpls[name] {
            return await impl(args)
        }

        if let impl = mutatingImpls[name], let intentBuilder = mutatingIntents[name] {
            let intent = intentBuilder(args)
            // EffectLog.executeWithEffect is throwing; ToolResult is value.
            // We never throw out of `dispatch` — we return `.failed` instead.
            do {
                return try await effects.executeWithEffect(
                    intent: intent,
                    geminiCallID: geminiCallID
                ) { @Sendable [effects] in
                    await impl(args, effects, geminiCallID)
                }
            } catch {
                return .failed(reason: .toolError(code: "effect_log"),
                               detail: String(describing: error))
            }
        }

        return .failed(reason: .preconditionFailed(reason: "unknown_tool"),
                       detail: "tool '\(name)' is not registered")
    }
}

/// A simple async counting semaphore. Used to cap tool-dispatch concurrency.
/// Not a fairness guarantee — FIFO on the contention path is good enough.
public actor AsyncSemaphore {
    private var value: Int
    private var waiters: [CheckedContinuation<Void, Never>] = []

    public init(value: Int) {
        precondition(value >= 0, "semaphore value must be non-negative")
        self.value = value
    }

    public func wait() async {
        if value > 0 { value -= 1; return }
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            waiters.append(c)
        }
    }

    public func signal() {
        if !waiters.isEmpty {
            waiters.removeFirst().resume()
        } else {
            value += 1
        }
    }
}
