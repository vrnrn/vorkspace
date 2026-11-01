use super::*;
use gpui::TestAppContext;
use vorkspace_core::store::Store;
fn saved() -> Value {
    json!({"cwd":"/project","text":"Keep 🙂 @mcp/docs","selectionStart":5,"selectionEnd":7,
            "attachments":[{"id":"photo-id","path":"/private/photo.png","name":"photo.png","kind":"image","mimeType":"image/png","size":42,"opaque":"retain"}],
            "mcpTags":[{"token":"@mcp/docs","opaque":"keep","server":{"provider":"codex","name":"docs","scope":"user","configPath":"/private/config.toml","transport":"stdio","enabled":true}}],"future":"preserve"})
}
#[gpui::test]
fn bot_recovery_restores_submission_mode_and_queue_intent(cx: &mut TestAppContext) {
    let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let summary = store.create("codex", "/project").unwrap();
    let chat = cx.new(|cx| Conversation::new(summary, Arc::new(Mutex::new(store)), cx));
    cx.run_until_parked();
    chat.update(cx, |chat, cx| {
        chat.configure_bot(
            vorkspace_core::bot::Bot::default(),
            std::env::temp_dir(),
            Default::default(),
            cx,
        );
        let mut payload = saved();
        payload["botSubmission"] = json!({"mode":"plan","queue":true});
        chat.install_composer_payload(&payload, cx);
        assert!(chat.settings.plan);
        assert!(chat.bot_ui.queue);
        assert_eq!(chat.input.read(cx).content, "Keep 🙂 @mcp/docs");
    });
}
#[gpui::test]
fn restoring_direct_submission_keeps_its_identity_until_acknowledged(cx: &mut TestAppContext) {
    let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let summary = store.create("codex", "/project").unwrap();
    let id = summary.id;
    let mut payload = saved();
    payload["botSubmission"] = json!({"mode":"plan", "queue":true});
    store
        .save_composer_recovery(
            id,
            vorkspace_core::store::ComposerRecovery {
                owner: "pending-send".into(),
                revision: 0,
                payload,
            },
        )
        .unwrap();
    let db = Arc::new(Mutex::new(store));
    let chat = cx.new(|cx| {
        let mut chat = Conversation::new(summary, db.clone(), cx);
        chat.configure_bot(
            vorkspace_core::bot::Bot::default(),
            std::env::temp_dir(),
            Default::default(),
            cx,
        );
        chat
    });
    cx.run_until_parked();
    chat.update(cx, |chat, cx| chat.resolve_composer_recovery(true, cx));
    cx.run_until_parked();
    chat.update(cx, |chat, cx| {
        let record = chat
            .pending_bot_submission(&chat.input.read(cx).content, &chat.attachments)
            .unwrap()
            .unwrap();
        assert_eq!(record.owner, "pending-send");
        assert!(chat.settings.plan);
        assert!(chat.bot_ui.queue);
        // Choosing the identical saved input must not discard its ID.
        chat.resolve_composer_recovery(false, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        locked(&db).unwrap().composer_recoveries(id).unwrap().len(),
        1
    );
}
#[gpui::test]
fn desktop_draft_restores_text_selection_files_and_mcp_then_saves_to_same_row(
    cx: &mut TestAppContext,
) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("vorkspace.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let id = session.id;
    store.save_composer_draft(id, 0, &saved()).unwrap();
    let db = Arc::new(Mutex::new(store));
    let chat = cx.new(|cx| Conversation::new(session.clone(), db.clone(), cx));
    cx.run_until_parked();
    assert_eq!(
        locked(&db)
            .unwrap()
            .composer_draft(id)
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    chat.update(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).content, "Keep 🙂 @mcp/docs");
        assert_eq!(chat.input.read(cx).selection_utf16(), 5..7);
        assert_eq!(
            chat.attachments,
            vec![std::path::PathBuf::from("/private/photo.png")]
        );
        assert!(
            chat.completion
                .read(cx)
                .prepare_prompt("Use @mcp/docs")
                .starts_with("MCP context:")
        );
        chat.input.update(cx, |input, cx| {
            input.set_text("Edited 🙂 @mcp/docs".into(), cx)
        });
        chat.capture_composer(cx);
        chat.composer_draft.flush().unwrap();
    });
    drop(chat);
    drop(db);
    let store = Store::open_vorkspace(&path).unwrap();
    let record = store.composer_draft(id).unwrap().unwrap();
    assert_eq!(record.revision, 2);
    assert_eq!(record.payload["text"], "Edited 🙂 @mcp/docs");
    assert_eq!(record.payload["attachments"], saved()["attachments"]);
    assert_eq!(record.payload["mcpTags"], saved()["mcpTags"]);
    assert_eq!(record.payload["future"], "preserve");
    let chat = cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(store)), cx));
    cx.run_until_parked();
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).content, "Edited 🙂 @mcp/docs")
    });
}
#[gpui::test]
fn typing_during_draft_load_wins_and_clear_keeps_a_revision_tombstone(cx: &mut TestAppContext) {
    let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let id = session.id;
    store.save_composer_draft(id, 0, &saved()).unwrap();
    let db = Arc::new(Mutex::new(store));
    let chat = cx.new(|cx| Conversation::new(session.clone(), db.clone(), cx));
    chat.update(cx, |chat, cx| {
        chat.input
            .update(cx, |input, cx| input.set_text("New intent".into(), cx));
        chat.persist_composer(cx);
    });
    cx.run_until_parked();
    chat.update(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).content, "New intent");
        assert!(chat.attachments.is_empty());
        chat.capture_composer(cx);
        chat.composer_draft.flush().unwrap();
        chat.input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        chat.capture_composer(cx);
        chat.composer_draft.flush().unwrap();
    });
    let saved = locked(&db).unwrap().composer_draft(id).unwrap().unwrap();
    assert!(saved.deleted);
    assert_eq!(saved.revision, 3);
    let reopened = cx.new(|cx| Conversation::new(session, db, cx));
    cx.run_until_parked();
    reopened.read_with(cx, |chat, cx| {
        assert!(chat.input.read(cx).content.is_empty())
    });
}
#[gpui::test]
fn quit_flush_before_load_completion_retains_the_latest_input(cx: &mut TestAppContext) {
    let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let id = session.id;
    store.save_composer_draft(id, 0, &saved()).unwrap();
    let db = Arc::new(Mutex::new(store));
    let chat = cx.new(|cx| Conversation::new(session, db.clone(), cx));
    chat.update(cx, |chat, cx| {
        chat.input.update(cx, |input, cx| {
            input.set_text("New text before restore completes".into(), cx)
        });
        chat.capture_composer(cx);
        chat.composer_draft.flush().unwrap();
    });
    cx.run_until_parked();
    let record = locked(&db).unwrap().composer_draft(id).unwrap().unwrap();
    assert_eq!(record.payload["text"], "New text before restore completes");
    assert_eq!(record.payload["future"], "preserve");
    chat.read_with(cx, |chat, cx| {
        assert_eq!(
            chat.input.read(cx).content,
            "New text before restore completes"
        )
    });
}
#[gpui::test]
fn same_mcp_token_saves_changed_server_identity_once_and_keeps_opaque_fields(
    cx: &mut TestAppContext,
) {
    let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let id = session.id;
    store.save_composer_draft(id, 0, &saved()).unwrap();
    let db = Arc::new(Mutex::new(store));
    let chat = cx.new(|cx| Conversation::new(session, db.clone(), cx));
    cx.run_until_parked();
    chat.update(cx, |chat, cx| {
        let mut tags = saved()["mcpTags"].clone();
        tags[0]["server"]["provider"] = json!("claude");
        tags[0]["server"]["configPath"] = json!("/other/config.json");
        chat.completion
            .update(cx, |completion, cx| completion.restore_mcp_tags(&tags, cx));
        assert!(chat.capture_composer(cx));
        chat.composer_draft.flush().unwrap();
        assert!(!chat.capture_composer(cx));
    });
    let draft = locked(&db).unwrap().composer_draft(id).unwrap().unwrap();
    assert_eq!(draft.revision, 2);
    assert_eq!(draft.payload["mcpTags"][0]["server"]["provider"], "claude");
    assert_eq!(
        draft.payload["mcpTags"][0]["server"]["configPath"],
        "/other/config.json"
    );
    assert_eq!(draft.payload["mcpTags"][0]["opaque"], "keep");
}
#[gpui::test]
fn conflicting_input_survives_close_reopen_and_explicit_restore_or_reload(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let id = session.id;
    store.save_composer_draft(id, 0, &saved()).unwrap();
    let db = Arc::new(Mutex::new(store));
    let chat = cx.new(|cx| Conversation::new(session.clone(), db.clone(), cx));
    cx.run_until_parked();
    let other = Store::open_vorkspace(&path).unwrap();
    let mut external = saved();
    external["text"] = json!("Other window input");
    other.save_composer_draft(id, 1, &external).unwrap();
    chat.update(cx, |chat, cx| {
        chat.input.update(cx, |input, cx| {
            input.set_text("My pending input".into(), cx)
        });
        chat.capture_composer(cx);
        assert!(chat.composer_draft.flush().is_err());
        assert!(chat.composer_recovery_banner(cx).is_some());
    });
    drop(chat);
    cx.run_until_parked();
    assert_eq!(
        other.composer_draft(id).unwrap().unwrap().payload["text"],
        "Other window input"
    );
    let reopened_store = Store::open_vorkspace(&path).unwrap();
    assert_eq!(reopened_store.composer_recoveries(id).unwrap().len(), 1);
    let reopened =
        cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(reopened_store)), cx));
    cx.run_until_parked();
    reopened.update(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).content, "Other window input");
        chat.resolve_composer_recovery(true, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        other.composer_draft(id).unwrap().unwrap().payload["text"],
        "My pending input"
    );
    assert!(other.composer_recoveries(id).unwrap().is_empty());
    external["text"] = json!("Newest other input");
    other.save_composer_draft(id, 3, &external).unwrap();
    reopened.update(cx, |chat, cx| {
        chat.input.update(cx, |input, cx| {
            input.set_text("My second pending input".into(), cx)
        });
        chat.capture_composer(cx);
        assert!(chat.composer_draft.flush().is_err());
        chat.resolve_composer_recovery(false, cx);
    });
    cx.run_until_parked();
    reopened.read_with(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).content, "Newest other input")
    });
    assert_eq!(other.composer_draft(id).unwrap().unwrap().revision, 4);
    assert!(other.composer_recoveries(id).unwrap().is_empty());
}
#[gpui::test]
fn selection_changes_during_recovery_publication_preserve_the_newer_composer(
    cx: &mut TestAppContext,
) {
    let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let id = session.id;
    store.save_composer_draft(id, 0, &saved()).unwrap();
    let mut recovery = saved();
    recovery["text"] = json!("Recovered different text");
    store
        .save_composer_recovery(
            id,
            vorkspace_core::store::ComposerRecovery {
                owner: "previous-window".into(),
                revision: 0,
                payload: recovery,
            },
        )
        .unwrap();
    let db = Arc::new(Mutex::new(store));
    let chat = cx.new(|cx| Conversation::new(session, db.clone(), cx));
    cx.run_until_parked();
    chat.update(cx, |chat, cx| {
        chat.resolve_composer_recovery(true, cx);
        chat.input
            .update(cx, |input, cx| input.select_utf16(0..4, cx));
    });
    cx.run_until_parked();
    chat.update(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).content, "Keep 🙂 @mcp/docs");
        assert_eq!(chat.input.read(cx).selection_utf16(), 0..4);
        chat.capture_composer(cx);
        chat.composer_draft.flush().unwrap();
    });
    let record = locked(&db).unwrap().composer_draft(id).unwrap().unwrap();
    assert_eq!(record.payload["selectionStart"], 0);
    assert_eq!(record.payload["selectionEnd"], 4);
}
