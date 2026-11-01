use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle};
use std::{sync::Mutex, time::Duration};
use vorkspace_core::store::Store;

fn conversation(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let summary = store.create("codex", "/tmp").unwrap();
    cx.update(|cx| {
        input::bind(cx);
        cx.set_global(vorkspace_ui::preferences::Preferences {
            follow_up: "queue".into(),
            ..Default::default()
        });
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                view.loaded = true;
                view.summary.has_messages = true;
                view.busy = true;
                view.follow_ups.state.paused = true;
                view
            })
        })
        .unwrap()
    })
}

fn request(id: &str, can_remember: bool) -> Approval {
    Approval {
        id: id.into(),
        title: "Run a command?".into(),
        detail: "git status --short".into(),
        can_remember,
    }
}

#[gpui::test]
fn approval_shelf_slides_up_without_moving_the_composer_or_replaying_on_redraw(
    cx: &mut TestAppContext,
) {
    let window = conversation(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    let input = visual.debug_bounds("composer-box").unwrap();
    for queued in [false, true] {
        window
            .update(visual, |view, _, cx| {
                view.approvals.clear();
                if queued {
                    view.enqueue_follow_up("A queued follow-up".into(), None, cx);
                    view.follow_ups.motion.elapsed(Duration::from_secs(1));
                }
                cx.notify();
            })
            .unwrap();
        visual.refresh().unwrap();
        window
            .update(visual, |view, _, cx| {
                view.approvals
                    .push_back(request(if queued { "second" } else { "first" }, true));
                cx.notify();
            })
            .unwrap();
        visual.refresh().unwrap();
        let start = visual.debug_bounds("approval-shelf").unwrap();
        assert!((start.top() - input.top()).abs() < px(1.));
        assert_eq!(visual.debug_bounds("composer-box").unwrap(), input);

        window
            .update(visual, |view, _, cx| {
                view.approval_ui.motion.elapsed(Duration::from_millis(40));
                cx.notify();
            })
            .unwrap();
        visual.refresh().unwrap();
        let middle = visual.debug_bounds("approval-shelf").unwrap();
        assert!(middle.top() < start.top() - px(5.));
        assert_eq!(visual.debug_bounds("composer-box").unwrap(), input);

        window
            .update(visual, |view, _, cx| {
                view.approval_ui.motion.elapsed(Duration::from_secs(1));
                cx.notify();
            })
            .unwrap();
        visual.refresh().unwrap();
        let settled = visual.debug_bounds("approval-shelf").unwrap();
        assert!(settled.top() < middle.top() - px(5.));
        assert_eq!(settled.bottom(), input.top());
        assert_eq!(visual.debug_bounds("composer-box").unwrap(), input);
        window.update(visual, |_, _, cx| cx.notify()).unwrap();
        visual.refresh().unwrap();
        assert_eq!(visual.debug_bounds("approval-shelf").unwrap(), settled);
    }
    vorkspace_ui::clock::use_test_clock(cx);
    window
        .update(visual, |view, _, cx| {
            view.approvals.clear();
            view.approvals.push_back(request("approval", true));
            cx.notify();
        })
        .unwrap();
    visual.refresh().unwrap();
    vorkspace_ui::clock::settle_animations(window, visual);
    assert!(visual.debug_bounds("approval-shelf").unwrap().size.height > px(60.));
    assert_eq!(visual.debug_bounds("composer-box").unwrap(), input);
}

#[gpui::test]
fn approval_shelf_buttons_resolve_each_request_with_the_selected_decision(cx: &mut TestAppContext) {
    let window = conversation(cx);
    vorkspace_ui::clock::use_test_clock(cx);
    let (send, receive) = std::sync::mpsc::channel();
    window
        .update(cx, |view, _, cx| {
            view.controls = Some(send);
            view.approvals.extend([
                request("once", true),
                request("session", true),
                request("denied", false),
            ]);
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    for (button, id) in [
        ("approval-allow", "once"),
        ("approval-session", "session"),
        ("approval-deny", "denied"),
    ] {
        if id == "denied" {
            assert!(visual.debug_bounds("approval-session").is_none());
        }
        vorkspace_ui::clock::settle_animations(window, visual);
        let bounds = visual.debug_bounds(button).unwrap();
        visual.simulate_click(bounds.center(), Default::default());
        visual.refresh().unwrap();
        let control = receive.try_recv().unwrap();
        assert_eq!(control.id, id);
        assert!(matches!(
            (id, control.decision),
            ("once", Decision::Allow) | ("session", Decision::Session) | ("denied", Decision::Deny)
        ));
        assert!(receive.try_recv().is_err());
    }
    vorkspace_ui::clock::settle_animations(window, visual);
    assert!(visual.debug_bounds("approval-shelf").is_none());
    window
        .read_with(visual, |view, _| assert!(view.approvals.is_empty()))
        .unwrap();
}
