import Foundation

// MARK: - LogEvent (read model)

public struct LogEvent: Sendable, Equatable {
    public let id: Int64
    public let tsWall: Int64
    public let tsMono: UInt64
    public let kind: String
    public let payload: JSONValue
    public let sessionID: String?
    public let correlationID: String?
    public let geminiCallID: String?       // Amendment D
    public let tombstoneFor: Int64?
}

// MARK: - Log actor

/// Append-only event log backed by SQLite WAL.
///
/// Writes are serialized through a bounded AsyncChannel writer Task (Amendment G drop policy).
/// Reads are satisfied directly on the actor (no queue) — they are reads, not writes.
/// `flush()` uses a barrier sentinel (Amendment Q) — no sleep loop.
///
/// Thread safety: All mutable state is actor-isolated. The writer Task is detached so
/// that it does not hold the actor across the AsyncChannel's next() suspension point —
/// which would block callers of query/queryCorrelation while the writer is idle.
public actor Log {

    // MARK: - Internal types

    private struct PendingEvent: Sendable {
        let tsWall: Int64
        let tsMono: UInt64
        let kind: String
        let payload: JSONValue
        let sessionID: String?
        let correlationID: String?
        let geminiCallID: String?           // Amendment D
        let tombstoneFor: Int64?
    }

    /// Amendment Q: WriteRequest is an enum so flush() can inject a barrier sentinel.
    private enum WriteRequest: Sendable {
        case write(PendingEvent, CheckedContinuation<Int64, Error>?)
        case barrier(CheckedContinuation<Void, Error>)

        /// Returns the event kind for drop-policy evaluation.
        /// Barriers use the synthetic kind "_barrier" which never matches priority tiers.
        func kindForPolicy() -> String {
            switch self {
            case .write(let e, _): return e.kind
            case .barrier:         return "_barrier"
            }
        }

        /// True if this request has a continuation that must eventually be
        /// resumed. The drop policy MUST not return `.dropIncoming` for these
        /// — the caller is awaiting and would deadlock indefinitely.
        var awaitsCompletion: Bool {
            switch self {
            case .write(_, let cont):  return cont != nil
            case .barrier:             return true
            }
        }
    }

    // MARK: - State

    private let db: SQLite
    private let path: String
    private let writeQueue: AsyncChannel<WriteRequest>
    private var writerTask: Task<Void, Never>?
    private var checkpointTask: Task<Void, Never>?

    // MARK: - Open

    /// Amendment J: synchronous open — DB creation and schema setup are blocking C calls.
    /// The drop-policy install and writer Task are fire-and-forget via `bootstrap()`.
    public static func open(path: String) throws -> Log {
        let db = try SQLite.open(path: path)
        // Amendment D: schema includes gemini_call_id column.
        try db.exec("""
            CREATE TABLE IF NOT EXISTS events(
              id              INTEGER PRIMARY KEY,
              ts_wall         INTEGER NOT NULL,
              ts_mono         INTEGER NOT NULL,
              kind            TEXT NOT NULL,
              payload         TEXT NOT NULL,
              session_id      TEXT,
              correlation_id  TEXT,
              gemini_call_id  TEXT,
              tombstone_for   INTEGER REFERENCES events(id)
            );
        """)
        try db.exec(
            "CREATE INDEX IF NOT EXISTS events_ts_mono ON events(ts_mono);"
        )
        try db.exec(
            "CREATE INDEX IF NOT EXISTS events_kind ON events(kind);"
        )
        try db.exec(
            "CREATE INDEX IF NOT EXISTS events_correlation ON events(correlation_id) WHERE correlation_id IS NOT NULL;"
        )
        // Amendment D: partial index on gemini_call_id.
        try db.exec(
            "CREATE INDEX IF NOT EXISTS events_gemini_call ON events(gemini_call_id) WHERE gemini_call_id IS NOT NULL;"
        )
        try db.exec(
            "CREATE INDEX IF NOT EXISTS events_tombstone ON events(tombstone_for);"
        )
        try db.exec("""
            CREATE VIRTUAL TABLE IF NOT EXISTS events_fts USING fts5(
              payload, content='events', content_rowid='id'
            );
        """)

        let log = Log(db: db, path: path)
        // Fire-and-forget: installDropPolicy runs before startWriter (FIFO within the actor).
        // The AsyncChannel buffers up to 4096 events; nothing fills that in the bootstrap gap.
        Task { await log.bootstrap() }
        return log
    }

    /// Runs drop-policy install then starts the writer, both on the actor, in order.
    private func bootstrap() async {
        await installDropPolicy()
        startWriter()
    }

    private init(db: SQLite, path: String) {
        self.db = db
        self.path = path
        // Capacity 4096 with custom drop decision installed after init.
        self.writeQueue = AsyncChannel(capacity: 4096, dropPolicy: .never)
    }

    // MARK: - Drop policy (Amendment G)

    private func installDropPolicy() async {
        await writeQueue.setDropDecision { req, buffered in
            let incoming = req.kindForPolicy()

            // Find a victim to evict (low-priority kinds, oldest first):
            //   state.observation  →  pattern.*  →  debug.*
            if let idx = buffered.firstIndex(where: {
                $0.kindForPolicy() == EventKind.stateObservation
            }) {
                return .replaceAtIndex(idx)
            }
            if let idx = buffered.firstIndex(where: {
                $0.kindForPolicy().hasPrefix("pattern.")
            }) {
                return .replaceAtIndex(idx)
            }
            if let idx = buffered.firstIndex(where: {
                $0.kindForPolicy().hasPrefix("debug.")
            }) {
                return .replaceAtIndex(idx)
            }

            // No low-priority victim available. Decide: block or drop?
            //
            // (a) Anything that has a caller-waiting continuation MUST block.
            //     `appendAndReturnID` and `flush` use continuations; dropping
            //     would leak the continuation and the caller would await
            //     forever. This is the bug the prior FIXME flagged — fixed.
            // (b) Critical kinds block: turn.*, effect.*, correction*, fact,
            //     tombstone — losing these is unacceptable per spec §6.1.
            // (c) Everything else (state.observation already considered above,
            //     unknown kinds) drops.
            if req.awaitsCompletion {
                return .blockUntilSpace
            }
            if incoming.hasPrefix("turn.")
                || incoming.hasPrefix("effect.")
                || incoming.hasPrefix("correction")
                || incoming == EventKind.fact
                || incoming == EventKind.tombstone {
                return .blockUntilSpace
            }
            return .dropIncoming
        }
    }

    // MARK: - Writer Task

    /// Starts the background writer as a *detached* Task.
    ///
    /// Detached so the for-await suspension on writeQueue.next() does NOT hold the Log actor.
    /// Each element is processed by calling back into the actor via `processRequest(_:)`.
    private func startWriter() {
        let queue = writeQueue  // capture the actor-isolated property now, on the actor
        writerTask = Task.detached { [weak self] in
            guard let self else { return }
            for await req in queue {
                await self.processRequest(req)
            }
        }
    }

    /// Executes one WriteRequest on the actor (serialised, no concurrency within processRequest).
    private func processRequest(_ req: WriteRequest) {
        switch req {
        case .write(let event, let cont):
            do {
                let id = try write(event)
                cont?.resume(returning: id)
            } catch {
                cont?.resume(throwing: error)
            }
        case .barrier(let cont):
            // All preceding .write requests have already been processed (FIFO queue).
            cont.resume()
        }
    }

    // MARK: - Public write API

    /// Fire-and-forget append. Returns immediately; write is queued asynchronously.
    ///
    /// - Note: `appendAndReturnID` should only be used for critical kinds (turn.*, effect.*,
    ///   correction*, fact, tombstone). Non-critical kinds may be dropped by the drop policy,
    ///   which would never resume the continuation — causing the caller to await forever.
    public func append(
        kind: String,
        sessionID: String? = nil,
        correlationID: String? = nil,
        geminiCallID: String? = nil,
        tombstoneFor: Int64? = nil,
        payload: JSONValue
    ) async {
        precondition(EventKind.isWellFormed(kind))
        let event = PendingEvent(
            tsWall: Int64(Date().timeIntervalSince1970 * 1000),
            tsMono: MonotonicClock.now(),
            kind: kind,
            payload: payload,
            sessionID: sessionID,
            correlationID: correlationID,
            geminiCallID: geminiCallID,
            tombstoneFor: tombstoneFor
        )
        await writeQueue.send(.write(event, nil))
    }

    /// Queues a write and returns the new row's `id`.
    ///
    /// Amendment C: uses `withCheckedThrowingContinuation` — no `unsafeBitCast` UB.
    ///
    /// The drop policy detects continuation-bearing requests via
    /// `WriteRequest.awaitsCompletion` and forces `.blockUntilSpace`, so this
    /// can be called for any kind without the prior continuation-leak risk.
    public func appendAndReturnID(
        kind: String,
        sessionID: String? = nil,
        correlationID: String? = nil,
        geminiCallID: String? = nil,
        payload: JSONValue
    ) async throws -> Int64 {
        precondition(EventKind.isWellFormed(kind))
        return try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Int64, Error>) in
            let event = PendingEvent(
                tsWall: Int64(Date().timeIntervalSince1970 * 1000),
                tsMono: MonotonicClock.now(),
                kind: kind,
                payload: payload,
                sessionID: sessionID,
                correlationID: correlationID,
                geminiCallID: geminiCallID,
                tombstoneFor: nil
            )
            Task { await self.writeQueue.send(.write(event, cont)) }
        }
    }

    /// Append with explicit timestamps — for testing ordering invariants.
    public func appendRaw(
        kind: String,
        tsWall: Int64,
        tsMono: UInt64,
        payload: JSONValue
    ) async {
        precondition(EventKind.isWellFormed(kind))
        let event = PendingEvent(
            tsWall: tsWall,
            tsMono: tsMono,
            kind: kind,
            payload: payload,
            sessionID: nil,
            correlationID: nil,
            geminiCallID: nil,
            tombstoneFor: nil
        )
        await writeQueue.send(.write(event, nil))
    }

    /// Amendment Q: barrier-based flush.
    ///
    /// Inserts a `.barrier` sentinel into the write queue. Because the queue is FIFO,
    /// when the writer processes the barrier all preceding writes have completed.
    /// SQLite WAL durability is per-write; the barrier only guarantees queue ordering.
    public func flush() async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            Task { await self.writeQueue.send(.barrier(cont)) }
        }
    }

    // MARK: - Write (sync, actor-isolated)

    /// Amendment E: sets PRAGMA synchronous based on event-kind durability tier before each INSERT.
    private func write(_ event: PendingEvent) throws -> Int64 {
        // Critical events require full durability; others accept the WAL default (NORMAL).
        let tier: String
        if event.kind.hasPrefix("turn.")
            || event.kind.hasPrefix("effect.")
            || event.kind.hasPrefix("correction") {
            tier = "FULL"
        } else {
            tier = "NORMAL"
        }
        try db.exec("PRAGMA synchronous = \(tier);")

        let payloadJSON: String
        if let data = try? JSONEncoder().encode(event.payload),
           let str = String(data: data, encoding: .utf8) {
            payloadJSON = str
        } else {
            payloadJSON = "null"
        }

        // Amendment D: 8 bound columns (id auto-assigned).
        // Indices: ts_wall=1, ts_mono=2, kind=3, payload=4,
        //          session_id=5, correlation_id=6, gemini_call_id=7, tombstone_for=8
        let stmt = try db.prepare("""
            INSERT INTO events
              (ts_wall, ts_mono, kind, payload, session_id, correlation_id, gemini_call_id, tombstone_for)
            VALUES (?,?,?,?,?,?,?,?);
        """)
        defer { stmt.finalize() }

        stmt.bind(1, event.tsWall)
        stmt.bind(2, Int64(bitPattern: event.tsMono))
        stmt.bind(3, event.kind)
        stmt.bind(4, payloadJSON)

        if let sid = event.sessionID     { stmt.bind(5, sid)        } else { stmt.bindNull(5) }
        if let cid = event.correlationID { stmt.bind(6, cid)        } else { stmt.bindNull(6) }
        if let gid = event.geminiCallID  { stmt.bind(7, gid)        } else { stmt.bindNull(7) }
        if let tf  = event.tombstoneFor  { stmt.bind(8, tf)         } else { stmt.bindNull(8) }

        _ = try stmt.step()
        return db.lastInsertRowID()
    }

    // MARK: - Read API (actor-isolated, no queue needed)

    /// Returns events of `kind` with `ts_mono >= since`, excluding tombstoned rows.
    public func query(kind: String, since: UInt64) async throws -> [LogEvent] {
        let stmt = try db.prepare("""
            SELECT id, ts_wall, ts_mono, kind, payload,
                   session_id, correlation_id, gemini_call_id, tombstone_for
            FROM events
            WHERE kind = ?
              AND ts_mono >= ?
              AND tombstone_for IS NULL
              AND id NOT IN (
                SELECT tombstone_for FROM events WHERE tombstone_for IS NOT NULL
              )
            ORDER BY ts_mono ASC;
        """)
        defer { stmt.finalize() }
        stmt.bind(1, kind)
        stmt.bind(2, Int64(bitPattern: since))
        var out: [LogEvent] = []
        while try stmt.step() { out.append(try Self.readRow(stmt)) }
        return out
    }

    /// Returns all events sharing the given `correlationID`, ordered by `ts_mono`.
    public func queryCorrelation(_ cid: String) async throws -> [LogEvent] {
        let stmt = try db.prepare("""
            SELECT id, ts_wall, ts_mono, kind, payload,
                   session_id, correlation_id, gemini_call_id, tombstone_for
            FROM events
            WHERE correlation_id = ?
            ORDER BY ts_mono ASC;
        """)
        defer { stmt.finalize() }
        stmt.bind(1, cid)
        var out: [LogEvent] = []
        while try stmt.step() { out.append(try Self.readRow(stmt)) }
        return out
    }

    /// Amendment D: returns all events sharing the given `geminiCallID`, ordered by `ts_mono`.
    public func queryGeminiCallID(_ id: String) async throws -> [LogEvent] {
        let stmt = try db.prepare("""
            SELECT id, ts_wall, ts_mono, kind, payload,
                   session_id, correlation_id, gemini_call_id, tombstone_for
            FROM events
            WHERE gemini_call_id = ?
            ORDER BY ts_mono ASC;
        """)
        defer { stmt.finalize() }
        stmt.bind(1, id)
        var out: [LogEvent] = []
        while try stmt.step() { out.append(try Self.readRow(stmt)) }
        return out
    }

    // MARK: - Row reader

    /// Column map (0-indexed):
    ///   0=id, 1=ts_wall, 2=ts_mono, 3=kind, 4=payload,
    ///   5=session_id, 6=correlation_id, 7=gemini_call_id, 8=tombstone_for
    ///
    /// `Statement.text(at:)` returns "" for SQL NULL. We treat "" as nil for all
    /// optional string columns — none of session_id / correlation_id / gemini_call_id
    /// ever stores a legitimately-empty string in this schema.
    private static func readRow(_ stmt: Statement) throws -> LogEvent {
        let id       = try stmt.int(at: 0)
        let tsWall   = try stmt.int(at: 1)
        let tsMono   = UInt64(bitPattern: try stmt.int(at: 2))
        let kind     = try stmt.text(at: 3)
        let payloadStr = try stmt.text(at: 4)
        let payload: JSONValue
        if let data = payloadStr.data(using: .utf8),
           let val = try? JSONDecoder().decode(JSONValue.self, from: data) {
            payload = val
        } else {
            payload = .null
        }
        let session     = try stmt.text(at: 5)
        let correlation = try stmt.text(at: 6)
        let gemini      = try stmt.text(at: 7)
        let tombstoneRaw = try stmt.int(at: 8)

        return LogEvent(
            id: id,
            tsWall: tsWall,
            tsMono: tsMono,
            kind: kind,
            payload: payload,
            sessionID:     session.isEmpty     ? nil : session,
            correlationID: correlation.isEmpty ? nil : correlation,
            geminiCallID:  gemini.isEmpty      ? nil : gemini,
            // ts_mono is stored as Int64(bitPattern: UInt64) so 0 means the actual row 0
            // (impossible since tombstone_for references another row id ≥ 1).
            // The integer column is NULL for non-tombstone rows so stmt.int returns 0.
            tombstoneFor: tombstoneRaw == 0 ? nil : tombstoneRaw
        )
    }

    // MARK: - Lifecycle

    /// Stops accepting new writes, drains the queue, and closes the database.
    public func shutdown() async {
        checkpointTask?.cancel()
        checkpointTask = nil
        await writeQueue.finish()
        _ = await writerTask?.value
        db.close()
    }
}

// MARK: - WAL checkpoint (Amendment F)

extension Log {

    /// PASSIVE checkpoint — non-blocking, safe to call frequently.
    ///
    /// Readers and writers are not interrupted. SQLite moves pages from the WAL
    /// back into the database file opportunistically. This is the default
    /// periodic path (every 5 minutes via `startPeriodicCheckpoint()`).
    public func checkpoint() async throws {
        try db.exec("PRAGMA wal_checkpoint(PASSIVE);")
    }

    /// Conditional TRUNCATE checkpoint. Returns true if TRUNCATE ran.
    ///
    /// Gated on three conditions (Amendment F):
    ///  1. WAL file size > `walThresholdBytes` (default 64 MiB)
    ///  2. No `turn.user` event in the last 30 seconds
    ///  3. No `effect.posted` without a matching `effect.outcome` (in-flight effects)
    ///
    /// Playback state is NOT checked here — Task 10.1 wires it via AppCoordinator.
    /// Falls back to PASSIVE if any gate is not met (keeps WAL bounded over time).
    public func checkpointIfIdle(walThresholdBytes: Int64 = 64 * 1024 * 1024) async throws -> Bool {
        // Gate 1: WAL file size
        let walPath = walFilePath()
        let walSize: Int64
        if let attrs = try? FileManager.default.attributesOfItem(atPath: walPath),
           let size = attrs[.size] as? NSNumber {
            walSize = size.int64Value
        } else {
            walSize = 0
        }
        guard walSize >= walThresholdBytes else {
            try db.exec("PRAGMA wal_checkpoint(PASSIVE);")
            return false
        }

        // Gate 2: No recent turn.user (within last 30 seconds)
        let cutoff = MonotonicClock.now() &- UInt64(30 * 1_000_000_000)
        let hasRecentTurnUser: Bool = try {
            let stmt = try db.prepare("""
                SELECT 1 FROM events
                WHERE kind = 'turn.user' AND ts_mono >= ?
                LIMIT 1;
            """)
            defer { stmt.finalize() }
            stmt.bind(1, Int64(bitPattern: cutoff))
            return try stmt.step()
        }()
        if hasRecentTurnUser {
            try db.exec("PRAGMA wal_checkpoint(PASSIVE);")
            return false
        }

        // Gate 3: No in-flight effect (effect.posted without a matching effect.outcome)
        let hasInFlight: Bool = try {
            let stmt = try db.prepare("""
                SELECT 1 FROM events
                WHERE kind = 'effect.posted'
                  AND correlation_id IS NOT NULL
                  AND correlation_id NOT IN (
                    SELECT correlation_id FROM events
                    WHERE kind = 'effect.outcome' AND correlation_id IS NOT NULL
                  )
                LIMIT 1;
            """)
            defer { stmt.finalize() }
            return try stmt.step()
        }()
        if hasInFlight {
            try db.exec("PRAGMA wal_checkpoint(PASSIVE);")
            return false
        }

        // All gates passed — TRUNCATE
        try db.exec("PRAGMA wal_checkpoint(TRUNCATE);")
        return true
    }

    /// Start a 5-minute periodic PASSIVE checkpoint loop. Call once after `open`.
    ///
    /// The loop task is stored on the actor and cancelled by `shutdown()`.
    public func startPeriodicCheckpoint() {
        checkpointTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 5 * 60 * 1_000_000_000)
                guard !Task.isCancelled, let self else { return }
                do { try await self.checkpoint() }
                catch { Loggers.memory.error("checkpoint failed: \(error.localizedDescription)") }
            }
        }
    }

    /// Path to the WAL sidecar file derived from the database path.
    private func walFilePath() -> String {
        "\(path)-wal"
    }
}
