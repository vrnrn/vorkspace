// Explicit disposable desktop fixture. No model, microphone, database or TCC prompt.
// Compile with Dictation.swift, DictationTarget.swift and Microphones.swift.
import AppKit

@main struct FocusFixture {
    static func main() {
        let args = CommandLine.arguments
        let app = NSApplication.shared
        if args.count >= 4 && args[1] == "--target" {
            app.setActivationPolicy(.regular)
            let output = args[3]
            let window = NSWindow(contentRect: NSRect(x: 250, y: 250, width: 600, height: 240), styleMask: [.titled, .closable], backing: .buffered, defer: false)
            window.title = "Disposable dictation field"
            let field = args[2] == "secure" ? NSSecureTextField() : NSTextField()
            field.frame = NSRect(x: 30, y: 100, width: 540, height: 36)
            field.placeholderString = "Synthetic fixture only"
            field.setAccessibilityIdentifier("vorkspace-live-fixture")
            field.stringValue = "Existing: "
            let other = NSTextField(frame: NSRect(x: 30, y: 40, width: 540, height: 36))
            other.setAccessibilityIdentifier("vorkspace-live-fixture-other")
            window.contentView!.addSubview(other)
            field.nextKeyView = other
            window.contentView!.addSubview(field)
            window.makeKeyAndOrderFront(nil); app.activate(ignoringOtherApps: true)
            window.makeFirstResponder(field)
            field.currentEditor()?.selectedRange = NSRange(location: field.stringValue.utf16.count, length: 0)
            let reset = DistributedNotificationCenter.default().addObserver(
                forName: NSNotification.Name("com.vorkspace.disposable-dictation.reset.\(getpid())"),
                object: nil, queue: .main) { _ in
                    guard app.isActive, let editor = field.currentEditor(), window.firstResponder === editor else { return }
                    field.stringValue = "Existing: "
                    editor.string = "Existing: "
                    editor.selectedRange = NSRange(location: "Existing: ".utf16.count, length: 0)
                }
            var ticks = 0
            let timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { _ in
                ticks += 1
                let state = "\(field.stringValue)\nfocus=\(window.firstResponder === field.currentEditor())\n"
                try! state.write(toFile: output, atomically: true, encoding: .utf8)
                if ticks == 600 { app.terminate(nil) }
            }
            withExtendedLifetime((window, field, timer, reset)) { app.run() }
            return
        }
        guard args.count == 3, args[1] == "--probe", let pid = Int32(args[2]) else { fatalError("--target native|secure OUTPUT or --probe PID") }
        guard AXIsProcessTrusted() else { print("BLOCKED: fixture process has no Accessibility trust; no permission requested, no paste attempted"); return }
        guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { fatalError("Fixture must be frontmost; refusing other destinations") }
        let application = AXUIElementCreateApplication(pid)
        let element = axElement(application, kAXFocusedUIElementAttribute)
        print("Observed focus role=\(element.flatMap { axString($0, kAXRoleAttribute) } ?? "missing") subrole=\(element.flatMap { axString($0, kAXSubroleAttribute) } ?? "missing")")
        guard dictationPrepareInsertion() else { print("Admission refused"); return }
        // Allow monitors to process asynchronous notifications before insertion.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
            guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { fatalError("Fixture lost focus") }
            let code = "synthetic dictation fixture".withCString { dictationPaste($0, 0, 0) }
            print("Insertion result=\(code)")
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
                dictationShutdown(); app.stop(nil)
            }
        }
        app.run()
    }
}
