
use super::*;
use gpui::{TestAppContext, WindowHandle};
fn fixture(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
    let store = vorkspace_core::store::Store::open(Path::new(":memory:")).unwrap();
    let summary = store.create("codex", "/missing/ticket-project").unwrap();
    cx.update(|cx| {
        input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut chat =
                    Conversation::new(summary, Arc::new(std::sync::Mutex::new(store)), cx);
                chat.loaded = true;
                chat
            })
        })
        .unwrap()
    })
}
const REFERENCE: &str =
    "Explain vorkspace://context/github?url=https%3A%2F%2Fgithub.com%2Facme%2Fdemo%2Fissues%2F42";
#[gpui::test]
fn failed_lookup_preserves_composer_selection_files_and_history(cx: &mut TestAppContext) {
    let window = fixture(cx);
    cx.run_until_parked();
    window
        .update(cx, |chat, _, cx| {
            chat.input.update(cx, |input, cx| {
                input.set_text(REFERENCE.into(), cx);
                input.select_utf16(2..9, cx);
            });
            chat.attachments.push("/missing/private.txt".into());
            chat.send(cx);
            assert!(!chat.busy);
            assert_eq!(chat.input.read(cx).content.as_str(), REFERENCE);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |chat, _, cx| {
            assert!(!chat.busy);
            assert!(
                chat.status
                    .contains("Ticket context storage is unavailable")
            );
            assert_eq!(chat.input.read(cx).content.as_str(), REFERENCE);
            assert_eq!(chat.input.read(cx).selection_utf16(), 2..9);
            assert_eq!(
                chat.attachments,
                vec![std::path::PathBuf::from("/missing/private.txt")]
            );
            assert!(chat.follow_ups.state.messages.is_empty());
            assert!(chat.transcript.read(cx).source_rows().is_empty());
            assert!(
                locked(&chat.store)
                    .unwrap()
                    .local_submissions(chat.summary.id)
                    .unwrap()
                    .is_empty()
            );
        })
        .unwrap();
}
#[gpui::test]
fn newer_typing_and_cancel_own_the_composer_before_lookup_completion(cx: &mut TestAppContext) {
    for cancel in [false, true] {
        let window = fixture(cx);
        cx.run_until_parked();
        window
            .update(cx, |chat, _, cx| {
                chat.input
                    .update(cx, |input, cx| input.set_text(REFERENCE.into(), cx));
                chat.send(cx);
                assert!(chat.ticket_context.pending.is_some());
                if cancel {
                    chat.stop(cx);
                    assert_eq!(chat.status, "Ready");
                    assert!(chat.ticket_context.pending.is_none());
                    assert_eq!(chat.input.read(cx).content.as_str(), REFERENCE);
                }
                chat.input.update(cx, |input, cx| {
                    input.set_text("My newer draft 🙂".into(), cx)
                });
                chat.status = "User owns the new draft".into();
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |chat, _, cx| {
                assert_eq!(chat.input.read(cx).content.as_str(), "My newer draft 🙂");
                assert_eq!(chat.status, "User owns the new draft");
                assert!(!chat.busy && chat.ticket_context.ready.is_none());
                assert!(chat.transcript.read(cx).source_rows().is_empty());
            })
            .unwrap();
    }
}
#[gpui::test]
fn an_older_remote_host_cannot_consume_a_typed_ticket_reference(cx: &mut TestAppContext) {
    let window = fixture(cx);
    cx.run_until_parked();
    window
        .update(cx, |chat, _, cx| {
            chat.summary.cwd = "remote://fixture/project".into();
            chat.remote_state.online = true;
            chat.input
                .update(cx, |input, cx| input.set_text(REFERENCE.into(), cx));
            chat.attachments.push("/missing/private.txt".into());
            assert!(chat.prepare_ticket_context(cx));
            assert!(chat.status.contains("Update the remote host"));
            assert_eq!(chat.input.read(cx).content.as_str(), REFERENCE);
            assert_eq!(chat.attachments.len(), 1);
            assert!(!chat.busy && chat.ticket_context.pending.is_none());
        })
        .unwrap();
}
#[gpui::test]
fn editing_a_pending_lookup_clears_its_loading_indicator(cx: &mut TestAppContext) {
    let window = fixture(cx);
    cx.run_until_parked();
    window
        .update(cx, |chat, _, cx| {
            chat.input
                .update(cx, |input, cx| input.set_text(REFERENCE.into(), cx));
            chat.send(cx);
            chat.input
                .update(cx, |input, cx| input.set_text("New draft".into(), cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |chat, _, cx| {
            assert_eq!(chat.input.read(cx).content.as_str(), "New draft");
            assert_eq!(chat.status, "Ready");
            assert!(!chat.busy && !chat.ticket_context.pending());
        })
        .unwrap();
}
#[gpui::test]
fn unknown_remote_capabilities_wait_for_loading_without_consuming_the_draft(
    cx: &mut TestAppContext,
) {
    let window = fixture(cx);
    cx.run_until_parked();
    window
        .update(cx, |chat, _, cx| {
            chat.summary.cwd = "remote://fixture/project".into();
            chat.loaded = false;
            chat.remote_state.online = false;
            chat.input
                .update(cx, |input, cx| input.set_text(REFERENCE.into(), cx));
            chat.send(cx);
            assert!(
                chat.status
                    .contains("Wait for this remote conversation to load")
            );
            assert!(!chat.status.contains("Update"));
            assert_eq!(chat.input.read(cx).content.as_str(), REFERENCE);
            assert!(!chat.busy && chat.ticket_context.pending.is_none());
        })
        .unwrap();
}
