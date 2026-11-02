import AppKit
import AudioToolbox
func verifyCaptureHealthContract() {
    assert(CaptureHealth.active.rawValue == 0 && CaptureHealth.failed.rawValue == 1)
    assert(CaptureHealth.full.rawValue == 2 && CaptureHealth.starting.rawValue == 3)
    var lifecycle = CaptureLifecycle()
    assert(lifecycle.health(full: false) == .failed) // No capture is not active.
    assert(lifecycle.beginStarting())
    assert(lifecycle.health(full: true) == .starting)
    lifecycle.receivedInput()
    assert(lifecycle.health(full: false) == .starting) // Still needs route commit.
    assert(lifecycle.beginArming())
    assert(lifecycle.arm(healthy: true))
    assert(lifecycle.health(full: false) == .active)
    assert(lifecycle.health(full: true) == .full)
    lifecycle.configurationChanged()
    assert(lifecycle.health(full: false) == .failed)
    var timeout = CaptureLifecycle()
    assert(timeout.beginStarting())
    assert(timeout.checkInputDeadline(now: 2, deadline: 2))
    assert(timeout.health(full: false) == .failed)
    dictationCancel()
    assert(dictationHealth() == CaptureHealth.failed.rawValue)
    print("Capture health ABI pending, active, full, timeout and absent-owner regression passed")
}
// Exact measured route regression: selecting built-in 79 succeeded, then the
// legacy setter's default aggregate returned as 117 with a matching tap format.
// Preserve refusal for that route even when every other check passes.
func verifyCaptureRouteEvidence() {
    var route = CaptureRouteEvidence(expectedDevice: 79, actualDevice: 79,
        expectedUID: "BuiltInMicrophoneDevice", actualUID: "BuiltInMicrophoneDevice",
        authorized: true, alive: true, deviceStatus: noErr, formatMatches: true, running: true)
    assert(route.failure == nil)
    route.actualDevice = 117
    assert(route.failure == "current-device-mismatch")
    var admission = CaptureLifecycle()
    assert(admission.beginStarting()); admission.receivedInput()
    assert(admission.beginArming())
    assert(!admission.arm(healthy: route.failure == nil))
    route.actualDevice = 79
    assert(!admission.arm(healthy: route.failure == nil))
    route.actualUID = "replacement"
    assert(route.failure == "device-uid")
    route.actualUID = "BuiltInMicrophoneDevice"
    route.deviceStatus = -50
    assert(route.failure == "current-device-osstatus:-50")
    route.deviceStatus = noErr; route.formatMatches = false
    assert(route.failure == "format-mismatch")
    route.formatMatches = true; route.authorized = false
    assert(route.failure == "permission")
    route.authorized = true; route.alive = false
    assert(route.failure == "device-not-alive")
    route.alive = true; route.running = false
    assert(route.failure == "engine-not-running")
    dictationCancel()
    assert(dictationStop { pcm, count in assert(pcm == nil && count == 0) } == 11)
    print("Measured aggregate route reversion and permission/device/format/engine refusal regressions passed")
}
// Event traces exercise the production policies without an engine, microphone,
// Accessibility grant, scheduling delay or ambient desktop destination.
func verifyCaptureNotificationLifecycle() {
    var selectedRoute = CaptureLifecycle()
    selectedRoute.configurationChanged() // Expected CurrentDevice/format setup.
    selectedRoute.configurationChanged() // Expected engine initialization.
    assert(!selectedRoute.failed && !selectedRoute.active)
    assert(selectedRoute.beginStarting())
    assert(!selectedRoute.beginArming()) // start() alone is not input readiness.
    selectedRoute.configurationChanged() // Measured delayed setup notice.
    assert(!selectedRoute.failed && !selectedRoute.active)
    selectedRoute.receivedInput()
    assert(selectedRoute.beginArming())
    assert(selectedRoute.arm(healthy: true))
    assert(selectedRoute.active)
    assert(!selectedRoute.checkInputDeadline(now: 500, deadline: 2))
    selectedRoute.configurationChanged() // An actual change during capture.
    assert(selectedRoute.failed && !selectedRoute.active)
    assert(!selectedRoute.arm(healthy: true)) // Readback cannot clear interruption.

    var duringBaseline = CaptureLifecycle()
    assert(duringBaseline.beginStarting()); duringBaseline.receivedInput()
    assert(duringBaseline.beginArming())
    let routeReadback = true
    duringBaseline.configurationChanged() // Between registration/read and commit.
    assert(!duringBaseline.arm(healthy: routeReadback))
    assert(duringBaseline.failed && !duringBaseline.active)

    var lostBeforeObservation = CaptureLifecycle()
    lostBeforeObservation.configurationChanged() // No active observer yet.
    assert(lostBeforeObservation.beginStarting()); lostBeforeObservation.receivedInput()
    assert(lostBeforeObservation.beginArming())
    assert(!lostBeforeObservation.arm(healthy: false)) // Fresh baseline catches loss.
    assert(!lostBeforeObservation.arm(healthy: true))

    var conversionFailure = CaptureLifecycle()
    conversionFailure.fail() // Tap conversion may fail while engine is starting.
    conversionFailure.configurationChanged()
    assert(!conversionFailure.beginStarting()) // Setup must not reset another failure.
    var noBuffers = CaptureLifecycle()
    assert(noBuffers.beginStarting())
    assert(!noBuffers.checkInputDeadline(now: 1.99, deadline: 2))
    assert(noBuffers.checkInputDeadline(now: 2, deadline: 2))
    noBuffers.receivedInput()
    assert(!noBuffers.readyToArm && !noBuffers.beginArming())
    print("Capture setup, arming-race, active-interruption and fresh-baseline regressions passed")
}
func verifyFocusNotificationLifecycle() {
    let field = FocusEvidence(pid: 101, window: 20, exposure: .editable, elementPresent: true)
    var unchanged = DestinationContinuity(original: field)
    for _ in 0..<3 {
        assert(unchanged.observe(field, sameElement: true, sameIdentity: false, sameWindow: true))
    }
    // A duplicate redraw can recreate its AX wrapper while keeping field identity.
    assert(unchanged.observe(field, sameElement: false, sameIdentity: true, sameWindow: true))
    // A different field in the same app/window must invalidate irreversibly.
    assert(!unchanged.observe(field, sameElement: false, sameIdentity: false, sameWindow: true))
    assert(!unchanged.observe(nil, sameElement: false, sameIdentity: false, sameWindow: false))
    assert(!unchanged.observe(field, sameElement: true, sameIdentity: true, sameWindow: true))

    var switchedWindow = DestinationContinuity(original: field)
    var otherWindow = field; otherWindow.window = 21
    assert(!switchedWindow.observe(otherWindow, sameElement: true, sameIdentity: true, sameWindow: true))
    assert(!switchedWindow.observe(field, sameElement: true, sameIdentity: true, sameWindow: true))
    var axWindowChanged = DestinationContinuity(original: field)
    assert(!axWindowChanged.observe(field, sameElement: true, sameIdentity: true, sameWindow: false))

    var unavailable = DestinationContinuity(original: field)
    assert(!unavailable.observe(nil, sameElement: false, sameIdentity: false, sameWindow: false))
    assert(!unavailable.observe(field, sameElement: true, sameIdentity: true, sameWindow: true))

    let opaque = FocusEvidence(pid: 101, window: 20, exposure: .opaque, elementPresent: false)
    var custom = DestinationContinuity(original: opaque)
    assert(custom.observe(opaque, sameElement: false, sameIdentity: false, sameWindow: true))
    custom.invalidate() // User navigation still fences opaque fields.
    assert(!custom.observe(opaque, sameElement: false, sameIdentity: false, sameWindow: true))
    print("Duplicate AX notification, stable identity, changed destination and sticky refusal regressions passed")
}
@main struct Tests {
 static func main() {
 verifyCaptureHealthContract()
 verifyCaptureRouteEvidence()
 verifyCaptureNotificationLifecycle()
 verifyFocusNotificationLifecycle()
 // The old guard rejected these keyboard responders solely for missing AX text semantics.
 assert(targetExposure(role: nil, subrole: nil, writable: false) == .opaque)
 assert(targetExposure(role: "AXCustomKeyboardResponder", subrole: nil, writable: false) == .opaque)
 assert(targetExposure(role: "AXGroup", subrole: nil, writable: false) == .opaque)
 assert(targetExposure(role: "AXTextArea", subrole: nil, writable: false) == .editable)
 assert(targetExposure(role: "AXTextField", subrole: "AXSecureTextField", writable: true) == .blocked)
 assert(targetExposure(role: "AXButton", subrole: nil, writable: false) == .blocked)
 let opaque = FocusEvidence(pid: 101, window: 20, exposure: .opaque, elementPresent: false)
 let text = FocusEvidence(pid: 101, window: 20, exposure: .editable, elementPresent: true)
 assert(destinationMatches(opaque, opaque, sameElement: false, sameIdentity: false, interrupted: false))
 assert(!destinationMatches(opaque, opaque, sameElement: false, sameIdentity: false, interrupted: true))
 assert(destinationMatches(text, text, sameElement: false, sameIdentity: true, interrupted: false))
 assert(!destinationMatches(text, text, sameElement: false, sameIdentity: false, interrupted: false))
 assert(!destinationMatches(text, opaque, sameElement: false, sameIdentity: false, interrupted: false))
 var switched = opaque; switched.pid = 102
 assert(!destinationMatches(opaque, switched, sameElement: false, sameIdentity: false, interrupted: false))
 switched = opaque; switched.window = 21
 assert(!destinationMatches(opaque, switched, sameElement: false, sameIdentity: false, interrupted: false))
 switched = opaque; switched.exposure = .blocked
 assert(!destinationMatches(opaque, switched, sameElement: true, sameIdentity: true, interrupted: false))
 // Transport metadata wins even when an external mic's name says built-in.
 let usb = InputDevice(id: 7, uid: "usb", name: "MacBook microphone", builtin: false)
 let builtin = InputDevice(id: 9, uid: "internal", name: "Internal input", builtin: true)
 assert(resolveMicrophone("builtin", devices: [usb, builtin], system: 7) == 9)
 assert(resolveMicrophone("system", devices: [usb, builtin], system: 7) == 7)
 assert(resolveMicrophone("uid:usb", devices: [builtin, usb], system: 9) == 7)
 assert(resolveMicrophone("uid:missing", devices: [builtin, usb], system: 9) == nil)
 assert(resolveMicrophone("builtin", devices: [usb], system: 7) == nil)
 assert(resolveMicrophone("webkit-opaque-id", devices: [usb], system: 7) == nil)
 print("Focus admission, destination changes, secure fields and native UID policy passed")
 print("Native inputs (enumeration only): \(inputDevices())")
 let board=NSPasteboard.withUniqueName();defer {board.releaseGlobally()}
 let original=NSPasteboardItem();original.setString("original",forType:.string);original.setData(Data("{\\rtf1\\ansi original}".utf8),forType:.rtf);board.writeObjects([original]);
 let saved=clipboardSnapshot(board)!;assert(clipboardSnapshot(board,limit:1)==nil)
 board.clearContents();board.setString("dictation",forType:.string);let revision=board.changeCount
 restoreClipboard(board,revision:revision,items:saved)
 assert(board.string(forType:.string)=="original");assert(board.data(forType:.rtf)==Data("{\\rtf1\\ansi original}".utf8))
 board.clearContents();board.setString("dictation",forType:.string);let older=board.changeCount
 board.clearContents();board.setString("new user copy",forType:.string)
 restoreClipboard(board,revision:older,items:saved);assert(board.string(forType:.string)=="new user copy")
 let restoration = ClipboardRestoration(); var first = 0; var second = 0
 restoration.install(revision: 1) { first += 1 }; restoration.flush()
 restoration.install(revision: 2) { second += 1 }; restoration.finish(revision: 1)
 assert(first == 1 && second == 0); restoration.flush(); assert(second == 1)
 print("Rich clipboard restoration and newer-copy fencing passed")
 }
}
