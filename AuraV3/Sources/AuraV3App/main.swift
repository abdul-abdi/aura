import AppKit
import AuraV3

let app = NSApplication.shared
// AppState.shared is @MainActor-isolated. Top-level executable code runs on the main thread
// before the Swift concurrency runtime's actor executor is involved, so the access is safe.
// We use MainActor.assumeIsolated to satisfy the type system without a Task hop.
MainActor.assumeIsolated {
    _ = AppState.shared  // boots the log and writes session.start
    app.setActivationPolicy(.accessory)
}
app.run()
