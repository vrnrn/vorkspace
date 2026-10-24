// Concatenated with Clipboard.swift by the tooling test, outside the source tree.
// No live clipboard is read or written; the deadline is controlled deterministically.
extension ClipboardMenu {
    fileprivate func verifyNarrowLayout() {
        visible = true
        menuFrame = NSRect(x: 500, y: 100, width: 380, height: 300)
        begin(); add(id: 1, title: String(repeating: "Long clipboard text ", count: 20), isImage: false, pixels: nil, width: 0, height: 0); end(message: "")
        menuFrame.size.width = 280
        layout()
        assert(search.frame.width == 256 && scroll.frame.width == 268)
        assert(panel.contentView!.subviews.compactMap { $0 as? NSButton }.allSatisfy { $0.frame.maxX <= 280 })
        let cell = tableView(table, viewFor: nil, row: 0) as! NSTableCellView
        assert(cell.textField!.frame.maxX <= cell.bounds.maxX)
        hide()
    }
    fileprivate func verifyPreviewLifecycle() {
        visible = true
        menuFrame = NSRect(x: 500, y: 100, width: 380, height: 440)
        let pixels = [UInt8](repeating: 255, count: 16)
        begin()
        pixels.withUnsafeBufferPointer { add(id: 1, title: "Image", isImage: true, pixels: $0.baseAddress, width: 2, height: 2) }
        add(id: 2, title: "Text", isImage: false, pixels: nil, width: 0, height: 0)
        end(message: "")
        assert(preview.isHidden && panel.frame.width == 380)
        hover.enter(1, at: 100)
        showPreview(1, at: 100.999)
        assert(preview.isHidden && panel.frame.width == 380)
        showPreview(1, at: 101)
        assert(!preview.isHidden && panel.frame.width == 656)
        // Moving to text or out of the list removes the whole pane immediately.
        hoverRow(1)
        assert(table.selectedRow == 1)
        assert(preview.isHidden && panel.frame.width == 380)
        hover.enter(1, at: 200)
        cancelPreview()
        hover.enter(1, at: 201)
        showPreview(1, at: 201.5) // An earlier hover's completion cannot reveal it.
        assert(preview.isHidden)
        showPreview(1, at: 202)
        assert(!preview.isHidden)
        // Filtering/eviction releases the image and invalidates delayed publication.
        begin(); end(message: "No matches")
        showPreview(1, at: 300)
        assert(preview.isHidden && preview.image == nil && entries.isEmpty)
        // Drain AppKit's temporary autoreleased getters before checking that
        // even a retained table cell cannot keep an evicted image alive.
        var retainedCell: NSView?
        weak var releasedImage: NSImage?
        autoreleasepool {
            begin()
            pixels.withUnsafeBufferPointer { add(id: 3, title: "Image", isImage: true, pixels: $0.baseAddress, width: 2, height: 2) }
            end(message: "")
            releasedImage = entries[0].image
            retainedCell = tableView(table, viewFor: nil, row: 0)
            begin(); end(message: "")
        }
        withExtendedLifetime(retainedCell) { assert(releasedImage == nil) }
        begin(); add(id: 4, title: "Image", isImage: true, pixels: nil, width: 0, height: 0); end(message: "")
        hover.enter(4, at: ProcessInfo.processInfo.systemUptime - 2)
        begin()
        pixels.withUnsafeBufferPointer { add(id: 4, title: "Image", isImage: true, pixels: $0.baseAddress, width: 2, height: 2) }
        end(message: "")
        assert(!preview.isHidden, "A late thumbnail preserves a continuous hover")
        end(message: "Clipboard history is off", enabled: false)
        assert(search.isHidden && scroll.isHidden && empty.isHidden && preview.isHidden)
        assert(panel.frame.width == 220 && panel.frame.height == 94)
        let buttons = panel.contentView!.subviews.compactMap { $0 as? NSButton }.filter { !$0.isHidden }
        assert(buttons.map { $0.title } == ["Settings…", "Quick Composer", "Quit vorkspace"])
        assert(panel.contentView!.subviews.filter { $0 is NSBox }.allSatisfy { $0.isHidden })
        let button = buttons[0] as! MenuButton
        let event = NSEvent.enterExitEvent(with: .mouseEntered, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 0, trackingNumber: 0, userData: nil)!
        button.mouseEntered(with: event)
        assert(button.layer?.backgroundColor == menuHighlight.cgColor && table.selectedRow == -1)
        assert(button.cell!.titleRect(forBounds: button.bounds).minX >= 8)
        assert(search.focusRingType == .none)
        button.mouseExited(with: event)
        assert(button.layer?.backgroundColor == nil)
        end(message: "", enabled: true)
        assert(!search.isHidden && !scroll.isHidden && panel.frame.width == 380)
        hide()
        assert(!visible && entries.isEmpty && pending.isEmpty && timer == nil)
        showPreview(1, at: 400)
        assert(preview.isHidden)
    }
}
private func verifyPlacement() {
    let screen = NSRect(x: 0, y: 0, width: 1440, height: 900)
    func frame(_ x: CGFloat, enabled: Bool = true) -> NSRect {
        clipboardMenuFrame(anchor: NSPoint(x: x, y: 900), screen: screen, enabled: enabled, height: 300)
    }
    assert(frame(400).minX == 400 && frame(400).width == 380)
    assert(frame(1100).minX == 1100 && frame(1100).width == 334)
    assert(frame(1300).width == 280 && frame(1300).maxX == 1434)
    assert(frame(1100, enabled: false).minX == 1100 && frame(1100, enabled: false).width == 220)
    // Display coordinates can be negative; a small display still contains the menu.
    let secondary = NSRect(x: -1000, y: -200, width: 1000, height: 700)
    let placed = clipboardMenuFrame(anchor: NSPoint(x: -800, y: 500), screen: secondary, enabled: true, height: 300)
    assert(placed.minX == -800 && secondary.contains(placed))
    let narrow = NSRect(x: 0, y: 0, width: 240, height: 200)
    let fitted = clipboardMenuFrame(anchor: NSPoint(x: 220, y: 200), screen: narrow, enabled: true, height: 300)
    assert(fitted.width == 228 && narrow.contains(fitted))
}
@main struct ClipboardTests {
    static func main() {
        _ = NSApplication.shared
        verifyPlacement()
        let menu = ClipboardMenu(callback: { _, _, _, _ in }, context: nil)
        menu.verifyPreviewLifecycle()
        menu.verifyNarrowLayout()
        print("Native clipboard preview lifecycle passed")
    }
}
