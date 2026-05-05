import Foundation

public struct EffectIntent: Sendable {
    public let operation: String
    public let args: JSONValue
    public init(operation: String, args: JSONValue) {
        self.operation = operation
        self.args = args
    }
}

public actor EffectLog {
    private let log: Log

    public init(log: Log) { self.log = log }

    /// Wrap any world-mutating action with append-only effect.posted/effect.outcome
    /// pair, sharing a UUID correlation_id. Outcome is logged whether the body
    /// succeeds or throws.
    ///
    /// Amendment D: optional `geminiCallID` pairs the effect with the Gemini
    /// toolCall.id that requested it; without it, duplicate cross-rotation
    /// dispatches are unfalsifiable.
    public func executeWithEffect<T: Sendable>(
        intent: EffectIntent,
        geminiCallID: String? = nil,
        body: @Sendable () async throws -> T
    ) async throws -> T {
        let cid = UUID().uuidString
        await log.append(
            kind: EventKind.effectPosted,
            correlationID: cid,
            geminiCallID: geminiCallID,
            payload: .object([
                "operation": .string(intent.operation),
                "args": intent.args,
            ])
        )

        let result: Result<T, Error>
        do {
            let value = try await body()
            result = .success(value)
        } catch {
            result = .failure(error)
        }

        switch result {
        case .success(let v):
            await log.append(
                kind: EventKind.effectOutcome,
                correlationID: cid,
                geminiCallID: geminiCallID,
                payload: .object([
                    "status": .string("ok"),
                    "operation": .string(intent.operation),
                ])
            )
            return v
        case .failure(let e):
            await log.append(
                kind: EventKind.effectOutcome,
                correlationID: cid,
                geminiCallID: geminiCallID,
                payload: .object([
                    "status": .string("failed"),
                    "operation": .string(intent.operation),
                    "error": .string(String(describing: e)),
                ])
            )
            throw e
        }
    }

    /// Find correlation IDs of posted effects with no matching outcome.
    /// Used at session start (boot orphan detection) and during rotation drain.
    public static func findOrphans(log: Log) async throws -> [String] {
        let posted = try await log.query(kind: EventKind.effectPosted, since: 0)
        let outcomes = try await log.query(kind: EventKind.effectOutcome, since: 0)
        let outcomeIDs = Set(outcomes.compactMap { $0.correlationID })
        return posted.compactMap { p in
            guard let cid = p.correlationID, !outcomeIDs.contains(cid) else { return nil }
            return cid
        }
    }
}
