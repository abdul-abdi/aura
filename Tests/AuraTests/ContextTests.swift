import XCTest
@testable import Aura

final class ContextTests: XCTestCase {

    // MARK: - sessionStartContext

    func test_emptyInputsProducesStablePrefixOnly() {
        let inputs = Context.Inputs(mode: "default", now: Date())
        let result = Context.sessionStartContext(inputs: inputs)
        XCTAssertTrue(result.text.contains("You are Aura"))
        XCTAssertTrue(result.text.contains("## Mode: default"))
        XCTAssertTrue(result.droppedSections.isEmpty)
    }

    func test_userProfileInjectedAsAboutTheUserSection() {
        let facts = [
            Context.Fact(entity: "user", predicate: "lives_in", value: "Kenya", confidence: 1.0),
            Context.Fact(entity: "user", predicate: "fav_editor", value: "Helix", confidence: 0.8),
        ]
        let inputs = Context.Inputs(mode: "default", now: Date(), userProfile: facts)
        let result = Context.sessionStartContext(inputs: inputs)
        XCTAssertTrue(result.text.contains("About the user"))
        XCTAssertTrue(result.text.contains("user.lives_in = Kenya"))
        XCTAssertTrue(result.text.contains("user.fav_editor = Helix"))
        XCTAssertTrue(result.text.contains("conf 0.80"))
    }

    func test_activeGoalStackAppearsInPrompt() {
        let inputs = Context.Inputs(mode: "default", now: Date(),
                                    activeGoalStack: ["ship MVP", "fix audio loop"])
        let result = Context.sessionStartContext(inputs: inputs)
        XCTAssertTrue(result.text.contains("Active goal stack"))
        XCTAssertTrue(result.text.contains("1. ship MVP"))
        XCTAssertTrue(result.text.contains("2. fix audio loop"))
    }

    func test_pureDeterministicGivenInputs() {
        // Amendment BM: function is pure given its inputs.
        let inputs = Context.Inputs(mode: "default", now: Date(),
                                    userProfile: [Context.Fact(entity: "user", predicate: "p", value: "v", confidence: 1.0)])
        let a = Context.sessionStartContext(inputs: inputs)
        let b = Context.sessionStartContext(inputs: inputs)
        XCTAssertEqual(a.text, b.text)
        XCTAssertEqual(a.personaHash, b.personaHash)
    }

    func test_personaHashIsStableAcrossCalls() {
        let inputs = Context.Inputs(mode: "default", now: Date())
        let a = Context.sessionStartContext(inputs: inputs)
        let b = Context.sessionStartContext(inputs: inputs)
        XCTAssertEqual(a.personaHash, b.personaHash)
        XCTAssertEqual(a.personaHash, Persona.stablePrefixHash)
    }

    func test_overBudgetDropsSectionsInReverseOrder() {
        // The stable prefix alone is ~500 tokens. Set a tight budget that forces
        // the assembler to drop the dynamic suffix sections one by one.
        let bigFacts = (0..<20).map {
            Context.Fact(entity: "user", predicate: "p\($0)",
                         value: String(repeating: "x", count: 50), confidence: 1.0)
        }
        let stablePrefixTokens = Context.approxTokens(Persona.stablePrefix)
        let tightBudget = stablePrefixTokens + 30 // just enough for the header, nothing else

        let inputs = Context.Inputs(mode: "default", now: Date(),
                                    userProfile: bigFacts,
                                    activeGoalStack: ["g"],
                                    inferredSubgoal: "do the thing",
                                    pendingTool: Context.PendingTool(name: "remember", args: .null))
        let result = Context.sessionStartContext(inputs: inputs, tokenBudget: tightBudget)
        XCTAssertFalse(result.droppedSections.isEmpty,
                       "must have recorded which sections were dropped")
        // The first thing dropped should be the most-recently-appended section
        // (pendingTool, then inferredSubgoal, then activeGoalStack, then userProfile).
        XCTAssertEqual(result.droppedSections.first, "Pending tool")
    }

    // MARK: - rotationDigest

    func test_rotationDigest_minimal() {
        let inputs = Context.Inputs(mode: "default", now: Date())
        let s = Context.rotationDigest(inputs: inputs)
        XCTAssertTrue(s.contains("[Aura session continuation]"))
    }

    func test_rotationDigest_includesGoalStack_subgoal_pendingTool_facts() {
        let inputs = Context.Inputs(
            mode: "default",
            now: Date(),
            userProfile: [
                Context.Fact(entity: "user", predicate: "lives_in", value: "Kenya", confidence: 1.0)
            ],
            activeGoalStack: ["ship"],
            inferredSubgoal: "fix audio loop",
            pendingTool: Context.PendingTool(name: "remember",
                                             args: .object(["entity": .string("user")]),
                                             geminiCallID: "call-1")
        )
        let s = Context.rotationDigest(inputs: inputs)
        XCTAssertTrue(s.contains("ship"))
        XCTAssertTrue(s.contains("fix audio loop"))
        XCTAssertTrue(s.contains("remember"))
        XCTAssertTrue(s.contains("user.lives_in=Kenya"))
    }

    // MARK: - Lenses.userProfile

    func test_lensesUserProfile_returnsMostRecentPerPredicate() async throws {
        let dbPath = "/tmp/aura_lens_\(UUID().uuidString).sqlite"
        let log = try Log.open(path: dbPath)
        defer {
            try? FileManager.default.removeItem(atPath: dbPath)
            try? FileManager.default.removeItem(atPath: dbPath + "-wal")
            try? FileManager.default.removeItem(atPath: dbPath + "-shm")
        }

        // Two values for the same predicate; the later one wins.
        await log.append(kind: EventKind.fact, payload: .object([
            "entity": .string("user"), "predicate": .string("fav_color"),
            "value": .string("blue"), "confidence": .number(1.0)
        ]))
        try await log.flush()
        try await Task.sleep(nanoseconds: 1_000_000)
        await log.append(kind: EventKind.fact, payload: .object([
            "entity": .string("user"), "predicate": .string("fav_color"),
            "value": .string("green"), "confidence": .number(1.0)
        ]))
        // Non-user fact must be filtered out.
        await log.append(kind: EventKind.fact, payload: .object([
            "entity": .string("project:aura"), "predicate": .string("status"),
            "value": .string("MVP"), "confidence": .number(1.0)
        ]))
        try await log.flush()

        let profile = try await Lenses.userProfile(log: log)
        XCTAssertEqual(profile.count, 1)
        XCTAssertEqual(profile.first?.value, "green",
                       "later fact should override earlier one for same predicate")

        await log.shutdown()
    }
}
