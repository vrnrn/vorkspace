use super::*;

#[test]
fn streamed_ordinary_load_retains_rich_payloads_across_append_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.db");
    let mut store = Store::open_vorkspace(&path).unwrap();
    store
        .conn
        .execute_batch(include_str!("../../../tests/fixtures/ordinary-schema.sql"))
        .unwrap();
    let rich = (0..64)
        .map(|i| {
            if i % 3 == 0 {
                json!({
                    "id":format!("block-{i}"), "role":"tool", "text":"Saved tool",
                    "tool":{"kind":"execute","title":"cat saved.txt",
                        "detail":"🙂 café 漢字\n".repeat(8_000),"future":{"keep":true}},
                    "future":{"ordinal":i},
                })
            } else {
                json!({
                    "id":format!("block-{i}"), "role":if i % 2 == 0 {"user"} else {"assistant"},
                    "text":format!("Saved message {i}: 🙂 café 漢字"),
                    "draft":i == 5,"future":{"ordinal":i},
                    "turnModel":{"harness":"codex","name":"Saved model","id":"codex:saved"},
                    "providerTurnId":format!("turn-{i}"),"durationMs":i * 10,"startedAt":123,
                    "turnMetrics":{"inputTokens":50,"outputTokens":10},
                })
            }
        })
        .collect::<Vec<_>>();
    for source in [Vec::new(), rich] {
        let id = store.create("codex", "/project").unwrap().id;
        let original = store.persisted_session_id(id).unwrap();
        store
            .conn
            .execute(
                "UPDATE sessions SET blocks_json='not a fallback transcript' WHERE id=?1",
                [&original],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO ordinary_transcripts VALUES(?1,0,1)",
                [&original],
            )
            .unwrap();
        // Stored position, not insertion order, determines the display.
        for (position, block) in source.iter().enumerate().rev() {
            store
                .conn
                .execute(
                    "INSERT INTO ordinary_blocks VALUES(?1,?2,?3,?4,?5,?6)",
                    params![
                        original,
                        block["id"].as_str().unwrap(),
                        position as i64,
                        block.to_string(),
                        block["role"] == "user",
                        block["draft"] == true
                    ],
                )
                .unwrap();
        }
        let mut loaded = store.load(id).unwrap();
        assert_eq!(loaded.rows, blocks::project(&source).unwrap());
        assert_eq!(
            serde_json::to_value(store.agent_settings(id).unwrap().unwrap().turns).unwrap(),
            serde_json::to_value(project_turns(&source).unwrap()).unwrap()
        );
        assert_eq!(
            store.shared.as_ref().unwrap().snapshots.borrow()[&id],
            HistoryRevision::of(&source)
        );
        let from = loaded.begin("user");
        loaded.append("Continue the same conversation");
        store.write_rows(id, from, &loaded.rows[from..]).unwrap();
        drop(store);
        store = Store::open_vorkspace(&path).unwrap();
        let id = store.resolve_session_id(&original).unwrap();
        assert_eq!(store.load(id).unwrap().rows, loaded.rows);
        assert_eq!(&store.shared_blocks(id).unwrap()[..source.len()], &source);
        store.conn.execute(
            "UPDATE ordinary_blocks SET block_json=json_set(block_json,'$.future.external',true)
             WHERE session_id=?1 AND position=0", [&original],
        ).unwrap();
        assert!(store
            .write_rows(id, from, &loaded.rows[from..])
            .unwrap_err()
            .contains("changed in another window"));
    }
}

#[test]
fn preference_only_save_never_reads_history_or_discards_unknown_model_controls() {
    let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let id = store.create("codex", "/project").unwrap().id;
    let original = store.persisted_session_id(id).unwrap();
    store
        .conn
        .execute(
            "UPDATE sessions SET model_settings=?2,blocks_json='unavailable history' WHERE id=?1",
            params![
                original,
                json!({"future-control":"retained","effort":"low"}).to_string()
            ],
        )
        .unwrap();
    let settings = Settings {
        effort: Some("high".into()),
        ..Default::default()
    };
    store.shared_save_agent_preferences(id, &settings).unwrap();
    let raw: String = store
        .conn
        .query_row(
            "SELECT model_settings FROM sessions WHERE id=?1",
            [&original],
            |row| row.get(0),
        )
        .unwrap();
    let controls: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(controls["future-control"], "retained");
    assert_eq!(controls["effort"], "high");
    let history: String = store
        .conn
        .query_row(
            "SELECT blocks_json FROM sessions WHERE id=?1",
            [&original],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(history, "unavailable history");
}

#[test]
fn streamed_indexed_bot_load_preserves_projection_and_revision_guards() {
    let mut store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    store
        .conn
        .execute_batch(include_str!("../../../tests/fixtures/bot-schema.sql"))
        .unwrap();
    let blocks = (0..1_000)
        .map(|i| {
            json!({
                "id":format!("block-{i}"), "role":if i % 2 == 0 { "user" } else { "assistant" },
                "text":format!("Saved message {i}: 🙂 café 漢字"),
                "future":{"preserved":true,"ordinal":i},
            })
        })
        .collect::<Vec<_>>();
    for blocks in [Vec::new(), blocks] {
        let id = store.create("codex", "/project").unwrap().id;
        let original = store.persisted_session_id(id).unwrap();
        store
            .conn
            .execute("INSERT INTO bot_transcripts VALUES(?1)", [&original])
            .unwrap();
        store.shared_store_blocks(id, &blocks).unwrap();
        let mut loaded = store.load(id).unwrap();
        assert_eq!(loaded.rows, blocks::project(&blocks).unwrap());
        assert_eq!(
            store.shared.as_ref().unwrap().snapshots.borrow()[&id],
            HistoryRevision::of(&blocks)
        );
        let from = loaded.begin("user");
        loaded.append("Continue the same conversation");
        store.write_rows(id, from, &loaded.rows[from..]).unwrap();
        assert_eq!(&store.shared_blocks(id).unwrap()[..blocks.len()], &blocks);
        store.conn.execute("UPDATE bot_blocks SET block_json=json_set(block_json,'$.future.external',true) WHERE session_id=?1 AND seq=1", [&original]).unwrap();
        let error = store
            .write_rows(id, from, &loaded.rows[from..])
            .unwrap_err();
        assert!(error.contains("changed in another window"), "{error}");
    }
}

#[test]
fn indexed_bot_stale_older_edit_rejects_append_then_reload_allows_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let mut store = Store::open_vorkspace(&path).unwrap();
    store
        .conn
        .execute_batch(include_str!("../../../tests/fixtures/bot-schema.sql"))
        .unwrap();
    let id = store.create("codex", "/project").unwrap().id;
    let original = store.persisted_session_id(id).unwrap();
    store
        .conn
        .execute("INSERT INTO bot_transcripts VALUES(?1)", [&original])
        .unwrap();
    let blocks: Vec<_> = (0..40)
        .map(|i| {
            json!({
                "id": format!("block-{i}"), "role": if i % 2 == 0 { "user" } else { "assistant" },
                "text": format!("Message {i}"), "future": {"ordinal": i}
            })
        })
        .collect();
    store.shared_store_blocks(id, &blocks).unwrap();
    let mut stale = store.load(id).unwrap();
    let from = stale.begin("user");
    stale.append("Pending append");
    let other = Connection::open(&path).unwrap();
    other.execute("UPDATE bot_blocks SET block_json=json_set(block_json,'$.text','External edit') WHERE session_id=?1 AND seq=1", [&original]).unwrap();
    assert!(
        store
            .write_rows(id, from, &stale.rows[from..])
            .unwrap_err()
            .contains("changed in another window")
    );
    let mut fresh = store.load(id).unwrap();
    assert_eq!(fresh.rows[0].text, "External edit");
    let from = fresh.begin("user");
    fresh.append("Pending append");
    store.write_rows(id, from, &fresh.rows[from..]).unwrap();
    drop(store);
    let store = Store::open_vorkspace(&path).unwrap();
    let id = store.resolve_session_id(&original).unwrap();
    let saved = store.shared_blocks(id).unwrap();
    assert_eq!(saved.len(), 41);
    assert_eq!(saved[0]["text"], "External edit");
    for (i, block) in saved[..40].iter().enumerate() {
        assert_eq!(block["id"], format!("block-{i}"));
        assert_eq!(block["future"]["ordinal"], i);
    }
    assert_eq!(saved[40]["text"], "Pending append");
}

#[test]
fn desktop_sidebar_recovers_only_hidden_launches_of_listed_callers_without_history_reads() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("vorkspace.db");
    let store = Store::open_vorkspace(&path).unwrap();
    store.conn.execute_batch("CREATE TABLE orchestration_workers(session_id TEXT PRIMARY KEY, lead_id TEXT NOT NULL)").unwrap();
    for (id, cwd, hidden, user) in [
        ("listed-caller", "/project", false, true),
        ("app-listed-caller-review-task", "/project", true, true),
        ("app-listed-caller-other-project", "/other", true, true),
        ("bot-conversation", "/project", true, true),
        ("app-bot-conversation-task", "/project", true, true),
        ("blank-caller", "/project", false, false),
        ("app-blank-caller-task", "/project", true, true),
        ("worker", "/project", false, true),
        ("app-worker-task", "/project", true, true),
        ("inbox", "/project", false, true),
    ] {
        store.conn.execute("INSERT INTO sessions(id,cwd,harness,model,runtime_mode,title,blocks_json,created_at,updated_at,has_user_message,sidebar_hidden) VALUES(?1,?2,'codex','codex:model','supervised',?1,'not JSON',1,2,?3,?4)", params![id,cwd,user,hidden]).unwrap();
    }
    store
        .conn
        .execute(
            "INSERT INTO orchestration_workers VALUES('worker','listed-caller')",
            [],
        )
        .unwrap();
    store
        .conn
        .execute("UPDATE sessions SET inbox_ask='ask' WHERE id='inbox'", [])
        .unwrap();
    let schema_version: i64 = store
        .conn
        .query_row("PRAGMA schema_version", [], |r| r.get(0))
        .unwrap();
    drop(store);
    let original = std::fs::read(&path).unwrap();
    let store = Store::read_vorkspace(&path).unwrap();
    let sessions = store.sessions().unwrap();
    let ids: HashSet<_> = sessions
        .iter()
        .map(|s| store.persisted_session_id(s.id).unwrap())
        .collect();
    assert_eq!(
        ids,
        HashSet::from([
            "listed-caller".into(),
            "app-listed-caller-review-task".into(),
            "blank-caller".into()
        ])
    );
    let metadata = store.session_sidebar_metadata(&["/project".into()]);
    let child = store
        .resolve_existing_session_id("app-listed-caller-review-task")
        .unwrap();
    let parent = store.resolve_existing_session_id("listed-caller").unwrap();
    assert!(metadata.sidebar_hidden.contains(&child));
    assert_eq!(metadata.started_by[&child], parent);
    assert_eq!(
        store
            .conn
            .query_row("PRAGMA schema_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        schema_version
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!path.with_file_name("vorkspace.db.gpui.json").exists());
}

#[test]
fn desktop_workspace_snapshot_restores_existing_ids_and_keeps_native_override() {
    let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let original = store.persisted_session_id(session.id).unwrap();
    let source = json!({"projectCwd":"/project","activeTabId":"saved-tab","tabs":[
        {"id":"saved-tab","focusedId":original,"layout":{"type":"leaf","id":original}},
        {"id":"stale-tab","layout":{"type":"leaf","id":"999999"}}
    ]});
    store
        .conn
        .execute(
            "INSERT INTO workspace_snapshot VALUES(1,?1,1)",
            [source.to_string()],
        )
        .unwrap();
    assert_eq!(
        store.resolve_existing_session_id(&original),
        Some(session.id)
    );
    assert_eq!(store.resolve_existing_session_id("999999"), None);
    let workspace: Value = parse(&store.ui_state().unwrap().unwrap()).unwrap();
    assert_eq!(workspace["tabs"], json!([session.id]));
    assert_eq!(workspace["active"], session.id);
    assert_eq!(workspace["desktop_workspace_snapshot"], source);
    store
        .save_ui_state(r#"{"project":"/native","tabs":[]}"#)
        .unwrap();
    let workspace: Value = parse(&store.ui_state().unwrap().unwrap()).unwrap();
    assert_eq!(workspace["project"], "/native");
    assert_eq!(workspace["tabs"], json!([]));
    assert_eq!(workspace["desktop_workspace_snapshot"], source);
}

#[test]
fn search_connections_share_the_same_read_only_preference_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_vorkspace(&directory.path().join("vorkspace.db")).unwrap();
    let reader = store.search_reader().unwrap().unwrap();
    assert!(std::ptr::eq(
        store.desktop_preferences().unwrap(),
        reader.desktop_preferences().unwrap()
    ));
    assert!(reader.create("codex", "/project").is_err());
}

#[test]
fn original_desktop_schema_opens_without_schema_or_history_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let schema = include_str!("../../../tests/fixtures/legacy-schema.sql");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(schema).unwrap();
    conn.execute("INSERT INTO sessions VALUES('old-string-id','/project','codex','codex:old','{}','supervised','Old title','thread','[{\"id\":\"u\",\"role\":\"user\",\"text\":\"Old history\"}]',1,2)",[]).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA schema_version", [], |r| r.get(0))
        .unwrap();
    let original = std::fs::read(&path).unwrap();
    let store = Store::open_vorkspace(&path).unwrap();
    let session = store.sessions().unwrap().pop().unwrap();
    assert_eq!(
        store.persisted_session_id(session.id).unwrap(),
        "old-string-id"
    );
    assert_eq!(store.load(session.id).unwrap().rows[0].text, "Old history");
    assert_eq!(
        conn.query_row("PRAGMA schema_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        version
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!path.with_file_name("vorkspace.db.gpui.json").exists());
}

#[test]
fn indexed_bot_history_stays_in_its_desktop_tables_with_block_ids_intact() {
    let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let (id, source) = saved(&store, json!([]));
    store
        .conn
        .execute_batch(include_str!("../../../tests/fixtures/bot-schema.sql"))
        .unwrap();
    store
        .conn
        .execute("INSERT INTO bot_transcripts VALUES(?1)", [&source])
        .unwrap();
    let blocks = vec![
        json!({"id":"bot-prompt","role":"user","text":"Bot history","future":"keep"}),
        json!({"id":"bot-answer","role":"assistant","text":"Original answer"}),
        json!({"id":"bot-draft","role":"user","text":"Draft","draft":true}),
    ];
    let tx = store.conn.unchecked_transaction().unwrap();
    store.shared_store_blocks(id, &blocks).unwrap();
    tx.commit().unwrap();
    let first_raw: String = store
        .conn
        .query_row(
            "SELECT block_json FROM bot_blocks WHERE block_id='bot-prompt'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let t = store.load(id).unwrap();
    assert_eq!(t.rows[0].text, "Bot history");
    let mut state = store.follow_ups(id).unwrap();
    state.draft = None;
    state.revision += 1;
    store.save_follow_ups(id, &state).unwrap();
    let mut t = store.load(id).unwrap();
    let from = t.begin("user");
    t.append("Continue");
    let mut store = store;
    store.write_rows(id, from, &t.rows[from..]).unwrap();
    assert_eq!(
        store
            .conn
            .query_row(
                "SELECT blocks_json FROM sessions WHERE id=?1",
                [&source],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "[]"
    );
    assert_eq!(
        store
            .conn
            .query_row(
                "SELECT block_json FROM bot_blocks WHERE block_id='bot-prompt'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        first_raw
    );
    assert_eq!(store.shared_blocks(id).unwrap()[0], blocks[0]);
    assert_eq!(
        store.shared_blocks(id).unwrap().last().unwrap()["text"],
        "Continue"
    );
}

#[test]
fn desktop_open_uses_existing_records_and_does_not_rewrite_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let row = store.create("codex", "/project").unwrap();
    let id = store.persisted_session_id(row.id).unwrap();
    assert!(id.parse::<i64>().is_err());
    assert_eq!(store.sessions().unwrap().len(), 1);
    drop(store);
    let original = std::fs::read(&path).unwrap();
    let store = Store::open_vorkspace(&path).unwrap();
    assert_eq!(store.sessions().unwrap()[0].id, row.id);
    assert_eq!(store.persisted_session_id(row.id).unwrap(), id);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!has_table(&store.conn, "native_sessions").unwrap());
}

fn saved(store: &Store, blocks: Value) -> (i64, String) {
    let original = "existing-session-uuid";
    store.conn.execute("INSERT INTO sessions(id,cwd,harness,model,model_settings,runtime_mode,title,provider_session_id,
        blocks_json,created_at,updated_at,has_user_message,provider_account_id)
        VALUES(?1,'/project','codex','codex:model-one','{\"reasoningEffort\":\"high\",\"futureSetting\":\"keep\"}',
        'auto-accept-edits','Original title','provider-thread',?2,100,200,1,'account-original')",
        params![original,blocks.to_string()]).unwrap();
    let session = store
        .sessions()
        .unwrap()
        .into_iter()
        .find(|s| store.persisted_session_id(s.id).unwrap() == original)
        .unwrap();
    (session.id, original.into())
}

#[test]
fn desktop_model_and_effort_decode_and_round_trip_without_rewriting_the_title() {
    let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let (id, source) = saved(&store, json!([]));
    store.conn.execute(
        "UPDATE sessions SET model='codex:gpt-6.1-sol',model_settings=?2,title='codex · Fix sidebar' WHERE id=?1",
        params![source, r#"{"reasoningEffort":"xhigh","serviceTier":"default"}"#],
    ).unwrap();
    let mut settings = store.agent_settings(id).unwrap().unwrap();
    assert_eq!(settings.model.as_deref(), Some("gpt-6.1-sol"));
    assert_eq!(settings.effort.as_deref(), Some("xhigh"));
    settings.effort = Some("high".into());
    store.save_agent_settings(id, &settings).unwrap();
    let saved = store.agent_settings(id).unwrap().unwrap();
    assert_eq!(saved.model, settings.model);
    assert_eq!(saved.effort, settings.effort);
    let (model, title, controls): (String, String, String) = store
        .conn
        .query_row(
            "SELECT model,title,model_settings FROM sessions WHERE id=?1",
            [&source],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(model, "codex:gpt-6.1-sol");
    assert_eq!(title, "codex · Fix sidebar");
    let controls: Value = parse(&controls).unwrap();
    assert_eq!(controls["reasoningEffort"], "high");
    assert_eq!(controls["serviceTier"], "default");
}

#[test]
fn writes_are_visible_in_desktop_tables_and_preserve_rich_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let mut store = Store::open_vorkspace(&path).unwrap();
    let original = json!([
        {"id":"prompt","role":"user","text":"Original 🙂","startedAt":100,
            "attachments":[{"id":"a","path":"/image.png","mimeType":"image/png","kind":"image","size":50}],
            "turnModel":{"harness":"codex","id":"codex:model-one","name":"Model"},
            "futureMetadata":{"keep":[1,2,3]}},
        {"id":"hidden","role":"user","text":"Internal","internal":true,"unknown":"preserve"},
        {"id":"image","role":"image","text":"","image":{"path":"/generated.png","future":"keep"}},
        {"id":"tool","role":"tool","text":"Read file","tool":{"callId":"call","kind":"read","title":"Read file",
            "status":"completed","detail":"🙂".repeat(30000),"preview":{"path":"/source.rs"},"futureToolField":[1,2]}},
        {"id":"reply","role":"assistant","text":"Original reply","artifactCards":[{"id":"document","future":true}]}
    ]);
    let (id, source) = saved(&store, original.clone());
    let mut t = store.load(id).unwrap();
    let from = t.rows.len();
    t.begin("user");
    t.append("New turn");
    t.begin("assistant");
    t.append("New reply");
    store.write_rows(id, from, &t.rows[from..]).unwrap();
    // First native save atomically normalizes rich source blocks; the old
    // sessions.blocks_json cache is no longer the authoritative history.
    let blocks = store.shared_blocks(id).unwrap();
    assert_eq!(&blocks[..5], original.as_array().unwrap());
    assert_eq!(blocks[5]["role"], "user");
    assert_eq!(blocks[6]["text"], "New reply");
    t.append(" continued");
    store
        .write_rows(id, t.rows.len() - 1, &t.rows[t.rows.len() - 1..])
        .unwrap();
    let blocks = store.shared_blocks(id).unwrap();
    assert_eq!(blocks[6]["text"], "New reply continued");
    assert_eq!(&blocks[..5], original.as_array().unwrap());
    store.title(id, "Renamed").unwrap();
    store.set_pinned(&[id], true).unwrap();
    store.set_archived(id, true).unwrap();
    let mut settings = store.agent_settings(id).unwrap().unwrap();
    assert_eq!(settings.account.id, "account-original");
    assert_eq!(settings.model.as_deref(), Some("model-one"));
    settings.effort = Some("low".into());
    store.save_agent_settings(id, &settings).unwrap();
    let (title,pinned,archived,controls,provider,model):(String,bool,bool,String,String,String)=store.conn.query_row(
        "SELECT title,pinned,archived,model_settings,provider_session_id,model FROM sessions WHERE id=?1",[&source],
        |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();
    assert_eq!(title, "Renamed");
    assert!(pinned && archived);
    assert_eq!(provider, "provider-thread");
    assert_eq!(model, "codex:model-one");
    let controls: Value = parse(&controls).unwrap();
    assert_eq!(controls["futureSetting"], "keep");
    assert_eq!(controls["reasoningEffort"], "low");
    drop(store);
    let store = Store::open_vorkspace(&path).unwrap();
    assert_eq!(store.persisted_session_id(id).unwrap(), source);
    assert_eq!(
        store
            .load(id)
            .unwrap()
            .rows
            .iter()
            .filter(|r| r.first && r.role == "user")
            .count(),
        2
    );
}

#[test]
fn concurrent_desktop_changes_are_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let mut store = Store::open_vorkspace(&path).unwrap();
    let (id, source) = saved(&store, json!([{"id":"u","role":"user","text":"Original"}]));
    let t = store.load(id).unwrap();
    let other = Connection::open(&path).unwrap();
    let changed = json!([{"id":"u","role":"user","text":"Edited in desktop","futureField":42}]);
    other
        .execute(
            "UPDATE sessions SET blocks_json=?2 WHERE id=?1",
            params![source, changed.to_string()],
        )
        .unwrap();
    let error = store.write_rows(id, 0, &t.rows).unwrap_err();
    assert!(error.contains("changed in another window"));
    assert_eq!(
        store.shared_blocks(id).unwrap(),
        changed.as_array().unwrap().clone()
    );
    assert!(store.save_follow_ups(id, &Default::default()).is_err());
}

#[test]
fn fast_forward_append_in_another_window_allows_saving_drafts_and_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let (id, source) = saved(&store, json!([{"id":"u","role":"user","text":"Original"}]));
    let _ = store.load(id).unwrap();
    let other = Connection::open(&path).unwrap();
    let appended = json!([
        {"id":"u","role":"user","text":"Original"},
        {"id":"a","role":"assistant","text":"New reply from another window"}
    ]);
    other
        .execute(
            "UPDATE sessions SET blocks_json=?2 WHERE id=?1",
            params![source, appended.to_string()],
        )
        .unwrap();
    let state = crate::follow_up::State {
        draft: Some(crate::follow_up::Message {
            id: "draft-1".into(),
            text: "My new draft prompt".into(),
            at: 100,
            ..Default::default()
        }),
        ..Default::default()
    };
    if let Err(e) = store.save_follow_ups(id, &state) {
        panic!("save_follow_ups failed with: {e}");
    }
    let blocks = store.shared_blocks(id).unwrap();
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0]["text"], "Original");
    assert_eq!(blocks[1]["text"], "New reply from another window");
    assert_eq!(blocks[2]["draft"], true);
    assert_eq!(blocks[2]["text"], "My new draft prompt");
    let mut settings = store.agent_settings(id).unwrap().unwrap();
    settings.effort = Some("low".into());
    store.save_agent_settings(id, &settings).unwrap();
    drop(store);
    let store = Store::open_vorkspace(&path).unwrap();
    assert_eq!(store.shared_blocks(id).unwrap(), blocks);
    assert_eq!(
        store.agent_settings(id).unwrap().unwrap().effort.as_deref(),
        Some("low")
    );
}

#[test]
fn stale_rich_history_rejects_draft_save_with_bounded_hashing_and_recovers_on_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let mut source: Vec<Value> = (0..128)
        .map(|i| {
            json!({"id":format!("u-{i}"),"role":"user","text":"Saved",
            "future":{"attachment":"x".repeat(1_024)}})
        })
        .collect();
    let (id, original) = saved(&store, json!(source));
    store.load(id).unwrap();
    let mut pending = store.follow_ups(id).unwrap();
    pending.draft = Some(crate::follow_up::Message {
        id: "unsent".into(),
        text: "Preserve this input".into(),
        at: source.len(),
        ..Default::default()
    });
    source[127]["future"]["attachment"] = json!("Edited in another window");
    let other = Connection::open(&path).unwrap();
    other
        .execute(
            "UPDATE sessions SET blocks_json=?2 WHERE id=?1",
            params![original, encode(&source).unwrap()],
        )
        .unwrap();
    HISTORY_REVISION_HASH_BLOCKS.with(|count| count.set(0));
    assert!(store
        .save_follow_ups(id, &pending)
        .unwrap_err()
        .contains("changed in another window"));
    let hashed = HISTORY_REVISION_HASH_BLOCKS.with(|count| count.get());
    assert!(hashed <= 3 * source.len(), "Hashed {hashed} source blocks");
    assert_eq!(store.shared_blocks(id).unwrap(), source);
    drop(store);
    let store = Store::open_vorkspace(&path).unwrap();
    store.load(id).unwrap();
    let mut fresh = store.follow_ups(id).unwrap();
    fresh.draft = pending.draft;
    store.save_follow_ups(id, &fresh).unwrap();
    drop(store);
    let store = Store::open_vorkspace(&path).unwrap();
    assert_eq!(&store.shared_blocks(id).unwrap()[..source.len()], source);
    assert_eq!(
        store.follow_ups(id).unwrap().draft.unwrap().text,
        "Preserve this input"
    );
}

#[test]
fn configuring_the_same_provider_preserves_its_resume_id() {
    let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let (id, _) = saved(&store, json!([]));
    let settings = store.agent_settings(id).unwrap().unwrap();
    store.configure_agent(id, "codex", &settings).unwrap();
    assert_eq!(
        store.sessions().unwrap()[0].provider_id.as_deref(),
        Some("provider-thread")
    );
    store
        .reset_provider_id_if(id, Some("another-thread"))
        .unwrap();
    assert_eq!(
        store.sessions().unwrap()[0].provider_id.as_deref(),
        Some("provider-thread")
    );
    store.change_provider(id, "claude").unwrap();
    assert_eq!(store.sessions().unwrap()[0].provider, "claude");
    assert!(store.sessions().unwrap()[0].provider_id.is_none());
}

#[test]
fn concurrent_queue_changes_are_preserved_and_transaction_is_rolled_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let (id, source) = saved(&store, json!([]));
    let mut state = store.follow_ups(id).unwrap();
    state.revision += 1;
    let other = Connection::open(&path).unwrap();
    let queued = json!([{"id":"desktop-message","text":"Keep this","future":true}]).to_string();
    other
        .execute(
            "UPDATE sessions SET queued_messages_json=?2 WHERE id=?1",
            params![source, queued],
        )
        .unwrap();
    assert!(store
        .save_follow_ups(id, &state)
        .unwrap_err()
        .contains("queue changed"));
    let raw: String = other
        .query_row(
            "SELECT queued_messages_json FROM sessions WHERE id=?1",
            [&source],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(raw, queued);
    // The failed save releases its transaction and permits a fresh load/save.
    let mut fresh = store.follow_ups(id).unwrap();
    fresh.revision += 1;
    store.save_follow_ups(id, &fresh).unwrap();
    assert_eq!(store.follow_ups(id).unwrap().messages[0].text, "Keep this");
}

#[test]
fn drafts_queues_and_receipts_keep_desktop_metadata_and_do_not_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let mut store = Store::open_vorkspace(&path).unwrap();
    let (id, source) = saved(
        &store,
        json!([
            {"id":"u","role":"user","text":"Sent"},
            {"id":"draft","role":"user","text":"Unsent","draft":true,"futureDraft":"keep"}
        ]),
    );
    let queued = json!([{"id":"queued","text":"Next","attachments":[{"id":"image","path":"/image.png","future":"keep"}],"futureQueue":true}]);
    store
        .conn
        .execute(
            "UPDATE sessions SET queued_messages_json=?2,is_draft=1 WHERE id=?1",
            params![source, queued.to_string()],
        )
        .unwrap();
    let mut t = store.load(id).unwrap();
    let mut state = store.follow_ups(id).unwrap();
    assert!(state.paused);
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.draft.as_ref().unwrap().text, "Unsent");
    state.messages[0].text = "Edited queued text".into();
    state.revision += 1;
    store.save_follow_ups(id, &state).unwrap();
    let raw: String = store
        .conn
        .query_row(
            "SELECT queued_messages_json FROM sessions WHERE id=?1",
            [&source],
            |r| r.get(0),
        )
        .unwrap();
    let queue: Value = parse(&raw).unwrap();
    assert_eq!(queue[0]["futureQueue"], true);
    assert_eq!(queue[0]["attachments"], queued[0]["attachments"]);
    assert_eq!(store.shared_blocks(id).unwrap()[1]["futureDraft"], "keep");
    let from = t.begin("user");
    t.append("Edited queued text");
    store
        .write_rows_with_message(id, from, &t.rows[from..], Some("queued"))
        .unwrap();
    drop(store);
    let store = Store::open_vorkspace(&path).unwrap();
    assert!(store.follow_ups(id).unwrap().messages.is_empty());
    let raw: String = store
        .conn
        .query_row(
            "SELECT queued_messages_json FROM sessions WHERE id=?1",
            [&source],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(raw, "[]");
    assert_eq!(
        store
            .setting(&format!("bot-delivered:{id}:queued"))
            .unwrap(),
        Some("1".into())
    );
}

#[test]
fn notes_reminders_search_and_read_only_connections_use_the_same_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let mut store = Store::open_vorkspace(&path).unwrap();
    let (id, source) = saved(
        &store,
        json!([{"id":"u","role":"user","text":"A searchable message"}]),
    );
    store.conn.execute("INSERT INTO notes(id,slug,title,body,tags_json,source_session_id,source_cwd,created_at,updated_at,artifact_kind)
        VALUES('existing-note','stable-slug','Note','Body','[\"tag\"]',?1,'/project',1,2,'document')",[&source]).unwrap();
    let mut note = store.notes().unwrap().pop().unwrap();
    assert_eq!(note.source_session, id);
    note.body = "Edited note".into();
    store.save_note(&note).unwrap();
    let (slug, body, kind): (String, String, String) = store
        .conn
        .query_row(
            "SELECT slug,body,artifact_kind FROM notes WHERE id='existing-note'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (slug.as_str(), body.as_str(), kind.as_str()),
        ("stable-slug", "Edited note", "document")
    );
    store.set_reminders(&[id], 1000).unwrap();
    let source_reminder: String = store
        .conn
        .query_row("SELECT session_id FROM session_reminders", [], |r| r.get(0))
        .unwrap();
    assert_eq!(source_reminder, source);
    assert_eq!(store.take_due_reminders(2000).unwrap().len(), 1);
    assert!(store.take_due_reminders(2000).unwrap().is_empty());
    let reader = store.search_reader().unwrap().unwrap();
    let (hits, _) = reader
        .search_sessions(
            "searchable",
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap();
    assert!(!hits.is_empty());
    assert!(reader.title(id, "Must not write").is_err());
    store.delete(id).unwrap();
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM sessions WHERE id=?1", [source], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(store.reminders().unwrap().is_empty());
}

#[test]
fn history_revision_is_fixed_size_and_covers_original_rich_metadata() {
    // One bounded guard, including its source length, never owns rich history.
    assert_eq!(std::mem::size_of::<HistoryRevision>(), 48);
    let blocks = vec![
        json!({"id":"u","role":"user","text":"same", "future":{"attachment":"x".repeat(1_000_000)}}),
    ];
    let revision = HistoryRevision::of(&blocks);
    assert!(revision.matches(&blocks));
    let mut changed = blocks.clone();
    changed[0]["future"]["attachment"] = json!("another source attachment");
    assert!(!revision.matches(&changed));
    assert!(!revision.matches(&[]));
}

#[test]
fn history_fast_forward_matches_the_reference_across_drafts_and_source_edits() {
    fn reference(expected: HistoryRevision, blocks: &[Value]) -> bool {
        let non_draft: Vec<Value> = blocks
            .iter()
            .filter(|block| block["draft"] != true)
            .cloned()
            .collect();
        (1..blocks.len()).any(|len| expected == HistoryRevision::of(&blocks[..len]))
            || (1..=non_draft.len()).any(|len| expected == HistoryRevision::of(&non_draft[..len]))
    }
    let source: Vec<Value> = (0..5)
        .map(|i| json!({"id":i,"text":format!("Saved {i}"),"future":{"keep":i}}))
        .collect();
    for mask in 0..32 {
        let mut candidate = source.clone();
        for (i, block) in candidate.iter_mut().enumerate() {
            block["draft"] = json!(mask & (1 << i) != 0);
        }
        let filtered: Vec<Value> = candidate
            .iter()
            .filter(|block| block["draft"] != true)
            .cloned()
            .collect();
        for base in [&candidate, &filtered] {
            for len in 0..=base.len() {
                let expected = HistoryRevision::of(&base[..len]);
                for changed_at in 0..=candidate.len() {
                    let mut changed = candidate.clone();
                    if let Some(block) = changed.get_mut(changed_at) {
                        block["future"]["keep"] = json!("External edit");
                    }
                    for current_len in 0..=changed.len() {
                        let current = &changed[..current_len];
                        assert_eq!(
                            expected.fast_forwards(current),
                            reference(expected, current)
                        );
                    }
                }
            }
        }
    }
    assert!(!HistoryRevision::Stored(1).fast_forwards(&source));
    assert!(!HistoryRevision::Invalidated.fast_forwards(&source));
}

#[test]
fn history_fast_forward_hashes_at_most_two_known_prefixes() {
    let source: Vec<Value> = (0..3_000)
        .map(|i| json!({"id":i,"text":"Saved","future":{"rich":"x".repeat(1_024)}}))
        .collect();
    let guard = HistoryRevision::of(&source);
    let mut edited = source.clone();
    edited[2_999]["future"]["rich"] = json!("External edit");
    let mut appended = source.clone();
    appended.push(json!({"id":"next","text":"New reply"}));
    let mut with_drafts = appended.clone();
    with_drafts.insert(1, json!({"id":"draft","text":"Unsent input","draft":true}));
    for (blocks, accepted) in [(&edited, false), (&appended, true), (&with_drafts, true)] {
        HISTORY_REVISION_HASH_BLOCKS.with(|count| count.set(0));
        assert_eq!(guard.fast_forwards(blocks), accepted);
        let hashed = HISTORY_REVISION_HASH_BLOCKS.with(|count| count.get());
        assert!(hashed <= 2 * source.len(), "Hashed {hashed} source blocks");
    }
}

#[test]
fn history_revision_ignores_object_key_order_and_source_whitespace() {
    let first: Vec<Value> =
        parse(r#"[{"id":"u","text":"same","future":{"a":1,"b":true}}]"#).unwrap();
    let second: Vec<Value> =
        parse(r#"[ { "future": {"b": true, "a": 1}, "text": "same", "id": "u" } ]"#).unwrap();
    assert_eq!(first, second);
    assert_eq!(HistoryRevision::of(&first), HistoryRevision::of(&second));
    let arrays = vec![json!(["a", "bc"])];
    assert_ne!(
        HistoryRevision::of(&arrays),
        HistoryRevision::of(&[json!(["ab", "c"])])
    );
}

#[test]
fn equivalent_desktop_json_can_be_saved_without_losing_rich_source_fields() {
    let mut store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let (id, original) = saved(
        &store,
        json!([{"id":"u","role":"user","text":"Original","future":{"x":42}}]),
    );
    let history = store.load(id).unwrap();
    store
        .conn
        .execute(
            "UPDATE sessions SET blocks_json=?2 WHERE id=?1",
            params![
                original,
                r#"[ { "future": {"x": 42}, "text": "Original", "role": "user", "id": "u" } ]"#
            ],
        )
        .unwrap();
    store.write_rows(id, 0, &history.rows).unwrap();
    assert_eq!(
        store.shared_blocks(id).unwrap()[0]["future"],
        json!({"x":42})
    );
    assert_eq!(store.shared.as_ref().unwrap().snapshots.borrow().len(), 1);
}

#[test]
fn history_revision_keeps_numeric_types_and_normalizes_float_zero() {
    let negative = vec![json!(-0.0)];
    let positive = vec![json!(0.0)];
    assert_eq!(negative, positive);
    assert_eq!(
        HistoryRevision::of(&negative),
        HistoryRevision::of(&positive)
    );
    assert_ne!(
        HistoryRevision::of(&positive),
        HistoryRevision::of(&[json!(0)])
    );
}

#[test]
fn existing_side_questions_import_their_exact_source_bound_without_writes() {
    let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let thread = json!({
        "id":"saved-side", "sourceEndBlockId":"answer", "createdAt":30, "updatedAt":40,
        "status":"error", "harness":"claude", "model":"claude:claude-opus-4-6",
        "modelSettings":{"reasoningEffort":"high","futureSetting":"preserved"},
        "providerThreadId":"original-provider-side-id", "error":"Saved failure", "futureField":42,
        "messages":[
            {"id":"side-question","role":"user","text":"Why that choice?","createdAt":31},
            {"id":"side-answer","role":"assistant","text":"Fallback answer","createdAt":32,
                "blocks":[{"id":"rich-side-answer","role":"assistant","text":"Rich side answer"}]}
        ]
    });
    let original = json!([
        {"id":"question","role":"user","text":"Main question","btwThreads":[thread.clone(),
            {"id":"missing-bound","sourceEndBlockId":"absent","messages":[]},
            {"id":"future-bound","sourceEndBlockId":"later-answer","messages":[]}]},
        {"id":"private","role":"reasoning","text":"Private main reasoning"},
        {"id":"hidden","role":"assistant","text":"Internal instruction","internal":true},
        {"id":"answer","role":"assistant","text":"Original main answer"},
        {"id":"later-question","role":"user","text":"Later question"},
        {"id":"later-answer","role":"assistant","text":"Later answer"}
    ]);
    let (session, source) = saved(&store, original.clone());
    let schema: i64 = store
        .conn
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .unwrap();
    let imported = store.desktop_side_threads(session).unwrap();
    assert_eq!(imported.len(), 1);
    let imported = &imported[0];
    assert_eq!(imported.id, "saved-side");
    assert_eq!(imported.provider, "claude");
    assert_eq!(imported.model.as_deref(), Some("claude-opus-4-6"));
    assert_eq!(imported.model_settings["futureSetting"], "preserved");
    assert_eq!(imported.status, "error");
    assert_eq!(imported.error.as_deref(), Some("Saved failure"));
    assert_eq!(imported.original, thread);
    assert_eq!(imported.rows[0].sent_at, Some(31));
    assert_eq!(
        imported
            .rows
            .iter()
            .map(|row| row.text.as_str())
            .collect::<String>(),
        "Why that choice?Rich side answer"
    );
    assert!(imported.snapshot.contains("Original main answer"));
    assert!(!imported.snapshot.contains("Later answer"));
    assert!(!imported.snapshot.contains("Private main reasoning"));
    assert!(!imported.snapshot.contains("Internal instruction"));
    let after: String = store
        .conn
        .query_row(
            "SELECT blocks_json FROM sessions WHERE id=?1",
            [&source],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(parse::<Value>(&after).unwrap(), original);
    assert_eq!(
        store
            .conn
            .query_row("PRAGMA schema_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        schema
    );
    assert!(store.setting(&format!("btw:{session}")).unwrap().is_none());
}

#[test]
fn desktop_turn_receipts_restore_exact_model_timing_metrics_and_projected_offsets() {
    let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let source = json!([
        {"id":"hidden","role":"system","text":"","future":true},
        {"id":"first","role":"user","text":"Saved prompt","sentAt":90,"startedAt":100,
         "durationMs":9500,"providerTurnId":"provider-turn",
         "turnModel":{"harness":"claude","id":"claude:old-model","name":"Saved Model","future":true},
         "turnMetrics":{"inputTokens":100,"outputTokens":20,"cacheReadTokens":40,"cacheHitPercent":40,"future":1},
         "noteCard":{"id":"note-one","title":"Saved context"}},
        {"id":"answer","role":"assistant","text":"Saved answer"},
        {"id":"second","role":"user","text":"Completion","internal":true,"startedAt":20000,"durationMs":0,
         "turnModel":{"harness":"codex","id":"codex:actual-model","name":"Actual Model"}},
        {"id":"reply","role":"assistant","text":"Done"},
        {"id":"legacy","role":"user","text":"Unknown old model"},
        {"id":"legacy-answer","role":"assistant","text":"Old answer"}
    ]);
    let (id, original) = saved(&store, source.clone());
    let raw: String = store
        .conn
        .query_row(
            "SELECT blocks_json FROM sessions WHERE id=?1",
            [&original],
            |row| row.get(0),
        )
        .unwrap();
    let history = store.load(id).unwrap();
    let settings = store.agent_settings(id).unwrap().unwrap();
    let first = history
        .rows
        .iter()
        .rposition(|row| row.role == "user" && row.text == "Saved prompt")
        .unwrap()
        + 1;
    let second = history
        .rows
        .iter()
        .position(|row| row.role == "bot-system")
        .unwrap()
        + 1;
    assert!(
        second > 3,
        "hidden note metadata contributes to row offsets"
    );
    let receipt = &settings.turns[&first];
    assert_eq!(receipt.provider, "claude");
    assert_eq!(receipt.model, "Saved Model");
    assert_eq!(receipt.model_id.as_deref(), Some("claude:old-model"));
    assert_eq!(receipt.provider_turn_id.as_deref(), Some("provider-turn"));
    assert_eq!(receipt.started_at, Some(100));
    assert_eq!(receipt.duration_ms, 9500);
    assert_eq!(receipt.completed_at, Some(9600));
    assert_eq!(receipt.metrics.unwrap().cache_hit_percent, Some(40.));
    assert_eq!(receipt.metrics.unwrap().cache_write_tokens, 0);
    assert_eq!(
        history.rows[first - 1].sent_at,
        Some(90),
        "send and start times remain distinct"
    );
    assert!(
        settings.turns[&second].has_duration(),
        "measured zero is present"
    );
    assert_eq!(settings.turns[&second].completed_at, Some(20000));
    assert_eq!(
        settings.turns.len(),
        2,
        "legacy model/time are not fabricated from the current picker"
    );
    assert_eq!(
        store.shared_blocks(id).unwrap(),
        source.as_array().unwrap().clone()
    );
    assert_eq!(
        store
            .conn
            .query_row(
                "SELECT blocks_json FROM sessions WHERE id=?1",
                [&original],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        raw,
        "reading metadata never rewrites source JSON"
    );
}

#[test]
fn fresh_native_turn_receipts_persist_source_fields_without_changing_saved_provenance() {
    let mut store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let mut transcript = Transcript::default();
    let prompt = transcript.begin("user");
    transcript.rows[prompt].sent_at = Some(100);
    transcript.append("New request");
    let turn = transcript.begin("assistant");
    transcript.append("Answer");
    store.write_rows(session.id, 0, &transcript.rows).unwrap();
    let mut settings = store.agent_settings(session.id).unwrap().unwrap();
    settings.turns.insert(
        turn,
        crate::agent::TurnInfo {
            provider: "codex".into(),
            model: "Chosen Model".into(),
            started_at: Some(100),
            ..Default::default()
        },
    );
    store.save_agent_settings(session.id, &settings).unwrap();
    let initial = store.shared_blocks(session.id).unwrap();
    assert_eq!(initial[0]["startedAt"], 100);
    assert!(
        initial[0]["durationMs"].is_null(),
        "a live turn has no invented duration"
    );
    let receipt = settings.turns.get_mut(&turn).unwrap();
    receipt.model_id = Some("actual-model".into());
    receipt.provider_turn_id = Some("provider-turn".into());
    receipt.duration_ms = 5000;
    receipt.duration_known = true;
    receipt.completed_at = Some(5100);
    receipt.metrics = Some(crate::agent::TurnMetrics {
        input_tokens: 25,
        output_tokens: 10,
        ..Default::default()
    });
    store.save_agent_settings(session.id, &settings).unwrap();
    let source = store.shared_blocks(session.id).unwrap();
    assert_eq!(
        source[0]["turnModel"],
        json!({"harness":"codex","id":"codex:actual-model","name":"Chosen Model"})
    );
    assert_eq!(source[0]["providerTurnId"], "provider-turn");
    assert_eq!(source[0]["durationMs"], 5000);
    assert_eq!(source[0]["turnMetrics"]["outputTokens"], 10);
    let restored = store.agent_settings(session.id).unwrap().unwrap();
    assert_eq!(restored.turns[&turn].completed_at, Some(5100));
    assert_eq!(store.load(session.id).unwrap().rows, transcript.rows);
    settings.model = Some("new-picker-value".into());
    settings.turns.get_mut(&turn).unwrap().model = "Wrong later picker".into();
    store.save_agent_settings(session.id, &settings).unwrap();
    assert_eq!(
        store.shared_blocks(session.id).unwrap(),
        source,
        "existing source turn provenance is immutable"
    );
    assert_eq!(
        store.agent_settings(session.id).unwrap().unwrap().turns[&turn].model,
        "Chosen Model"
    );
}

#[test]
fn indexed_bot_turn_receipts_restore_and_save_in_the_original_block_table() {
    let mut store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let (id, original) = saved(&store, json!([]));
    store
        .conn
        .execute_batch(include_str!("../../../tests/fixtures/bot-schema.sql"))
        .unwrap();
    store
        .conn
        .execute("INSERT INTO bot_transcripts VALUES(?1)", [&original])
        .unwrap();
    let source = vec![
        json!({"id":"bot-prompt","role":"user","text":"Saved Bot","startedAt":100,"durationMs":7000,
          "turnModel":{"harness":"codex","id":"codex:model","name":"Saved Model"},"future":{"keep":true}}),
        json!({"id":"bot-answer","role":"assistant","text":"Done"}),
    ];
    store.shared_store_blocks(id, &source).unwrap();
    let mut transcript = store.load(id).unwrap();
    assert_eq!(
        store.agent_settings(id).unwrap().unwrap().turns[&1].model,
        "Saved Model"
    );
    let start = transcript.begin("bot-system");
    transcript.rows[start].sent_at = Some(9000);
    transcript.append("Follow up");
    let turn = transcript.begin("assistant");
    transcript.append("New report");
    store
        .write_rows(id, start, &transcript.rows[start..])
        .unwrap();
    let mut settings = store.agent_settings(id).unwrap().unwrap();
    settings.turns.insert(
        turn,
        crate::agent::TurnInfo {
            provider: "claude".into(),
            model: "New Model".into(),
            model_id: Some("new-model".into()),
            started_at: Some(9000),
            duration_ms: 2000,
            duration_known: true,
            ..Default::default()
        },
    );
    store.save_agent_settings(id, &settings).unwrap();
    assert_eq!(store.shared_blocks(id).unwrap()[0], source[0]);
    assert_eq!(store.shared_blocks(id).unwrap()[2]["durationMs"], 2000);
    assert_eq!(
        store.agent_settings(id).unwrap().unwrap().turns[&turn].completed_at,
        Some(11000)
    );
    assert_eq!(store.load(id).unwrap().rows, transcript.rows);
    assert_eq!(
        store
            .conn
            .query_row(
                "SELECT blocks_json FROM sessions WHERE id=?1",
                [&original],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "[]"
    );
}

#[test]
fn fresh_receipt_preserves_the_matching_desktop_model_id_spelling() {
    let mut store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    let (id, original) = saved(&store, json!([]));
    store
        .conn
        .execute(
            "UPDATE sessions SET harness='claude',model='claude:sonnet-4.6' WHERE id=?1",
            [&original],
        )
        .unwrap();
    let mut transcript = Transcript::default();
    transcript.begin("user");
    transcript.append("New request");
    let turn = transcript.begin("assistant");
    store.write_rows(id, 0, &transcript.rows).unwrap();
    let mut settings = store.agent_settings(id).unwrap().unwrap();
    settings.turns.insert(
        turn,
        crate::agent::TurnInfo {
            provider: "claude".into(),
            model: "Sonnet 4.6".into(),
            model_id: Some("claude-sonnet-4-6".into()),
            started_at: transcript.rows[0].sent_at,
            ..Default::default()
        },
    );
    store.save_agent_settings(id, &settings).unwrap();
    assert_eq!(
        store.shared_blocks(id).unwrap()[0]["turnModel"]["id"],
        "claude:sonnet-4.6"
    );
}

#[test]
fn first_app_command_preserves_an_active_scalar_history_guard_and_append() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.db");
    let mut store = Store::open_vorkspace(&path).unwrap();
    assert!(!has_table(&store.conn, "app_command_receipts").unwrap());
    let session = store.create("codex", "/project").unwrap();
    let id = session.id;
    let original = store.persisted_session_id(id).unwrap();
    let source = json!([{"id":"saved-user","role":"user","text":"Keep this prompt","future":{"opaque":"retained"}}]);
    store
        .conn
        .execute(
            "UPDATE sessions SET blocks_json=?2 WHERE id=?1",
            params![original, source.to_string()],
        )
        .unwrap();
    let mut history = store.load(id).unwrap();
    let from = history.begin("assistant");
    history.append("Saved partial answer");
    store.write_rows(id, from, &history.rows[from..]).unwrap();
    assert!(matches!(
        store.shared.as_ref().unwrap().snapshots.borrow()[&id],
        HistoryRevision::Stored(_)
    ));
    let schema: i64 = store
        .conn
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        store
            .admit_app_command(
                id,
                "first-effect",
                "notes.write",
                &json!({"title":"A note","body":"Saved"})
            )
            .unwrap(),
        crate::store::AppCommandAdmission::Admitted
    );
    store
        .finish_app_command(
            id,
            "first-effect",
            "notes.write",
            &json!({"title":"A note","body":"Saved"}),
            &Ok(json!({"accepted":true})),
        )
        .unwrap();
    // A rejected child draft may record recovery input while its caller is
    // still streaming. Those local metadata writes must preserve its guard.
    let draft = json!({"cwd":"/project","text":"Keep newer input","selectionStart":0,"selectionEnd":0,"attachments":[],"mcpTags":[]});
    store.save_composer_draft(id, 0, &draft).unwrap();
    store
        .save_composer_recovery(
            id,
            crate::store::ComposerRecovery {
                owner: "failed-child-draft".into(),
                revision: 0,
                payload: draft,
            },
        )
        .unwrap();
    assert_eq!(
        store
            .conn
            .query_row("PRAGMA schema_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        schema
    );
    let from = history.begin("assistant");
    history.append("Appended after the command");
    store.write_rows(id, from, &history.rows[from..]).unwrap();
    drop(store);
    let reopened = Store::open_vorkspace(&path).unwrap();
    let id = reopened.resolve_session_id(&original).unwrap();
    assert_eq!(reopened.load(id).unwrap().rows, history.rows);
    assert_eq!(
        reopened.shared_blocks(id).unwrap()[0]["future"],
        json!({"opaque":"retained"})
    );
}

#[test]
fn app_command_schema_coordination_does_not_bless_external_schema_drift() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.db");
    let mut store = Store::open_vorkspace(&path).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let id = session.id;
    let mut history = crate::transcript::Transcript::default();
    let from = history.begin("user");
    history.append("Durable original");
    store.write_rows(id, from, &history.rows[from..]).unwrap();
    let original = history.rows.clone();
    assert!(matches!(
        store.shared.as_ref().unwrap().snapshots.borrow()[&id],
        HistoryRevision::Stored(_)
    ));
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE external_schema_change(value TEXT)")
        .unwrap();
    store
        .admit_app_command(id, "after-external-ddl", "notes.write", &json!({}))
        .unwrap();
    store
        .finish_app_command(
            id,
            "after-external-ddl",
            "notes.write",
            &json!({}),
            &Ok(json!({})),
        )
        .unwrap();
    let from = history.begin("assistant");
    history.append("A stale append must be rejected");
    assert!(store
        .write_rows(id, from, &history.rows[from..])
        .unwrap_err()
        .contains("changed in another window"));
    drop(store);
    assert_eq!(
        Store::open_vorkspace(&path).unwrap().load(id).unwrap().rows,
        original
    );
}

#[test]
fn deleting_a_session_removes_only_its_own_sidecar_metadata() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("db");
    let mut store = Store::open_vorkspace(&path).unwrap();
    let child = store.create("codex", "/project").unwrap().id;
    let other = store.create("codex", "/project").unwrap().id;
    let prefixes = [
        "agent",
        "context",
        "follow-up",
        "started-by",
        "orchestration",
        "inbox-ask",
        "automation",
        "composer",
        "edit-recovery",
    ];
    for prefix in prefixes {
        store
            .save_setting(&format!("{prefix}:{child}"), "remove child metadata")
            .unwrap();
        store
            .save_setting(&format!("{prefix}:{other}"), "keep other metadata")
            .unwrap();
        store
            .save_setting(&format!("{prefix}:{child}0"), "keep boundary lookalike")
            .unwrap();
    }
    store
        .save_setting(&format!("bot-delivered:{child}:message"), "1")
        .unwrap();
    store
        .save_setting(&format!("bot-delivered:{child}0:message"), "keep")
        .unwrap();
    store
        .save_setting("unrelated-setting", "keep unknown")
        .unwrap();
    store.delete(child).unwrap();
    assert!(store.bot_session(child).unwrap().is_none());
    assert!(store.bot_session(other).unwrap().is_some());
    drop(store);
    let store = Store::open_vorkspace(&path).unwrap();
    for prefix in prefixes {
        assert!(store
            .setting(&format!("{prefix}:{child}"))
            .unwrap()
            .is_none());
        assert_eq!(
            store
                .setting(&format!("{prefix}:{other}"))
                .unwrap()
                .as_deref(),
            Some("keep other metadata")
        );
        assert_eq!(
            store
                .setting(&format!("{prefix}:{child}0"))
                .unwrap()
                .as_deref(),
            Some("keep boundary lookalike")
        );
    }
    let extras = store.shared.as_ref().unwrap().extras.borrow();
    assert!(!extras.contains_key(&format!("bot-delivered:{child}:message")));
    assert_eq!(extras[&format!("bot-delivered:{child}0:message")], "keep");
    assert_eq!(extras["unrelated-setting"], "keep unknown");
}

#[test]
fn failed_sidecar_removal_reports_committed_deletion_and_can_finish_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("db");
    let mut store = Store::open_vorkspace(&path).unwrap();
    let child = store.create("codex", "/project").unwrap().id;
    let key = format!("agent:{child}");
    store
        .save_setting(&key, "preserve until cleanup succeeds")
        .unwrap();
    let sidecar = store.shared.as_ref().unwrap().extras_path.clone().unwrap();
    let backup = root.path().join("saved-extras.json");
    std::fs::rename(&sidecar, &backup).unwrap();
    std::fs::create_dir(&sidecar).unwrap();
    let error = store.delete(child).unwrap_err();
    assert!(
        error.contains("Conversation was deleted, but its settings"),
        "{error}"
    );
    assert!(store.bot_session(child).unwrap().is_none());
    assert!(store.setting(&key).unwrap().is_some());
    std::fs::remove_dir(&sidecar).unwrap();
    std::fs::rename(&backup, &sidecar).unwrap();
    store.delete(child).unwrap();
    assert!(store.setting(&key).unwrap().is_none());
    drop(store);
    assert!(Store::open_vorkspace(&path)
        .unwrap()
        .setting(&key)
        .unwrap()
        .is_none());
}
