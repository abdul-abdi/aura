import Foundation
import Network

public actor NetworkPathWatchdog {
    public enum Status: Sendable { case satisfied, unsatisfied, requiresConnection }
    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "dev.abdullahi.AuraV3.netpath")
    private var onChange: (@Sendable (Status) -> Void)?
    /// Amendment M: dedup key is "status + sorted interface names".
    /// WiFi→Ethernet handoff is satisfied→satisfied; an interface-set change
    /// is the signal we actually care about.
    private var lastKey: String?

    public init() {}

    public func start(_ onChange: @escaping @Sendable (Status) -> Void) {
        self.onChange = onChange
        monitor.pathUpdateHandler = { [weak self] path in
            let s: Status
            switch path.status {
            case .satisfied: s = .satisfied
            case .unsatisfied: s = .unsatisfied
            case .requiresConnection: s = .requiresConnection
            @unknown default: s = .unsatisfied
            }
            let interfaces = path.availableInterfaces.map(\.name).sorted().joined(separator: ",")
            let key = "\(s):\(interfaces)"
            Task { await self?.notify(status: s, key: key) }
        }
        monitor.start(queue: queue)
    }

    private func notify(status: Status, key: String) {
        if lastKey != key {
            lastKey = key
            onChange?(status)
        }
    }

    public func stop() { monitor.cancel() }
}
