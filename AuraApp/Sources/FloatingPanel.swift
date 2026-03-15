import AppKit

/// A floating NSPanel that hovers above other windows.
/// Used as the main Aura conversation interface, anchored to the menu bar.
/// Styled as a borderless glass popover — no traffic lights, no title bar chrome.
final class FloatingPanel: NSPanel {
    init() {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 380, height: 520),
            styleMask: [.fullSizeContentView, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )

        isFloatingPanel = true
        level = .floating
        isMovableByWindowBackground = true
        backgroundColor = .clear
        isOpaque = false
        hasShadow = true
        animationBehavior = .utilityWindow

        // Size constraints
        minSize = NSSize(width: 320, height: 400)
        maxSize = NSSize(width: 500, height: 800)
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

    /// Dismiss when the panel loses key status (user clicked outside).
    override func resignKey() {
        super.resignKey()
        guard isVisible, !isHiding else { return }
        isHiding = true
        NotificationCenter.default.post(name: NSNotification.Name("AuraPanelDismiss"), object: nil)
    }

    /// Called by AppDelegate after the hide animation completes to reset the guard.
    func resetHidingState() {
        isHiding = false
    }
}
