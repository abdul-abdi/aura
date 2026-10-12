import CoreGraphics
import Foundation

/// Synthetic mouse and keyboard events (needs the Accessibility permission).
enum Input {
    static func move(to p: CGPoint) {
        post(CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: p, mouseButton: .left))
    }

    static func click(at p: CGPoint, right: Bool = false, count: Int = 1) {
        move(to: p)
        usleep(40_000)  // let the app register the hover
        let (down, up, button): (CGEventType, CGEventType, CGMouseButton) =
            right ? (.rightMouseDown, .rightMouseUp, .right) : (.leftMouseDown, .leftMouseUp, .left)
        for n in 1...max(1, min(count, 3)) {
            for type in [down, up] {
                let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: p, mouseButton: button)
                event?.setIntegerValueField(.mouseEventClickState, value: Int64(n))
                post(event)
            }
        }
    }

    static func drag(from a: CGPoint, to b: CGPoint) {
        move(to: a)
        usleep(40_000)
        post(CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown, mouseCursorPosition: a, mouseButton: .left))
        for i in 1...20 {
            let t = CGFloat(i) / 20
            let p = CGPoint(x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t)
            post(CGEvent(mouseEventSource: nil, mouseType: .leftMouseDragged, mouseCursorPosition: p, mouseButton: .left))
            usleep(10_000)
        }
        post(CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: b, mouseButton: .left))
    }

    /// Positive `lines` scroll down / right.
    static func scroll(at p: CGPoint, down lines: Int32 = 0, right columns: Int32 = 0) {
        move(to: p)
        usleep(20_000)
        post(CGEvent(scrollWheelEvent2Source: nil, units: .line, wheelCount: 2, wheel1: -lines, wheel2: -columns, wheel3: 0))
    }

    /// Press a key with modifiers, e.g. press("c", ["cmd"]). Returns false for unknown keys.
    static func press(_ key: String, modifiers: [String] = []) -> Bool {
        guard let code = keyCodes[key.lowercased()] else { return false }
        var flags = CGEventFlags()
        for m in modifiers {
            switch m.lowercased() {
            case "cmd", "command": flags.insert(.maskCommand)
            case "shift": flags.insert(.maskShift)
            case "option", "opt", "alt": flags.insert(.maskAlternate)
            case "ctrl", "control": flags.insert(.maskControl)
            case "fn": flags.insert(.maskSecondaryFn)
            default: break
            }
        }
        for down in [true, false] {
            let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down)
            event?.flags = flags
            post(event)
        }
        return true
    }

    /// Type text into the focused field, a few characters per event.
    static func type(_ text: String) {
        var rest = Substring(text)
        while !rest.isEmpty {
            let chunk = Array(String(rest.prefix(8)).utf16)
            rest = rest.dropFirst(8)
            for down in [true, false] {
                let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down)
                event?.flags = []
                event?.keyboardSetUnicodeString(stringLength: chunk.count, unicodeString: chunk)
                post(event)
            }
            usleep(8_000)
        }
    }

    private static func post(_ event: CGEvent?) { event?.post(tap: .cghidEventTap) }

    /// macOS virtual key codes (ANSI layout).
    static let keyCodes: [String: CGKeyCode] = {
        var map: [String: CGKeyCode] = [
            "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11,
            "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21,
            "6": 22, "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29, "]": 30, "o": 31,
            "u": 32, "[": 33, "i": 34, "p": 35, "l": 37, "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42,
            ",": 43, "/": 44, "n": 45, "m": 46, ".": 47, "`": 50,
            "return": 36, "enter": 36, "tab": 48, "space": 49, "delete": 51, "backspace": 51,
            "escape": 53, "esc": 53, "forwarddelete": 117, "home": 115, "end": 119,
            "pageup": 116, "pagedown": 121, "left": 123, "right": 124, "down": 125, "up": 126,
        ]
        let fKeys: [CGKeyCode] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111]
        for (i, code) in fKeys.enumerated() { map["f\(i + 1)"] = code }
        return map
    }()
}
