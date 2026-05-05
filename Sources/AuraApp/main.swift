import AppKit
import SwiftUI
import Aura

let app = NSApplication.shared
app.setActivationPolicy(.accessory)

/// Top-level holder keeps the coordinator alive for the duration of NSApplication.run().
/// Declared outside MainActor.assumeIsolated so it's a real top-level binding with
/// lifetime tied to the process, not the scope of the assumeIsolated block.
@MainActor
final class CoordinatorHolder {
    static let shared = CoordinatorHolder()
    var coordinator: AppCoordinator?
    var menuBar: MenuBarController?     // must be retained — `_ = MenuBarController(...)` deallocs immediately, killing the Combine subscription and click target
    var onboardingWindow: NSWindow?
    var onboardingDelegate: OnboardingWindowDelegate?
}

/// Quits the app if the user closes the onboarding window without completing it.
/// Without this delegate, closing the red X leaves the app running with no API key
/// and no UX path to recover.
@MainActor
final class OnboardingWindowDelegate: NSObject, NSWindowDelegate {
    var didComplete: Bool = false
    func windowWillClose(_ notification: Notification) {
        if !didComplete {
            // User closed onboarding without finishing — there's no API key, so
            // staying open would leave the app permanently offline.
            NSApp.terminate(nil)
        }
    }
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
    // Default is true (legacy MRC behaviour); under ARC this causes a double-release
    // when the window is also retained by CoordinatorHolder. Standard modern fix.
    window.isReleasedWhenClosed = false
    window.title = "Welcome to Aura"
    let delegate = OnboardingWindowDelegate()
    window.delegate = delegate
    window.contentView = NSHostingView(rootView: OnboardingView {
        // onComplete: mark done so the delegate's windowWillClose
        // doesn't terminate the app, then dismiss + start coordinator.
        CoordinatorHolder.shared.onboardingDelegate?.didComplete = true
        CoordinatorHolder.shared.onboardingWindow?.close()
        CoordinatorHolder.shared.onboardingWindow = nil
        CoordinatorHolder.shared.onboardingDelegate = nil
        startCoordinator()
    })
    window.center()
    window.makeKeyAndOrderFront(nil)
    NSApp.activate(ignoringOtherApps: true)
    CoordinatorHolder.shared.onboardingWindow = window
    CoordinatorHolder.shared.onboardingDelegate = delegate
}

// AppState.shared is @MainActor-isolated. Top-level executable code runs on the main thread
// before the Swift concurrency runtime's actor executor is involved, so the access is safe.
// We use MainActor.assumeIsolated to satisfy the type system without a Task hop.
MainActor.assumeIsolated {
    let state = AppState.shared  // boots the log and writes session.start
    // Must retain the MenuBarController — its Combine subscription, click target,
    // and right-click menu actions all live on the instance. The system retains the
    // NSStatusItem so the dot remains visible regardless, but discarding the
    // controller silently kills the dot's reactivity.
    CoordinatorHolder.shared.menuBar = MenuBarController(state: state)
    _ = FloatingPanelController.shared  // singleton — retained internally

    if Config.apiKey() == nil {
        showOnboarding()
    } else {
        startCoordinator()
    }
}

app.run()
