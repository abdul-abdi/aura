import XCTest
@testable import Aura

final class PerceptualHashTests: XCTestCase {
    func test_sameDataYieldsSameHash() {
        let a = Data(repeating: 0xAB, count: 1024)
        XCTAssertEqual(PerceptualHash.hash(a), PerceptualHash.hash(a))
    }

    func test_differentDataYieldsDifferentHash() {
        let a = Data(repeating: 0xAB, count: 1024)
        let b = Data(repeating: 0xCD, count: 1024)
        XCTAssertNotEqual(PerceptualHash.hash(a), PerceptualHash.hash(b))
    }

    func test_emptyDataReturnsSeed() {
        XCTAssertEqual(PerceptualHash.hash(Data()), 0xcbf29ce484222325)
    }

    func test_strideDoesNotChangeForUniformData() {
        // For uniform data, every sampled byte is the same, so stride choice
        // shouldn't matter for equality of two equal byte sequences.
        let a = Data(repeating: 0x42, count: 4096)
        let b = Data(repeating: 0x42, count: 4096)
        XCTAssertEqual(PerceptualHash.hash(a, stride: 64), PerceptualHash.hash(b, stride: 64))
        XCTAssertEqual(PerceptualHash.hash(a, stride: 128), PerceptualHash.hash(b, stride: 128))
    }
}
