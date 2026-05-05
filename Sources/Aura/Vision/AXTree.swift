import Foundation
import ApplicationServices

public struct AXNode: Sendable, Equatable {
    public let role: String
    public let label: String?
    public let title: String?
    public let frame: CGRect?
    public let children: [AXNode]
}

public enum AXTree {
    public static func snapshotFocusedApp(maxNodes: Int = 50,
                                            maxDepth: Int = 5,
                                            deadlineMs: Int = 500) -> AXNode? {
        guard AXIsProcessTrusted() else { return nil }
        let systemWide = AXUIElementCreateSystemWide()
        var focused: CFTypeRef?
        let rc = AXUIElementCopyAttributeValue(systemWide,
                                                 kAXFocusedApplicationAttribute as CFString,
                                                 &focused)
        guard rc == .success, let focused else { return nil }

        let deadline = Date().addingTimeInterval(Double(deadlineMs) / 1000.0)
        var counter = 0
        return walk(focused as! AXUIElement,
                     depth: 0, maxDepth: maxDepth,
                     deadline: deadline, counter: &counter, max: maxNodes)
    }

    private static func walk(_ element: AXUIElement,
                               depth: Int, maxDepth: Int,
                               deadline: Date,
                               counter: inout Int, max: Int) -> AXNode? {
        guard counter < max, Date() < deadline else { return nil }
        counter += 1
        let role = string(element, kAXRoleAttribute) ?? "AXUnknown"
        let label = string(element, kAXDescriptionAttribute) ?? string(element, "AXLabel")
        let title = string(element, kAXTitleAttribute)
        let frame = rect(element)

        var children: [AXNode] = []
        if depth < maxDepth {
            var rawKids: CFTypeRef?
            if AXUIElementCopyAttributeValue(element, kAXChildrenAttribute as CFString, &rawKids) == .success,
               let kidArray = rawKids as? [AXUIElement] {
                for k in kidArray {
                    if let child = walk(k, depth: depth + 1, maxDepth: maxDepth,
                                         deadline: deadline, counter: &counter, max: max) {
                        children.append(child)
                    }
                }
            }
        }
        return AXNode(role: role, label: label, title: title, frame: frame, children: children)
    }

    private static func string(_ element: AXUIElement, _ attr: String) -> String? {
        var v: CFTypeRef?
        if AXUIElementCopyAttributeValue(element, attr as CFString, &v) == .success {
            return v as? String
        }
        return nil
    }

    private static func rect(_ element: AXUIElement) -> CGRect? {
        var pos: CFTypeRef?, size: CFTypeRef?
        let rc1 = AXUIElementCopyAttributeValue(element, kAXPositionAttribute as CFString, &pos)
        let rc2 = AXUIElementCopyAttributeValue(element, kAXSizeAttribute as CFString, &size)
        guard rc1 == .success, rc2 == .success else { return nil }
        var p = CGPoint.zero, s = CGSize.zero
        if let pos { AXValueGetValue((pos as! AXValue), .cgPoint, &p) }
        if let size { AXValueGetValue((size as! AXValue), .cgSize, &s) }
        return CGRect(origin: p, size: s)
    }
}
