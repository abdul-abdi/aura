import AppKit

@MainActor
public final class SleepWakeWatchdog {
    private let onSleep: @MainActor () -> Void
    private let onWake: @MainActor () -> Void
    private var sleepObserver: NSObjectProtocol?
    private var wakeObserver: NSObjectProtocol?

    public init(onSleep: @escaping @MainActor () -> Void,
                 onWake: @escaping @MainActor () -> Void) {
        self.onSleep = onSleep
        self.onWake = onWake
    }

    public func start() {
        let nc = NSWorkspace.shared.notificationCenter
        sleepObserver = nc.addObserver(forName: NSWorkspace.willSleepNotification,
                                          object: nil, queue: nil) { [weak self] _ in
            Task { @MainActor in self?.onSleep() }
        }
        wakeObserver = nc.addObserver(forName: NSWorkspace.didWakeNotification,
                                         object: nil, queue: nil) { [weak self] _ in
            Task { @MainActor in self?.onWake() }
        }
    }

    public func stop() {
        let nc = NSWorkspace.shared.notificationCenter
        if let o = sleepObserver { nc.removeObserver(o) }
        if let o = wakeObserver { nc.removeObserver(o) }
        sleepObserver = nil
        wakeObserver = nil
    }
}
