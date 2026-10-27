use super::*;
use gpui::{TestAppContext, WindowHandle};
use std::sync::Mutex;
use vorkspace_core::{store::Store, transcript::attachments};

#[gpui::test]
fn unsupported_history_keeps_the_unsent_composer_and_declines_external_work(
    cx: &mut TestAppContext,
) {
    let window = conversation(cx);
    for provider in [
        "cursor", "grok", "opencode", "fx", "hermes", "devin", "unknown",
    ] {
        window
            .update(cx, |view, _, cx| {
                view.summary.provider = provider.into();
                view.input
                    .update(cx, |input, cx| input.set_text("My unsent draft".into(), cx));
                view.attachments = vec!["/missing/retained.png".into()];
                let generation = view.submission_generation;
                view.send(cx);
                assert!(!view.busy);
                assert!(view.status.contains("no longer supported"));
                assert_eq!(view.input.read(cx).content.as_str(), "My unsent draft");
                assert_eq!(
                    view.attachments,
                    vec![std::path::PathBuf::from("/missing/retained.png")]
                );
                assert_eq!(view.submission_generation, generation);
                assert!(view.transcript.read(cx).source_rows().is_empty());
                assert!(
                    locked(&view.store)
                        .unwrap()
                        .local_submissions(view.summary.id)
                        .unwrap()
                        .is_empty()
                );
                view.input
                    .update(cx, |input, cx| input.set_text(String::new(), cx));
                view.attachments.clear();
                assert!(!view.ready_for_external(cx));
                assert!(
                    view.voice_send("Voice request".into(), cx)
                        .unwrap_err()
                        .contains("no longer supported")
                );
                assert!(
                    view.app_start_message("/operator App request".into(), cx)
                        .is_err()
                );
                assert!(!view.settings.operator);
                assert!(view.follow_ups.state.messages.is_empty());
                assert!(
                    locked(&view.store)
                        .unwrap()
                        .follow_ups(view.summary.id)
                        .unwrap()
                        .messages
                        .is_empty()
                );

                let queued = vorkspace_core::follow_up::Message {
                    id: "saved-queue".into(),
                    text: "Retained queued request".into(),
                    ..Default::default()
                };
                view.follow_ups.state.messages = vec![queued.clone()];
                view.follow_ups.state.revision = 1;
                view.follow_ups.persisted = 1;
                view.settings.new_worktree = true;
                view.pump_follow_ups(cx);
                assert!(!view.preparing_worktree);
                assert!(view.follow_ups.sending.is_none());
                assert_eq!(view.follow_ups.state.messages, vec![queued]);
                view.launch_prompt("Direct retry".into(), cx);
                assert!(!view.busy);
                assert!(view.transcript.read(cx).source_rows().is_empty());
                view.follow_ups.state.messages.clear();
            })
            .unwrap();
    }
}

#[gpui::test]
fn dropped_sender_releases_the_current_ui_and_pauses_queue_without_replay(cx: &mut TestAppContext) {
    for stop_before_close in [false, true] {
        let window = conversation(cx);
        cx.run_until_parked();
        let (tx, rx) = async_channel::bounded(1);
        let (generation, owner, attempt, rows) = window
            .update(cx, |view, _, cx| {
                let attempt = format!("dropped-sender-{stop_before_close}");
                let owner = {
                    let store = locked(&view.store).unwrap();
                    store
                        .reserve_local_submission(
                            view.summary.id,
                            vorkspace_core::store::SubmissionInput {
                                attempt_id: attempt.clone(),
                                user_turn_id: "sent-turn".into(),
                                session_id: String::new(),
                                payload_json: "{\"text\":\"Sent message\"}".into(),
                                user_turn: Some(
                                    serde_json::json!({"id":"sent-turn","role":"user","text":"Sent message","attachments":[]}),
                                ),
                            },
                        )
                        .unwrap();
                    assert!(store.dispatch_local_submission(&attempt).unwrap().claimed);
                    store.local_submission_owner()
                };
                view.begin_turn(cx);
                view.preparing_worktree = true;
                view.planning_orchestration = true;
                let rows = vec![
                    vorkspace_core::transcript::Row {
                        role: "user".into(),
                        text: "Sent message".into(),
                        first: true,
                        sent_at: Some(1),
                    },
                    vorkspace_core::transcript::Row {
                        role: "assistant".into(),
                        text: "Partial response".into(),
                        first: true,
                        sent_at: Some(2),
                    },
                ];
                view.transcript
                    .update(cx, |view, cx| view.patch(0, rows.clone(), cx));
                view.input
                    .update(cx, |input, cx| input.set_text("Unsent draft".into(), cx));
                view.follow_ups
                    .state
                    .messages
                    .push(vorkspace_core::follow_up::Message {
                        id: "queued".into(),
                        text: "Keep this queued".into(),
                        ..Default::default()
                    });
                view.follow_ups.sending = Some("queued".into());
                view.follow_ups.requested = Some("queued".into());
                view.follow_ups.continuations.push_back("queued".into());
                view.follow_ups.ready = true;
                view.follow_ups.resuming = true;
                let (controls, _) = std::sync::mpsc::channel();
                view.controls = Some(controls);
                view.approvals.push_back(Approval {
                    id: "approval".into(),
                    title: "Approve".into(),
                    detail: String::new(),
                    can_remember: false,
                });
                let question = cx.new(|cx| {
                    crate::questions::QuestionCard::new(
                        vorkspace_core::agent::Question {
                            id: "question".into(),
                            title: "Question".into(),
                            items: vec![],
                            auto_resolve_at: None,
                        },
                        cx,
                    )
                });
                view.questions.push_back((question, vec![]));
                let generation = view.submission_generation;
                view.receive_turn_updates(
                    rx,
                    generation,
                    view.summary.provider.clone(),
                    view.settings.account.id.clone(),
                    None,
                    cx,
                );
                if stop_before_close {
                    view.stop(cx);
                }
                (generation, owner, attempt, rows)
            })
            .unwrap();
        cx.run_until_parked();
        // Match the worker RAII ordering: release acceptance ownership before
        // the last sender disappears. Recovery must never infer non-delivery.
        owner.release(&attempt);
        drop(tx);
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert!(!view.busy && !view.stopping && !view.preparing_worktree);
                assert!(!view.planning_orchestration);
                assert!(
                    view.controls.is_none()
                        && view.approvals.is_empty()
                        && view.questions.is_empty()
                );
                assert_eq!(view.status, UNCONFIRMED_TURN);
                assert_eq!(
                    view.submission_generation, generation,
                    "no queue replay starts a new turn"
                );
                assert!(view.follow_ups.state.paused);
                assert!(!view.follow_ups.ready && !view.follow_ups.resuming);
                assert!(view.follow_ups.sending.is_none() && view.follow_ups.requested.is_none());
                assert!(view.follow_ups.continuations.is_empty());
                assert_eq!(view.follow_ups.state.messages.len(), 1);
                assert_eq!(view.follow_ups.state.messages[0].text, "Keep this queued");
                assert_eq!(
                    view.follow_ups.state.messages[0].error.as_deref(),
                    Some(UNCONFIRMED_TURN)
                );
                assert_eq!(view.input.read(cx).content.as_ref(), "Unsent draft");
                assert_eq!(view.transcript.read(cx).source_rows(), rows);
                assert!(
                    view.transcript.read(cx).turns[&1].completed_at.is_some(),
                    "streaming was ended"
                );
                let receipt = view
                    .unconfirmed_submission
                    .as_ref()
                    .expect("ordinary recovery refresh ran");
                assert_eq!(receipt.attempt_id, attempt);
                assert_eq!(receipt.phase, "uncertain");
                assert!(receipt.outcome.is_none());
                let saved = locked(&view.store)
                    .unwrap()
                    .follow_ups(view.summary.id)
                    .unwrap();
                assert!(saved.paused);
                assert_eq!(saved.messages, view.follow_ups.state.messages);
            })
            .unwrap();
    }
}

#[gpui::test]
fn an_old_dropped_sender_cannot_release_a_successor_turn(cx: &mut TestAppContext) {
    let window = conversation(cx);
    cx.run_until_parked();
    let (tx, rx) = async_channel::bounded(1);
    let (generation, epoch) = window
        .update(cx, |view, _, cx| {
            view.begin_turn(cx);
            view.receive_turn_updates(
                rx,
                view.submission_generation,
                view.summary.provider.clone(),
                view.settings.account.id.clone(),
                None,
                cx,
            );
            view.begin_turn(cx);
            view.cancel = Arc::new(AtomicBool::new(false));
            view.status = "Successor turn is working".into();
            let (controls, _) = std::sync::mpsc::channel();
            view.controls = Some(controls);
            view.follow_ups.ready = true;
            (view.submission_generation, view.submission_recovery_epoch)
        })
        .unwrap();
    cx.run_until_parked();
    drop(tx);
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert_eq!(view.submission_generation, generation);
            assert!(view.busy && !view.stopping);
            assert!(view.controls.is_some() && view.follow_ups.ready);
            assert!(!view.follow_ups.state.paused);
            assert!(!view.cancel.load(Ordering::Relaxed));
            assert_eq!(view.submission_recovery_epoch, epoch);
            assert_eq!(view.status, "Successor turn is working");
        })
        .unwrap();
}

fn conversation(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
    let store = Store::open(Path::new(":memory:")).unwrap();
    // Fail after saving the turn, without ever launching a real provider.
    let summary = store
        .create("codex", "/missing/optimistic-send-project")
        .unwrap();
    cx.update(|cx| {
        input::bind(cx);
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
fn send_paints_before_storage_and_keeps_one_message_after_startup_failure(cx: &mut TestAppContext) {
    for text in [
        "First message".to_owned(),
        "🙂 café\n".repeat(400),
        String::new(),
    ] {
        let window = conversation(cx);
        let attachment = std::path::PathBuf::from("/missing/optimistic-attachment.png");
        let sent_at = window
            .update(cx, |view, _, cx| {
                view.attachments.push(attachment.clone());
                view.input
                    .update(cx, |input, cx| input.set_text(text.clone(), cx));
                view.send(cx);
                assert!(view.busy);
                assert!(view.summary.has_messages);
                assert!(view.input.read(cx).content.is_empty());
                assert!(view.attachments.is_empty());
                let rows = view.transcript.read(cx).source_rows();
                assert_eq!(
                    rows.iter()
                        .filter(|row| row.first && row.is_prompt())
                        .count(),
                    1
                );
                assert_eq!(
                    rows.iter()
                        .filter(|row| row.is_prompt())
                        .map(|row| row.text.as_str())
                        .collect::<String>(),
                    text
                );
                assert_eq!(rows.last().unwrap().role, "assistant");
                let manifest = rows
                    .iter()
                    .find(|row| row.role == attachments::ROLE)
                    .unwrap();
                assert_eq!(
                    attachments::paths(manifest).unwrap(),
                    std::slice::from_ref(&attachment)
                );
                assert!(
                    locked(&view.store)
                        .unwrap()
                        .load(view.summary.id)
                        .unwrap()
                        .rows
                        .is_empty(),
                    "the bubble must exist before the background save runs"
                );
                rows[0].sent_at
            })
            .unwrap();
        super::receive_tests::settle_turn(window, cx);
        window
            .update(cx, |view, _, cx| {
                assert!(!view.busy);
                assert_ne!(
                    view.status, UNCONFIRMED_TURN,
                    "a final update confirms normal termination"
                );
                let rows = view.transcript.read(cx).source_rows();
                assert_eq!(
                    rows.iter()
                        .filter(|row| row.first && row.is_prompt())
                        .count(),
                    1
                );
                assert_eq!(
                    rows[0].sent_at, sent_at,
                    "acceptance keeps the original send time"
                );
                assert_eq!(rows.last().unwrap().role, "error");
                assert_eq!(
                    rows,
                    locked(&view.store)
                        .unwrap()
                        .load(view.summary.id)
                        .unwrap()
                        .rows
                );
                // A later send must append after the failed message, not replace it.
                view.input
                    .update(cx, |input, cx| input.set_text("Try again".into(), cx));
                view.send(cx);
                assert_eq!(
                    view.transcript
                        .read(cx)
                        .source_rows()
                        .iter()
                        .filter(|row| row.first && row.is_prompt())
                        .count(),
                    2
                );
            })
            .unwrap();
        super::receive_tests::settle_turn(window, cx);
        window
            .update(cx, |view, _, cx| {
                let rows = view.transcript.read(cx).source_rows();
                assert_eq!(
                    rows.iter()
                        .filter(|row| row.first && row.is_prompt())
                        .count(),
                    2
                );
                assert_eq!(
                    rows,
                    locked(&view.store)
                        .unwrap()
                        .load(view.summary.id)
                        .unwrap()
                        .rows
                );
            })
            .unwrap();
    }
}

#[gpui::test]
fn worktree_setup_shows_the_prompt_immediately_and_preserves_it_on_failure(
    cx: &mut TestAppContext,
) {
    let window = conversation(cx);
    window
        .update(cx, |view, _, cx| {
            view.settings.new_worktree = true;
            view.settings.plan = true;
            view.input.update(cx, |input, cx| {
                input.set_text("Start in a worktree".into(), cx)
            });
            view.send(cx);
            assert!(view.busy && view.preparing_worktree);
            assert_eq!(view.status, "Creating worktree…");
            assert!(view.input.read(cx).content.is_empty());
            assert!(!view.settings.plan);
            assert_eq!(
                view.transcript.read(cx).source_rows()[0].text,
                "Start in a worktree"
            );
            view.input
                .update(cx, |input, cx| input.set_text("Follow up".into(), cx));
            view.send(cx);
            assert_eq!(view.follow_ups.state.messages[0].text, "Follow up");
            assert_eq!(view.follow_ups.state.messages[0].mode, None);
            view.input
                .update(cx, |input, cx| input.set_text("Unsent draft".into(), cx));
        })
        .unwrap();
    super::receive_tests::settle_turn(window, cx);
    window
        .update(cx, |view, _, cx| {
            assert!(!view.busy && !view.preparing_worktree);
            assert_eq!(view.input.read(cx).content, "Unsent draft");
            assert!(view.follow_ups.state.paused);
            assert_eq!(view.follow_ups.state.messages[0].text, "Follow up");
            let rows = view.transcript.read(cx).source_rows();
            assert_eq!(rows[0].text, "Start in a worktree");
            assert_eq!(rows.last().unwrap().role, "error");
            assert_eq!(
                rows,
                locked(&view.store)
                    .unwrap()
                    .load(view.summary.id)
                    .unwrap()
                    .rows
            );
        })
        .unwrap();
}

#[gpui::test]
fn stopping_before_the_worker_runs_keeps_the_optimistic_message(cx: &mut TestAppContext) {
    for new_worktree in [false, true] {
        let window = conversation(cx);
        window
            .update(cx, |view, _, cx| {
                view.settings.new_worktree = new_worktree;
                view.input.update(cx, |input, cx| {
                    input.set_text("Keep this message".into(), cx)
                });
                view.send(cx);
                view.stop(cx);
                assert!(!view.busy);
                assert_eq!(
                    view.transcript.read(cx).source_rows()[0].text,
                    "Keep this message"
                );
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert!(!view.stopping);
                assert!(!view.preparing_worktree);
                let rows = view.transcript.read(cx).source_rows();
                assert_eq!(
                    rows.iter()
                        .filter(|row| row.first && row.is_prompt())
                        .count(),
                    1
                );
                assert_eq!(rows[0].text, "Keep this message");
                assert_eq!(
                    rows,
                    locked(&view.store)
                        .unwrap()
                        .load(view.summary.id)
                        .unwrap()
                        .rows
                );
            })
            .unwrap();
    }
}
