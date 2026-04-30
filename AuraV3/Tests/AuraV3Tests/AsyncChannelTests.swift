import XCTest
@testable import AuraV3

final class AsyncChannelTests: XCTestCase {
    func test_sendAndReceive_inOrder() async {
        let chan = AsyncChannel<Int>(capacity: 4)
        Task {
            await chan.send(1)
            await chan.send(2)
            await chan.send(3)
            await chan.finish()
        }
        var got: [Int] = []
        for await v in chan { got.append(v) }
        XCTAssertEqual(got, [1, 2, 3])
    }

    func test_capacityBackpressure_dropOldest() async {
        let chan = AsyncChannel<Int>(capacity: 2, dropPolicy: .dropOldest)
        await chan.send(1)
        await chan.send(2)
        await chan.send(3)  // displaces 1
        await chan.finish()

        var got: [Int] = []
        for await v in chan { got.append(v) }
        XCTAssertEqual(got, [2, 3])
    }

    func test_capacityBackpressure_dropNewest() async {
        let chan = AsyncChannel<Int>(capacity: 2, dropPolicy: .dropNewest)
        await chan.send(1)
        await chan.send(2)
        await chan.send(3)  // dropped
        await chan.finish()

        var got: [Int] = []
        for await v in chan { got.append(v) }
        XCTAssertEqual(got, [1, 2])
    }

    func test_dropDecision_replaceAtIndex_takesPriority() async {
        // Simulate Log writer behavior: incoming high-priority displaces a low-priority element.
        let chan = AsyncChannel<String>(capacity: 2)
        await chan.setDropDecision { newItem, buffered in
            // Drop the first "low" we find; keep "high".
            if newItem.hasPrefix("high") {
                if let idx = buffered.firstIndex(where: { $0.hasPrefix("low") }) {
                    return .replaceAtIndex(idx)
                }
            }
            return .dropIncoming
        }
        await chan.send("low-1")
        await chan.send("low-2")
        await chan.send("high-1")  // displaces "low-1"
        await chan.send("high-2")  // displaces "low-2"
        await chan.send("low-3")   // dropped (no low to displace)
        await chan.finish()

        var got: [String] = []
        for await v in chan { got.append(v) }
        XCTAssertEqual(got, ["high-1", "high-2"])
    }

    func test_blockUntilSpace_waitsForConsumer() async {
        // dropPolicy: .never (default) means a full channel blocks the sender.
        let chan = AsyncChannel<Int>(capacity: 1)
        await chan.send(1)  // buffered
        let started = Date()

        let producer = Task { [chan] in
            await chan.send(2)  // blocks until 1 is consumed
        }

        // Give producer a chance to block.
        try? await Task.sleep(nanoseconds: 50_000_000)  // 50ms
        // Drain 1 — this should unblock the producer.
        _ = await chan.next()
        await producer.value
        let elapsed = Date().timeIntervalSince(started)
        XCTAssertGreaterThan(elapsed, 0.04)  // producer was blocked

        let next = await chan.next()  // should be 2
        XCTAssertEqual(next, 2)
        await chan.finish()
    }
}
