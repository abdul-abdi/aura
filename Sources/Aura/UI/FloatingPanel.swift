import AppKit
import SwiftUI
import Carbon.HIToolbox

@MainActor
public final class FloatingPanelController {
    public static let shared = FloatingPanelController()

    private let panel: NSPanel
    private var hotKeyRef: EventHotKeyRef?
    // The InstallEventHandler reference is intentionally leaked: we register
    // exactly once at app start and never unregister (the handler lives until
    // process exit). The leak is bounded and innocuous.

    private init() {
        self.panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 720, height: 520),
            styleMask: [.titled, .closable, .resizable, .nonactivatingPanel, .fullSizeContentView],
            backing: .buffered, defer: false)
        panel.isFloatingPanel = true
        panel.level = .floating
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.backgroundColor = NSColor.windowBackgroundColor.withAlphaComponent(0.92)
        panel.isReleasedWhenClosed = false
        panel.contentView = NSHostingView(rootView: PanelRoot())
        registerHotKey()
    }

    public func toggle() {
        if panel.isVisible {
            panel.orderOut(nil)
        } else {
            panel.center()
            panel.makeKeyAndOrderFront(nil)
        }
    }

    private func registerHotKey() {
        // Amendment U: 'AUR3' (0x41555233) — unique to Aura v3, avoids collisions.
        let id = EventHotKeyID(signature: OSType(0x41555233), id: 1)
        let modifiers: UInt32 = UInt32(cmdKey | shiftKey)
        let keyCode: UInt32 = UInt32(kVK_ANSI_A)
        var ref: EventHotKeyRef?
        let status = RegisterEventHotKey(keyCode, modifiers, id, GetApplicationEventTarget(), 0, &ref)
        if status == noErr { self.hotKeyRef = ref }
        var spec = EventTypeSpec(
            eventClass: OSType(kEventClassKeyboard),
            eventKind: UInt32(kEventHotKeyPressed)
        )
        // Handler reference intentionally not retained — see class doc.
        InstallEventHandler(
            GetApplicationEventTarget(),
            { _, _, _ in
                DispatchQueue.main.async { FloatingPanelController.shared.toggle() }
                return noErr
            },
            1, &spec, nil, nil
        )
    }
}

private struct PanelRoot: View {
    var body: some View {
        VStack(spacing: 0) {
            GoalStackView()
            CarryingForwardStrip()
            Divider()
            HStack(spacing: 0) {
                ConversationPane()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                Divider()
                MindPlaceholder()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }
}

private struct ConversationPane: View {
    var body: some View {
        TranscriptView(log: AppState.shared.log)
    }
}

private struct MindPlaceholder: View {
    var body: some View {
        VStack {
            Text("Mind").foregroundStyle(.secondary).padding()
            Spacer()
        }
    }
}
