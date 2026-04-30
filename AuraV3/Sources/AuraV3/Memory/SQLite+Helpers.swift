import Foundation
import SQLite3

public enum SQLiteError: Error, CustomStringConvertible {
    case open(Int32, String)
    case prepare(Int32, String)
    case step(Int32, String)

    public var description: String {
        switch self {
        case .open(let rc, let msg):    return "sqlite open error \(rc): \(msg)"
        case .prepare(let rc, let msg): return "sqlite prepare error \(rc): \(msg)"
        case .step(let rc, let msg):    return "sqlite step error \(rc): \(msg)"
        }
    }
}

/// Tiny ergonomic wrapper around the system sqlite3 C API.
/// Single-threaded by design; users own the synchronization.
///
/// On open: WAL journal mode + autocheckpoint(1000). Synchronous defaults to NORMAL
/// (the WAL default). Callers (e.g. Log.write per amendment E) set synchronous per-write
/// based on event-kind durability tier.
public final class SQLite {
    private var handle: OpaquePointer?

    private init(handle: OpaquePointer?) { self.handle = handle }

    public static func open(path: String) throws -> SQLite {
        var handle: OpaquePointer?
        let rc = sqlite3_open_v2(
            path, &handle,
            SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX,
            nil
        )
        guard rc == SQLITE_OK else {
            let msg = handle.flatMap { String(cString: sqlite3_errmsg($0)) } ?? "?"
            sqlite3_close(handle)
            throw SQLiteError.open(rc, msg)
        }
        let db = SQLite(handle: handle)
        try db.exec("PRAGMA journal_mode = WAL;")
        // Amendment E: do NOT force synchronous = FULL globally.
        // Default after WAL is NORMAL (1). Log.write sets per-kind tier on each insert.
        try db.exec("PRAGMA wal_autocheckpoint = 1000;")
        return db
    }

    public func close() {
        if let h = handle { sqlite3_close_v2(h) }
        handle = nil
    }

    public func exec(_ sql: String) throws {
        let rc = sqlite3_exec(handle, sql, nil, nil, nil)
        guard rc == SQLITE_OK else {
            let msg = handle.flatMap { String(cString: sqlite3_errmsg($0)) } ?? "?"
            throw SQLiteError.step(rc, msg)
        }
    }

    public func prepare(_ sql: String) throws -> Statement {
        var stmt: OpaquePointer?
        let rc = sqlite3_prepare_v2(handle, sql, -1, &stmt, nil)
        guard rc == SQLITE_OK else {
            let msg = handle.flatMap { String(cString: sqlite3_errmsg($0)) } ?? "?"
            throw SQLiteError.prepare(rc, msg)
        }
        return Statement(handle: stmt)
    }

    public func query<T>(_ sql: String, _ map: (Statement) throws -> T) throws -> [T] {
        let s = try prepare(sql)
        defer { s.finalize() }
        var out: [T] = []
        while try s.step() { out.append(try map(s)) }
        return out
    }

    public func lastInsertRowID() -> Int64 { sqlite3_last_insert_rowid(handle) }
}

public final class Statement {
    private var stmt: OpaquePointer?

    init(handle: OpaquePointer?) { self.stmt = handle }
    deinit { finalize() }

    public func finalize() {
        if let s = stmt { sqlite3_finalize(s) }
        stmt = nil
    }

    public func bind(_ index: Int32, _ value: Int64) {
        sqlite3_bind_int64(stmt, index, value)
    }

    public func bind(_ index: Int32, _ value: String) {
        // SQLITE_TRANSIENT lets SQLite copy the bytes.
        let SQLITE_TRANSIENT = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
        sqlite3_bind_text(stmt, index, value, -1, SQLITE_TRANSIENT)
    }

    public func bindNull(_ index: Int32) {
        sqlite3_bind_null(stmt, index)
    }

    public func step() throws -> Bool {
        let rc = sqlite3_step(stmt)
        switch rc {
        case SQLITE_ROW:  return true
        case SQLITE_DONE: return false
        default:
            let msg = stmt.flatMap {
                String(cString: sqlite3_errmsg(sqlite3_db_handle($0)))
            } ?? "?"
            throw SQLiteError.step(rc, msg)
        }
    }

    public func text(at column: Int32) throws -> String {
        guard let p = sqlite3_column_text(stmt, column) else { return "" }
        return String(cString: p)
    }

    public func int(at column: Int32) throws -> Int64 {
        sqlite3_column_int64(stmt, column)
    }
}
