import XCTest
@testable import Aura

/// Plan 1 placeholder. The real `Context.swift` lands in Plan 3 (§9.5 of the
/// spec — full prompt assembly with token budgets, fixture replay, lens
/// composition). For Plan 1, the only context-shaped surface is
/// `RotationDigest.build(...)` which is exercised by `RotationInvariantTests`.
///
/// This file exists per Plan 1's declared file structure so Plan 3 has a place
/// to land its fixture-driven test set without a Package.swift change.
final class ContextDigestTests: XCTestCase {

    func test_placeholder_planThreeFillsThisIn() {
        // Intentionally empty. RotationInvariantTests already covers the
        // Plan-1-scope digest behavior. Plan 3 will replace this body with
        // the deterministic fixture-replay set described in spec §15
        // "Context fixture set" + the chars/token calibration assertion.
        XCTAssertTrue(true)
    }
}
