import AppKit
@main struct Tests {
 static func main() {
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
