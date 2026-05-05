import AVFoundation

@MainActor
public final class AudioConfigWatchdog {
    private let onChange: @MainActor () -> Void
    private weak var engine: AVAudioEngine?
    private var observer: NSObjectProtocol?

    public init(engine: AVAudioEngine, onChange: @escaping @MainActor () -> Void) {
        self.engine = engine
        self.onChange = onChange
    }

    public func start() {
        observer = NotificationCenter.default.addObserver(
            forName: .AVAudioEngineConfigurationChange,
            object: engine, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in self?.onChange() }
        }
    }

    public func stop() {
        if let o = observer { NotificationCenter.default.removeObserver(o) }
        observer = nil
    }
}
