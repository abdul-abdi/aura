import AVFoundation
import ApplicationServices
import ScreenCaptureKit
import OSLog

public actor PermissionWatchdog {
    public enum Service: String, Sendable { case microphone, accessibility, screen }

    private let log: Log    // SQLite event-log actor (NOT the OSLog enum)
    private var lostServices: Set<Service> = []
    private var probeTasks: [Service: Task<Void, Never>] = [:]

    public init(log: Log) { self.log = log }

    public func reportFailure(_ service: Service) async {
        guard !lostServices.contains(service) else { return }
        lostServices.insert(service)
        await log.append(kind: EventKind.statePermissionLost,
                          payload: .object(["service": .string(service.rawValue)]))
        Loggers.observation.warning("permission lost: \(service.rawValue, privacy: .public)")
        startProbing(service: service)
    }

    private func startProbing(service: Service) {
        let task = Task { [weak self] in
            guard let self else { return }
            while await self.isLost(service) {
                try? await Task.sleep(nanoseconds: 60_000 * 1_000_000)  // 60s
                if Task.isCancelled { return }
                if await self.checkRegrant(service) {
                    await self.markRegranted(service)
                    return
                }
            }
        }
        probeTasks[service] = task
    }

    private func isLost(_ s: Service) -> Bool { lostServices.contains(s) }

    private func checkRegrant(_ s: Service) async -> Bool {
        switch s {
        case .accessibility:
            return AXIsProcessTrusted()
        case .microphone:
            return AVCaptureDevice.authorizationStatus(for: .audio) == .authorized
        case .screen:
            return await PermissionWatchdog.probeScreenAccess()
        }
    }

    /// Amendment L: SCShareableContent.current probe replaces the
    /// deprecated CGPreflightScreenCaptureAccess (deprecated macOS 15.1).
    /// Best-effort 250ms deadline — note the underlying call may not honor
    /// cancellation in all cases.
    private static func probeScreenAccess() async -> Bool {
        do {
            return try await withThrowingTaskGroup(of: Bool.self) { group in
                group.addTask {
                    _ = try await SCShareableContent.current
                    return true
                }
                group.addTask {
                    try await Task.sleep(nanoseconds: 250 * 1_000_000)
                    throw NSError(domain: "Aura.Probe", code: 1,
                                   userInfo: [NSLocalizedDescriptionKey: "timeout"])
                }
                let result = try await group.next() ?? false
                group.cancelAll()
                return result
            }
        } catch {
            return false
        }
    }

    private func markRegranted(_ s: Service) async {
        lostServices.remove(s)
        probeTasks[s]?.cancel()
        probeTasks.removeValue(forKey: s)
        await log.append(kind: "state.permission_regranted",
                          payload: .object(["service": .string(s.rawValue)]))
        Loggers.observation.info("permission regranted: \(s.rawValue, privacy: .public)")
    }
}
