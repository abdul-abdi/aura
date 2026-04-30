import XCTest
@testable import AuraV3

final class BargeInTests: XCTestCase {
    func test_calibrationSetsThresholdAboveAmbient() {
        let detector = BargeInDetector()
        let ambientFrames = (0..<100).map { _ in [Float](repeating: 0.005, count: 256) }
        for f in ambientFrames { detector.observeAmbient(frame: f) }
        detector.commitCalibration()
        XCTAssertGreaterThan(detector.threshold, 0.005)
    }

    func test_uncalibrated_neverBargesIn() {
        // Amendment V: shouldBargeIn returns false before calibration is committed.
        let detector = BargeInDetector()
        let loud = [Float](repeating: 0.5, count: 256)
        XCTAssertFalse(detector.shouldBargeIn(frame: loud))
        XCTAssertFalse(detector.shouldBargeIn(frame: loud))  // even after multiple calls
    }

    func test_singleSpikeDoesNotTriggerBargeIn() {
        let detector = BargeInDetector()
        for _ in 0..<50 { detector.observeAmbient(frame: [Float](repeating: 0.005, count: 256)) }
        detector.commitCalibration()
        let loudFrame = [Float](repeating: 0.5, count: 256)
        XCTAssertFalse(detector.shouldBargeIn(frame: loudFrame))
    }

    func test_twoConsecutiveLoudFramesTriggerBargeIn() {
        let detector = BargeInDetector()
        for _ in 0..<50 { detector.observeAmbient(frame: [Float](repeating: 0.005, count: 256)) }
        detector.commitCalibration()
        let loud = [Float](repeating: 0.5, count: 256)
        _ = detector.shouldBargeIn(frame: loud)
        XCTAssertTrue(detector.shouldBargeIn(frame: loud))
    }

    func test_reverbGuardSuppressesBargeIn() {
        let detector = BargeInDetector()
        detector.calibrateSynthetic()
        detector.startReverbGuard(durationMs: 1000)
        let loud = [Float](repeating: 0.5, count: 256)
        _ = detector.shouldBargeIn(frame: loud)
        XCTAssertFalse(detector.shouldBargeIn(frame: loud), "reverb guard active")
    }

    func test_calibrateSyntheticSetsIsCalibrated() {
        let detector = BargeInDetector()
        XCTAssertFalse(detector.isCalibrated)
        detector.calibrateSynthetic()
        XCTAssertTrue(detector.isCalibrated)
    }
}
