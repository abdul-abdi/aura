import SwiftUI
import Combine

// MARK: - TranscriptRow

/// One display row in the transcript.
public enum TranscriptRow: Identifiable, Equatable {
    case user(id: Int64, text: String)
    case assistant(id: Int64, text: String)
    case toolCall(id: Int64, name: String, args: String)
    case toolResult(id: Int64, success: Bool, detail: String)

    public var id: Int64 {
        switch self {
        case .user(let id, _):           return id
        case .assistant(let id, _):      return id
        case .toolCall(let id, _, _):    return id
        case .toolResult(let id, _, _):  return id
        }
    }
}

// MARK: - TranscriptViewModel

/// Loads and polls the event log, publishing rows for SwiftUI to render.
///
/// Polling replaces a reactive subscription until Plan 4 amendment CH lands a
/// `Log.subscribe` API. At that point, replace the timer loop with a subscription.
@MainActor
public final class TranscriptViewModel: ObservableObject {
    @Published public private(set) var rows: [TranscriptRow] = []

    private let log: Log
    /// Highest event id seen so far. Used to deduplicate on each poll (`id > maxSeenID`).
    private var maxSeenID: Int64 = 0
    /// ts_mono cursor passed to SQL `since:` for a cheap perf filter.
    /// SQL uses `>=`, so the real dedup is done by id comparison above.
    private var lastTsMono: UInt64 = 0

    private var pollTask: Task<Void, Never>?

    public init(log: Log) {
        self.log = log
    }

    // MARK: - Load / poll

    /// Queries all four turn kinds, merges by ts_mono, appends new rows.
    /// Safe to call directly from tests without waiting for the timer.
    public func refresh() async {
        do {
            let kinds: [String] = [
                EventKind.turnUser,
                EventKind.turnAssistant,
                EventKind.turnToolCall,
                EventKind.turnToolResult,
            ]
            var fetched: [LogEvent] = []
            for kind in kinds {
                let events = try await log.query(kind: kind, since: lastTsMono)
                fetched.append(contentsOf: events)
            }
            // Deduplicate: only events whose id we haven't seen yet.
            let newEvents = fetched.filter { $0.id > maxSeenID }
            guard !newEvents.isEmpty else { return }

            let sorted = newEvents.sorted { $0.tsMono < $1.tsMono }
            let newRows = sorted.compactMap { TranscriptRow(event: $0) }
            rows.append(contentsOf: newRows)

            if let maxID = sorted.last?.id { maxSeenID = maxID }
            if let maxMono = sorted.last?.tsMono { lastTsMono = maxMono }
        } catch {
            // Log errors without crashing; transcript is best-effort UI.
            Loggers.app.error("TranscriptViewModel refresh error: \(error.localizedDescription)")
        }
    }

    /// Starts a 1-second polling loop. Call from `.onAppear`.
    public func startPolling() {
        guard pollTask == nil else { return }
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refresh()
                try? await Task.sleep(nanoseconds: 1_000_000_000)
            }
        }
    }

    /// Stops the polling loop. Call from `.onDisappear`.
    public func stopPolling() {
        pollTask?.cancel()
        pollTask = nil
    }
}

// MARK: - LogEvent → TranscriptRow

private extension TranscriptRow {
    init?(event: LogEvent) {
        switch event.kind {
        case EventKind.turnUser:
            let text = event.payload["text"]?.stringValue ?? ""
            self = .user(id: event.id, text: text)

        case EventKind.turnAssistant:
            let text = event.payload["text"]?.stringValue ?? ""
            self = .assistant(id: event.id, text: text)

        case EventKind.turnToolCall:
            // Expected payload: {"name": String, "args": JSONValue}
            // Defensive: fall back to "<unknown>" if shape doesn't match.
            let name = event.payload["name"]?.stringValue ?? "<unknown>"
            let args: String
            if let argsVal = event.payload["args"] {
                args = argsVal.compactDescription
            } else {
                args = ""
            }
            self = .toolCall(id: event.id, name: name, args: args)

        case EventKind.turnToolResult:
            // Expected payload: {"status": "ok"|"failed", "detail": String}
            let status = event.payload["status"]?.stringValue ?? ""
            let detail = event.payload["detail"]?.stringValue ?? ""
            let success = (status == "ok")
            self = .toolResult(id: event.id, success: success, detail: detail)

        default:
            return nil
        }
    }
}

// MARK: - JSONValue compact description

private extension JSONValue {
    /// A short, human-readable description for pill display. Avoids importing Foundation encoders.
    var compactDescription: String {
        switch self {
        case .null:           return "null"
        case .bool(let b):    return b ? "true" : "false"
        case .number(let n):  return n.truncatingRemainder(dividingBy: 1) == 0 ? String(Int(n)) : String(n)
        case .string(let s):  return s
        case .array(let a):   return "[\(a.map(\.compactDescription).joined(separator: ", "))]"
        case .object(let o):
            let pairs = o.map { "\($0.key): \($0.value.compactDescription)" }.sorted().joined(separator: ", ")
            return "{\(pairs)}"
        }
    }
}

// MARK: - TranscriptView

public struct TranscriptView: View {
    @StateObject private var vm: TranscriptViewModel

    public init(log: Log) {
        _vm = StateObject(wrappedValue: TranscriptViewModel(log: log))
    }

    public var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 6) {
                    ForEach(vm.rows) { row in
                        rowView(for: row)
                            .id(row.id)
                    }
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
            .onChange(of: vm.rows.count) {
                if let last = vm.rows.last {
                    withAnimation(.easeOut(duration: 0.2)) {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }
        }
        .onAppear {
            Task { await vm.refresh() }
            vm.startPolling()
        }
        .onDisappear { vm.stopPolling() }
    }

    // MARK: - Row renderers

    @ViewBuilder
    private func rowView(for row: TranscriptRow) -> some View {
        switch row {
        case .user(_, let text):
            HStack {
                Spacer(minLength: 40)
                Text(text)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 7)
                    .background(Color.accentColor.opacity(0.18))
                    .foregroundStyle(.primary)
                    .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
                    .textSelection(.enabled)
            }

        case .assistant(_, let text):
            HStack {
                Text(text)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 7)
                    .background(Color(nsColor: .controlBackgroundColor))
                    .foregroundStyle(.primary)
                    .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
                    .textSelection(.enabled)
                Spacer(minLength: 40)
            }

        case .toolCall(_, let name, let args):
            HStack(spacing: 4) {
                Text("🔧")
                Text("\(name)(\(args))")
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 4)
            .background(Color(nsColor: .quaternaryLabelColor).opacity(0.25))
            .clipShape(Capsule())

        case .toolResult(_, let success, let detail):
            HStack(spacing: 4) {
                Text(success ? "✓" : "✗")
                    .foregroundStyle(success ? Color.green : Color.red)
                Text(detail)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 4)
            .background(Color(nsColor: .quaternaryLabelColor).opacity(0.25))
            .clipShape(Capsule())
        }
    }
}
