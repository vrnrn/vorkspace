use super::*;
use rusqlite::hooks::{AuthAction, Authorization};
use serde_json::json;
use std::sync::atomic::AtomicUsize;

#[test]
fn composer_draft_migration_preserves_existing_sessions_and_saved_draft_turns() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.lock_conn().unwrap();
    let mut session = sample("existing-draft", "/tmp/a", "Saved draft");
    session.blocks =
        json!([{"id":"saved","role":"user","text":"Explicit saved draft","draft":true}]);
    upsert_session(&conn, &session).unwrap();
    conn.execute_batch(
        "DROP TABLE composer_drafts; DELETE FROM schema_migrations WHERE version >= 20;",
    )
    .unwrap();
    migrate(&conn).unwrap();
    migrate(&conn).unwrap();
    assert_eq!(
        get_session(&conn, &session.id).unwrap().unwrap().blocks,
        session.blocks
    );
    assert!(list_by_project(&conn, &session.cwd).unwrap()[0].draft);
    let (migration, rows): (i64, i64) = conn.query_row(
            "SELECT (SELECT COUNT(*) FROM schema_migrations WHERE version = 20), (SELECT COUNT(*) FROM composer_drafts)",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
    assert_eq!((migration, rows), (1, 0));
    // Recorded migrations from another build do not prevent table repair.
    conn.execute_batch("DROP TABLE composer_drafts;").unwrap();
    migrate(&conn).unwrap();
    assert_eq!(
        get_session(&conn, &session.id).unwrap().unwrap().blocks,
        session.blocks
    );
    assert!(conn.prepare("SELECT * FROM composer_drafts").is_ok());
}

#[test]
fn composer_draft_session_deletion_cleanup_is_atomic_with_transcript_deletion() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.lock_conn().unwrap();
    upsert_session(&conn, &sample("delete-draft", "/tmp/a", "Delete me")).unwrap();
    let raw = json!({"cwd":"/tmp/a","text":"Unsent input","selectionStart":12,"selectionEnd":12,"attachments":[],"mcpTags":[]}).to_string();
    conn.execute("INSERT INTO composer_drafts (session_id,cwd,revision,payload_json,updated_at) VALUES ('delete-draft','/tmp/a',4,?1,1)", [&raw]).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_draft_session_delete BEFORE DELETE ON sessions BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
    assert!(delete_session(&conn, "delete-draft").is_err());
    let preserved: (String, i64) = conn
        .query_row(
            "SELECT payload_json,deleted FROM composer_drafts WHERE session_id = 'delete-draft'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(preserved, (raw, 0));
    assert!(get_session(&conn, "delete-draft").unwrap().is_some());
    conn.execute_batch("DROP TRIGGER reject_draft_session_delete;")
        .unwrap();
    delete_session(&conn, "delete-draft").unwrap();
    assert!(get_session(&conn, "delete-draft").unwrap().is_none());
    let (cleared, deleted): (String, i64) = conn
        .query_row(
            "SELECT payload_json,deleted FROM composer_drafts WHERE session_id = 'delete-draft'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(serde_json::from_str::<Value>(&cleared).unwrap()["text"], "");
    // A session with no transcript/draft yet still receives a fence.
    delete_session(&conn, "never-persisted").unwrap();
    let deleted: i64 = conn
        .query_row(
            "SELECT deleted FROM composer_drafts WHERE session_id = 'never-persisted'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(deleted, 1);
}

#[test]
fn composer_draft_saved_turn_discard_preserves_unsent_typing_and_reusable_identity() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.lock_conn().unwrap();
    upsert_session(&conn, &sample("saved-only", "/tmp/a", "Saved draft")).unwrap();
    let raw = json!({"cwd":"/tmp/a","text":"Separate unsent input","selectionStart":0,"selectionEnd":0,"attachments":[],"mcpTags":[]}).to_string();
    conn.execute("INSERT INTO composer_drafts (session_id,cwd,revision,payload_json,updated_at) VALUES ('saved-only','/tmp/a',2,?1,1)", [&raw]).unwrap();
    delete_session_record(&conn, "saved-only", true).unwrap();
    assert!(get_session(&conn, "saved-only").unwrap().is_none());
    let row: (String, i64, i64) = conn.query_row("SELECT payload_json, revision, deleted FROM composer_drafts WHERE session_id = 'saved-only'", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
    assert_eq!(row, (raw, 2, 0));
    let receipt_tombstones: i64 = conn.query_row(
            "SELECT COUNT(*) FROM local_submission_deleted_sessions WHERE session_id = 'saved-only'",
            [], |row| row.get(0),
        ).unwrap();
    assert_eq!(receipt_tombstones, 0);
    upsert_session(&conn, &sample("saved-only", "/tmp/a", "Reused")).unwrap();
    assert!(get_session(&conn, "saved-only").unwrap().is_some());
}

#[test]
fn search_keeps_older_bot_messages_available_after_conversion() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.lock_conn().unwrap();
    let mut bot = sample("bot", "/tmp/a", "Bot");
    bot.blocks = json!([{ "id":"old", "role":"user", "text":"Archived needle" }, { "id":"answer", "role":"assistant", "text":"Done" }]);
    upsert_session(&conn, &bot).unwrap();
    crate::bot_transcript::migrate_transcript(&conn, "bot").unwrap();
    let result = search_sessions(
        &conn,
        &SessionSearchOptions {
            query: "Archived needle".into(),
            cwd: None,
            include_archived: false,
            search_owner: "bot-search".into(),
        },
    )
    .unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].block_id.as_deref(), Some("old"));
}

#[test]
fn startup_does_not_read_saved_transcripts() {
    let data_dir = std::env::temp_dir().join(format!(
        "monocode-startup-transcripts-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let path = data_dir.join("monocode.db");
    {
        let store = SessionStore::open(path.clone()).unwrap();
        let conn = store.lock_conn().unwrap();
        conn.execute(
            "INSERT INTO sessions (
                   id, cwd, harness, model, runtime_mode, title,
                   blocks_json, created_at, updated_at
                 ) VALUES ('large', '/tmp', 'codex', 'test', 'supervised', 'Large', ?1, 1, 1)",
            ["x".repeat(8_000_000)],
        )
        .unwrap();
    }

    let transcript_reads = Arc::new(AtomicUsize::new(0));
    let reads = Arc::clone(&transcript_reads);
    let mut managed = None;
    init_with(
        || Ok(data_dir.clone()),
        |requested_path| {
            assert_eq!(requested_path, path);
            let store = SessionStore::open(requested_path)?;
            store
                .lock_conn()?
                .authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
                    if matches!(
                        context.action,
                        AuthAction::Read {
                            table_name: "sessions",
                            column_name: "blocks_json"
                        }
                    ) {
                        reads.fetch_add(1, Ordering::Relaxed);
                        Authorization::Deny
                    } else {
                        Authorization::Allow
                    }
                }))
                .map_err(|error| error.to_string())?;
            Ok(store)
        },
        |store| managed = Some(store),
    )
    .unwrap();
    assert_eq!(transcript_reads.load(Ordering::Relaxed), 0);
    drop(managed);
    let _ = std::fs::remove_dir_all(data_dir);
}

fn sample(id: &str, cwd: &str, title: &str) -> SessionUpsert {
    SessionUpsert {
        sidebar_hidden: false,
        id: id.into(),
        cwd: cwd.into(),
        harness: "cursor".into(),
        model: "gpt-5".into(),
        model_settings: json!({ "thinking": "high" }),
        runtime_mode: "supervised".into(),
        title: title.into(),
        provider_session_id: Some("acp-session-1".into()),
        provider_account_id: None,
        blocks: json!([{ "id": "b1", "role": "user", "text": "hello" }]),
        queued_messages: vec![],
        queue_status: None,
        context_used: None,
        context_window: None,
        branch: None,
        worktree_cwd: None,
        worktree_removed: false,
        linked_work_item: None,
        automation_id: None,
    }
}

#[test]
fn legacy_inbox_chats_are_not_normal_sessions() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.lock_conn().unwrap();
    upsert_session(&conn, &sample("project", "/tmp/project", "Login")).unwrap();
    upsert_session(&conn, &sample("ask", "/tmp/project", "Login")).unwrap();
    conn.execute("UPDATE sessions SET inbox_ask = '{}' WHERE id = 'ask'", [])
        .unwrap();
    migrate(&conn).unwrap();
    let rows = list_by_project(&conn, "/tmp/project").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "project");
    assert!(get_session(&conn, "ask").unwrap().is_none());
    let search = search_sessions(
        &conn,
        &SessionSearchOptions {
            query: "Login".into(),
            cwd: None,
            include_archived: true,
            search_owner: String::new(),
        },
    )
    .unwrap();
    assert!(search.hits.iter().all(|hit| hit.session_id == "project"));
    // Hiding the old implementation's records does not delete their data.
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn orchestration_history_groups_workers_and_keeps_their_transcripts() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    for id in ["lead", "worker-a", "worker-b", "unrelated"] {
        upsert_session(&conn, &sample(id, "/tmp/a", id)).unwrap();
    }
    let run = json!({"status": "active", "tasks": [
        {"sessionId": "worker-a", "title": "UI", "harness": "codex", "model": "one", "status": "running", "prompt": "private instructions", "result": "large result"},
        {"sessionId": "worker-b", "title": "Tests", "harness": "claude", "model": "two", "status": "queued"}
    ]});
    save_orchestration(&conn, "lead", &run).unwrap();
    // Reopening/migration must not hydrate, pause or otherwise mutate runs.
    migrate(&conn).unwrap();
    let rows = list_by_project(&conn, "/tmp/a").unwrap();
    assert_eq!(rows.len(), 2);
    let lead = rows.iter().find(|row| row.id == "lead").unwrap();
    let summary = lead.orchestration.as_ref().unwrap();
    assert_eq!(summary["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(summary["status"], "active");
    assert!(summary["tasks"][0].get("prompt").is_none());
    assert!(summary["tasks"][0].get("result").is_none());
    let worker = get_session(&conn, "worker-a").unwrap().unwrap();
    assert_eq!(worker.orchestration_lead_id.as_deref(), Some("lead"));
    assert!(has_user_block(&worker.blocks));
    // A later run replaces the card's agents without resurfacing old chats.
    save_orchestration(&conn, "lead", &json!({"status": "active", "tasks": []})).unwrap();
    assert_eq!(list_by_project(&conn, "/tmp/a").unwrap().len(), 2);
}

#[test]
fn worker_upserts_carry_ownership_before_the_run_is_saved() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let mut worker = sample("worker", "/tmp/a", "Worker");
    worker.blocks =
        json!([{"id": "u", "role": "user", "text": "Task", "orchestrationLeadId": "lead"}]);
    worker.linked_work_item = Some(json!({"kind": "pr", "number": 1}));
    let summary = upsert_session(&conn, &worker).unwrap();
    assert_eq!(summary.orchestration_lead_id.as_deref(), Some("lead"));
    assert!(list_by_project(&conn, "/tmp/a").unwrap().is_empty());
    assert!(list_linked(&conn).unwrap().is_empty());
    // An older renderer's next write cannot accidentally detach a worker.
    worker.blocks = json!([{"id": "u", "role": "user", "text": "Task"}]);
    upsert_session(&conn, &worker).unwrap();
    assert!(list_by_project(&conn, "/tmp/a").unwrap().is_empty());
}

#[test]
fn migration_groups_workers_from_the_earlier_preview() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    for id in ["lead", "old-worker", "current-worker"] {
        upsert_session(&conn, &sample(id, "/tmp/a", id)).unwrap();
    }
    conn.execute(
        "UPDATE sessions SET blocks_json = ?1 WHERE id = 'old-worker'",
        [
            json!([{"role": "user", "orchestrationLeadId": "lead", "text": "Old task"}])
                .to_string(),
        ],
    )
    .unwrap();
    conn.execute("INSERT INTO orchestration_runs(lead_id, state) VALUES ('lead', ?1)", [json!({"status": "paused", "tasks": [{"sessionId": "current-worker", "title": "Task", "harness": "codex", "model": "one", "status": "completed"}]}).to_string()]).unwrap();
    conn.execute_batch("DROP TABLE orchestration_sidebar; DROP TABLE orchestration_workers;")
        .unwrap();
    migrate(&conn).unwrap();
    let rows = list_by_project(&conn, "/tmp/a").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "lead");
    assert_eq!(
        rows[0].orchestration.as_ref().unwrap()["tasks"][0]["title"],
        "Task"
    );
    assert_eq!(
        worker_parent(&conn, "old-worker").unwrap().as_deref(),
        Some("lead")
    );
    assert_eq!(
        worker_parent(&conn, "current-worker").unwrap().as_deref(),
        Some("lead")
    );
}

/// The sidebar query must stay answerable from the index alone. Selecting a
/// column the index does not carry silently reintroduces a table seek per
/// row, and every summary column sits behind a ~180 KB `blocks_json` blob
/// in the record, so that regression is worth ~30x on a real project.
#[test]
fn list_by_project_is_served_by_a_covering_index() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    upsert_session(&conn, &sample("s1", "/tmp/a", "A1")).unwrap();
    for upgraded in [false, true] {
        if upgraded {
            // Exercise an existing v15 database with the old projection.
            conn.execute_batch(
                "DROP INDEX sessions_cwd_cover_idx;
                     CREATE INDEX sessions_cwd_cover_idx
                       ON sessions (cwd, has_user_message, updated_at DESC, id, harness,
                                    model, runtime_mode, title, provider_session_id,
                                    created_at, branch, archived, pinned, linked_work_item_json);
                     DELETE FROM schema_migrations WHERE version >= 16;",
            )
            .unwrap();
            migrate(&conn).unwrap();
        }
        let plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN
                 SELECT id, cwd, harness, model, runtime_mode, title, provider_session_id,
                        created_at, updated_at, branch, archived, pinned,
                        linked_work_item_json, worktree_cwd, worktree_removed, is_draft,
                        automation_id, sidebar_hidden,
                        (SELECT summary FROM orchestration_sidebar WHERE lead_id = sessions.id)
                 FROM sessions
                 WHERE cwd = ?1
                   AND has_user_message = 1
                   AND id NOT IN (SELECT id FROM sessions WHERE inbox_ask IS NOT NULL)
                   AND id NOT IN (SELECT session_id FROM orchestration_workers)
                 ORDER BY updated_at DESC, id ASC",
                params!["/tmp/a"],
                |row| row.get(3),
            )
            .unwrap();
        assert!(
            plan.contains("COVERING INDEX"),
            "sidebar listing fell back to table seeks: {plan}"
        );
    }
}

#[test]
fn upsert_tracks_whether_a_user_has_spoken() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let mut quiet = sample("s1", "/tmp/a", "Quiet");
    quiet.blocks = json!([{ "id": "b1", "role": "assistant", "text": "hi" }]);
    upsert_session(&conn, &quiet).unwrap();
    assert!(list_by_project(&conn, "/tmp/a").unwrap().is_empty());

    // The flag has to follow the transcript, not just the first write.
    quiet.blocks = json!([
        { "id": "b1", "role": "assistant", "text": "hi" },
        { "id": "b2", "role": "user", "text": "hello" }
    ]);
    upsert_session(&conn, &quiet).unwrap();
    assert_eq!(list_by_project(&conn, "/tmp/a").unwrap().len(), 1);
}

#[test]
fn draft_marker_round_trips_through_session_summaries() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let mut session = sample("s1", "/tmp/a", "Draft");
    session.blocks = json!([
        { "id": "b1", "role": "user", "text": "start" },
        { "id": "b2", "role": "assistant", "text": "done" },
        { "id": "b3", "role": "user", "text": "hello", "draft": true }
    ]);

    let saved = upsert_session(&conn, &session).unwrap();
    assert!(saved.draft);
    assert!(list_by_project(&conn, "/tmp/a").unwrap()[0].draft);

    session.blocks = json!([
        { "id": "b1", "role": "user", "text": "start" },
        { "id": "b2", "role": "assistant", "text": "done" },
        { "id": "b3", "role": "user", "text": "hello" }
    ]);
    let sent = upsert_session(&conn, &session).unwrap();
    assert!(!sent.draft);
    assert!(!list_by_project(&conn, "/tmp/a").unwrap()[0].draft);
}

#[test]
fn migrate_creates_sessions_table() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    let table: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'sessions'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table, 1);
    let branch: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'branch'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(branch, 1);
}

#[test]
fn upsert_preserves_created_at_and_updates_fields() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let first = upsert_session(&conn, &sample("s1", "/tmp/a", "First")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let mut next = sample("s1", "/tmp/a", "Updated");
    next.provider_session_id = Some("acp-session-2".into());
    next.blocks = json!([
        { "id": "b1", "role": "user", "text": "hello" },
        { "id": "b2", "role": "assistant", "text": "world" }
    ]);
    let second = upsert_session(&conn, &next).unwrap();
    assert_eq!(second.created_at, first.created_at);
    assert!(second.updated_at > first.updated_at);
    assert_eq!(second.title, "Updated");
    assert_eq!(second.provider_session_id.as_deref(), Some("acp-session-2"));
}

#[test]
fn saving_a_loaded_transcript_keeps_its_omitted_tool_output() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let output = "line\n".repeat(1_000);
    let mut row = sample("s1", "/tmp/a", "First");
    row.blocks = json!([
        { "id": "u", "role": "user", "text": "run" },
        { "id": "t", "role": "tool", "text": "npm test",
          "tool": { "kind": "execute", "status": "completed", "detail": output } }
    ]);
    upsert_session(&conn, &row).unwrap();

    // What `session_get` hands the transcript.
    let mut loaded = get_session(&conn, "s1").unwrap().unwrap().blocks;
    crate::tool_detail::omit_large_details(&mut loaded);
    assert!(loaded[1]["tool"].get("detail").is_none());
    loaded
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "a", "role": "assistant", "text": "done" }));
    row.blocks = loaded;
    upsert_session(&conn, &row).unwrap();

    let stored = get_session(&conn, "s1").unwrap().unwrap().blocks;
    assert_eq!(stored[1]["tool"]["detail"], output);
    assert!(stored[1]["tool"].get("detailOmitted").is_none());
    assert_eq!(stored[2]["text"], "done");
    assert_eq!(
        crate::tool_detail::stored_detail(&conn, "s1", "t", None).unwrap(),
        Some(output)
    );
}

#[test]
fn context_usage_round_trips() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let mut row = sample("s1", "/tmp/a", "First");
    row.context_used = Some(29_821);
    row.context_window = Some(1_000_000);
    upsert_session(&conn, &row).unwrap();
    let stored = get_session(&conn, "s1").unwrap().unwrap();
    assert_eq!(stored.context_used, Some(29_821));
    assert_eq!(stored.context_window, Some(1_000_000));
}

#[test]
fn queued_messages_round_trip_and_clear_without_changing_the_transcript() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let mut row = sample("queued", "/tmp/a", "Conversation");
    row.queued_messages = vec![
        json!({"id":"first", "text":"One", "attachments":[]}),
        json!({"id":"second", "text":"", "attachments":[{"id":"image", "kind":"image", "data":"bytes"}], "error":"Offline"}),
    ];
    row.queue_status = Some("paused".into());
    upsert_session(&conn, &row).unwrap();
    let stored = get_session(&conn, "queued").unwrap().unwrap();
    assert_eq!(stored.queued_messages, row.queued_messages);
    assert_eq!(stored.queue_status.as_deref(), Some("paused"));
    assert_eq!(stored.blocks, row.blocks);

    row.queued_messages.clear();
    row.queue_status = None;
    upsert_session(&conn, &row).unwrap();
    let stored = get_session(&conn, "queued").unwrap().unwrap();
    assert!(stored.queued_messages.is_empty());
    assert_eq!(stored.queue_status, None);
    assert_eq!(stored.blocks, row.blocks);
}

#[test]
fn sidebar_visibility_round_trips_without_hiding_session_access() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.lock_conn().unwrap();
    let mut row = sample("hidden", "/tmp/a", "Review");
    row.sidebar_hidden = true;
    row.linked_work_item = Some(json!({"kind": "pr", "repo": "a/b", "number": 1}));
    assert!(upsert_session(&conn, &row).unwrap().sidebar_hidden);
    assert!(list_by_project(&conn, &row.cwd).unwrap()[0].sidebar_hidden);
    assert!(list_linked(&conn).unwrap()[0].sidebar_hidden);
    let stored = get_session(&conn, &row.id).unwrap().unwrap();
    assert!(stored.sidebar_hidden);
    assert_eq!(stored.blocks, row.blocks);
    row.title = "Review complete".into();
    upsert_session(&conn, &row).unwrap();
    assert!(get_session(&conn, &row.id).unwrap().unwrap().sidebar_hidden);
}

#[test]
fn sidebar_visibility_migration_keeps_existing_sessions_visible() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.lock_conn().unwrap();
    upsert_session(&conn, &sample("existing", "/tmp/a", "Existing")).unwrap();
    conn.execute_batch(
        "DROP INDEX sessions_cwd_cover_idx;
             ALTER TABLE sessions DROP COLUMN sidebar_hidden;
             DELETE FROM schema_migrations WHERE version = 19;",
    )
    .unwrap();
    migrate(&conn).unwrap();
    migrate(&conn).unwrap();
    assert!(!list_by_project(&conn, "/tmp/a").unwrap()[0].sidebar_hidden);
    assert!(
        !get_session(&conn, "existing")
            .unwrap()
            .unwrap()
            .sidebar_hidden
    );
}

#[test]
fn linked_work_item_round_trips() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let mut row = sample("s1", "/tmp/a", "Fix PR");
    row.linked_work_item = Some(json!({
        "kind": "pr",
        "repo": "openai/codex",
        "number": 42,
        "url": "https://github.com/openai/codex/pull/42"
    }));

    let summary = upsert_session(&conn, &row).unwrap();
    assert_eq!(summary.linked_work_item, row.linked_work_item);
    let listed = list_by_project(&conn, "/tmp/a").unwrap();
    assert_eq!(listed[0].linked_work_item, row.linked_work_item);
    let stored = get_session(&conn, "s1").unwrap().unwrap();
    assert_eq!(stored.linked_work_item, row.linked_work_item);
}

#[test]
fn linked_work_item_can_be_set_and_removed_without_rewriting_the_session() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    upsert_session(&conn, &sample("s1", "/tmp/a", "Fix PR")).unwrap();
    let linked = json!({
        "kind": "pr",
        "repo": "openai/codex",
        "number": 42,
        "url": "https://github.com/openai/codex/pull/42"
    });

    set_linked_work_item(&conn, "s1", Some(&linked)).unwrap();
    assert_eq!(
        get_session(&conn, "s1").unwrap().unwrap().linked_work_item,
        Some(linked)
    );

    set_linked_work_item(&conn, "s1", None).unwrap();
    assert_eq!(
        get_session(&conn, "s1").unwrap().unwrap().linked_work_item,
        None
    );
}

#[test]
fn automation_id_round_trips() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let mut row = sample("s1", "/tmp/a", "Nightly review");
    row.automation_id = Some("automation-id".into());

    let summary = upsert_session(&conn, &row).unwrap();
    assert_eq!(summary.automation_id.as_deref(), Some("automation-id"));
    let listed = list_by_project(&conn, "/tmp/a").unwrap();
    assert_eq!(listed[0].automation_id.as_deref(), Some("automation-id"));
    let stored = get_session(&conn, "s1").unwrap().unwrap();
    assert_eq!(stored.automation_id.as_deref(), Some("automation-id"));
}

#[test]
fn list_linked_finds_threads_across_projects() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let linked = json!({
        "kind": "pr",
        "repo": "openai/codex",
        "number": 42,
        "url": "https://github.com/openai/codex/pull/42"
    });
    let mut first = sample("s1", "/tmp/a", "First");
    first.linked_work_item = Some(linked.clone());
    let mut second = sample("s2", "/tmp/b", "Second");
    second.linked_work_item = Some(linked);
    upsert_session(&conn, &first).unwrap();
    upsert_session(&conn, &second).unwrap();
    upsert_session(&conn, &sample("s3", "/tmp/a", "Unlinked")).unwrap();

    let rows = list_linked(&conn).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row.id == "s1"));
    assert!(rows.iter().any(|row| row.id == "s2"));
    assert!(rows.iter().all(|row| row.linked_work_item.is_some()));
}

#[test]
fn context_usage_is_absent_until_a_harness_reports() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    upsert_session(&conn, &sample("s1", "/tmp/a", "First")).unwrap();
    let stored = get_session(&conn, "s1").unwrap().unwrap();
    assert_eq!(stored.context_used, None);
    assert_eq!(stored.context_window, None);
}

#[test]
fn migration_v3_adds_context_columns() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 3",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn upsert_same_blocks_keeps_updated_at() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let first = upsert_session(&conn, &sample("s1", "/tmp/a", "First")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let mut next = sample("s1", "/tmp/a", "First");
    next.model = "gpt-5.4".into();
    let second = upsert_session(&conn, &next).unwrap();
    assert_eq!(second.updated_at, first.updated_at);
    assert_eq!(second.model, "gpt-5.4");
}

#[test]
fn list_does_not_sum_write_preview_diffs() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let mut session = sample("s1", "/tmp/a", "Diffs");
    session.blocks = json!([
        { "id": "b1", "role": "user", "text": "edit it" },
        {
            "id": "b2",
            "role": "tool",
            "text": "",
            "tool": {
                "preview": {
                    "kind": "write",
                    "path": "src/a.ts",
                    "additions": 12,
                    "deletions": 3
                }
            }
        }
    ]);
    let summary = upsert_session(&conn, &session).unwrap();
    assert_eq!(summary.additions, 0);
    assert_eq!(summary.deletions, 0);
    let listed = list_by_project(&conn, "/tmp/a").unwrap();
    assert_eq!(listed[0].additions, 0);
    assert_eq!(listed[0].deletions, 0);
}

#[test]
fn list_by_project_filters_and_orders() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    upsert_session(&conn, &sample("s1", "/tmp/a", "A1")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    upsert_session(&conn, &sample("s2", "/tmp/a", "A2")).unwrap();
    upsert_session(&conn, &sample("s3", "/tmp/b", "B1")).unwrap();
    let mut empty = sample("s4", "/tmp/a", "Empty");
    empty.blocks = json!([]);
    upsert_session(&conn, &empty).unwrap();
    let listed = list_by_project(&conn, "/tmp/a").unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, "s2");
    assert_eq!(listed[1].id, "s1");
}

#[test]
fn delete_removes_session() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    upsert_session(&conn, &sample("s1", "/tmp/a", "First")).unwrap();
    delete_session(&conn, "s1").unwrap();
    assert!(get_session(&conn, "s1").unwrap().is_none());
    assert!(list_by_project(&conn, "/tmp/a").unwrap().is_empty());
}

#[test]
fn deleting_a_lead_releases_current_and_earlier_workers() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    for id in ["lead", "earlier", "current", "other-lead", "other-worker"] {
        let mut session = sample(id, "/tmp/a", id);
        session.linked_work_item = Some(json!({"kind": "pr", "number": 1}));
        if ["earlier", "current"].contains(&id) {
            session.blocks = json!([{"id":"u","role":"user","text":"Keep this transcript","orchestrationLeadId":"lead"}]);
        }
        upsert_session(&conn, &session).unwrap();
    }
    save_orchestration(
        &conn,
        "lead",
        &json!({"status":"stopped","tasks":[{"id":"task","sessionId":"current"}]}),
    )
    .unwrap();
    let other = json!({"status":"active","tasks":[{"id":"other-task","sessionId":"other-worker"}]});
    save_orchestration(&conn, "other-lead", &other).unwrap();
    delete_session(&conn, "lead").unwrap();

    for id in ["earlier", "current"] {
        let worker = get_session(&conn, id).unwrap().unwrap();
        assert!(worker.orchestration_lead_id.is_none());
        assert!(worker.blocks[0].get("orchestrationLeadId").is_none());
        assert_eq!(worker.blocks[0]["text"], "Keep this transcript");
        assert!(list_by_project(&conn, "/tmp/a")
            .unwrap()
            .iter()
            .any(|row| row.id == id));
        assert!(list_linked(&conn).unwrap().iter().any(|row| row.id == id));
    }
    assert!(orchestration_summary(&conn, "lead").unwrap().is_none());
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM orchestration_runs WHERE lead_id = 'lead'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        worker_parent(&conn, "other-worker").unwrap().as_deref(),
        Some("other-lead")
    );
    let raw: String = conn
        .query_row(
            "SELECT state FROM orchestration_runs WHERE lead_id = 'other-lead'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&raw).unwrap(), other);
    migrate(&conn).unwrap();
    assert!(worker_parent(&conn, "current").unwrap().is_none());
}

#[test]
fn deleting_a_worker_prunes_parent_state_and_does_not_release_dependencies() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    for id in ["lead", "worker", "dependent", "earlier"] {
        upsert_session(&conn, &sample(id, "/tmp/a", id)).unwrap();
    }
    remember_worker(&conn, "earlier", "lead").unwrap();
    save_orchestration(
        &conn,
        "lead",
        &json!({"status":"active", "requests":{"old":{"result":"worker"}}, "tasks":[
            {"id":"task", "sessionId":"worker", "status":"running", "dependsOn":[]},
            {"id":"next", "sessionId":"dependent", "status":"queued", "dependsOn":["task"]}
        ]}),
    )
    .unwrap();
    delete_session(&conn, "worker").unwrap();
    assert!(get_session(&conn, "worker").unwrap().is_none());
    assert!(worker_parent(&conn, "worker").unwrap().is_none());
    assert_eq!(
        worker_parent(&conn, "earlier").unwrap().as_deref(),
        Some("lead")
    );
    let raw: String = conn
        .query_row(
            "SELECT state FROM orchestration_runs WHERE lead_id = 'lead'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let run: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(run["status"], "stopped");
    assert_eq!(run["requests"], json!({}));
    assert_eq!(run["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(run["tasks"][0]["sessionId"], "dependent");
    assert_eq!(run["tasks"][0]["status"], "cancelled");
    assert_eq!(run["tasks"][0]["dependsOn"], json!([]));
    let summary = orchestration_summary(&conn, "lead").unwrap().unwrap();
    assert_eq!(summary["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(summary["tasks"][0]["sessionId"], "dependent");
    assert_eq!(summary["tasks"][0]["status"], "cancelled");
}

#[test]
fn failed_session_deletion_rolls_back_orchestration_cleanup() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    for id in ["lead", "worker"] {
        upsert_session(&conn, &sample(id, "/tmp/a", id)).unwrap();
    }
    save_orchestration(
        &conn,
        "lead",
        &json!({"status":"stopped", "tasks":[{"id":"task", "sessionId":"worker"}]}),
    )
    .unwrap();
    conn.execute_batch("CREATE TRIGGER reject_delete BEFORE DELETE ON sessions BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
    for id in ["lead", "worker"] {
        assert!(delete_session(&conn, id).is_err());
        assert!(get_session(&conn, id).unwrap().is_some());
        assert_eq!(
            worker_parent(&conn, "worker").unwrap().as_deref(),
            Some("lead")
        );
        assert_eq!(
            orchestration_summary(&conn, "lead").unwrap().unwrap()["tasks"][0]["sessionId"],
            "worker"
        );
    }
}

#[test]
fn migration_v6_adds_archived_column() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 6",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    let archived: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'archived'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archived, 1);
}

#[test]
fn migration_v7_adds_worktree_cwd_column() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 7",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    let column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'worktree_cwd'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(column, 1);
}

#[test]
fn removed_worktree_state_round_trips_and_can_be_reselected() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let mut session = sample("s1", "/tmp/a", "First");
    session.worktree_cwd = Some("/tmp/a-worktrees/feature".into());
    session.worktree_removed = true;
    session.branch = Some("stale-branch".into());
    let summary = upsert_session(&conn, &session).unwrap();
    assert!(summary.worktree_removed);
    assert!(summary.branch.is_none());
    let listed = list_by_project(&conn, "/tmp/a").unwrap();
    assert!(listed[0].worktree_removed);
    assert!(listed[0].branch.is_none());
    let restored = get_session(&conn, "s1").unwrap().unwrap();
    assert!(restored.worktree_removed);
    assert_eq!(restored.blocks, session.blocks);
    session.worktree_removed = false;
    session.worktree_cwd = None;
    session.branch = Some("main".into());
    upsert_session(&conn, &session).unwrap();
    let restored = get_session(&conn, "s1").unwrap().unwrap();
    assert!(!restored.worktree_removed);
    assert_eq!(restored.branch.as_deref(), Some("main"));
}

#[test]
fn archive_round_trips_and_survives_upsert() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    upsert_session(&conn, &sample("s1", "/tmp/a", "First")).unwrap();
    set_archived(&conn, "s1", true).unwrap();
    let listed = list_by_project(&conn, "/tmp/a").unwrap();
    assert!(listed[0].archived);
    let mut next = sample("s1", "/tmp/a", "Updated");
    next.blocks = json!([
        { "id": "b1", "role": "user", "text": "hello" },
        { "id": "b2", "role": "assistant", "text": "world" }
    ]);
    let summary = upsert_session(&conn, &next).unwrap();
    assert!(summary.archived);
    assert_eq!(summary.title, "Updated");
    set_archived(&conn, "s1", false).unwrap();
    let listed = list_by_project(&conn, "/tmp/a").unwrap();
    assert!(!listed[0].archived);
}

#[test]
fn migration_v11_adds_pinned_column() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 11",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    let pinned: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'pinned'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pinned, 1);
}

#[test]
fn pin_round_trips_and_survives_upsert() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    upsert_session(&conn, &sample("s1", "/tmp/a", "First")).unwrap();
    set_pinned(&conn, "s1", true).unwrap();
    let listed = list_by_project(&conn, "/tmp/a").unwrap();
    assert!(listed[0].pinned);
    let mut next = sample("s1", "/tmp/a", "Updated");
    next.blocks = json!([
        { "id": "b1", "role": "user", "text": "hello" },
        { "id": "b2", "role": "assistant", "text": "world" }
    ]);
    let summary = upsert_session(&conn, &next).unwrap();
    assert!(summary.pinned);
    assert_eq!(summary.title, "Updated");
    set_pinned(&conn, "s1", false).unwrap();
    let listed = list_by_project(&conn, "/tmp/a").unwrap();
    assert!(!listed[0].pinned);
}

#[test]
fn get_round_trips_blocks_provider_session_and_account() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let mut session = sample("s1", "/tmp/a", "First");
    session.provider_account_id = Some("account-work".into());
    upsert_session(&conn, &session).unwrap();
    let record = get_session(&conn, "s1").unwrap().unwrap();
    assert_eq!(record.id, "s1");
    assert_eq!(record.provider_session_id.as_deref(), Some("acp-session-1"));
    assert_eq!(record.provider_account_id.as_deref(), Some("account-work"));
    assert_eq!(record.model_settings["thinking"], "high");
    assert_eq!(record.blocks.as_array().unwrap().len(), 1);
    assert_eq!(record.blocks[0]["text"], "hello");
}

#[test]
fn upsert_snapshots_git_branch() {
    let dir = std::env::temp_dir().join(format!(
        "monocode-session-git-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let init = std::process::Command::new("git")
        .args(["init"])
        .current_dir(&dir)
        .output();
    let Ok(init) = init else {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    };
    if !init.status.success() {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let head = std::process::Command::new("git")
        .args(["symbolic-ref", "HEAD", "refs/heads/fix-sidebar"])
        .current_dir(&dir)
        .status();
    if head.map(|status| !status.success()).unwrap_or(true) {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let origin = std::process::Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widget.git",
        ])
        .current_dir(&dir)
        .status();
    if origin.map(|status| !status.success()).unwrap_or(true) {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    let cwd = dir.to_string_lossy().into_owned();
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let summary = upsert_session(&conn, &sample("s1", &cwd, "First")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(summary.branch.as_deref(), Some("fix-sidebar"));
    assert_eq!(summary.repo.as_deref(), Some("widget"));
}

#[test]
fn upsert_keeps_session_branch_and_worktree() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let mut session = sample("s1", "/tmp/a", "First");
    session.branch = Some("feat/picker".into());
    session.worktree_cwd = Some("/tmp/a-feat".into());
    let summary = upsert_session(&conn, &session).unwrap();
    assert_eq!(summary.branch.as_deref(), Some("feat/picker"));
    assert_eq!(summary.worktree_cwd.as_deref(), Some("/tmp/a-feat"));
    let record = get_session(&conn, "s1").unwrap().unwrap();
    assert_eq!(record.branch.as_deref(), Some("feat/picker"));
    assert_eq!(record.worktree_cwd.as_deref(), Some("/tmp/a-feat"));
    let listed = list_by_project(&conn, "/tmp/a").unwrap();
    assert_eq!(listed[0].branch.as_deref(), Some("feat/picker"));
    assert_eq!(listed[0].worktree_cwd.as_deref(), Some("/tmp/a-feat"));
}

#[test]
fn rebase_project_moves_saved_sessions_to_the_renamed_path() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    upsert_session(&conn, &sample("moved", "/tmp/old", "Moved")).unwrap();
    upsert_session(&conn, &sample("other", "/tmp/other", "Other")).unwrap();

    rebase_project(&conn, "/tmp/old", "/tmp/new").unwrap();

    assert!(list_by_project(&conn, "/tmp/old").unwrap().is_empty());
    assert_eq!(list_by_project(&conn, "/tmp/new").unwrap()[0].id, "moved");
    assert_eq!(list_by_project(&conn, "/tmp/other").unwrap()[0].id, "other");
}

#[test]
fn migration_v4_creates_in_flight_table() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 4",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    let table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'in_flight_sessions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(table, 1);
}

#[test]
fn in_flight_set_take_is_ordered_and_destructive() {
    let store = SessionStore::open_in_memory().unwrap();
    let mut conn = store.conn.lock().unwrap();
    replace_in_flight(
        &mut conn,
        &[
            InFlightSession {
                session_id: "s2".into(),
                cwd: "/tmp/b".into(),
            },
            InFlightSession {
                session_id: "s1".into(),
                cwd: "/tmp/a".into(),
            },
        ],
    )
    .unwrap();
    let first = take_in_flight(&mut conn).unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].session_id, "s2");
    assert_eq!(first[0].cwd, "/tmp/b");
    assert_eq!(first[1].session_id, "s1");
    let second = take_in_flight(&mut conn).unwrap();
    assert!(second.is_empty());
}

#[test]
fn in_flight_list_does_not_clear() {
    let store = SessionStore::open_in_memory().unwrap();
    let mut conn = store.conn.lock().unwrap();
    replace_in_flight(
        &mut conn,
        &[InFlightSession {
            session_id: "s1".into(),
            cwd: "/tmp/a".into(),
        }],
    )
    .unwrap();
    let listed = list_in_flight(&conn).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].session_id, "s1");
    let listed_again = list_in_flight(&conn).unwrap();
    assert_eq!(listed_again.len(), 1);
}

#[test]
fn in_flight_replace_clears_previous() {
    let store = SessionStore::open_in_memory().unwrap();
    let mut conn = store.conn.lock().unwrap();
    replace_in_flight(
        &mut conn,
        &[InFlightSession {
            session_id: "old".into(),
            cwd: "/tmp/old".into(),
        }],
    )
    .unwrap();
    replace_in_flight(&mut conn, &[]).unwrap();
    assert!(take_in_flight(&mut conn).unwrap().is_empty());
}

#[test]
fn migration_v5_creates_workspace_snapshot_table() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 5",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    let table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'workspace_snapshot'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(table, 1);
}

#[test]
fn workspace_snapshot_round_trips_and_replaces() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    set_workspace_snapshot(&conn, r#"{"tabs":[{"id":"t1"}]}"#).unwrap();
    let first = get_workspace_snapshot(&conn).unwrap().unwrap();
    assert!(first.contains("t1"));
    set_workspace_snapshot(&conn, r#"{"tabs":[{"id":"t2"}]}"#).unwrap();
    let second = get_workspace_snapshot(&conn).unwrap().unwrap();
    assert!(second.contains("t2"));
    assert!(!second.contains("t1"));
}

#[test]
fn migrate_creates_workspace_tables_when_versions_already_recorded() {
    let path = std::env::temp_dir().join(format!(
        "monocode-stale-migrations-{}-{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (
                   version INTEGER PRIMARY KEY,
                   applied_at INTEGER NOT NULL
                 );
                 CREATE TABLE sessions (
                   id TEXT PRIMARY KEY,
                   cwd TEXT NOT NULL,
                   harness TEXT NOT NULL,
                   model TEXT NOT NULL,
                   model_settings TEXT NOT NULL DEFAULT '{}',
                   runtime_mode TEXT NOT NULL,
                   title TEXT NOT NULL,
                   provider_session_id TEXT,
                   blocks_json TEXT NOT NULL DEFAULT '[]',
                   created_at INTEGER NOT NULL,
                   updated_at INTEGER NOT NULL
                 );
                 INSERT INTO schema_migrations (version, applied_at)
                   VALUES (1, 1), (2, 1), (3, 1), (4, 1), (5, 1);",
        )
        .unwrap();
    }
    let store = SessionStore::open(path.clone()).unwrap();
    let conn = store.conn.lock().unwrap();
    let inflight: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'in_flight_sessions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    let snapshot: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'workspace_snapshot'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    drop(conn);
    drop(store);
    let _ = std::fs::remove_file(&path);
    assert_eq!(inflight, 1);
    assert_eq!(snapshot, 1);
}

#[test]
fn migrate_restores_the_covering_index_when_versions_already_recorded() {
    let path = std::env::temp_dir().join(format!(
        "monocode-stale-index-{}-{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    {
        // A database that claims every version but carries none of the
        // columns or indexes those versions describe.
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (
                   version INTEGER PRIMARY KEY,
                   applied_at INTEGER NOT NULL
                 );
                 CREATE TABLE sessions (
                   id TEXT PRIMARY KEY,
                   cwd TEXT NOT NULL,
                   harness TEXT NOT NULL,
                   model TEXT NOT NULL,
                   model_settings TEXT NOT NULL DEFAULT '{}',
                   runtime_mode TEXT NOT NULL,
                   title TEXT NOT NULL,
                   provider_session_id TEXT,
                   blocks_json TEXT NOT NULL DEFAULT '[]',
                   created_at INTEGER NOT NULL,
                   updated_at INTEGER NOT NULL
                 );
                 INSERT INTO schema_migrations (version, applied_at)
                   VALUES (1, 1), (2, 1), (3, 1), (4, 1), (5, 1), (6, 1), (7, 1),
                          (8, 1), (9, 1), (10, 1), (11, 1), (12, 1), (13, 1),
                          (14, 1), (15, 1), (16, 1), (17, 1), (18, 1);",
        )
        .unwrap();
    }
    let store = SessionStore::open(path.clone()).unwrap();
    let conn = store.conn.lock().unwrap();
    let index: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index'
                   AND name = 'sessions_cwd_cover_idx'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(index, 1);
    upsert_session(&conn, &sample("s1", "/tmp/a", "Restored index")).unwrap();
    let result = search_sessions(
        &conn,
        &SessionSearchOptions {
            query: "Restored".into(),
            cwd: None,
            include_archived: false,
            search_owner: String::new(),
        },
    )
    .expect("unscoped search must not fail when the index was restored");
    assert!(result
        .hits
        .iter()
        .any(|hit| hit.session_id == "s1" && hit.kind == "conversation"));
    drop(conn);
    drop(store);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn session_search_uses_a_query_only_read_connection() {
    let path = std::env::temp_dir().join(format!(
        "monocode-session-read-conn-{}-{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = SessionStore::open(path.clone()).unwrap();
    upsert_session(
        &store.conn.lock().unwrap(),
        &sample("s1", "/tmp/a", "Searchable title"),
    )
    .unwrap();

    let read_conn = store.read_conn.lock().unwrap();
    let conn = read_conn.as_ref().unwrap();
    let result = search_sessions_with_connection(
        conn,
        &SessionSearchOptions {
            query: "Searchable".into(),
            cwd: None,
            include_archived: false,
            search_owner: String::new(),
        },
        None,
    )
    .unwrap();

    assert!(result.hits.iter().any(|hit| hit.session_id == "s1"));
    assert!(conn
        .execute("UPDATE sessions SET title = 'nope' WHERE id = 's1'", [])
        .is_err());
    drop(read_conn);
    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn oversized_transcript_keeps_title_hit_without_hiding_older_messages() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let mut small = sample("small", "/tmp/a", "Needle title");
    small.blocks = json!([{ "id": "small", "role": "user", "text": "needle" }]);
    upsert_session(&conn, &small).unwrap();
    let mut huge = sample("huge", "/tmp/a", "Needle title");
    huge.blocks = json!([{
        "id": "huge",
        "role": "user",
        "text": "needle ".repeat(1_500_000),
    }]);
    upsert_session(&conn, &huge).unwrap();
    conn.execute("UPDATE sessions SET updated_at = 1 WHERE id = 'small'", [])
        .unwrap();
    conn.execute("UPDATE sessions SET updated_at = 2 WHERE id = 'huge'", [])
        .unwrap();

    let result = search_sessions(
        &conn,
        &SessionSearchOptions {
            query: "needle".into(),
            cwd: None,
            include_archived: false,
            search_owner: String::new(),
        },
    )
    .unwrap();

    assert!(result.truncated);
    assert!(result
        .hits
        .iter()
        .any(|hit| hit.session_id == "huge" && hit.kind == "conversation"));
    assert!(!result
        .hits
        .iter()
        .any(|hit| hit.session_id == "huge" && hit.kind == "message"));
    assert!(result
        .hits
        .iter()
        .any(|hit| hit.session_id == "small" && hit.kind == "message"));
}

#[test]
fn session_search_without_a_cwd_avoids_the_sessions_table() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let sql = format!("EXPLAIN QUERY PLAN {}", search_sessions_sql(false, false));
    let mut statement = conn.prepare(&sql).unwrap();
    let plan: Vec<String> = statement
        .query_map(
            params!["%needle%", MAX_SEARCH_BUFFER_BYTES as i64, 401i64],
            |row| row.get::<_, String>(3),
        )
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert!(plan
        .iter()
        .any(|step| step.contains("USING INDEX sessions_cwd_cover_idx")));
    for step in &plan {
        if step.contains("sessions") {
            assert!(step.contains("USING INDEX"), "table access: {step}");
        }
    }
}

#[test]
fn a_new_session_search_supersedes_the_same_owner() {
    let owner = "session-search-owner-test";
    let first = begin_session_search(owner).unwrap();
    assert!(first.is_current());
    let second = begin_session_search(owner).unwrap();
    assert!(!first.is_current());
    assert!(second.is_current());
    cancel_owned_session_search(owner);
    assert!(!second.is_current());
}

#[test]
fn a_completed_session_search_releases_its_token() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    upsert_session(&conn, &sample("s1", "/tmp/a", "Needle title")).unwrap();
    drop(conn);
    let owner = "completed-session-search-owner";

    let result = search_store(
        &store,
        &SessionSearchOptions {
            query: "Needle".into(),
            cwd: None,
            include_archived: false,
            search_owner: owner.into(),
        },
    )
    .unwrap();

    assert!(result
        .hits
        .iter()
        .any(|hit| hit.session_id == "s1" && hit.kind == "conversation"));
    assert!(!session_search_token_registered(owner));
}

#[test]
fn a_superseded_session_search_does_not_release_the_newer_token() {
    let owner = "superseded-release-owner";
    let first = begin_session_search(owner).unwrap();
    let second = begin_session_search(owner).unwrap();

    end_session_search(Some(&first));

    assert!(session_search_token_registered(owner));
    assert!(!first.is_current());
    assert!(second.is_current());
    cancel_owned_session_search(owner);
}

#[test]
fn cancelling_one_session_search_does_not_cancel_another() {
    let first = begin_session_search("first-session-owner").unwrap();
    let second = begin_session_search("second-session-owner").unwrap();

    cancel_owned_session_search("first-session-owner");

    assert!(!first.is_current());
    assert!(second.is_current());
}

#[test]
fn a_superseded_session_search_returns_no_hits() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    upsert_session(&conn, &sample("s1", "/tmp/a", "Searchable title")).unwrap();
    let owner = "superseded-session-search-test";
    let stale = begin_session_search(owner).unwrap();
    begin_session_search(owner);
    let options = SessionSearchOptions {
        query: "Searchable".into(),
        cwd: None,
        include_archived: false,
        search_owner: owner.into(),
    };

    let result = search_sessions_with_connection(&conn, &options, Some(&stale)).unwrap();

    assert!(result.hits.is_empty());
    assert!(!result.truncated);
    // The progress handler belongs to the search, not the shared connection.
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    cancel_owned_session_search(owner);
}

#[test]
fn a_failed_session_search_releases_its_token() {
    let store = SessionStore::open_in_memory().unwrap();
    // Point the read connection at a database with no schema, so the search
    // fails inside SQLite. That error return is the path that used to skip
    // the release, and `SearchView` swallows the rejection.
    *store.read_conn.lock().unwrap() = Some(Connection::open_in_memory().unwrap());
    let owner = "failed-session-search-owner";

    let error = search_store(
        &store,
        &SessionSearchOptions {
            query: "Needle".into(),
            cwd: None,
            include_archived: false,
            search_owner: owner.into(),
        },
    );

    assert!(error.is_err(), "a search with no schema must fail");
    assert!(!session_search_token_registered(owner));
}

#[test]
fn a_test_helper_session_search_releases_its_token() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    upsert_session(&conn, &sample("s1", "/tmp/a", "Needle title")).unwrap();
    let owner = "helper-session-search-owner";

    let result = search_sessions(
        &conn,
        &SessionSearchOptions {
            query: "Needle".into(),
            cwd: None,
            include_archived: false,
            search_owner: owner.into(),
        },
    )
    .unwrap();

    assert!(result
        .hits
        .iter()
        .any(|hit| hit.session_id == "s1" && hit.kind == "conversation"));
    assert!(!session_search_token_registered(owner));
}

#[test]
fn search_uses_top_level_user_blocks_for_the_materialized_filter() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let mut session = sample("s1", "/tmp/a", "Unrelated title");
    session.blocks = json!([
        { "id": "t1", "role": "tool", "text": "wrapper", "tool": {
            "preview": { "output": "nested role: user needle" }
        } }
    ]);
    upsert_session(&conn, &session).unwrap();

    let result = search_sessions(
        &conn,
        &SessionSearchOptions {
            query: "needle".into(),
            cwd: None,
            include_archived: false,
            search_owner: String::new(),
        },
    )
    .unwrap();

    assert!(result.hits.is_empty());
}

#[test]
fn search_finds_title_and_message_hits() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    let mut session = sample("s1", "/tmp/a", "Fix sidebar search");
    session.blocks = json!([
        { "id": "u1", "role": "user", "text": "Please search the sidebar filter chips" },
        { "id": "a1", "role": "assistant", "text": "I will look through the explorer next." }
    ]);
    upsert_session(&conn, &session).unwrap();
    upsert_session(&conn, &sample("s2", "/tmp/b", "Unrelated title")).unwrap();

    let result = search_sessions(
        &conn,
        &SessionSearchOptions {
            query: "sidebar".into(),
            cwd: None,
            include_archived: false,
            search_owner: String::new(),
        },
    )
    .unwrap();
    let kinds: Vec<_> = result
        .hits
        .iter()
        .map(|hit| {
            (
                hit.kind.as_str(),
                hit.session_id.as_str(),
                hit.block_id.as_deref(),
            )
        })
        .collect();
    assert!(kinds.contains(&("conversation", "s1", None)));
    assert!(kinds.contains(&("message", "s1", Some("u1"))));
    assert!(!result.hits.iter().any(|hit| hit.session_id == "s2"));
}

#[test]
fn search_skips_inbox_ask_sessions_when_scoped_and_unscoped() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    upsert_session(&conn, &sample("ask", "/tmp/a", "Needle ask")).unwrap();
    upsert_session(&conn, &sample("chat", "/tmp/a", "Needle chat")).unwrap();
    conn.execute("UPDATE sessions SET inbox_ask = '{}' WHERE id = 'ask'", [])
        .unwrap();

    for cwd in [None, Some("/tmp/a".to_string())] {
        let result = search_sessions(
            &conn,
            &SessionSearchOptions {
                query: "Needle".into(),
                cwd,
                include_archived: false,
                search_owner: String::new(),
            },
        )
        .unwrap();
        assert!(result.hits.iter().all(|hit| hit.session_id == "chat"));
    }
}

#[test]
fn search_respects_cwd_and_skips_archived() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    upsert_session(&conn, &sample("s1", "/tmp/a", "Search project a")).unwrap();
    upsert_session(&conn, &sample("s2", "/tmp/b", "Search project b")).unwrap();
    set_archived(&conn, "s1", true).unwrap();

    let scoped = search_sessions(
        &conn,
        &SessionSearchOptions {
            query: "Search project".into(),
            cwd: Some("/tmp/a".into()),
            include_archived: false,
            search_owner: String::new(),
        },
    )
    .unwrap();
    assert!(scoped.hits.is_empty());

    let with_archived = search_sessions(
        &conn,
        &SessionSearchOptions {
            query: "Search project".into(),
            cwd: Some("/tmp/a".into()),
            include_archived: true,
            search_owner: String::new(),
        },
    )
    .unwrap();
    assert!(with_archived
        .hits
        .iter()
        .any(|hit| hit.session_id == "s1" && hit.kind == "conversation"));

    let other = search_sessions(
        &conn,
        &SessionSearchOptions {
            query: "Search project".into(),
            cwd: Some("/tmp/b".into()),
            include_archived: false,
            search_owner: String::new(),
        },
    )
    .unwrap();
    assert!(other.hits.iter().any(|hit| hit.session_id == "s2"));
}

#[test]
fn search_empty_query_is_empty() {
    let store = SessionStore::open_in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    upsert_session(&conn, &sample("s1", "/tmp/a", "Anything")).unwrap();
    let result = search_sessions(
        &conn,
        &SessionSearchOptions {
            query: "   ".into(),
            cwd: None,
            include_archived: false,
            search_owner: String::new(),
        },
    )
    .unwrap();
    assert!(result.hits.is_empty());
    assert!(!result.truncated);
}
