import AppKit
import AVFoundation
import Speech

private struct MeetingConfiguration: Decodable { let id: String; let microphone: String; let rules: [DictationWord] }
@MainActor private final class MeetingDestination: DictationDestination {
    var onInvalidated: (() -> Void)?
    var valid = true
    func invalidate() { valid = false }
    func check() throws { if !valid { throw CancellationError() } }
    func offer(_ text: String) throws { try check() }
    func finish() async throws { try check() }
}

/// One parent owns the language reservation and both channel lifecycles. It
/// publishes Stop only after both analyzers and native audio sources have closed.
@MainActor private final class MeetingController {
    static let shared = MeetingController()
    private var active: Task<Void, Never>?
    private var turns = [SpeechTurn]()
    private var id = ""
    private var stopped = false
    private var failure: Error?
    private var ready = Set<String>()
    private var callback: (@convention(c) (UnsafePointer<CChar>) -> Void)?
    func emit(_ values: [String: Any]) {
        var values = values; values["id"] = id
        guard let data = try? JSONSerialization.data(withJSONObject: values), let text = String(data: data, encoding: .utf8) else { return }
        text.withCString { callback?($0) }
    }
    func start(_ config: MeetingConfiguration, _ callback: @escaping @convention(c) (UnsafePointer<CChar>) -> Void) {
        guard active == nil else { return }
        self.callback = callback; id = config.id; stopped = false; failure = nil; ready = []
        emit(["phase": "starting"])
        active = Task { [self] in
            var locale: Locale?
            var reserved = false
            do {
                guard SpeechTranscriber.isAvailable,
                      let supported = await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current) else {
                    throw DictationError("This Mac or language does not support on-device speech recognition.")
                }
                locale = supported
                guard await SpeechTranscriber.installedLocales.contains(supported) else {
                    throw DictationError("Download your speech language in Dictation settings before recording a meeting.")
                }
                guard !self.stopped else { throw CancellationError() }
                if AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined {
                    _ = await AVCaptureDevice.requestAccess(for: .audio)
                }
                guard !self.stopped else { throw CancellationError() }
                guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
                    throw DictationError("Allow Microphone for vorkspace helper in System Settings.")
                }
                guard CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess() else {
                    throw DictationError("Allow Screen & System Audio Recording for vorkspace helper in System Settings, then record again.")
                }
                reserved = try await AssetInventory.reserve(locale: supported)
                guard reserved else { throw DictationError("macOS could not reserve the speech language.") }
                guard !self.stopped else { throw CancellationError() }
                let origin = CMClockGetTime(CMClockGetHostTimeClock())
                let words = try DictationWords(config.rules)
                let microphone = SpeechTurn(target: MeetingDestination(), locale: supported, words: words, durationLimit: nil, reservesLocale: false) {
                    try SpeechCapture(microphone: config.microphone, format: $0, clockOrigin: origin)
                }
                let system = SpeechTurn(target: MeetingDestination(), locale: supported, words: words, durationLimit: nil, reservesLocale: false) {
                    SystemSpeechCapture(format: $0, origin: origin)
                }
                self.turns = [microphone, system]
                for (source, turn) in [("microphone", microphone), ("system", system)] {
                    turn.onReady = { [weak self] in
                        guard let self, !self.stopped else { return }
                        self.ready.insert(source)
                        if self.ready.count == 2 { self.emit(["phase": "recording"]) }
                    }
                    turn.onCancelled = { [weak self] in self?.cancel() }
                    turn.onResult = { [weak self] text, final, range in
                        guard let self, self.failure == nil else { return }
                        let start = range.start.seconds * 1000
                        let end = CMTimeRangeGetEnd(range).seconds * 1000
                        guard text.utf8.count <= 64 * 1024, start.isFinite, end.isFinite,
                              start >= 0, end >= start, end <= Double(Int64.max) else {
                            throw DictationError("Speech recognition returned an invalid meeting segment.")
                        }
                        self.emit(["source": source, "text": text, "final": final,
                                   "start_ms": Int64(start), "end_ms": Int64(end)])
                    }
                }
                await withTaskGroup(of: Error?.self) { group in
                    for turn in self.turns {
                        group.addTask { @MainActor in
                            do { try await turn.run(); return nil } catch { return error }
                        }
                    }
                    for await error in group {
                        if let error, !(error is CancellationError), self.failure == nil { self.failure = error }
                        if error != nil { self.cancel() }
                    }
                }
            } catch {
                if !(error is CancellationError) { self.failure = error }
                self.cancel()
            }
            self.turns = []
            if reserved, let locale { await AssetInventory.release(reservedLocale: locale) }
            if let error = self.failure { self.emit(["phase": "failed", "error": error.localizedDescription]) }
            else { self.emit(["phase": "stopped"]) }
            self.active = nil
        }
    }
    func finish() {
        guard active != nil else { return }
        stopped = true
        emit(["phase": "finishing"])
        if ready.count < 2 { cancel() }
        else { for turn in turns { turn.finish() } }
    }
    func cancel() {
        stopped = true
        for turn in turns where !turn.cancelled { turn.cancel() }
    }
}
@_cdecl("vorkspace_transcription_start")
func transcriptionStart(_ json: UnsafePointer<CChar>, _ callback: @escaping @convention(c) (UnsafePointer<CChar>) -> Void) {
    MainActor.assumeIsolated {
        guard let config = try? JSONDecoder().decode(MeetingConfiguration.self, from: Data(String(cString: json).utf8)) else { return }
        MeetingController.shared.start(config, callback)
    }
}
@_cdecl("vorkspace_transcription_finish")
func transcriptionFinish() { MainActor.assumeIsolated { MeetingController.shared.finish() } }
@_cdecl("vorkspace_transcription_cancel")
func transcriptionCancel() { MainActor.assumeIsolated { MeetingController.shared.cancel() } }
