use super::*;
use gpui::{TestAppContext, WindowHandle};
use std::sync::Mutex;
use vorkspace_core::{store::Store, transcript::attachments};

fn conversation(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
    let store = Store::open(Path::new(":memory:")).unwrap();
    // Fail after saving the turn, without ever launching a real provider.
    let mut summary = store
        .create("codex", "/missing/optimistic-send-project")
        .unwrap();
    summary.provider = "fixture".into();
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
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert!(!view.busy);
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
        cx.run_until_parked();
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
    cx.run_until_parked();
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
