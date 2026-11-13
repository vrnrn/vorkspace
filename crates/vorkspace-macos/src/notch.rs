//! AppKit placement uses display points, independently of workspace zoom.
use gpui::Window;
use objc2::{MainThreadMarker, msg_send, runtime::NSObjectProtocol, sel};
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_app_kit::{
    NSColor, NSScreen, NSStatusWindowLevel, NSView, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSString};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

pub const WING_WIDTH: f32 = 54.;

#[derive(Clone, Default)]
pub struct Placement {
    live: Arc<AtomicBool>,
    revision: Arc<AtomicU64>,
}
impl Placement {
    pub fn open(&self) {
        self.live.store(true, Ordering::Relaxed);
    }
    pub fn close(&self) {
        self.live.store(false, Ordering::Relaxed);
    }
}

pub struct ScreenChanges {
    center: Retained<NSNotificationCenter>,
    observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    signal: async_channel::Sender<()>,
}

impl ScreenChanges {
    pub fn new() -> (Self, async_channel::Receiver<()>) {
        let (signal, receiver) = async_channel::bounded(1);
        let sender = signal.clone();
        let block = block2::RcBlock::new(move |_: std::ptr::NonNull<NSNotification>| {
            let _ = sender.try_send(());
        });
        let center = NSNotificationCenter::defaultCenter();
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(&NSString::from_str(
                    "NSApplicationDidChangeScreenParametersNotification",
                )),
                None,
                None,
                &block,
            )
        };
        (
            Self {
                center,
                observer,
                signal,
            },
            receiver,
        )
    }
}

impl Drop for ScreenChanges {
    fn drop(&mut self) {
        self.signal.close();
        unsafe {
            self.center.removeObserver(self.observer.as_ref());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub x: f64,
    pub top: f64,
    pub screen_width: f32,
    pub notch_width: f32,
    pub notch_height: f32,
    /// Gap between the two AppKit auxiliary areas, in global screen points.
    /// Vox's padded, centered width is deliberately kept separate.
    pub notch_edges: Option<(f64, f64)>,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            x: 0.,
            top: 900.,
            screen_width: 1440.,
            notch_width: 185.,
            notch_height: 32.,
            notch_edges: None,
        }
    }
}

impl Geometry {
    pub fn read() -> Self {
        let Some(mtm) = MainThreadMarker::new() else {
            return Self::default();
        };
        let Some(screen) = NSScreen::mainScreen(mtm) else {
            return Self::default();
        };
        let frame = screen.frame();
        let has_safe_area = screen.respondsToSelector(sel!(safeAreaInsets));
        let height = if has_safe_area {
            screen.safeAreaInsets().top as f32
        } else {
            0.
        };
        let (width, notch_edges) = if height > 0. {
            let left = screen.auxiliaryTopLeftArea();
            let right = screen.auxiliaryTopRightArea();
            let start = left.origin.x + left.size.width;
            let end = right.origin.x;
            let screen_end = frame.origin.x + frame.size.width;
            let edges = (left.size.width > 0.
                && right.size.width > 0.
                && start.is_finite()
                && end.is_finite()
                && start >= frame.origin.x
                && end <= screen_end
                && start < end)
                .then_some((start, end));
            (frame.size.width - left.size.width - right.size.width, edges)
        } else {
            (0., None)
        };
        Self {
            x: frame.origin.x,
            top: frame.origin.y + frame.size.height,
            screen_width: frame.size.width as f32,
            notch_width: if width > 0. { width as f32 + 4. } else { 120. },
            notch_height: if height > 0. { height } else { 30. },
            notch_edges,
        }
    }

    pub fn compact_width(self) -> f32 {
        (self.notch_width + WING_WIDTH * 2.).min(self.screen_width)
    }

    pub(crate) fn frame(self, width: f32, height: f32) -> NSRect {
        NSRect::new(
            NSPoint::new(
                self.x + (self.screen_width - width) as f64 / 2.,
                self.top - height as f64,
            ),
            NSSize::new(width as f64, height as f64),
        )
    }
}

pub fn place(
    window: &Window,
    geometry: Geometry,
    width: f32,
    height: f32,
    placement: &Placement,
    cx: &gpui::App,
) {
    place_frame(window, geometry.frame(width, height), placement, cx);
}

fn place_frame(window: &Window, frame: NSRect, placement: &Placement, cx: &gpui::App) {
    #[cfg(test)]
    if std::env::var_os("VORKSPACE_VOX_NATIVE_SMOKE").is_none() {
        return;
    }
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
    let Some(native) = view.window() else { return };
    let placement = placement.clone();
    let revision = placement.revision.fetch_add(1, Ordering::Relaxed) + 1;
    // AppKit resize notifications call back into GPUI synchronously. Run after
    // the current App/window borrow, coalescing older frames and closed panels.
    cx.foreground_executor()
        .spawn(async move {
            if !placement.live.load(Ordering::Relaxed)
                || placement.revision.load(Ordering::Relaxed) != revision
            {
                return;
            }
            // GPUI panels are titled, and AppKit keeps titled windows below
            // the menu bar, which would hang this surface under the notch.
            let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
            if native.styleMask() != style {
                native.setStyleMask(style);
            }
            native.setCanHide(false);
            native.setHidesOnDeactivate(false);
            native.setHasShadow(false);
            native.setOpaque(false);
            native.setBackgroundColor(Some(&NSColor::clearColor()));
            native.setLevel(NSStatusWindowLevel + 1);
            native.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
                    | NSWindowCollectionBehavior::Stationary
                    | NSWindowCollectionBehavior::IgnoresCycle,
            );
            if native.respondsToSelector(sel!(setBecomesKeyOnlyIfNeeded:)) {
                let _: () = unsafe { msg_send![&native, setBecomesKeyOnlyIfNeeded: true] };
            }
            if native.frame() != frame {
                native.setFrame_display(frame, false);
            }
            if !native.isVisible() {
                native.orderFrontRegardless();
            }
        })
        .detach();
}

#[cfg(debug_assertions)]
pub fn snapshot(window: &Window) -> serde_json::Value {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return serde_json::Value::Null;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return serde_json::Value::Null;
    };
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
    let Some(native) = view.window() else {
        return serde_json::Value::Null;
    };
    let frame = native.frame();
    serde_json::json!({
        "frame":{"x":frame.origin.x,"y":frame.origin.y,"width":frame.size.width,"height":frame.size.height},
        "top":frame.origin.y + frame.size.height,
        "canHide":native.canHide(),"hidesOnDeactivate":native.hidesOnDeactivate(),
        "isVisible":native.isVisible(),"isKey":native.isKeyWindow(),"level":native.level(),"spaces":native.collectionBehavior().bits(),
    })
}

/// Shared black notch body, including its concave shoulders.
pub fn body() -> impl gpui::IntoElement {
    use gpui::{PathBuilder, canvas, point, prelude::*, px};
    const SHOULDER: f32 = 6.;
    canvas(
        |_, _, _| (),
        |bounds, _, window, _| {
            let x = bounds.left();
            let y = bounds.top();
            let w = bounds.size.width;
            let h = bounds.size.height;
            let top = px(SHOULDER);
            let bottom = px(14.).min(h / 2.);
            let mut shape = PathBuilder::fill();
            shape.move_to(point(x, y));
            shape.line_to(point(x + w, y));
            shape.curve_to(point(x + w - top, y + top), point(x + w - top, y));
            shape.line_to(point(x + w - top, y + h - bottom));
            shape.curve_to(
                point(x + w - top - bottom, y + h),
                point(x + w - top, y + h),
            );
            shape.line_to(point(x + top + bottom, y + h));
            shape.curve_to(point(x + top, y + h - bottom), point(x + top, y + h));
            shape.line_to(point(x + top, y + top));
            shape.curve_to(point(x, y), point(x + top, y));
            shape.close();
            if let Ok(shape) = shape.build() {
                window.paint_path(shape, gpui::black());
            }
        },
    )
    .absolute()
    .size_full()
}

/// A notch never becomes key or activates the workspace on opening.
pub fn window_options(geometry: Geometry) -> gpui::WindowOptions {
    use gpui::*;
    WindowOptions {
        titlebar: None,
        kind: WindowKind::PopUp,
        focus: false,
        show: false,
        inactive_frame_interval: None,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size(px(geometry.notch_width), px(geometry.notch_height)),
        ))),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notch_expansion_stays_centered_and_attached_on_offset_displays() {
        let geometry = Geometry {
            x: -1920.,
            top: 1080.,
            screen_width: 1920.,
            ..Default::default()
        };
        for (width, height) in [(geometry.compact_width(), 32.), (460., 180.), (185., 32.)] {
            let frame = geometry.frame(width, height);
            assert_eq!(frame.origin.x + frame.size.width / 2., -960.);
            assert_eq!(frame.origin.y + frame.size.height, 1080.);
        }
    }
}
