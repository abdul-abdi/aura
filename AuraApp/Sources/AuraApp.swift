import AppKit
import Carbon
import Sparkle
import SwiftUI

@main
struct AuraApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        // We use a MenuBarExtra-free approach — the AppDelegate manages the NSStatusItem
        // and floating panel directly for full control over behavior.
        Settings {
            EmptyView()
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem!
    private var floatingPanel: FloatingPanel?
    private var appState = AppState()
    private var connection: DaemonConnection?
    private var daemonProcess: Process?
    private var hotKeyRef: UnsafeMutableRawPointer?
    private var dotColorTimer: Timer?
    private let updaterController = SPUStandardUpdaterController(
        startingUpdater: true, updaterDelegate: nil, userDriverDelegate: nil
    )

    // MARK: - Lifecycle

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Hide dock icon — this is a menu bar app
        NSApp.setActivationPolicy(.accessory)

        setupStatusItem()
        setupFloatingPanel()
        setupGlobalHotKey()
        launchDaemonAndConnect()

        // Auto-open panel on first launch so the user sees the welcome screen
        if appState.showOnboarding {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
                self?.togglePanel()
            }
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        dotColorTimer?.invalidate()
        dotColorTimer = nil
        appState.requestShutdown()
        connection?.disconnect()
        terminateDaemon()
    }

    // MARK: - Status Item

    private func setupStatusItem() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)

        guard let button = statusItem.button else { return }

        updateStatusItemIcon(color: .gray)

        button.action = #selector(statusItemClicked(_:))
        button.target = self
        button.sendAction(on: [.leftMouseUp, .rightMouseUp])

        // Accessibility
        button.setAccessibilityLabel("Aura: disconnected")
        button.toolTip = "Aura — ⌘⇧A to toggle"
    }

    private func updateStatusItemIcon(color: DotColorName) {
        guard let button = statusItem.button else { return }

        let size = NSSize(width: 18, height: 18)
        let image = NSImage(size: size, flipped: false) { rect in
            let dotSize: CGFloat = 10
            let offset = (18 - dotSize) / 2

            let (r, g, b) = color.rgb
            let nsColor = NSColor(red: r, green: g, blue: b, alpha: 1.0)

            // Draw glow for active states
            if color != .gray {
                let glowColor = NSColor(red: r, green: g, blue: b, alpha: 0.25)
                glowColor.setFill()
                let glowInset: CGFloat = 2
                let glowRect = NSRect(
                    x: offset - glowInset,
                    y: offset - glowInset,
                    width: dotSize + glowInset * 2,
                    height: dotSize + glowInset * 2
                )
                NSBezierPath(ovalIn: glowRect).fill()
            }

            nsColor.setFill()
            let dotRect = NSRect(x: offset, y: offset, width: dotSize, height: dotSize)
            NSBezierPath(ovalIn: dotRect).fill()

            return true
        }

        image.isTemplate = false
        button.image = image
    }

    @objc private func statusItemClicked(_ sender: Any?) {
        guard let event = NSApp.currentEvent else { return }

        if event.type == .rightMouseUp {
            showContextMenu()
        } else {
            togglePanel()
        }
    }

    private func showContextMenu() {
        let menu = NSMenu()

        let toggleItem = NSMenuItem(
            title: "Toggle Panel  ⌘⇧A",
            action: #selector(togglePanelClicked),
            keyEquivalent: ""
        )
        toggleItem.target = self
        menu.addItem(toggleItem)

        menu.addItem(.separator())

        let reconnectItem = NSMenuItem(
            title: "Reconnect",
            action: #selector(reconnectClicked),
            keyEquivalent: ""
        )
        reconnectItem.target = self
        menu.addItem(reconnectItem)

        let updateItem = NSMenuItem(
            title: "Check for Updates...",
            action: #selector(SPUStandardUpdaterController.checkForUpdates(_:)),
            keyEquivalent: ""
        )
        updateItem.target = updaterController
        menu.addItem(updateItem)

        menu.addItem(.separator())

        let quitItem = NSMenuItem(
            title: "Quit Aura",
            action: #selector(quitClicked),
            keyEquivalent: "q"
        )
        quitItem.target = self
        menu.addItem(quitItem)

        statusItem.menu = menu
        statusItem.button?.performClick(nil)
        // Remove menu after showing so left-click works next time
        statusItem.menu = nil
    }

    @objc private func togglePanelClicked() {
        togglePanel()
    }

    @objc private func reconnectClicked() {
        appState.requestReconnect()
    }

    @objc private func quitClicked() {
        NSApp.terminate(nil)
    }

    // MARK: - Floating Panel

    private func setupFloatingPanel() {
        let panel = FloatingPanel()
        let hostingView = NSHostingView(rootView: ContentView(appState: appState))
        hostingView.frame = panel.contentView!.bounds
        hostingView.autoresizingMask = [.width, .height]
        panel.contentView?.addSubview(hostingView)

        self.floatingPanel = panel

        // Observe Escape-key dismiss notification posted by FloatingPanel.cancelOperation
        NotificationCenter.default.addObserver(
            forName: NSNotification.Name("AuraPanelDismiss"),
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.hidePanel()
                // Reset guard after the animation so a subsequent Escape works
                try? await Task.sleep(nanoseconds: 200_000_000)
                self?.floatingPanel?.resetHidingState()
            }
        }
    }

    func togglePanel() {
        guard let panel = floatingPanel else { return }

        if panel.isVisible {
            hidePanel()
        } else {
            showPanel()
        }
    }

    private func showPanel() {
        guard let panel = floatingPanel else { return }
        positionPanel()

        // Capture the final (correctly positioned) frame before we shrink it
        let finalFrame = panel.frame
        let scaledFrame = NSRect(
            x: finalFrame.midX - finalFrame.width * 0.48,
            y: finalFrame.midY - finalFrame.height * 0.48,
            width: finalFrame.width * 0.96,
            height: finalFrame.height * 0.96
        )

        // Start from a slightly-scaled-down, transparent state
        panel.alphaValue = 0
        panel.setFrame(scaledFrame, display: false)
        panel.makeKeyAndOrderFront(nil)

        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.2
            context.timingFunction = CAMediaTimingFunction(name: .easeOut)
            panel.animator().alphaValue = 1
            panel.animator().setFrame(finalFrame, display: true)
        } completionHandler: {
            // Notify SwiftUI views that the panel is visible so they can grab focus
            NotificationCenter.default.post(
                name: NSNotification.Name("AuraPanelDidShow"), object: nil
            )
        }
    }

    private func hidePanel() {
        guard let panel = floatingPanel else { return }

        let currentFrame = panel.frame
        let scaledFrame = NSRect(
            x: currentFrame.midX - currentFrame.width * 0.48,
            y: currentFrame.midY - currentFrame.height * 0.48,
            width: currentFrame.width * 0.96,
            height: currentFrame.height * 0.96
        )

        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.15
            context.timingFunction = CAMediaTimingFunction(name: .easeIn)
            panel.animator().alphaValue = 0
            panel.animator().setFrame(scaledFrame, display: true)
        } completionHandler: {
            panel.orderOut(nil)
            // Reset state for the next show
            panel.alphaValue = 1
            panel.setFrame(currentFrame, display: false)
        }
    }

    private func positionPanel() {
        guard let panel = floatingPanel,
              let button = statusItem.button,
              let buttonWindow = button.window else { return }

        let buttonRect = button.convert(button.bounds, to: nil)
        let screenRect = buttonWindow.convertToScreen(buttonRect)

        let panelWidth = panel.frame.width
        let x = screenRect.midX - panelWidth / 2
        let y = screenRect.minY - 4

        panel.setFrameTopLeftPoint(NSPoint(x: x, y: y))
    }

    // MARK: - Global Hot Key (Cmd+Shift+A)

    private func setupGlobalHotKey() {
        // Register Cmd+Shift+A as global hotkey
        var hotKeyID = EventHotKeyID()
        hotKeyID.signature = OSType(0x41555241) // "AURA"
        hotKeyID.id = 1

        let modifiers: UInt32 = UInt32(cmdKey | shiftKey)
        let keyCode: UInt32 = UInt32(kVK_ANSI_A)

        var hotKeyRefUnmanaged: EventHotKeyRef?
        let status = RegisterEventHotKey(
            keyCode,
            modifiers,
            hotKeyID,
            GetEventDispatcherTarget(),
            0,
            &hotKeyRefUnmanaged
        )

        if status == noErr {
            // Install handler
            var eventSpec = EventTypeSpec(
                eventClass: OSType(kEventClassKeyboard),
                eventKind: UInt32(kEventHotKeyPressed)
            )

            let selfPtr = Unmanaged.passUnretained(self).toOpaque()

            InstallEventHandler(
                GetEventDispatcherTarget(),
                { (_, event, userData) -> OSStatus in
                    guard let userData else { return OSStatus(eventNotHandledErr) }
                    let delegate = Unmanaged<AppDelegate>.fromOpaque(userData)
                        .takeUnretainedValue()
                    DispatchQueue.main.async {
                        delegate.togglePanel()
                    }
                    return noErr
                },
                1,
                &eventSpec,
                selfPtr,
                nil
            )
        }
    }

    // MARK: - Daemon Management

    private func launchDaemonAndConnect() {
        launchDaemon()

        connection = DaemonConnection()
        appState.configure(connection: connection!)

        Task {
            await connection?.connect()
            if connection?.isConnected == true {
                appState.markConnected()
                updateStatusItemIcon(color: .green)
            }
        }

        // Observe dot color changes to update the status item icon
        observeDotColor()
    }

    private func launchDaemon() {
        // Look for aura-daemon in the app bundle first, then in PATH
        let bundlePath = Bundle.main.bundlePath
        let daemonInBundle = "\(bundlePath)/Contents/MacOS/aura-daemon"

        let daemonPath: String
        if FileManager.default.fileExists(atPath: daemonInBundle) {
            daemonPath = daemonInBundle
        } else {
            // Development: look for the cargo-built binary
            let devPath = findDevelopmentDaemon()
            guard let path = devPath else {
                appState.statusMessage = "Daemon not found"
                return
            }
            daemonPath = path
        }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: daemonPath)
        process.arguments = ["--headless"]
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice

        do {
            try process.run()
            daemonProcess = process
        } catch {
            appState.statusMessage = "Failed to launch daemon"
        }
    }

    private func findDevelopmentDaemon() -> String? {
        // Check relative to workspace root during development
        let candidates = [
            "../target/release/aura-daemon",
            "../target/debug/aura-daemon",
        ]

        let bundlePath = Bundle.main.bundlePath
        for candidate in candidates {
            let url = URL(fileURLWithPath: bundlePath)
                .appendingPathComponent(candidate)
                .standardized
            if FileManager.default.fileExists(atPath: url.path) {
                return url.path
            }
        }

        // In production the daemon is always in the bundle.
        // Skip `which` lookup — it blocks the main thread.
        return nil
    }

    private func terminateDaemon() {
        guard let process = daemonProcess, process.isRunning else { return }
        process.terminate()
        daemonProcess = nil
    }

    private func observeDotColor() {
        // Poll dot color changes to update the menu bar icon
        // Using a simple timer since @Observable doesn't bridge to NSStatusItem directly
        dotColorTimer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
            guard let self else { return }
            Task { @MainActor in
                self.updateStatusItemIcon(color: self.appState.dotColor)
                self.statusItem.button?.setAccessibilityLabel("Aura: \(self.appState.statusMessage)")
            }
        }
    }
}

// MARK: - DotColorName helpers

extension DotColorName {
    var rgb: (CGFloat, CGFloat, CGFloat) {
        switch self {
        case .gray: return (0.55, 0.55, 0.55)
        case .green: return (0.30, 0.88, 0.52)
        case .amber: return (1.0, 0.78, 0.28)
        case .red: return (0.92, 0.28, 0.28)
        }
    }
}
