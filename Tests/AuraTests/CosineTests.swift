import XCTest
@testable import Aura

final class CosineTests: XCTestCase {

    func test_identicalVectorsHaveSimilarityOne() {
        let v: [Float] = [1, 2, 3, 4, 5]
        XCTAssertEqual(Cosine.similarity(v, v), 1.0, accuracy: 1e-6)
    }

    func test_orthogonalVectorsHaveSimilarityZero() {
        let a: [Float] = [1, 0]
        let b: [Float] = [0, 1]
        XCTAssertEqual(Cosine.similarity(a, b), 0.0, accuracy: 1e-6)
    }

    func test_oppositeVectorsHaveSimilarityNegOne() {
        let a: [Float] = [1, 2, 3]
        let b: [Float] = [-1, -2, -3]
        XCTAssertEqual(Cosine.similarity(a, b), -1.0, accuracy: 1e-6)
    }

    func test_emptyVectorsReturnZero() {
        XCTAssertEqual(Cosine.similarity([], []), 0)
    }

    func test_mismatchedLengthsReturnZero() {
        XCTAssertEqual(Cosine.similarity([1, 2, 3], [1, 2]), 0)
    }

    func test_zeroVectorReturnsZero() {
        XCTAssertEqual(Cosine.similarity([0, 0, 0], [1, 2, 3]), 0)
    }

    func test_distanceIsOneMinusSimilarity() {
        let a: [Float] = [1, 0, 0]
        let b: [Float] = [0, 1, 0]
        XCTAssertEqual(Cosine.distance(a, b), 1.0, accuracy: 1e-6)
    }

    func test_scaleInvariant() {
        // Cosine ignores magnitude — similarity of (v) and (kv) is 1 for any k > 0.
        let a: [Float] = [1, 2, 3]
        let b: [Float] = [3, 6, 9]
        XCTAssertEqual(Cosine.similarity(a, b), 1.0, accuracy: 1e-6)
    }
}
