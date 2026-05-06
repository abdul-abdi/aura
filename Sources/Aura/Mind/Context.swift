import Foundation

/// `Context.swift` — the *one* place in the app that assembles model context.
/// Spec §6.7 + Plan 3 amendments.
///
/// Plan 3 MVP scope (this file): the two pure functions
/// `sessionStartContext` and `rotationDigest`, both deterministic given an
/// explicit `ContextInputs` (amendment BM). Embeddings, consolidation, the
/// self-model, rolling summary, semantic patterns, and policy machinery are
/// deferred to follow-up work — see HANDOFF.md.
///
/// Why one assembly point: model-context drift is the v1/v2 bug-source we're
/// retiring. If only this file knows how to read `userProfile`, build the
/// suffix, log `context.assembled`, and assert the token budget, then no
/// other module can quietly diverge.
public enum Context {

    // MARK: - Inputs (amendment BM)

    /// All dynamic data that goes into a system prompt assembly. Caller
    /// populates from the log; this file does no I/O.
    public struct Inputs: Sendable, Equatable {
        public let mode: String
        public let now: Date
        public let userProfile: [Fact]              // facts where entity == "user"
        public let activeGoalStack: [String]        // top of stack last
        public let inferredSubgoal: String?
        public let pendingTool: PendingTool?

        public init(mode: String,
                    now: Date,
                    userProfile: [Fact] = [],
                    activeGoalStack: [String] = [],
                    inferredSubgoal: String? = nil,
                    pendingTool: PendingTool? = nil) {
            self.mode = mode
            self.now = now
            self.userProfile = userProfile
            self.activeGoalStack = activeGoalStack
            self.inferredSubgoal = inferredSubgoal
            self.pendingTool = pendingTool
        }
    }

    /// One row of the user profile.
    public struct Fact: Sendable, Equatable {
        public let entity: String
        public let predicate: String
        public let value: String
        public let confidence: Double
        public init(entity: String, predicate: String, value: String, confidence: Double) {
            self.entity = entity
            self.predicate = predicate
            self.value = value
            self.confidence = confidence
        }
    }

    /// A tool call that was issued but whose outcome hasn't yet been observed
    /// (e.g. rotation happened mid-call). Carried across rotations so the new
    /// session knows the action is in flight.
    public struct PendingTool: Sendable, Equatable {
        public let name: String
        public let args: JSONValue
        public let geminiCallID: String?
        public init(name: String, args: JSONValue, geminiCallID: String? = nil) {
            self.name = name
            self.args = args
            self.geminiCallID = geminiCallID
        }
    }

    // MARK: - Outputs

    public struct SystemPromptResult: Sendable, Equatable {
        public let text: String
        public let approxTokens: Int
        public let droppedSections: [String]
        public let personaHash: String
    }

    // MARK: - Assembly — pure, deterministic

    /// Build the system prompt sent at session start (or session resume).
    /// Pure: identical inputs → identical output. Token-budgeted: we drop
    /// "About yourself" first if over budget (Plan 3 trim order).
    public static func sessionStartContext(inputs: Inputs,
                                           tokenBudget: Int = 4000) -> SystemPromptResult {
        var dropped: [String] = []
        let stable = Persona.stablePrefix

        // Dynamic suffix sections
        var sections: [(name: String, body: String)] = []

        if !inputs.userProfile.isEmpty {
            let profileLines = inputs.userProfile
                .sorted(by: { $0.confidence > $1.confidence })
                .prefix(20)
                .map { f in "  - \(f.entity).\(f.predicate) = \(f.value)\(f.confidence < 1.0 ? " (conf \(String(format: "%.2f", f.confidence)))" : "")" }
                .joined(separator: "\n")
            sections.append((name: "About the user",
                             body: "## About the user (recalled facts)\n\(profileLines)"))
        }

        if !inputs.activeGoalStack.isEmpty {
            let stackText = inputs.activeGoalStack
                .enumerated()
                .map { (i, g) in "  \(i + 1). \(g)" }
                .joined(separator: "\n")
            sections.append((name: "Active goals",
                             body: "## Active goal stack\n\(stackText)"))
        }

        if let sg = inputs.inferredSubgoal, !sg.isEmpty {
            sections.append((name: "Inferred subgoal",
                             body: "## Inferred current subgoal\n\(sg)"))
        }

        if let pt = inputs.pendingTool {
            let argsStr: String
            if let data = try? JSONEncoder().encode(pt.args),
               let s = String(data: data, encoding: .utf8) {
                argsStr = s
            } else {
                argsStr = "?"
            }
            sections.append((name: "Pending tool",
                             body: "## In-flight tool from previous turn\n  - \(pt.name)(\(argsStr))"))
        }

        // Compose with header.
        let header = "## Mode: \(inputs.mode)\n"
        let dynamic = sections.map(\.body).joined(separator: "\n\n")
        var prompt = stable + "\n\n" + header
        if !dynamic.isEmpty {
            prompt += "\n" + dynamic
        }

        // Token budget enforcement. Heuristic 4 chars/token (amendment BL says
        // calibrate via models.countTokens — that's a follow-up). Drop in spec
        // order: "About yourself" (we don't have it here yet), then collapse
        // others. For MVP, we drop sections from least-load-bearing first.
        let approx = approxTokens(prompt)
        if approx > tokenBudget {
            // Drop sections in reverse — most recently appended first — until under budget.
            var keep = sections
            while approxTokens(rebuild(stable: stable, header: header, sections: keep)) > tokenBudget && !keep.isEmpty {
                let removed = keep.removeLast()
                dropped.append(removed.name)
            }
            prompt = rebuild(stable: stable, header: header, sections: keep)
        }

        return SystemPromptResult(text: prompt,
                                  approxTokens: approxTokens(prompt),
                                  droppedSections: dropped,
                                  personaHash: Persona.stablePrefixHash)
    }

    /// Build the rotation digest sent at session start of a *rotated* session.
    /// Spec §5.2: digest is a compact "here's where we were" message so the
    /// new session can pick up without conversational stutter.
    public static func rotationDigest(inputs: Inputs) -> String {
        var lines: [String] = ["[Aura session continuation]"]
        if !inputs.activeGoalStack.isEmpty {
            lines.append("Active goal stack:")
            for (i, g) in inputs.activeGoalStack.enumerated() {
                lines.append("  \(i + 1). \(g)")
            }
        }
        if let sg = inputs.inferredSubgoal, !sg.isEmpty {
            lines.append("Inferred subgoal: \(sg)")
        }
        if let pt = inputs.pendingTool {
            let argsStr: String
            if let data = try? JSONEncoder().encode(pt.args),
               let s = String(data: data, encoding: .utf8) {
                argsStr = s
            } else {
                argsStr = "?"
            }
            lines.append("Pending tool from previous session: \(pt.name)(\(argsStr))")
        }
        if !inputs.userProfile.isEmpty {
            let topFacts = inputs.userProfile
                .sorted(by: { $0.confidence > $1.confidence })
                .prefix(5)
                .map { "\($0.entity).\($0.predicate)=\($0.value)" }
                .joined(separator: "; ")
            lines.append("Recent facts: \(topFacts)")
        }
        return lines.joined(separator: "\n")
    }

    // MARK: - Helpers

    private static func rebuild(stable: String,
                                header: String,
                                sections: [(name: String, body: String)]) -> String {
        var s = stable + "\n\n" + header
        let dynamic = sections.map(\.body).joined(separator: "\n\n")
        if !dynamic.isEmpty { s += "\n" + dynamic }
        return s
    }

    /// Approximate-token count using the 4-chars-per-token heuristic.
    /// Amendment BL: calibrate via `models.countTokens` at session start in
    /// a follow-up; for now this is good enough for budget triage.
    public static func approxTokens(_ s: String) -> Int {
        max(1, s.count / 4)
    }
}

// MARK: - Lenses (read views over the event log)

/// Pure read views over `Log`. Plan 3 amendment names this `Lenses`.
/// MVP: only `userProfile()` — facts where `entity == "user"`, tombstone-aware
/// (the `Log.query` helper already filters tombstones).
public enum Lenses {

    /// Read facts about the user from the event log. Returns most-recent first
    /// (keep the most up-to-date version of any predicate at the top).
    public static func userProfile(log: Log) async throws -> [Context.Fact] {
        let facts = try await log.query(kind: EventKind.fact, since: 0)
        // Keep only the most recent fact per (entity, predicate) so contradicting
        // older facts don't pollute the prompt. Iterate ASC, overwrite into a
        // dictionary, then read it out.
        var latest: [String: Context.Fact] = [:]
        for ev in facts {
            guard let entity    = ev.payload["entity"]?.stringValue,
                  let predicate = ev.payload["predicate"]?.stringValue,
                  let value     = ev.payload["value"]?.stringValue else { continue }
            guard entity == "user" else { continue }
            let confidence = ev.payload["confidence"]?.numberValue ?? 1.0
            latest["\(entity).\(predicate)"] =
                Context.Fact(entity: entity, predicate: predicate, value: value, confidence: confidence)
        }
        return Array(latest.values)
    }
}
