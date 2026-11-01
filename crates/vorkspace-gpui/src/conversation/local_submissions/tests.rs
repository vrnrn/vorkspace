use super::*;
use gpui::TestAppContext;
use std::sync::Mutex;
use vorkspace_core::store::{Store, SubmissionInput};
fn fixture(cx: &mut TestAppContext) -> (tempfile::TempDir, Database, Entity<Conversation>, i64) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_vorkspace(&directory.path().join("sessions.db")).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let id = session.id;
    store.reserve_local_submission(id,SubmissionInput{attempt_id:"unconfirmed".into(),user_turn_id:"saved-message".into(),session_id:String::new(),payload_json:"{}".into(),user_turn:Some(serde_json::json!({"id":"saved-message","role":"user","text":"Original request","attachments":[{"id":"image","name":"private.png","kind":"image","mimeType":"image/png","size":7,"path":"/private/cached.png"}]}))}).unwrap();
    store.dispatch_local_submission("unconfirmed").unwrap();
    store
        .settle_local_submission("unconfirmed", "uncertain", Some("Disconnected".into()))
        .unwrap();
    let db = Arc::new(Mutex::new(store));
    let chat = cx.new(|cx| Conversation::new(session, db.clone(), cx));
    cx.run_until_parked();
    chat.update(cx, |chat, cx| chat.refresh_submission_recovery(cx));
    cx.run_until_parked();
    (directory, db, chat, id)
}
#[gpui::test]
fn recovery_copy_is_explicit_durable_and_preserves_typing_after_the_click(cx: &mut TestAppContext) {
    let (_directory, db, chat, id) = fixture(cx);
    chat.update(cx, |chat, cx| {
        assert!(chat.local_submission_banner(cx).is_some());
        chat.input
            .update(cx, |input, cx| input.set_text("New user input".into(), cx));
        chat.review_local_submission(true, cx);
        assert_eq!(chat.input.read(cx).content, "New user input");
        assert!(!chat.reviewing_submission);
        chat.input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        chat.review_local_submission(true, cx);
        assert_eq!(chat.input.read(cx).content, "Original request");
        assert_eq!(
            chat.attachments,
            [std::path::PathBuf::from("/private/cached.png")]
        );
        chat.input.update(cx, |input, cx| {
            input.set_text("Original request with more detail".into(), cx);
            input.select_utf16(0..8, cx);
        });
    });
    cx.run_until_parked();
    chat.update(cx, |chat, cx| {
        assert_eq!(
            chat.input.read(cx).content,
            "Original request with more detail"
        );
        assert_eq!(chat.input.read(cx).selection_utf16(), 0..8);
        chat.capture_composer(cx);
        chat.composer_draft.flush().unwrap();
        assert!(chat.local_submission_banner(cx).is_none());
    });
    let store = locked(&db).unwrap();
    assert_eq!(
        store.composer_draft(id).unwrap().unwrap().payload["text"],
        "Original request with more detail"
    );
    let receipt = &store.local_submissions(id).unwrap()[0];
    assert_eq!(receipt.phase, "uncertain");
    assert_eq!(receipt.outcome.as_deref(), Some("reviewed"));
    assert_eq!(
        receipt.user_turn.as_ref().unwrap()["text"],
        "Original request"
    );
}
#[gpui::test]
fn delayed_refresh_cannot_republish_a_dismissed_notice(cx: &mut TestAppContext) {
    let (_directory, _db, chat, _id) = fixture(cx);
    let (epoch, receipt) = chat.update(cx, |chat, cx| {
        let epoch = chat.submission_recovery_epoch;
        let receipt = chat.unconfirmed_submission.clone();
        chat.review_local_submission(false, cx);
        (epoch, receipt)
    });
    cx.run_until_parked();
    chat.update(cx, |chat, cx| {
        assert!(chat.unconfirmed_submission.is_none());
        chat.install_submission_recovery(epoch, Ok(receipt), cx);
        assert!(chat.unconfirmed_submission.is_none());
    });
}

#[gpui::test]
fn recovery_cannot_replace_a_saved_draft_before_its_initial_restoration(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_vorkspace(&directory.path().join("sessions.db")).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let id = session.id;
    let saved = serde_json::json!({
        "cwd":"/project", "text":"Unseen saved draft", "selectionStart":3,
        "selectionEnd":9, "attachments":[{"path":"/private/saved.png"}], "mcpTags":[]
    });
    store.save_composer_draft(id, 0, &saved).unwrap();
    store.reserve_local_submission(id,SubmissionInput{attempt_id:"pending-read".into(),user_turn_id:"receipt-copy".into(),session_id:String::new(),payload_json:"{}".into(),user_turn:Some(serde_json::json!({"id":"receipt-copy","role":"user","text":"Unconfirmed request"}))}).unwrap();
    store.dispatch_local_submission("pending-read").unwrap();
    store
        .settle_local_submission("pending-read", "uncertain", None)
        .unwrap();
    let recovery = store.latest_unconfirmed_submission(id).unwrap();
    let db = Arc::new(Mutex::new(store));
    let chat = cx.new(|cx| Conversation::new(session, db.clone(), cx));
    // Hold the scheduled initial draft read/publication. The independent
    // receipt read is allowed to finish first, while visible input is empty.
    chat.update(cx, |chat, cx| {
        chat.unconfirmed_submission = recovery;
        assert!(chat.input.read(cx).content.is_empty());
        assert!(!chat.composer_draft.ready_for_replacement());
        assert!(chat.local_submission_banner(cx).is_some());
        chat.review_local_submission(true, cx);
        assert!(!chat.reviewing_submission);
        assert!(chat.input.read(cx).content.is_empty());
    });
    assert_eq!(
        locked(&db)
            .unwrap()
            .composer_draft(id)
            .unwrap()
            .unwrap()
            .payload,
        saved
    );
    cx.run_until_parked();
    chat.update(cx, |chat, cx| {
        assert!(chat.composer_draft.ready_for_replacement());
        assert_eq!(chat.input.read(cx).content, "Unseen saved draft");
        assert_eq!(chat.input.read(cx).selection_utf16(), 3..9);
        assert_eq!(
            chat.attachments,
            [std::path::PathBuf::from("/private/saved.png")]
        );
        chat.review_local_submission(true, cx);
        assert!(!chat.reviewing_submission);
        assert_eq!(chat.input.read(cx).content, "Unseen saved draft");
    });
    let store = locked(&db).unwrap();
    assert_eq!(store.composer_draft(id).unwrap().unwrap().revision, 1);
    assert_eq!(store.composer_draft(id).unwrap().unwrap().payload, saved);
    assert!(store.latest_unconfirmed_submission(id).unwrap().is_some());
}

#[gpui::test]
fn recovery_cannot_replace_an_in_flight_attachment_selection(cx: &mut TestAppContext) {
    let (_directory, db, chat, id) = fixture(cx);
    chat.update(cx, |chat, cx| {
        assert!(chat.composer_draft.ready_for_replacement());
        chat.attaching = true;
        chat.review_local_submission(true, cx);
        assert!(!chat.reviewing_submission);
        assert!(chat.input.read(cx).content.is_empty());
        assert!(chat.attachments.is_empty());
        chat.attaching = false;
        chat.pending_attachments
            .push("/private/selected.png".into());
        chat.review_local_submission(true, cx);
        assert!(!chat.reviewing_submission);
        assert!(chat.input.read(cx).content.is_empty());
        assert_eq!(
            chat.pending_attachments,
            [std::path::PathBuf::from("/private/selected.png")]
        );
        assert!(chat.attachments.is_empty());
    });
    cx.run_until_parked();
    assert!(
        locked(&db)
            .unwrap()
            .latest_unconfirmed_submission(id)
            .unwrap()
            .is_some()
    );
}
