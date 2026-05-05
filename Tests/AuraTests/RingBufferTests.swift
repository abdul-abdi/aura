import XCTest
@testable import Aura

final class RingBufferTests: XCTestCase {
    func test_writeAndReadBackInOrder() {
        let buf = RingBuffer<Int>(capacity: 8)
        for i in 0..<5 { _ = buf.write([i]) }
        let out = buf.readUpTo(5)
        XCTAssertEqual(out, [0, 1, 2, 3, 4])
    }

    func test_overflowDropsOldest() {
        let buf = RingBuffer<Int>(capacity: 4)
        _ = buf.write([1, 2, 3, 4, 5, 6])
        let out = buf.readUpTo(10)
        XCTAssertEqual(out, [3, 4, 5, 6])
    }

    func test_clearEmptiesBuffer() {
        let buf = RingBuffer<Int>(capacity: 4)
        _ = buf.write([1, 2, 3])
        buf.clear()
        XCTAssertTrue(buf.readUpTo(10).isEmpty)
    }

    func test_writeMoreThanCapacityInOneCallStillKeepsLastN() {
        let buf = RingBuffer<Int>(capacity: 3)
        _ = buf.write([10, 20, 30, 40, 50])
        XCTAssertEqual(buf.readUpTo(10), [30, 40, 50])
    }

    func test_currentCountReflectsBufferedElements() {
        let buf = RingBuffer<Int>(capacity: 8)
        _ = buf.write([1, 2, 3])
        XCTAssertEqual(buf.currentCount, 3)
        _ = buf.readUpTo(2)
        XCTAssertEqual(buf.currentCount, 1)
    }
}
