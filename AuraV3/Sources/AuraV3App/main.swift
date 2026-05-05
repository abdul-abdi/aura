import AppKit
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
}

// AppState.shared is @MainActor-isolated. Top-level executable code runs on the main thread
// before the Swift concurrency runtime's actor executor is involved, so the access is safe.
// We use MainActor.assumeIsolated to satisfy the type system without a Task hop.
MainActor.assumeIsolated {
    let state = AppState.shared  // boots the log and writes session.start
    _ = MenuBarController(state: state)
    _ = FloatingPanelController.shared  // forces lazy init + hotkey registration
    let coordinator = AppCoordinator()
    CoordinatorHolder.shared.coordinator = coordinator
    coordinator.start()
}
app.run()
