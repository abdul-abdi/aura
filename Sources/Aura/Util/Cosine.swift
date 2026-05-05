import Foundation
import Accelerate

/// SIMD cosine similarity for f32 vectors. Used by the embedding fallback path
/// in §6.3 of the spec — `NLContextualEmbedding` produces 768-dim vectors and
/// retrieval scores them via cosine.
///
/// Why vDSP: a 768-dim dot product on the CPU is faster than the Swift array
/// loop by ~10x via NEON. At our v3.0 scaling boundary (≤30-day window of
/// facts, ≤50K vectors) that translates to a recall pass under 30 ms.
public enum Cosine {

    /// Cosine similarity between two equal-length vectors, in [-1, 1].
    /// Returns 0 for empty / mismatched / zero-magnitude inputs (a defensible
    /// "neutral" score that downstream rankers can tie-break on).
    public static func similarity(_ a: [Float], _ b: [Float]) -> Float {
        guard a.count == b.count, !a.isEmpty else { return 0 }
        let n = vDSP_Length(a.count)

        var dot: Float = 0
        var sumSqA: Float = 0
        var sumSqB: Float = 0
        vDSP_dotpr(a, 1, b, 1, &dot, n)
        vDSP_svesq(a, 1, &sumSqA, n)
        vDSP_svesq(b, 1, &sumSqB, n)

        let denom = (sumSqA * sumSqB).squareRoot()
        guard denom > 0 else { return 0 }
        return dot / denom
    }

    /// Cosine distance = 1 − similarity. Useful when ranking ascending.
    public static func distance(_ a: [Float], _ b: [Float]) -> Float {
        1 - similarity(a, b)
    }
}
