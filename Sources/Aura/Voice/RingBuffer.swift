import Foundation
import os

/// Single-producer / single-consumer ring buffer for realtime audio.
///
/// Producer (audio render thread): try-lock; **drops the frame** on contention
/// rather than blocking. The audio thread MUST never wait on a lock.
/// Consumer: blocking lock; never runs on the audio thread.
///
/// Amendment H pragmatic adaptation: the original amendment text proposes
/// `Synchronization.Atomic<Int>` head/tail indices — a true lock-free ring —
/// but that requires `swift-tools-version:6.0` (the `Synchronization` framework
/// is Swift 6.0+). We're on `swift-tools-version:5.10`. `OSAllocatedUnfairLock
/// .lockIfAvailable()` gives us the same realtime-safety invariant ("never
/// block on the audio thread") without the dependency upgrade. The trade-off
/// is documented; producer contention causes a frame drop instead of a stall.
///
/// Used by:
/// - `AudioCapture` as the spec §5.2 invariant-3 "2-second mic ring" — frames
///   captured during a session rotation are buffered here and replayed on the
///   new session iff the resume isn't a wake-from-sleep (replay would invent
///   speech the user never made).
public final class RingBuffer<T>: @unchecked Sendable where T: Sendable {
    private var storage: [T?]
    private var head: Int = 0
    private var tail: Int = 0
    private var count: Int = 0
    private let capacity: Int
    private let lock = OSAllocatedUnfairLock()
    /// Frames dropped because the consumer was holding the lock when the audio
    /// thread tried to write. Read by diagnostics; racy increment is fine.
    public private(set) var droppedFrames: Int = 0

    public init(capacity: Int) {
        precondition(capacity > 0, "RingBuffer capacity must be > 0")
        self.capacity = capacity
        self.storage = Array(repeating: nil, count: capacity)
    }

    /// Producer-side write. Returns the count actually written; 0 if the lock
    /// was contended (caller treats that as "frame dropped"). Overwrites the
    /// oldest element when the buffer fills.
    @discardableResult
    public func write(_ items: [T]) -> Int {
        guard lock.lockIfAvailable() else {
            droppedFrames += items.count
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

    /// Consumer-side read. Removes up to `n` items from the head of the queue.
    public func readUpTo(_ n: Int) -> [T] {
        lock.withLock {
            var out: [T] = []
            let take = Swift.min(n, count)
            out.reserveCapacity(take)
            for _ in 0..<take {
                if let v = storage[tail] {
                    out.append(v)
                    storage[tail] = nil
                }
                tail = (tail + 1) % capacity
                count -= 1
            }
            return out
        }
    }

    /// Discard all buffered elements.
    public func clear() {
        lock.withLock {
            tail = head
            count = 0
            storage = Array(repeating: nil, count: capacity)
        }
    }

    public var currentCount: Int { lock.withLock { count } }
}
