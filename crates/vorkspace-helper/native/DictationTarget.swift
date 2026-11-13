import AppKit
import ApplicationServices
import Carbon

private func focusedElement(_ parent: AXUIElement, _ attribute: String) throws -> AXUIElement? {
    var value: CFTypeRef?
    let status = AXUIElementCopyAttributeValue(parent, attribute as CFString, &value)
    if status == .noValue || status == .attributeUnsupported || status == .notImplemented { return nil }
    guard status == .success, let value, CFGetTypeID(value) == AXUIElementGetTypeID() else {
        throw DictationError("macOS could not confirm the focused field. Try again.")
    }
    return (value as! AXUIElement)
}

private func elementString(_ element: AXUIElement, _ attribute: String) -> String? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success else { return nil }
    return value as? String
}

private func selectedRange(_ element: AXUIElement) -> CFRange? {
    var value: CFTypeRef?
    var range = CFRange()
    guard AXUIElementCopyAttributeValue(element, kAXSelectedTextRangeAttribute as CFString, &value) == .success,
          let value, CFGetTypeID(value) == AXValueGetTypeID(),
          AXValueGetValue(value as! AXValue, .cfRange, &range),
          range.location >= 0, range.length >= 0, range.location <= Int.max - range.length else { return nil }
    return range
}

private func writable(_ element: AXUIElement, _ attribute: String) -> Bool {
    var settable: DarwinBoolean = false
    return AXUIElementIsAttributeSettable(element, attribute as CFString, &settable) == .success && settable.boolValue
}

struct DictationEdit: Equatable {
    let erase: Int
    let append: String
    init(from previous: String, to next: String) {
        let common = zip(previous, next).prefix(while: { $0 == $1 }).count
        erase = previous.count - common
        append = String(next.dropFirst(common))
    }
}

struct DictationModifierPolicy {
    let key: UInt16
    let modifiers: UInt32
    func permits(_ held: [UInt16], triggerHeld: Bool) -> Bool {
        held.allSatisfy { code in
            guard triggerHeld else { return false }
            if code == key { return true }
            switch Int(code) {
            case kVK_Command, kVK_RightCommand: return modifiers & 256 != 0
            case kVK_Shift, kVK_RightShift: return modifiers & 512 != 0
            case kVK_Option, kVK_RightOption: return modifiers & 2048 != 0
            case kVK_Control, kVK_RightControl: return modifiers & 4096 != 0
            default: return false
            }
        }
    }
}

/// One focused destination owns all provisional text. Real input revokes that
/// ownership permanently, even if the user later returns to the same field.
@MainActor
protocol DictationDestination: AnyObject {
    var onInvalidated: (() -> Void)? { get set }
    var historyText: String { get }
    func invalidate()
    func check() throws
    func offer(_ text: String) throws
    func finish() async throws
}

extension DictationDestination { var historyText: String { "" } }

@MainActor
final class DictationTarget: DictationDestination {
    let pid: pid_t
    let tag = Int64.random(in: 1...Int64.max)
    var onInvalidated: (() -> Void)?
    private let application: AXUIElement
    private let element: AXUIElement?
    private let window: AXUIElement?
    private var monitor: Any?
    private var activation: NSObjectProtocol?
    private var observer: AXObserver?
    private(set) var valid = true
    private var rendered = ""
    private(set) var historyText = ""
    private let modifierPolicy: DictationModifierPolicy
    private var expectedSelection: CFRange?
    private var anchor = 0
    private var delivered = false
    private var pending: String?
    private var retry: Task<Void, Never>?
    var usesTextRange: Bool { expectedSelection != nil }

    init(keyCode: UInt16, modifiers: UInt32) throws {
        modifierPolicy = DictationModifierPolicy(key: keyCode, modifiers: modifiers)
        guard AXIsProcessTrusted(), !IsSecureEventInputEnabled(),
              let app = NSWorkspace.shared.frontmostApplication,
              app.processIdentifier != getpid() else {
            throw DictationError("Focus a text field and allow Accessibility for vorkspace helper.")
        }
        pid = app.processIdentifier
        application = AXUIElementCreateApplication(pid)
        AXUIElementSetMessagingTimeout(application, 0.1)
        element = try focusedElement(application, kAXFocusedUIElementAttribute)
        window = try focusedElement(application, kAXFocusedWindowAttribute)
        if let element {
            AXUIElementSetMessagingTimeout(element, 0.1)
            let role = elementString(element, kAXRoleAttribute) ?? ""
            let subrole = elementString(element, kAXSubroleAttribute) ?? ""
            guard role != "AXSecureTextField", subrole != kAXSecureTextFieldSubrole,
                  ![kAXButtonRole, kAXCheckBoxRole, kAXRadioButtonRole, kAXSliderRole,
                    kAXMenuRole, kAXMenuItemRole, kAXStaticTextRole, kAXImageRole].contains(role)
            else { throw DictationError("Focus an editable text field to dictate.") }
            if writable(element, kAXSelectedTextAttribute), writable(element, kAXSelectedTextRangeAttribute),
               let range = selectedRange(element) {
                expectedSelection = range
                anchor = range.location
            }
        }
        var flags: NSEvent.ModifierFlags = []
        if modifiers & 256 != 0 { flags.insert(.command) }
        if modifiers & 512 != 0 { flags.insert(.shift) }
        if modifiers & 2048 != 0 { flags.insert(.option) }
        if modifiers & 4096 != 0 { flags.insert(.control) }
        let shortcutFlags = flags
        monitor = NSEvent.addGlobalMonitorForEvents(matching: [.keyDown, .leftMouseDown, .rightMouseDown, .otherMouseDown, .scrollWheel]) { [weak self] event in
            guard let self else { return }
            if event.cgEvent?.getIntegerValueField(.eventSourceUserData) == self.tag,
               event.cgEvent?.getIntegerValueField(.eventSourceUnixProcessID) == Int64(getpid()) { return }
            if event.type == .keyDown, event.keyCode == keyCode,
               event.modifierFlags.intersection([.command, .control, .option, .shift]) == shortcutFlags { return }
            self.invalidate()
        }
        guard monitor != nil else { throw DictationError("Allow Accessibility for vorkspace helper, then try again.") }
        activation = NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { try? self?.check() }
        }
        var axObserver: AXObserver?
        if AXObserverCreate(pid, { _, _, _, context in
            guard let context else { return }
            MainActor.assumeIsolated {
                try? Unmanaged<DictationTarget>.fromOpaque(context).takeUnretainedValue().check()
            }
        }, &axObserver) == .success, let axObserver {
            observer = axObserver
            for name in [kAXFocusedUIElementChangedNotification, kAXFocusedWindowChangedNotification] {
                AXObserverAddNotification(axObserver, application, name as CFString, Unmanaged.passUnretained(self).toOpaque())
            }
            CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(axObserver), .commonModes)
        }
        try check()
    }

    func invalidate() {
        guard valid else { return }
        valid = false
        pending = nil
        retry?.cancel()
        onInvalidated?()
    }

    func check() throws {
        do {
            guard canDeliver else { throw CancellationError() }
            let current = try focusedElement(application, kAXFocusedUIElementAttribute)
            let sameElement = element.map { original in
                current.map { CFEqual(original, $0) } ?? false
            } ?? (current == nil)
            guard sameElement else { throw CancellationError() }
            if let window {
                guard let current = try focusedElement(application, kAXFocusedWindowAttribute),
                      CFEqual(window, current) else { throw CancellationError() }
            }
            guard canDeliver else { throw CancellationError() }
        } catch {
            invalidate()
            throw error
        }
    }

    private var canDeliver: Bool {
        valid && AXIsProcessTrusted() && !IsSecureEventInputEnabled()
            && NSWorkspace.shared.frontmostApplication?.processIdentifier == pid
    }

    func offer(_ text: String) throws {
        guard text.utf8.count <= 32 * 1024 else { throw DictationError("Finish this dictation before starting another.") }
        try check()
        pending = text
        if try flushPending() { return }
        guard retry == nil else { return }
        retry = Task { [weak self] in
            defer { self?.retry = nil }
            let deadline = ContinuousClock.now + .seconds(3)
            while let self, self.pending != nil, !Task.isCancelled {
                do {
                    try await Task.sleep(for: .milliseconds(20))
                    if try self.flushPending() { return }
                    if ContinuousClock.now >= deadline { self.invalidate(); return }
                } catch { self.invalidate(); return }
            }
        }
    }

    private func flushPending() throws -> Bool {
        try check()
        guard let next = pending else { return true }
        let modifierKeys = [kVK_Command, kVK_RightCommand, kVK_Shift, kVK_RightShift,
                            kVK_Option, kVK_RightOption, kVK_Control, kVK_RightControl]
        let held = modifierKeys.filter { CGEventSource.keyState(.hidSystemState, key: CGKeyCode($0)) }.map(UInt16.init)
        guard modifierPolicy.permits(held, triggerHeld: CGEventSource.keyState(.hidSystemState, key: modifierPolicy.key)) else { return false }
        let edit = DictationEdit(from: rendered, to: next)
        if edit.erase == 0 && edit.append.isEmpty { pending = nil; return true }
        if let element, let selection = expectedSelection {
            guard let current = selectedRange(element), current.location == selection.location,
                  current.length == selection.length else { invalidate(); throw CancellationError() }
            if delivered && !rendered.isEmpty {
                var range = CFRange(location: anchor, length: rendered.utf16.count)
                let parameter = AXValueCreate(.cfRange, &range)!
                var value: CFTypeRef?
                let status = AXUIElementCopyParameterizedAttributeValue(element,
                    kAXStringForRangeParameterizedAttribute as CFString, parameter, &value)
                if status == .success {
                    guard value as? String == rendered else { invalidate(); throw CancellationError() }
                } else if status != .attributeUnsupported && status != .notImplemented {
                    invalidate(); throw CancellationError()
                }
            }
            // Use the native text range when available. It acknowledges the
            // edit synchronously and detects programmatic caret/content changes.
            let common = String(rendered.prefix(rendered.count - edit.erase)).utf16.count
            var range = delivered ? CFRange(location: anchor + common, length: rendered.utf16.count - common) : selection
            let parameter = AXValueCreate(.cfRange, &range)!
            pending = nil; rendered = next; delivered = true
            try check()
            guard AXUIElementSetAttributeValue(element, kAXSelectedTextRangeAttribute as CFString, parameter) == .success else {
                invalidate(); throw DictationError("This field refused dictation. Try again in another field.")
            }
            try check()
            guard AXUIElementSetAttributeValue(element, kAXSelectedTextAttribute as CFString, edit.append as CFString) == .success else {
                invalidate(); throw DictationError("This field refused dictation. Try again in another field.")
            }
            historyText = next
            expectedSelection = CFRange(location: anchor + next.utf16.count, length: 0)
            return true
        }
        // Consume before posting: CGEvent has no delivery acknowledgement. An
        // uncertain external effect is never retried or replayed into a new turn.
        pending = nil
        rendered = next
        delivered = true
        for _ in 0..<edit.erase { try post(key: CGKeyCode(kVK_Delete), units: []) }
        let units = Array(edit.append.utf16)
        var offset = 0
        while offset < units.count {
            var end = min(offset + 20, units.count)
            if end < units.count, (0xD800...0xDBFF).contains(units[end - 1]) { end -= 1 }
            try post(key: 0, units: Array(units[offset..<end]))
            offset = end
        }
        historyText = next
        return true
    }

    private func post(key: CGKeyCode, units: [UniChar]) throws {
        try check()
        guard let source = CGEventSource(stateID: .privateState),
              let down = CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: false) else {
            invalidate()
            throw DictationError("macOS could not deliver text to this field. Try again.")
        }
        for event in [down, up] {
            event.flags = []
            event.setIntegerValueField(.eventSourceUserData, value: tag)
            if !units.isEmpty { event.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units) }
            event.postToPid(pid)
        }
    }

    func finish() async throws {
        while pending != nil {
            try check()
            try await Task.sleep(for: .milliseconds(20))
        }
        try check()
    }

    deinit {
        retry?.cancel()
        if let monitor { NSEvent.removeMonitor(monitor) }
        if let activation { NSWorkspace.shared.notificationCenter.removeObserver(activation) }
        if let observer { CFRunLoopRemoveSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(observer), .commonModes) }
    }
}
