import Combine
import SwiftUI

/// The window that drops down from the menu bar dot.
struct Panel: View {
    @ObservedObject var aura: Aura

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Circle().fill(Color(nsColor: aura.dot.color)).frame(width: 10, height: 10)
                Text(aura.status).font(.callout).lineLimit(3)
                Spacer()
                if aura.running {
                    Button(aura.muted ? "Unmute" : "Mute") { aura.toggleMute() }
                }
            }
            if aura.running {
                Transcript(lines: aura.lines)
                MessageField(aura: aura)
            } else {
                Setup(aura: aura)
            }
            HStack {
                Text("⌘⇧A mutes and unmutes").font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("Quit Aura") { NSApp.terminate(nil) }
            }
        }
        .padding(14)
        .frame(width: 380, height: 520)
    }
}

private struct Transcript: View {
    let lines: [Aura.Line]

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    if lines.isEmpty {
                        Text("Just talk — Aura is listening and can see your screen.").foregroundStyle(.secondary)
                    }
                    ForEach(lines) { line in
                        row(line).id(line.id)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .onChange(of: lines.last?.text) { _, _ in
                if let id = lines.last?.id { proxy.scrollTo(id, anchor: .bottom) }
            }
        }
    }

    @ViewBuilder
    private func row(_ line: Aura.Line) -> some View {
        switch line.kind {
        case .user:
            Text(line.text)
                .padding(8)
                .background(Color.accentColor.opacity(0.15), in: RoundedRectangle(cornerRadius: 8))
                .frame(maxWidth: .infinity, alignment: .trailing)
        case .aura:
            Text(line.text).textSelection(.enabled)
        case .action:
            Label(line.text, systemImage: line.failed ? "exclamationmark.triangle" : "cursorarrow.click")
                .font(.caption)
                .foregroundStyle(line.failed ? Color.orange : Color.secondary)
        }
    }
}

private struct MessageField: View {
    @ObservedObject var aura: Aura
    @State private var text = ""

    var body: some View {
        TextField("Or type a message…", text: $text)
            .textFieldStyle(.roundedBorder)
            .onSubmit {
                aura.send(text)
                text = ""
            }
    }
}

private struct Setup: View {
    @ObservedObject var aura: Aura
    @State private var key = Config.apiKey ?? ""
    @State private var microphone = Permissions.microphone
    @State private var screen = Permissions.screen
    @State private var accessibility = Permissions.accessibility
    @State private var error: String?
    private let poll = Timer.publish(every: 1, on: .main, in: .common).autoconnect()

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Welcome to Aura").font(.title2.bold())
            Text("Talk to your Mac. Aura hears you, sees your screen, and clicks and types for you.")
                .foregroundStyle(.secondary)

            VStack(alignment: .leading, spacing: 6) {
                Text("1. Gemini API key").font(.headline)
                HStack {
                    SecureField("Paste your key", text: $key).textFieldStyle(.roundedBorder)
                    Button("Paste") {
                        key = NSPasteboard.general.string(forType: .string)?
                            .trimmingCharacters(in: .whitespacesAndNewlines) ?? key
                    }
                }
                Link("Get a free key at aistudio.google.com", destination: URL(string: "https://aistudio.google.com/apikey")!)
                    .font(.caption)
            }

            VStack(alignment: .leading, spacing: 6) {
                Text("2. Permissions").font(.headline)
                permission("Microphone", "to hear you", microphone) {
                    Permissions.requestMicrophone { microphone = $0 }
                }
                permission("Screen Recording", "to see your screen", screen) { Permissions.requestScreen() }
                permission("Accessibility", "to click and type", accessibility) { Permissions.requestAccessibility() }
                if !screen {
                    Text("After allowing Screen Recording, macOS may ask you to quit and reopen Aura.")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }

            if let error { Text(error).font(.caption).foregroundStyle(.red) }
            Spacer()
            Button("Start Aura", action: start)
                .buttonStyle(.borderedProminent)
                .frame(maxWidth: .infinity)
                .disabled(key.trimmingCharacters(in: .whitespacesAndNewlines).count < 20 || !microphone)
        }
        .onReceive(poll) { _ in
            microphone = Permissions.microphone
            screen = Permissions.screen
            accessibility = Permissions.accessibility
        }
    }

    private func permission(_ name: String, _ why: String, _ granted: Bool, request: @escaping () -> Void) -> some View {
        HStack {
            Image(systemName: granted ? "checkmark.circle.fill" : "circle")
                .foregroundStyle(granted ? Color.green : Color.secondary)
            Text(name)
            Text(why).font(.caption).foregroundStyle(.secondary)
            Spacer()
            if !granted { Button("Allow", action: request) }
        }
    }

    private func start() {
        do {
            try Config.save(apiKey: key.trimmingCharacters(in: .whitespacesAndNewlines))
            error = nil
            aura.start()
        } catch {
            self.error = error.localizedDescription
        }
    }
}
