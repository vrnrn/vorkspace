use super::*;

fn source(path: &Path) -> Connection {
    let source = Connection::open(path).unwrap();
    source.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
            CREATE TABLE sessions(id TEXT PRIMARY KEY,cwd TEXT NOT NULL,harness TEXT NOT NULL,
                model TEXT NOT NULL,model_settings TEXT NOT NULL,runtime_mode TEXT NOT NULL,
                title TEXT NOT NULL,provider_session_id TEXT,blocks_json TEXT NOT NULL,updated_at INTEGER NOT NULL);")
            .unwrap();
    source
}

fn add(source: &Connection, id: &str, cwd: &Path, blocks: Value, updated: i64) {
    source.execute(r#"INSERT INTO sessions(id,cwd,harness,model,model_settings,runtime_mode,title,provider_session_id,blocks_json,updated_at)
            VALUES(?1,?2,'codex','codex:test-model','{"reasoningEffort":"high","fast":"true","serviceTier":"fast"}','auto-accept-edits','Saved conversation','original-thread',?3,?4)"#,
            params![id,cwd.to_string_lossy(),blocks.to_string(),updated]).unwrap();
}

#[test]
fn imports_normalized_ordinary_source_and_keeps_the_source_read_only() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.db");
    let mut source = Store::open_vorkspace(&path).unwrap();
    let id = source
        .create("codex", directory.path().to_str().unwrap())
        .unwrap()
        .id;
    let mut history = Transcript::default();
    history.begin("user");
    history.append("Saved normalized prompt");
    history.begin("assistant");
    history.append("Saved answer 🙂");
    source.write_rows(id, 0, &history.rows).unwrap();
    let blocks = source.shared_blocks(id).unwrap();
    let expected = transcript(&blocks).unwrap();
    let original = std::fs::read(&path).unwrap();
    let schema: i64 = source
        .conn
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .unwrap();
    let mut target = Store::open(&directory.path().join("native.db")).unwrap();
    let report = target.import_vorkspace(&path).unwrap();
    assert_eq!((report.imported, report.skipped), (1, 0));
    let imported = target.sessions().unwrap().pop().unwrap();
    assert_eq!(target.load(imported.id).unwrap().rows, expected.rows);
    assert_eq!(source.shared_blocks(id).unwrap(), blocks);
    assert_eq!(
        source
            .conn
            .query_row("PRAGMA schema_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        schema
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn copies_committed_wal_history_and_settings_without_changing_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let source = source(&path);
    source
        .execute_batch(
            "ALTER TABLE sessions ADD COLUMN archived INTEGER DEFAULT 1;
            ALTER TABLE sessions ADD COLUMN pinned INTEGER DEFAULT 1;
            ALTER TABLE sessions ADD COLUMN worktree_cwd TEXT;
            ALTER TABLE sessions ADD COLUMN worktree_removed INTEGER DEFAULT 0;
            ALTER TABLE sessions ADD COLUMN context_used INTEGER DEFAULT 42;
            ALTER TABLE sessions ADD COLUMN context_window INTEGER DEFAULT 1000;",
        )
        .unwrap();
    let text = "🙂漢字 café\n\n".repeat(3000);
    add(
        &source,
        "original",
        dir.path(),
        json!([
            {"role":"user","text":"Saved prompt","startedAt":100},
            {"role":"reasoning","text":"Visible summary"},
            {"role":"tool","id":"tool-1","text":"Run tests","tool":{
                "kind":"execute","title":"cargo test","status":"error","detail":"🙂".repeat(20000)}},
            {"role":"assistant","text":text},
            {"role":"user","text":"Unsent draft","draft":true},
            {"role":"user","text":"Internal prompt","internal":true}
        ]),
        200,
    );
    source
        .execute(
            "UPDATE sessions SET worktree_cwd=?1",
            [dir.path().join("worktree").to_string_lossy()],
        )
        .unwrap();
    let original = std::fs::read(&path).unwrap();
    let mut store = Store::open(&dir.path().join("native.sqlite3")).unwrap();
    let existing = store
        .create("claude", &dir.path().to_string_lossy())
        .unwrap();
    store
        .title(existing.id, "Existing native conversation")
        .unwrap();
    let report = store.import_vorkspace(&path).unwrap();
    assert_eq!(
        (report.imported, report.existing, report.skipped),
        (1, 0, 0)
    );
    assert_eq!(report.latest_project.as_deref(), dir.path().to_str());
    let sessions = store.sessions().unwrap();
    let imported = sessions.iter().find(|s| s.id != existing.id).unwrap();
    assert_eq!(imported.provider_id.as_deref(), Some("original-thread"));
    assert_eq!(imported.updated_at, 200);
    assert!(imported.archived && imported.pinned && imported.has_messages);
    let settings = store.agent_settings(imported.id).unwrap().unwrap();
    assert_eq!(settings.model.as_deref(), Some("test-model"));
    assert_eq!(settings.effort.as_deref(), Some("high"));
    assert!(settings.fast);
    assert_eq!(settings.service_tier.as_deref(), Some("fast"));
    assert_eq!(settings.access, crate::agent::AccessMode::AutoAcceptEdits);
    assert_eq!(
        settings.worktree_cwd.as_deref(),
        dir.path().join("worktree").to_str()
    );
    assert_eq!(store.context_usage(imported.id).unwrap().unwrap().used, 42);
    let history = store.load(imported.id).unwrap();
    assert_eq!(history.rows[0].text, "Saved prompt");
    assert_eq!(history.rows[0].sent_at, Some(100));
    assert_eq!(history.rows[1].sent_at, None);
    let tool: ToolCall = serde_json::from_str(&history.rows[2].text).unwrap();
    assert_eq!(tool.state, ToolState::Failed);
    assert!(tool.output.starts_with("… earlier output omitted …\n"));
    let answer: Vec<_> = history
        .rows
        .iter()
        .filter(|r| r.role == "assistant")
        .collect();
    assert_eq!(
        answer.iter().map(|r| r.text.as_str()).collect::<String>(),
        text
    );
    assert!(answer
        .iter()
        .all(|r| r.text.len() <= 2048 && r.text.matches('\n').count() <= 12));
    assert!(!history
        .rows
        .iter()
        .any(|r| r.text.contains("Unsent draft") || r.text.contains("Internal prompt")));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(
        source
            .query_row("SELECT count(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    // Reruns must preserve native edits and discover only newly saved chats.
    store.title(imported.id, "Edited in GPUI").unwrap();
    source
        .execute("UPDATE sessions SET title='Changed at source'", [])
        .unwrap();
    add(
        &source,
        "new",
        dir.path(),
        json!([{"role":"user","text":"New conversation"}]),
        300,
    );
    let report = store.import_vorkspace(&path).unwrap();
    assert_eq!((report.imported, report.existing), (1, 1));
    let sessions = store.sessions().unwrap();
    assert_eq!(
        sessions.iter().find(|s| s.id == imported.id).unwrap().title,
        "Edited in GPUI"
    );
    assert_eq!(
        sessions.iter().find(|s| s.id == existing.id).unwrap().title,
        "Existing native conversation"
    );
}

#[test]
fn older_schema_works_and_hidden_remote_and_empty_histories_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let source = source(&path);
    let prompt = json!([{"role":"user","text":"Hello"}]);
    add(&source, "local", dir.path(), prompt.clone(), 100);
    add(
        &source,
        "remote",
        Path::new("ssh://host/project"),
        prompt.clone(),
        110,
    );
    add(
        &source,
        "draft",
        dir.path(),
        json!([{"role":"user","text":"Draft","draft":true}]),
        120,
    );
    add(&source, "unknown", dir.path(), prompt.clone(), 130);
    source
        .execute(
            "UPDATE sessions SET harness='future-provider' WHERE id='unknown'",
            [],
        )
        .unwrap();
    let mut store = Store::open(&dir.path().join("native.sqlite3")).unwrap();
    let report = store.import_vorkspace(&path).unwrap();
    assert_eq!((report.imported, report.skipped), (2, 2));
    assert!(!store.sessions().unwrap()[0].archived);
    source
        .execute_batch(
            "ALTER TABLE sessions ADD COLUMN sidebar_hidden INTEGER DEFAULT 0;
            ALTER TABLE sessions ADD COLUMN inbox_ask TEXT;",
        )
        .unwrap();
    add(&source, "hidden", dir.path(), prompt.clone(), 140);
    source
        .execute("UPDATE sessions SET sidebar_hidden=1 WHERE id='hidden'", [])
        .unwrap();
    add(&source, "inbox", dir.path(), prompt, 150);
    source
        .execute("UPDATE sessions SET inbox_ask='{}' WHERE id='inbox'", [])
        .unwrap();
    let report = store.import_vorkspace(&path).unwrap();
    assert_eq!(
        (report.imported, report.existing, report.skipped),
        (0, 2, 4)
    );
}

#[test]
fn unsupported_provider_history_keeps_its_identity_model_and_transcript_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let source = source(&path);
    let providers = [
        "cursor", "grok", "opencode", "fx", "hermes", "devin", "unknown",
    ];
    for (index, provider) in providers.iter().enumerate() {
        add(
            &source,
            provider,
            dir.path(),
            json!([
                {"role":"user","text":"Saved request"},
                {"role":"assistant","text":"Saved answer"}
            ]),
            index as i64,
        );
        source
            .execute(
                "UPDATE sessions SET harness=?1,model=?2 WHERE id=?1",
                params![provider, format!("{provider}:original-model")],
            )
            .unwrap();
    }
    let native = dir.path().join("native.sqlite3");
    let mut store = Store::open(&native).unwrap();
    assert_eq!(
        store.import_vorkspace(&path).unwrap().imported,
        providers.len()
    );
    let before = store.sessions().unwrap();
    for summary in &before {
        assert!(providers.contains(&summary.provider.as_str()));
        assert_eq!(summary.provider_id.as_deref(), Some("original-thread"));
        assert_eq!(
            store
                .agent_settings(summary.id)
                .unwrap()
                .unwrap()
                .model
                .as_deref(),
            Some("original-model")
        );
        let transcript = store.load(summary.id).unwrap();
        assert!(transcript.rows.iter().any(|row| row.text == "Saved answer"));
        store
            .title(summary.id, "Edited while viewing history")
            .unwrap();
    }
    drop(store);

    let mut reopened = Store::open(&native).unwrap();
    let report = reopened.import_vorkspace(&path).unwrap();
    assert_eq!((report.imported, report.existing), (0, providers.len()));
    for summary in reopened.sessions().unwrap() {
        let original = before.iter().find(|s| s.id == summary.id).unwrap();
        assert_eq!(summary.provider, original.provider);
        assert_eq!(summary.provider_id, original.provider_id);
        assert_eq!(summary.title, "Edited while viewing history");
        assert!(reopened
            .load(summary.id)
            .unwrap()
            .rows
            .iter()
            .any(|r| r.text == "Saved request"));
        assert!(reopened
            .create(&summary.provider, dir.path().to_str().unwrap())
            .is_err());
    }
}

#[test]
fn invalid_history_rolls_back_the_entire_import() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let source = source(&path);
    add(
        &source,
        "valid",
        dir.path(),
        json!([{"role":"user","text":"Hello"}]),
        200,
    );
    add(&source, "invalid", dir.path(), json!([]), 100);
    source
        .execute(
            "UPDATE sessions SET blocks_json='broken json' WHERE id='invalid'",
            [],
        )
        .unwrap();
    let mut store = Store::open(&dir.path().join("native.sqlite3")).unwrap();
    assert!(store.import_vorkspace(&path).is_err());
    assert!(store.sessions().unwrap().is_empty());
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM native_settings", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    source
        .execute("DELETE FROM sessions WHERE id='invalid'", [])
        .unwrap();
    assert_eq!(store.import_vorkspace(&path).unwrap().imported, 1);
}

#[test]
fn missing_or_wrong_source_is_rejected_without_creating_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.db");
    let mut store = Store::open(&dir.path().join("native.sqlite3")).unwrap();
    assert!(store.import_vorkspace(&path).is_err());
    assert!(!path.exists());
    let error = store
        .import_vorkspace(&dir.path().join("native.sqlite3"))
        .unwrap_err();
    assert!(error.contains("Not a vorkspace session database"));
}

#[test]
fn model_picker_keys_are_translated_to_provider_model_ids() {
    assert_eq!(
        crate::session::native_model("codex:gpt-6.1-sol", "codex").as_deref(),
        Some("gpt-6.1-sol")
    );
    assert_eq!(
        crate::session::native_model("claude:opus-4.8[effort=high]", "claude").as_deref(),
        Some("claude-opus-4-8")
    );
    assert_eq!(
        crate::session::native_model("claude:sonnet", "claude").as_deref(),
        Some("sonnet")
    );
    assert_eq!(
        crate::session::native_model("opencode:provider/model", "opencode").as_deref(),
        Some("provider/model")
    );
    assert_eq!(crate::session::native_model("codex:default", "codex"), None);
}
