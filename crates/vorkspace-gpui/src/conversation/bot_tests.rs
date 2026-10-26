use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle, size};
use std::{cell::RefCell, rc::Rc, sync::Mutex};
use vorkspace_core::{bot::peers::Peer, store::Store};

fn message(id: &str, text: &str, internal: bool) -> Message {
    Message {
        id: id.into(),
        text: text.into(),
        internal,
        peer: None,
        sent_at: None,
        error: None,
        attachments: Vec::new(),
    }
}

fn peer_message() -> Message {
    Message {
        peer: Some(Peer {
            from: "merlin".into(),
            hop: 1,
            expect_reply: true,
        }),
        ..message(
            "peer",
            "<bot_peer_message>\nFrom: {\"id\":\"merlin\",\"name\":\"Merlin\",\"hop\":1}\nYour final reply is sent back automatically.\n</bot_peer_message>\n\nPlease review the change.",
            true,
        )
    }
}

fn chat(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        input::bind(cx);
        let store = Store::open(Path::new(":memory:")).unwrap();
        let summary = store.create("codex", "/private/tmp/bot-queue-ui").unwrap();
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                view.loaded = true;
                view.configure_bot(
                    Bot {
                        name: "Sam".into(),
                        ..Default::default()
                    },
                    "/private/tmp".into(),
                    Default::default(),
                    cx,
                );
                view.bot_post("assistant", "Checking the change.".into(), cx)
                    .unwrap();
                view.input
                    .update(cx, |input, cx| input.set_text("Unsent draft".into(), cx));
                view
            })
        })
        .unwrap()
    })
}

#[gpui::test]
fn automatic_updates_stay_quiet_and_peer_delivery_keeps_one_transcript_message(
    cx: &mut TestAppContext,
) {
    let window = chat(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    for width in [360., 900.] {
        visual.simulate_resize(size(px(width), px(600.)));
        window
            .update(visual, |view, _, cx| {
                view.busy = false;
                view.bot_ui.pending.clear();
                view.bot_ui.delivered = None;
                view.sync_bot_pending(cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        let composer = visual.debug_bounds("bot-composer").unwrap();
        window
            .update(visual, |view, _, cx| {
                view.busy = true;
                view.bot_ui.pending = vec![
                    message("background", "[Delegated work completed]", true),
                    peer_message(),
                    message("human", "Check recovery next", false),
                ];
                view.sync_bot_pending(cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("bot-background-update-error").is_none());
        assert!(visual.debug_bounds("bot-pending-peer").is_some());
        assert!(visual.debug_bounds("bot-pending-human").is_some());
        assert_eq!(visual.debug_bounds("bot-composer").unwrap(), composer);

        window
            .update(visual, |view, _, cx| {
                view.busy = false;
                view.bot_post("bot-system", peer_message().text, cx)
                    .unwrap();
                view.bot_ui.delivered = Some("peer".into());
                view.sync_bot_pending(cx);
                view.busy = true;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("bot-pending-peer").is_none());
        assert!(visual.debug_bounds("bot-peer-message-label").is_some());
        assert!(visual.debug_bounds("bot-background-update-error").is_none());
        assert_eq!(visual.debug_bounds("bot-composer").unwrap(), composer);
        window
            .update(visual, |view, _, cx| {
                assert_eq!(
                    view.bot_ui.pending.len(),
                    3,
                    "presentation must preserve the pending messages"
                );
                assert_eq!(view.input.read(cx).content.as_ref(), "Unsent draft");
            })
            .unwrap();
    }
}

#[gpui::test]
fn background_and_peer_errors_show_recovery_controls_once(cx: &mut TestAppContext) {
    let window = chat(cx);
    let events = Rc::new(RefCell::new(Vec::new()));
    let _subscriptions = window
        .update(cx, |view, _, cx| {
            let background = events.clone();
            let peer = events.clone();
            let subscriptions = (
                cx.subscribe(&cx.entity(), move |_, _, event: &QueueChanged, _| {
                    background
                        .borrow_mut()
                        .push((event.id.clone(), event.remove));
                }),
                cx.subscribe(
                    &view.transcript,
                    move |_, _, event: &crate::transcript::BotResponseAction, _| {
                        if let crate::transcript::BotResponseAction::Queue { id, remove } = event {
                            peer.borrow_mut().push((id.clone(), *remove));
                        }
                    },
                ),
            );
            let mut background = message("background", "[Delegated work completed]", true);
            background.error = Some("Couldn't deliver the background update".into());
            let mut peer = peer_message();
            peer.error = Some("Stopped before the peer reply completed".into());
            view.bot_ui.pending = vec![background, peer];
            view.bot_ui.delivered = Some("peer".into());
            view.sync_bot_pending(cx);
            cx.notify();
            subscriptions
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(360.), px(600.)));
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-background-update-error").is_some());
    assert!(visual.debug_bounds("bot-pending-peer").is_some());
    for selector in [
        "bot-queue-retry",
        "bot-queue-discard",
        "bot-pending-retry-peer",
        "bot-pending-discard-peer",
    ] {
        let button = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing {selector}"));
        assert!(
            button.left() >= px(0.) && button.right() <= px(360.),
            "{selector} must fit the viewport: {button:?}"
        );
        visual.simulate_click(button.center(), Default::default());
        visual.run_until_parked();
    }
    assert_eq!(
        *events.borrow(),
        vec![
            ("background".into(), false),
            ("background".into(), true),
            ("peer".into(), false),
            ("peer".into(), true),
        ]
    );
    window
        .update(visual, |view, _, cx| {
            view.bot_ui.pending.retain(|message| message.peer.is_some());
            view.sync_bot_pending(cx);
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-background-update-error").is_none());
    assert!(visual.debug_bounds("bot-pending-retry-peer").is_some());
    assert!(visual.debug_bounds("bot-pending-discard-peer").is_some());
}

#[gpui::test]
fn only_the_users_own_message_steers_a_running_bot_turn(cx: &mut TestAppContext) {
    let window = chat(cx);
    let (controls, received) = std::sync::mpsc::channel();
    let failures = Rc::new(RefCell::new(Vec::new()));
    window
        .update(cx, |view, _, cx| {
            let seen = failures.clone();
            cx.subscribe(&cx.entity(), move |_, _, event: &SteerFailed, _| {
                seen.borrow_mut()
                    .push((event.id.clone(), event.error.clone()));
            })
            .detach();
            view.busy = true;
            view.controls = Some(controls);
            let user = Message {
                attachments: vec!["/tmp/plan.png".into()],
                ..message("user", "Use the other approach", false)
            };
            assert!(
                !view.bot_steer(&user, cx),
                "a turn that has not reported readiness keeps the message queued"
            );
            view.follow_ups.ready = true;
            assert!(!view.bot_steer(&message("report", "Completion", true), cx));
            assert!(!view.bot_steer(&peer_message(), cx));
            assert!(view.bot_steer(&user, cx));
            assert!(
                !view.bot_steer(&message("next", "And this", false), cx),
                "one steer is in flight at a time"
            );
            assert!(view.bot_delivered().is_none());
            view.follow_up_delivered("user", cx);
            assert_eq!(view.bot_delivered(), Some("user"));
            assert!(view.bot_steer(&message("next", "And this", false), cx));
            view.fail_follow_up("next", "turn is closing".into(), cx);
            assert!(view.follow_ups.sending.is_none());
        })
        .unwrap();
    let Decision::Steer(steer) = received.try_recv().unwrap().decision else {
        panic!("expected steering");
    };
    assert_eq!(steer.id, "user");
    assert_eq!(steer.text, "Use the other approach");
    assert_eq!(steer.attachments, [PathBuf::from("/tmp/plan.png")]);
    assert!(matches!(received.try_recv().unwrap().decision, Decision::Steer(m) if m.id == "next"));
    assert!(received.try_recv().is_err());
    assert_eq!(
        *failures.borrow(),
        [("next".to_owned(), "turn is closing".to_owned())]
    );
}

#[gpui::test]
fn a_final_update_keeps_the_accepted_steer_receipt(cx: &mut TestAppContext) {
    let window = chat(cx);
    let (controls, _received) = std::sync::mpsc::channel();
    window
        .update(cx, |view, _, cx| {
            view.busy = true;
            view.controls = Some(controls);
            view.follow_ups.ready = true;
            assert!(view.bot_steer(&message("steer", "Change course", false), cx));
            // A stop can skip the intermediate acknowledgement. The final
            // update then carries the turn's initial receipt and the steer.
            view.apply_receipts(Some("initial".into()), vec!["steer".into()], Vec::new(), cx);
            assert_eq!(view.bot_delivered(), Some("steer"));
            assert!(view.follow_ups.sending.is_none());
        })
        .unwrap();
}
