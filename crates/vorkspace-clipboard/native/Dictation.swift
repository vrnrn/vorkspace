import AppKit
import AVFoundation
import Speech

private struct DictationConfiguration: Decodable, Equatable {
    var enabled = false
    var microphone = "system"
    var keyCode: UInt16 = 49
    var modifiers: UInt32 = 4608
    var rules = [DictationWord]()
}

private enum DictationPhase: String, Encodable {
    case off, checking, microphone, accessibility, model, downloading, ready
    case starting, listening, finishing, cancelling, error, unavailable
}

private struct DictationStatus: Encodable {
    let phase: DictationPhase
    let message: String
    let active: Bool
}

/// Apple's finalized phrases are immutable. Only its current volatile phrase
/// can replace text, so there is no guessed stability or punctuation heuristic.
struct SpeechText {
    private(set) var finalized = ""
    private(set) var volatile = ""
    mutating func receive(_ text: String, final: Bool) -> String {
        if final { finalized += text; volatile = "" }
        else { volatile = text }
        return finalized + volatile
    }
}

@MainActor
final class SpeechTurn {
    let target: any DictationDestination
    let locale: Locale
    let words: DictationWords
    private(set) var capture: (any SpeechInput)?
    private let makeInput: (AVAudioFormat) throws -> any SpeechInput
    private var analyzer: SpeechAnalyzer?
    private var reader: Task<Void, Error>?
    private var limit: Task<Void, Never>?
    private var finishTimeout: Task<Void, Never>?
    private let end = AsyncStream<Void>.makeStream(bufferingPolicy: .bufferingOldest(1))
    private var failure: Error?
    private(set) var cancelled = false
    private(set) var finishing = false
    var onReady: (() -> Void)?
    var onCancelled: (() -> Void)?
    var onLimit: (() -> Void)?

    init(target: any DictationDestination, locale: Locale, words: DictationWords,
         makeInput: @escaping (AVAudioFormat) throws -> any SpeechInput) {
        self.target = target; self.locale = locale; self.words = words
        self.makeInput = makeInput
        target.onInvalidated = { [weak self] in self?.cancel() }
    }

    func cancel(_ error: Error? = nil) {
        guard !cancelled else { return }
        cancelled = true
        failure = error
        target.invalidate()
        capture?.stop()
        onCancelled?()
        end.continuation.yield(())
        if let analyzer { Task { await analyzer.cancelAndFinishNow() } }
    }

    func finish() {
        guard !cancelled, !finishing else { return }
        finishing = true
        capture?.stop()
        end.continuation.yield(())
        finishTimeout = Task { [weak self] in
            try? await Task.sleep(for: .seconds(8))
            guard !Task.isCancelled, let self, self.finishing else { return }
            self.cancel(DictationError("Speech recognition did not finish. Try again."))
        }
    }

    private func check() throws {
        if cancelled { throw failure ?? CancellationError() }
        try target.check()
        if !finishing { try capture?.check() }
    }

    func run() async throws {
        var reserved = false
        var outcome: Error?
        do {
            let transcriber = SpeechTranscriber(locale: locale, preset: .progressiveTranscription)
            reserved = try await AssetInventory.reserve(locale: locale)
            guard reserved else { throw DictationError("macOS could not prepare this speech language. Try again.") }
            try check()
            guard await AssetInventory.status(forModules: [transcriber]) == .installed else {
                throw DictationError("Download this language in Dictation settings first.")
            }
            try check()
            let analyzer = SpeechAnalyzer(modules: [transcriber], options: .init(priority: .userInitiated, modelRetention: .lingering))
            self.analyzer = analyzer
            guard let format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [transcriber]) else {
                throw DictationError("macOS could not prepare microphone audio for this language.")
            }
            try check()
            try await analyzer.prepareToAnalyze(in: format)
            try check()
            let capture = try makeInput(format)
            self.capture = capture
            capture.onReady = { [weak self] in
                Task { @MainActor in
                    guard let self, !self.cancelled, !self.finishing else { return }
                    do { try self.check(); self.onReady?() }
                    catch { self.cancel(error) }
                }
            }
            capture.onFailure = { [weak self] error in
                Task { @MainActor in self?.cancel(error) }
            }
            reader = Task {
                var transcript = SpeechText()
                do {
                    for try await result in transcriber.results {
                        try self.check()
                        let text = transcript.receive(String(result.text.characters), final: result.isFinal)
                        try self.target.offer(self.words.apply(text))
                    }
                } catch { self.cancel(error); throw error }
            }
            try await analyzer.start(inputSequence: capture.stream)
            try check()
            try capture.start()
            limit = Task { [weak self] in
                try? await Task.sleep(for: .seconds(120))
                guard !Task.isCancelled, let self, !self.cancelled, !self.finishing else { return }
                self.onLimit?()
            }
            for await _ in end.stream { break }
            try check()
            try await analyzer.finalizeAndFinishThroughEndOfInput()
            try check()
            try await reader?.value
            try await target.finish()
        } catch { outcome = cancelled ? (failure ?? CancellationError()) : error }
        // Retain the turn until actual native cleanup and locale release finish.
        capture?.stop()
        limit?.cancel(); finishTimeout?.cancel()
        target.onInvalidated = nil
        target.invalidate()
        if let analyzer { await analyzer.cancelAndFinishNow() }
        reader?.cancel()
        _ = try? await reader?.value
        if reserved { await AssetInventory.release(reservedLocale: locale) }
        finishing = false
        if let outcome { throw outcome }
    }
}

/// This is the sole owner of speech sessions, permissions and installation.
/// Rust owns only the helper's shortcut, microphone lease and notch presentation.
@MainActor
private final class DictationController {
    static let shared = DictationController()
    var callback: (@convention(c) (UnsafePointer<CChar>) -> Void)?
    private var configuration = DictationConfiguration()
    private var phase = DictationPhase.off
    private var message = "Dictation is off."
    private var generation: UInt64 = 0
    private var preparation: Task<Void, Never>?
    private var installation: AssetInstallationRequest?
    private var turn: SpeechTurn?
    private var locale: Locale?
    private var activation: NSObjectProtocol?

    init() {
        activation = NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, self.phase == .microphone || self.phase == .accessibility else { return }
                self.check()
            }
        }
    }

    func configure(_ value: DictationConfiguration) {
        guard configuration != value else { publish(); return }
        generation &+= 1
        preparation?.cancel(); preparation = nil
        installation?.progress.cancel(); installation = nil
        configuration = value
        if !value.enabled { cancel() }
        if turn == nil { check() }
    }

    private func set(_ phase: DictationPhase, _ message: String) {
        self.phase = phase; self.message = message; publish()
    }

    func publish() {
        let status = DictationStatus(phase: phase, message: message, active: turn != nil)
        guard let data = try? JSONEncoder().encode(status), let json = String(data: data, encoding: .utf8) else { return }
        json.withCString { callback?($0) }
    }

    private func current(_ generation: UInt64) -> Bool {
        self.generation == generation && configuration.enabled && turn == nil
    }

    func check() {
        guard turn == nil else { return }
        generation &+= 1
        let generation = generation
        preparation?.cancel()
        locale = nil
        guard configuration.enabled else { preparation = nil; set(.off, "Dictation is off."); return }
        set(.checking, "Checking speech recognition…")
        preparation = Task {
            guard SpeechTranscriber.isAvailable else {
                self.checked(generation, .unavailable, "This Mac does not support Apple's on-device speech engine."); return
            }
            guard let locale = await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current) else {
                self.checked(generation, .unavailable, "Your Mac's language is not supported by on-device speech recognition."); return
            }
            guard self.current(generation) else { return }
            self.locale = locale
            guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
                self.checked(generation, .microphone, "Allow microphone access to start dictating."); return
            }
            guard AXIsProcessTrusted() else {
                self.checked(generation, .accessibility, "Allow Accessibility for vorkspace Clipboard to type into other apps."); return
            }
            let transcriber = SpeechTranscriber(locale: locale, preset: .progressiveTranscription)
            // AssetInventory.status also reflects this app's reservation. An
            // already installed, shared language needs no download consent;
            // the actual turn reserves it before checking module readiness.
            let installed = await SpeechTranscriber.installedLocales.contains(locale)
            guard self.current(generation) else { return }
            if installed { self.checked(generation, .ready, "Ready. Speech stays on this Mac."); return }
            let status = await AssetInventory.status(forModules: [transcriber])
            guard self.current(generation) else { return }
            switch status {
            case .installed: self.checked(generation, .ready, "Ready. Speech stays on this Mac.")
            case .supported, .downloading: self.checked(generation, .model, "Download your language's speech files. macOS manages them.")
            default: self.checked(generation, .unavailable, "On-device speech recognition is unavailable for this language.")
            }
        }
    }

    private func checked(_ generation: UInt64, _ phase: DictationPhase, _ message: String) {
        guard current(generation) else { return }
        preparation = nil
        set(phase, message)
    }

    func setUp() {
        guard configuration.enabled, turn == nil, preparation == nil else { return }
        switch phase {
        case .microphone:
            if AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined {
                AVCaptureDevice.requestAccess(for: .audio) { _ in Task { @MainActor in self.check() } }
            } else { openPrivacy("Privacy_Microphone") }
        case .accessibility:
            let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
            _ = AXIsProcessTrustedWithOptions(options)
            openPrivacy("Privacy_Accessibility")
        case .model:
            guard let locale else { check(); return }
            let generation = generation
            set(.downloading, "Downloading speech files through macOS…")
            preparation = Task {
                do {
                    let transcriber = SpeechTranscriber(locale: locale, preset: .progressiveTranscription)
                    let request = try await AssetInventory.assetInstallationRequest(supporting: [transcriber])
                    guard self.current(generation) else { return }
                    self.installation = request
                    try await request?.downloadAndInstall()
                    guard self.current(generation) else { return }
                    self.installation = nil; self.preparation = nil; self.check()
                } catch {
                    guard self.current(generation) else { return }
                    self.installation = nil; self.preparation = nil
                    self.set(.error, "Speech files could not be installed. Check your connection and try again.")
                }
            }
        default: check()
        }
    }

    private func openPrivacy(_ pane: String) {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?\(pane)") { NSWorkspace.shared.open(url) }
    }

    func toggle() {
        if let turn {
            if phase == .listening { finish() } else { turn.cancel(); set(.cancelling, "Cancelling…") }
            return
        }
        guard configuration.enabled else { return }
        // A turn-specific failure is presentation, not a permanent admission
        // lock. A new deliberate shortcut gets a fresh target and speech turn;
        // missing permissions/assets still fail their ordinary preflight.
        guard phase == .ready || phase == .error, let locale else { publish(); return }
        do {
            let target = try DictationTarget(keyCode: configuration.keyCode, modifiers: configuration.modifiers)
            let words = try DictationWords(configuration.rules)
            let microphone = configuration.microphone
            let turn = SpeechTurn(target: target, locale: locale, words: words,
                makeInput: { try SpeechCapture(microphone: microphone, format: $0) })
            self.turn = turn
            turn.onReady = { [weak self, weak turn] in
                guard let self, let turn, self.turn === turn else { return }
                self.set(.listening, "Listening")
            }
            turn.onLimit = { [weak self, weak turn] in
                guard let self, let turn, self.turn === turn else { return }
                self.finish()
            }
            turn.onCancelled = { [weak self, weak turn] in
                guard let self, let turn, self.turn === turn else { return }
                self.set(.cancelling, "Cancelling…")
            }
            set(.starting, "Starting microphone…")
            Task {
                var failure: Error?
                do { try await turn.run() } catch { failure = error }
                guard self.turn === turn else { return }
                self.turn = nil
                if let error = failure, !(error is CancellationError), self.configuration.enabled {
                    self.set(.error, error.localizedDescription)
                } else { self.check() }
            }
        } catch { set(.error, error.localizedDescription) }
    }

    func finish() {
        guard let turn else { return }
        turn.finish()
        set(.finishing, "Finishing…")
    }

    func cancel() {
        guard let turn else { return }
        turn.cancel()
        set(.cancelling, "Cancelling…")
    }

    var level: Float { turn?.capture?.level ?? 0 }
}

@_cdecl("vorkspace_dictation_configure")
func dictationConfigure(_ json: UnsafePointer<CChar>, _ callback: @escaping @convention(c) (UnsafePointer<CChar>) -> Void) {
    MainActor.assumeIsolated {
        let controller = DictationController.shared
        controller.callback = callback
        guard let configuration = try? JSONDecoder().decode(DictationConfiguration.self, from: Data(String(cString: json).utf8)) else { return }
        controller.configure(configuration)
    }
}
@_cdecl("vorkspace_dictation_toggle")
func dictationToggle() { MainActor.assumeIsolated { DictationController.shared.toggle() } }
@_cdecl("vorkspace_dictation_setup")
func dictationSetup() { MainActor.assumeIsolated { DictationController.shared.setUp() } }
@_cdecl("vorkspace_dictation_cancel")
func dictationCancel() { MainActor.assumeIsolated { DictationController.shared.cancel() } }
@_cdecl("vorkspace_dictation_level")
func dictationLevel() -> Float { MainActor.assumeIsolated { DictationController.shared.level } }
