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

/// Build a minimal main menu so standard editing keybindings work in any
/// NSTextField/SecureField. Without an Edit menu, Cmd+C/Cmd+V/Cmd+X never
/// reach the responder chain — paste is silently dead in onboarding's API
/// key field. This is the platform's expected wiring; AppKit doesn't auto-
/// generate it for `.accessory`-policy apps.
@MainActor
func installMainMenu() {
    let mainMenu = NSMenu()

    // App menu (the bold "Aura" item)
    let appMenuItem = NSMenuItem()
    let appMenu = NSMenu()
    appMenu.addItem(withTitle: "About Aura",
                    action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)),
                    keyEquivalent: "")
    appMenu.addItem(NSMenuItem.separator())
    appMenu.addItem(withTitle: "Hide Aura",
                    action: #selector(NSApplication.hide(_:)),
                    keyEquivalent: "h")
    let hideOthers = appMenu.addItem(withTitle: "Hide Others",
                                     action: #selector(NSApplication.hideOtherApplications(_:)),
                                     keyEquivalent: "h")
    hideOthers.keyEquivalentModifierMask = [.option, .command]
    appMenu.addItem(withTitle: "Show All",
                    action: #selector(NSApplication.unhideAllApplications(_:)),
                    keyEquivalent: "")
    appMenu.addItem(NSMenuItem.separator())
    appMenu.addItem(withTitle: "Quit Aura",
                    action: #selector(NSApplication.terminate(_:)),
                    keyEquivalent: "q")
    appMenuItem.submenu = appMenu
    mainMenu.addItem(appMenuItem)

    // Edit menu — load-bearing. NSText* responders only respond to cut/copy/
    // paste/selectAll when an Edit menu with the standard items exists in
    // NSApp.mainMenu. Without these, Cmd+V is a silent no-op in text fields.
    let editMenuItem = NSMenuItem()
    let editMenu = NSMenu(title: "Edit")
    editMenu.addItem(withTitle: "Undo",
                     action: Selector(("undo:")), keyEquivalent: "z")
    let redo = editMenu.addItem(withTitle: "Redo",
                                action: Selector(("redo:")), keyEquivalent: "z")
    redo.keyEquivalentModifierMask = [.shift, .command]
    editMenu.addItem(NSMenuItem.separator())
    editMenu.addItem(withTitle: "Cut",
                     action: #selector(NSText.cut(_:)), keyEquivalent: "x")
    editMenu.addItem(withTitle: "Copy",
                     action: #selector(NSText.copy(_:)), keyEquivalent: "c")
    editMenu.addItem(withTitle: "Paste",
                     action: #selector(NSText.paste(_:)), keyEquivalent: "v")
    editMenu.addItem(withTitle: "Select All",
                     action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
    editMenuItem.submenu = editMenu
    mainMenu.addItem(editMenuItem)

    NSApp.mainMenu = mainMenu
}

@MainActor
func startCoordinator() {
    let coordinator = AppCoordinator()
    CoordinatorHolder.shared.coordinator = coordinator
    coordinator.start()
}

@MainActor
func showOnboarding() {
    // While onboarding is on screen, behave as a regular foregroundable app so
    // the window can be a proper key window and accept keyboard input. Without
    // this, .accessory denies focus to non-status-bar windows and Cmd-V/typing
    // are flaky. We switch back to .accessory in the onComplete handler.
    NSApp.setActivationPolicy(.regular)

    let window = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: 520, height: 380),
        styleMask: [.titled, .closable, .fullSizeContentView],
        backing: .buffered, defer: false
    )
    // Default is true (legacy MRC behaviour); under ARC this causes a double-release
    // when the window is also retained by CoordinatorHolder. Standard modern fix.
    window.isReleasedWhenClosed = false
    window.title = "Welcome to Aura"
    window.titlebarAppearsTransparent = true
    window.isMovableByWindowBackground = true

    let delegate = OnboardingWindowDelegate()
    window.delegate = delegate
    window.contentView = NSHostingView(rootView: OnboardingView {
        // onComplete: mark done so the delegate's windowWillClose
        // doesn't terminate the app, then dismiss + start coordinator.
        CoordinatorHolder.shared.onboardingDelegate?.didComplete = true
        CoordinatorHolder.shared.onboardingWindow?.close()
        CoordinatorHolder.shared.onboardingWindow = nil
        CoordinatorHolder.shared.onboardingDelegate = nil
        // Back to menu-bar-only mode now that we're done.
        NSApp.setActivationPolicy(.accessory)
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
    installMainMenu()  // Cmd-C/V/X work in text fields after this

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
