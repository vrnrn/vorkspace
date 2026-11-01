use objc2_app_kit::NSView;

/// Rebuild the cursor rectangles belonging to a native child's parent view.
///
/// GPUIView excludes direct native children from its cursor rectangles, so
/// moving, hiding, or showing one of those children changes the parent's
/// cursor region. Use the child hierarchy to target its owning NSWindow.
pub(crate) fn invalidate_parent_cursor_rects(child: &NSView) {
    let Some(parent) = (unsafe { child.superview() }) else {
        return;
    };
    if let Some(window) = parent.window() {
        window.invalidateCursorRectsForView(&parent);
    }
}
