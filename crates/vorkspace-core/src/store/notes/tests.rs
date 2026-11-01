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
