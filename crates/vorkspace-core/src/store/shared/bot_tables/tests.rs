use super::*;
#[test]
fn earlier_and_current_owners_remain_separate_through_open_append_edit_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let old = store.create("codex", "/old").unwrap();
    let current = store.create("codex", "/current").unwrap();
    let old_id = store.persisted_session_id(old.id).unwrap();
    let current_id = store.persisted_session_id(current.id).unwrap();
    let prefix = String::from_utf8(vec![109, 111, 110, 111]).unwrap();
    let schema = include_str!("../../../../tests/fixtures/bot-schema.sql");
    store.conn.execute_batch(schema).unwrap();
    store
        .conn
        .execute_batch(&schema.replace("bot_", &format!("{prefix}_")))
        .unwrap();
    for id in [&old_id, &current_id] {
        store
            .conn
            .execute("INSERT INTO bot_transcripts VALUES(?1)", [id])
            .unwrap();
        store
            .conn
            .execute(
                &format!("INSERT INTO {prefix}_transcripts VALUES(?1)"),
                [id],
            )
            .unwrap();
    }
    let mut saved = json!({"id":"saved","role":"user","text":"Original history","future":{"secret":"opaque"},"tool":{"input":{"uninterpreted":true}}});
    saved[format!("{prefix}Habit")] = json!({"id":"habit","future":5});
    saved[format!("{prefix}SpawnedSessions")] = json!([{"sessionId":current_id,"cwd":"/current","title":"Existing child","harness":"codex","model":"gpt-6","future":7}]);
    for id in [&old_id, &current_id] {
        store.conn.execute(&format!("INSERT INTO {prefix}_blocks(session_id,seq,block_id,block_json,search_text,is_user,is_draft) VALUES(?1,1,'saved',?2,'',1,0)"),params![id,saved.to_string()]).unwrap();
    }
    let current_block =
        json!({"id":"current","role":"user","text":"Current owner","future":{"keep":true}});
    store.conn.execute("INSERT INTO bot_blocks(session_id,seq,block_id,block_json,search_text,is_user,is_draft) VALUES(?1,1,'current',?2,'',1,0)",params![current_id,current_block.to_string()]).unwrap();
    drop(store);
    let before = std::fs::read(&path).unwrap();
    {
        let store = Store::open_vorkspace(&path).unwrap();
        assert_eq!(
            store.shared_bot_tables(&old_id).unwrap().unwrap().blocks,
            format!("{prefix}_blocks")
        );
        assert_eq!(
            store
                .shared_bot_tables(&current_id)
                .unwrap()
                .unwrap()
                .blocks,
            "bot_blocks"
        );
        assert!(store.bot_session(old.id).unwrap().unwrap().has_messages);
        assert!(store
            .load(old.id)
            .unwrap()
            .rows
            .iter()
            .any(|row| row.text == "Original history"));
        assert_eq!(store.bot_work(old.id).unwrap().unwrap().1.len(), 1);
        assert!(store
            .load(current.id)
            .unwrap()
            .rows
            .iter()
            .any(|row| row.text == "Current owner"));
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("ALTER TABLE sessions DROP COLUMN has_user_message")
            .unwrap();
    }
    {
        let store = Store::open_vorkspace(&path).unwrap();
        assert!(store.bot_session(old.id).unwrap().unwrap().has_messages);
        assert!(store.bot_session(current.id).unwrap().unwrap().has_messages);
    }
    {
        let mut store = Store::open_vorkspace(&path).unwrap();
        let mut history = store.load(old.id).unwrap();
        let start = history.begin("assistant");
        history.append("Appended answer");
        store
            .write_rows(old.id, start, &history.rows[start..])
            .unwrap();
        let start = history.rows.len() - 1;
        history.rows[start].text = "Edited answer".into();
        store
            .write_rows(old.id, start, &history.rows[start..])
            .unwrap();
        let raw: String = store
            .conn
            .query_row(
                &format!("SELECT block_json FROM {prefix}_blocks WHERE session_id=?1 AND seq=1"),
                [&old_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&raw).unwrap(), saved);
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT count(*) FROM bot_blocks WHERE session_id=?1",
                    [&old_id],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
    let store = Store::open_vorkspace(&path).unwrap();
    assert!(store
        .load(old.id)
        .unwrap()
        .rows
        .iter()
        .any(|row| row.text == "Edited answer"));
    assert!(store
        .load(current.id)
        .unwrap()
        .rows
        .iter()
        .any(|row| row.text == "Current owner"));
    // A completed earlier conversion explicitly made current root presence
    // authoritative, including a user's deliberately emptied transcript.
    store
        .conn
        .execute("INSERT INTO schema_migrations VALUES(22,1)", [])
        .unwrap();
    assert_eq!(
        store.shared_bot_tables(&old_id).unwrap().unwrap().blocks,
        "bot_blocks"
    );
}
#[test]
fn incomplete_earlier_storage_is_refused_without_creating_a_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.db");
    drop(Store::open_vorkspace(&path).unwrap());
    let prefix = String::from_utf8(vec![109, 111, 110, 111]).unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&format!(
        "CREATE TABLE {prefix}_transcripts(session_id TEXT PRIMARY KEY)"
    ))
    .unwrap();
    drop(conn);
    let before = std::fs::read(&path).unwrap();
    assert!(Store::open_vorkspace(&path)
        .err()
        .unwrap()
        .contains("incomplete"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
