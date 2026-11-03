import AVFoundation
import Foundation
import Speech
import ApplicationServices
import AppKit

@MainActor private var fixturePhases = [String]()
private func fixtureStatus(_ pointer: UnsafePointer<CChar>) {
    MainActor.assumeIsolated {
        if let json = try? JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as? [String: Any],
           let phase = json["phase"] as? String { fixturePhases.append(phase) }
    }
}
@MainActor private func awaitFixturePhase(_ phase: String) async throws {
    let deadline = ContinuousClock.now + .seconds(5)
    while fixturePhases.last != phase {
        guard ContinuousClock.now < deadline else { throw DictationError("Native controller fixture did not reach \(phase): \(fixturePhases)") }
        try await Task.sleep(for: .milliseconds(20))
    }
}

private func fixtureValue(_ pid: pid_t, _ attribute: String) -> String? {
    let app = AXUIElementCreateApplication(pid)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(app, kAXFocusedUIElementAttribute as CFString, &value) == .success,
          let value, CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
    let element = value as! AXUIElement
    var result: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &result) == .success else { return nil }
    return result as? String
}

private func fixtureElement(_ pid: pid_t) -> AXUIElement? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(AXUIElementCreateApplication(pid), kAXFocusedUIElementAttribute as CFString, &value) == .success,
          let value, CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
    return (value as! AXUIElement)
}

@MainActor
private final class MemoryDestination: DictationDestination {
    var onInvalidated: (() -> Void)?
    var valid = true
    var text = ""
    var updates = 0
    var firstUpdate: ContinuousClock.Instant?
    var cancelAfterFirst = false
    func invalidate() { valid = false }
    func check() throws { if !valid { throw CancellationError() } }
    func offer(_ value: String) throws {
        try check()
        if value.isEmpty { return }
        text = value; updates += 1
        if firstUpdate == nil { firstUpdate = .now }
        if cancelAfterFirst { valid = false; onInvalidated?() }
    }
    func finish() async throws { try check() }
}

/// Explicit fixture input uses the production converter and speech turn. It
/// never opens a microphone, text field, clipboard, database or preferences.
private final class FileSpeechInput: SpeechInput {
    let stream: AsyncThrowingStream<AnalyzerInput, Error>
    let level: Float = 0
    var onReady: (@Sendable () -> Void)?
    var onFailure: (@Sendable (Error) -> Void)?
    var onEnd: (@MainActor () -> Void)?
    private let continuation: AsyncThrowingStream<AnalyzerInput, Error>.Continuation
    private let file: AVAudioFile
    private let converter: SpeechBufferConverter
    private var producer: Task<Void, Never>?
    private let offset: Double?
    private(set) var reachedEnd = false
    init(url: URL, format: AVAudioFormat, offset: Double? = nil) throws {
        self.offset = offset
        (stream, continuation) = AsyncThrowingStream.makeStream(bufferingPolicy: .bufferingOldest(32))
        file = try AVAudioFile(forReading: url, commonFormat: .pcmFormatFloat32, interleaved: false)
        guard file.length > 0 else { throw DictationError("The generated speech fixture contains no audio.") }
        converter = try SpeechBufferConverter(from: file.processingFormat, to: format)
    }
    @MainActor func start() throws {
        producer = Task {
            do {
                let frames = AVAudioFrameCount(min(8192, file.processingFormat.sampleRate / 10))
                var first = true
                var delivered: Int64 = 0
                while !Task.isCancelled, file.framePosition < file.length {
                    let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: frames)!
                    try file.read(into: buffer)
                    let output = try converter.convert(buffer)
                    let start = offset.map { CMTime(seconds: $0, preferredTimescale: 48_000) + CMTime(value: delivered, timescale: CMTimeScale(output.format.sampleRate)) }
                    delivered += Int64(output.frameLength)
                    if case .dropped = continuation.yield(AnalyzerInput(buffer: output, bufferStartTime: start)) {
                        throw DictationError("Fixture input overflowed.")
                    }
                    if first { first = false; onReady?() }
                    try await Task.sleep(for: .milliseconds(100))
                }
                guard !Task.isCancelled else { return }
                reachedEnd = true
                continuation.finish()
                onEnd?()
            } catch {
                continuation.finish(throwing: error)
                if !(error is CancellationError) { onFailure?(error) }
            }
        }
    }
    @MainActor func stop() { producer?.cancel(); continuation.finish() }
}

@main
struct DictationTests {
    @MainActor private static func keyEvent(_ owner: DictationKey, _ type: CGEventType,
                                            _ code: CGKeyCode, _ flags: CGEventFlags, repeatKey: Bool = false) -> Bool {
        let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: type == .keyDown)!
        event.flags = flags
        event.setIntegerValueField(.eventSourceUserData, value: 0)
        event.setIntegerValueField(.keyboardEventAutorepeat, value: repeatKey ? 1 : 0)
        return owner.receive(type, event)
    }

    @MainActor private static func shortcutReleaseOrders() {
        let chordFlags: CGEventFlags = [.maskControl, .maskShift]
        // Every ordering must produce exactly one tap, at Space's release,
        // and swallow both Space edges without swallowing modifier events.
        let releaseOrders: [[CGKeyCode]] = [[49, 59, 56], [49, 56, 59], [59, 49, 56],
                                           [56, 49, 59], [59, 56, 49], [56, 59, 49]]
        for order in releaseOrders {
            let owner = DictationKey(key: 49, flags: chordFlags, isolated: false)
            var actions = [Int32]()
            owner.action = { actions.append($0) }
            precondition(keyEvent(owner, .keyDown, 49, chordFlags))
            var flags = chordFlags
            var released = false
            for code in order {
                if code == 59 { flags.remove(.maskControl) }
                if code == 56 { flags.remove(.maskShift) }
                precondition(keyEvent(owner, code == 49 ? .keyUp : .flagsChanged, code, flags) == (code == 49))
                released = released || code == 49
                precondition(actions == (released ? [0] : []))
            }
            precondition(!keyEvent(owner, .keyUp, 49, []))
            owner.stop()
        }
        // Real chords/typing still cancel pending taps, including after one
        // of the original modifiers has been released.
        let interruptions: [(CGEventType, CGKeyCode, CGEventFlags)] = [
            (.flagsChanged, 55, [.maskShift, .maskCommand]), (.keyDown, 0, [.maskShift])
        ]
        for interruption in interruptions {
            let owner = DictationKey(key: 49, flags: chordFlags, isolated: false)
            var actions = [Int32]()
            owner.action = { actions.append($0) }
            precondition(keyEvent(owner, .keyDown, 49, chordFlags))
            precondition(!keyEvent(owner, .flagsChanged, 59, [.maskShift]))
            precondition(!keyEvent(owner, interruption.0, interruption.1, interruption.2))
            precondition(!keyEvent(owner, .keyUp, 49, []))
            precondition(actions.isEmpty)
            owner.stop()
        }
        let option = DictationKey(key: 61, flags: .maskAlternate, isolated: true)
        var actions = [Int32]()
        option.action = { actions.append($0) }
        precondition(keyEvent(option, .flagsChanged, 61, CGEventFlags(rawValue: CGEventFlags.maskAlternate.rawValue | 0x40)))
        precondition(!keyEvent(option, .flagsChanged, 56, [.maskAlternate, .maskShift]))
        precondition(!keyEvent(option, .flagsChanged, 61, []))
        precondition(actions.isEmpty)
        option.stop()
    }

    @MainActor private static func shortcutAutorepeat() {
        let flags: CGEventFlags = [.maskControl, .maskShift]
        let owner = DictationKey(key: 49, flags: flags, isolated: false)
        var actions = [Int32]()
        owner.action = { actions.append($0) }
        // A repeat with no admitted physical press must not start dictation.
        precondition(!keyEvent(owner, .keyDown, 49, flags, repeatKey: true))
        precondition(!keyEvent(owner, .keyUp, 49, flags))
        precondition(actions.isEmpty)
        precondition(keyEvent(owner, .keyDown, 49, flags))
        precondition(keyEvent(owner, .keyDown, 49, flags, repeatKey: true))
        precondition(!keyEvent(owner, .flagsChanged, 55, flags.union(.maskCommand)))
        precondition(!keyEvent(owner, .flagsChanged, 55, flags))
        precondition(!keyEvent(owner, .keyDown, 49, flags, repeatKey: true))
        precondition(!keyEvent(owner, .keyUp, 49, flags))
        precondition(actions.isEmpty)
        // A fresh press recovers. Its repeats stay swallowed even after the
        // original modifiers release, without producing duplicate actions.
        precondition(keyEvent(owner, .keyDown, 49, flags))
        precondition(!keyEvent(owner, .flagsChanged, 59, [.maskShift]))
        precondition(keyEvent(owner, .keyDown, 49, [.maskShift], repeatKey: true))
        precondition(actions.isEmpty)
        precondition(keyEvent(owner, .keyUp, 49, [.maskShift]))
        precondition(actions == [0])
        owner.stop()
    }

    @MainActor static func main() async throws {
        let format = AVAudioFormat(standardFormatWithSampleRate: 16_000, channels: 1)!
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 1600)!
        buffer.frameLength = 1600
        var timeline = SpeechTimeline()
        let origin = CMTime(seconds: 10, preferredTimescale: 48_000)
        let first = try timeline.start(host: origin + CMTime(seconds: 0.3, preferredTimescale: 48_000), origin: origin, buffer: buffer)
        precondition(abs(first.seconds - 0.3) < 0.00001)
        let rounded = try timeline.start(host: origin + CMTime(seconds: 0.399, preferredTimescale: 48_000), origin: origin, buffer: buffer)
        precondition(abs(rounded.seconds - 0.4) < 0.00001)
        let gap = try timeline.start(host: origin + CMTime(seconds: 0.8, preferredTimescale: 48_000), origin: origin, buffer: buffer)
        precondition(abs(gap.seconds - 0.8) < 0.00001)
        do { _ = try timeline.start(host: origin, origin: origin, buffer: buffer); preconditionFailure("Backwards audio must fail.") } catch {}
        print("Meeting clock gaps, resampler rounding and backwards-clock refusal passed")
        shortcutReleaseOrders()
        shortcutAutorepeat()
        let chord = DictationModifierPolicy(key: 49, modifiers: 4608)
        precondition(chord.permits([59, 56], triggerHeld: true))
        precondition(chord.permits([62, 60], triggerHeld: true))
        precondition(!chord.permits([59, 56, 55], triggerHeld: true))
        precondition(!chord.permits([59, 56], triggerHeld: false))
        precondition(chord.permits([], triggerHeld: false))
        let option = DictationModifierPolicy(key: 61, modifiers: 0)
        precondition(option.permits([61], triggerHeld: true))
        precondition(!option.permits([58, 61], triggerHeld: true))
        var gesture = DictationGesture()
        precondition(gesture.press())
        precondition(!gesture.press())
        precondition(gesture.release() == 0)
        precondition(gesture.release() == nil)
        precondition(gesture.press())
        precondition(gesture.hold())
        precondition(!gesture.hold())
        precondition(gesture.release() == 2)
        precondition(gesture.press())
        gesture.cancel()
        precondition(!gesture.hold())
        precondition(gesture.release() == nil)
        let words = try DictationWords([
            DictationWord(heard: "Vorkspace", preferred: "Vorkspace"),
            DictationWord(heard: "work space", preferred: "Vorkspace"),
            DictationWord(heard: "workspace", preferred: "Vorkspace"),
            DictationWord(heard: "clawed", preferred: "Claude"),
            DictationWord(heard: "café", preferred: "Café"),
            DictationWord(heard: "C++", preferred: "C++")])
        precondition(words.apply("A WORK  SPACE, not work spaces. clawed!") == "A Vorkspace, not work spaces. Claude!")
        precondition(words.apply("vorkspace workspace Cafe\u{301} C++ and C++17") == "Vorkspace Vorkspace Café C++ and C++17")
        let longest = try DictationWords([
            DictationWord(heard: "work", preferred: "job"),
            DictationWord(heard: "work space", preferred: "Vorkspace"),
            DictationWord(heard: "job", preferred: "job")])
        precondition(longest.apply("work space and work") == "Vorkspace and job")
        let noCascade = try DictationWords([
            DictationWord(heard: "one", preferred: "two"),
            DictationWord(heard: "two", preferred: "two")])
        precondition(noCascade.apply("one two") == "two two")
        do {
            _ = try DictationWords([DictationWord(heard: "term", preferred: "One"), DictationWord(heard: "TERM", preferred: "Other")])
            preconditionFailure("Ambiguous words must be refused.")
        } catch {}

        var speech = SpeechText()
        var field = "Existing: "
        var owned = ""
        for (text, final) in [("The work", false), ("The work space", false),
                              ("The work space is ready.", true), (" Call clawed", false),
                              (" Call clawed?", true)] {
            let next = words.apply(speech.receive(text, final: final))
            let edit = DictationEdit(from: owned, to: next)
            for _ in 0..<edit.erase { field.removeLast() }
            field += edit.append; owned = next
            precondition(field == "Existing: " + next)
        }
        precondition(field == "Existing: The Vorkspace is ready. Call Claude?")
        // Compare edits with direct replacement, including whole grapheme clusters.
        let variants = ["", "a", "ab", "a😀", "a👩🏽‍💻", "café", "cafe\u{301}", "new\nline", "longer text, with punctuation."]
        for previous in variants { for next in variants {
            let edit = DictationEdit(from: previous, to: next)
            var actual = "Untouched: " + previous
            for _ in 0..<edit.erase { actual.removeLast() }
            actual += edit.append
            precondition(actual == "Untouched: " + next)
        } }
        print("Native dictation words, phrase replacement and Unicode edit regressions passed")

        if CommandLine.arguments.count == 3, CommandLine.arguments[1] == "--recovery-fixture" {
            guard let pid = pid_t(CommandLine.arguments[2]),
                  NSWorkspace.shared.frontmostApplication?.processIdentifier == pid,
                  fixtureValue(pid, kAXIdentifierAttribute) == "vorkspace-dictation-fixture",
                  fixtureValue(pid, kAXValueAttribute) == "Existing: " else {
                throw DictationError("The untouched disposable field must be frontmost.")
            }
            // The missing UID fails before a microphone is opened. Exercise the
            // production controller twice without changing its configuration.
            let configuration = #"{"enabled":true,"microphone":"uid:missing-fixture-device","keyCode":49,"modifiers":4608,"isolatedKey":false,"rules":[]}"#
            configuration.withCString { dictationConfigure($0, fixtureStatus, { _, _ in }) }
            try await awaitFixturePhase("ready")
            for attempt in 1...2 {
                dictationToggle()
                try await awaitFixturePhase("error")
                precondition(fixturePhases.filter { $0 == "starting" }.count == attempt)
                precondition(fixtureValue(pid, kAXValueAttribute) == "Existing: ")
            }
            print("Native controller hotkey recovery after a failed turn passed")
        }

        if CommandLine.arguments.count == 4, CommandLine.arguments[1] == "--ownership-fixture" {
            guard let pid = pid_t(CommandLine.arguments[2]),
                  NSWorkspace.shared.frontmostApplication?.processIdentifier == pid,
                  fixtureValue(pid, kAXIdentifierAttribute) == "vorkspace-dictation-fixture",
                  fixtureValue(pid, kAXValueAttribute) == "Existing: ",
                  let element = fixtureElement(pid) else {
                throw DictationError("The untouched disposable field must be frontmost.")
            }
            let target = try DictationTarget(keyCode: 49, modifiers: 4608)
            precondition(target.usesTextRange)
            try target.offer("Draft")
            precondition(fixtureValue(pid, kAXValueAttribute) == "Existing: Draft")
            let mode = CommandLine.arguments[3]
            var range = mode == "caret" ? CFRange(location: 0, length: 0) : CFRange(location: 10, length: 5)
            let parameter = AXValueCreate(.cfRange, &range)!
            precondition(AXUIElementSetAttributeValue(element, kAXSelectedTextRangeAttribute as CFString, parameter) == .success)
            if mode == "content" {
                precondition(AXUIElementSetAttributeValue(element, kAXSelectedTextAttribute as CFString, "USER!" as CFString) == .success)
            }
            do { try target.offer("A revised hypothesis"); preconditionFailure("A changed field must refuse publication.") }
            catch is CancellationError {}
            let expected = mode == "caret" ? "Existing: Draft" : "Existing: USER!"
            precondition(fixtureValue(pid, kAXValueAttribute) == expected)
            precondition(!target.valid)
            print("Native field ownership refusal passed: \(mode)")
        }

        if CommandLine.arguments.count == 3, CommandLine.arguments[1] == "--meeting-speech-fixture" {
            let url = URL(fileURLWithPath: CommandLine.arguments[2])
            guard let locale = await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current),
                  await SpeechTranscriber.installedLocales.contains(locale),
                  try await AssetInventory.reserve(locale: locale) else { throw DictationError("Meeting fixture requires an installed speech language.") }
            var finalized = [String: String]()
            var ranges = [String: Double]()
            var turns = [SpeechTurn]()
            for (source, offset) in [("microphone", 0.0), ("system", 0.5)] {
                weak var owner: SpeechTurn?
                let turn = SpeechTurn(target: MemoryDestination(), locale: locale, words: words, durationLimit: nil, reservesLocale: false) { format in
                    let input = try FileSpeechInput(url: url, format: format, offset: offset)
                    input.onEnd = { owner?.finish() }
                    return input
                }
                owner = turn
                turn.onResult = { text, final, range in
                    if final { finalized[source, default: ""] += text; ranges[source] = min(ranges[source] ?? .infinity, range.start.seconds) }
                }
                turns.append(turn)
            }
            let microphoneTurn = turns[0], systemTurn = turns[1]
            do {
                async let microphone: Void = microphoneTurn.run()
                async let system: Void = systemTurn.run()
                _ = try await (microphone, system)
            } catch { await AssetInventory.release(reservedLocale: locale); throw error }
            await AssetInventory.release(reservedLocale: locale)
            for source in ["microphone", "system"] {
                precondition(finalized[source]?.contains("Vorkspace") == true, "Both real analyzers must transcribe and apply custom words.")
            }
            precondition((ranges["system"] ?? 0) >= 0.5, "System results must retain their own source clock.")
            print("Two concurrent real speech analyzers, shared language reservation, source clocks and custom words passed")
        }
        if CommandLine.arguments.count == 3, CommandLine.arguments[1] == "--speech-fixture" {
            let url = URL(fileURLWithPath: CommandLine.arguments[2])
            guard url.path.hasPrefix("/"),
                  let locale = await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current) else {
                throw DictationError("Speech fixture requires a supported macOS language.")
            }
            for cancel in [false, true] {
                let destination = MemoryDestination()
                destination.cancelAfterFirst = cancel
                weak var owner: SpeechTurn?
                var input: FileSpeechInput?
                let turn = SpeechTurn(target: destination, locale: locale, words: words) { format in
                    let file = try FileSpeechInput(url: url, format: format)
                    file.onEnd = { owner?.finish() }
                    input = file
                    return file
                }
                owner = turn
                var cancellationObserved = false
                turn.onCancelled = { cancellationObserved = true }
                let started = ContinuousClock.now
                do {
                    try await turn.run()
                    precondition(!cancel)
                } catch is CancellationError { precondition(cancel) }
                await Task.yield()
                precondition(cancellationObserved == cancel, "Retiring timers must not cancel a successful turn.")
                precondition(destination.updates > 0, "The real engine must publish live text.")
                if cancel {
                    precondition(destination.updates == 1 && input?.reachedEnd == false, "Cancellation stops input and all later publication.")
                    print("Real speech cancellation and cleanup passed")
                } else {
                    precondition(destination.text.contains("Vorkspace"), "The controlled fixture must apply its custom word.")
                    let first = started.duration(to: destination.firstUpdate!)
                    print("Real speech fixture: first text \(first), \(destination.updates) updates, final: \(destination.text)")
                }
            }
        }
        if CommandLine.arguments.count == 4, CommandLine.arguments[1] == "--field-fixture" {
            let expectedPID = pid_t(CommandLine.arguments[3]) ?? 0
            let frontmost = NSWorkspace.shared.frontmostApplication?.processIdentifier == expectedPID
            let tagged = frontmost && fixtureValue(expectedPID, kAXIdentifierAttribute) == "vorkspace-dictation-fixture"
            let untouched = tagged && fixtureValue(expectedPID, kAXValueAttribute) == "Existing: "
            print("Field fixture admission: frontmost=\(frontmost) tagged=\(tagged) untouched=\(untouched)")
            guard let pid = pid_t(CommandLine.arguments[3]),
                  NSWorkspace.shared.frontmostApplication?.processIdentifier == pid,
                  fixtureValue(pid, kAXIdentifierAttribute) == "vorkspace-dictation-fixture",
                  fixtureValue(pid, kAXValueAttribute) == "Existing: ",
                  let locale = await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current) else {
                throw DictationError("Only the frontmost, untouched disposable dictation fixture may receive input.")
            }
            let target = try DictationTarget(keyCode: 49, modifiers: 4608)
            print("Native field delivery: \(target.usesTextRange ? "accessibility range" : "Unicode keyboard")")
            weak var owner: SpeechTurn?
            var firstField: ContinuousClock.Instant?
            var ended = false
            let start = ContinuousClock.now
            var firstWasLive = false
            let observer = Task { @MainActor in
                while !Task.isCancelled {
                    if fixtureValue(pid, kAXValueAttribute) != "Existing: ", firstField == nil {
                        firstField = .now; firstWasLive = !ended
                    }
                    try? await Task.sleep(for: .milliseconds(20))
                }
            }
            defer { observer.cancel() }
            let turn = SpeechTurn(target: target, locale: locale, words: words) { format in
                let file = try FileSpeechInput(url: URL(fileURLWithPath: CommandLine.arguments[2]), format: format)
                file.onEnd = { ended = true; owner?.finish() }
                return file
            }
            owner = turn
            try await turn.run()
            let deadline = ContinuousClock.now + .seconds(2)
            while !(fixtureValue(pid, kAXValueAttribute)?.hasSuffix("changes.") ?? false), ContinuousClock.now < deadline {
                try await Task.sleep(for: .milliseconds(20))
            }
            let value = fixtureValue(pid, kAXValueAttribute) ?? ""
            precondition(value.hasPrefix("Existing: ") && value.contains("Vorkspace") && value.hasSuffix("changes."), "Actual field must preserve its prefix and receive the corrected final transcript: \(value)")
            precondition(firstField != nil && firstWasLive && ended)
            print("Real field fixture passed: first visible text \(start.duration(to: firstField!)), final: \(value)")
        }
    }
}
