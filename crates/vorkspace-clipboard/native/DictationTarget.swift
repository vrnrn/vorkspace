import AppKit
import ApplicationServices
import Carbon

func axElement(_ element: AXUIElement, _ attribute: String) -> AXUIElement? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success,
          let value, CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
    return (value as! AXUIElement)
}
func axString(_ element: AXUIElement, _ attribute: String) -> String? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success else { return nil }
    return value as? String
}
enum TargetExposure: Equatable { case editable, opaque, blocked }
func targetExposure(role: String?, subrole: String?, writable: Bool, enabled: Bool = true) -> TargetExposure {
    if !enabled || role == "AXSecureTextField" || subrole == kAXSecureTextFieldSubrole { return .blocked }
    if writable || [kAXTextFieldRole, kAXTextAreaRole, kAXComboBoxRole].contains(role ?? "") { return .editable }
    // Reject known non-text controls. Missing or custom semantics do not prove
    // that a keyboard responder is absent; opaque responders use stronger fences.
    if [kAXButtonRole, kAXCheckBoxRole, kAXRadioButtonRole, kAXSliderRole,
        kAXMenuRole, kAXMenuItemRole, kAXStaticTextRole, kAXImageRole,
        kAXProgressIndicatorRole, kAXScrollBarRole, "AXLink"].contains(role ?? "") { return .blocked }
    return .opaque
}
struct FocusEvidence {
    var pid: pid_t
    var window: CGWindowID
    var exposure: TargetExposure
    var elementPresent: Bool
}
// The policy is also exercised without ambient desktop state by native regressions.
func destinationMatches(_ original: FocusEvidence, _ current: FocusEvidence, sameElement: Bool, sameIdentity: Bool, interrupted: Bool) -> Bool {
    guard !interrupted, original.pid == current.pid, original.window == current.window,
          current.exposure != .blocked else { return false }
    if original.exposure == .editable {
        return current.exposure == .editable && (sameElement || sameIdentity)
    }
    // An opaque root is not a field identity. The input/activation/AX monitors
    // below fence every observed navigation; never downgrade a known text node.
    return original.elementPresent == current.elementPresent && (sameElement || !original.elementPresent)
}
private struct FocusSnapshot {
    let evidence: FocusEvidence
    let element: AXUIElement?
    let focusedWindow: AXUIElement?
    let identity: String?
    static func read() -> FocusSnapshot? {
        guard !IsSecureEventInputEnabled(), let app = NSWorkspace.shared.frontmostApplication else { return nil }
        let pid = app.processIdentifier
        // Window numbers are stable across AX wrapper recreation. Reading these
        // public metadata keys requires neither screen capture nor window titles.
        guard let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]],
              let window = windows.first(where: { ($0[kCGWindowOwnerPID as String] as? Int32) == pid && ($0[kCGWindowLayer as String] as? Int) == 0 }),
              let number = window[kCGWindowNumber as String] as? UInt32 else { return nil }
        let application = AXUIElementCreateApplication(pid)
        AXUIElementSetMessagingTimeout(application, 0.25)
        var value: CFTypeRef?
        let status = AXUIElementCopyAttributeValue(application, kAXFocusedUIElementAttribute as CFString, &value)
        // Unsupported/missing semantics are an opaque keyboard responder;
        // a timed-out, disabled or invalid AX connection is unavailable.
        guard [.success, .noValue, .attributeUnsupported, .notImplemented].contains(status) else { return nil }
        let element: AXUIElement?
        if let value, CFGetTypeID(value) == AXUIElementGetTypeID() {
            element = (value as! AXUIElement)
        } else { element = nil }
        var exposure: TargetExposure = .opaque
        var identity: String?
        if let element {
            var writable: DarwinBoolean = false
            let selected = AXUIElementIsAttributeSettable(element, kAXSelectedTextAttribute as CFString, &writable) == .success && writable.boolValue
            let role = axString(element, kAXRoleAttribute)
            var enabled: CFTypeRef?
            AXUIElementCopyAttributeValue(element, kAXEnabledAttribute as CFString, &enabled)
            exposure = targetExposure(role: role, subrole: axString(element, kAXSubroleAttribute), writable: selected, enabled: (enabled as? Bool) != false)
            // Only a provider-supplied identifier is evidence for recreated nodes.
            // Role/name/geometry alone cannot distinguish two destinations safely.
            if let id = axString(element, kAXIdentifierAttribute), !id.isEmpty, exposure == .editable {
                identity = "\(role ?? ""):\(id)"
            }
        }
        return FocusSnapshot(evidence: FocusEvidence(pid: pid, window: number, exposure: exposure, elementPresent: element != nil), element: element, focusedWindow: axElement(application, kAXFocusedWindowAttribute), identity: identity)
    }
}
final class DictationTarget {
    private let original: FocusSnapshot
    private var interrupted = false
    private var inputMonitor: Any?
    private var activation: NSObjectProtocol?
    private var observer: AXObserver?
    private init(_ snapshot: FocusSnapshot) { original = snapshot }
    static func admit() -> DictationTarget? {
        guard AXIsProcessTrusted(), let snapshot = FocusSnapshot.read(), snapshot.evidence.exposure != .blocked else { return nil }
        let owner = DictationTarget(snapshot)
        // Global monitor sees other processes only. The helper never activates or
        // accepts keyboard focus. Any intervening user input can change a custom
        // responder even when its AX root stays identical, so fail closed.
        owner.inputMonitor = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown, .otherMouseDown, .keyDown, .scrollWheel]) { [weak owner] event in
            if event.type == .keyDown && event.keyCode == 43 && event.modifierFlags.intersection([.command, .control, .option, .shift]) == .command { return }
            owner?.interrupted = true
        }
        guard owner.inputMonitor != nil else { return nil }
        owner.activation = NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main) { [weak owner] note in
            guard let owner else { return }
            if (note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication)?.processIdentifier != owner.original.evidence.pid { owner.interrupted = true }
        }
        var observer: AXObserver?
        if AXObserverCreate(snapshot.evidence.pid, { _, _, _, context in
            guard let context else { return }
            Unmanaged<DictationTarget>.fromOpaque(context).takeUnretainedValue().interrupted = true
        }, &observer) == .success, let observer {
            owner.observer = observer
            let application = AXUIElementCreateApplication(snapshot.evidence.pid)
            for name in [kAXFocusedUIElementChangedNotification, kAXFocusedWindowChangedNotification] {
                _ = AXObserverAddNotification(observer, application, name as CFString, Unmanaged.passUnretained(owner).toOpaque())
            }
            CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(observer), .commonModes)
        }
        guard owner.isCurrent() else { return nil }
        return owner
    }
    func isCurrent() -> Bool {
        guard let current = FocusSnapshot.read() else { return false }
        if let window = original.focusedWindow {
            guard let currentWindow = current.focusedWindow, CFEqual(window, currentWindow) else { return false }
        }
        let sameElement = original.element.flatMap { a in current.element.map { CFEqual(a, $0) } } ?? false
        return destinationMatches(original.evidence, current.evidence, sameElement: sameElement,
            sameIdentity: original.identity != nil && original.identity == current.identity, interrupted: interrupted)
    }
    deinit {
        if let inputMonitor { NSEvent.removeMonitor(inputMonitor) }
        if let activation { NSWorkspace.shared.notificationCenter.removeObserver(activation) }
        if let observer { CFRunLoopRemoveSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(observer), .commonModes) }
    }
}
