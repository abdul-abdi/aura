import AppKit
import Foundation

/// The tools Gemini can call, and what they do.
enum Tools {
    static let declarations: [[String: Any]] = [
        fn("get_screen_context",
           "Frontmost app, window, focused element, clipboard, and the clickable or typeable elements on screen with ids and positions. Use it before click_element."),
        fn("click_element", "Click an element by id from the latest get_screen_context.",
           ["id": int("Element id")], required: ["id"]),
        fn("click", "Click a point on the screen view (x and y from 0 to 1000; 0,0 is top-left).",
           ["x": num("0 = left edge, 1000 = right edge"), "y": num("0 = top edge, 1000 = bottom edge"),
            "button": str("left (default) or right"), "count": int("2 for double-click, 3 for triple-click")],
           required: ["x", "y"]),
        fn("type_text", "Type text into the focused field.", ["text": str("Text to type")], required: ["text"]),
        fn("press_key", "Press a key or shortcut, e.g. key \"c\" with modifiers [\"cmd\"].",
           ["key": str("a-z, 0-9, return, tab, space, delete, escape, up, down, left, right, home, end, pageup, pagedown, f1-f12, or punctuation"),
            "modifiers": list("Any of cmd, shift, option, ctrl, fn")],
           required: ["key"]),
        fn("scroll", "Scroll the content under a point (default: screen center).",
           ["direction": str("up, down, left or right"), "amount": int("Lines to scroll, default 5"),
            "x": num("0-1000"), "y": num("0-1000")],
           required: ["direction"]),
        fn("drag", "Drag from one point to another (0-1000 coordinates).",
           ["from_x": num("Start x"), "from_y": num("Start y"), "to_x": num("End x"), "to_y": num("End y")],
           required: ["from_x", "from_y", "to_x", "to_y"]),
        fn("open", "Open or switch to an app by name, open a file or folder, or open a URL.",
           ["target": str("e.g. \"Safari\", \"~/Downloads\", \"https://news.ycombinator.com\"")], required: ["target"]),
        fn("click_menu", "Click a menu bar command of the frontmost app.",
           ["path": list("Menu titles from the menu bar down, e.g. [\"File\", \"Save As…\"]")], required: ["path"]),
        fn("run_applescript", "Run AppleScript to control Mac apps (Finder, Mail, Music, Calendar, System Events…). Returns its result.",
           ["script": str("AppleScript source")], required: ["script"]),
        fn("run_browser_js", "Run JavaScript in the active tab of the frontmost browser (Safari or Chrome-based) and return the result. Best way to read, click and fill web pages.",
           ["code": str("JavaScript; the value of the last expression is returned")], required: ["code"]),
        fn("set_clipboard", "Put text on the clipboard.", ["text": str("Text")], required: ["text"]),
        fn("remember", "Save a lasting fact about the user or their preferences.",
           ["fact": str("The fact, as one short sentence")], required: ["fact"]),
        fn("forget", "Delete remembered facts that contain this text.", ["text": str("Text to match")], required: ["text"]),
        fn("quit_aura", "Quit Aura. Only when the user asks you to quit or close Aura."),
    ]

    /// Tools after which a fresh screenshot is sent before replying.
    static let changesScreen: Set<String> = [
        "click_element", "click", "type_text", "press_key", "scroll", "drag", "open", "click_menu",
        "run_applescript", "run_browser_js",
    ]

    private static let needsAccessibility: Set<String> = [
        "get_screen_context", "click_element", "click", "type_text", "press_key", "scroll", "drag", "click_menu",
    ]

    /// Elements from the latest get_screen_context, addressed by index.
    private static var elements: [AX.Element] = []

    /// Run a tool. Called off the main thread, one tool at a time.
    static func run(_ name: String, _ args: [String: Any]) async -> [String: Any] {
        if needsAccessibility.contains(name), !Permissions.accessibility {
            return ["error": "Accessibility permission is off. Ask the user to enable Aura in System Settings › Privacy & Security › Accessibility."]
        }
        let text = args["text"] as? String
        switch name {
        case "get_screen_context":
            return context()
        case "click_element":
            guard let id = integer(args["id"]), elements.indices.contains(id) else {
                return ["error": "Unknown element id. Call get_screen_context first."]
            }
            let element = elements[id]
            let center = CGPoint(x: element.frame.midX, y: element.frame.midY)
            if Screen.bounds.contains(center) {
                Input.click(at: center)
            } else if !AX.press(element.ref) {
                return ["error": "That element is not on screen"]
            }
            return ["ok": true, "clicked": "\(element.role.dropFirst(2)) \"\(element.label)\""]
        case "click":
            guard let x = double(args["x"]), let y = double(args["y"]) else { return missing("x and y") }
            Input.click(at: Screen.point(x: x, y: y), right: args["button"] as? String == "right",
                        count: integer(args["count"]) ?? 1)
            return ["ok": true]
        case "type_text":
            guard let text else { return missing("text") }
            if text.count > 300 {
                setClipboard(text)
                _ = Input.press("v", modifiers: ["cmd"])
            } else {
                Input.type(text)
            }
            return ["ok": true]
        case "press_key":
            guard let key = args["key"] as? String else { return missing("key") }
            return Input.press(key, modifiers: args["modifiers"] as? [String] ?? [])
                ? ["ok": true] : ["error": "Unknown key \"\(key)\""]
        case "scroll":
            let amount = Int32(integer(args["amount"]) ?? 5)
            let at = Screen.point(x: double(args["x"]) ?? 500, y: double(args["y"]) ?? 500)
            switch (args["direction"] as? String ?? "down").lowercased() {
            case "up": Input.scroll(at: at, down: -amount)
            case "left": Input.scroll(at: at, right: -amount)
            case "right": Input.scroll(at: at, right: amount)
            default: Input.scroll(at: at, down: amount)
            }
            return ["ok": true]
        case "drag":
            guard let fx = double(args["from_x"]), let fy = double(args["from_y"]),
                  let tx = double(args["to_x"]), let ty = double(args["to_y"])
            else { return missing("from_x, from_y, to_x and to_y") }
            Input.drag(from: Screen.point(x: fx, y: fy), to: Screen.point(x: tx, y: ty))
            return ["ok": true]
        case "open":
            guard let target = args["target"] as? String else { return missing("target") }
            return await open(target.trimmingCharacters(in: .whitespacesAndNewlines))
        case "click_menu":
            guard let path = args["path"] as? [String], !path.isEmpty else { return missing("path") }
            if let error = AX.clickMenu(path) { return ["error": error] }
            return ["ok": true]
        case "run_applescript":
            guard let script = args["script"] as? String else { return missing("script") }
            if script.range(of: "do shell script", options: .caseInsensitive) != nil {
                return ["error": "Shell commands from AppleScript are blocked for safety."]
            }
            return await osascript(script)
        case "run_browser_js":
            guard let code = args["code"] as? String else { return missing("code") }
            return await browserJavaScript(code)
        case "set_clipboard":
            guard let text else { return missing("text") }
            setClipboard(text)
            return ["ok": true]
        case "remember":
            guard let fact = args["fact"] as? String else { return missing("fact") }
            Memory.remember(fact)
            return ["ok": true]
        case "forget":
            guard let text else { return missing("text") }
            return ["removed": Memory.forget(text)]
        default:
            return ["error": "Unknown tool \(name)"]
        }
    }

    /// Short label for the activity feed.
    static func describe(_ name: String, _ args: [String: Any]) -> String {
        switch name {
        case "type_text": return "Typing “\(String((args["text"] as? String ?? "").prefix(40)))”"
        case "press_key":
            let mods = (args["modifiers"] as? [String] ?? []).map { "\($0)+" }.joined()
            return "Pressing \(mods)\(args["key"] as? String ?? "")"
        case "open": return "Opening \(args["target"] as? String ?? "")"
        case "click_menu": return "Menu \((args["path"] as? [String] ?? []).joined(separator: " › "))"
        case "remember": return "Remembering “\(args["fact"] as? String ?? "")”"
        default: return name.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }

    // MARK: Implementations

    private static func context() -> [String: Any] {
        let area = Screen.bounds
        elements = AX.elements(in: area)
        let listed = elements.enumerated().map { i, e -> String in
            let p = Screen.normalized(CGPoint(x: e.frame.midX, y: e.frame.midY), in: area)
            return "\(i): \(e.role.dropFirst(2)) \"\(e.label)\" at (\(p.x), \(p.y))"
        }
        return [
            "app": NSWorkspace.shared.frontmostApplication?.localizedName ?? "",
            "window": AX.focusedWindowTitle(),
            "focused": AX.focusedElement(),
            "clipboard": String((NSPasteboard.general.string(forType: .string) ?? "").prefix(300)),
            "elements": listed,
        ]
    }

    private static func open(_ target: String) async -> [String: Any] {
        let path = (target as NSString).expandingTildeInPath
        var url: URL?
        if target.contains("://") {
            url = URL(string: target)
        } else if FileManager.default.fileExists(atPath: path) {
            url = URL(fileURLWithPath: path)
        } else if !target.contains(" "), target.contains("."), !target.lowercased().hasSuffix(".app") {
            url = URL(string: "https://\(target)")
        }
        if let url {
            return NSWorkspace.shared.open(url) ? ["ok": true] : ["error": "Could not open \(target)"]
        }
        let result = await run("/usr/bin/open", ["-a", target])
        return result.status == 0 ? ["ok": true] : ["error": "No app named \(target) was found"]
    }

    private static func osascript(_ script: String) async -> [String: Any] {
        let result = await run("/usr/bin/osascript", ["-e", script])
        if result.status == 0 { return ["ok": true, "result": String(result.output.prefix(4000))] }
        var response: [String: Any] = ["error": result.error.isEmpty ? "AppleScript failed" : String(result.error.prefix(1000))]
        if result.error.contains("-1743") {
            response["hint"] = "macOS blocked Aura from controlling that app. Ask the user to allow it in System Settings › Privacy & Security › Automation."
        }
        return response
    }

    private static func browserJavaScript(_ code: String) async -> [String: Any] {
        let app = NSWorkspace.shared.frontmostApplication?.localizedName ?? ""
        let js = code.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
        let chromium = ["Google Chrome", "Google Chrome Canary", "Chromium", "Brave Browser", "Microsoft Edge", "Arc", "Vivaldi", "Opera"]
        let script: String
        if app.hasPrefix("Safari") {
            script = "tell application \"\(app)\" to do JavaScript \"\(js)\" in current tab of front window"
        } else if chromium.contains(app) {
            script = "tell application \"\(app)\" to execute front window's active tab javascript \"\(js)\""
        } else {
            return ["error": "The frontmost app (\(app)) is not Safari or a Chrome-based browser."]
        }
        var result = await osascript(script)
        if result["error"] != nil, result["hint"] == nil {
            result["hint"] = "The browser may block scripting. Safari: enable Develop › Allow JavaScript from Apple Events. Chrome: View › Developer › Allow JavaScript from Apple Events. Otherwise use clicks and typing."
        }
        return result
    }

    private static func setClipboard(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    /// Run a command with a timeout; returns exit status, stdout and stderr.
    static func run(_ path: String, _ arguments: [String], timeout: TimeInterval = 30) async
        -> (status: Int32, output: String, error: String)
    {
        await withCheckedContinuation { done in
            DispatchQueue.global().async {
                let process = Process()
                process.executableURL = URL(fileURLWithPath: path)
                process.arguments = arguments
                let out = Pipe(), err = Pipe()
                process.standardOutput = out
                process.standardError = err
                do { try process.run() } catch {
                    return done.resume(returning: (-1, "", error.localizedDescription))
                }
                let killer = DispatchWorkItem { if process.isRunning { process.terminate() } }
                DispatchQueue.global().asyncAfter(deadline: .now() + timeout, execute: killer)
                let output = out.fileHandleForReading.readDataToEndOfFile()
                let error = err.fileHandleForReading.readDataToEndOfFile()
                process.waitUntilExit()
                killer.cancel()
                let trim = { (d: Data) in String(decoding: d, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines) }
                done.resume(returning: (process.terminationStatus, trim(output), trim(error)))
            }
        }
    }

    // MARK: Helpers

    private static func missing(_ what: String) -> [String: Any] { ["error": "Missing \(what)"] }
    private static func double(_ v: Any?) -> Double? { (v as? NSNumber)?.doubleValue ?? (v as? String).flatMap(Double.init) }
    private static func integer(_ v: Any?) -> Int? { double(v).map { Int($0) } }

    private static func fn(
        _ name: String, _ description: String, _ properties: [String: [String: Any]] = [:], required: [String] = []
    ) -> [String: Any] {
        // BLOCKING: Gemini waits for each result (and the fresh screenshot) before its next step.
        var declaration: [String: Any] = ["name": name, "description": description, "behavior": "BLOCKING"]
        if !properties.isEmpty {
            declaration["parameters"] = ["type": "OBJECT", "properties": properties, "required": required] as [String: Any]
        }
        return declaration
    }
    private static func str(_ d: String) -> [String: Any] { ["type": "STRING", "description": d] }
    private static func num(_ d: String) -> [String: Any] { ["type": "NUMBER", "description": d] }
    private static func int(_ d: String) -> [String: Any] { ["type": "INTEGER", "description": d] }
    private static func list(_ d: String) -> [String: Any] { ["type": "ARRAY", "items": ["type": "STRING"], "description": d] }
}
