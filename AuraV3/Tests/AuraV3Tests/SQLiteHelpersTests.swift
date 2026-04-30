import XCTest
@testable import AuraV3

final class SQLiteHelpersTests: XCTestCase {
    var dbPath: String!

    override func setUp() async throws {
        dbPath = "/tmp/aura_test_\(UUID().uuidString).sqlite"
    }
    override func tearDown() async throws {
        try? FileManager.default.removeItem(atPath: dbPath)
        try? FileManager.default.removeItem(atPath: dbPath + "-wal")
        try? FileManager.default.removeItem(atPath: dbPath + "-shm")
    }

    func test_openCreateExecuteFetch() throws {
        let db = try SQLite.open(path: dbPath)
        defer { db.close() }
        try db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL);")
        try db.exec("INSERT INTO t (v) VALUES ('hello'), ('world');")
        let rows = try db.query("SELECT v FROM t ORDER BY id;") { stmt in
            try stmt.text(at: 0)
        }
        XCTAssertEqual(rows, ["hello", "world"])
    }

    func test_walModeAfterOpen() throws {
        let db = try SQLite.open(path: dbPath)
        defer { db.close() }
        let mode = try db.query("PRAGMA journal_mode;") { try $0.text(at: 0) }
        XCTAssertEqual(mode.first?.lowercased(), "wal")
    }

    /// Amendment E: SQLite.open does NOT force synchronous=FULL globally.
    /// Under WAL, the default is NORMAL (1). Log.write sets per-kind tier when needed.
    /// This test verifies (a) the default is NORMAL after open, and (b) the helper
    /// supports setting the pragma to FULL via exec.
    func test_synchronousDefaultsToNormalAfterWal_andCanBeSetToFull() throws {
        let db = try SQLite.open(path: dbPath)
        defer { db.close() }
        let s1 = try db.query("PRAGMA synchronous;") { try $0.int(at: 0) }
        XCTAssertEqual(s1.first, 1) // NORMAL == 1 (WAL default per amendment E)
        try db.exec("PRAGMA synchronous = 2;")
        let s2 = try db.query("PRAGMA synchronous;") { try $0.int(at: 0) }
        XCTAssertEqual(s2.first, 2) // FULL == 2
    }

    func test_bindParametersAndQuery() throws {
        let db = try SQLite.open(path: dbPath)
        defer { db.close() }
        try db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, k TEXT, n INTEGER);")
        let ins = try db.prepare("INSERT INTO t (k, n) VALUES (?, ?);")
        ins.bind(1, "hello")
        ins.bind(2, Int64(42))
        _ = try ins.step()
        ins.finalize()

        let rows = try db.query("SELECT k, n FROM t WHERE k = 'hello';") { stmt in
            (try stmt.text(at: 0), try stmt.int(at: 1))
        }
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows.first?.0, "hello")
        XCTAssertEqual(rows.first?.1, 42)
    }
}
