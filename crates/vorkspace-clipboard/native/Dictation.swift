import AppKit
import AVFoundation
import ApplicationServices
import AudioToolbox
import CoreAudio
import Carbon

// One attempt, never reset. Protected by the capture's PCM lock. Expected
// setup has no observer; changes after beginArming are sticky, including changes
// racing the baseline read or teardown. No delay or failed=false reset is used.
struct CaptureLifecycle {
    private enum Phase { case setup, arming, active }
    private var phase = Phase.setup
    private(set) var failed = false
    var active: Bool { phase == .active && !failed }
    mutating func configurationChanged() {
        if phase != .setup { failed = true }
    }
    mutating func fail() { failed = true }
    mutating func beginArming() -> Bool {
        guard phase == .setup, !failed else { return false }
        phase = .arming
        return true
    }
    mutating func arm(healthy: Bool) -> Bool {
        guard phase == .arming else { return false }
        if !healthy { failed = true }
        guard !failed else { return false }
        phase = .active
        return true
    }
}
// Snapshot policy shared by admission and every health read. A matching format
// alone cannot certify a device: a default aggregate may keep the same format.
struct CaptureRouteEvidence {
    var expectedDevice: AudioDeviceID
    var actualDevice: AudioDeviceID
    var expectedUID: String
    var actualUID: String?
    var authorized: Bool
    var alive: Bool
    var deviceStatus: OSStatus
    var formatMatches: Bool
    var running: Bool
    var failure: String? {
        if !authorized { return "permission" }
        if !alive { return "device-not-alive" }
        if actualUID != expectedUID { return "device-uid" }
        if deviceStatus != noErr { return "current-device-osstatus:\(deviceStatus)" }
        if actualDevice != expectedDevice { return "current-device-mismatch" }
        if !formatMatches { return "format-mismatch" }
        if !running { return "engine-not-running" }
        return nil
    }
}
// Main-thread owner. Only the tap and notification callback touch locked state.
// No idle capture, and no engine operations while holding the PCM lock.
private final class DictationCapture {
    let engine = AVAudioEngine()
    let lock = NSLock()
    var samples = [Float]()
    var retainSamples = true
    #if DEBUG
    var diagnostic = CaptureDiagnostics()
    #endif
    var lifecycle = CaptureLifecycle()
    var full = false
    var device: AudioDeviceID = 0
    var deviceUID = ""
    var format: AVAudioFormat?
    var tapped = false
    var observer: NSObjectProtocol?
    func arm() -> Bool {
        lock.lock(); let arming = lifecycle.beginArming(); lock.unlock()
        guard arming else { return false }
        // Route/format/tap setup and engine.start have completed. Register before
        // validating the fresh baseline, so an interruption during validation
        // cannot be erased by a successful readback. queue:nil delivers on the
        // posting thread; no main-queue setup notification is carried into capture.
        observer = NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: nil) { [weak self] _ in
            guard let self else { return }
            self.lock.lock()
            #if DEBUG
            if self.diagnostic.notifications.count < 16 {
                self.diagnostic.notifications.append(self.diagnostic.teardownStarted ? "controlled-teardown" : (self.lifecycle.active ? "active" : "arming-or-failed"))
            }
            #endif
            self.lifecycle.configurationChanged(); self.lock.unlock()
        }
        let healthy = routeIsHealthy()
        lock.lock(); defer { lock.unlock() }
        return lifecycle.arm(healthy: healthy)
    }
    func routeIsHealthy() -> Bool { routeFailure() == nil }
    func routeFailure() -> String? {
        guard let format else { return "missing-format" }
        let node = engine.inputNode
        guard let unit = node.audioUnit else { return "missing-audio-unit" }
        var actual: AudioDeviceID = 0
        var size = UInt32(MemoryLayout<AudioDeviceID>.size)
        let status = AudioUnitGetProperty(unit, kAudioOutputUnitProperty_CurrentDevice,
            kAudioUnitScope_Global, 0, &actual, &size)
        return CaptureRouteEvidence(
            expectedDevice: device, actualDevice: actual,
            expectedUID: deviceUID, actualUID: audioString(device, kAudioDevicePropertyDeviceUID),
            authorized: AVCaptureDevice.authorizationStatus(for: .audio) == .authorized,
            alive: audioScalar(device, kAudioDevicePropertyDeviceIsAlive) == 1,
            deviceStatus: status, formatMatches: node.outputFormat(forBus: 0).isEqual(format),
            running: engine.isRunning).failure
    }
    deinit { if let observer { NotificationCenter.default.removeObserver(observer) } }
    func stop() {
        #if DEBUG
        lock.lock(); diagnostic.teardownStarted = true; lock.unlock()
        #endif
        engine.stop()
        if tapped { engine.inputNode.removeTap(onBus: 0); tapped = false }
        if let observer { NotificationCenter.default.removeObserver(observer); self.observer = nil }
    }
}
// A late timer from an earlier paste must never discard its successor's snapshot.
final class ClipboardRestoration {
    private var pending: (revision: Int, apply: () -> Void)?
    func flush() { let old = pending; pending = nil; old?.apply() }
    func install(revision: Int, apply: @escaping () -> Void) { pending = (revision, apply) }
    func finish(revision: Int) { guard pending?.revision == revision else { return }; flush() }
}
private let restoration = ClipboardRestoration()
private var capture: DictationCapture?
private var targetOwner: DictationTarget?
// Also used by the explicit focus fixture: admits insertion without audio/model work.
func dictationPrepareInsertion() -> Bool {
    targetOwner = DictationTarget.admit()
    return targetOwner != nil
}

@_cdecl("vorkspace_dictation_permissions")
func dictationPermissions() {
    AVCaptureDevice.requestAccess(for: .audio) { _ in }
    let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
    _ = AXIsProcessTrustedWithOptions(options)
}

// 1 mic permission, 2 Accessibility, 3 engine failure, 4 admission unavailable,
// 8 selected input unavailable. Paste uses 9 for a changed destination.
@_cdecl("vorkspace_dictation_start")
func dictationStart(_ microphone: UnsafePointer<CChar>) -> Int32 {
    guard capture == nil else { return 3 }
    guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else { return 1 }
    guard AXIsProcessTrusted() else { return 2 }
    guard dictationPrepareInsertion() else { return 4 }
    return startCapture(String(cString: microphone))
}
// Microphone ownership is independent of insertion admission. Only normal
// dictation prepares a destination; the debug capture probe never pastes.
private func startCapture(_ microphone: String, retainSamples: Bool = true) -> Int32 {
    guard capture == nil else { return 3 }
    guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else { return 1 }
    let state = DictationCapture()
    state.retainSamples = retainSamples
    #if DEBUG
    if !retainSamples { diagnosticCapture = state }
    #endif
    var started = false
    defer { if !started { targetOwner = nil; state.stop() } }
    let devices = inputDevices()
    guard let device = resolveMicrophone(microphone, devices: devices, system: defaultInputDevice()),
          let identity = devices.first(where: { $0.id == device }) else { return 8 }
    state.device = device
    state.deviceUID = identity.uid
    let node = state.engine.inputNode
    // Select this engine's input unit, never the system-wide default. Re-read
    // format only after selection; Bluetooth/USB inputs can differ in rate/channels.
    guard let unit = node.audioUnit else { return 3 }
    // Select through the engine's AU owner. Mutating the legacy AudioUnit
    // underneath it leaves default-device tracking enabled, which can replace
    // this route asynchronously with CADefaultDeviceAggregate at engine start.
    do {
        try node.auAudioUnit.setDeviceID(device)
        #if DEBUG
        state.diagnostic.selectionStatus = noErr
        #endif
    } catch {
        #if DEBUG
        state.diagnostic.selectionStatus = OSStatus((error as NSError).code)
        #endif
        return 8
    }
    var actual: AudioDeviceID = 0
    var size = UInt32(MemoryLayout<AudioDeviceID>.size)
    guard AudioUnitGetProperty(unit, kAudioOutputUnitProperty_CurrentDevice,
        kAudioUnitScope_Global, 0, &actual, &size) == noErr, actual == device, audioString(device, kAudioDevicePropertyDeviceUID) == identity.uid else { return 8 }
    let format = node.outputFormat(forBus: 0)
    guard format.sampleRate > 0, format.channelCount > 0,
          let output = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16000, channels: 1, interleaved: false),
          let converter = AVAudioConverter(from: format, to: output) else { return 3 }
    state.format = format
    if retainSamples { state.samples.reserveCapacity(16000 * 120) }
    node.installTap(onBus: 0, bufferSize: 2048, format: format) { buffer, _ in
        let capacity = AVAudioFrameCount(Double(buffer.frameLength) * 16000 / format.sampleRate + 32)
        guard let result = AVAudioPCMBuffer(pcmFormat: output, frameCapacity: capacity) else { return }
        var supplied = false
        var error: NSError?
        let conversionStatus = converter.convert(to: result, error: &error) { _, status in
            if supplied { status.pointee = .noDataNow; return nil }
            supplied = true; status.pointee = .haveData; return buffer
        }
        state.lock.lock(); defer { state.lock.unlock() }
        _ = conversionStatus
        #if DEBUG
        state.diagnostic.callbacks += 1
        state.diagnostic.inputFrames += Int(buffer.frameLength)
        state.diagnostic.outputFrames += Int(result.frameLength)
        state.diagnostic.conversionStatus = conversionStatus.rawValue
        if let error { state.diagnostic.conversionError = "\(error.domain):\(error.code)" }
        #endif
        if error != nil { state.lifecycle.fail(); return }
        guard state.retainSamples else { return }
        guard let pcm = result.floatChannelData?[0] else { return }
        let count = min(Int(result.frameLength), 16000 * 120 - state.samples.count)
        if count > 0 { state.samples.append(contentsOf: UnsafeBufferPointer(start: pcm, count: count)) }
        state.full = state.samples.count == 16000 * 120
    }
    state.tapped = true
    do { try state.engine.start() } catch {
        #if DEBUG
        let error = error as NSError
        state.diagnostic.startError = "\(error.domain):\(error.code)"
        #endif
        state.stop(); return 3
    }
    guard state.arm() else { return 3 }
    capture = state; started = true
    return 0
}
@_cdecl("vorkspace_dictation_health")
func dictationHealth() -> Int32 {
    guard let state = capture else { return 0 }
    let failure = state.routeFailure()
    state.lock.lock(); defer { state.lock.unlock() }
    if !state.lifecycle.active || failure != nil {
        #if DEBUG
        state.diagnostic.healthFailure = failure ?? (state.diagnostic.conversionError == nil
            ? "configuration-notification" : "conversion-error")
        #endif
        return 1
    }
    return state.full ? 2 : 0
}
@_cdecl("vorkspace_dictation_stop")
func dictationStop(_ callback: @convention(c) (UnsafePointer<Float>?, Int) -> Void) -> Int32 {
    guard let state = capture else { callback(nil, 0); return 11 }
    let health = dictationHealth()
    state.stop(); capture = nil
    state.lock.lock()
    let failed = state.lifecycle.failed
    let pcm = state.samples
    state.samples.removeAll()
    state.lock.unlock()
    // Validate after tap teardown as well: stop can overlap a device notification.
    guard health != 1, !failed,
          audioScalar(state.device, kAudioDevicePropertyDeviceIsAlive) == 1,
          audioString(state.device, kAudioDevicePropertyDeviceUID) == state.deviceUID else {
        targetOwner = nil; callback(nil, 0); return 11
    }
    pcm.withUnsafeBufferPointer { callback($0.baseAddress, $0.count) }
    return 0
}
@_cdecl("vorkspace_dictation_cancel")
func dictationCancel() {
    capture?.stop(); capture = nil; targetOwner = nil
}

// Retain every representation or decline the paste. No partial rich snapshots.
func clipboardSnapshot(_ board: NSPasteboard, limit: Int = 32 * 1024 * 1024) -> [NSPasteboardItem]? {
    let revision = board.changeCount
    var bytes = 0
    var items = [NSPasteboardItem]()
    for item in board.pasteboardItems ?? [] {
        let copy = NSPasteboardItem()
        for type in item.types {
            guard let data = item.data(forType: type) else { return nil }
            bytes += data.count
            guard bytes <= limit else { return nil }
            copy.setData(data, forType: type)
        }
        items.append(copy)
    }
    return revision == board.changeCount ? items : nil
}
func restoreClipboard(_ board: NSPasteboard, revision: Int, items: [NSPasteboardItem]) {
    guard board.changeCount == revision else { return }
    board.clearContents()
    if !items.isEmpty { board.writeObjects(items) }
}
@_cdecl("vorkspace_dictation_paste")
func dictationPaste(_ text: UnsafePointer<CChar>) -> Int32 {
    guard AXIsProcessTrusted() else { return 2 }
    guard let owner = targetOwner, owner.isCurrent() else { return 9 }
    // Caller waits for shortcut modifiers to lift. Never synthesize a modified paste.
    guard !dictationModifiersDown() else { return 5 }
    restoration.flush()
    let board = NSPasteboard.general
    guard let saved = clipboardSnapshot(board) else { return 6 }
    // Pasteboard providers may have taken time; validate the destination again.
    guard owner.isCurrent() else { return 9 }
    guard !dictationModifiersDown() else { return 5 }
    guard let source = CGEventSource(stateID: .combinedSessionState),
          let down = CGEvent(keyboardEventSource: source, virtualKey: 9, keyDown: true),
          let up = CGEvent(keyboardEventSource: source, virtualKey: 9, keyDown: false) else { return 7 }
    board.clearContents()
    guard board.setString(String(cString: text), forType: .string),
          board.setString("1", forType: NSPasteboard.PasteboardType("com.vorkspace.transient-dictation")) else {
        restoreClipboard(board, revision: board.changeCount, items: saved); return 7
    }
    let revision = board.changeCount
    guard owner.isCurrent(), !dictationModifiersDown() else {
        restoreClipboard(board, revision: revision, items: saved); return 9
    }
    down.flags = .maskCommand; up.flags = .maskCommand
    // One uncertain external effect, never retried.
    targetOwner = nil
    down.post(tap: .cghidEventTap); up.post(tap: .cghidEventTap)
    let restore = { restoreClipboard(board, revision: revision, items: saved) }
    restoration.install(revision: revision, apply: restore)
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { restoration.finish(revision: revision) }
    return 0
}
@_cdecl("vorkspace_dictation_modifiers_down")
func dictationModifiersDown() -> Bool {
    !CGEventSource.flagsState(.combinedSessionState).intersection([.maskCommand, .maskControl, .maskAlternate, .maskShift]).isEmpty
}

@_cdecl("vorkspace_dictation_shutdown")
func dictationShutdown() { dictationCancel(); restoration.flush() }

#if DEBUG
// Explicit opt-in metadata only. No PCM values, levels, transcript, clipboard,
// destination or model access. Shares the production owner/converter/health/stop.
private struct CaptureDiagnostics {
    var selectionStatus: OSStatus?
    var callbacks = 0
    var inputFrames = 0
    var outputFrames = 0
    var notifications = [String]()
    var conversionStatus: Int = 0
    var conversionError: String?
    var startError: String?
    var healthFailure: String?
    var teardownStarted = false
}
private var diagnosticCapture: DictationCapture?
private func captureMetadata(_ state: DictationCapture) -> [String: Any] {
    let node = state.engine.inputNode
    var actual: AudioDeviceID = 0
    var size = UInt32(MemoryLayout<AudioDeviceID>.size)
    let status = node.audioUnit.map { AudioUnitGetProperty($0, kAudioOutputUnitProperty_CurrentDevice,
        kAudioUnitScope_Global, 0, &actual, &size) }
    func format(_ f: AVAudioFormat) -> [String: Any] {
        ["rate": f.sampleRate, "channels": f.channelCount,
         "commonFormat": f.commonFormat.rawValue, "interleaved": f.isInterleaved,
         "description": f.description]
    }
    let routeFailure = state.routeFailure()
    let currentFormat = format(node.outputFormat(forBus: 0))
    let hardwareFormat = format(node.inputFormat(forBus: 0))
    let running = state.engine.isRunning
    let attached = node.engine === state.engine
    state.lock.lock()
    let d = state.diagnostic
    let lifecycle = state.lifecycle
    let retainedSamples = state.samples.count
    state.lock.unlock()
    return ["permission": AVCaptureDevice.authorizationStatus(for: .audio).rawValue,
            "configuredDevice": state.device, "configuredUID": state.deviceUID,
            "currentDevice": actual, "currentUID": audioString(actual, kAudioDevicePropertyDeviceUID) ?? "unavailable",
            "getDeviceOSStatus": status as Any? ?? NSNull(),
            "setDeviceOSStatus": d.selectionStatus as Any? ?? NSNull(),
            "deviceAlive": audioScalar(state.device, kAudioDevicePropertyDeviceIsAlive) as Any? ?? NSNull(),
            "configuredFormat": state.format.map(format) as Any? ?? NSNull(),
            "currentFormat": currentFormat, "hardwareFormat": hardwareFormat,
            "engineRunning": running, "engineAttached": attached,
            "controlledTeardown": d.teardownStarted,
            "routeFailure": routeFailure as Any? ?? NSNull(),
            "healthFailure": d.healthFailure as Any? ?? NSNull(),
            "lifecycleFailed": lifecycle.failed, "lifecycleActive": lifecycle.active,
            "notifications": d.notifications, "callbacks": d.callbacks,
            "inputFrames": d.inputFrames, "convertedFrames": d.outputFrames,
            "conversionStatus": d.conversionStatus,
            "conversionError": d.conversionError as Any? ?? NSNull(),
            "startError": d.startError as Any? ?? NSNull(), "retainedSamples": retainedSamples]
}

@_cdecl("vorkspace_dictation_capture_probe")
func dictationCaptureProbe(_ microphone: UnsafePointer<CChar>, _ requestPermission: Bool, _ interrupt: Bool,
                           _ report: @convention(c) (UnsafePointer<CChar>) -> Void) -> Int32 {
    precondition(Thread.isMainThread)
    _ = NSApplication.shared // Identified bundle owns TCC; never registers shortcuts.
    var result: [String: Any] = ["bundleID": Bundle.main.bundleIdentifier ?? "unbundled",
        "bundlePath": Bundle.main.bundlePath, "durationLimitSeconds": 2.5,
        "microphone": String(cString: microphone), "audioRetained": false,
        "injectedInterruption": interrupt]
    if requestPermission && AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined {
        // Only this explicit flag can display the OS consent prompt. No engine is
        // opened while waiting, and no Accessibility permission is requested.
        var answered = false
        AVCaptureDevice.requestAccess(for: .audio) { _ in DispatchQueue.main.async { answered = true } }
        let deadline = Date().addingTimeInterval(30)
        while !answered && Date() < deadline { RunLoop.current.run(until: Date().addingTimeInterval(0.05)) }
    }
    result["permission"] = AVCaptureDevice.authorizationStatus(for: .audio).rawValue
    var code: Int32 = 1
    if AVCaptureDevice.authorizationStatus(for: .audio) != .authorized {
        result["outcome"] = "blocked-microphone-permission"
    } else {
        code = startCapture(String(cString: microphone), retainSamples: false)
        result["startCode"] = code
        if let state = diagnosticCapture { result["afterStart"] = captureMetadata(state) }
        var polls = [[String: Any]]()
        if code == 0 {
            let deadline = Date().addingTimeInterval(2.5)
            while Date() < deadline {
                RunLoop.current.run(until: Date().addingTimeInterval(0.1))
                if interrupt && polls.count == 3, let state = diagnosticCapture {
                    NotificationCenter.default.post(name: .AVAudioEngineConfigurationChange, object: state.engine)
                }
                let health = dictationHealth()
                if let state = diagnosticCapture {
                    polls.append(["health": health, "metadata": captureMetadata(state)])
                }
                if health != 0 { break }
            }
            code = dictationStop { _, _ in } // Probe retains no PCM; no insertion.
            result["stopCode"] = code
            if let state = diagnosticCapture {
                result["afterStop"] = captureMetadata(state)
                if code == 0 && state.diagnostic.outputFrames == 0 { code = 12 }
            }
        }
        result["polls"] = polls
        result["outcomeCode"] = code
        result["outcome"] = code == 0 ? "healthy-capture-and-stop" : "capture-failed"
    }
    dictationCancel()
    diagnosticCapture = nil
    if let data = try? JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys]),
       let json = String(data: data, encoding: .utf8) { json.withCString { report($0) } }
    return code
}
#endif
