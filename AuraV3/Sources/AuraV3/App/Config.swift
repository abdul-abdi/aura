import Foundation

/// Lightweight TOML reader for our specific keys. We parse line-by-line — no general-purpose TOML.
public struct Config: Sendable {
    public var voice: String = "Kore"
    public var modelOverride: String? = nil
    public var procedural: ProceduralConfig = .init()

    public struct ProceduralConfig: Sendable {
        public var enabled: Bool = false
    }

    public static let keychainService = "dev.abdullahi.AuraV3"
    public static let keychainAccount = "api_key"

    public static var path: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/aura/config.toml")
    }

    public static func load() -> Config {
        guard let data = try? Data(contentsOf: path),
              let text = String(data: data, encoding: .utf8) else {
            return Config()
        }
        var cfg = Config()
        for line in text.split(separator: "\n", omittingEmptySubsequences: true) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("#") || trimmed.isEmpty { continue }
            let parts = trimmed.split(separator: "=", maxSplits: 1).map {
                $0.trimmingCharacters(in: .whitespaces)
            }
            guard parts.count == 2 else { continue }
            let key = parts[0]
            let raw = parts[1].trimmingCharacters(in: CharacterSet(charactersIn: "\""))
            switch key {
            case "voice": cfg.voice = raw
            case "model_override": cfg.modelOverride = raw
            case "procedural.enabled": cfg.procedural.enabled = (raw == "true")
            default: break
            }
        }
        return cfg
    }

    public static func apiKey() -> String? {
        Keychain.get(service: keychainService, account: keychainAccount)
    }
}
