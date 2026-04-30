import Foundation
import os

/// Single-producer / single-consumer ring buffer for realtime audio.
/// Producer (audio render thread): try-lock; drop frame on contention rather than block.
/// Consumer: blocking lock (never on audio thread).
///
/// Amendment H pragmatic adaptation: the original amendment proposes
/// `Synchronization.Atomic` (Swift 6.0+), but we're on swift-tools-version:5.10.
/// `OSAllocatedUnfairLock.lockIfAvailable()` gives us the same realtime-safety
/// invariant ("never block on audio thread") without the dependency upgrade.
public final class RingBuffer<T>: @unchecked Sendable {
    private var storage: [T?]
    private var head: Int = 0
    private var tail: Int = 0
    private var count: Int = 0
    private let capacity: Int
    private let lock = OSAllocatedUnfairLock()
    /// Frames dropped because the consumer was holding the lock when the audio
    /// thread tried to write. Read by the watchdog/diagnostics path.
    public private(set) var droppedFrames: Int = 0

    public init(capacity: Int) {
        precondition(capacity > 0)
        self.capacity = capacity
        self.storage = Array(repeating: nil, count: capacity)
    }

    /// Producer-side write. Returns count of items actually written; 0 if the
    /// lock was contended (caller should treat that as "frame dropped").
    @discardableResult
    public func write(_ items: [T]) -> Int {
        guard lock.lockIfAvailable() else {
            droppedFrames += items.count   // racy increment, fine for diagnostics
            return 0
        }
        defer { lock.unlock() }
        for item in items {
            storage[head] = item
            head = (head + 1) % capacity
            if count < capacity { count += 1 }
            else { tail = (tail + 1) % capacity }   // overwrite oldest
        }
        return items.count
    }

    public func readUpTo(_ n: Int) -> [T] {
        lock.withLock {
            var out: [T] = []
            let take = min(n, count)
            out.reserveCapacity(take)
            for _ in 0..<take {
                if let v = storage[tail] { out.append(v); storage[tail] = nil }
                tail = (tail + 1) % capacity
                count -= 1
            }
            return out
        }
    }

    public func clear() {
        lock.withLock {
            tail = head
            count = 0
            storage = Array(repeating: nil, count: capacity)
        }
    }

    public var currentCount: Int { lock.withLock { count } }
}
