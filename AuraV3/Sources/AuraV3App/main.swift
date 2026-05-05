import AppKit
import SwiftUI
import AuraV3

let app = NSApplication.shared
app.setActivationPolicy(.accessory)

/// Top-level holder keeps the coordinator alive for the duration of NSApplication.run().
/// Declared outside MainActor.assumeIsolated so it's a real top-level binding with
/// lifetime tied to the process, not the scope of the assumeIsolated block.
@MainActor
final class CoordinatorHolder {
    static let shared = CoordinatorHolder()
    var coordinator: AppCoordinator?
    var onboardingWindow: NSWindow?
}

@MainActor
func startCoordinator() {
    let coordinator = AppCoordinator()
    CoordinatorHolder.shared.coordinator = coordinator
    coordinator.start()
}

@MainActor
func showOnboarding() {
    let window = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: 520, height: 360),
        styleMask: [.titled, .closable],
        backing: .buffered, defer: false
    )
    window.title = "Welcome to Aura"
    window.contentView = NSHostingView(rootView: OnboardingView {
        // onComplete: dismiss window + start coordinator
        CoordinatorHolder.shared.onboardingWindow?.close()
        CoordinatorHolder.shared.onboardingWindow = nil
        startCoordinator()
    })
    window.center()
    window.makeKeyAndOrderFront(nil)
    NSApp.activate(ignoringOtherApps: true)
    CoordinatorHolder.shared.onboardingWindow = window
}

// AppState.shared is @MainActor-isolated. Top-level executable code runs on the main thread
// before the Swift concurrency runtime's actor executor is involved, so the access is safe.
// We use MainActor.assumeIsolated to satisfy the type system without a Task hop.
MainActor.assumeIsolated {
    let state = AppState.shared  // boots the log and writes session.start
    _ = MenuBarController(state: state)
    _ = FloatingPanelController.shared  // forces lazy init + hotkey registration

    if Config.apiKey() == nil {
        showOnboarding()
    } else {
        startCoordinator()
    }
}

app.run()
