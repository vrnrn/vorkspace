//! src-tauri/src/macos.rs's traffic lights: 14 pt buttons, 6 pt apart, 12 pt
//! from the left edge. GPUI's `traffic_light_position` only moves the row and
//! keeps AppKit's own spacing, which on recent macOS crowds the sidebar
//! toggle. Auto Layout pins the buttons, so GPUI's frame updates on resize,
//! focus and fullscreen don't move them back.

use gpui::Window;
use objc2_app_kit::{NSView, NSWindow, NSWindowButton};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

const BUTTON_SIZE: f64 = 14.;
const LEFT_MARGIN: f64 = 12.;
const BUTTON_SPACING: f64 = 6.;

/// Pins the buttons once; AppKit can recreate them, so call again whenever
/// the window may have rebuilt its titlebar.
pub(crate) fn pin(window: &Window, title_bar_height: f32) {
    let Some(ns_window) = ns_window(window) else {
        return;
    };
    let bottom = (title_bar_height as f64 - BUTTON_SIZE) / 2.;
    let kinds = [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ];
    for (i, kind) in kinds.into_iter().enumerate() {
        let Some(button) = ns_window.standardWindowButton(kind) else {
            continue;
        };
        // Already pinned: a second set of constraints would only conflict.
        if !button.translatesAutoresizingMaskIntoConstraints() {
            continue;
        }
        let Some(titlebar) = (unsafe { button.superview() }) else {
            continue;
        };
        let x = LEFT_MARGIN + i as f64 * (BUTTON_SIZE + BUTTON_SPACING);
        button.setTranslatesAutoresizingMaskIntoConstraints(false);
        // Bottom, like GPUI's own origin: the titlebar view's height varies
        // between windowed and fullscreen, the distance from its bottom does not.
        for constraint in [
            button.widthAnchor().constraintEqualToConstant(BUTTON_SIZE),
            button.heightAnchor().constraintEqualToConstant(BUTTON_SIZE),
            button
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&titlebar.leadingAnchor(), x),
            button
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&titlebar.bottomAnchor(), -bottom),
        ] {
            constraint.setActive(true);
        }
    }
}

fn ns_window(window: &Window) -> Option<objc2::rc::Retained<NSWindow>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    // SAFETY: GPUI's AppKit handle points at its live content view.
    let view = unsafe { &*appkit.ns_view.as_ptr().cast::<NSView>() };
    view.window()
}
pub(crate) fn show(window: &Window) {
    if let Some(window) = ns_window(window) {
        if window.isMiniaturized() {
            window.deminiaturize(None);
        }
        window.makeKeyAndOrderFront(None);
    }
}
/// Return keyboard input to GPUI after interacting with an embedded native view.
pub(crate) fn focus_content(window: &Window) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    let view = unsafe { &*appkit.ns_view.as_ptr().cast::<NSView>() };
    if let Some(window) = view.window() {
        window.makeFirstResponder(Some(view));
    }
}
#[cfg(not(test))]
pub(crate) fn hide(window: &Window) {
    if let Some(window) = ns_window(window) {
        window.orderOut(None);
    }
}
