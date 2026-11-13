use super::*;
use serde_json::json;

fn payload() -> Value {
    json!({"cwd":"/project","text":"Saved 🙂 draft @mcp/docs","selectionStart":6,"selectionEnd":8,
            "attachments":[{"id":"private-image","path":"/private/photo.png","kind":"image","mimeType":"image/png","size":42,"future":"retain"}],
            "mcpTags":[{"token":"@mcp/docs","server":{"provider":"codex","name":"docs","scope":"user","configPath":"/private/config.toml","transport":"stdio","enabled":true}}],
            "future":{"keep":"opaque"}})
}

fn reserve_draft(store: &Store, id: i64, origin: &ComposerOrigin, attempt: &str) {
    let user_turn_id = origin.message_id();
    store
        .reserve_local_submission(
            id,
            SubmissionInput {
                attempt_id: attempt.into(),
                user_turn_id: user_turn_id.clone(),
                session_id: String::new(),
                payload_json: json!({"text": payload()["text"]}).to_string(),
                user_turn: Some(
                    json!({"id": user_turn_id, "role": "user", "text": payload()["text"]}),
                ),
            },
        )
        .unwrap();
}

#[test]
fn dispatch_transfers_only_the_offered_composer_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let open = { Store::test };
    let store = open(&path).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let revision = store
        .save_composer_draft(session.id, 0, &payload())
        .unwrap();
    let origin = store.composer_origin(session.id, revision).unwrap();
    reserve_draft(&store, session.id, &origin, "first-attempt");
    assert!(!store.composer_draft(session.id).unwrap().unwrap().deleted);
    assert!(
        store
            .dispatch_composer_submission("first-attempt", &origin)
            .unwrap()
            .claimed
    );
    drop(store);
    let store = open(&path).unwrap();
    let consumed = store.composer_draft(session.id).unwrap().unwrap();
    assert!(consumed.deleted);
    assert_eq!(consumed.revision, revision);
    assert_eq!(consumed.payload, payload());
    assert!(store.composer_origin(session.id, revision).is_err());

    let mut successor = payload();
    successor["text"] = json!("A newer draft");
    let revision = store
        .save_composer_draft(session.id, revision, &successor)
        .unwrap();
    assert!(
        !store
            .dispatch_composer_submission("first-attempt", &origin)
            .unwrap()
            .claimed
    );
    let origin = store.composer_origin(session.id, revision).unwrap();
    reserve_draft(&store, session.id, &origin, "second-attempt");
    successor["text"] = json!("Another edit");
    store
        .save_composer_draft(session.id, revision, &successor)
        .unwrap();
    assert!(
        store
            .dispatch_composer_submission("second-attempt", &origin)
            .unwrap()
            .claimed
    );
    let retained = store.composer_draft(session.id).unwrap().unwrap();
    assert!(!retained.deleted);
    assert_eq!(retained.payload, successor);
}

#[test]
fn failed_draft_transfer_rolls_back_the_local_dispatch_claim() {
    let store = Store::test(std::path::Path::new(":memory:")).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let revision = store
        .save_composer_draft(session.id, 0, &payload())
        .unwrap();
    let origin = store.composer_origin(session.id, revision).unwrap();
    reserve_draft(&store, session.id, &origin, "retry-after-write-failure");
    store
        .conn
        .execute_batch(
            "CREATE TRIGGER fail_transfer BEFORE UPDATE OF deleted ON composer_drafts
         WHEN NEW.deleted=1 BEGIN SELECT RAISE(ABORT, 'simulated disk error'); END;",
        )
        .unwrap();
    assert!(store
        .dispatch_composer_submission("retry-after-write-failure", &origin)
        .is_err());
    assert_eq!(
        store.local_submissions(session.id).unwrap()[0].phase,
        "reserved"
    );
    assert!(!store.composer_draft(session.id).unwrap().unwrap().deleted);
    store
        .conn
        .execute_batch("DROP TRIGGER fail_transfer;")
        .unwrap();
    assert!(
        store
            .dispatch_composer_submission("retry-after-write-failure", &origin)
            .unwrap()
            .claimed
    );
    assert!(store.composer_draft(session.id).unwrap().unwrap().deleted);
}

#[test]
fn a_distinct_queue_entry_can_commit_while_the_older_turn_is_offered() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let open = { Store::test };
    let store = open(&path).unwrap();
    let session = store.create("codex", "/project").unwrap().id;
    let revision = store.save_composer_draft(session, 0, &payload()).unwrap();
    let first = store.composer_origin(session, revision).unwrap();
    reserve_draft(&store, session, &first, "older-turn");
    assert!(store
        .save_composer_after_local_offer(&first, &Value::Null)
        .is_err());
    assert!(store.composer_recoveries(session).unwrap().is_empty());

    let mut newer = payload();
    newer["text"] = json!("A distinct follow-up");
    let (revision, recovery) = store
        .save_composer_after_local_offer(&first, &newer)
        .unwrap();
    assert_eq!(recovery.unwrap().payload, payload());
    let second = store.composer_origin(session, revision).unwrap();
    let queue = crate::follow_up::State {
        revision: 1,
        paused: true,
        messages: vec![crate::follow_up::Message {
            id: "queue-next".into(),
            text: "A distinct follow-up".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    store
        .accept_composer_message(session, &queue, &second, "queue-next")
        .unwrap();
    assert!(!store.composer_transfer_committed(&first).unwrap());
    assert!(store.composer_transfer_committed(&second).unwrap());
    drop(store);

    let reopened = open(&path).unwrap();
    let records = reopened.composer_recoveries(session).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].payload, payload());
    assert_eq!(
        reopened.follow_ups(session).unwrap().messages,
        queue.messages
    );
    assert!(
        reopened
            .dispatch_composer_submission("older-turn", &first)
            .unwrap()
            .claimed
    );
    assert!(reopened.composer_recoveries(session).unwrap().is_empty());
    assert_eq!(
        reopened.composer_draft(session).unwrap().unwrap().payload,
        newer
    );
}

#[test]
fn a_queued_version_cannot_also_dispatch_as_an_initial_turn() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let open = { Store::test };
    let editor = open(&path).unwrap();
    let worker = open(&path).unwrap();
    let session = editor.create("codex", "/project").unwrap();
    {
        worker
            .resolve_session_id(&editor.persisted_session_id(session.id).unwrap())
            .unwrap();
    }
    let revision = editor
        .save_composer_draft(session.id, 0, &payload())
        .unwrap();
    let origin = editor.composer_origin(session.id, revision).unwrap();
    reserve_draft(&worker, session.id, &origin, "other-window-attempt");
    let mut successor = payload();
    successor["text"] = json!("A distinct input");
    editor
        .save_composer_draft(session.id, revision, &successor)
        .unwrap();
    let mut queue = editor.follow_ups(session.id).unwrap();
    queue.revision += 1;
    queue.messages.push(crate::follow_up::Message {
        id: "queue-owner".into(),
        text: payload()["text"].as_str().unwrap().into(),
        ..Default::default()
    });
    editor
        .accept_composer_message(session.id, &queue, &origin, "queue-owner")
        .unwrap();
    assert!(worker.composer_transfer_committed(&origin).unwrap());
    assert!(worker
        .dispatch_composer_submission("other-window-attempt", &origin)
        .is_err());
    assert_eq!(
        worker.local_submissions(session.id).unwrap()[0].phase,
        "reserved"
    );
    let reopened = open(&path).unwrap();
    let queue = reopened.follow_ups(session.id).unwrap();
    assert_eq!(queue.messages.len(), 1);
    assert_eq!(queue.messages[0].id, "queue-owner");
    let draft = reopened.composer_draft(session.id).unwrap().unwrap();
    assert!(!draft.deleted);
    assert_eq!(draft.payload, successor);
}
#[test]
fn saved_input_roundtrips_in_its_existing_table_without_sidecar_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let source = store.persisted_session_id(session.id).unwrap();
    store.conn.execute_batch("CREATE TABLE composer_drafts(session_id TEXT PRIMARY KEY,cwd TEXT NOT NULL,revision INTEGER NOT NULL,payload_json TEXT NOT NULL,deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER NOT NULL);").unwrap();
    store
        .conn
        .execute(
            "INSERT INTO composer_drafts VALUES(?1,'/project',7,?2,0,1)",
            params![source, payload().to_string()],
        )
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let saved = store.composer_draft(session.id).unwrap().unwrap();
    assert_eq!(saved.revision, 7);
    assert_eq!(saved.payload, payload());
    assert!(!saved.deleted);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let mut changed = saved.payload;
    changed["text"] = json!("Edited 🙂 draft @mcp/docs");
    changed["selectionEnd"] = json!(9);
    assert_eq!(
        store.save_composer_draft(session.id, 7, &changed).unwrap(),
        8
    );
    drop(store);
    let reopened = Store::open_vorkspace(&path).unwrap();
    let saved = reopened.composer_draft(session.id).unwrap().unwrap();
    assert_eq!(saved.payload, changed);
    assert_eq!(saved.revision, 8);
    assert_eq!(
        reopened
            .conn
            .query_row("SELECT session_id FROM composer_drafts", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        source
    );
    assert!(!path.with_file_name("desktop.db.gpui.json").exists());
}
#[test]
fn stale_writer_cannot_replace_or_clear_successor_input() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.db");
    let first = Store::open_vorkspace(&path).unwrap();
    let session = first.create("codex", "/project").unwrap();
    let second = Store::open_vorkspace(&path).unwrap();
    assert_eq!(
        first
            .save_composer_draft(session.id, 0, &payload())
            .unwrap(),
        1
    );
    let mut cleared = payload();
    cleared["text"] = json!("");
    cleared["selectionStart"] = json!(0);
    cleared["selectionEnd"] = json!(0);
    cleared["attachments"] = json!([]);
    cleared["mcpTags"] = json!([]);
    assert!(second.save_composer_draft(session.id, 0, &cleared).is_err());
    assert_eq!(
        first.composer_draft(session.id).unwrap().unwrap().payload,
        payload()
    );
    assert_eq!(
        first.save_composer_draft(session.id, 1, &cleared).unwrap(),
        2
    );
    let tombstone = second.composer_draft(session.id).unwrap().unwrap();
    assert_eq!(tombstone.revision, 2);
    assert!(tombstone.deleted);
    assert!(second
        .save_composer_draft(session.id, 1, &payload())
        .is_err());
}
#[test]
fn missing_draft_table_is_read_without_creation_and_failed_writes_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let session = store.create("codex", "/project").unwrap();
    assert!(store.composer_draft(session.id).unwrap().is_none());
    assert!(!sessions::has_table(&store.conn, "composer_drafts").unwrap());
    assert!(store
        .save_composer_draft(session.id, 1, &payload())
        .is_err());
    assert!(!sessions::has_table(&store.conn, "composer_drafts").unwrap());
    let reader = Store::read_vorkspace(&path).unwrap();
    assert!(reader
        .save_composer_draft(session.id, 0, &payload())
        .is_err());
    assert!(!sessions::has_table(&store.conn, "composer_drafts").unwrap());
}
#[test]
fn malformed_or_unavailable_draft_fields_are_preserved_for_recovery() {
    let store = Store::test(Path::new(":memory:")).unwrap();
    let session = store.create("codex", "/project").unwrap();
    store
        .save_composer_draft(session.id, 0, &payload())
        .unwrap();
    let source = store.persisted_session_id(session.id).unwrap();
    let mut unavailable = payload();
    unavailable["attachments"][0]
        .as_object_mut()
        .unwrap()
        .remove("path");
    let mut invalid = payload();
    invalid["selectionEnd"] = json!(9999);
    let mut invalid_mcp = payload();
    invalid_mcp["mcpTags"][0]["server"]
        .as_object_mut()
        .unwrap()
        .remove("configPath");
    for raw in [
        "{".to_owned(),
        unavailable.to_string(),
        invalid.to_string(),
        invalid_mcp.to_string(),
    ] {
        store
            .conn
            .execute(
                "UPDATE composer_drafts SET payload_json=?2 WHERE session_id=?1",
                params![source, raw],
            )
            .unwrap();
        assert!(store.composer_draft(session.id).is_err());
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT payload_json FROM composer_drafts WHERE session_id=?1",
                    [&source],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            raw
        );
    }
}
#[test]
fn independent_writers_keep_distinct_recovery_rows_and_clear_only_their_owner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.db");
    let mut first = Store::open_vorkspace(&path).unwrap();
    let session = first.create("codex", "/project").unwrap();
    let second = Store::open_vorkspace(&path).unwrap();
    assert!(first.composer_recoveries(session.id).unwrap().is_empty());
    assert!(!sessions::has_table(&first.conn, "composer_draft_recoveries").unwrap());
    for (store, owner) in [(&first, "writer-one"), (&second, "writer-two")] {
        store
            .save_composer_recovery(
                session.id,
                ComposerRecovery {
                    owner: owner.into(),
                    revision: 7,
                    payload: payload(),
                },
            )
            .unwrap();
    }
    assert_eq!(first.composer_recoveries(session.id).unwrap().len(), 2);
    first
        .clear_composer_recovery(session.id, "writer-one")
        .unwrap();
    assert_eq!(
        second.composer_recoveries(session.id).unwrap()[0].owner,
        "writer-two"
    );
    drop(second);
    let reopened = Store::open_vorkspace(&path).unwrap();
    assert_eq!(reopened.composer_recoveries(session.id).unwrap().len(), 1);
    drop(reopened);
    first.delete(session.id).unwrap();
    assert_eq!(
        first
            .conn
            .query_row(
                "SELECT COUNT(*) FROM composer_draft_recoveries",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}
