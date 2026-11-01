
use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle};
use std::sync::Mutex;
use vorkspace_core::store::Store;

fn conversation(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let summary = store.create("codex", "/tmp").unwrap();
    cx.update(|cx| {
        input::bind(cx);
        cx.set_global(crate::settings::Preferences {
            follow_up: "queue".into(),
            ..Default::default()
        });
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                view.loaded = true;
                view
            })
        })
        .unwrap()
    })
}

#[gpui::test]
fn queue_entrances_slide_above_a_stationary_composer_for_the_first_and_later_messages(
    cx: &mut TestAppContext,
) {
    use std::time::Duration;
    let window = conversation(cx);
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            view.summary.has_messages = true;
            view.busy = true;
            view.follow_ups.state.paused = true;
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    let bounds = |window: WindowHandle<Conversation>, cx: &mut TestAppContext| {
        window
            .update(cx, |view, window, _| {
                let composer = window
                    .painted_quads()
                    .into_iter()
                    .find(|quad| {
                        (quad.corner_radii.top_left.0 / window.scale_factor() - 8.).abs() < 0.1
                            && quad.bounds.size.width.0 / window.scale_factor() > 300.
                    })
                    .expect("the composer surface must be painted");
                (
                    f32::from(view.composer_anchor.get().unwrap().top()),
                    composer.bounds.top().0 / window.scale_factor(),
                )
            })
            .unwrap()
    };
    let (mut queue_top, input_top) = bounds(window, cx);
    for (index, text) in ["First queued message", "A second queued message"]
        .into_iter()
        .enumerate()
    {
        window
            .update(cx, |view, _, cx| {
                view.enqueue_follow_up(text.into(), None, cx);
                view.follow_ups.motion.elapsed(Duration::ZERO);
                cx.notify();
            })
            .unwrap();
        cx.refresh().unwrap();
        let (starting, composer) = bounds(window, cx);
        assert!(
            (composer - input_top).abs() < 1.,
            "the input must stay still"
        );
        assert!(
            (starting - queue_top).abs() < 8.,
            "the entrance starts behind the input"
        );

        window
            .update(cx, |view, _, cx| {
                view.follow_ups.motion.elapsed(Duration::from_millis(40));
                cx.notify();
            })
            .unwrap();
        cx.refresh().unwrap();
        let (middle, composer) = bounds(window, cx);
        assert!((composer - input_top).abs() < 1.);
        assert!(middle < starting - 5., "the queue must slide upward");

        window
            .update(cx, |view, _, cx| {
                view.follow_ups.motion.elapsed(Duration::from_secs(1));
                cx.notify();
            })
            .unwrap();
        cx.refresh().unwrap();
        let (settled, composer) = bounds(window, cx);
        assert!((composer - input_top).abs() < 1.);
        assert!(
            settled < middle - 5.,
            "the spring must settle into the full row height"
        );
        assert!(queue_top - settled >= if index == 0 { 60. } else { 30. });
        queue_top = settled;
    }
}

#[gpui::test]
fn submitted_modes_clear_after_queueing_and_do_not_leak_into_the_next_message(
    cx: &mut TestAppContext,
) {
    for mode in ["operator", "orchestrator", "plan"] {
        let window = conversation(cx);
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                view.busy = true;
                view.follow_ups.state.paused = true;
                view.toggle_mode(mode, cx);
                let selected = view.input.read(cx).content.clone();
                view.send(cx);
                assert_eq!(
                    view.input.read(cx).content,
                    selected,
                    "an empty submission keeps its mode"
                );
                view.input
                    .update(cx, |i, cx| i.set_text(format!("/{mode} first message"), cx));
                view.send(cx);
                assert!(view.input.read(cx).content.is_empty());
                assert!(
                    !view.settings.plan && !view.settings.operator && !view.settings.orchestrator
                );
                view.input
                    .update(cx, |i, cx| i.set_text("ordinary follow-up".into(), cx));
                view.send(cx);
                let messages = &view.follow_ups.state.messages;
                assert_eq!(messages[0].mode.as_deref(), Some(mode));
                assert_eq!(messages[0].text, "first message");
                assert_eq!(messages[1].mode, None);
                assert_eq!(messages[1].text, "ordinary follow-up");
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, _| {
                let store = locked(&view.store).unwrap();
                let saved = store.agent_settings(view.summary.id).unwrap().unwrap();
                assert!(!saved.plan && !saved.operator && !saved.orchestrator);
                let queue = store.follow_ups(view.summary.id).unwrap();
                assert_eq!(queue.messages[0].mode.as_deref(), Some(mode));
                assert_eq!(queue.messages[1].mode, None);
            })
            .unwrap();
    }
}

#[gpui::test]
fn voice_send_is_durable_before_acceptance_and_never_consumes_drafts(cx: &mut TestAppContext) {
    let window = conversation(cx);
    cx.run_until_parked();
    cx.update(|cx| {
        let prefs = crate::settings::Preferences {
            follow_up: "queue".into(),
            ..Default::default()
        };
        cx.set_global(prefs);
    });
    window
        .update(cx, |view, _, cx| {
            view.busy = true;
            view.input
                .update(cx, |i, cx| i.set_text("typed draft".into(), cx));
            assert!(view.voice_send("voice message".into(), cx).is_err());
            assert_eq!(view.input.read(cx).content, "typed draft");
            view.input.update(cx, |i, cx| i.set_text(String::new(), cx));
            view.attachments.push("/tmp/unsent-image.png".into());
            assert!(view.voice_send("voice message".into(), cx).is_err());
            assert_eq!(view.attachments.len(), 1);
            view.attachments.clear();
            view.voice_send("voice message".into(), cx).unwrap();
            let saved = locked(&view.store)
                .unwrap()
                .follow_ups(view.summary.id)
                .unwrap();
            assert_eq!(saved.messages.len(), 1);
            assert_eq!(saved.messages[0].text, "voice message");
            assert!(view.input.read(cx).content.is_empty());
        })
        .unwrap();
}
#[gpui::test]
fn busy_sends_keep_their_order_attachments_and_edits_until_accepted(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let (controls, received) = std::sync::mpsc::channel();
    window
        .update(cx, |view, _, cx| {
            view.busy = true;
            view.controls = Some(controls);
            view.input
                .update(cx, |i, cx| i.set_text("first".into(), cx));
            view.attachments.push("/tmp/image.png".into());
            view.send(cx);
            view.input
                .update(cx, |i, cx| i.set_text("second".into(), cx));
            view.send(cx);
            assert!(view.input.read(cx).content.is_empty());
            assert_eq!(
                view.follow_ups
                    .state
                    .messages
                    .iter()
                    .map(|m| m.text.as_str())
                    .collect::<Vec<_>>(),
                ["first", "second"]
            );
        })
        .unwrap();
    cx.run_until_parked();
    assert!(received.try_recv().is_err());
    window
        .update(cx, |view, w, cx| {
            let id = view.follow_ups.state.messages[0].id.clone();
            view.edit_queued(id, w, cx);
            view.follow_ups
                .edit
                .as_ref()
                .unwrap()
                .input
                .update(cx, |i, cx| i.set_text("edited 🙂\nwith detail".into(), cx));
            view.follow_ups.ready = true;
            view.follow_ups.state.messages[0].steer = true;
            view.pump_follow_ups(cx);
            assert!(
                received.try_recv().is_err(),
                "editing the head must block delivery"
            );
            view.finish_queue_edit(true, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let control = received.try_recv().unwrap();
    let Decision::Steer(message) = control.decision else {
        panic!("expected steering");
    };
    assert_eq!(message.text, "edited 🙂\nwith detail");
    assert_eq!(
        message.attachments,
        [std::path::PathBuf::from("/tmp/image.png")]
    );
    window
        .update(cx, |view, _, cx| {
            assert_eq!(
                view.follow_ups.state.messages.len(),
                2,
                "a write to the control channel is not an acceptance"
            );
            view.follow_up_delivered(&message.id, cx);
            assert_eq!(view.follow_ups.state.messages[0].text, "second");
            assert!(view.follow_ups.sending.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn stop_pauses_the_queue_and_retires_its_controls(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let (controls, received) = std::sync::mpsc::channel();
    window
        .update(cx, |view, _, cx| {
            view.busy = true;
            view.controls = Some(controls);
            view.input
                .update(cx, |input, cx| input.set_text("keep this".into(), cx));
            view.send(cx);
            view.stop(cx);
            assert!(view.stopping);
            assert!(view.follow_ups.state.paused);
            assert!(!view.follow_ups.ready);
            assert!(view.controls.is_none());
            assert_eq!(view.follow_ups.state.messages[0].text, "keep this");
        })
        .unwrap();
    cx.run_until_parked();
    assert!(matches!(
        received.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected)
    ));
}

#[gpui::test]
fn a_rejected_steer_keeps_its_text_for_retry(cx: &mut TestAppContext) {
    let window = conversation(cx);
    cx.update(|cx| {
        cx.update_global::<crate::settings::Preferences, _>(|prefs, _| {
            prefs.follow_up = "steer".into()
        });
    });
    let (controls, received) = std::sync::mpsc::channel();
    window
        .update(cx, |view, _, cx| {
            view.busy = true;
            view.controls = Some(controls);
            view.follow_ups.ready = true;
            view.input
                .update(cx, |input, cx| input.set_text("keep this".into(), cx));
            view.send(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let id = received.try_recv().unwrap().id;
    window
        .update(cx, |view, _, cx| {
            view.fail_follow_up(&id, "Rejected by provider".into(), cx);
            assert!(view.follow_ups.state.paused);
            assert_eq!(view.follow_ups.state.messages[0].text, "keep this");
            assert_eq!(
                view.follow_ups.state.messages[0].error.as_deref(),
                Some("Rejected by provider")
            );
        })
        .unwrap();
}

#[gpui::test]
fn working_sessions_and_bots_can_save_a_draft_without_sending_it(cx: &mut TestAppContext) {
    for bot in [false, true] {
        let window = conversation(cx);
        let (controls, received) = std::sync::mpsc::channel();
        window
            .update(cx, |view, _, cx| {
                if bot {
                    view.configure_bot(
                        vorkspace_core::bot::Bot::default(),
                        "/tmp".into(),
                        Default::default(),
                        cx,
                    );
                }
                view.busy = true;
                view.summary.has_messages = true;
                view.follow_ups.ready = true;
                view.controls = Some(controls);
                cx.notify();
            })
            .unwrap();
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let add = visual.debug_bounds("composer-add").unwrap().center();
        visual.simulate_click(add, Default::default());
        visual.run_until_parked();
        let draft = visual.debug_bounds("menu-action-draft").unwrap().center();
        visual.simulate_click(draft, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |view, _, cx| {
                assert!(
                    view.follow_ups.draft_selected,
                    "Draft is selectable while working"
                );
                view.input.update(cx, |input, cx| {
                    input.set_text("/draft Save this for later".into(), cx)
                });
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        let save = visual.debug_bounds("send-or-stop").unwrap().center();
        visual.simulate_click(save, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |view, _, cx| {
                assert!(view.busy, "saving must not stop the running agent");
                assert_eq!(
                    view.follow_ups.state.draft.as_ref().unwrap().text,
                    "Save this for later"
                );
                assert!(view.follow_ups.state.messages.is_empty());
                assert!(view.input.read(cx).content.is_empty());
            })
            .unwrap();
        assert!(
            received.try_recv().is_err(),
            "saving must not send or steer"
        );
        window
            .update(visual, |view, _, _| {
                assert_eq!(
                    locked(&view.store)
                        .unwrap()
                        .follow_ups(view.summary.id)
                        .unwrap()
                        .draft
                        .unwrap()
                        .text,
                    "Save this for later"
                );
            })
            .unwrap();
    }
}

#[gpui::test]
fn draft_commands_save_without_starting_an_agent_and_remove_restores_a_blank(
    cx: &mut TestAppContext,
) {
    let window = conversation(cx);
    window
        .update(cx, |view, _, cx| {
            view.input.update(cx, |i, cx| {
                i.set_text(" /DRAFT Explore this later".into(), cx)
            });
            view.attachments.push("/tmp/note.txt".into());
            view.send(cx);
            assert!(!view.busy);
            assert!(view.summary.has_messages);
            assert!(
                view.transcript.read(cx).source_rows().is_empty(),
                "saved drafts must never enter provider history"
            );
            assert_eq!(
                view.follow_ups.state.draft.as_ref().unwrap().text,
                "Explore this later"
            );
            assert!(view.input.read(cx).content.is_empty());
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            let saved = locked(&view.store)
                .unwrap()
                .follow_ups(view.summary.id)
                .unwrap();
            assert_eq!(
                saved.draft.unwrap().attachments,
                [std::path::PathBuf::from("/tmp/note.txt")]
            );
            view.input
                .update(cx, |i, cx| i.set_text("my next message".into(), cx));
            view.draft_action(false, cx);
            assert!(!view.summary.has_messages);
            assert_eq!(view.input.read(cx).content, "my next message");
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert!(!locked(&view.store).unwrap().sessions().unwrap()[0].has_messages);
        })
        .unwrap();
}

#[gpui::test]
fn sending_a_saved_draft_preserves_the_new_composer(cx: &mut TestAppContext) {
    let window = conversation(cx);
    cx.update(|cx| {
        cx.update_global::<crate::settings::Preferences, _>(|prefs, _| {
            prefs.follow_up = "steer".into()
        })
    });
    let (controls, received) = std::sync::mpsc::channel();
    window
        .update(cx, |view, _, cx| {
            view.input
                .update(cx, |i, cx| i.set_text("/draft send later".into(), cx));
            view.send(cx);
            view.busy = true;
            view.follow_ups.ready = true;
            view.controls = Some(controls);
            view.input
                .update(cx, |i, cx| i.set_text("keep typing".into(), cx));
            view.attachments.push("/tmp/new-file.txt".into());
            view.draft_action(true, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let control = received.try_recv().unwrap();
    let Decision::Steer(message) = control.decision else {
        panic!("expected saved draft");
    };
    assert_eq!(message.text, "send later");
    assert!(message.attachments.is_empty());
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.input.read(cx).content, "keep typing");
            assert_eq!(
                view.attachments,
                [std::path::PathBuf::from("/tmp/new-file.txt")]
            );
            assert!(view.follow_ups.state.draft.is_none());
        })
        .unwrap();
}
