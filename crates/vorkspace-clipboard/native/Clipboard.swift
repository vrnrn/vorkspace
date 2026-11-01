// Native presentation only. Rust owns history, byte limits, search and copying.
// Maccy-inspired minimal clipboard menu; see licenses/LICENSE-MACCY-MIT.
import AppKit

// Identity plus a deadline prevents a timer for an old row from reopening a preview.
struct ImageHover {
    private(set) var item: UInt64?
    private var deadline: TimeInterval = 0
    mutating func enter(_ id: UInt64?, at time: TimeInterval) {
        guard id != item else { return }
        item = id
        deadline = time + 1
    }
    func ready(at time: TimeInterval) -> UInt64? { time >= deadline ? item : nil }
    mutating func cancel() { item = nil }
}

// Align to the status item's left edge, shrinking before shifting at the edge.
private func clipboardMenuFrame(anchor: NSPoint, screen: NSRect, enabled: Bool, height: CGFloat) -> NSRect {
    let available = screen.insetBy(dx: 6, dy: 6)
    let preferred: CGFloat = enabled ? 380 : 220
    let minimum = min(enabled ? 280 : 220, available.width)
    let room = available.maxX - max(anchor.x, available.minX)
    let width = min(preferred, available.width, max(minimum, room))
    let height = min(height, available.height)
    return NSRect(x: min(max(anchor.x, available.minX), available.maxX - width),
        y: max(available.minY, min(anchor.y - height - 6, available.maxY - height)),
        width: width, height: height)
}

private typealias Callback = @convention(c) (UnsafeMutableRawPointer?, UInt32, UInt64, UnsafePointer<CChar>?) -> Void
private struct Entry {
    let id: UInt64
    let title: String
    let isImage: Bool
    let image: NSImage?
}
private final class Panel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}
private var menuHighlight: NSColor { .labelColor.withAlphaComponent(0.12) }
private final class MenuButtonCell: NSButtonCell {
    override func titleRect(forBounds rect: NSRect) -> NSRect {
        super.titleRect(forBounds: rect).insetBy(dx: 8, dy: 0)
    }
}
private final class MenuButton: NSButton {
    override class var cellClass: AnyClass? { get { MenuButtonCell.self } set {} }
    var entered: (() -> Void)?
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }
    override func mouseEntered(with event: NSEvent) {
        wantsLayer = true
        layer?.cornerRadius = 5
        layer?.backgroundColor = menuHighlight.cgColor
        contentTintColor = .labelColor
        entered?()
    }
    func resetHover() { layer?.backgroundColor = nil; contentTintColor = .labelColor }
    override func mouseExited(with event: NSEvent) { resetHover() }
}
private final class MenuRow: NSTableRowView {
    override var interiorBackgroundStyle: NSView.BackgroundStyle { .normal }
    override func drawSelection(in dirtyRect: NSRect) {
        menuHighlight.setFill()
        NSBezierPath(roundedRect: bounds, xRadius: 5, yRadius: 5).fill()
    }
}
private final class HistoryTable: NSTableView {
    var hover: ((Int) -> Void)?
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero,
            options: [.mouseMoved, .mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }
    override func mouseMoved(with event: NSEvent) {
        hover?(row(at: convert(event.locationInWindow, from: nil)))
    }
    override func mouseExited(with event: NSEvent) { hover?(-1) }
}
private final class ClipboardMenu: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate, NSWindowDelegate {
    private let callback: Callback
    private let context: UnsafeMutableRawPointer?
    private let panel = Panel(contentRect: NSRect(x: 0, y: 0, width: 380, height: 440),
        styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    private let search = NSSearchField()
    private let table = HistoryTable()
    private let scroll = NSScrollView()
    private let preview = NSImageView()
    private let empty = NSTextField(labelWithString: "No clipboard items")
    private let imageViews = NSHashTable<NSImageView>.weakObjects()
    private var enabled = true
    private var entries: [Entry] = []
    private var pending: [Entry] = []
    private var hover = ImageHover()
    private var timer: DispatchWorkItem?
    private var outside: Any?
    private var local: Any?
    private var menuFrame = NSRect.zero
    private var anchor: NSPoint?
    private var statusFrame = NSRect.zero
    private var awaitingStatusAction = false
    private var selectedID: UInt64? { entries.indices.contains(table.selectedRow) ? entries[table.selectedRow].id : nil }
    private(set) var visible = false

    init(callback: @escaping Callback, context: UnsafeMutableRawPointer?) {
        self.callback = callback
        self.context = context
        super.init()
        panel.isReleasedWhenClosed = false
        panel.level = .popUpMenu
        panel.hasShadow = true
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.delegate = self
        let material = NSVisualEffectView()
        material.material = .menu
        material.blendingMode = .behindWindow
        material.state = .active
        material.wantsLayer = true
        material.layer?.cornerRadius = 10
        material.layer?.masksToBounds = true
        panel.contentView = material
        search.focusRingType = .none
        search.placeholderString = "Search clipboard…"
        search.setAccessibilityLabel("Search clipboard")
        search.delegate = self
        search.sendsSearchStringImmediately = true
        search.sendsWholeSearchString = false
        material.addSubview(search)
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("history"))
        column.width = 350
        table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        table.addTableColumn(column)
        table.headerView = nil
        table.backgroundColor = .clear
        table.style = .plain
        table.selectionHighlightStyle = .regular
        table.intercellSpacing = NSSize(width: 0, height: 2)
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.action = #selector(choose)
        table.setAccessibilityLabel("Clipboard")
        table.hover = { [weak self] row in self?.hoverRow(row) }
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.autohidesScrollers = true
        material.addSubview(scroll)
        empty.textColor = .secondaryLabelColor
        empty.font = .systemFont(ofSize: 13)
        material.addSubview(empty)
        preview.imageScaling = .scaleProportionallyUpOrDown
        preview.setAccessibilityLabel("Image preview")
        preview.isHidden = true
        material.addSubview(preview)
        for (index, title) in ["Clear history", "Settings…", "Quick Composer", "Quit vorkspace"].enumerated() {
            let button = MenuButton(title: title, target: self, action: #selector(footer(_:)))
            button.entered = { [weak self] in
                self?.table.deselectAll(nil)
                self?.cancelPreview()
            }
            button.tag = index + 2
            button.isBordered = false
            button.contentTintColor = .labelColor
            button.alignment = .left
            button.font = .menuFont(ofSize: 13)
            button.frame = NSRect(x: 12, y: 8 + (3 - index) * 26, width: 356, height: 26)
            material.addSubview(button)
        }
        let line = NSBox()
        line.boxType = .separator
        line.frame = NSRect(x: 12, y: 116, width: 356, height: 1)
        material.addSubview(line)
    }
    private func send(_ action: UInt32, _ id: UInt64 = 0, _ text: String = "") {
        text.withCString { callback(context, action, id, $0) }
    }
    func begin() { pending.removeAll(keepingCapacity: true) }
    func add(id: UInt64, title: String, isImage: Bool, pixels: UnsafePointer<UInt8>?, width: Int, height: Int) {
        // Only bounded thumbnails cross the bridge; original payloads stay in Rust.
        var image = entries.first(where: { $0.id == id })?.image
        if image == nil, let pixels, width > 0, height > 0, width <= 256, height <= 256 {
            let data = Data(bytes: pixels, count: width * height * 4)
            if let provider = CGDataProvider(data: data as CFData),
               let cg = CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32,
                bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: [.byteOrder32Little, CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue)],
                provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent) {
                image = NSImage(cgImage: cg, size: NSSize(width: width, height: height))
            }
        }
        pending.append(Entry(id: id, title: title, isImage: isImage, image: image))
    }
    func end(message: String, enabled: Bool = true) {
        self.enabled = enabled
        search.isHidden = !enabled
        scroll.isHidden = !enabled
        for view in panel.contentView!.subviews {
            if let button = view as? NSButton, button.tag == 2 { button.isHidden = !enabled }
            if view is NSBox { view.isHidden = !enabled }
        }
        let selected = selectedID
        // A thumbnail arriving for the same rows preserves the hover deadline.
        // Filtering, reordering, eviction and disable invalidate its identity.
        if !enabled || entries.count != pending.count || !zip(entries, pending).allSatisfy({ $0.id == $1.id }) {
            cancelPreview()
        }
        imageViews.allObjects.forEach { $0.image = nil }
        entries = pending
        pending.removeAll(keepingCapacity: true)
        table.reloadData()
        if !entries.isEmpty {
            table.selectRowIndexes(IndexSet(integer: selected.flatMap { id in entries.firstIndex { $0.id == id } } ?? 0), byExtendingSelection: false)
        }
        empty.stringValue = message.isEmpty ? "No clipboard items" : message
        empty.isHidden = !enabled || !entries.isEmpty
        if visible {
            let top = menuFrame.maxY
            menuFrame.size.height = contentHeight
            menuFrame.size.width = preferredWidth
            menuFrame.origin.y = top - menuFrame.height
            placeMenu()
        }
        layout()
        let time = ProcessInfo.processInfo.systemUptime
        if let id = hover.ready(at: time) { showPreview(id, at: time) }
    }
    func toggle(x: Double, y: Double, statusWidth: Double, statusHeight: Double, statusClick: Bool) {
        // Exit the GPUI/AppKit callback before claiming the key window.
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            // Revalidate after queueing: a click may dismiss after Rust's preflight.
            guard acceptToggle(statusClick: statusClick) else { return }
            if visible { hide(); return }
            visible = true
            search.stringValue = ""
            statusFrame = NSRect(x: x, y: y, width: statusWidth, height: statusHeight)
            anchor = NSPoint(x: x, y: y)
            placeMenu()
            layout()
            panel.setFrame(menuFrame, display: true)
            panel.makeKeyAndOrderFront(nil)
            panel.makeFirstResponder(enabled ? search : nil)
            outside = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] event in self?.outsideClick(event) }
            local = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown, .keyDown]) { [weak self] event in
                guard let self else { return event }
                return localEvent(event)
            }
        }
    }
    private func localEvent(_ event: NSEvent) -> NSEvent? {
        guard visible else { return event }
        if event.type == .keyDown, event.keyCode == 53 { hide(); return nil }
        if event.type != .keyDown && event.window !== panel { outsideClick(event) }
        return event
    }
    private func outsideClick(_ event: NSEvent) {
        guard visible else { return }
        let point = event.window?.convertPoint(toScreen: event.locationInWindow) ?? event.locationInWindow
        let statusClick = event.type == .leftMouseDown && statusFrame.contains(point)
        hide()
        // macOS observes the click globally before forwarding it to the button.
        // Its mouse-down action completes this dismissal, rather than reopening.
        if statusClick { awaitingStatusAction = true }
    }
    func acceptToggle(statusClick: Bool) -> Bool {
        if statusClick && awaitingStatusAction {
            awaitingStatusAction = false
            return false
        }
        return true
    }
    func hide() {
        guard visible else { return }
        visible = false
        cancelPreview()
        panel.orderOut(nil)
        panel.contentView!.subviews.compactMap { $0 as? MenuButton }.forEach { $0.resetHover() }
        if let outside { NSEvent.removeMonitor(outside) }
        if let local { NSEvent.removeMonitor(local) }
        outside = nil; local = nil
        imageViews.allObjects.forEach { $0.image = nil }
        entries.removeAll(); pending.removeAll()
        table.reloadData()
        search.stringValue = ""
        send(7)
    }
    func windowDidResignKey(_ notification: Notification) { hide() }
    @objc private func choose() {
        guard let id = selectedID else { return }
        hide()
        send(1, id)
    }
    @objc private func footer(_ button: NSButton) {
        let action = UInt32(button.tag)
        if action != 2 { hide() }
        send(action)
    }
    func controlTextDidChange(_ notification: Notification) {
        let query = String(search.stringValue.prefix(1024))
        if query != search.stringValue { search.stringValue = query }
        cancelPreview()
        send(6, 0, query)
    }
    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.moveDown(_:)), #selector(NSResponder.moveUp(_:)):
            guard !entries.isEmpty else { return true }
            let delta = selector == #selector(NSResponder.moveDown(_:)) ? 1 : -1
            let row = min(max(table.selectedRow + delta, 0), entries.count - 1)
            table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
            table.scrollRowToVisible(row)
            cancelPreview()
            return true
        case #selector(NSResponder.insertNewline(_:)): choose(); return true
        default: return false
        }
    }
    func numberOfRows(in tableView: NSTableView) -> Int { entries.count }
    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat { entries[row].isImage ? 60 : 28 }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let entry = entries[row]
        let cell = NSTableCellView(frame: NSRect(x: 0, y: 0, width: table.bounds.width, height: entry.isImage ? 60 : 28))
        if entry.isImage, let image = entry.image {
            let view = NSImageView(frame: NSRect(x: 8, y: 3, width: min(100, image.size.width * 54 / image.size.height), height: 54))
            view.image = image
            imageViews.add(view)
            view.imageScaling = .scaleProportionallyUpOrDown
            cell.addSubview(view)
        } else {
            let label = NSTextField(labelWithString: entry.title)
            label.font = .menuFont(ofSize: 13)
            label.lineBreakMode = .byTruncatingTail
            label.frame = NSRect(x: 8, y: entry.isImage ? 21 : 5, width: max(0, cell.bounds.width - 16), height: 18)
            label.autoresizingMask = [.width]
            cell.addSubview(label)
            cell.textField = label
        }
        cell.setAccessibilityLabel(entry.title)
        return cell
    }
    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        let view = MenuRow()
        view.wantsLayer = true
        view.layer?.cornerRadius = 5
        view.layer?.masksToBounds = true
        return view
    }
    private func hoverRow(_ row: Int) {
        if entries.indices.contains(row) {
            table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        }
        let id = entries.indices.contains(row) && entries[row].isImage ? entries[row].id : nil
        guard id != hover.item else { return }
        cancelPreview()
        hover.enter(id, at: ProcessInfo.processInfo.systemUptime)
        guard let id else { return }
        let work = DispatchWorkItem { [weak self] in
            self?.showPreview(id, at: ProcessInfo.processInfo.systemUptime)
        }
        timer = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 1, execute: work)
    }
    private func showPreview(_ id: UInt64, at time: TimeInterval) {
        guard visible, hover.ready(at: time) == id,
            let entry = entries.first(where: { $0.id == id }), let image = entry.image else { return }
        preview.image = image
        preview.isHidden = false
        layout()
    }
    private func cancelPreview() {
        timer?.cancel(); timer = nil
        hover.cancel()
        preview.image = nil
        preview.isHidden = true
        if visible { layout() }
    }
    private var preferredWidth: CGFloat { enabled ? 380 : 220 }
    private func placeMenu() {
        guard let anchor else { return }
        let screen = NSScreen.screens.first { $0.frame.contains(anchor) } ?? NSScreen.main!
        menuFrame = clipboardMenuFrame(anchor: anchor, screen: screen.visibleFrame, enabled: enabled, height: contentHeight)
    }
    private var contentHeight: CGFloat {
        // Native menus fit their contents; long histories scroll in a bounded list.
        enabled ? min(520, max(72, entries.reduce(CGFloat(16)) { $0 + ($1.isImage ? 62 : 30) }) + 172) : 94
    }
    private func layout() {
        // Expand to the left, preserving every menu row's screen position.
        let extra: CGFloat = preview.isHidden ? 0 : 276
        var frame = menuFrame
        frame.origin.x -= extra
        frame.size.width += extra
        let left = NSScreen.screens.first(where: { $0.frame.intersects(menuFrame) })?.visibleFrame.minX ?? 0
        let offset = frame.minX >= left ? extra : 0
        if offset == 0 { frame.origin.x = menuFrame.minX }
        if visible { panel.setFrame(frame, display: true) }
        let width = menuFrame.width > 0 ? menuFrame.width : preferredWidth
        search.frame = NSRect(x: offset + 12, y: menuFrame.height - 40, width: max(0, width - 24), height: 28)
        scroll.frame = NSRect(x: offset + 6, y: 124, width: max(0, width - 12), height: max(0, menuFrame.height - 172))
        table.setFrameSize(NSSize(width: scroll.contentSize.width, height: table.frame.height))
        table.tableColumns.first?.width = scroll.contentSize.width
        empty.frame = NSRect(x: offset + 20, y: menuFrame.height - 98, width: max(0, width - 40), height: 44)
        preview.frame = NSRect(x: offset == 0 ? width + 12 : 12, y: 12, width: 252, height: max(0, menuFrame.height - 24))
        for view in panel.contentView!.subviews where view is NSButton || view is NSBox {
            let inset: CGFloat = view is NSButton ? 6 : 12
            var rect = view.frame; rect.origin.x = offset + inset; rect.size.width = max(0, width - inset * 2); view.frame = rect
        }
    }
    deinit {
        timer?.cancel()
        if let outside { NSEvent.removeMonitor(outside) }
        if let local { NSEvent.removeMonitor(local) }
        panel.orderOut(nil)
    }
}

@_cdecl("vorkspace_clipboard_create")
func create(_ callback: @escaping @convention(c) (UnsafeMutableRawPointer?, UInt32, UInt64, UnsafePointer<CChar>?) -> Void, _ context: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer {
    Unmanaged.passRetained(ClipboardMenu(callback: callback, context: context)).toOpaque()
}
@_cdecl("vorkspace_clipboard_destroy")
func destroy(_ pointer: UnsafeMutableRawPointer) { Unmanaged<ClipboardMenu>.fromOpaque(pointer).release() }
@_cdecl("vorkspace_clipboard_visible")
func visible(_ pointer: UnsafeMutableRawPointer) -> Bool { Unmanaged<ClipboardMenu>.fromOpaque(pointer).takeUnretainedValue().visible }
@_cdecl("vorkspace_clipboard_accept_toggle")
func acceptToggle(_ pointer: UnsafeMutableRawPointer, _ statusClick: Bool) -> Bool { Unmanaged<ClipboardMenu>.fromOpaque(pointer).takeUnretainedValue().acceptToggle(statusClick: statusClick) }
@_cdecl("vorkspace_clipboard_toggle")
func toggle(_ pointer: UnsafeMutableRawPointer, _ x: Double, _ y: Double, _ width: Double, _ height: Double, _ statusClick: Bool) { Unmanaged<ClipboardMenu>.fromOpaque(pointer).takeUnretainedValue().toggle(x: x, y: y, statusWidth: width, statusHeight: height, statusClick: statusClick) }
@_cdecl("vorkspace_clipboard_hide")
func hide(_ pointer: UnsafeMutableRawPointer) { Unmanaged<ClipboardMenu>.fromOpaque(pointer).takeUnretainedValue().hide() }
@_cdecl("vorkspace_clipboard_begin")
func begin(_ pointer: UnsafeMutableRawPointer) { Unmanaged<ClipboardMenu>.fromOpaque(pointer).takeUnretainedValue().begin() }
@_cdecl("vorkspace_clipboard_add")
func add(_ pointer: UnsafeMutableRawPointer, _ id: UInt64, _ title: UnsafePointer<CChar>, _ isImage: Bool, _ pixels: UnsafePointer<UInt8>?, _ width: Int, _ height: Int) {
    Unmanaged<ClipboardMenu>.fromOpaque(pointer).takeUnretainedValue().add(id: id, title: String(cString: title), isImage: isImage, pixels: pixels, width: width, height: height)
}
@_cdecl("vorkspace_clipboard_end")
func end(_ pointer: UnsafeMutableRawPointer, _ message: UnsafePointer<CChar>, _ enabled: Bool) { Unmanaged<ClipboardMenu>.fromOpaque(pointer).takeUnretainedValue().end(message: String(cString: message), enabled: enabled) }
