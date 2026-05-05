import Foundation
import os

/// A single-slot mailbox: producer overwrites; consumer takes (consuming).
/// T is constrained to Sendable so concurrent put/take across isolation domains is safe.
/// Lock-protected; `OSAllocatedUnfairLock` is NEVER held across await.
public final class FrameSlot<T: Sendable>: @unchecked Sendable {
    private var slot: T?
    private let lock = OSAllocatedUnfairLock()

    public init() {}

    public func put(_ value: T) {
        lock.withLock { slot = value }
    }

    public func take() -> T? {
        lock.withLock {
            let v = slot
            slot = nil
            return v
        }
    }
}
