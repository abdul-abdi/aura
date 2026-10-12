import AppKit
import ApplicationServices

/// Accessibility helpers: what's clickable on screen, and menu bar commands.
enum AX {
    struct Element {
        let ref: AXUIElement
        let role: String
        let label: String
        let frame: CGRect
    }

    static let interactiveRoles: Set<String> = [
        "AXButton", "AXLink", "AXTextField", "AXTextArea", "AXSearchField", "AXComboBox",
        "AXCheckBox", "AXRadioButton", "AXPopUpButton", "AXMenuButton", "AXSlider", "AXTab",
        "AXDisclosureTriangle", "AXCell", "AXMenuItem", "AXSegmentedControl", "AXIncrementor",
    ]

    static func app() -> (element: AXUIElement, name: String)? {
        guard let app = NSWorkspace.shared.frontmostApplication else { return nil }
        let element = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(element, 1)
        // Chrome and Electron apps only build their accessibility tree when asked.
        AXUIElementSetAttributeValue(element, "AXManualAccessibility" as CFString, kCFBooleanTrue)
        return (element, app.localizedName ?? "")
    }

    static func attribute<T>(_ element: AXUIElement, _ name: String) -> T? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
        return value as? T
    }

    static func children(_ element: AXUIElement) -> [AXUIElement] {
        attribute(element, kAXChildrenAttribute) ?? []
    }

    static func frame(_ element: AXUIElement) -> CGRect? {
        guard let p: AXValue = attribute(element, kAXPositionAttribute),
              let s: AXValue = attribute(element, kAXSizeAttribute)
        else { return nil }
        var origin = CGPoint.zero
        var size = CGSize.zero
        guard AXValueGetValue(p, .cgPoint, &origin), AXValueGetValue(s, .cgSize, &size) else { return nil }
        return CGRect(origin: origin, size: size)
    }

    static func label(_ element: AXUIElement) -> String {
        for name in [kAXTitleAttribute, kAXDescriptionAttribute, kAXValueAttribute, "AXPlaceholderValue", kAXHelpAttribute] {
            if let text: String = attribute(element, name), let clean = tidy(text) { return clean }
        }
        // Web links and buttons often keep their text in a child element.
        for child in children(element).prefix(3) {
            for name in [kAXValueAttribute, kAXTitleAttribute, kAXDescriptionAttribute] {
                if let text: String = attribute(child, name), let clean = tidy(text) { return clean }
            }
        }
        return ""
    }

    private static func tidy(_ text: String) -> String? {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines).replacingOccurrences(of: "\n", with: " ")
        return t.isEmpty ? nil : String(t.prefix(80))
    }

    /// Interactive elements of the frontmost window that are visible in `area`.
    static func elements(in area: CGRect, limit: Int = 80) -> [Element] {
        guard let app = app()?.element, let window: AXUIElement = attribute(app, kAXFocusedWindowAttribute) else { return [] }
        var queue = [window]
        var index = 0
        var found: [Element] = []
        let deadline = Date().addingTimeInterval(1.5)
        while index < queue.count, found.count < limit, index < 4000, Date() < deadline {
            let element = queue[index]
            index += 1
            let role: String = attribute(element, kAXRoleAttribute) ?? ""
            if interactiveRoles.contains(role), let f = frame(element), f.width > 1, f.height > 1, area.intersects(f) {
                found.append(Element(ref: element, role: role, label: label(element), frame: f))
            }
            queue.append(contentsOf: children(element))
        }
        return found
    }

    static func focusedWindowTitle() -> String {
        guard let app = app()?.element, let window: AXUIElement = attribute(app, kAXFocusedWindowAttribute) else { return "" }
        return attribute(window, kAXTitleAttribute) ?? ""
    }

    static func focusedElement() -> String {
        guard let app = app()?.element, let element: AXUIElement = attribute(app, kAXFocusedUIElementAttribute) else {
            return "none"
        }
        let role: String = attribute(element, kAXRoleAttribute) ?? "?"
        return "\(role.replacingOccurrences(of: "AX", with: "")) \"\(label(element))\""
    }

    static func press(_ element: AXUIElement) -> Bool {
        AXUIElementPerformAction(element, kAXPressAction as CFString) == .success
    }

    /// Click a menu bar item of the frontmost app, e.g. ["File", "Save As…"]. Returns an error message on failure.
    static func clickMenu(_ path: [String]) -> String? {
        guard let target = app(), let bar: AXUIElement = attribute(target.element, kAXMenuBarAttribute) else {
            return "The frontmost app has no menu bar"
        }
        let name = target.name
        var items = children(bar)
        for (i, wanted) in path.enumerated() {
            let titles = items.map { (attribute($0, kAXTitleAttribute) as String?) ?? "" }
            let key = normalize(wanted)
            guard let match = titles.firstIndex(where: { normalize($0) == key })
                ?? titles.firstIndex(where: { normalize($0).contains(key) })
            else {
                return "No menu item '\(wanted)' in \(name). Available: "
                    + titles.filter { !$0.isEmpty }.joined(separator: ", ")
            }
            if i == path.count - 1 { return press(items[match]) ? nil : "Could not click '\(wanted)'" }
            // A menu bar item / submenu item holds its menu as the only child.
            guard let menu = children(items[match]).first else { return "'\(wanted)' has no submenu" }
            items = children(menu)
        }
        return "Empty menu path"
    }

    private static func normalize(_ s: String) -> String {
        s.replacingOccurrences(of: "...", with: "…").trimmingCharacters(in: .whitespaces).lowercased()
    }
}
