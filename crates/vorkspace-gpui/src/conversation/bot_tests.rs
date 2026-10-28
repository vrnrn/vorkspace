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
        crate::clock::use_test_clock(cx);
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
fn typing_keeps_the_unchanged_bot_transcript_rendered(cx: &mut TestAppContext) {
    let window = chat(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(900.), px(680.)));
    window
        .update(visual, |view, window, cx| {
            window.focus(&view.input.focus_handle(cx), cx);
        })
        .unwrap();
    visual.run_until_parked();
    // Establish the nonempty composer and its final geometry before measuring.
    visual.simulate_input("a");
    visual.run_until_parked();
    let (transcript, draft, source, renders) = window
        .read_with(visual, |view, cx| {
            let transcript = view.transcript.read(cx);
            (
                view.transcript.clone(),
                view.input.read(cx).content.to_string(),
                transcript.source_rows().to_vec(),
                transcript.render_passes,
            )
        })
        .unwrap();
    for _ in 0..60 {
        visual.simulate_input("a");
        visual.run_until_parked();
    }
    transcript.read_with(visual, |view, _| {
        assert_eq!(view.render_passes, renders);
        assert_eq!(view.source_rows(), source);
    });
    window
        .update(visual, |view, window, cx| {
            assert_eq!(
                view.input.read(cx).content.as_ref(),
                draft + &"a".repeat(60)
            );
            assert!(view.input.focus_handle(cx).is_focused(window));
        })
        .unwrap();

    window
        .update(visual, |view, _, cx| {
            view.bot_post("assistant", "A newly published reply.".into(), cx)
                .unwrap();
        })
        .unwrap();
    visual.run_until_parked();
    transcript.read_with(visual, |view, _| {
        assert!(view.render_passes > renders);
        assert_eq!(
            view.source_rows().last().unwrap().text,
            "A newly published reply."
        );
    });
}

#[gpui::test]
fn header_ticks_do_not_notify_the_chat_or_rebuild_its_transcript(cx: &mut TestAppContext) {
    use std::cell::Cell;
    let window = chat(cx);
    window
        .update(cx, |view, _, cx| view.show_bot_header(true, cx))
        .unwrap();
    cx.run_until_parked();
    let (header, transcript) = window
        .read_with(cx, |view, _| {
            (view.bot_ui.header.clone().unwrap(), view.transcript.clone())
        })
        .unwrap();
    let chat = window.entity(cx).unwrap();
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _subscription =
        cx.update(|cx| cx.observe(&chat, move |_, _| observed.set(observed.get() + 1)));
    let renders = transcript.read_with(cx, |view, _| view.render_passes);
    for _ in 0..60 {
        // The real animation timer notifies precisely this retained owner.
        // Perpetual timers stay suppressed on the deterministic test clock.
        header.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }
    assert_eq!(notifications.get(), 0);
    assert_eq!(
        transcript.read_with(cx, |view, _| view.render_passes),
        renders
    );
    window
        .read_with(cx, |view, _| {
            assert_eq!(view.bot_ui.header.as_ref().unwrap(), &header);
        })
        .unwrap();
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
        let composer = visual.debug_bounds("composer-box").unwrap();
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
        assert_eq!(visual.debug_bounds("composer-box").unwrap(), composer);

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
        assert_eq!(visual.debug_bounds("composer-box").unwrap(), composer);
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
    crate::clock::settle_animations(window, visual);
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
        crate::clock::settle_animations(window, visual);
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
    crate::clock::settle_animations(window, visual);
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

fn card_fixture(
    cx: &mut TestAppContext,
) -> (
    WindowHandle<Conversation>,
    Database,
    i64,
    usize,
    serde_json::Value,
) {
    let window = chat(cx);
    let (store, id, index, card) = window.update(cx, |view, _, cx| {
        let card = serde_json::json!({"id":"refresh-card","type":"pr","repo":"synthetic/repo","number":1,"title":"Old"});
        let index = view.bot_rows(cx).len();
        view.bot_post("bot-card", card.to_string(), cx).unwrap();
        (view.store.clone(), view.summary.id, index, card)
    }).unwrap();
    (window, store, id, index, card)
}

#[gpui::test]
fn card_save_admission_stays_responsive_and_publishes_latest_at_the_current_row(
    cx: &mut TestAppContext,
) {
    let (window, store, id, index, card) = card_fixture(cx);
    let guard = locked(&store).unwrap();
    window
        .update(cx, |view, _, cx| {
            // This would deadlock if admission took the Store mutex on the UI.
            view.bot_patch_card(
                index,
                "refresh-card",
                view.submission_generation,
                card.clone(),
                card.clone(),
                cx,
            )
            .unwrap();
            assert!(view.bot_ui.card_versions.is_empty());
            for title in ["One", "Two"] {
                let mut next = card.clone();
                next["title"] = serde_json::json!(title);
                view.bot_patch_card(
                    index,
                    "refresh-card",
                    view.submission_generation,
                    card.clone(),
                    next,
                    cx,
                )
                .unwrap();
            }
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&view.bot_rows(cx)[index].text).unwrap(),
                card
            );
            view.input.update(cx, |input, cx| {
                input.set_text("Still responsive".into(), cx)
            });
            // A reprojected row position cannot redirect the completion's identity.
            let mut rows = view.bot_rows(cx).to_vec();
            rows.insert(
                0,
                vorkspace_core::transcript::Row {
                    role: "assistant".into(),
                    text: "Synthetic prefix".into(),
                    first: true,
                    sent_at: None,
                },
            );
            view.transcript.update(cx, |t, cx| t.patch(0, rows, cx));
        })
        .unwrap();
    drop(guard);
    cx.update(|cx| crate::runtime::card_persistence::finish_for_test(cx));
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            let (_, saved) = card_at(view.bot_rows(cx), index, "refresh-card").unwrap();
            assert_eq!(saved["title"], "Two");
            assert_eq!(view.bot_rows(cx)[0].text, "Synthetic prefix");
            assert_eq!(view.input.read(cx).content, "Still responsive");
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &locked(&store).unwrap().load(id).unwrap().rows[index].text
        )
        .unwrap()["title"],
        "Two"
    );
}

#[gpui::test]
fn committed_card_refresh_survives_a_new_submission_and_the_next_refresh(cx: &mut TestAppContext) {
    for advance_before_commit in [true, false] {
        let (window, store, session, index, original) = card_fixture(cx);
        let mut committed = original.clone();
        committed["title"] = serde_json::json!("Committed");
        let guard = locked(&store).unwrap();
        let generation = window
            .update(cx, |view, _, cx| {
                let generation = view.submission_generation;
                view.bot_patch_card(
                    index,
                    "refresh-card",
                    generation,
                    original.clone(),
                    committed.clone(),
                    cx,
                )
                .unwrap();
                if advance_before_commit {
                    view.submission_generation += 1;
                }
                generation
            })
            .unwrap();
        drop(guard);
        // Wait for the actual worker's commit without handling the UI receipt
        // or closing the worker that must perform the following refresh.
        cx.update(|cx| crate::runtime::card_persistence::wait_for_saves_for_test(cx));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                &locked(&store).unwrap().load(session).unwrap().rows[index].text
            )
            .unwrap(),
            committed
        );
        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    card_at(view.bot_rows(cx), index, "refresh-card").unwrap().1,
                    original
                );
                if !advance_before_commit {
                    view.submission_generation += 1;
                }
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    card_at(view.bot_rows(cx), index, "refresh-card").unwrap().1,
                    committed
                );
                assert!(view.bot_ui.card_versions.is_empty());
                assert_eq!(view.input.read(cx).content, "Unsent draft");
                // A fetch begun in the old submission is still inadmissible,
                // even though it now happens to match the reconciled base.
                let mut delayed = committed.clone();
                delayed["title"] = serde_json::json!("Obsolete fetch");
                view.bot_patch_card(
                    index,
                    "refresh-card",
                    generation,
                    committed.clone(),
                    delayed,
                    cx,
                )
                .unwrap();
                assert!(view.bot_ui.card_versions.is_empty());
                let mut latest = committed.clone();
                latest["title"] = serde_json::json!("Latest");
                let expected = card_at(view.bot_rows(cx), index, "refresh-card").unwrap().1;
                view.bot_patch_card(
                    index,
                    "refresh-card",
                    view.submission_generation,
                    expected,
                    latest,
                    cx,
                )
                .unwrap();
                assert!(!view.bot_ui.card_versions.is_empty());
            })
            .unwrap();
        cx.update(|cx| crate::runtime::card_persistence::wait_for_saves_for_test(cx));
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    card_at(view.bot_rows(cx), index, "refresh-card").unwrap().1["title"],
                    "Latest"
                );
                assert!(view.bot_ui.card_versions.is_empty());
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                &locked(&store).unwrap().load(session).unwrap().rows[index].text
            )
            .unwrap()["title"],
            "Latest"
        );
    }
    cx.update(|cx| crate::runtime::card_persistence::finish_for_test(cx));
}

#[gpui::test]
fn card_save_completion_keeps_replacements_and_reconciles_new_submissions(cx: &mut TestAppContext) {
    let first = card_fixture(cx);
    let second = card_fixture(cx);
    let guard1 = locked(&first.1).unwrap();
    let guard2 = locked(&second.1).unwrap();
    for (fixture, replacement) in [(&first, true), (&second, false)] {
        let (window, _, _, index, card) = fixture;
        window
            .update(cx, |view, _, cx| {
                let mut next = card.clone();
                next["title"] = serde_json::json!("New");
                view.bot_patch_card(
                    *index,
                    "refresh-card",
                    view.submission_generation,
                    card.clone(),
                    next,
                    cx,
                )
                .unwrap();
                if replacement {
                    let mut row = view.bot_rows(cx)[*index].clone();
                    let mut new_card = card.clone();
                    new_card["id"] = serde_json::json!("replacement");
                    row.text = new_card.to_string();
                    view.transcript
                        .update(cx, |t, cx| t.patch(*index, vec![row], cx));
                } else {
                    view.submission_generation += 1;
                }
            })
            .unwrap();
    }
    drop(guard1);
    drop(guard2);
    cx.update(|cx| crate::runtime::card_persistence::finish_for_test(cx));
    cx.run_until_parked();
    for (fixture, replacement) in [(&first, true), (&second, false)] {
        let (window, store, id, index, _) = fixture;
        window
            .update(cx, |view, _, cx| {
                let visible: serde_json::Value =
                    serde_json::from_str(&view.bot_rows(cx)[*index].text).unwrap();
                assert_eq!(visible["title"], if replacement { "Old" } else { "New" });
                assert_eq!(
                    visible["id"],
                    if replacement {
                        "replacement"
                    } else {
                        "refresh-card"
                    }
                );
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                &locked(store).unwrap().load(*id).unwrap().rows[*index].text
            )
            .unwrap()["title"],
            "New"
        );
    }
}

#[gpui::test]
fn delayed_pr_reply_is_rejected_at_the_owning_card_before_admission(cx: &mut TestAppContext) {
    let (window, store, id, index, card) = card_fixture(cx);
    let mut newer = card.clone();
    newer["title"] = serde_json::json!("Newer");
    let row = locked(&store)
        .unwrap()
        .refresh_bot_card(id, &card, &newer)
        .unwrap()
        .unwrap();
    window
        .update(cx, |view, _, cx| {
            view.transcript
                .update(cx, |t, cx| t.patch(index, vec![row], cx));
            let mut delayed = card.clone();
            delayed["title"] = serde_json::json!("Delayed");
            view.bot_patch_card(
                index,
                "refresh-card",
                view.submission_generation,
                card,
                delayed,
                cx,
            )
            .unwrap();
            assert!(view.bot_ui.card_versions.is_empty());
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&view.bot_rows(cx)[index].text).unwrap(),
                newer
            );
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &locked(&store).unwrap().load(id).unwrap().rows[index].text
        )
        .unwrap(),
        newer
    );
}

#[gpui::test]
fn card_refresh_from_an_older_submission_is_rejected_before_store_admission(
    cx: &mut TestAppContext,
) {
    let (window, store, _, index, card) = card_fixture(cx);
    let guard = locked(&store).unwrap();
    window
        .update(cx, |view, _, cx| {
            let generation = view.submission_generation;
            view.submission_generation += 1;
            let mut delayed = card.clone();
            delayed["title"] = serde_json::json!("Delayed");
            view.bot_patch_card(index, "refresh-card", generation, card.clone(), delayed, cx)
                .unwrap();
            assert!(view.bot_ui.card_versions.is_empty());
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&view.bot_rows(cx)[index].text).unwrap(),
                card
            );
        })
        .unwrap();
    drop(guard);
}

#[gpui::test]
fn bot_plus_offers_upload_and_queue_and_queue_applies_to_one_message(cx: &mut TestAppContext) {
    let window = chat(cx);
    let sent = Rc::new(RefCell::new(Vec::new()));
    let output = sent.clone();
    let _events = window
        .update(cx, |_, _, cx| {
            cx.subscribe(&cx.entity(), move |_, _, event: &Submitted, _| {
                output.borrow_mut().push((event.text.clone(), event.queue))
            })
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("composer-add").is_some());
    assert!(visual.debug_bounds("composer-queue-mode").is_none());
    window
        .update(visual, |view, window, cx| {
            let mut events = Vec::new();
            let (_, items, _, _) = view.menu_items(ComposerMenu::Add, &mut events, window, cx);
            let ids: Vec<_> = items
                .iter()
                .filter_map(|item| match item {
                    MenuItem::Action(action) => Some(action.id.to_string()),
                    _ => None,
                })
                .collect();
            assert_eq!(ids, ["upload", "queue"]);
            view.menu_choice("queue", cx);
            assert!(view.bot_ui.queue);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("composer-queue-mode").is_some());
    window
        .update(visual, |view, _, cx| {
            view.send(cx);
            assert!(!view.bot_ui.queue, "queueing applies to one message");
            view.input
                .update(cx, |input, cx| input.set_text("Next".into(), cx));
            view.send(cx);
        })
        .unwrap();
    assert_eq!(
        *sent.borrow(),
        [
            ("Unsent draft".to_owned(), true),
            ("Next".to_owned(), false)
        ]
    );
}
