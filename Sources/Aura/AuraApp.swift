import AppKit
import Carbon.HIToolbox
import SwiftUI

@main
struct AuraApp: App {
    @StateObject private var aura = Aura()

    var body: some Scene {
        MenuBarExtra {
            Panel(aura: aura)
        } label: {
            Image(nsImage: aura.dot.icon)
        }
        .menuBarExtraStyle(.window)
    }
}

extension Aura.Dot {
    var color: NSColor {
        switch self {
        case .off, .muted: .systemGray
        case .connecting: .systemOrange
        case .listening: .systemGreen
        case .speaking: .systemBlue
        case .problem: .systemRed
        }
    }

    /// A colored dot for the menu bar (not a template image, so it keeps its color).
    var icon: NSImage {
        let color = color
        let image = NSImage(size: NSSize(width: 16, height: 16), flipped: false) { rect in
            color.setFill()
            NSBezierPath(ovalIn: rect.insetBy(dx: 3, dy: 3)).fill()
            return true
        }
        image.isTemplate = false
        return image
    }
}

/// Global ⌘⇧A shortcut to mute and unmute.
enum HotKey {
    private static var action: (() -> Void)?

    static func register(_ action: @escaping () -> Void) {
        self.action = action
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), { _, _, _ in
            DispatchQueue.main.async { HotKey.action?() }
            return noErr
        }, 1, &spec, nil, nil)
        var ref: EventHotKeyRef?
        let id = EventHotKeyID(signature: OSType(0x4155_5241), id: 1)  // "AURA"
        RegisterEventHotKey(UInt32(kVK_ANSI_A), UInt32(cmdKey | shiftKey), id, GetApplicationEventTarget(), 0, &ref)
    }
}
