//! Opt in with VORKSPACE_NATIVE_CURSOR_PROBE=1. Opens only disposable fixture windows.
//! Native events are delivered locally through AppKit; no global input permissions,
//! screen capture, installed application, user data, or providers are involved.
#[cfg(target_os = "macos")]
mod probe {
    use gpui::{prelude::*, *};
    use objc2::{class, msg_send, runtime::AnyObject};
    use objc2_foundation::{NSPoint, NSSize};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::time::Duration;

    type Id = *mut AnyObject;

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn SetThemeCursor(cursor: u32) -> i32;
    }

    struct Surface {
        style: CursorStyle,
    }
    impl Render for Surface {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .child(div().w(px(120.)).h_full().cursor(self.style))
                .child(div().w(px(120.)).h_full().cursor(CursorStyle::PointingHand))
        }
    }

    fn handles(w: &Window) -> (Id, Id) {
        let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(w).unwrap().as_raw()
        else {
            panic!("AppKit window required")
        };
        let view = handle.ns_view.as_ptr().cast::<AnyObject>();
        unsafe { (msg_send![view, window], view) }
    }

    unsafe fn cursor(style: CursorStyle) -> Id {
        unsafe {
            match style {
                CursorStyle::Arrow => msg_send![class!(NSCursor), arrowCursor],
                CursorStyle::IBeam => msg_send![class!(NSCursor), IBeamCursor],
                CursorStyle::PointingHand => msg_send![class!(NSCursor), pointingHandCursor],
                CursorStyle::Crosshair => msg_send![class!(NSCursor), crosshairCursor],
                CursorStyle::ClosedHand => msg_send![class!(NSCursor), closedHandCursor],
                CursorStyle::OpenHand => msg_send![class!(NSCursor), openHandCursor],
                CursorStyle::ResizeLeftRight => msg_send![class!(NSCursor), resizeLeftRightCursor],
                CursorStyle::ResizeUpDown => msg_send![class!(NSCursor), resizeUpDownCursor],
                CursorStyle::ResizeLeft => msg_send![class!(NSCursor), resizeLeftCursor],
                CursorStyle::ResizeRight => msg_send![class!(NSCursor), resizeRightCursor],
                CursorStyle::ResizeColumn => msg_send![class!(NSCursor), resizeLeftRightCursor],
                CursorStyle::ResizeRow => msg_send![class!(NSCursor), resizeUpDownCursor],
                CursorStyle::ResizeUp => msg_send![class!(NSCursor), resizeUpCursor],
                CursorStyle::ResizeDown => msg_send![class!(NSCursor), resizeDownCursor],
                CursorStyle::ResizeUpLeftDownRight => {
                    msg_send![class!(NSCursor), _windowResizeNorthWestSouthEastCursor]
                }
                CursorStyle::ResizeUpRightDownLeft => {
                    msg_send![class!(NSCursor), _windowResizeNorthEastSouthWestCursor]
                }
                CursorStyle::IBeamCursorForVerticalLayout => {
                    msg_send![class!(NSCursor), IBeamCursorForVerticalLayout]
                }
                CursorStyle::OperationNotAllowed => {
                    msg_send![class!(NSCursor), operationNotAllowedCursor]
                }
                CursorStyle::DragLink => msg_send![class!(NSCursor), dragLinkCursor],
                CursorStyle::DragCopy => msg_send![class!(NSCursor), dragCopyCursor],
                CursorStyle::ContextualMenu => msg_send![class!(NSCursor), contextualMenuCursor],
            }
        }
    }

    fn check(style: CursorStyle, transition: &str) {
        unsafe {
            let current: Id = msg_send![class!(NSCursor), currentCursor];
            let expected = cursor(style);
            if current != expected {
                eprintln!(
                    "FAIL {transition}: expected {style:?} ({expected:p}), actual {current:p}"
                );
                // Avoid unwinding through AppKit's application callback.
                std::process::exit(1);
            }
            println!("PASS {transition}: {style:?} ({current:p})");
        }
    }

    fn check_displayed(style: CursorStyle, transition: &str) {
        unsafe {
            // currentCursor only reports this application's AppKit stack. A
            // native reset can change the displayed cursor without changing it.
            let displayed: Id = msg_send![class!(NSCursor), currentSystemCursor];
            if displayed.is_null() {
                eprintln!("FAIL {transition}: system cursor is unavailable");
                std::process::exit(1);
            }
            let expected = cursor(style);
            let actual_hot: NSPoint = msg_send![displayed, hotSpot];
            let expected_hot: NSPoint = msg_send![expected, hotSpot];
            let actual_image: Id = msg_send![displayed, image];
            let expected_image: Id = msg_send![expected, image];
            let actual_size: NSSize = msg_send![actual_image, size];
            let expected_size: NSSize = msg_send![expected_image, size];
            // Arrow, I-beam, and pointing hand have distinct hotspots. Comparing
            // the image extent as well verifies the displayed cursor signature.
            if actual_hot != expected_hot || actual_size != expected_size {
                eprintln!(
                    "FAIL {transition}: expected displayed {style:?} hot={expected_hot:?} size={expected_size:?}, actual hot={actual_hot:?} size={actual_size:?}"
                );
                std::process::exit(1);
            }
            println!(
                "PASS {transition}: displayed {style:?} hot={actual_hot:?} size={actual_size:?}"
            );
        }
    }

    fn reset_displayed_to_arrow() {
        let status = unsafe { SetThemeCursor(0) }; // kThemeArrowCursor
        if status != 0 {
            eprintln!("FAIL native system reset: OSStatus {status}");
            std::process::exit(1);
        }
    }

    unsafe fn mouse_move(native: Id, x: f64) {
        unsafe { mouse_event(native, x, false) }
    }

    unsafe fn mouse_event(native: Id, x: f64, queued: bool) {
        unsafe {
            let number: isize = msg_send![native, windowNumber];
            let view: Id = msg_send![native, contentView];
            let bounds: objc2_foundation::NSRect = msg_send![view, bounds];
            // Keep the real and locally generated event coordinates in agreement.
            // Move only this disposable fixture under the stationary system pointer;
            // no global mouse injection or Accessibility permission is needed.
            let pointer: NSPoint = msg_send![class!(NSEvent), mouseLocation];
            let _: () = msg_send![native, setFrameOrigin: NSPoint::new(pointer.x - x, pointer.y - (bounds.size.height - 80.))];
            let event: Id = msg_send![class!(NSEvent),
                mouseEventWithType: 5usize,
                location: NSPoint::new(x, bounds.size.height - 80.),
                modifierFlags: 0usize, timestamp: 0f64, windowNumber: number,
                context: std::ptr::null::<AnyObject>(), eventNumber: 0isize,
                clickCount: 0isize, pressure: 0f32];
            if queued {
                let app: Id = msg_send![class!(NSApplication), sharedApplication];
                let _: () = msg_send![app, postEvent: event, atStart: false];
            } else {
                let _: () = msg_send![native, sendEvent: event];
            }
        }
    }

    pub fn run() {
        gpui::Application::with_platform(std::rc::Rc::new(gpui_macos::MacPlatform::new(false))).run(|cx| {
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(200.), px(200.)),
                            size(px(360.), px(240.)),
                        ))),
                        // Match the shipped application's native chrome, not only
                        // the simpler opaque, undecorated fixture window.
                        show: false,
                        window_background: WindowBackgroundAppearance::Blurred,
                        app_owns_titlebar_drag: true,
                        titlebar: Some(TitlebarOptions {
                            title: Some("Vorkspace cursor fixture".into()),
                            appears_transparent: true,
                            traffic_light_position: Some(point(px(12.), px(13.))),
                        }),
                        ..Default::default()
                    },
                    |window, cx| {
                        window.activate_window();
                        cx.new(|_| Surface {
                            style: CursorStyle::IBeam,
                        })
                    },
                )
                .unwrap();
            let (native, view) = cx
                .update_window(window.into(), |_, w, _| handles(w))
                .unwrap();
            unsafe {
                let pointer: NSPoint = msg_send![class!(NSEvent), mouseLocation];
                let _: () =
                    msg_send![native, setFrameOrigin: NSPoint::new(pointer.x-180., pointer.y-120.)];
            }
            cx.activate(true);
            cx.spawn(async move |cx| {
                // Allow AppKit to activate/lay out the owned window, not to resolve assertions.
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                cx.update_window(window.into(), |_, w, cx| {
                    let _ = w.draw(cx);
                })
                .unwrap();
                unsafe {
                    let areas: Id = msg_send![view, trackingAreas];
                    let count: usize = msg_send![areas, count];
                    let registered = (0..count).any(|index| {
                        let area: Id = msg_send![areas, objectAtIndex: index];
                        let options: usize = msg_send![area, options];
                        let owner: Id = msg_send![area, owner];
                        options == (0x04 | 0x20 | 0x200) && owner == view
                    });
                    if !registered {
                        eprintln!("FAIL key-window cursor-update tracking area missing");
                        std::process::exit(1);
                    }
                    println!("PASS AppKit cursor-update tracking area registered");
                    mouse_move(native, 300.);
                }
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                check(CursorStyle::Arrow, "initial background");
                // Exercise NSApplication's normal event queue as well as the
                // direct NSWindow responder calls below.
                for (x, expected) in [
                    (60., CursorStyle::IBeam),
                    (180., CursorStyle::PointingHand),
                    (300., CursorStyle::Arrow),
                ] {
                    unsafe { mouse_event(native, x, true); }
                    cx.background_executor().timer(Duration::from_millis(50)).await;
                    check(expected, "queued application mouse transition");
                }
                for (x, expected) in [
                    (60., CursorStyle::IBeam),
                    (180., CursorStyle::PointingHand),
                    (60., CursorStyle::IBeam),
                    (300., CursorStyle::Arrow),
                    (180., CursorStyle::PointingHand),
                    (300., CursorStyle::Arrow),
                ] {
                    unsafe {
                        mouse_move(native, x);
                    }
                    // Deliberately no run-loop turn: invalidating cursor rects alone is too late.
                    check(expected, "synchronous native mouse transition");
                    cx.background_executor()
                        .timer(Duration::from_millis(20))
                        .await;
                    check(expected, "after AppKit tracking updates");
                }
                unsafe {
                    mouse_move(native, 60.);
                }
                check(CursorStyle::IBeam, "text entry");
                // AppKit can replace the process cursor during menu/window transitions.
                // Invoke the real registered responder callback to test same-style restoration.
                unsafe {
                    let _: () = msg_send![cursor(CursorStyle::Arrow), set];
                    let _: () = msg_send![view, cursorUpdate: std::ptr::null::<AnyObject>()];
                }
                check(CursorStyle::IBeam, "same-style cursorUpdate restoration");
                for (x, style) in [
                    (60., CursorStyle::IBeam),
                    (180., CursorStyle::PointingHand),
                ] {
                    unsafe { mouse_move(native, x); }
                    check_displayed(style, "before native system reset");
                    // Carbon changes the system cursor without updating the
                    // NSCursor stack, reproducing the observed live mismatch.
                    reset_displayed_to_arrow();
                    check(style, "AppKit style survives native system reset");
                    check_displayed(CursorStyle::Arrow, "native system reset");
                    cx.update_window(window.into(), |_, w, cx| {
                        let _ = w.draw(cx);
                    }).unwrap();
                    check_displayed(style, "unchanged repaint restores displayed cursor");
                    reset_displayed_to_arrow();
                    check(style, "same-style mouse transition starts with unchanged AppKit state");
                    check_displayed(CursorStyle::Arrow, "system reset before same-style mouse move");
                    unsafe { mouse_move(native, x); }
                    check_displayed(style, "same-style mouse move restores displayed cursor");
                }
                // The style sweep below changes the left text region; leave the
                // physical pointer there after testing the fixed right-hand link.
                unsafe { mouse_move(native, 60.); }
                let styles = [
                    CursorStyle::PointingHand,
                    CursorStyle::Arrow,
                    CursorStyle::IBeam,
                    CursorStyle::Crosshair,
                    CursorStyle::OpenHand,
                    CursorStyle::ClosedHand,
                    CursorStyle::ResizeLeftRight,
                    CursorStyle::ResizeUpDown,
                    CursorStyle::ResizeLeft,
                    CursorStyle::ResizeRight,
                    CursorStyle::ResizeColumn,
                    CursorStyle::ResizeRow,
                    CursorStyle::ResizeUp,
                    CursorStyle::ResizeDown,
                    CursorStyle::ResizeUpLeftDownRight,
                    CursorStyle::ResizeUpRightDownLeft,
                    CursorStyle::IBeamCursorForVerticalLayout,
                    CursorStyle::OperationNotAllowed,
                    CursorStyle::DragLink,
                    CursorStyle::DragCopy,
                    CursorStyle::ContextualMenu,
                ];
                for style in styles {
                    window
                        .update(cx, |surface, _, cx| {
                            surface.style = style;
                            cx.notify();
                        })
                        .unwrap();
                    cx.update_window(window.into(), |_, w, cx| {
                        let _ = w.draw(cx);
                    })
                    .unwrap();
                    check(style, "stationary content replacement");
                    cx.update_window(window.into(), |_, w, cx| {
                        let _ = w.draw(cx);
                    })
                    .unwrap();
                    check(style, "unchanged repaint");
                }
                // A separate native child view owns its own cursor. Background GPUI
                // repaints and cursorUpdate callbacks must not overwrite it.
                unsafe {
                    let child: Id = msg_send![class!(NSView), alloc];
                    let bounds: objc2_foundation::NSRect = msg_send![view, bounds];
                    let child: Id = msg_send![child, initWithFrame: bounds];
                    let _: () = msg_send![view, addSubview: child];
                    let _: () = msg_send![cursor(CursorStyle::Crosshair), set];
                    window
                        .update(cx, |surface, _, cx| {
                            surface.style = CursorStyle::IBeam;
                            cx.notify();
                        })
                        .unwrap();
                    cx.update_window(window.into(), |_, w, cx| {
                        let _ = w.draw(cx);
                    })
                    .unwrap();
                    let _: () = msg_send![view, cursorUpdate: std::ptr::null::<AnyObject>()];
                    check(CursorStyle::Crosshair, "native child ownership");
                    let _: () = msg_send![child, removeFromSuperview];
                    let _: () = msg_send![child, release];
                    let _: () = msg_send![view, cursorUpdate: std::ptr::null::<AnyObject>()];
                }
                check(CursorStyle::IBeam, "return from native child");
                let overlay = unsafe {
                    let rect: objc2_foundation::NSRect = msg_send![native, frame];
                    let overlay: Id = msg_send![class!(NSWindow), alloc];
                    let overlay: Id = msg_send![overlay, initWithContentRect: rect,
                        styleMask: 1usize, backing: 2usize, defer: false];
                    let _: () = msg_send![overlay, setReleasedWhenClosed: false];
                    let _: () = msg_send![overlay, orderFrontRegardless];
                    let _: () = msg_send![cursor(CursorStyle::Crosshair), set];
                    overlay
                };
                // AppKit must map the new native window before testing ownership.
                for _ in 0..100 {
                    let mapped = unsafe {
                        let pointer: NSPoint = msg_send![class!(NSEvent), mouseLocation];
                        let top: isize = msg_send![class!(NSWindow), windowNumberAtPoint: pointer, belowWindowWithWindowNumber: 0isize];
                        let number: isize = msg_send![overlay, windowNumber];
                        top == number
                    };
                    if mapped { break; }
                    cx.background_executor().timer(Duration::from_millis(10)).await;
                }
                unsafe { let _: () = msg_send![cursor(CursorStyle::Crosshair), set]; }
                window
                    .update(cx, |surface, _, cx| {
                        surface.style = CursorStyle::Arrow;
                        cx.notify();
                    })
                    .unwrap();
                cx.update_window(window.into(), |_, w, cx| {
                    let _ = w.draw(cx);
                })
                .unwrap();
                unsafe {
                    let _: () = msg_send![view, cursorUpdate: std::ptr::null::<AnyObject>()];
                }
                check(CursorStyle::Crosshair, "covering native window ownership");
                unsafe {
                    let _: () =
                        msg_send![overlay, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                unsafe {
                    let _: () = msg_send![cursor(CursorStyle::Crosshair), set];
                }
                window
                    .update(cx, |surface, _, cx| {
                        surface.style = CursorStyle::IBeam;
                        cx.notify();
                    })
                    .unwrap();
                cx.update_window(window.into(), |_, w, cx| {
                    let _ = w.draw(cx);
                })
                .unwrap();
                unsafe {
                    let _: () = msg_send![view, cursorUpdate: std::ptr::null::<AnyObject>()];
                }
                check(
                    CursorStyle::Crosshair,
                    "inactive GPUI window cannot take cursor",
                );
                unsafe {
                    let _: () = msg_send![overlay, close];
                    let _: () = msg_send![overlay, release];
                    let _: () =
                        msg_send![native, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                cx.update_window(window.into(), |_, w, cx| {
                    let _ = w.draw(cx);
                })
                .unwrap();
                unsafe {
                    let _: () = msg_send![view, cursorUpdate: std::ptr::null::<AnyObject>()];
                }
                // Window activation may deliver a real movement at the physical
                // pointer. Establish the test's local text position again.
                unsafe { mouse_move(native, 60.); }
                check(CursorStyle::IBeam, "return to GPUI key window");
                cx.update_window(window.into(), |_, w, cx| {
                    w.set_ui_scale(1.5, cx);
                    let _ = w.draw(cx);
                })
                .unwrap();
                unsafe {
                    mouse_move(native, 60.);
                }
                check(CursorStyle::IBeam, "scaled native coordinates");
                unsafe {
                    mouse_move(native, 330.);
                }
                check(CursorStyle::PointingHand, "scaled link coordinates");
                unsafe {
                    mouse_move(native, 355.);
                }
                // x=355 remains within the scaled link (logical x~237).
                check(CursorStyle::PointingHand, "scaled same-view movement");
                let expected = cx
                    .update_window(window.into(), |_, w, cx| {
                        w.set_ui_scale(1., cx);
                        let _ = w.draw(cx);
                        // Zoom resamples the physical pointer; local NSEvents do not move it.
                        if w.mouse_position().x < px(120.) {
                            CursorStyle::IBeam
                        } else if w.mouse_position().x < px(240.) {
                            CursorStyle::PointingHand
                        } else {
                            CursorStyle::Arrow
                        }
                    })
                    .unwrap();
                check(
                    expected,
                    "stationary scale invalidation resamples native pointer",
                );
                println!("Native cursor probe passed");
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }
}
fn main() {
    #[cfg(target_os = "macos")]
    if std::env::var_os("VORKSPACE_NATIVE_CURSOR_PROBE").is_some() {
        probe::run();
        return;
    }
    println!("native cursor probe skipped (set VORKSPACE_NATIVE_CURSOR_PROBE=1 on macOS)");
}
