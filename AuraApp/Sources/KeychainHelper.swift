import Foundation

/// Manages device tokens in the macOS login Keychain via the `security` CLI.
///
/// Uses the CLI instead of Security.framework to avoid per-app ACL prompts
/// when multiple ad-hoc signed binaries (SwiftUI app + Rust daemon) access
/// the same Keychain item. The `-A` flag allows any application to read the
/// item without triggering a password dialog.
enum KeychainHelper {
    static let service = "com.aura.desktop"

    @discardableResult
    static func saveString(account: String, value: String) -> Bool {
        // Delete existing item first (ignore errors)
        _ = deleteItem(account: account)

        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/usr/bin/security")
        task.arguments = [
            "add-generic-password",
            "-s", service,
            "-a", account,
            "-w", value,
            "-A",  // Allow any application to access (no per-app ACL prompts)
        ]

        do {
            try task.run()
            task.waitUntilExit()
            return task.terminationStatus == 0
        } catch {
            print("[Aura] Keychain save failed: \(error.localizedDescription)")
            return false
        }
    }

    static func readString(account: String) -> String? {
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/usr/bin/security")
        task.arguments = [
            "find-generic-password",
            "-s", service,
            "-a", account,
            "-w",  // Output password only
        ]

        let pipe = Pipe()
        task.standardOutput = pipe
        task.standardError = FileHandle.nullDevice

        do {
            try task.run()
            task.waitUntilExit()
            guard task.terminationStatus == 0 else { return nil }
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            let value = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines)
            return value?.isEmpty == true ? nil : value
        } catch {
            return nil
        }
    }

    @discardableResult
    static func delete(account: String) -> Bool {
        return deleteItem(account: account)
    }

    private static func deleteItem(account: String) -> Bool {
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/usr/bin/security")
        task.arguments = [
            "delete-generic-password",
            "-s", service,
            "-a", account,
        ]
        task.standardOutput = FileHandle.nullDevice
        task.standardError = FileHandle.nullDevice

        do {
            try task.run()
            task.waitUntilExit()
            return task.terminationStatus == 0
        } catch {
            return false
        }
    }

    // Legacy API wrappers for compatibility
    @discardableResult
    static func save(account: String, data: Data) -> Bool {
        guard let value = String(data: data, encoding: .utf8) else { return false }
        return saveString(account: account, value: value)
    }

    static func read(account: String) -> Data? {
        guard let value = readString(account: account) else { return nil }
        return value.data(using: .utf8)
    }
}
