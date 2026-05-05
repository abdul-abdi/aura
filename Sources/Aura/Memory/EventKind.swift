import Foundation

/// Event kinds are namespaced strings, NOT a closed enum. The constants below
/// are conventional seed names; new kinds can be added by writing them.
public enum EventKind {
    public static let turnUser              = "turn.user"
    public static let turnAssistant         = "turn.assistant"
    public static let turnToolCall          = "turn.tool_call"
    public static let turnToolResult        = "turn.tool_result"
    public static let stateObservation      = "state.observation"
    public static let statePermissionLost   = "state.permission_lost"
    public static let stateWake             = "state.wake"

    // §6.1 Hickey-pass split — was the single mutable `correction` kind. The
    // immutable triplet (proposed / confirmed / revoked) lets the lens query
    // the active rule set as a pure JOIN over append-only events.
    public static let correctionProposed    = "correction.proposed"
    public static let correctionConfirmed   = "correction.confirmed"
    public static let correctionRevoked     = "correction.revoked"
    /// Deprecated singular form. Kept so Plan 1 history doesn't break; new
    /// callers must use `correctionProposed` / `correctionConfirmed` /
    /// `correctionRevoked`. Plan 3's behavioral lens reads only the split form.
    @available(*, deprecated, renamed: "correctionProposed", message: "Use correction.proposed/confirmed/revoked — spec §6.1 Hickey-pass.")
    public static let correction            = "correction"

    public static let fact                  = "fact"
    /// Resolution of a `pattern.contradiction` — user picked one fact, dismissed
    /// the other. Spec §6.1: forcing this through `correction` was type confusion;
    /// adjudication is "which fact is true," not "what behavior to change."
    public static let factAdjudicated       = "fact.adjudicated"
    public static let goalActive            = "goal.active"
    public static let goalCleared           = "goal.cleared"
    public static let goalSubgoalInferred   = "goal.subgoal_inferred"
    public static let triggerFired          = "trigger.fired"
    public static let sessionStart          = "session.start"
    public static let sessionEnd            = "session.end"
    public static let summaryRolling        = "summary.rolling"
    public static let contextAssembled      = "context.assembled"
    public static let contextTrimmed        = "context.trimmed"
    public static let effectPosted          = "effect.posted"
    public static let effectOutcome         = "effect.outcome"
    /// Spec §9.7 — appended when a Policy supervisor's mutating-call ceiling
    /// (≤30 calls/min by default) trips and the next call is gated.
    public static let effectRateLimited     = "effect.rate_limited"
    /// Spec §6.1 (Hickey-pass review) — Policy supervisor caught an exception.
    /// `lastError` lives as an event, not as actor cache state.
    public static let policyFailed          = "policy.failed"
    public static let patternSemantic       = "pattern.semantic"
    public static let patternProcedural     = "pattern.procedural"
    public static let patternContradiction  = "pattern.contradiction"
    public static let playbookDemonstrated  = "playbook.demonstrated"
    public static let targetingObserved     = "targeting.observed"
    public static let configChange          = "config.change"
    public static let tombstone             = "tombstone"

    /// Open schema validation: kind is non-empty and contains no whitespace.
    public static func isWellFormed(_ kind: String) -> Bool {
        guard !kind.isEmpty else { return false }
        return !kind.contains(where: { $0.isWhitespace })
    }
}
