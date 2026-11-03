// Disposable native destination for the explicit --field-fixture regression.
// It owns synthetic text only and closes itself if its test driver disappears.
import AppKit

@main
struct DictationFieldFixture {
    @MainActor static func main() {
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        app.finishLaunching()
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 620, height: 180),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "Dictation test"
        let field = NSTextView(frame: NSRect(x: 16, y: 16, width: 588, height: 148))
        field.setAccessibilityIdentifier("vorkspace-dictation-fixture")
        field.font = .systemFont(ofSize: 16)
        field.string = "Existing: "
        field.setSelectedRange(NSRange(location: (field.string as NSString).length, length: 0))
        window.contentView?.addSubview(field)
        window.center()
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(field)
        app.activate()
        if CommandLine.arguments.count == 3, CommandLine.arguments[1] == "--pid-path" {
            try? String(getpid()).write(toFile: CommandLine.arguments[2], atomically: true, encoding: .utf8)
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 45) { app.terminate(nil) }
        app.run()
    }
}
