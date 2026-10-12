import AVFoundation
import AppKit
import ApplicationServices
import Foundation

struct AuraError: LocalizedError {
    let errorDescription: String?
    init(_ message: String) { errorDescription = message }
}

/// Settings in ~/.config/aura/config.toml (`key = "value"` lines).
/// Environment variables take precedence: GEMINI_API_KEY, AURA_MODEL, AURA_VOICE.
enum Config {
    static let file = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".config/aura/config.toml")

    static var apiKey: String? { value("api_key", env: "GEMINI_API_KEY") }
    static var model: String { value("model", env: "AURA_MODEL") ?? "gemini-3.8-live" }
    static var voice: String { value("voice", env: "AURA_VOICE") ?? "Kore" }

    static func value(_ key: String, env: String) -> String? {
        if let v = ProcessInfo.processInfo.environment[env], !v.isEmpty { return v }
        let v = parse((try? String(contentsOf: file, encoding: .utf8)) ?? "")[key]
        return v?.isEmpty == false ? v : nil
    }

    static func parse(_ text: String) -> [String: String] {
        var out: [String: String] = [:]
        for line in text.split(whereSeparator: \.isNewline) {
            let t = line.trimmingCharacters(in: .whitespaces)
            guard !t.hasPrefix("#"), let eq = t.firstIndex(of: "=") else { continue }
            let key = t[..<eq].trimmingCharacters(in: .whitespaces)
            var val = t[t.index(after: eq)...].trimmingCharacters(in: .whitespaces)
            if val.count >= 2, val.hasPrefix("\""), val.hasSuffix("\"") { val = String(val.dropFirst().dropLast()) }
            out[key] = val
        }
        return out
    }

    /// Store the API key, keeping any other settings already in the file.
    static func save(apiKey: String) throws {
        let old = (try? String(contentsOf: file, encoding: .utf8)) ?? ""
        let others = old.split(whereSeparator: \.isNewline).map(String.init)
            .filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("api_key") }
        let text = (["api_key = \"\(apiKey)\""] + others).joined(separator: "\n") + "\n"
        let fm = FileManager.default
        try fm.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try text.write(to: file, atomically: true, encoding: .utf8)
        try fm.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
    }
}

/// Facts the user asked Aura to remember. Included in every session's instructions.
enum Memory {
    static let file = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/Application Support/Aura/memory.json")

    static func load() -> [String] {
        (try? JSONDecoder().decode([String].self, from: Data(contentsOf: file))) ?? []
    }

    static func save(_ facts: [String]) {
        try? FileManager.default.createDirectory(
            at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? JSONEncoder().encode(facts).write(to: file)
    }

    static func remember(_ fact: String) {
        var facts = load().filter { $0 != fact }
        facts.append(fact)
        save(Array(facts.suffix(200)))
    }

    /// Removes facts containing `text`; returns how many were removed.
    static func forget(_ text: String) -> Int {
        let facts = load()
        let kept = facts.filter { !$0.localizedCaseInsensitiveContains(text) }
        save(kept)
        return facts.count - kept.count
    }
}

enum Permissions {
    static var microphone: Bool { AVCaptureDevice.authorizationStatus(for: .audio) == .authorized }
    static var screen: Bool { CGPreflightScreenCaptureAccess() }
    static var accessibility: Bool { AXIsProcessTrusted() }

    static func requestMicrophone(_ done: @escaping (Bool) -> Void) {
        if AVCaptureDevice.authorizationStatus(for: .audio) == .denied {
            openSettings("Privacy_Microphone")
            return done(false)
        }
        AVCaptureDevice.requestAccess(for: .audio) { ok in DispatchQueue.main.async { done(ok) } }
    }

    static func requestScreen() {
        if !CGRequestScreenCaptureAccess() { openSettings("Privacy_ScreenCapture") }
    }

    static func requestAccessibility() {
        let prompt = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        if !AXIsProcessTrustedWithOptions([prompt: true] as CFDictionary) { openSettings("Privacy_Accessibility") }
    }

    static func openSettings(_ pane: String) {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?\(pane)") {
            NSWorkspace.shared.open(url)
        }
    }
}
