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

    // Guard against double-firing cancelOperation while a hide animation is already in flight
    private var isHiding = false

    /// Close on Escape — post notification so AppDelegate handles the animated dismiss.
    override func cancelOperation(_ sender: Any?) {
        guard !isHiding else { return }
        isHiding = true
        NotificationCenter.default.post(name: NSNotification.Name("AuraPanelDismiss"), object: nil)
    }

    /// Called by AppDelegate after the hide animation completes to reset the guard.
    func resetHidingState() {
        isHiding = false
    }

    // resignKey() override intentionally removed — clicking outside should NOT auto-dismiss
    // a floating panel. Users expect to interact with other apps while the panel stays open.
    // Escape (cancelOperation) is the standard dismiss gesture.
}
