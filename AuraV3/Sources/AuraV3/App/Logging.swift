import Foundation
import OSLog

/// OSLog category loggers, grouped under one subsystem.
/// Renamed from `Log` to `Loggers` to avoid collision with `actor Log` (Memory/Log.swift, Task 1.3).
public enum Loggers {
    public static let subsystem = "dev.abdullahi.AuraV3"

    public static let app         = Logger(subsystem: subsystem, category: "app")
    public static let memory      = Logger(subsystem: subsystem, category: "memory")
    public static let transport   = Logger(subsystem: subsystem, category: "transport")
    public static let voice       = Logger(subsystem: subsystem, category: "voice")
    public static let vision      = Logger(subsystem: subsystem, category: "vision")
    public static let observation = Logger(subsystem: subsystem, category: "observation")
    public static let action      = Logger(subsystem: subsystem, category: "action")
    public static let policy      = Logger(subsystem: subsystem, category: "policy")
    public static let context     = Logger(subsystem: subsystem, category: "context")
    public static let rotation    = Logger(subsystem: subsystem, category: "rotation")
}
