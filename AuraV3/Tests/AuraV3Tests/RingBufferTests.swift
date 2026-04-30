import XCTest
@testable import AuraV3

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

    func test_currentCountTracksWrites() {
        let buf = RingBuffer<Int>(capacity: 8)
        _ = buf.write([1, 2, 3])
        XCTAssertEqual(buf.currentCount, 3)
        _ = buf.readUpTo(2)
        XCTAssertEqual(buf.currentCount, 1)
    }

    func test_readUpToReturnsAtMostRequested() {
        let buf = RingBuffer<Int>(capacity: 8)
        _ = buf.write([1, 2, 3, 4, 5])
        let out = buf.readUpTo(3)
        XCTAssertEqual(out.count, 3)
        XCTAssertEqual(out, [1, 2, 3])
    }
}
