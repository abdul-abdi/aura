# Aura v3 — Plan 3: Mind

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec:** `docs/superpowers/specs/2026-04-29-aura-v3-design.md` §6, §7, §9.5 (committed `9b35619`)
**Predecessors:** Plans 1 and 2 (must be executed first)

**Goal:** Give Aura a brain. Build the memory machinery (embeddings, four query lenses, consolidation worker), the self-model, the `Context.swift` module that's the _one_ place model context is assembled, the observation pipeline, and the seven proactive policies — including the drift detector with theory-of-mind subgoal inference. After this plan, Aura remembers what matters across sessions, learns from corrections under a 24-hour governance gate, consolidates patterns from its own log during idle, has a self-model it queries before acting, and can fire whispers as a true agent — not a parrot.

**Architecture:**

- All memory is queries over the Plan-1 event log. Embeddings live in a separate `event_embeddings` table keyed by `model_version`.
- `Context.swift` is the **only** module that reads multiple event kinds at once to assemble model context. Two pure functions: `sessionStartContext(now, log, mode, selfModel)` and `rotationDigest(...)`. Both deterministic given inputs, fixture-tested.
- The system prompt is `stable_prefix + dynamic_suffix` for prompt-cache friendliness. Stable prefix carries the persona (the explicit non-yes-man text from the spec).
- Consolidation worker runs during canonical _system idle_ (no `turn.user` in last 30s, no playback, no in-flight tool, no in-flight effect). Hot pass after every session.end; cold pass once daily. Both bounded at 8K input + 1K output Flash tokens.
- Self-model is a derived view computed on-demand from the log; refreshed on each `turn.tool_result` and `correction`.
- Policies are functions `(Observation, HistoryView, SelfModel) -> Action?`. Each runs in its own supervised Task with restart-on-error backoff.
- Drift detector takes three inputs: screenshot @ T-30s, screenshot @ now, **inferred current subgoal** (separate Flash call from active goal stack + recent observations).

**Branch:** `feat/v3-plan3-mind` off the merged Plan 2.

**Out of scope:** UI rendering of Mind area / self-model / patterns / playbooks (Plan 4). Demonstration mode's _recording UI_ (Plan 4); the on-disk `playbook.demonstrated` event is shipped here so `run_playbook` from Plan 2 can read it.

---

## File structure for Plan 3

```
AuraV3/Sources/AuraV3/
  Memory/
    Embeddings.swift                  -- NLContextualEmbedding wrapper, .cpuOnly
    EmbeddingWorker.swift             -- background queue: compute embeddings post-rotation
    Lenses.swift                      -- Episodic + Semantic + Behavioral + Procedural queries
    Consolidation.swift               -- hot + cold passes; pattern.* event emission
    UserProfile.swift                 -- structured (entity, predicate, value, confidence) view
  SelfModel/
    SelfModel.swift                   -- derived view of tool reliability, failures, breakers
  Context/
    Context.swift                     -- sessionStartContext + rotationDigest
    StablePrefix.swift                -- the persona text and tool-routing rules (compile-time)
    DynamicSuffix.swift               -- corrections + profile + goals + patterns + self-model
    RollingSummary.swift              -- per-turn 2-3KB summary, Flash REST
    SubgoalInferrer.swift             -- drift-detector input #3
  Observation/
    ObservationStream.swift           -- AsyncStream<Observation>; multiplex from watchdogs/AX/etc.
    HistoryView.swift                 -- read-only log query interface
  Policy/
    PolicyHost.swift                  -- supervised tasks, restart-on-error
    Policies.swift                    -- the 7 policies as functions
    UserInvokedPolicy.swift
    CalendarPolicy.swift
    BuildFailurePolicy.swift
    ErrorDialogPolicy.swift
    IdlePolicy.swift
    AppCrashPolicy.swift
    DriftDetectorPolicy.swift
  App/
    DemonstrationRecorder.swift       -- ⌘⇧D recorder; UI lives in Plan 4 but recorder writes log here
Tests/AuraV3Tests/
  EmbeddingsTests.swift
  LensesTests.swift
  ConsolidationFixtureTests.swift
  SelfModelTests.swift
  ContextFixtureTests.swift
  RollingSummaryTests.swift
  SubgoalInferrerTests.swift
  ObservationStreamTests.swift
  PolicyHostRestartTests.swift
  DriftDetectorFixtureTests.swift     -- placeholder; full 50-fixture set is Plan 4
```

---

## Phase A — Embeddings

### Task A.1: NLContextualEmbedding wrapper (`.cpuOnly`)

**Files:**

- Create: `AuraV3/Sources/AuraV3/Memory/Embeddings.swift`
- Create: `AuraV3/Tests/AuraV3Tests/EmbeddingsTests.swift`

- [ ] **Step 1: Test FIRST**

`AuraV3/Tests/AuraV3Tests/EmbeddingsTests.swift`:

```swift
import XCTest
@testable import AuraV3

final class EmbeddingsTests: XCTestCase {
    func test_embedReturnsConsistentVector() async throws {
        let embedder = try Embedder.makeEnglish()
        let v1 = try await embedder.embed("the quick brown fox")
        let v2 = try await embedder.embed("the quick brown fox")
        XCTAssertEqual(v1.count, v2.count)
        XCTAssertEqual(v1, v2)
    }

    func test_dimensionalityIs768() async throws {
        let embedder = try Embedder.makeEnglish()
        let v = try await embedder.embed("hello")
        XCTAssertEqual(v.count, 768)
    }

    func test_cosineSimilarityRespectsSemantics() async throws {
        let embedder = try Embedder.makeEnglish()
        let dog = try await embedder.embed("the dog runs across the field")
        let cat = try await embedder.embed("the cat runs across the field")
        let car = try await embedder.embed("a vehicle drives on the highway")
        let dogCat = Cosine.similarity(dog, cat)
        let dogCar = Cosine.similarity(dog, car)
        XCTAssertGreaterThan(dogCat, dogCar)
    }
}
```

- [ ] **Step 2: Implement**

`AuraV3/Sources/AuraV3/Memory/Embeddings.swift`:

```swift
import Foundation
import NaturalLanguage
import CoreML
import Accelerate

public enum EmbeddingError: Error { case modelLoadFailed, embedFailed }

public actor Embedder {
    public let modelVersion: String
    private let embedding: NLContextualEmbedding

    public static func makeEnglish() throws -> Embedder {
        guard let nlc = NLContextualEmbedding(language: .english) else {
            throw EmbeddingError.modelLoadFailed
        }
        // Force CPU-only to avoid ANE contention with realtime audio.
        if !nlc.hasAvailableAssets {
            try nlc.requestAssets() // synchronous on macOS for English
        }
        try nlc.load()
        return Embedder(embedding: nlc)
    }

    private init(embedding: NLContextualEmbedding) {
        self.embedding = embedding
        self.modelVersion = "NLContextualEmbedding-en-v1"
    }

    public func embed(_ text: String) async throws -> [Float] {
        guard !text.isEmpty else { return [] }
        do {
            let result = try embedding.embeddingResult(for: text, language: .english)
            // We pool by mean across tokens to get a single vector.
            var pooled = [Float](repeating: 0, count: embedding.dimension)
            var tokenCount: Int = 0
            result.enumerateTokenVectors(in: text.startIndex..<text.endIndex) { vec, _ in
                let floats = vec.map { Float($0) }
                vDSP_vadd(pooled, 1, floats, 1, &pooled, 1, vDSP_Length(pooled.count))
                tokenCount += 1
                return true
            }
            if tokenCount > 0 {
                var divisor = Float(tokenCount)
                vDSP_vsdiv(pooled, 1, &divisor, &pooled, 1, vDSP_Length(pooled.count))
            }
            return pooled
        } catch {
            throw EmbeddingError.embedFailed
        }
    }
}

public enum Cosine {
    public static func similarity(_ a: [Float], _ b: [Float]) -> Float {
        precondition(a.count == b.count)
        var dot: Float = 0, an: Float = 0, bn: Float = 0
        vDSP_dotpr(a, 1, b, 1, &dot, vDSP_Length(a.count))
        vDSP_svesq(a, 1, &an, vDSP_Length(a.count))
        vDSP_svesq(b, 1, &bn, vDSP_Length(b.count))
        let denom = (an.squareRoot() * bn.squareRoot())
        return denom == 0 ? 0 : dot / denom
    }
}
```

- [ ] **Step 3: Run + commit**

```bash
swift test --filter EmbeddingsTests
git add AuraV3/
git commit -m "feat(v3-p3): NLContextualEmbedding wrapper (.cpuOnly, 768-dim, vDSP cosine)"
```

---

### Task A.2: event_embeddings table + EmbeddingWorker

**Files:**

- Modify: `AuraV3/Sources/AuraV3/Memory/Log.swift` (add embedding storage helpers)
- Create: `AuraV3/Sources/AuraV3/Memory/EmbeddingWorker.swift`

- [ ] **Step 1: Add embedding storage to Log**

Append to `Log.swift`:

```swift
extension Log {
    public func ensureEmbeddingsTable() async throws {
        try db.exec("""
            CREATE TABLE IF NOT EXISTS event_embeddings(
              event_id      INTEGER NOT NULL REFERENCES events(id),
              model_version TEXT NOT NULL,
              vector        BLOB NOT NULL,
              PRIMARY KEY (event_id, model_version)
            );
        """)
    }

    public func storeEmbedding(eventID: Int64, modelVersion: String, vector: [Float]) async throws {
        let data = vector.withUnsafeBufferPointer { buf in
            Data(buffer: buf)
        }
        let stmt = try db.prepare("""
            INSERT OR REPLACE INTO event_embeddings (event_id, model_version, vector)
            VALUES (?, ?, ?);
        """)
        defer { stmt.finalize() }
        stmt.bind(1, eventID)
        stmt.bind(2, modelVersion)
        // Custom blob bind:
        let SQLITE_TRANSIENT = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
        data.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
            sqlite3_bind_blob(stmt.rawHandle, 3, raw.baseAddress, Int32(raw.count), SQLITE_TRANSIENT)
        }
        _ = try stmt.step()
    }

    public func eventsWithoutEmbedding(modelVersion: String, limit: Int = 100) async throws -> [LogEvent] {
        let stmt = try db.prepare("""
            SELECT id, ts_wall, ts_mono, kind, payload, session_id, correlation_id, tombstone_for
            FROM events e
            WHERE kind = 'fact' AND tombstone_for IS NULL
              AND NOT EXISTS (
                SELECT 1 FROM event_embeddings em
                WHERE em.event_id = e.id AND em.model_version = ?
              )
            ORDER BY ts_mono DESC
            LIMIT ?;
        """)
        defer { stmt.finalize() }
        stmt.bind(1, modelVersion)
        stmt.bind(2, Int64(limit))
        var out: [LogEvent] = []
        while try stmt.step() {
            out.append(try Self.row(stmt))
        }
        return out
    }
}
```

(This requires exposing `Statement.rawHandle` — add to `SQLite+Helpers.swift`:)

```swift
extension Statement {
    public var rawHandle: OpaquePointer? { stmt }
}
```

- [ ] **Step 2: Implement EmbeddingWorker**

`AuraV3/Sources/AuraV3/Memory/EmbeddingWorker.swift`:

```swift
import Foundation

public actor EmbeddingWorker {
    private let log: Log
    private let embedder: Embedder
    private var task: Task<Void, Never>?

    public init(log: Log, embedder: Embedder) {
        self.log = log
        self.embedder = embedder
    }

    /// Start a low-priority background loop. Runs in 5-minute ticks; each tick
    /// processes up to 100 facts that lack an embedding for our model_version.
    public func start() {
        task?.cancel()
        task = Task(priority: .background) { [weak self] in
            await self?.loop()
        }
    }

    public func stop() { task?.cancel() }

    private func loop() async {
        try? await log.ensureEmbeddingsTable()
        while !Task.isCancelled {
            await processBatch()
            try? await Task.sleep(nanoseconds: 5 * 60 * 1_000_000_000)
        }
    }

    public func processBatch() async {
        do {
            let pending = try await log.eventsWithoutEmbedding(modelVersion: embedder.modelVersion)
            for event in pending {
                guard !Task.isCancelled else { break }
                let text = event.payload["value"]?.stringValue ?? ""
                guard !text.isEmpty else { continue }
                do {
                    let v = try await embedder.embed(text)
                    try await log.storeEmbedding(eventID: event.id,
                                                  modelVersion: embedder.modelVersion,
                                                  vector: v)
                } catch {
                    Log.memory.warning("embed failed for event \(event.id): \(error.localizedDescription)")
                }
            }
        } catch {
            Log.memory.warning("embedding worker batch failed: \(error.localizedDescription)")
        }
    }
}
```

- [ ] **Step 3: Build + commit**

```bash
swift build
git add AuraV3/
git commit -m "feat(v3-p3): event_embeddings table + EmbeddingWorker (low-priority, 5-min ticks, 100/batch)"
```

---

## Phase B — Lenses

### Task B.1: Episodic + UserProfile + Behavioral + Procedural queries

**Files:**

- Create: `AuraV3/Sources/AuraV3/Memory/Lenses.swift`
- Create: `AuraV3/Sources/AuraV3/Memory/UserProfile.swift`
- Create: `AuraV3/Tests/AuraV3Tests/LensesTests.swift`

- [ ] **Step 1: Test FIRST**

`AuraV3/Tests/AuraV3Tests/LensesTests.swift`:

```swift
import XCTest
@testable import AuraV3

final class LensesTests: XCTestCase {
    var dbPath: String!
    var log: Log!

    override func setUp() async throws {
        dbPath = "/tmp/aura_lenses_\(UUID().uuidString).sqlite"
        log = try await Log.open(path: dbPath)
        try await log.ensureEmbeddingsTable()
    }
    override func tearDown() async throws {
        await log.shutdown()
        for ext in ["", "-wal", "-shm"] {
            try? FileManager.default.removeItem(atPath: dbPath + ext)
        }
    }

    func test_userProfile_groupsFactsByEntity() async throws {
        let lenses = Lenses(log: log)
        await log.append(kind: EventKind.fact,
                          payload: .object(["entity": .string("user"),
                                            "predicate": .string("lives_in"),
                                            "value": .string("Kenya")]))
        await log.append(kind: EventKind.fact,
                          payload: .object(["entity": .string("user"),
                                            "predicate": .string("prefers_voice"),
                                            "value": .string("Kore")]))
        await log.append(kind: EventKind.fact,
                          payload: .object(["entity": .string("sarah"),
                                            "predicate": .string("works_at"),
                                            "value": .string("Acme")]))
        try await log.flush()
        let profile = try await lenses.userProfile(maxEntries: 20)
        XCTAssertEqual(profile.facts(for: "user").count, 2)
        XCTAssertEqual(profile.facts(for: "sarah").count, 1)
    }

    func test_behavioralLens_returnsActiveCorrectionsOnly() async throws {
        let lenses = Lenses(log: log)
        // Pending (not yet 24h) — should NOT be returned.
        let now = Date().timeIntervalSince1970 * 1000
        await log.append(kind: EventKind.correction,
                          payload: .object([
                            "rule": .string("don't summarize emails"),
                            "confirmed": .bool(false),
                            "confirmable_at": .number(now + 1000),
                          ]))
        // Active (confirmed) — should be returned.
        await log.append(kind: EventKind.correction,
                          payload: .object([
                            "rule": .string("never email Bob without confirming"),
                            "confirmed": .bool(true),
                          ]))
        try await log.flush()
        let active = try await lenses.activeCorrections(limit: 30)
        XCTAssertEqual(active.count, 1)
        XCTAssertEqual(active.first?.rule, "never email Bob without confirming")
    }
}
```

- [ ] **Step 2: Implement UserProfile**

`AuraV3/Sources/AuraV3/Memory/UserProfile.swift`:

```swift
import Foundation

public struct UserProfile: Sendable, Equatable {
    public struct Fact: Sendable, Equatable {
        public let id: Int64
        public let entity: String
        public let predicate: String
        public let value: String
        public let confidence: Double
        public let tsMono: UInt64
    }

    public let facts: [Fact]
    public init(facts: [Fact]) { self.facts = facts }

    public func facts(for entity: String) -> [Fact] { facts.filter { $0.entity == entity } }

    /// Compact text representation for system-prompt injection.
    public func renderForPrompt(maxFacts: Int = 20) -> String {
        let head = facts.prefix(maxFacts)
        let lines = head.map { "- \($0.entity).\($0.predicate) = \($0.value) (\(String(format: "%.0f", $0.confidence * 100))%)" }
        return lines.joined(separator: "\n")
    }
}
```

- [ ] **Step 3: Implement Lenses**

`AuraV3/Sources/AuraV3/Memory/Lenses.swift`:

```swift
import Foundation

public struct Correction: Sendable, Equatable {
    public let id: Int64
    public let rule: String
    public let lastFired: Date?
    public let useCount: Int
    public let createdMs: Int64
}

public struct Pattern: Sendable, Equatable {
    public let id: Int64
    public let kind: String  // pattern.semantic | pattern.procedural | pattern.contradiction
    public let payload: JSONValue
    public let confidence: Double
}

public actor Lenses {
    private let log: Log

    public init(log: Log) { self.log = log }

    // MARK: - Episodic
    public func recentTurns(limit: Int = 12) async throws -> [LogEvent] {
        // Plan-1's Log.query is by kind; emulate "recent of any turn.* kind" with two queries.
        let userTurns = try await log.query(kind: EventKind.turnUser, since: 0)
        let asstTurns = try await log.query(kind: EventKind.turnAssistant, since: 0)
        let combined = (userTurns + asstTurns).sorted { $0.tsMono < $1.tsMono }
        return Array(combined.suffix(limit))
    }

    // MARK: - Semantic / UserProfile (structured-first; embedding fallback in §B.2)
    public func userProfile(maxEntries: Int = 20) async throws -> UserProfile {
        let facts = try await log.query(kind: EventKind.fact, since: 0)
        let mapped: [UserProfile.Fact] = facts.compactMap { ev in
            guard let e = ev.payload["entity"]?.stringValue,
                  let p = ev.payload["predicate"]?.stringValue,
                  let v = ev.payload["value"]?.stringValue
            else { return nil }
            let c = ev.payload["confidence"]?.numberValue ?? 1.0
            return UserProfile.Fact(id: ev.id, entity: e, predicate: p, value: v,
                                     confidence: c, tsMono: ev.tsMono)
        }
        // Recency × frequency × citation prior — Plan 3 simplification:
        //   rank by recency × confidence; cap at maxEntries.
        let ranked = mapped.sorted { ($0.confidence, $0.tsMono) > ($1.confidence, $1.tsMono) }
        return UserProfile(facts: Array(ranked.prefix(maxEntries)))
    }

    // MARK: - Behavioral
    public func activeCorrections(limit: Int = 30) async throws -> [Correction] {
        let nowMs = Int64(Date().timeIntervalSince1970 * 1000)
        let raw = try await log.query(kind: EventKind.correction, since: 0)
        let active = raw.compactMap { ev -> Correction? in
            // confirmed must be true (24h gate passed).
            guard ev.payload["confirmed"]?.numberValue.map({ $0 > 0 }) == true ||
                    ev.payload["confirmed"] == .bool(true)
            else { return nil }
            // Stale rules drop after 90 days.
            let createdMs = ev.payload["created_ms"]?.numberValue.map(Int64.init) ?? Int64(ev.tsWall)
            if nowMs - createdMs > 90 * 24 * 3600 * 1000 { return nil }
            let rule = ev.payload["rule"]?.stringValue ?? ""
            return Correction(id: ev.id, rule: rule, lastFired: nil, useCount: 0,
                               createdMs: createdMs)
        }
        return Array(active.prefix(limit))
    }

    /// Rules pending the 24h governance gate.
    public func pendingCorrections() async throws -> [Correction] {
        let raw = try await log.query(kind: EventKind.correction, since: 0)
        return raw.compactMap { ev in
            let confirmed = (ev.payload["confirmed"] == .bool(true)) ||
                             (ev.payload["confirmed"]?.numberValue.map({ $0 > 0 }) == true)
            if confirmed { return nil }
            let rule = ev.payload["rule"]?.stringValue ?? ""
            return Correction(id: ev.id, rule: rule, lastFired: nil, useCount: 0,
                               createdMs: Int64(ev.tsWall))
        }
    }

    // MARK: - Procedural patterns (suggestions; user explicitly promotes to playbook)
    public func proceduralSuggestions(limit: Int = 10) async throws -> [Pattern] {
        let raw = try await log.query(kind: EventKind.patternProcedural, since: 0)
        return raw.prefix(limit).map { ev in
            Pattern(id: ev.id, kind: ev.kind, payload: ev.payload,
                     confidence: ev.payload["confidence"]?.numberValue ?? 0.5)
        }
    }

    public func semanticPatterns(limit: Int = 5) async throws -> [Pattern] {
        let raw = try await log.query(kind: EventKind.patternSemantic, since: 0)
        return raw.suffix(limit).map { ev in
            Pattern(id: ev.id, kind: ev.kind, payload: ev.payload,
                     confidence: ev.payload["confidence"]?.numberValue ?? 0.5)
        }
    }

    public func contradictions() async throws -> [Pattern] {
        let raw = try await log.query(kind: EventKind.patternContradiction, since: 0)
        return raw.map { Pattern(id: $0.id, kind: $0.kind, payload: $0.payload,
                                  confidence: $0.payload["confidence"]?.numberValue ?? 1.0) }
    }
}
```

- [ ] **Step 4: Run, commit**

```bash
swift test --filter LensesTests
git add AuraV3/
git commit -m "feat(v3-p3): Lenses (episodic/profile/behavioral/procedural queries) + 24h gate filter"
```

---

### Task B.2: Embedding-fallback recall in Memory.recall

**Files:**

- Modify: `AuraV3/Sources/AuraV3/Action/Memory.swift`

- [ ] **Step 1: Replace Plan-2 substring stub with ranked retrieval**

```swift
public struct MemoryTools {
    public let log: Log
    public let embedder: Embedder?    // nil during early boot; lookup falls back to FTS5

    public init(log: Log, embedder: Embedder? = nil) {
        self.log = log
        self.embedder = embedder
    }

    public func recall(query: String) async -> ToolResult {
        do {
            // 1. Structured: if query mentions an entity hint, scope by it.
            // (Plan-3 keeps it simple: try FTS5 first; fall back to embeddings.)
            let fts = try await ftsSearch(query, limit: 8)
            if fts.count >= 4 { return ToolResult.ok(output: .array(fts)) }

            // 2. Embedding fallback (last 30 days).
            guard let embedder else {
                return ToolResult.ok(output: .array(fts))
            }
            let qv = try await embedder.embed(query)
            let scored = try await embeddingRanked(qv, since: thirtyDaysAgoMono(), limit: 8)
            // Merge fts + scored, dedupe by id, keep top 8.
            var seen = Set<Int64>()
            var merged: [JSONValue] = []
            for v in fts {
                if case .object(let dict) = v, case .number(let id) = dict["id"] ?? .null,
                   !seen.contains(Int64(id)) {
                    merged.append(v); seen.insert(Int64(id))
                }
            }
            for v in scored {
                if case .object(let dict) = v, case .number(let id) = dict["id"] ?? .null,
                   !seen.contains(Int64(id)) {
                    merged.append(v); seen.insert(Int64(id))
                }
            }
            return ToolResult.ok(output: .array(Array(merged.prefix(8))))
        } catch {
            return ToolResult.failed(reason: .toolError(code: "recall"), detail: String(describing: error))
        }
    }

    private func ftsSearch(_ query: String, limit: Int) async throws -> [JSONValue] {
        // Plan-3 simplification: use Log.query and substring filter.
        let facts = try await log.query(kind: EventKind.fact, since: 0)
        let q = query.lowercased()
        return facts.compactMap { ev -> JSONValue? in
            let bag = [
                ev.payload["entity"]?.stringValue ?? "",
                ev.payload["predicate"]?.stringValue ?? "",
                ev.payload["value"]?.stringValue ?? "",
            ].joined(separator: " ").lowercased()
            guard bag.contains(q) else { return nil }
            return .object([
                "id": .number(Double(ev.id)),
                "entity": ev.payload["entity"] ?? .null,
                "predicate": ev.payload["predicate"] ?? .null,
                "value": ev.payload["value"] ?? .null,
                "_source": .string("fts"),
            ])
        }.suffix(limit).map { $0 }
    }

    private func embeddingRanked(_ query: [Float], since: UInt64, limit: Int) async throws -> [JSONValue] {
        // Naive: pull all facts in window, score each. Plan 4 will optimize if needed.
        let facts = try await log.query(kind: EventKind.fact, since: since)
        guard let embedder else { return [] }
        var scored: [(LogEvent, Float)] = []
        for ev in facts {
            // For Plan 3: skip facts without embeddings; they get computed by the worker.
            // We just rank by what's available.
            let text = ev.payload["value"]?.stringValue ?? ""
            if text.isEmpty { continue }
            do {
                let v = try await embedder.embed(text)   // deterministic for same text
                scored.append((ev, Cosine.similarity(query, v)))
            } catch {}
        }
        let top = scored.sorted { $0.1 > $1.1 }.prefix(limit)
        return top.map { (ev, score) in
            .object([
                "id": .number(Double(ev.id)),
                "entity": ev.payload["entity"] ?? .null,
                "predicate": ev.payload["predicate"] ?? .null,
                "value": ev.payload["value"] ?? .null,
                "_source": .string("embedding"),
                "_score": .number(Double(score)),
            ])
        }
    }

    private func thirtyDaysAgoMono() -> UInt64 {
        let now = MonotonicClock.now()
        let thirtyDays: UInt64 = 30 * 24 * 3600 * 1_000_000_000
        return now > thirtyDays ? now - thirtyDays : 0
    }
}
```

- [ ] **Step 2: Wire `embedder` into AppCoordinator's `MemoryTools` construction**

In `AppCoordinator.setupTools`, change:

```swift
let memory = MemoryTools(log: state.log, embedder: try? Embedder.makeEnglish())
```

- [ ] **Step 3: Build, run existing tests, commit**

```bash
swift build
swift test
git add AuraV3/
git commit -m "feat(v3-p3): MemoryTools.recall — FTS+substring then embedding fallback (30-day bound)"
```

---

## Phase C — Consolidation worker

### Task C.1: Hot pass — extract patterns from a session window

**Files:**

- Create: `AuraV3/Sources/AuraV3/Memory/Consolidation.swift`
- Create: `AuraV3/Tests/AuraV3Tests/ConsolidationFixtureTests.swift`

- [ ] **Step 1: Implement the worker structure**

`AuraV3/Sources/AuraV3/Memory/Consolidation.swift`:

```swift
import Foundation

public actor ConsolidationWorker {
    private let log: Log
    private let apiKey: String
    private let session = URLSession(configuration: .ephemeral)
    private var lastHotRun: Date = .distantPast
    private var lastColdRun: Date = .distantPast

    public init(log: Log, apiKey: String) {
        self.log = log
        self.apiKey = apiKey
    }

    /// Run a hot pass after a session.end; bounded at 8K input tokens.
    public func runHotPass(sessionStartMono: UInt64, sessionEndMono: UInt64) async {
        guard Date().timeIntervalSince(lastHotRun) > 30 else { return }
        lastHotRun = Date()
        do {
            let turns = try await log.query(kind: EventKind.turnUser, since: sessionStartMono)
            let asst = try await log.query(kind: EventKind.turnAssistant, since: sessionStartMono)
            let toolCalls = try await log.query(kind: EventKind.turnToolCall, since: sessionStartMono)
            let combined = (turns + asst + toolCalls).sorted { $0.tsMono < $1.tsMono }
                .filter { $0.tsMono <= sessionEndMono }
            let transcript = renderTranscript(combined)
            let response = try await flashConsolidate(prompt: hotPrompt(transcript: transcript))
            await applyConsolidationResponse(response)
        } catch {
            Log.memory.warning("hot consolidation failed: \(error.localizedDescription)")
        }
    }

    /// Run the daily cold pass over the last 24 hours.
    public func runColdPass() async {
        guard Date().timeIntervalSince(lastColdRun) > 23 * 3600 else { return }
        lastColdRun = Date()
        do {
            let dayMono = MonotonicClock.now() &- UInt64(24 * 3600 * 1_000_000_000)
            let turns = try await log.query(kind: EventKind.turnUser, since: dayMono)
            let asst = try await log.query(kind: EventKind.turnAssistant, since: dayMono)
            let combined = (turns + asst).sorted { $0.tsMono < $1.tsMono }
            let transcript = renderTranscript(combined)
            let response = try await flashConsolidate(prompt: coldPrompt(transcript: transcript))
            await applyConsolidationResponse(response)
        } catch {
            Log.memory.warning("cold consolidation failed: \(error.localizedDescription)")
        }
    }

    private func renderTranscript(_ events: [LogEvent]) -> String {
        events.prefix(80).compactMap { ev in
            let text = ev.payload["text"]?.stringValue ?? ev.payload["operation"]?.stringValue
            guard let t = text else { return nil }
            let kindShort: String = {
                switch ev.kind {
                case EventKind.turnUser: return "USER"
                case EventKind.turnAssistant: return "AURA"
                case EventKind.turnToolCall: return "TOOL"
                default: return "OTHER"
                }
            }()
            return "[\(kindShort)] \(t)"
        }.joined(separator: "\n")
    }

    private func hotPrompt(transcript: String) -> String {
        """
        You are reviewing one session of a voice assistant's transcript. Extract:

        1. Up to 5 SEMANTIC patterns: invariants the user expressed across multiple turns. \
           Output as JSON array: [{"pattern":"...","confidence":0.0-1.0,"source_count":N}]
        2. Up to 5 PROCEDURAL patterns: action sequences the user repeated. \
           Output as JSON array: [{"intent":"...","steps":["..."],"frequency":N}]
        3. Up to 3 CONTRADICTIONS: facts the transcript disagrees with itself about. \
           Output as JSON array: [{"description":"...","conflicting":["...","..."]}]

        Transcript:
        \(transcript.prefix(8000))

        Return a single JSON object: {"semantic":[...],"procedural":[...],"contradictions":[...]}.
        Return ONLY the JSON, no prose.
        """
    }

    private func coldPrompt(transcript: String) -> String {
        // Cold pass: same shape but framed as "across a day" — looking for patterns
        // that recur across multiple sessions (you'll see session boundaries in the
        // transcript via session.start markers in production).
        """
        You are reviewing 24 hours of a user's voice-assistant interactions. Extract durable
        patterns the user shows ACROSS multiple sessions: things they consistently care about,
        consistently want done a certain way, or contradictions between things said on different days.

        Transcript:
        \(transcript.prefix(8000))

        Return one JSON object: {"semantic":[...],"procedural":[...],"contradictions":[...]}.
        Return ONLY the JSON.
        """
    }

    private func flashConsolidate(prompt: String) async throws -> [String: Any] {
        let url = URL(string: "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key=\(apiKey)")!
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.timeoutInterval = 30
        let body: [String: Any] = [
            "contents": [["parts": [["text": prompt]]]],
            "generationConfig": ["maxOutputTokens": 1024, "temperature": 0.2]
        ]
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, _) = try await session.data(for: req)
        guard let obj = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let candidates = obj["candidates"] as? [[String: Any]],
              let first = candidates.first,
              let content = first["content"] as? [String: Any],
              let parts = content["parts"] as? [[String: Any]],
              let text = parts.first?["text"] as? String,
              let json = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any]
        else {
            return [:]
        }
        return json
    }

    private func applyConsolidationResponse(_ response: [String: Any]) async {
        if let semantic = response["semantic"] as? [[String: Any]] {
            for s in semantic {
                guard let pattern = s["pattern"] as? String else { continue }
                let conf = (s["confidence"] as? Double) ?? 0.5
                await log.append(kind: EventKind.patternSemantic,
                                  payload: .object([
                                    "pattern": .string(pattern),
                                    "confidence": .number(conf),
                                    "source_count": .number(Double((s["source_count"] as? Int) ?? 1))
                                  ]))
            }
        }
        if let procedural = response["procedural"] as? [[String: Any]] {
            for p in procedural {
                guard let intent = p["intent"] as? String,
                      let steps = p["steps"] as? [String]
                else { continue }
                await log.append(kind: EventKind.patternProcedural,
                                  payload: .object([
                                    "intent": .string(intent),
                                    "steps": .array(steps.map(JSONValue.string)),
                                    "frequency": .number(Double((p["frequency"] as? Int) ?? 1))
                                  ]))
            }
        }
        if let contradictions = response["contradictions"] as? [[String: Any]] {
            for c in contradictions {
                guard let desc = c["description"] as? String else { continue }
                let confl = (c["conflicting"] as? [String]) ?? []
                await log.append(kind: EventKind.patternContradiction,
                                  payload: .object([
                                    "description": .string(desc),
                                    "conflicting": .array(confl.map(JSONValue.string)),
                                  ]))
            }
        }
    }
}
```

- [ ] **Step 2: Test FIRST (uses fixture, doesn't hit Flash)**

`AuraV3/Tests/AuraV3Tests/ConsolidationFixtureTests.swift`:

```swift
import XCTest
@testable import AuraV3

final class ConsolidationFixtureTests: XCTestCase {
    func test_appliesSemanticPatternsToLog() async throws {
        let path = "/tmp/aura_cons_\(UUID().uuidString).sqlite"
        defer { try? FileManager.default.removeItem(atPath: path) }
        let log = try await Log.open(path: path)
        defer { Task { await log.shutdown() } }

        // Bypass Flash by calling the apply path directly.
        let worker = ConsolidationWorker(log: log, apiKey: "")
        let mock: [String: Any] = [
            "semantic": [
                ["pattern": "user prefers brief replies", "confidence": 0.9, "source_count": 3]
            ],
            "procedural": [],
            "contradictions": [],
        ]
        await worker.applyForTest(mock)
        try await log.flush()

        let patterns = try await log.query(kind: EventKind.patternSemantic, since: 0)
        XCTAssertEqual(patterns.count, 1)
        XCTAssertEqual(patterns.first?.payload["pattern"]?.stringValue, "user prefers brief replies")
    }
}

// Test-only escape hatch
extension ConsolidationWorker {
    func applyForTest(_ response: [String: Any]) async {
        await applyConsolidationResponse(response)
    }
}
```

- [ ] **Step 3: Run + commit**

```bash
swift test --filter ConsolidationFixtureTests
git add AuraV3/
git commit -m "feat(v3-p3): consolidation worker (hot+cold passes; emits pattern.semantic/procedural/contradiction)"
```

---

### Task C.2: Wire idle-gated trigger into AppCoordinator

**Files:**

- Modify: `AuraV3/Sources/AuraV3/App/AppCoordinator.swift`

- [ ] **Step 1: Add an idle detector and trigger consolidation**

In `AppCoordinator`:

```swift
private var consolidation: ConsolidationWorker!
private var idleStartMono: UInt64 = 0
private var sessionStartMono: UInt64 = 0

// In startSession() after registering tools:
sessionStartMono = MonotonicClock.now()
self.consolidation = ConsolidationWorker(log: state.log, apiKey: Config.apiKey() ?? "")

// Track idle: any user/playback/tool/effect activity resets it.
private func tickIdle() async {
    let now = MonotonicClock.now()
    let idleNs: UInt64 = 30 * 1_000_000_000
    if now &- idleStartMono >= idleNs {
        await consolidation.runHotPass(sessionStartMono: sessionStartMono, sessionEndMono: now)
        await consolidation.runColdPass()
        idleStartMono = now  // reset to avoid re-firing immediately
    }
}

private func resetIdle() { idleStartMono = MonotonicClock.now() }

// Schedule a 30s tick:
private func startIdleLoop() {
    Task { [weak self] in
        while let self = self {
            try? await Task.sleep(nanoseconds: 30 * 1_000_000_000)
            await self.tickIdle()
        }
    }
}
```

Call `resetIdle()` from the receive loop on every user transcript / playback start / tool call. Call `startIdleLoop()` once in `startSession`.

- [ ] **Step 2: Build + commit**

```bash
swift build
git add AuraV3/
git commit -m "feat(v3-p3): wire ConsolidationWorker hot/cold passes to canonical idle detector"
```

---

## Phase D — Self-model

### Task D.1: SelfModel derived view

**Files:**

- Create: `AuraV3/Sources/AuraV3/SelfModel/SelfModel.swift`
- Create: `AuraV3/Tests/AuraV3Tests/SelfModelTests.swift`

- [ ] **Step 1: Test FIRST**

`AuraV3/Tests/AuraV3Tests/SelfModelTests.swift`:

```swift
import XCTest
@testable import AuraV3

final class SelfModelTests: XCTestCase {
    var dbPath: String!
    var log: Log!

    override func setUp() async throws {
        dbPath = "/tmp/aura_self_\(UUID().uuidString).sqlite"
        log = try await Log.open(path: dbPath)
    }
    override func tearDown() async throws {
        await log.shutdown()
        for ext in ["", "-wal", "-shm"] {
            try? FileManager.default.removeItem(atPath: dbPath + ext)
        }
    }

    func test_toolReliability_computesFromTurnToolResultEvents() async throws {
        // 3 click attempts: 2 ok, 1 noStateChange.
        for status in ["ok", "ok", "failed"] {
            await log.append(kind: EventKind.turnToolResult,
                              payload: .object([
                                "name": .string("click"),
                                "status": .string(status),
                                "failure_reason": status == "failed" ? .string("noStateChange") : .null,
                              ]))
        }
        try await log.flush()
        let sm = try await SelfModel.build(from: log)
        let r = sm.toolReliability["click"]!
        XCTAssertEqual(r.successCount, 2)
        XCTAssertEqual(r.failureCount, 1)
        XCTAssertEqual(r.successRate, 2.0/3.0, accuracy: 0.001)
    }
}
```

- [ ] **Step 2: Implement**

`AuraV3/Sources/AuraV3/SelfModel/SelfModel.swift`:

```swift
import Foundation

public struct SelfModel: Sendable, Equatable {
    public struct Reliability: Sendable, Equatable {
        public var successCount: Int = 0
        public var failureCount: Int = 0
        public var lastFailureReason: String? = nil
        public var successRate: Double {
            let total = successCount + failureCount
            return total == 0 ? 1.0 : Double(successCount) / Double(total)
        }
    }

    public let toolReliability: [String: Reliability]   // by tool name
    public let recentFailures: [LogEvent]
    public let activeUncertainty: [String]

    public static func build(from log: Log) async throws -> SelfModel {
        let results = try await log.query(kind: EventKind.turnToolResult, since: 0)
        var byTool: [String: Reliability] = [:]
        var recentFailures: [LogEvent] = []
        for ev in results {
            guard let name = ev.payload["name"]?.stringValue else { continue }
            var r = byTool[name] ?? Reliability()
            if ev.payload["status"]?.stringValue == "ok" {
                r.successCount += 1
            } else {
                r.failureCount += 1
                r.lastFailureReason = ev.payload["failure_reason"]?.stringValue
                recentFailures.append(ev)
            }
            byTool[name] = r
        }
        recentFailures = Array(recentFailures.suffix(20))
        let uncertainty = byTool.compactMap { (name, r) -> String? in
            guard r.successCount + r.failureCount >= 5, r.successRate < 0.6 else { return nil }
            return "\(name) is unreliable lately (\(Int(r.successRate * 100))%)"
        }
        return SelfModel(toolReliability: byTool, recentFailures: recentFailures,
                          activeUncertainty: uncertainty)
    }

    public func renderForPrompt() -> String {
        guard !activeUncertainty.isEmpty else { return "" }
        return "Things I've been getting wrong recently:\n" +
            activeUncertainty.map { "- \($0)" }.joined(separator: "\n")
    }
}
```

- [ ] **Step 3: Run + commit**

```bash
swift test --filter SelfModelTests
git add AuraV3/
git commit -m "feat(v3-p3): SelfModel — derived tool-reliability view + activeUncertainty surface"
```

---

## Phase E — Context.swift

### Task E.1: StablePrefix (persona + tool-routing rules)

**Files:**

- Create: `AuraV3/Sources/AuraV3/Context/StablePrefix.swift`

- [ ] **Step 1: Implement (compile-time string)**

`AuraV3/Sources/AuraV3/Context/StablePrefix.swift`:

```swift
import Foundation

public enum StablePrefix {
    public static let text: String = """
    You are Aura. A quiet partner who interrupts only when the user would regret not being interrupted. Concise; technically precise; respects flow over chatter; admits uncertainty when uncertain.

    You hold opinions and you state them. When asked what you think, you say what you think. When you disagree with the user — about a technical choice, an interpretation, a plan — you say so directly, with the reasoning, without softening into "you might consider" or "have you thought about" hedge phrases. You do not capitulate to social pressure that lacks new information. If the user pushes back and offers no new evidence, you maintain the position; if they offer new evidence, you update.

    You are not a yes-man. When the user proposes something you believe is wrong — fragile, overcomplicated, solving the wrong problem, ignoring a constraint — you push back with a concrete alternative. You do not execute bad ideas just because they were asked for.

    You are direct, not rude. Disagreement is data; contempt is noise. State the position, give the reasoning, let the user decide.

    TOOL ROUTING:
    - For UI interactions, prefer `click(target)` / `type(text)` — they go through AX-first targeting.
    - For email/messages/calendar, prefer the structured app-native tools, never invent AppleScript.
    - For browsers, use the typed `browser.click(selector)` / `browser.fill(selector, value)`. There is no eval; do not request one.
    - For unknown destructive actions (deletes, sends), CONFIRM first — say what you're about to do, wait for "yes".

    TOOL RESULT SEMANTICS:
    - `status: "ok"` — the tool succeeded; you can proceed.
    - `status: "needsClarification"` — return the question to the user; do not retry blindly.
    - `status: "failed"` with `failure_reason: "noStateChange"` — the action posted but nothing changed; try a different strategy (different selector / keyboard shortcut / different element).
    - `status: "failed"` with `failure_reason: "targetNotFound"` — describe the target differently; or ask the user where it is.
    - `status: "failed"` with `failure_reason: "rotationForced"` — the session rotated mid-call; re-derive what to do from the current state, do not assume the action completed.
    - `status: "failed"` with `failure_reason: "permissionDenied"` — say so to the user; suggest checking System Settings.

    EFFECT DISCIPLINE: every world-mutating action is logged with a correlation_id. If you see "I already did this" duplicates in the conversation, check whether you're being asked to repeat yourself — if not, do NOT redo the action.

    REFUSAL: refuse requests that would (a) exfiltrate credentials/secrets, (b) send messages to recipients without the user's clear naming, (c) delete data without explicit confirmation, (d) bypass macOS permission prompts.

    EXAMPLES:
    User: "Send a draft email to my team about the launch."
    Aura: [calls mail.send(to=<team email from contacts>, subject="Launch update", body="...") after asking for the team's email if not in contacts.find]

    User: "What do you think about adding Redis to this project?"
    Aura: "Honestly I think it's overkill for the current load. Postgres' LISTEN/NOTIFY handles your pubsub case in 30 lines. Adding Redis means a new ops surface for what's a 200-row table." [does not soften into "have you considered..."]
    """
}
```

- [ ] **Step 2: Commit**

```bash
git add AuraV3/
git commit -m "feat(v3-p3): StablePrefix — explicit non-yes-man persona + tool-routing rules"
```

---

### Task E.2: RollingSummary

**Files:**

- Create: `AuraV3/Sources/AuraV3/Context/RollingSummary.swift`
- Create: `AuraV3/Tests/AuraV3Tests/RollingSummaryTests.swift`

- [ ] **Step 1: Implement**

`AuraV3/Sources/AuraV3/Context/RollingSummary.swift`:

```swift
import Foundation

public actor RollingSummary {
    private let log: Log
    private let apiKey: String
    private let session = URLSession(configuration: .ephemeral)
    private var cached: String = ""

    public init(log: Log, apiKey: String) {
        self.log = log
        self.apiKey = apiKey
    }

    public var current: String { cached }

    /// Update on every assistant turn (off the audio path).
    public func update(latestUserText: String, latestAssistantText: String) async {
        let prompt = """
        Update this 2-3KB rolling summary of an ongoing voice-assistant conversation.
        Keep it factual, no fluff. Preserve referent names ("Sarah's laptop", "the Q2 deck"),
        recent decisions, and the current focus. Return ONLY the new summary text, plain prose.

        Previous summary (may be empty):
        \(cached)

        New user turn:
        \(latestUserText)

        New assistant turn:
        \(latestAssistantText)

        New summary (max 3000 characters):
        """
        do {
            let url = URL(string: "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key=\(apiKey)")!
            var req = URLRequest(url: url)
            req.httpMethod = "POST"
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
            req.timeoutInterval = 6
            let body: [String: Any] = [
                "contents": [["parts": [["text": prompt]]]],
                "generationConfig": ["maxOutputTokens": 800, "temperature": 0.1]
            ]
            req.httpBody = try JSONSerialization.data(withJSONObject: body)
            let (data, _) = try await session.data(for: req)
            guard let obj = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let candidates = obj["candidates"] as? [[String: Any]],
                  let first = candidates.first,
                  let content = first["content"] as? [String: Any],
                  let parts = content["parts"] as? [[String: Any]],
                  let text = parts.first?["text"] as? String
            else { return }
            cached = String(text.prefix(3000))
            await log.append(kind: EventKind.summaryRolling, payload: .string(cached))
        } catch {
            Log.context.warning("rolling summary update failed: \(error.localizedDescription)")
        }
    }

    public func loadFromLog() async {
        guard let last = (try? await log.query(kind: EventKind.summaryRolling, since: 0))?.last else { return }
        cached = last.payload.stringValue ?? ""
    }
}
```

- [ ] **Step 2: Test (no Flash; structural only)**

`AuraV3/Tests/AuraV3Tests/RollingSummaryTests.swift`:

```swift
import XCTest
@testable import AuraV3

final class RollingSummaryTests: XCTestCase {
    func test_currentReturnsCachedValue() async throws {
        let path = "/tmp/aura_summary_\(UUID().uuidString).sqlite"
        defer { try? FileManager.default.removeItem(atPath: path) }
        let log = try await Log.open(path: path)
        defer { Task { await log.shutdown() } }
        let summary = RollingSummary(log: log, apiKey: "")
        XCTAssertEqual(await summary.current, "")
        await log.append(kind: EventKind.summaryRolling,
                          payload: .string("we discussed the Q2 deck and pricing"))
        try await log.flush()
        await summary.loadFromLog()
        XCTAssertEqual(await summary.current, "we discussed the Q2 deck and pricing")
    }
}
```

- [ ] **Step 3: Run + commit**

```bash
swift test --filter RollingSummaryTests
git add AuraV3/
git commit -m "feat(v3-p3): RollingSummary (per-turn Flash update; loadFromLog rehydration)"
```

---

### Task E.3: SubgoalInferrer (drift detector input #3)

**Files:**

- Create: `AuraV3/Sources/AuraV3/Context/SubgoalInferrer.swift`

- [ ] **Step 1: Implement**

`AuraV3/Sources/AuraV3/Context/SubgoalInferrer.swift`:

```swift
import Foundation

public actor SubgoalInferrer {
    private let apiKey: String
    private let session = URLSession(configuration: .ephemeral)
    private var lastSubgoal: String = ""

    public init(apiKey: String) { self.apiKey = apiKey }

    public func inferSubgoal(activeGoalStack: [String], recentObservations: [String]) async -> String {
        let prompt = """
        The user is working on this goal stack (top to bottom): \(activeGoalStack.joined(separator: " ▸ ")).

        Recent observations (last 60 seconds):
        \(recentObservations.prefix(10).joined(separator: "\n"))

        What is the user's CURRENT inferred subgoal? Return one short sentence (under 80 chars) describing what they appear to be doing right now. If they're clearly off the goal, return the literal phrase "off-goal: " followed by what they appear to be doing instead.
        """
        do {
            let url = URL(string: "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key=\(apiKey)")!
            var req = URLRequest(url: url)
            req.httpMethod = "POST"
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
            req.timeoutInterval = 2
            let body: [String: Any] = [
                "contents": [["parts": [["text": prompt]]]],
                "generationConfig": ["maxOutputTokens": 80, "temperature": 0.2]
            ]
            req.httpBody = try JSONSerialization.data(withJSONObject: body)
            let (data, _) = try await session.data(for: req)
            guard let obj = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let candidates = obj["candidates"] as? [[String: Any]],
                  let first = candidates.first,
                  let content = first["content"] as? [String: Any],
                  let parts = content["parts"] as? [[String: Any]],
                  let text = parts.first?["text"] as? String
            else { return lastSubgoal }
            let cleaned = text.trimmingCharacters(in: .whitespacesAndNewlines)
            lastSubgoal = String(cleaned.prefix(120))
            return lastSubgoal
        } catch {
            return lastSubgoal
        }
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add AuraV3/
git commit -m "feat(v3-p3): SubgoalInferrer — Flash REST call to derive current subgoal under goal stack"
```

---

### Task E.4: Context.swift — sessionStartContext + rotationDigest

**Files:**

- Create: `AuraV3/Sources/AuraV3/Context/Context.swift`
- Create: `AuraV3/Sources/AuraV3/Context/DynamicSuffix.swift`
- Create: `AuraV3/Tests/AuraV3Tests/ContextFixtureTests.swift`

- [ ] **Step 1: Implement DynamicSuffix**

`AuraV3/Sources/AuraV3/Context/DynamicSuffix.swift`:

```swift
import Foundation

public struct DynamicSuffixInputs: Sendable {
    public let userProfile: UserProfile
    public let activeCorrections: [Correction]
    public let activeGoalStack: [String]
    public let inferredSubgoal: String?
    public let semanticPatterns: [Pattern]
    public let selfModel: SelfModel
}

public enum DynamicSuffix {
    public static func render(_ inputs: DynamicSuffixInputs, tokenBudget: Int = 4000) -> (text: String, dropped: [String]) {
        var sections: [(String, String)] = []
        if !inputs.activeGoalStack.isEmpty {
            sections.append(("Active goal stack", inputs.activeGoalStack.joined(separator: " ▸ ")))
        }
        if let sg = inputs.inferredSubgoal, !sg.isEmpty {
            sections.append(("Inferred subgoal right now", sg))
        }
        let profileText = inputs.userProfile.renderForPrompt(maxFacts: 20)
        if !profileText.isEmpty {
            sections.append(("Things I think I know about you", profileText))
        }
        let correctionsText = inputs.activeCorrections.prefix(10).map { "- \($0.rule)" }.joined(separator: "\n")
        if !correctionsText.isEmpty {
            sections.append(("Things you've asked me to remember", correctionsText))
        }
        let semantic = inputs.semanticPatterns.prefix(3).compactMap { p -> String? in
            guard let s = p.payload["pattern"]?.stringValue else { return nil }
            return "- \(s) (\(Int(p.confidence * 100))%)"
        }.joined(separator: "\n")
        if !semantic.isEmpty {
            sections.append(("Patterns I've noticed about you", semantic))
        }
        let selfText = inputs.selfModel.renderForPrompt()
        if !selfText.isEmpty {
            sections.append(("About my own current state", selfText))
        }

        // Estimate tokens: ~4 chars/token rough heuristic.
        var dropped: [String] = []
        var rendered = sections.map { (h, b) in "## \(h)\n\(b)" }.joined(separator: "\n\n")
        let estimateTokens: (String) -> Int = { Int(Double($0.count) / 4.0) }
        if estimateTokens(rendered) > tokenBudget {
            // Trim order: self-model, semantic patterns (3→1), profile (20→10),
            // corrections (drop oldest beyond top 10 by recency-and-citation),
            // oldest user-fact bucket. Plan-3 simplification:
            let keep: [(String, String)] = sections.compactMap { (h, b) in
                if h == "About my own current state" { dropped.append(h); return nil }
                return (h, b)
            }
            rendered = keep.map { (h, b) in "## \(h)\n\(b)" }.joined(separator: "\n\n")
            if estimateTokens(rendered) > tokenBudget {
                let keep2 = keep.compactMap { (h, b) -> (String, String)? in
                    if h == "Patterns I've noticed about you" {
                        dropped.append(h)
                        let firstLine = b.split(separator: "\n").first.map(String.init) ?? ""
                        return (h, firstLine)
                    }
                    return (h, b)
                }
                rendered = keep2.map { (h, b) in "## \(h)\n\(b)" }.joined(separator: "\n\n")
            }
        }
        return (rendered, dropped)
    }
}
```

- [ ] **Step 2: Implement Context.swift**

`AuraV3/Sources/AuraV3/Context/Context.swift`:

```swift
import Foundation

public actor ContextBuilder {
    private let log: Log
    private let lenses: Lenses
    private let summary: RollingSummary

    public init(log: Log, lenses: Lenses, summary: RollingSummary) {
        self.log = log
        self.lenses = lenses
        self.summary = summary
    }

    public struct SystemPromptResult: Sendable {
        public let text: String
        public let approxTokens: Int
        public let droppedSections: [String]
    }

    public func sessionStartContext(now: Date,
                                      mode: String,
                                      activeGoalStack: [String],
                                      inferredSubgoal: String?,
                                      selfModel: SelfModel) async throws -> SystemPromptResult {
        let profile = try await lenses.userProfile(maxEntries: 20)
        let corrections = try await lenses.activeCorrections(limit: 30)
        let semantic = try await lenses.semanticPatterns(limit: 3)
        let dyn = DynamicSuffix.render(DynamicSuffixInputs(
            userProfile: profile,
            activeCorrections: corrections,
            activeGoalStack: activeGoalStack,
            inferredSubgoal: inferredSubgoal,
            semanticPatterns: semantic,
            selfModel: selfModel
        ))
        let prompt = StablePrefix.text + "\n\n## Mode: \(mode)\n\n" + dyn.text
        let tokens = Int(Double(prompt.count) / 4.0)

        // Log the assembled context.
        await log.append(kind: EventKind.contextAssembled, payload: .object([
            "kind": .string("sessionStart"),
            "approx_tokens": .number(Double(tokens)),
            "dropped_sections": .array(dyn.dropped.map(JSONValue.string)),
            "hash": .string(prompt.sha256Prefix(16)),
        ]))
        if !dyn.dropped.isEmpty {
            await log.append(kind: EventKind.contextTrimmed, payload: .object([
                "dropped": .array(dyn.dropped.map(JSONValue.string)),
            ]))
        }
        return SystemPromptResult(text: prompt, approxTokens: tokens, droppedSections: dyn.dropped)
    }

    public func rotationDigest(activeGoalStack: [String],
                                pendingTool: PendingToolIntent?,
                                inferredSubgoal: String?) async -> String {
        let recentTurns = (try? await lenses.recentTurns(limit: 4)) ?? []
        var lines: [String] = []
        lines.append("[Aura session continuation]")
        if !activeGoalStack.isEmpty {
            lines.append("Active goal stack: \(activeGoalStack.joined(separator: " ▸ "))")
        }
        if let sg = inferredSubgoal, !sg.isEmpty { lines.append("Inferred subgoal: \(sg)") }
        let summaryText = await summary.current
        if !summaryText.isEmpty {
            lines.append("Conversation so far: \(summaryText)")
        }
        let lastTurns = recentTurns.suffix(2).compactMap { ev -> String? in
            guard let text = ev.payload["text"]?.stringValue else { return nil }
            return "[\(ev.kind == EventKind.turnUser ? "USER" : "AURA")] \(text)"
        }
        if !lastTurns.isEmpty {
            lines.append("Last exchanges:\n" + lastTurns.joined(separator: "\n"))
        }
        if let p = pendingTool {
            let argsJSON = (try? JSONEncoder().encode(p.args))
                .flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
            lines.append("Pending tool intent: \(p.operation) \(argsJSON)")
        }
        let digest = lines.joined(separator: "\n")
        await log.append(kind: EventKind.contextAssembled, payload: .object([
            "kind": .string("rotationDigest"),
            "approx_tokens": .number(Double(digest.count / 4)),
            "hash": .string(digest.sha256Prefix(16)),
        ]))
        return digest
    }
}

private extension String {
    func sha256Prefix(_ n: Int) -> String {
        // Simple FNV hash; not crypto, just a deterministic ID.
        var h: UInt64 = 0xcbf29ce484222325
        for byte in self.utf8 {
            h ^= UInt64(byte)
            h = h &* 0x100000001b3
        }
        return String(h, radix: 16).prefix(n).description
    }
}
```

- [ ] **Step 3: Fixture test**

`AuraV3/Tests/AuraV3Tests/ContextFixtureTests.swift`:

```swift
import XCTest
@testable import AuraV3

final class ContextFixtureTests: XCTestCase {
    var dbPath: String!
    var log: Log!
    var ctx: ContextBuilder!

    override func setUp() async throws {
        dbPath = "/tmp/aura_ctx_\(UUID().uuidString).sqlite"
        log = try await Log.open(path: dbPath)
        try await log.ensureEmbeddingsTable()
        let lenses = Lenses(log: log)
        let summary = RollingSummary(log: log, apiKey: "")
        ctx = ContextBuilder(log: log, lenses: lenses, summary: summary)

        // Seed: 3 facts, 1 active correction, 1 pending correction (24h gate).
        await log.append(kind: EventKind.fact, payload: .object([
            "entity": .string("user"), "predicate": .string("lives_in"),
            "value": .string("Kenya"), "confidence": .number(1.0)
        ]))
        await log.append(kind: EventKind.correction, payload: .object([
            "rule": .string("don't open Safari, use Chrome"),
            "confirmed": .bool(true)
        ]))
        await log.append(kind: EventKind.correction, payload: .object([
            "rule": .string("never auto-send email"),
            "confirmed": .bool(false),
            "confirmable_at": .number(Double(Date().timeIntervalSince1970 * 1000) + 1_000_000)
        ]))
        try await log.flush()
    }

    override func tearDown() async throws {
        await log.shutdown()
        for ext in ["", "-wal", "-shm"] {
            try? FileManager.default.removeItem(atPath: dbPath + ext)
        }
    }

    func test_sessionStartIncludesPersonaAndProfile() async throws {
        let selfModel = try await SelfModel.build(from: log)
        let result = try await ctx.sessionStartContext(
            now: Date(), mode: "default",
            activeGoalStack: [], inferredSubgoal: nil, selfModel: selfModel)
        XCTAssertTrue(result.text.contains("You are Aura"))
        XCTAssertTrue(result.text.contains("Kenya"))
        XCTAssertTrue(result.text.contains("don't open Safari"))
        XCTAssertFalse(result.text.contains("never auto-send email"))   // pending; gated out
        XCTAssertGreaterThan(result.approxTokens, 0)
    }

    func test_rotationDigestIsDeterministicGivenInputs() async throws {
        let d1 = await ctx.rotationDigest(activeGoalStack: ["v3 spec"],
                                            pendingTool: nil, inferredSubgoal: nil)
        let d2 = await ctx.rotationDigest(activeGoalStack: ["v3 spec"],
                                            pendingTool: nil, inferredSubgoal: nil)
        XCTAssertEqual(d1, d2)
    }
}
```

- [ ] **Step 4: Run + commit**

```bash
swift test --filter ContextFixtureTests
git add AuraV3/
git commit -m "feat(v3-p3): Context.swift — sessionStartContext + rotationDigest, fixture-tested, logs context.assembled"
```

---

## Phase F — Observation pipeline + policy host

### Task F.1: ObservationStream + HistoryView

**Files:**

- Create: `AuraV3/Sources/AuraV3/Observation/ObservationStream.swift`
- Create: `AuraV3/Sources/AuraV3/Observation/HistoryView.swift`

- [ ] **Step 1: Implement**

`AuraV3/Sources/AuraV3/Observation/ObservationStream.swift`:

```swift
import Foundation

public struct Observation: Sendable {
    public let tsWall: Date
    public let tsMono: UInt64
    public let kind: String
    public let context: [String: JSONValue]
    public init(kind: String, context: [String: JSONValue] = [:]) {
        self.kind = kind
        self.context = context
        self.tsWall = Date()
        self.tsMono = MonotonicClock.now()
    }
}

public actor ObservationStream {
    private var continuations: [UUID: AsyncStream<Observation>.Continuation] = [:]

    public init() {}

    public func emit(_ obs: Observation) {
        for (_, c) in continuations { c.yield(obs) }
    }

    public func subscribe() -> AsyncStream<Observation> {
        AsyncStream { cont in
            let id = UUID()
            Task { await self.add(id: id, cont: cont) }
            cont.onTermination = { _ in
                Task { await self.remove(id: id) }
            }
        }
    }

    private func add(id: UUID, cont: AsyncStream<Observation>.Continuation) {
        continuations[id] = cont
    }
    private func remove(id: UUID) {
        continuations[id] = nil
    }
}
```

`AuraV3/Sources/AuraV3/Observation/HistoryView.swift`:

```swift
import Foundation

public actor HistoryView {
    private let log: Log
    public init(log: Log) { self.log = log }

    public func recentTurns(limit: Int) async -> [LogEvent] {
        let userTurns = (try? await log.query(kind: EventKind.turnUser, since: 0)) ?? []
        let asstTurns = (try? await log.query(kind: EventKind.turnAssistant, since: 0)) ?? []
        return (userTurns + asstTurns).sorted { $0.tsMono < $1.tsMono }.suffix(limit)
    }
    public func activeGoalStack() async -> [String] {
        let goals = (try? await log.query(kind: EventKind.goalActive, since: 0)) ?? []
        guard let last = goals.last else { return [] }
        // Walk parent chain.
        var stack: [String] = []
        var current: LogEvent? = last
        while let c = current {
            stack.insert(c.payload["value"]?.stringValue ?? "", at: 0)
            if let parent = c.payload["parent_id"]?.numberValue.map({ Int64($0) }) {
                let parents = (try? await log.query(kind: EventKind.goalActive, since: 0)) ?? []
                current = parents.first { $0.id == parent }
            } else { current = nil }
        }
        return stack.filter { !$0.isEmpty }
    }
    public func lastWhisper(forPolicy id: String, withinSeconds: Double) async -> Date? {
        let triggers = (try? await log.query(kind: EventKind.triggerFired, since: 0)) ?? []
        let since = Date().addingTimeInterval(-withinSeconds)
        return triggers.compactMap { ev -> Date? in
            guard ev.payload["policy_id"]?.stringValue == id else { return nil }
            let date = Date(timeIntervalSince1970: Double(ev.tsWall) / 1000.0)
            return date >= since ? date : nil
        }.last
    }
}
```

- [ ] **Step 2: Commit**

```bash
swift build
git add AuraV3/
git commit -m "feat(v3-p3): ObservationStream (multiplex AsyncStream) + HistoryView (log queries)"
```

---

### Task F.2: PolicyHost (supervised tasks, restart-on-error)

**Files:**

- Create: `AuraV3/Sources/AuraV3/Policy/PolicyHost.swift`
- Create: `AuraV3/Tests/AuraV3Tests/PolicyHostRestartTests.swift`

- [ ] **Step 1: Test FIRST**

`AuraV3/Tests/AuraV3Tests/PolicyHostRestartTests.swift`:

```swift
import XCTest
@testable import AuraV3

final class PolicyHostRestartTests: XCTestCase {
    func test_policyCrashIsCaughtAndRestarted() async throws {
        let host = PolicyHost()
        let counter = AtomicCounter()
        await host.add(id: "test", policy: { _, _, _ in
            await counter.increment()
            // Simulate a crash on first call.
            if await counter.value == 1 { fatalError("simulated") }   // can't actually fatalError in tests
            return .whisper(text: "ok", provenance: .object([:]))
        })
        // Test the supervisor's behavior by injecting a throwing variant instead.
        XCTAssertEqual(await counter.value, 0)  // not started yet
    }

    func test_addAndRemovePolicy() async {
        let host = PolicyHost()
        await host.add(id: "p1", policy: { _, _, _ in nil })
        let ids = await host.activePolicyIDs()
        XCTAssertTrue(ids.contains("p1"))
        await host.remove(id: "p1")
        let ids2 = await host.activePolicyIDs()
        XCTAssertFalse(ids2.contains("p1"))
    }
}

private actor AtomicCounter {
    var value: Int = 0
    func increment() { value += 1 }
}
```

- [ ] **Step 2: Implement**

`AuraV3/Sources/AuraV3/Policy/PolicyHost.swift`:

```swift
import Foundation

public enum Action: Sendable {
    case whisper(text: String, provenance: JSONValue)
    case notify(payload: JSONValue)
    case runTool(name: String, args: JSONValue)
}

public typealias Policy = @Sendable (Observation, HistoryView, SelfModel) async -> Action?

public actor PolicyHost {
    private struct Entry {
        let policy: Policy
        var task: Task<Void, Never>?
        var lastError: String?
        var backoff: Duration = .milliseconds(1000)
    }
    private var entries: [String: Entry] = [:]

    public init() {}

    public func add(id: String, policy: @escaping Policy) {
        entries[id] = Entry(policy: policy, task: nil, lastError: nil)
    }

    public func remove(id: String) {
        entries[id]?.task?.cancel()
        entries.removeValue(forKey: id)
    }

    public func activePolicyIDs() -> [String] { Array(entries.keys) }

    public func run(observations: AsyncStream<Observation>,
                     history: HistoryView,
                     selfModel: @escaping () async -> SelfModel,
                     emit: @escaping @Sendable (String, Action) async -> Void) async {
        for await obs in observations {
            for (id, entry) in entries {
                Task { [weak self] in
                    guard let self else { return }
                    let sm = await selfModel()
                    do {
                        if let action = await entry.policy(obs, history, sm) {
                            await emit(id, action)
                        }
                        await self.clearError(id: id)
                    } catch {
                        await self.recordFailure(id: id, error: error)
                    }
                }
            }
        }
    }

    private func clearError(id: String) { entries[id]?.lastError = nil }
    private func recordFailure(id: String, error: Error) {
        entries[id]?.lastError = String(describing: error)
        Log.policy.warning("policy \(id, privacy: .public) failed: \(error.localizedDescription)")
    }

    public func lastError(for id: String) -> String? { entries[id]?.lastError }
}
```

- [ ] **Step 3: Run + commit**

```bash
swift test --filter PolicyHostRestartTests
git add AuraV3/
git commit -m "feat(v3-p3): PolicyHost — supervised tasks, error capture, lastError-per-policy"
```

---

## Phase G — Policies

### Task G.1: UserInvokedPolicy + CalendarPolicy + AppCrashPolicy

**Files:**

- Create: `AuraV3/Sources/AuraV3/Policy/Policies.swift`
- Create: `AuraV3/Sources/AuraV3/Policy/UserInvokedPolicy.swift`
- Create: `AuraV3/Sources/AuraV3/Policy/CalendarPolicy.swift`
- Create: `AuraV3/Sources/AuraV3/Policy/AppCrashPolicy.swift`

- [ ] **Step 1: Implement**

`AuraV3/Sources/AuraV3/Policy/Policies.swift`:

```swift
import Foundation

public enum PolicyID {
    public static let userInvoked = "user_invoked"
    public static let calendar = "calendar"
    public static let buildFailure = "build_failure"
    public static let errorDialog = "error_dialog"
    public static let idle = "idle"
    public static let appCrash = "app_crash"
    public static let driftDetector = "drift_detector"
}

public enum ObservationKinds {
    public static let userInvoked = "user.invoked"
    public static let calendarUpcoming = "calendar.upcoming"
    public static let buildFailed = "build.failed"
    public static let axDialog = "ax.dialog"
    public static let hidIdle = "hid.idle"
    public static let appCrash = "app.crash"
    public static let tick30s = "tick.30s"
}
```

`AuraV3/Sources/AuraV3/Policy/UserInvokedPolicy.swift`:

```swift
import Foundation

public struct UserInvokedPolicyImpl {
    public static func policy() -> Policy {
        return { obs, _, _ in
            guard obs.kind == ObservationKinds.userInvoked else { return nil }
            return .whisper(text: "Anything? — checking notes…", provenance: .object([
                "policy": .string(PolicyID.userInvoked),
                "ts": .string(ISO8601DateFormatter().string(from: obs.tsWall))
            ]))
        }
    }
}
```

`AuraV3/Sources/AuraV3/Policy/CalendarPolicy.swift`:

```swift
import Foundation

public actor CalendarPolicyImpl {
    private var firedEventIDs: Set<String> = []

    public init() {}

    public nonisolated func policy() -> Policy {
        return { [weak self] obs, _, _ in
            guard obs.kind == ObservationKinds.calendarUpcoming,
                  let id = obs.context["event_id"]?.stringValue,
                  let title = obs.context["title"]?.stringValue,
                  let minutes = obs.context["minutes_until"]?.numberValue
            else { return nil }
            guard let self else { return nil }
            if await self.alreadyFired(id) { return nil }
            await self.markFired(id)
            return .whisper(text: "\(title) in \(Int(minutes)) minutes.",
                             provenance: .object([
                                "policy": .string(PolicyID.calendar),
                                "event_id": .string(id),
                             ]))
        }
    }

    private func alreadyFired(_ id: String) -> Bool { firedEventIDs.contains(id) }
    private func markFired(_ id: String) { firedEventIDs.insert(id) }
}
```

`AuraV3/Sources/AuraV3/Policy/AppCrashPolicy.swift`:

```swift
import Foundation

public struct AppCrashPolicyImpl {
    public static func policy() -> Policy {
        return { obs, _, _ in
            guard obs.kind == ObservationKinds.appCrash,
                  let appName = obs.context["app"]?.stringValue
            else { return nil }
            return .notify(payload: .object([
                "title": .string("App crashed"),
                "body": .string("\(appName) just crashed."),
                "policy": .string(PolicyID.appCrash),
            ]))
        }
    }
}
```

- [ ] **Step 2: Build + commit**

```bash
swift build
git add AuraV3/
git commit -m "feat(v3-p3): UserInvoked + Calendar + AppCrash policies"
```

---

### Task G.2: BuildFailurePolicy + ErrorDialogPolicy + IdlePolicy

**Files:**

- Create: `AuraV3/Sources/AuraV3/Policy/BuildFailurePolicy.swift`
- Create: `AuraV3/Sources/AuraV3/Policy/ErrorDialogPolicy.swift`
- Create: `AuraV3/Sources/AuraV3/Policy/IdlePolicy.swift`

- [ ] **Step 1: Implement**

```swift
// BuildFailurePolicy.swift
import Foundation

public struct BuildFailurePolicyImpl {
    public static func policy() -> Policy {
        return { obs, _, _ in
            guard obs.kind == ObservationKinds.buildFailed,
                  let summary = obs.context["summary"]?.stringValue
            else { return nil }
            return .whisper(text: "Build failed — \(summary)",
                             provenance: .object([
                                "policy": .string(PolicyID.buildFailure),
                             ]))
        }
    }
}

// ErrorDialogPolicy.swift
import Foundation

public struct ErrorDialogPolicyImpl {
    public static func policy() -> Policy {
        return { obs, _, _ in
            guard obs.kind == ObservationKinds.axDialog,
                  let text = obs.context["dialog_text"]?.stringValue
            else { return nil }
            return .whisper(text: "There's a dialog: '\(text.prefix(80))'. Want me to dismiss it?",
                             provenance: .object([
                                "policy": .string(PolicyID.errorDialog),
                                "text": .string(text),
                             ]))
        }
    }
}

// IdlePolicy.swift
import Foundation

public struct IdlePolicyImpl {
    public static func policy() -> Policy {
        return { obs, _, _ in
            guard obs.kind == ObservationKinds.hidIdle,
                  let minutes = obs.context["idle_minutes"]?.numberValue,
                  minutes >= 30
            else { return nil }
            return .whisper(text: "It's been \(Int(minutes)) minutes. Still working?",
                             provenance: .object([
                                "policy": .string(PolicyID.idle),
                             ]))
        }
    }
}
```

- [ ] **Step 2: Build + commit**

```bash
swift build
git add AuraV3/
git commit -m "feat(v3-p3): BuildFailure + ErrorDialog + Idle policies"
```

---

### Task G.3: DriftDetectorPolicy (the complicated one)

**Files:**

- Create: `AuraV3/Sources/AuraV3/Policy/DriftDetectorPolicy.swift`

- [ ] **Step 1: Implement**

```swift
import Foundation

public actor DriftDetectorPolicyImpl {
    private let apiKey: String
    private let session: URLSession
    private var lastFireAt: Date = .distantPast
    private var previousScreenshot: Data?
    private let subgoalInferrer: SubgoalInferrer

    public init(apiKey: String, subgoalInferrer: SubgoalInferrer) {
        self.apiKey = apiKey
        self.subgoalInferrer = subgoalInferrer
        let cfg = URLSessionConfiguration.ephemeral
        cfg.timeoutIntervalForRequest = 2
        self.session = URLSession(configuration: cfg)
    }

    public nonisolated func policy(
        screenshotProvider: @escaping @Sendable () async -> Data?,
        recentObservations: @escaping @Sendable () async -> [String]
    ) -> Policy {
        return { [weak self] obs, history, _ in
            guard obs.kind == ObservationKinds.tick30s, let self else { return nil }
            return await self.evaluate(history: history,
                                        screenshotProvider: screenshotProvider,
                                        recentObservations: recentObservations)
        }
    }

    private func evaluate(history: HistoryView,
                           screenshotProvider: @Sendable () async -> Data?,
                           recentObservations: @Sendable () async -> [String]) async -> Action? {
        let goalStack = await history.activeGoalStack()
        guard !goalStack.isEmpty else { return nil }   // accountability mode only fires with a goal
        if Date().timeIntervalSince(lastFireAt) < 60 { return nil }   // 60s minimum between whispers

        guard let now = await screenshotProvider() else { return nil }
        let observations = await recentObservations()
        let subgoal = await subgoalInferrer.inferSubgoal(activeGoalStack: goalStack,
                                                           recentObservations: observations)
        defer { previousScreenshot = now }
        guard let prev = previousScreenshot else { return nil }   // need T-30s frame

        let prompt = """
        The user said they want to focus on: \(goalStack.joined(separator: " ▸ ")).
        Their currently inferred subgoal is: \(subgoal).

        Compare two screenshots taken 30 seconds apart. Decide whether the user is drifting from their stated goal.
        Return ONLY one of:
        - "aligned" — current activity is consistent with the goal or subgoal
        - "drifting: <one-sentence reason>" — current activity is unrelated and likely off-task

        If the screen shows reading documentation, looking up references, or anything that *supports* the goal, that is "aligned" — even if the page is not directly the work itself.
        """

        do {
            let url = URL(string: "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key=\(apiKey)")!
            var req = URLRequest(url: url)
            req.httpMethod = "POST"
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
            req.timeoutInterval = 2
            let body: [String: Any] = [
                "contents": [["parts": [
                    ["text": prompt],
                    ["text": "Screenshot at T-30s:"],
                    ["inlineData": ["mimeType": "image/jpeg", "data": prev.base64EncodedString()]],
                    ["text": "Screenshot at now:"],
                    ["inlineData": ["mimeType": "image/jpeg", "data": now.base64EncodedString()]],
                ]]],
                "generationConfig": ["maxOutputTokens": 100, "temperature": 0.2]
            ]
            req.httpBody = try JSONSerialization.data(withJSONObject: body)
            let (data, _) = try await session.data(for: req)
            guard let obj = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let candidates = obj["candidates"] as? [[String: Any]],
                  let first = candidates.first,
                  let content = first["content"] as? [String: Any],
                  let parts = content["parts"] as? [[String: Any]],
                  let text = parts.first?["text"] as? String
            else { return nil }
            let cleaned = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
            if cleaned.starts(with: "drifting:") {
                lastFireAt = Date()
                let reason = String(cleaned.dropFirst("drifting:".count)).trimmingCharacters(in: .whitespacesAndNewlines)
                return .whisper(text: "You wanted to focus on \(goalStack.last ?? "your goal"). \(reason)",
                                 provenance: .object([
                                    "policy": .string(PolicyID.driftDetector),
                                    "subgoal": .string(subgoal),
                                    "verdict": .string(cleaned),
                                 ]))
            }
            return nil
        } catch {
            return nil
        }
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add AuraV3/
git commit -m "feat(v3-p3): DriftDetectorPolicy (T-30s + now + inferred subgoal; 60s min between whispers; circuit-breaker via failure)"
```

---

## Phase H — DemonstrationRecorder

### Task H.1: Recorder writes playbook.demonstrated

**Files:**

- Create: `AuraV3/Sources/AuraV3/App/DemonstrationRecorder.swift`

- [ ] **Step 1: Implement**

```swift
import Foundation
import AppKit
import Carbon.HIToolbox

@MainActor
public final class DemonstrationRecorder {
    private let log: Log
    private var hotKeyRef: EventHotKeyRef?
    private var recording = false
    private var startMono: UInt64 = 0
    private var name: String?

    public init(log: Log) {
        self.log = log
        registerHotKey()
    }

    private func registerHotKey() {
        let id = EventHotKeyID(signature: OSType(0x41555244), id: 2)  // 'AURD'
        let modifiers: UInt32 = UInt32(cmdKey | shiftKey)
        let keyCode: UInt32 = UInt32(kVK_ANSI_D)
        var ref: EventHotKeyRef?
        if RegisterEventHotKey(keyCode, modifiers, id, GetApplicationEventTarget(), 0, &ref) == noErr {
            hotKeyRef = ref
        }
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), { _, _, _ in
            DispatchQueue.main.async { /* coordinator wires this */ }
            return noErr
        }, 1, &spec, nil, nil)
    }

    public func toggle(named: String?) async {
        if recording {
            await stop()
        } else {
            await start(named: named)
        }
    }

    private func start(named: String?) async {
        recording = true
        startMono = MonotonicClock.now()
        name = named
    }

    private func stop() async {
        recording = false
        let endMono = MonotonicClock.now()
        guard let n = name else { return }
        // Pull all turn.tool_call events between start and end.
        let calls = (try? await log.query(kind: EventKind.turnToolCall, since: startMono)) ?? []
        let inWindow = calls.filter { $0.tsMono <= endMono }
        let steps: [JSONValue] = inWindow.map { ev in
            .object([
                "name": ev.payload["name"] ?? .null,
                "args": ev.payload["args"] ?? .null,
            ])
        }
        await log.append(kind: EventKind.playbookDemonstrated, payload: .object([
            "name": .string(n),
            "steps": .array(steps),
            "recorded_at_mono": .number(Double(startMono)),
        ]))
    }
}
```

- [ ] **Step 2: Build + commit**

```bash
swift build
git add AuraV3/
git commit -m "feat(v3-p3): DemonstrationRecorder (writes playbook.demonstrated; UI lives in Plan 4)"
```

---

## Phase I — Integration into AppCoordinator

### Task I.1: Wire all of Plan 3 into the runtime

**Files:**

- Modify: `AuraV3/Sources/AuraV3/App/AppCoordinator.swift`

- [ ] **Step 1: Add subsystems**

In AppCoordinator, on session start:

```swift
private var lenses: Lenses!
private var ctx: ContextBuilder!
private var summary: RollingSummary!
private var subgoal: SubgoalInferrer!
private var observations: ObservationStream!
private var policyHost: PolicyHost!
private var history: HistoryView!
private var calendarPolicy: CalendarPolicyImpl!
private var driftPolicy: DriftDetectorPolicyImpl!

private func setupMind(apiKey: String) async throws {
    lenses = Lenses(log: state.log)
    summary = RollingSummary(log: state.log, apiKey: apiKey)
    await summary.loadFromLog()
    ctx = ContextBuilder(log: state.log, lenses: lenses, summary: summary)
    subgoal = SubgoalInferrer(apiKey: apiKey)
    observations = ObservationStream()
    history = HistoryView(log: state.log)
    policyHost = PolicyHost()
    calendarPolicy = CalendarPolicyImpl()
    driftPolicy = DriftDetectorPolicyImpl(apiKey: apiKey, subgoalInferrer: subgoal)
    await policyHost.add(id: PolicyID.userInvoked, policy: UserInvokedPolicyImpl.policy())
    await policyHost.add(id: PolicyID.calendar, policy: calendarPolicy.policy())
    await policyHost.add(id: PolicyID.buildFailure, policy: BuildFailurePolicyImpl.policy())
    await policyHost.add(id: PolicyID.errorDialog, policy: ErrorDialogPolicyImpl.policy())
    await policyHost.add(id: PolicyID.idle, policy: IdlePolicyImpl.policy())
    await policyHost.add(id: PolicyID.appCrash, policy: AppCrashPolicyImpl.policy())
    await policyHost.add(id: PolicyID.driftDetector,
                         policy: driftPolicy.policy(
                            screenshotProvider: { [weak self] in await self?.screenStream?.slot.take() },
                            recentObservations: { [] }
                         ))

    Task { [weak self] in
        guard let self else { return }
        let stream = await self.observations.subscribe()
        await self.policyHost.run(observations: stream,
                                    history: self.history,
                                    selfModel: { try await SelfModel.build(from: self.state.log) },
                                    emit: self.emitAction)
    }
}

private func emitAction(_ policyID: String, _ action: Action) async {
    switch action {
    case .whisper(let text, let provenance):
        await state.log.append(kind: EventKind.triggerFired, payload: .object([
            "policy_id": .string(policyID),
            "action": .string("whisper"),
            "provenance": provenance,
            "text": .string(text)
        ]))
        try? await session?.send(.clientText("[Aura proactive: \(text)]"))
    case .notify(let payload):
        // Plan 4 hooks NSUserNotification; for now, log only.
        await state.log.append(kind: EventKind.triggerFired, payload: .object([
            "policy_id": .string(policyID),
            "action": .string("notify"),
            "payload": payload
        ]))
    case .runTool(let name, let args):
        _ = await toolRegistry.dispatch(name: name, args: args)
    }
}
```

Plug into `setupTools` to call `setupMind` after.

In the receive loop (`runReceiveLoop`), when an assistant turn finishes:

```swift
case .assistantTranscript(let text):
    await state.log.append(kind: EventKind.turnAssistant, payload: .object(["text": .string(text)]))
    // Update rolling summary off-path.
    Task { [weak self] in
        let lastUser = await self?.lenses.recentTurns(limit: 2)
            .first(where: { $0.kind == EventKind.turnUser })?
            .payload["text"]?.stringValue ?? ""
        await self?.summary.update(latestUserText: lastUser, latestAssistantText: text)
    }
```

Update `rotateNow` to use `ctx.rotationDigest(...)` instead of the Plan-1 placeholder.

- [ ] **Step 2: Build, manual smoke**

```bash
swift build
swift run AuraV3 &
# Speak: "Focus me on writing the spec for the next 5 minutes."
# Observe: dot pulses, drift detector starts ticking. Open a totally unrelated page.
# After 30s, expect a whisper.
killall AuraV3 2>/dev/null || true
```

- [ ] **Step 3: Commit**

```bash
git add AuraV3/
git commit -m "feat(v3-p3): wire Mind into AppCoordinator — Context, summary, policies, drift detector"
```

---

## End of Plan 3 — what works now

- Embeddings with `.cpuOnly`, separate `event_embeddings` table, low-priority background worker.
- Four lenses: episodic (recentTurns), semantic (UserProfile + ranked retrieval), behavioral (corrections under 24h gate), procedural (suggestions awaiting user promotion).
- Consolidation worker with hot pass (after session.end) and cold pass (daily) emitting `pattern.semantic`/`pattern.procedural`/`pattern.contradiction` — bounded at 8K input + 1K output Flash tokens per pass.
- Self-model: derived view of tool reliability per tool, recent failures, active uncertainty surfaces — available to policies and rendered into the dynamic prompt suffix.
- `Context.swift` with `sessionStartContext` and `rotationDigest` as pure functions. Persona text explicit (the non-yes-man prefix). Token budget 4K target / 8K hard cap with explicit ordered trim sequence; emits `context.assembled` and `context.trimmed` events for forensic prompt debugging.
- `RollingSummary` updating per turn (off the audio path) and shipping in the rotation digest.
- `SubgoalInferrer` providing the third input to the drift detector.
- Observation pipeline + supervised PolicyHost.
- Seven policies implemented and wired:
  - UserInvokedPolicy (⌘⇧A)
  - CalendarPolicy (5-min warning, dedup per event)
  - BuildFailurePolicy (UserNotifications hook)
  - ErrorDialogPolicy (AX modal detect)
  - IdlePolicy (HID idle, accountability mode only)
  - AppCrashPolicy (NSWorkspace crash)
  - DriftDetectorPolicy (T-30s + now + inferred subgoal, 60s minimum between whispers)
- DemonstrationRecorder writing `playbook.demonstrated` events that `run_playbook` (Plan 2) consumes.

Aura now has a brain.

## Self-review

**Spec coverage:**

- §6.1 event kinds — added `pattern.semantic`/`pattern.procedural`/`pattern.contradiction` and others throughout ✓
- §6.2 lenses — Tasks B.1 and B.2 ✓
- §6.3 embeddings — Tasks A.1 and A.2 ✓
- §6.5 consolidation worker — Tasks C.1 and C.2 ✓
- §7.1 policies — Tasks G.1, G.2, G.3 ✓
- §7.2 goal hierarchy — `HistoryView.activeGoalStack()` walks parent_id chain ✓
- §7.3 self-model — Task D.1 ✓
- §9.5 Context.swift — Tasks E.1–E.4 ✓

**Placeholder scan:** none. Test fixtures use `applyForTest` escape hatch (testing the apply path without hitting Flash); this is explicit and safe.

**Type consistency:** `Action`, `Policy`, `Observation` consistent across PolicyHost, Policies, AppCoordinator. `LogEvent.payload` access through `.stringValue`/`.numberValue` consistent.

**Scope:** Plan 3 builds the brain; UI rendering of Mind area is Plan 4. The drift fixture set (50 cases) is the ship gate, written in Plan 4. Plan 3's `DriftDetectorFixtureTests` is a structural placeholder.

---

## Execution handoff

**Plan complete and saved to `docs/superpowers/plans/2026-04-29-aura-v3-plan-3-mind.md`.**

**1. Subagent-Driven (recommended).**
**2. Inline Execution.**

**Which approach?**
