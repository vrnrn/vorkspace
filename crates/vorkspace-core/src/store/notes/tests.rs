use super::*;

fn store() -> Store {
    Store::open(Path::new(":memory:")).unwrap()
}
fn input(id: &str, title: &str, body: &str) -> NoteUpsert {
    NoteUpsert {
        id: id.into(),
        title: title.into(),
        body: body.into(),
        ..Default::default()
    }
}
fn schema_version(store: &Store) -> i64 {
    store
        .conn
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn desktop_note_crud_keeps_source_tags_and_project_moves() {
    let store = store();
    let session = store.create("codex", "/project").unwrap();
    let mut request = input("saved-note", "  A title  ", "# Heading\r\n\r\nBody\rtext");
    request.tags = vec![
        " #Design Notes ".into(),
        "design-notes".into(),
        "Rust".into(),
    ];
    request.source_session_id = Some(session.id.to_string());
    request.source_cwd = Some("/project".into());
    let saved = store.upsert_note(&request).unwrap();
    assert_eq!(saved.title, "A title");
    assert_eq!(saved.body, "# Heading\n\nBody\ntext");
    assert_eq!(saved.tags, ["design-notes", "rust"]);
    assert_eq!(store.note_source_session(&saved), Some(session.id));
    store
        .conn
        .execute("UPDATE notes SET updated_at=42 WHERE id=?1", [&saved.id])
        .unwrap();
    let saved = store.get_note(&saved.id).unwrap().unwrap();
    let mut update = saved.upsert(false);
    update.source_cwd = Some("/different".into());
    update.source_session_id = Some("other-source".into());
    let moved = store.upsert_note(&update).unwrap();
    assert_eq!(moved.source_cwd.as_deref(), Some("/different"));
    assert_eq!(moved.updated_at, 42);
    assert_eq!(moved.source_session_id, saved.source_session_id);
    update.source_cwd = None;
    assert_eq!(
        store.upsert_note(&update).unwrap().source_cwd,
        moved.source_cwd
    );
    update.source_cwd = Some("~".into());
    assert!(store.upsert_note(&update).unwrap().project_name().is_none());
    store.delete_note(&saved.id).unwrap();
    assert!(store.list_notes().unwrap().is_empty());
}

#[test]
fn pending_slugs_finalize_once_and_do_not_steal_existing_references() {
    let store = store();
    let established = store
        .upsert_note(&input("established", "Real title", "Body"))
        .unwrap();
    let mut pending = store.upsert_note(&input("pending", "", "")).unwrap();
    assert!(pending.slug_pending);
    pending.title = "Real title".into();
    let typed = store.upsert_note(&pending.upsert(false)).unwrap();
    assert_eq!(typed.slug, pending.slug);
    assert!(typed.slug_pending);
    let finalized = store.upsert_note(&typed.upsert(true)).unwrap();
    assert_eq!(finalized.slug, "real-title-2");
    assert!(!finalized.slug_pending);
    assert_eq!(
        store.get_note(&established.id).unwrap().unwrap().slug,
        "real-title"
    );
    let mut rename = finalized.upsert(true);
    rename.title = "Different title".into();
    assert_eq!(store.upsert_note(&rename).unwrap().slug, finalized.slug);
    let mut placeholder = store
        .upsert_note(&input("placeholder", "", ""))
        .unwrap()
        .upsert(true);
    placeholder.title = "Untitled 2".into();
    assert!(store.upsert_note(&placeholder).unwrap().slug_pending);
}

#[test]
fn artifacts_are_excluded_and_cannot_be_overwritten_by_notes() {
    let store = store();
    ensure_notes_table(&store.conn).unwrap();
    let artifact = input("artifact", "Document", "Artifact text");
    upsert_content(&store.conn, &artifact, "artifact", Some("document")).unwrap();
    let note = store
        .upsert_note(&input("note", "Note", "Note text"))
        .unwrap();
    assert_eq!(store.list_notes().unwrap(), [note]);
    assert!(store.get_note("artifact").unwrap().is_none());
    assert!(store.upsert_note(&artifact).is_err());
    store.delete_note("artifact").unwrap();
    let count: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM notes WHERE id='artifact'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn note_mentions_expand_in_reference_order_once_and_ignore_missing_slugs() {
    let store = store();
    let a = store.upsert_note(&input("alpha", "Alpha", "A")).unwrap();
    let b = store.upsert_note(&input("beta", "Beta", "B")).unwrap();
    let prompt = "Use @note/beta and @note/alpha and @note/beta @note/missing";
    assert_eq!(
        store.apply_notes_to_turn(prompt).unwrap(),
        crate::notes::inject_prompt(prompt, &[b, a])
    );
    assert_eq!(
        store.apply_notes_to_turn("@note/missing").unwrap(),
        "@note/missing"
    );
}

#[test]
fn response_note_facade_and_legacy_settings_share_one_saved_identity() {
    let store = store();
    let session = store.create("codex", "/project").unwrap();
    let mut response = crate::response::Note {
        id: 123,
        title: "Response".into(),
        body: "Saved response".into(),
        tags: vec!["tag".into()],
        source_session: session.id,
        source_cwd: "/project".into(),
    };
    store
        .conn
        .execute(
            "INSERT INTO native_settings(key,value) VALUES('note:123',?1)",
            [serde_json::to_string(&response).unwrap()],
        )
        .unwrap();
    assert_eq!(store.list_notes().unwrap().len(), 1);
    store.save_note(&response).unwrap();
    response.body = "Edited response".into();
    store.save_note(&response).unwrap();
    assert_eq!(store.list_notes().unwrap().len(), 1);
    let compatible = store.notes().unwrap();
    assert_eq!(compatible.len(), 1);
    assert_eq!(compatible[0].id, response.id);
    assert_eq!(compatible[0].body, response.body);
    assert_eq!(compatible[0].source_session, session.id);
    store.delete_note("gpui-note-123").unwrap();
    assert!(store.notes().unwrap().is_empty());
}

#[test]
fn shared_response_bridge_preserves_original_identity_slug_and_subtype() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_vorkspace(&directory.path().join("vorkspace.db")).unwrap();
    store.conn.execute("INSERT INTO notes(id,slug,title,body,created_at,updated_at,artifact_kind) VALUES('source-id','stable','Saved','Body',1,2,'document')", []).unwrap();
    let mut compatible = store.notes().unwrap().pop().unwrap();
    compatible.body = "Changed".into();
    store.save_note(&compatible).unwrap();
    let saved = store.get_note("source-id").unwrap().unwrap();
    assert_eq!(saved.slug, "stable");
    assert_eq!(saved.body, "Changed");
    assert_eq!(store.list_notes().unwrap().len(), 1);
    let subtype: String = store
        .conn
        .query_row(
            "SELECT artifact_kind FROM notes WHERE id='source-id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(subtype, "document");
}

const RECORDING_A: &str = "0123456789abcdef0123456789abcdef";
const RECORDING_B: &str = "1123456789abcdef0123456789abcdef";

#[test]
fn recording_links_read_does_not_create_schema() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("vorkspace.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let before = schema_version(&store);
    assert!(store.note_recordings().unwrap().is_empty());
    assert_eq!(schema_version(&store), before);
    assert!(!super::super::shared::has_table(&store.conn, "note_recordings").unwrap());
    let reader = Store::read_vorkspace(&path).unwrap();
    assert!(reader.note_recordings().unwrap().is_empty());
    assert!(reader.attach_note_recording("note", RECORDING_A).is_err());
    assert!(reader
        .import_recording_note(RECORDING_A, "Title", 1)
        .is_err());
    assert_eq!(schema_version(&reader), before);
    store.attach_note_recording("note", RECORDING_A).unwrap();
    assert_eq!(
        reader.note_recordings().unwrap(),
        store.note_recordings().unwrap()
    );
}

#[test]
fn recording_note_import_is_idempotent_and_preserves_edits_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("vorkspace.db");
    let expected = {
        let store = Store::open_vorkspace(&path).unwrap();
        let imported = store
            .import_recording_note(RECORDING_A, "  Planning meeting  ", 1234)
            .unwrap()
            .unwrap();
        assert_eq!(imported.id, format!("meeting-{RECORDING_A}"));
        assert_eq!(imported.title, "Planning meeting");
        assert!(imported.body.is_empty());
        assert_eq!((imported.created_at, imported.updated_at), (1234, 1234));
        assert!(!imported.slug_pending);
        let mut edited = imported.upsert(true);
        edited.title = "My planning notes".into();
        edited.body = "Only my own notes belong here.".into();
        edited.tags = vec!["design".into(), "follow-up".into()];
        edited.source_cwd = Some("/project".into());
        edited.source_session_id = Some("source-session".into());
        store.upsert_note(&edited).unwrap();
        store
            .conn
            .execute(
                "UPDATE notes SET updated_at = 5678 WHERE id = ?1",
                [&imported.id],
            )
            .unwrap();
        let expected = store.get_note(&imported.id).unwrap().unwrap();
        assert_eq!(expected.slug, imported.slug);
        assert_eq!(
            store
                .import_recording_note(RECORDING_A, "Provider renamed the meeting", 9999)
                .unwrap(),
            Some(expected.clone())
        );
        expected
    };
    let reopened = Store::open_vorkspace(&path).unwrap();
    assert_eq!(
        reopened
            .import_recording_note(RECORDING_A, "Different title", 0)
            .unwrap(),
        Some(expected.clone())
    );
    assert_eq!(
        reopened.list_notes().unwrap().as_slice(),
        std::slice::from_ref(&expected)
    );
    assert_eq!(
        reopened.note_recordings().unwrap(),
        [NoteRecording {
            note_id: expected.id,
            recording_id: RECORDING_A.into(),
            deleted: false,
        }]
    );
}

#[test]
fn recording_import_reuses_an_existing_deterministic_note_without_overwriting_it() {
    let store = store();
    let note_id = format!("meeting-{RECORDING_A}");
    let mut edited = input(&note_id, "Already edited", "Keep this body");
    edited.tags = vec!["keep".into()];
    edited.source_cwd = Some("/already-associated".into());
    edited.source_session_id = Some("source-session".into());
    let existing = store.upsert_note(&edited).unwrap();
    assert_eq!(
        store
            .import_recording_note(RECORDING_A, "Recording title", 100)
            .unwrap(),
        Some(existing.clone())
    );
    assert_eq!(store.list_notes().unwrap(), [existing]);
    assert_eq!(store.note_recordings().unwrap().len(), 1);
}

#[test]
fn reserved_recording_identity_recovers_an_unsaved_note_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("vorkspace.db");
    {
        let store = Store::open_vorkspace(&path).unwrap();
        store
            .attach_note_recording("ephemeral-note", RECORDING_A)
            .unwrap();
        store
            .attach_note_recording("ephemeral-note", RECORDING_A)
            .unwrap();
        assert!(store.list_notes().unwrap().is_empty());
        assert_eq!(store.note_recordings().unwrap().len(), 1);
    }
    let reopened = Store::open_vorkspace(&path).unwrap();
    let recovered = reopened
        .import_recording_note(RECORDING_A, "Recovered meeting", 42)
        .unwrap()
        .unwrap();
    assert_eq!(recovered.id, "ephemeral-note");
    assert_eq!(recovered.title, "Recovered meeting");
    assert_eq!((recovered.created_at, recovered.updated_at), (42, 42));
    assert!(recovered.body.is_empty());
    assert_eq!(
        reopened
            .import_recording_note(RECORDING_A, "Changed", 99)
            .unwrap(),
        Some(recovered)
    );
}

#[test]
fn deleted_recording_note_and_stale_saves_cannot_resurrect_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("vorkspace.db");
    let note_id = {
        let store = Store::open_vorkspace(&path).unwrap();
        let stale_store = Store::open_vorkspace(&path).unwrap();
        let note = store
            .import_recording_note(RECORDING_A, "A meeting", 42)
            .unwrap()
            .unwrap();
        let stale_note = stale_store.get_note(&note.id).unwrap().unwrap();
        store.delete_note(&note.id).unwrap();
        assert!(stale_store.upsert_note(&stale_note.upsert(true)).is_err());
        assert_eq!(
            stale_store
                .import_recording_note(RECORDING_A, "A meeting", 42)
                .unwrap(),
            None
        );
        store.delete_note(&note.id).unwrap();
        note.id
    };
    let reopened = Store::open_vorkspace(&path).unwrap();
    assert!(reopened.get_note(&note_id).unwrap().is_none());
    assert_eq!(
        reopened
            .import_recording_note(RECORDING_A, "Recovered", 99)
            .unwrap(),
        None
    );
    assert!(reopened
        .attach_note_recording(&note_id, RECORDING_A)
        .is_err());
    assert!(reopened
        .attach_note_recording("another-note", RECORDING_A)
        .is_err());
    assert!(reopened
        .attach_note_recording(&note_id, RECORDING_B)
        .is_err());
    assert_eq!(
        reopened.note_recordings().unwrap(),
        [NoteRecording {
            note_id,
            recording_id: RECORDING_A.into(),
            deleted: true,
        }]
    );
}

#[test]
fn deleting_a_reserved_recording_note_tombstones_without_a_note_row() {
    let store = store();
    store
        .attach_note_recording("unsaved-note", RECORDING_A)
        .unwrap();
    store.delete_note("unsaved-note").unwrap();
    assert!(store.list_notes().unwrap().is_empty());
    assert!(store.note_recordings().unwrap()[0].deleted);
    assert_eq!(
        store
            .import_recording_note(RECORDING_A, "Must stay deleted", 100)
            .unwrap(),
        None
    );
    assert!(store
        .upsert_note(&input("unsaved-note", "Stale save", "Body"))
        .is_err());
}

#[test]
fn recording_attachment_rejects_relinking_and_artifact_ownership() {
    let store = store();
    store
        .attach_note_recording("first-note", RECORDING_A)
        .unwrap();
    assert!(store
        .attach_note_recording("second-note", RECORDING_A)
        .is_err());
    assert!(store
        .attach_note_recording("first-note", RECORDING_B)
        .is_err());
    let artifact = input("artifact", "Artifact", "Document body");
    upsert_content(&store.conn, &artifact, "artifact", Some("document")).unwrap();
    assert!(store
        .attach_note_recording("artifact", RECORDING_B)
        .is_err());
    assert_eq!(store.note_recordings().unwrap().len(), 1);
    assert_eq!(store.note_recordings().unwrap()[0].note_id, "first-note");
}

#[test]
fn recording_link_ids_are_validated_before_any_schema_write() {
    let store = store();
    for invalid in ["", "../escape", "space here", "bad/identity", "nöté"] {
        assert!(store.attach_note_recording(invalid, RECORDING_A).is_err());
    }
    for invalid in [
        "",
        "0123456789abcdef",
        "0123456789abcdef0123456789abcde/",
        "0123456789abcdef0123456789abcdeg",
        "0123456789abcdef0123456789abcdef0",
    ] {
        assert!(store.attach_note_recording("note", invalid).is_err());
        assert!(store.import_recording_note(invalid, "Title", 0).is_err());
    }
    assert!(!super::super::shared::has_table(&store.conn, "notes").unwrap());
    assert!(!super::super::shared::has_table(&store.conn, "note_recordings").unwrap());
}

#[test]
fn external_note_deletion_keeps_a_recording_tombstone() {
    let store = store();
    let note = store
        .import_recording_note(RECORDING_A, "Meeting", 1)
        .unwrap()
        .unwrap();
    store
        .conn
        .execute("DELETE FROM notes WHERE id = ?1", [&note.id])
        .unwrap();
    assert!(store.note_recordings().unwrap()[0].deleted);
    assert!(store.upsert_note(&note.upsert(true)).is_err());
    assert_eq!(
        store
            .import_recording_note(RECORDING_A, "Must stay deleted", 100)
            .unwrap(),
        None
    );
}

#[test]
fn recording_import_failure_rolls_back_link_publication() {
    let store = store();
    ensure_notes_table(&store.conn).unwrap();
    store
        .conn
        .execute_batch(
            "CREATE TRIGGER fail_import BEFORE INSERT ON notes BEGIN
               SELECT RAISE(ABORT, 'injected failure');
             END;",
        )
        .unwrap();
    assert!(store
        .import_recording_note(RECORDING_A, "Meeting", 1)
        .is_err());
    assert!(store.list_notes().unwrap().is_empty());
    assert!(store.note_recordings().unwrap().is_empty());
    store
        .conn
        .execute_batch("DROP TRIGGER fail_import")
        .unwrap();
    assert!(store
        .import_recording_note(RECORDING_A, "Meeting", 1)
        .unwrap()
        .is_some());
    assert_eq!(store.note_recordings().unwrap().len(), 1);
}

#[test]
fn recording_note_delete_failure_rolls_back_tombstone_and_preserves_note() {
    let store = store();
    let note = store
        .import_recording_note(RECORDING_A, "Meeting", 1)
        .unwrap()
        .unwrap();
    store
        .conn
        .execute_batch(
            "CREATE TRIGGER fail_delete BEFORE DELETE ON notes BEGIN
               SELECT RAISE(ABORT, 'injected failure');
             END;",
        )
        .unwrap();
    assert!(store.delete_note(&note.id).is_err());
    assert_eq!(store.get_note(&note.id).unwrap(), Some(note));
    assert!(!store.note_recordings().unwrap()[0].deleted);
}

#[test]
fn recording_link_writes_do_not_invalidate_active_history_schema_guards() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_vorkspace(&directory.path().join("vorkspace.db")).unwrap();
    store.shared_history_schema().unwrap();
    let version = schema_version(&store);
    store
        .attach_note_recording("recording-note", RECORDING_A)
        .unwrap();
    store
        .import_recording_note(RECORDING_A, "Meeting", 123)
        .unwrap();
    store.delete_note("recording-note").unwrap();
    assert_eq!(schema_version(&store), version);
}
