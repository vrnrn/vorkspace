import AppKit
import AVFoundation
import ApplicationServices

// Main-thread owner. Only the tap touches PCM, behind the lock. No idle capture.
private final class DictationCapture {
    let engine = AVAudioEngine()
    let lock = NSLock()
    var samples = [Float]()
    var failed = false
    var full = false
    var target: AXUIElement?
    var pid: pid_t = 0
    var observer: NSObjectProtocol?
    init() {
        observer = NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: nil) { [weak self] _ in
            guard let self else { return }
            self.lock.lock(); self.failed = true; self.lock.unlock()
        }
    }
    deinit { if let observer { NotificationCenter.default.removeObserver(observer) } }
    func stop() {
        engine.stop()
        engine.inputNode.removeTap(onBus: 0)
        if let observer { NotificationCenter.default.removeObserver(observer) }
    }
    func focused() -> AXUIElement? {
        guard let app = NSWorkspace.shared.frontmostApplication else { return nil }
        let application = AXUIElementCreateApplication(app.processIdentifier)
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(application, kAXFocusedUIElementAttribute as CFString, &value) == .success,
              let value, CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
        return (value as! AXUIElement)
    }
}
private func editableTarget(_ target: AXUIElement) -> Bool {
    var role: CFTypeRef?, subrole: CFTypeRef?
    AXUIElementCopyAttributeValue(target, kAXRoleAttribute as CFString, &role)
    AXUIElementCopyAttributeValue(target, kAXSubroleAttribute as CFString, &subrole)
    if subrole as? String == kAXSecureTextFieldSubrole { return false }
    var editable: DarwinBoolean = false
    let selected = AXUIElementIsAttributeSettable(target, kAXSelectedTextAttribute as CFString, &editable) == .success && editable.boolValue
    return selected || [kAXTextFieldRole, kAXTextAreaRole, kAXComboBoxRole].contains(role as? String ?? "")
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
private var targetOwner: DictationCapture?

@_cdecl("vorkspace_dictation_permissions")
func dictationPermissions() {
    AVCaptureDevice.requestAccess(for: .audio) { _ in }
    let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
    _ = AXIsProcessTrustedWithOptions(options)
}

// 1 mic permission, 2 Accessibility, 3 device failure, 4 target unavailable.
@_cdecl("vorkspace_dictation_start")
func dictationStart() -> Int32 {
    guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else { return 1 }
    guard AXIsProcessTrusted() else { return 2 }
    let state = DictationCapture()
    guard let app = NSWorkspace.shared.frontmostApplication, let target = state.focused() else { return 4 }
    guard editableTarget(target) else { return 4 }
    state.target = target; state.pid = app.processIdentifier
    let node = state.engine.inputNode
    let format = node.outputFormat(forBus: 0)
    guard format.sampleRate > 0, format.channelCount > 0,
          let output = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16000, channels: 1, interleaved: false),
          let converter = AVAudioConverter(from: format, to: output) else { return 3 }
    state.samples.reserveCapacity(16000 * 120)
    node.installTap(onBus: 0, bufferSize: 2048, format: format) { buffer, _ in
        let capacity = AVAudioFrameCount(Double(buffer.frameLength) * 16000 / format.sampleRate + 32)
        guard let result = AVAudioPCMBuffer(pcmFormat: output, frameCapacity: capacity) else { return }
        var supplied = false
        var error: NSError?
        converter.convert(to: result, error: &error) { _, status in
            if supplied { status.pointee = .noDataNow; return nil }
            supplied = true; status.pointee = .haveData; return buffer
        }
        state.lock.lock(); defer { state.lock.unlock() }
        if error != nil { state.failed = true; return }
        guard let pcm = result.floatChannelData?[0] else { return }
        let count = min(Int(result.frameLength), 16000 * 120 - state.samples.count)
        if count > 0 { state.samples.append(contentsOf: UnsafeBufferPointer(start: pcm, count: count)) }
        state.full = state.samples.count == 16000 * 120
    }
    do { try state.engine.start() } catch { state.stop(); return 3 }
    capture = state; targetOwner = state
    return 0
}
@_cdecl("vorkspace_dictation_health")
func dictationHealth() -> Int32 {
    guard let state = capture else { return 0 }
    state.lock.lock(); defer { state.lock.unlock() }
    if state.failed || !state.engine.isRunning || AVCaptureDevice.authorizationStatus(for: .audio) != .authorized { return 1 }
    return state.full ? 2 : 0
}
@_cdecl("vorkspace_dictation_stop")
func dictationStop(_ callback: @convention(c) (UnsafePointer<Float>?, Int) -> Void) {
    guard let state = capture else { callback(nil, 0); return }
    state.stop(); capture = nil
    state.lock.lock(); let pcm = state.samples; state.samples.removeAll(); state.lock.unlock()
    pcm.withUnsafeBufferPointer { callback($0.baseAddress, $0.count) }
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
    guard let owner = targetOwner, let target = owner.target,
          NSWorkspace.shared.frontmostApplication?.processIdentifier == owner.pid,
          let current = owner.focused(), CFEqual(target, current), editableTarget(current) else { return 4 }
    // Caller waits for shortcut modifiers to lift. Never synthesize a modified paste.
    guard !dictationModifiersDown() else { return 5 }
    restoration.flush()
    let board = NSPasteboard.general
    guard let saved = clipboardSnapshot(board) else { return 6 }
    // Pasteboard providers may have taken time; validate the destination again.
    guard NSWorkspace.shared.frontmostApplication?.processIdentifier == owner.pid,
          let focused = owner.focused(), CFEqual(target, focused), editableTarget(focused),
          !dictationModifiersDown() else { return 4 }
    guard let source = CGEventSource(stateID: .combinedSessionState),
          let down = CGEvent(keyboardEventSource: source, virtualKey: 9, keyDown: true),
          let up = CGEvent(keyboardEventSource: source, virtualKey: 9, keyDown: false) else { return 7 }
    board.clearContents()
    guard board.setString(String(cString: text), forType: .string),
          board.setString("1", forType: NSPasteboard.PasteboardType("com.vorkspace.transient-dictation")) else {
        restoreClipboard(board, revision: board.changeCount, items: saved); return 7
    }
    let revision = board.changeCount
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
