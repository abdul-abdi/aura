import AppKit
import Combine

@MainActor
public final class MenuBarController {
    private let item: NSStatusItem
    private var cancellable: AnyCancellable?

    public init(state: AppState) {
        self.item = NSStatusBar.system.statusItem(withLength: 24)
        self.item.button?.image = MenuBarController.image(for: state.dotState)
        self.item.button?.imagePosition = .imageOnly
        self.item.button?.target = self
        self.item.button?.action = #selector(onClick)
        self.cancellable = state.$dotState.sink { [weak self] s in
            self?.item.button?.image = MenuBarController.image(for: s)
        }
        installRightClickMenu()
    }

    @objc private func onClick(_ sender: Any?) {
        FloatingPanelController.shared.toggle()
    }

    private func installRightClickMenu() {
        let menu = NSMenu()
        menu.addItem(withTitle: "Mute mic", action: #selector(muteMic), keyEquivalent: "m").target = self
        menu.addItem(withTitle: "Mute proactive", action: #selector(muteProactive), keyEquivalent: "p").target = self
        menu.addItem(NSMenuItem.separator())
        menu.addItem(withTitle: "Quit Aura", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        item.menu = menu
    }

    @objc private func muteMic() { AppState.shared.toggleMic() }
    @objc private func muteProactive() { AppState.shared.toggleProactive() }

    private static func image(for state: AppState.DotState) -> NSImage {
        let size = NSSize(width: 18, height: 18)
        let img = NSImage(size: size)
        img.lockFocus()
        defer { img.unlockFocus() }
        let color: NSColor
        switch state {
        case .idle:      color = .systemGray
        case .listening: color = .systemGreen
        case .speaking:  color = .systemBlue
        case .rotating:  color = .systemOrange
        case .offline:   color = .systemRed
        }
        color.setFill()
        NSBezierPath(ovalIn: NSRect(x: 4, y: 4, width: 10, height: 10)).fill()
        img.isTemplate = false
        return img
    }
}
