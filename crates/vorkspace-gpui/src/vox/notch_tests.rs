use super::*;
use gpui::{AppContext, TestAppContext, VisualTestContext, WindowHandle};
use std::time::Duration;

fn voice(_cx: &mut gpui::Context<Vox>) -> Vox {
    let store = Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::open_vorkspace(std::path::Path::new(":memory:")).unwrap(),
    ));
    let prefs = crate::settings::Preferences {
        vox_enabled: true,
        vox_silent: true,
        ..Default::default()
    };
    let mut voice = Vox::new(WeakEntity::new_invalid(), store, &prefs, "/project".into());
    voice.state = "live".into();
    voice.face = "o-o".into();
    voice.active.store(true, Ordering::Relaxed);
    voice.notch.touched = crate::clock::Instant::now();
    // This is a display-only fixture: no provider or microphone is started.
    voice
}

fn window(cx: &mut TestAppContext) -> WindowHandle<Vox> {
    crate::clock::use_test_clock(cx);
    let window = cx.update(|cx| {
        crate::input::bind(cx);
        cx.open_window(Default::default(), |_, cx| cx.new(voice))
            .unwrap()
    });
    // As the controller does once the notch opens.
    window
        .update(cx, |voice, _, _| voice.panel = Some(window))
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_mouse_move(gpui::point(px(900.), px(600.)), None, Default::default());
    visual.run_until_parked();
    window
}

/// Gives the notch the keyboard, with the pointer away from it.
fn activate(window: WindowHandle<Vox>, cx: &mut TestAppContext) {
    window
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.simulate_mouse_move(gpui::point(px(900.), px(600.)), None, Default::default());
    visual.run_until_parked();
}

#[gpui::test]
fn disabling_vox_cannot_be_reversed_by_a_late_panel_click(cx: &mut TestAppContext) {
    let window = window(cx);
    window
        .update(cx, |voice, window, cx| {
            voice.enabled = false;
            voice.active.store(false, Ordering::Relaxed);
            voice.notch.close();
            voice.toggle_call(window, cx);
            assert!(voice.notch.closing);
            assert!(!voice.active.load(Ordering::Relaxed));
            assert!(!voice.running());
        })
        .unwrap();
}

#[gpui::test]
fn idle_timeout_ends_capture_and_retracts_without_clearing_the_reply(cx: &mut TestAppContext) {
    let window = window(cx);
    window
        .update(cx, |voice, _, cx| {
            voice.reply = "Keep this completed reply".into();
            voice.reply_id = "one".into();
            voice.arm_idle(cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(29));
    cx.run_until_parked();
    window
        .update(cx, |voice, _, _| {
            assert!(voice.active.load(Ordering::Relaxed))
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    window
        .update(cx, |voice, _, _| {
            assert!(!voice.active.load(Ordering::Relaxed));
            assert!(voice.notch.closing);
            assert_eq!(voice.reply, "Keep this completed reply");
        })
        .unwrap();
}

#[gpui::test]
fn speech_processing_and_reading_pause_the_idle_deadline(cx: &mut TestAppContext) {
    let window = window(cx);
    for activity in ["hearing", "thinking", "replying", "reading"] {
        window
            .update(cx, |voice, _, cx| {
                voice.notch.hearing = activity == "hearing";
                voice.notch.hovered = activity == "reading";
                voice.notch.activity = if matches!(activity, "thinking" | "replying") {
                    activity.into()
                } else {
                    "listening".into()
                };
                voice.touch(cx);
            })
            .unwrap();
        cx.executor().advance_clock(Duration::from_secs(90));
        cx.run_until_parked();
        window
            .update(cx, |voice, _, _| {
                assert!(voice.active.load(Ordering::Relaxed), "{activity}")
            })
            .unwrap();
    }
    window
        .update(cx, |voice, _, cx| {
            voice.notch.hearing = false;
            voice.notch.hovered = false;
            voice.notch.activity = "listening".into();
            voice.touch(cx);
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(30));
    cx.run_until_parked();
    window
        .update(cx, |voice, _, _| {
            assert!(!voice.active.load(Ordering::Relaxed))
        })
        .unwrap();
}

#[gpui::test]
fn silent_reply_stays_below_the_notch_and_survives_status_updates(cx: &mut TestAppContext) {
    let window = window(cx);
    window
        .update(cx, |voice, _, cx| {
            voice.reply = "A selectable reply below the notch. ".repeat(20);
            voice.reply_id = "one".into();
            cx.notify();
        })
        .unwrap();
    crate::clock::settle_animations(window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let gap = visual.debug_bounds("vox-notch-gap").unwrap();
    let face = visual.debug_bounds("vox-toggle").unwrap();
    let status = visual.debug_bounds("vox-status").unwrap();
    let reply = visual.debug_bounds("vox-reply").unwrap();
    assert!(face.right() <= gap.left());
    assert!(status.left() >= gap.right());
    assert_eq!(reply.top(), gap.bottom());
    assert!(reply.size.height <= px(184.));
    let input = window
        .update(visual, |voice, _, _| voice.reply_input.clone().unwrap())
        .unwrap();
    window
        .update(visual, |voice, _, cx| {
            voice.display_activity(&json!({"display":{"activity":"thinking"}}), cx);
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |voice, _, _| {
            assert_eq!(voice.reply_input.as_ref(), Some(&input));
            assert_eq!(voice.status(), "Thinking");
        })
        .unwrap();
}

#[gpui::test]
fn an_old_idle_deadline_cannot_stop_a_new_call(cx: &mut TestAppContext) {
    let window = window(cx);
    window
        .update(cx, |voice, _, cx| voice.arm_idle(cx))
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(20));
    window
        .update(cx, |voice, _, cx| {
            voice.generation += 1;
            voice.touch(cx);
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(11));
    cx.run_until_parked();
    window
        .update(cx, |voice, _, _| {
            assert!(voice.active.load(Ordering::Relaxed))
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(19));
    cx.run_until_parked();
    window
        .update(cx, |voice, _, _| {
            assert!(!voice.active.load(Ordering::Relaxed))
        })
        .unwrap();
}

#[gpui::test]
fn keyboard_reading_keeps_a_completed_reply_open_until_dismissal(cx: &mut TestAppContext) {
    use gpui::Focusable;
    let window = window(cx);
    window
        .update(cx, |voice, _, cx| {
            voice.reply = "Read this without a pointer over the notch".into();
            voice.reply_id = "one".into();
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    activate(window, cx);
    window
        .update(cx, |voice, window, cx| {
            window.focus(
                &voice
                    .reply_input
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .focus_handle(cx),
                cx,
            );
            voice.touch(cx);
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(35));
    cx.run_until_parked();
    window
        .update(cx, |voice, _, cx| {
            assert!(voice.active.load(Ordering::Relaxed));
            assert!(voice.dismiss_reply(cx));
            voice.touch(cx);
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(30));
    cx.run_until_parked();
    window
        .update(cx, |voice, _, _| {
            assert!(!voice.active.load(Ordering::Relaxed))
        })
        .unwrap();
}

#[test]
#[ignore = "Requires Metal and VORKSPACE_VOX_CAPTURE_DIR; display-only fixture"]
fn native_notch_state_captures() {
    let output = std::path::PathBuf::from(std::env::var_os("VORKSPACE_VOX_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    crate::clock::use_test_clock(&cx);
    let window = cx
        .open_window(gpui::size(px(480.), px(240.)), |_, cx| cx.new(voice))
        .unwrap();
    for (name, activity, reply) in [
        ("listening", "listening", ""),
        ("thinking", "thinking", ""),
        ("speaking", "replying", ""),
        (
            "silent",
            "listening",
            "The new interface lives at your notch. You can keep working in another app while Vox listens and replies here.",
        ),
    ] {
        window
            .update(&mut cx, |voice, _, cx| {
                voice.notch.activity = activity.into();
                voice.silent = !reply.is_empty();
                voice.reply = reply.into();
                voice.reply_id = name.into();
                voice.face = if activity == "thinking" { "p-p" } else { "o-o" }.into();
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        crate::clock::settle_headless_animations(window, &mut cx);
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join(format!("vox-{name}.png")))
            .unwrap();
    }
}

#[gpui::test]
fn a_focused_reply_stops_counting_as_reading_after_switching_apps(cx: &mut TestAppContext) {
    use gpui::Focusable;
    let window = window(cx);
    window
        .update(cx, |voice, _, cx| {
            voice.reply = "Focused, then left behind".into();
            voice.reply_id = "one".into();
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    activate(window, cx);
    window
        .update(cx, |voice, window, cx| {
            let reply = voice
                .reply_input
                .as_ref()
                .unwrap()
                .read(cx)
                .focus_handle(cx);
            window.focus(&reply, cx);
            voice.touch(cx);
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.deactivate_window();
    visual.simulate_mouse_move(gpui::point(px(900.), px(600.)), None, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |voice, window, cx| {
            // Deactivation keeps the window's focus.
            assert!(!window.is_window_active());
            let reply = voice
                .reply_input
                .as_ref()
                .unwrap()
                .read(cx)
                .focus_handle(cx);
            assert!(reply.is_focused(window));
        })
        .unwrap();
    visual.executor().advance_clock(Duration::from_secs(30));
    visual.run_until_parked();
    window
        .update(visual, |voice, _, _| {
            assert!(!voice.active.load(Ordering::Relaxed))
        })
        .unwrap();
}

#[gpui::test]
fn idle_deadlines_use_the_notch_window_when_another_window_renders_vox(cx: &mut TestAppContext) {
    use gpui::Focusable;
    let window = window(cx);
    let voice = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    // A workspace window that reads Vox while rendering, as the rail does,
    // becomes the entity's most recent window.
    struct Workspace(gpui::Entity<Vox>);
    impl Render for Workspace {
        fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
            div().child(if self.0.read(cx).running() {
                "on"
            } else {
                "off"
            })
        }
    }
    let workspace = cx.update(|cx| {
        let voice = voice.clone();
        cx.open_window(Default::default(), |_, cx| cx.new(|_| Workspace(voice)))
            .unwrap()
    });
    cx.run_until_parked();
    window
        .update(cx, |voice, _, cx| {
            voice.reply = "Read in the notch".into();
            voice.reply_id = "one".into();
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    activate(window, cx);
    window
        .update(cx, |voice, window, cx| {
            let reply = voice
                .reply_input
                .as_ref()
                .unwrap()
                .read(cx)
                .focus_handle(cx);
            window.focus(&reply, cx);
        })
        .unwrap();
    cx.run_until_parked();
    workspace
        .update(cx, |_, window, cx| {
            window.refresh();
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    window.update(cx, |voice, _, cx| voice.touch(cx)).unwrap();
    cx.executor().advance_clock(Duration::from_secs(35));
    cx.run_until_parked();
    window
        .update(cx, |voice, _, _| {
            assert!(
                voice.active.load(Ordering::Relaxed),
                "the reply focused in the notch keeps the call open"
            )
        })
        .unwrap();
}
