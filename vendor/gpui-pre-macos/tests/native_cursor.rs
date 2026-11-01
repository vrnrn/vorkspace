//! Opt in with VORKSPACE_NATIVE_CURSOR_PROBE=1, or use
//! VORKSPACE_NATIVE_CURSOR_PHYSICAL_PROBE=1 for the hand-driven 15-second fixture.
//! Native events are delivered locally through AppKit; no global input permissions,
//! screen capture, installed application, user data, or providers are involved.
#[cfg(target_os = "macos")]
mod probe {
    use gpui::{prelude::*, *};
    use objc2::{class, msg_send, runtime::AnyObject};
    use objc2_foundation::{NSPoint, NSSize};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{
        fs::OpenOptions,
        io::Write,
        os::unix::fs::OpenOptionsExt,
        time::{Duration, Instant},
    };

    type Id = *mut AnyObject;

    fn dump_trace() {
        let (events, dropped) = gpui::cursor_diagnostic::snapshot();
        for suffix in 0..100 {
            let path = format!(
                "/tmp/vokky-native-cursor-trace-{}-{suffix}.tsv",
                std::process::id()
            );
            let Ok(mut file) = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            else {
                continue;
            };
            let _ = writeln!(
                file,
                "sequence\tmonotonic_ns\tgpui_window\tnative_window\tframe\tkind\tdetail"
            );
            let retained = events.len();
            let _ = writeln!(file, "# dropped={dropped} retained={retained}");
            for event in events {
                let _ = writeln!(file, "{event}");
            }
            eprintln!("cursor trace: {path} (retained={retained}, dropped={dropped})");
            return;
        }
        eprintln!("could not create a unique cursor trace file in /tmp");
    }

    fn fail(message: &str) -> ! {
        eprintln!("FAIL {message}");
        dump_trace();
        std::process::exit(1);
    }

    fn trace_count(native: Id, kind: &'static str) -> usize {
        let native_window: isize = unsafe { msg_send![native, windowNumber] };
        let (events, _) = gpui::cursor_diagnostic::snapshot();
        events
            .iter()
            .filter(|event| event.native_window == native_window as u64 && event.kind == kind)
            .count()
    }

    fn trace_count_detail(native: Id, kind: &'static str, needle: &str) -> usize {
        let native_window: isize = unsafe { msg_send![native, windowNumber] };
        let (events, _) = gpui::cursor_diagnostic::snapshot();
        events
            .iter()
            .filter(|event| {
                event.native_window == native_window as u64
                    && event.kind == kind
                    && event.detail.contains(needle)
            })
            .count()
    }

    struct Surface {
        style: CursorStyle,
        revision: u64,
        physical_probe: bool,
    }
    impl Render for Surface {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let label = if self.physical_probe {
                format!("TEXT · IBeam · {}", self.revision)
            } else {
                self.revision.to_string()
            };
            let background = div().w(px(120.)).h_full().child(if self.physical_probe {
                "BACKGROUND · Arrow"
            } else {
                ""
            });
            let background = if self.style == CursorStyle::Crosshair {
                background.cursor(CursorStyle::Crosshair)
            } else {
                background
            };
            div()
                .size_full()
                .flex()
                .child(div().w(px(120.)).h_full().cursor(self.style).child(label))
                .child(
                    div()
                        .w(px(120.))
                        .h_full()
                        .cursor(CursorStyle::PointingHand)
                        .child(if self.physical_probe {
                            "LINK · PointingHand"
                        } else {
                            ""
                        }),
                )
                .child(background)
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
                fail(&format!(
                    "{transition}: expected {style:?} ({expected:p}), actual {current:p}"
                ));
            }
            println!("PASS {transition}: {style:?} ({current:p})");
        }
    }

    fn same_signature(left: Id, right: Id) -> bool {
        unsafe {
            let left_hot: NSPoint = msg_send![left, hotSpot];
            let right_hot: NSPoint = msg_send![right, hotSpot];
            let left_image: Id = msg_send![left, image];
            let right_image: Id = msg_send![right, image];
            let left_size: NSSize = msg_send![left_image, size];
            let right_size: NSSize = msg_send![right_image, size];
            left_hot == right_hot && left_size == right_size
        }
    }

    fn displayed_name(displayed: Id) -> &'static str {
        unsafe {
            if same_signature(displayed, cursor(CursorStyle::Arrow)) {
                "Arrow"
            } else if same_signature(displayed, cursor(CursorStyle::IBeam)) {
                "IBeam"
            } else if same_signature(displayed, cursor(CursorStyle::PointingHand)) {
                "PointingHand"
            } else {
                "Other"
            }
        }
    }

    fn check_displayed(native: Id, style: CursorStyle, transition: &str) {
        unsafe {
            // currentCursor only reports this application's AppKit stack. A
            // native reset can change the displayed cursor without changing it.
            let displayed: Id = msg_send![class!(NSCursor), currentSystemCursor];
            if displayed.is_null() {
                fail(&format!("{transition}: system cursor is unavailable"));
            }
            let expected = cursor(style);
            let actual_hot: NSPoint = msg_send![displayed, hotSpot];
            let expected_hot: NSPoint = msg_send![expected, hotSpot];
            let actual_image: Id = msg_send![displayed, image];
            let expected_image: Id = msg_send![expected, image];
            let actual_size: NSSize = msg_send![actual_image, size];
            let expected_size: NSSize = msg_send![expected_image, size];
            let actual_name = displayed_name(displayed);
            let native_window: isize = msg_send![native, windowNumber];
            gpui::cursor_diagnostic::record_native(
                native_window as u64,
                "display.currentSystemCursor.sample",
                format!(
                    "expected={style:?} observed={actual_name} hot=({}, {}) size=({}, {}) phase={transition}",
                    actual_hot.x, actual_hot.y, actual_size.width, actual_size.height
                ),
            );
            // Arrow, I-beam, and pointing hand have distinct hotspots. Comparing
            // the image extent as well verifies the displayed cursor signature.
            if actual_hot != expected_hot || actual_size != expected_size {
                fail(&format!(
                    "{transition}: expected displayed {style:?} hot=({}, {}) size=({}, {}), actual {actual_name} hot=({}, {}) size=({}, {})",
                    expected_hot.x,
                    expected_hot.y,
                    expected_size.width,
                    expected_size.height,
                    actual_hot.x,
                    actual_hot.y,
                    actual_size.width,
                    actual_size.height,
                ));
            }
            println!(
                "PASS {transition}: displayed {style:?} hot=({}, {}) size=({}, {})",
                actual_hot.x, actual_hot.y, actual_size.width, actual_size.height
            );
        }
    }

    unsafe fn invalidate_cursor_rects(native: Id, view: Id) {
        unsafe {
            let native_window: isize = msg_send![native, windowNumber];
            gpui::cursor_diagnostic::record_native(
                native_window as u64,
                "fixture.invalidateCursorRects.begin",
                format!("view={view:p}"),
            );
            let _: () = msg_send![native, invalidateCursorRectsForView: view];
            gpui::cursor_diagnostic::record_native(
                native_window as u64,
                "fixture.invalidateCursorRects.end",
                format!("view={view:p}"),
            );
        }
    }

    fn sample_physical_cursor(native: Id, view: Id) -> (Option<CursorStyle>, bool, String) {
        unsafe {
            let number: isize = msg_send![native, windowNumber];
            let is_key: bool = msg_send![native, isKeyWindow];
            let screen_point: NSPoint = msg_send![class!(NSEvent), mouseLocation];
            let top: isize = msg_send![class!(NSWindow),
                windowNumberAtPoint: screen_point, belowWindowWithWindowNumber: 0isize];
            let window_point: NSPoint = msg_send![native, mouseLocationOutsideOfEventStream];
            let content: Id = msg_send![native, contentView];
            let content_point: NSPoint = msg_send![content,
                convertPoint: window_point, fromView: std::ptr::null::<AnyObject>()];
            let hit: Id = msg_send![content, hitTest: content_point];
            let view_point: NSPoint = msg_send![view,
                convertPoint: window_point, fromView: std::ptr::null::<AnyObject>()];
            let bounds: objc2_foundation::NSRect = msg_send![view, bounds];
            let owns_pointer = is_key && top == number && hit == view;
            let in_view = view_point.x >= bounds.origin.x
                && view_point.y >= bounds.origin.y
                && view_point.x < bounds.origin.x + bounds.size.width
                && view_point.y < bounds.origin.y + bounds.size.height;
            let expected = if !owns_pointer || !in_view {
                None
            } else if view_point.x < bounds.origin.x + 120. {
                Some(CursorStyle::IBeam)
            } else if view_point.x < bounds.origin.x + 240. {
                Some(CursorStyle::PointingHand)
            } else {
                Some(CursorStyle::Arrow)
            };
            let displayed: Id = msg_send![class!(NSCursor), currentSystemCursor];
            let observed = if displayed.is_null() {
                "Unavailable"
            } else {
                displayed_name(displayed)
            };
            let actual_hot = if displayed.is_null() {
                NSPoint::new(0., 0.)
            } else {
                msg_send![displayed, hotSpot]
            };
            let actual_image: Id = if displayed.is_null() {
                std::ptr::null_mut()
            } else {
                msg_send![displayed, image]
            };
            let actual_size: NSSize = if actual_image.is_null() {
                NSSize::new(0., 0.)
            } else {
                msg_send![actual_image, size]
            };
            let matches = expected.is_none_or(|expected| {
                !displayed.is_null() && same_signature(displayed, cursor(expected))
            });
            let detail = format!(
                "expected={expected:?} observed={observed} matches={matches} owned={owns_pointer} key={} top={top} hit={hit:p} view={view:p} local=({}, {}) hot=({}, {}) size=({}, {})",
                is_key,
                view_point.x,
                view_point.y,
                actual_hot.x,
                actual_hot.y,
                actual_size.width,
                actual_size.height,
            );
            (expected, matches, detail)
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
        gpui::cursor_diagnostic::clear();
        let physical_probe = std::env::var_os("VORKSPACE_NATIVE_CURSOR_PHYSICAL_PROBE").is_some();
        gpui::Application::with_platform(std::rc::Rc::new(gpui_macos::MacPlatform::new(false))).run(move |cx| {
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(200.), px(200.)),
                            size(px(360.), px(240.)),
                        ))),
                        // Match the shipped application's native chrome, not only
                        // the simpler opaque, undecorated fixture window.
                        show: physical_probe,
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
                            revision: 0,
                            physical_probe,
                        })
                    },
                )
                .unwrap();
            let (native, view) = cx
                .update_window(window.into(), |_, w, _| handles(w))
                .unwrap();
            if !physical_probe {
                unsafe {
                    let pointer: NSPoint = msg_send![class!(NSEvent), mouseLocation];
                    let _: () = msg_send![native,
                        setFrameOrigin: NSPoint::new(pointer.x-180., pointer.y-120.)];
                }
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
                if physical_probe {
                    println!(
                        "PHYSICAL CURSOR PROBE: move the real pointer through TEXT, LINK, and BACKGROUND for 15 seconds; pause in each area for at least 150 ms. No events are injected."
                    );
                    let native_window: isize = unsafe { msg_send![native, windowNumber] };
                    let mouse_moves_before =
                        trace_count_detail(native, "appkit.event.begin", "NSMouseMoved");
                    let started = Instant::now();
                    let mut text_samples = 0;
                    let mut link_samples = 0;
                    let mut background_samples = 0;
                    let mut boundary_samples = 0;
                    let mut entry_samples = 0;
                    let mut stable_text_samples = 0;
                    let mut stable_link_samples = 0;
                    let mut stable_background_samples = 0;
                    let mut last_region = None;
                    let mut region_since = Instant::now();
                    let mut stable_mismatches = 0;
                    while started.elapsed() < Duration::from_secs(15) {
                        let (region, matches, detail) = sample_physical_cursor(native, view);
                        if region != last_region {
                            last_region = region;
                            region_since = Instant::now();
                        }
                        let region_age = region_since.elapsed();
                        let stable = region.is_some()
                            && region_age >= Duration::from_millis(25);
                        let phase = if region.is_none() {
                            boundary_samples += 1;
                            "boundary"
                        } else if stable {
                            match region {
                                Some(CursorStyle::IBeam) => stable_text_samples += 1,
                                Some(CursorStyle::PointingHand) => stable_link_samples += 1,
                                Some(CursorStyle::Arrow) => stable_background_samples += 1,
                                _ => {}
                            }
                            "stable"
                        } else {
                            entry_samples += 1;
                            "entry"
                        };
                        gpui::cursor_diagnostic::record_native(
                            native_window as u64,
                            "physical.currentSystemCursor.sample",
                            format!(
                                "{detail} phase={phase} region_age_ms={}",
                                region_age.as_millis()
                            ),
                        );
                        // Physical coordinates can advance before the queued
                        // entry event reaches GPUI. Retain every sample in the
                        // trace, and assess stability inside a settled region.
                        if stable && !matches {
                            stable_mismatches += 1;
                        }
                        match region {
                            Some(CursorStyle::IBeam) => text_samples += 1,
                            Some(CursorStyle::PointingHand) => link_samples += 1,
                            Some(CursorStyle::Arrow) => background_samples += 1,
                            _ => {}
                        }
                        cx.background_executor()
                            .timer(Duration::from_millis(8))
                            .await;
                    }
                    let mouse_moves_after =
                        trace_count_detail(native, "appkit.event.begin", "NSMouseMoved");
                    let mouse_moves = mouse_moves_after.saturating_sub(mouse_moves_before);
                    if mouse_moves < 5
                        || text_samples < 10
                        || link_samples < 10
                        || background_samples < 10
                        || stable_text_samples < 5
                        || stable_link_samples < 5
                        || stable_background_samples < 5
                    {
                        fail(&format!(
                            "physical observation incomplete: mouse_moves={mouse_moves} text_samples={text_samples} link_samples={link_samples} background_samples={background_samples} boundary_samples={boundary_samples} entry_samples={entry_samples} stable_text_samples={stable_text_samples} stable_link_samples={stable_link_samples} stable_background_samples={stable_background_samples}"
                        ));
                    }
                    println!(
                        "Physical cursor probe: mouse_moves={mouse_moves} text={text_samples} link={link_samples} background={background_samples} boundary={boundary_samples} entry={entry_samples} stable_text={stable_text_samples} stable_link={stable_link_samples} stable_background={stable_background_samples} stable_mismatches={stable_mismatches}"
                    );
                    if stable_mismatches > 0 {
                        fail(&format!("physical cursor stability: {stable_mismatches} mismatching samples inside settled regions"));
                    }
                    dump_trace();
                    cx.update(|cx| cx.quit());
                    return;
                }
                unsafe {
                    invalidate_cursor_rects(native, view);
                    mouse_move(native, 300.);
                }
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                if trace_count(native, "appkit.resetCursorRects.add") == 0 {
                    fail("AppKit did not request a GPUIView cursor rectangle");
                }
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
                    cx.background_executor()
                        .timer(Duration::from_millis(20))
                        .await;
                    check(expected, "AppKit cursor-rectangle transition");
                    check_displayed(native, expected, "displayed AppKit cursor-rectangle transition");
                }
                unsafe {
                    mouse_move(native, 60.);
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                check(CursorStyle::IBeam, "text entry");
                // Temporal invariant: once the owned pointer is inside this stable
                // text hit area, every cadence sample must remain IBeam through
                // same-area movement and stationary redraws.
                unsafe { mouse_move(native, 60.); }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                check_displayed(native, CursorStyle::IBeam, "stable text target entry");
                for sample in 0..30 {
                    let x = 36. + ((sample % 6) as f64 * 12.);
                    let callback_count = trace_count(native, "appkit.gpui_callback.begin");
                    unsafe { mouse_event(native, x, true); }
                    cx.background_executor()
                        .timer(Duration::from_millis(8))
                        .await;
                    if trace_count(native, "appkit.gpui_callback.begin") <= callback_count {
                        fail("queued same-area movement did not reach the GPUI callback");
                    }
                    check_displayed(native, CursorStyle::IBeam, "stable text display-cadence sample");
                    if sample % 5 == 0 {
                        window
                            .update(cx, |surface, _, cx| {
                                surface.revision = surface.revision.wrapping_add(1);
                                cx.notify();
                            })
                            .unwrap();
                        cx.update_window(window.into(), |_, w, cx| {
                            let _ = w.draw(cx);
                        })
                        .unwrap();
                        check_displayed(native, CursorStyle::IBeam, "stationary text redraw sample");
                    }
                }
                let beam_rects_before = trace_count_detail(
                    native,
                    "appkit.resetCursorRects.add",
                    "style=IBeam",
                );
                unsafe { invalidate_cursor_rects(native, view); }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                if trace_count_detail(
                    native,
                    "appkit.resetCursorRects.add",
                    "style=IBeam",
                ) <= beam_rects_before
                {
                    fail("stationary AppKit invalidation did not rebuild the IBeam rectangle");
                }
                check_displayed(
                    native,
                    CursorStyle::IBeam,
                    "stationary same-style AppKit rectangle rebuild",
                );
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
                    cx.background_executor()
                        .timer(Duration::from_millis(20))
                        .await;
                    check(style, "stationary content replacement");
                    check_displayed(native, style, "displayed stationary content replacement");
                    cx.update_window(window.into(), |_, w, cx| {
                        let _ = w.draw(cx);
                    })
                    .unwrap();
                    check(style, "unchanged repaint");
                }
                // A real AppKit text control sits over a GPUI region with a
                // deliberately different cursor. Its own cursor rectangle must
                // win while the pointer is inside the child.
                window
                    .update(cx, |surface, _, cx| {
                        surface.style = CursorStyle::Crosshair;
                        cx.notify();
                    })
                    .unwrap();
                cx.update_window(window.into(), |_, w, cx| {
                    let _ = w.draw(cx);
                })
                .unwrap();
                let cursor_rect_rebuilds_before_child_add =
                    trace_count(native, "appkit.resetCursorRects.add");
                let child = unsafe {
                    let bounds: objc2_foundation::NSRect = msg_send![view, bounds];
                    let frame = objc2_foundation::NSRect::new(
                        NSPoint::new(250., bounds.size.height - 120.),
                        NSSize::new(50., 80.),
                    );
                    let child: Id = msg_send![class!(NSTextField), alloc];
                    let child: Id = msg_send![child, initWithFrame: frame];
                    let _: () = msg_send![view, addSubview: child];
                    let _: () = msg_send![native, invalidateCursorRectsForView: child];
                    // This is the explicit owner hook used by app native
                    // overlays after attaching or changing a child's frame.
                    invalidate_cursor_rects(native, view);
                    child
                };
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                if trace_count(native, "appkit.resetCursorRects.add")
                    <= cursor_rect_rebuilds_before_child_add
                {
                    fail("native child attachment did not rebuild the GPUIView cursor rectangles");
                }
                unsafe {
                    mouse_move(native, 275.);
                    let bounds: objc2_foundation::NSRect = msg_send![view, bounds];
                    let point = NSPoint::new(275., bounds.size.height - 80.);
                    let hit: Id = msg_send![view, hitTest: point];
                    if hit != child {
                        fail(&format!("native text child did not own its cursor test point: {hit:p} != {child:p}"));
                    }
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                check_displayed(native, CursorStyle::IBeam, "native text child cursor rectangle");
                unsafe { mouse_move(native, 245.); }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                check_displayed(native, CursorStyle::Crosshair, "GPUIView cursor outside native child");
                let button = unsafe {
                    let bounds: objc2_foundation::NSRect = msg_send![view, bounds];
                    let frame = objc2_foundation::NSRect::new(
                        NSPoint::new(300., bounds.size.height - 120.),
                        NSSize::new(50., 80.),
                    );
                    let button: Id = msg_send![class!(NSButton), alloc];
                    let button: Id = msg_send![button, initWithFrame: frame];
                    let _: () = msg_send![view, addSubview: button];
                    let _: () = msg_send![native, invalidateCursorRectsForView: button];
                    button
                };
                unsafe {
                    mouse_move(native, 325.);
                    let bounds: objc2_foundation::NSRect = msg_send![view, bounds];
                    let point = NSPoint::new(325., bounds.size.height - 80.);
                    let hit: Id = msg_send![view, hitTest: point];
                    if hit != button {
                        fail(&format!("native button did not own its cursor test point: {hit:p} != {button:p}"));
                    }
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                check_displayed(native, CursorStyle::Arrow, "native button retains the default cursor");
                let cursor_rect_rebuilds_before_child_move =
                    trace_count(native, "appkit.resetCursorRects.add");
                unsafe {
                    let moved_frame = objc2_foundation::NSRect::new(
                        NSPoint::new(310., 120.),
                        NSSize::new(50., 80.),
                    );
                    let _: () = msg_send![button, setFrame: moved_frame];
                    // NSView frame changes do not rebuild GPUIView's parent
                    // rectangles automatically; native child owners request
                    // this exact-window invalidation after layout changes.
                    invalidate_cursor_rects(native, view);
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                if trace_count(native, "appkit.resetCursorRects.add")
                    <= cursor_rect_rebuilds_before_child_move
                {
                    fail("native child frame change did not rebuild the GPUIView cursor rectangles");
                }
                unsafe { mouse_move(native, 305.); }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                check_displayed(
                    native,
                    CursorStyle::Crosshair,
                    "GPUIView cursor returns to the old native child frame",
                );
                unsafe { mouse_move(native, 325.); }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                check_displayed(native, CursorStyle::Arrow, "native cursor follows moved button");
                unsafe {
                    let _: () = msg_send![button, setHidden: true];
                    invalidate_cursor_rects(native, view);
                    mouse_move(native, 325.);
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                check_displayed(
                    native,
                    CursorStyle::Crosshair,
                    "hidden native child returns cursor ownership",
                );
                unsafe {
                    let _: () = msg_send![button, setHidden: false];
                    invalidate_cursor_rects(native, view);
                    mouse_move(native, 325.);
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                check_displayed(native, CursorStyle::Arrow, "shown native child owns cursor again");
                let cursor_rect_rebuilds_before_removal =
                    trace_count(native, "appkit.resetCursorRects.add");
                unsafe {
                    let _: () = msg_send![child, removeFromSuperview];
                    let _: () = msg_send![button, removeFromSuperview];
                    let _: () = msg_send![child, release];
                    let _: () = msg_send![button, release];
                    mouse_move(native, 275.);
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                if trace_count(native, "appkit.resetCursorRects.add")
                    <= cursor_rect_rebuilds_before_removal
                {
                    fail("native child removal did not rebuild the GPUIView cursor rectangles");
                }
                check_displayed(native, CursorStyle::Crosshair, "GPUIView cursor after native child removal");
                let overlay = unsafe {
                    let rect: objc2_foundation::NSRect = msg_send![native, frame];
                    let overlay: Id = msg_send![class!(NSWindow), alloc];
                    let overlay: Id = msg_send![overlay, initWithContentRect: rect,
                        styleMask: 1usize, backing: 2usize, defer: false];
                    let _: () = msg_send![overlay, setReleasedWhenClosed: false];
                    let _: () = msg_send![overlay, orderFrontRegardless];
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
                // The pointer remains stationary in this local fixture, so
                // AppKit has not selected a cursor for the newly-covered
                // window. Seed a test-only stack sentinel to verify that
                // drawing the covered GPUI window cannot publish over it.
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
                check(CursorStyle::Crosshair, "covering native window ownership");
                unsafe {
                    let _: () =
                        msg_send![overlay, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                // Re-seed after AppKit activation, then ensure a style update
                // to the now-inactive GPUI window cannot replace the owner.
                unsafe { let _: () = msg_send![cursor(CursorStyle::Crosshair), set]; }
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
                check(CursorStyle::Crosshair, "inactive GPUI window cannot take cursor");
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
                // Window activation may deliver a real movement at the physical
                // pointer. Establish the test's local text position again.
                unsafe { mouse_move(native, 60.); }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                check(CursorStyle::IBeam, "return to GPUI key window");
                cx.update_window(window.into(), |_, w, cx| {
                    w.set_ui_scale(1.5, cx);
                    let _ = w.draw(cx);
                })
                .unwrap();
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                unsafe {
                    mouse_move(native, 60.);
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                check(CursorStyle::IBeam, "scaled native coordinates");
                unsafe {
                    // At 1.5x, the link occupies the second 120-point column;
                    // x=180 is centered inside that native-coordinate region.
                    mouse_move(native, 180.);
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                check(CursorStyle::PointingHand, "scaled link coordinates");
                unsafe {
                    mouse_move(native, 195.);
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                // x=195 remains inside the scaled link column.
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
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                check(
                    expected,
                    "stationary scale invalidation resamples native pointer",
                );
                println!("Native cursor probe passed");
                dump_trace();
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }
}
fn main() {
    #[cfg(target_os = "macos")]
    if std::env::var_os("VORKSPACE_NATIVE_CURSOR_PROBE").is_some()
        || std::env::var_os("VORKSPACE_NATIVE_CURSOR_PHYSICAL_PROBE").is_some()
    {
        probe::run();
        return;
    }
    println!("native cursor probe skipped (set VORKSPACE_NATIVE_CURSOR_PROBE=1 on macOS)");
}
