/// A bounded async MPSC channel backed by a Swift actor.
///
/// Supports three drop policies for casual consumers:
///   - `.dropOldest`  — displaces the head of the buffer when full
///   - `.dropNewest`  — discards the incoming value when full
///   - `.never`       — blocks the sender until the consumer drains space
///
/// For fine-grained control (e.g. log writer giving priority to turn.*
/// events over state.observation events), install a per-event closure
/// via `setDropDecision(_:)`. When set, it overrides `dropPolicy`.
public actor AsyncChannel<Element: Sendable>: AsyncSequence {
    public typealias AsyncIterator = Iterator

    // MARK: - Types

    public enum DropPolicy: Sendable {
        case dropOldest
        case dropNewest
        case never
    }

    /// Per-event decision returned by the `dropDecision` closure.
    public enum DropDecision: Sendable {
        /// No-op marker — callers should not return this when the buffer is full.
        /// Treated the same as `.dropIncoming` in the full-buffer path.
        case accept
        /// Discard the incoming value; buffer is unchanged.
        case dropIncoming
        /// Remove the buffered element at `index`, then append the incoming value.
        case replaceAtIndex(Int)
        /// Await space: the sender suspends until a consumer removes at least one element.
        case blockUntilSpace
    }

    // MARK: - State

    private var buffer: [Element] = []
    public let capacity: Int
    private let dropPolicy: DropPolicy
    private var dropDecision: (@Sendable (Element, [Element]) -> DropDecision)?

    /// Continuations of consumers waiting for the next element.
    private var receiveContinuations: [CheckedContinuation<Element?, Never>] = []
    /// Senders that are blocked waiting for buffer space.
    private var sendContinuations: [(Element, CheckedContinuation<Void, Never>)] = []
    private var finished = false

    // MARK: - Init

    public init(capacity: Int, dropPolicy: DropPolicy = .never) {
        precondition(capacity > 0, "AsyncChannel capacity must be > 0")
        self.capacity = capacity
        self.dropPolicy = dropPolicy
    }

    // MARK: - Configuration

    /// Install a per-event drop-decision closure.
    ///
    /// When set, this overrides `dropPolicy` when the buffer is full.
    /// The closure receives the incoming element and the current buffer snapshot.
    public func setDropDecision(
        _ decide: @Sendable @escaping (Element, [Element]) -> DropDecision
    ) {
        self.dropDecision = decide
    }

    // MARK: - Send

    public func send(_ value: Element) async {
        // Fast path: wake a parked consumer directly.
        if !receiveContinuations.isEmpty {
            let c = receiveContinuations.removeFirst()
            c.resume(returning: value)
            return
        }

        // Buffer has room — just append.
        if buffer.count < capacity {
            buffer.append(value)
            return
        }

        // Buffer is full — consult drop decision.
        let decision: DropDecision
        if let f = dropDecision {
            decision = f(value, buffer)
        } else {
            switch dropPolicy {
            case .dropOldest: decision = .replaceAtIndex(0)
            case .dropNewest: decision = .dropIncoming
            case .never:      decision = .blockUntilSpace
            }
        }

        switch decision {
        case .accept:
            // Treat as dropIncoming when buffer is full.
            return

        case .dropIncoming:
            return

        case .replaceAtIndex(let idx):
            if idx >= 0, idx < buffer.count {
                buffer.remove(at: idx)
            }
            buffer.append(value)

        case .blockUntilSpace:
            await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
                sendContinuations.append((value, cont))
            }
            // Resumed by `next()` after it removes an element.
        }
    }

    // MARK: - Finish

    public func finish() {
        finished = true
        for c in receiveContinuations { c.resume(returning: nil) }
        receiveContinuations.removeAll()
        // Unblock any blocked senders so they don't leak.
        for (_, cont) in sendContinuations { cont.resume() }
        sendContinuations.removeAll()
    }

    // MARK: - Receive

    public func next() async -> Element? {
        if !buffer.isEmpty {
            let value = buffer.removeFirst()
            // Wake one blocked sender — their value enters the buffer.
            if !sendContinuations.isEmpty {
                let (queuedValue, cont) = sendContinuations.removeFirst()
                buffer.append(queuedValue)
                cont.resume()
            }
            return value
        }
        if finished { return nil }
        return await withCheckedContinuation { c in
            receiveContinuations.append(c)
        }
    }

    // MARK: - AsyncSequence

    public nonisolated func makeAsyncIterator() -> Iterator {
        Iterator(channel: self)
    }

    public struct Iterator: AsyncIteratorProtocol {
        let channel: AsyncChannel
        public mutating func next() async -> Element? {
            await channel.next()
        }
    }
}
