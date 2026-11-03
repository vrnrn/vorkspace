import AppKit

/// Modifier-only shortcuts need both edges. A chord cancels a pending isolated
/// modifier, preserving normal Fn/media keys and Option typing.
struct DictationGesture {
    private(set) var down = false
    private(set) var holding = false
    mutating func press() -> Bool {
        guard !down else { return false }
        down = true
        return true
    }
    mutating func hold() -> Bool {
        guard down, !holding else { return false }
        holding = true
        return true
    }
    mutating func release() -> Int32? {
        guard down else { return nil }
        let action: Int32 = holding ? 2 : 0
        self = Self()
        return action
    }
    mutating func cancel() { self = Self() }
}

@MainActor
final class DictationKey {
    private var tap: CFMachPort?
    private var source: CFRunLoopSource?
    private var deadline: Task<Void, Never>?
    private var gesture = DictationGesture()
    private var key: CGKeyCode = 63
    private var flags: CGEventFlags = .maskSecondaryFn
    private var isolated = true
    private var epoch: UInt64 = 0
    var action: ((Int32) -> Void)?
    var onFailure: (() -> Void)?

    init(key: CGKeyCode = 63, flags: CGEventFlags = .maskSecondaryFn, isolated: Bool = true) {
        self.key = key
        self.flags = flags
        self.isolated = isolated
    }

    func bind(key: UInt16, modifiers: UInt32, isolated: Bool) -> Bool {
        stop()
        self.key = key
        self.isolated = isolated
        if isolated { flags = key == 63 ? .maskSecondaryFn : .maskAlternate }
        else {
            flags = []
            if modifiers & 256 != 0 { flags.insert(.maskCommand) }
            if modifiers & 512 != 0 { flags.insert(.maskShift) }
            if modifiers & 2048 != 0 { flags.insert(.maskAlternate) }
            if modifiers & 4096 != 0 { flags.insert(.maskControl) }
        }
        let mask = [CGEventType.flagsChanged, .keyDown, .keyUp].reduce(CGEventMask(0)) { $0 | (1 << $1.rawValue) }
        tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .defaultTap,
            eventsOfInterest: mask, callback: { _, type, event, pointer in
                guard let pointer else { return Unmanaged.passUnretained(event) }
                return MainActor.assumeIsolated {
                    let owner = Unmanaged<DictationKey>.fromOpaque(pointer).takeUnretainedValue()
                    return owner.receive(type, event) ? nil : Unmanaged.passUnretained(event)
                }
            }, userInfo: Unmanaged.passUnretained(self).toOpaque())
        guard let tap, let source = CFMachPortCreateRunLoopSource(nil, tap, 0) else { stop(); return false }
        self.source = source
        CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)
        return true
    }

    func stop() {
        epoch &+= 1
        deadline?.cancel(); deadline = nil
        gesture.cancel()
        if let source { CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes) }
        if let tap { CFMachPortInvalidate(tap) }
        tap = nil; source = nil
    }

    func receive(_ type: CGEventType, _ event: CGEvent) -> Bool {
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            stop()
            onFailure?()
            return false
        }
        // Our Unicode fallback emits tagged keyboard events. They are output,
        // never a user's modifier chord or an instruction to end a hold.
        if event.getIntegerValueField(.eventSourceUnixProcessID) == Int64(getpid()),
           event.getIntegerValueField(.eventSourceUserData) != 0 { return false }
        let code = CGKeyCode(event.getIntegerValueField(.keyboardEventKeycode))
        let all: CGEventFlags = [.maskCommand, .maskControl, .maskAlternate, .maskShift, .maskSecondaryFn]
        // Function flags on arrow keys do not change the configured chord.
        let relevant = isolated ? all : all.subtracting(.maskSecondaryFn)
        let matches = event.flags.intersection(relevant) == flags
        if code != key {
            // Once a chord is admitted, its trigger owns the release edge.
            // Releasing its modifiers first is valid; adding another modifier
            // still cancels it, as does any mismatch for an isolated modifier.
            let conflictingModifiers = isolated ? !matches : !event.flags.intersection(relevant).subtracting(flags).isEmpty
            if gesture.down && (type == .keyDown || (type == .flagsChanged && conflictingModifiers)) {
                let wasHolding = gesture.holding
                deadline?.cancel(); epoch &+= 1; gesture.cancel()
                if wasHolding { action?(2) }
            }
            return false
        }
        // AppKit's aggregate Option bit remains set when the other Option key
        // is held; the device-side bit distinguishes a right-key release.
        let modifierDown = key == 61 ? (event.flags.rawValue & 0x40 != 0) : matches
        let pressed = isolated ? (type == .flagsChanged && matches && modifierDown) : (type == .keyDown && matches)
        if pressed {
            if gesture.press() {
                epoch &+= 1
                let epoch = epoch
                deadline = Task { [weak self] in
                    try? await Task.sleep(for: .milliseconds(250))
                    guard !Task.isCancelled, let self, self.epoch == epoch, self.gesture.hold() else { return }
                    self.action?(1)
                }
            }
            return true
        }
        if gesture.down && (type == .keyUp || type == .flagsChanged) {
            deadline?.cancel(); epoch &+= 1
            if let action = gesture.release() { self.action?(action) }
            return true
        }
        return false
    }
}
