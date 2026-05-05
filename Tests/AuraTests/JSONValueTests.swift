import XCTest
@testable import Aura

final class JSONValueTests: XCTestCase {
    func test_encodeAndDecodeRoundtrip() throws {
        let original: JSONValue = .object([
            "kind": .string("turn.user"),
            "ts_wall": .number(1714374000000),
            "active": .bool(true),
            "tags": .array([.string("a"), .string("b")]),
            "missing": .null,
        ])
        let data = try JSONEncoder().encode(original)
        let decoded = try JSONDecoder().decode(JSONValue.self, from: data)
        XCTAssertEqual(original, decoded)
    }

    func test_extractByKey() {
        let v: JSONValue = .object(["entity": .string("user")])
        XCTAssertEqual(v["entity"]?.stringValue, "user")
        XCTAssertNil(v["missing"])
    }
}
