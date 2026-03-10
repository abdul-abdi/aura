import AppKit

/// A floating NSPanel that hovers above other windows.
/// Used as the main Aura conversation interface, anchored to the menu bar.
final class FloatingPanel: NSPanel {
    init() {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 380, height: 520),
            styleMask: [.titled, .closable, .fullSizeContentView, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )

        isFloatingPanel = true
        level = .floating
        titlebarAppearsTransparent = true
        titleVisibility = .hidden
        isMovableByWindowBackground = true
        backgroundColor = .clear
        isOpaque = false
        hasShadow = true
        animationBehavior = .utilityWindow

        // Size constraints
        minSize = NSSize(width: 320, height: 400)
        maxSize = NSSize(width: 500, height: 800)

        // Allow the panel to become key (for text input) but not main
        // canBecomeKey is overridden below
    }

    // Allow text input focus
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    // Close on Escape key
    override func cancelOperation(_ sender: Any?) {
        animator().alphaValue = 0
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { [weak self] in
            self?.orderOut(nil)
            self?.alphaValue = 1
        }
    }

    // Resign key when clicking outside
    override func resignKey() {
        super.resignKey()
        animator().alphaValue = 0
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { [weak self] in
            self?.orderOut(nil)
            self?.alphaValue = 1
        }
    }
}
