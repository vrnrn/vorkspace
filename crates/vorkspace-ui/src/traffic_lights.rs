//! Native traffic lights: 14 pt buttons, 6 pt apart, 12 pt
//! from the left edge. GPUI's `traffic_light_position` only moves the row and
//! keeps AppKit's own spacing, which on recent macOS crowds the sidebar
//! toggle. Auto Layout pins the buttons, so GPUI's frame updates on resize,
//! focus and fullscreen don't move them back.

use crate::theme::{
    TITLE_BAR_HEIGHT, TRAFFIC_LIGHT_MARGIN, TRAFFIC_LIGHT_SIZE, TRAFFIC_LIGHT_SPACING,
};
use gpui::Window;
use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSWindow, NSWindowButton};
use objc2_foundation::ns_string;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Reuse native constraints, updating their vertical offset for this frame's
/// UI zoom. AppKit may recreate the buttons during fullscreen transitions.
pub fn pin(window: &Window) {
    if window.is_fullscreen() {
        return;
    }
    let Some(ns_window) = ns_window(window) else {
        return;
    };
    let height = TITLE_BAR_HEIGHT * window.ui_scale();
    if pin_buttons(&ns_window, height as f64) {
        // GPUI owns the native titlebar container's frame as well as its
        // position on resize/focus. Keep its stored AppKit-point offset current;
        // constraints alone cannot center buttons in a stale-height container.
        window.set_traffic_light_position(gpui::point(
            gpui::px(TRAFFIC_LIGHT_MARGIN),
            gpui::px((height - TRAFFIC_LIGHT_SIZE) / 2.),
        ));
    }
}

fn pin_buttons(ns_window: &NSWindow, title_bar_height_points: f64) -> bool {
    let mut changed = false;
    let size = TRAFFIC_LIGHT_SIZE as f64;
    let bottom = (title_bar_height_points - size) / 2.;
    let kinds = [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ];
    for (i, kind) in kinds.into_iter().enumerate() {
        let Some(button) = ns_window.standardWindowButton(kind) else {
            continue;
        };
        let Some(titlebar) = (unsafe { button.superview() }) else {
            continue;
        };
        // The titlebar owns constraints spanning itself and a button. Identify
        // ours by both marker and button identity; autoresizing-mask state alone
        // neither proves ownership nor allows a live zoom to update the offset.
        if let Some(constraint) = titlebar.constraints().iter().find(|constraint| {
            constraint.identifier().as_deref() == Some(ns_string!("vorkspace.traffic-light.bottom"))
                // SAFETY: both items remain alive through the titlebar/button.
                && unsafe { constraint.firstItem() }.is_some_and(|item| {
                    Retained::as_ptr(&item) == Retained::as_ptr(&button).cast()
                })
        }) {
            if constraint.constant() != -bottom {
                constraint.setConstant(-bottom);
                changed = true;
            }
            continue;
        }
        let x = TRAFFIC_LIGHT_MARGIN as f64 + i as f64 * (size + TRAFFIC_LIGHT_SPACING as f64);
        changed = true;
        button.setTranslatesAutoresizingMaskIntoConstraints(false);
        // GPUI also positions buttons from the container's bottom, in AppKit points.
        let vertical = button
            .bottomAnchor()
            .constraintEqualToAnchor_constant(&titlebar.bottomAnchor(), -bottom);
        vertical.setIdentifier(Some(ns_string!("vorkspace.traffic-light.bottom")));
        for constraint in [
            button.widthAnchor().constraintEqualToConstant(size),
            button.heightAnchor().constraintEqualToConstant(size),
            button
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&titlebar.leadingAnchor(), x),
            vertical,
        ] {
            constraint.setActive(true);
        }
    }
    changed
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
pub fn show(window: &Window) {
    if let Some(window) = ns_window(window) {
        if window.isMiniaturized() {
            window.deminiaturize(None);
        }
        window.makeKeyAndOrderFront(None);
    }
}
/// Return keyboard input to GPUI after interacting with an embedded native view.
pub fn focus_content(window: &Window) {
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
pub fn hide(window: &Window) {
    if let Some(window) = ns_window(window) {
        window.orderOut(None);
    }
}
