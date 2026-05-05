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
    public static let correction            = "correction"
    public static let fact                  = "fact"
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
