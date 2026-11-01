
#[test]
fn project_deletion_admission_releases_after_completion() {
    let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
    store.begin_project_deletion("/project/").unwrap();
    assert!(store.create("codex", "/project").is_err());
    assert!(store.create("codex", "/project/sub").is_err());
    assert!(store.create("codex", "/project-other").is_ok());
    store.end_project_deletion("/project");
    assert!(store.create("codex", "/project").is_ok());
}

#[test]
fn related_settings_publish_atomically_on_native_and_shared_failures() {
    use super::Store;
    let dir = tempfile::tempdir().unwrap();
    let native = dir.path().join("native.db");
    let db = Store::open(&native).unwrap();
    db.save_settings(&[("history", "old"), ("draft", "keep")])
        .unwrap();
    db.conn.execute_batch("CREATE TRIGGER reject_draft BEFORE UPDATE ON native_settings WHEN NEW.key='draft' BEGIN SELECT RAISE(ABORT,'injected write failure'); END;").unwrap();
    assert!(db
        .save_settings(&[("history", "new"), ("draft", "")])
        .is_err());
    drop(db);
    let db = Store::open(&native).unwrap();
    assert_eq!(db.setting("history").unwrap().as_deref(), Some("old"));
    assert_eq!(db.setting("draft").unwrap().as_deref(), Some("keep"));

    let path = dir.path().join("shared.db");
    let db = Store::open_vorkspace(&path).unwrap();
    db.save_settings(&[("history", "old"), ("draft", "keep")])
        .unwrap();
    let sidecar = dir.path().join("shared.db.gpui.json");
    let backup = dir.path().join("saved.json");
    std::fs::rename(&sidecar, &backup).unwrap();
    std::fs::create_dir(&sidecar).unwrap();
    assert!(db
        .save_settings(&[("history", "new"), ("draft", "")])
        .is_err());
    assert!(db.setting("history").is_err());
    assert!(db.setting("draft").is_err());
    drop(db);
    std::fs::remove_dir(&sidecar).unwrap();
    std::fs::rename(&backup, &sidecar).unwrap();
    let db = Store::open_vorkspace(&path).unwrap();
    assert_eq!(db.setting("history").unwrap().as_deref(), Some("old"));
    assert_eq!(db.setting("draft").unwrap().as_deref(), Some("keep"));
}
#[test]
fn removed_worktree_keeps_history_and_requires_reattachment() {
    let mut store = super::Store::open(std::path::Path::new(":memory:")).unwrap();
    let session = store.create("codex", "/project-worktrees/feature").unwrap();
    let settings = crate::agent::Settings {
        worktree_cwd: Some("/project-worktrees/feature".into()),
        ..Default::default()
    };
    store.save_agent_settings(session.id, &settings).unwrap();
    store
        .provider_id(session.id, "old-provider-thread")
        .unwrap();
    store
        .write_rows(
            session.id,
            0,
            &[crate::transcript::Row {
                role: "user".into(),
                text: "Keep my conversation".into(),
                first: true,
                sent_at: None,
            }],
        )
        .unwrap();
    let detached = store
        .detach_worktree("/project-worktrees/feature", "/project")
        .unwrap();
    assert_eq!(detached.len(), 1);
    assert_eq!(detached[0].0.cwd, "/project");
    assert!(detached[0].0.provider_id.is_none());
    assert!(detached[0].1.worktree_removed);
    assert_eq!(
        store.load(session.id).unwrap().rows[0].text,
        "Keep my conversation"
    );
    assert!(
        store
            .agent_settings(session.id)
            .unwrap()
            .unwrap()
            .worktree_removed
    );
}

#[test]
fn workspace_membership_reads_main_and_worktree_sessions_without_loading_history() {
    let store = super::Store::open(std::path::Path::new(":memory:")).unwrap();
    let main = store.create("codex", "/project").unwrap();
    let feature = store.create("claude", "/project").unwrap();
    let corrupt = store.create("codex", "/project").unwrap();
    store
        .save_agent_settings(
            feature.id,
            &crate::agent::Settings {
                worktree_cwd: Some("/project-feature".into()),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .save_setting(&format!("agent:{}", corrupt.id), "invalid json")
        .unwrap();
    let scopes = store.session_worktrees().unwrap();
    assert_eq!(scopes.len(), 3);
    assert_eq!(scopes[&main.id], None);
    assert_eq!(scopes[&feature.id].as_deref(), Some("/project-feature"));
    assert_eq!(scopes[&corrupt.id], None);
}

use super::*;
#[test]
fn archive_and_activity_time_survive_restart_without_changing_history() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("native.db");
    let mut store = Store::open(&path).unwrap();
    let session = store.create("codex", "/project").unwrap();
    assert!(session.updated_at > 0);
    store
        .conn
        .execute(
            "UPDATE native_sessions SET updated_at=1 WHERE id=?1",
            [session.id],
        )
        .unwrap();
    let rows = vec![Row {
        role: "user".into(),
        text: "Keep this conversation".into(),
        first: true,
        sent_at: None,
    }];
    store.write_rows(session.id, 0, &rows).unwrap();
    store.provider_id(session.id, "resume-this-thread").unwrap();
    let updated_at = store.sessions().unwrap()[0].updated_at;
    assert!(updated_at > 1);
    store.set_archived(session.id, true).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    let saved = &store.sessions().unwrap()[0];
    assert!(saved.archived);
    assert_eq!(
        saved.updated_at, updated_at,
        "archiving does not make an old session look new"
    );
    assert_eq!(saved.provider_id.as_deref(), Some("resume-this-thread"));
    assert_eq!(store.load(session.id).unwrap().rows, rows);
    store.set_archived(session.id, false).unwrap();
    assert!(!store.sessions().unwrap()[0].archived);
}
#[test]
fn started_links_and_orchestration_summaries_survive_restart_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.sqlite3");
    let store = Store::open(&path).unwrap();
    let lead = store.create("codex", "/p").unwrap().id;
    let child = store.create("codex", "/p").unwrap().id;
    store.save_started_link(child, lead).unwrap();
    let summary = crate::orchestration::Summary {
        tasks: vec![crate::orchestration::SummaryTask {
            session: Some(child),
            title: "Tests".into(),
            provider: "codex".into(),
            model: None,
            status: "running".into(),
        }],
        live: true,
    };
    store.save_orchestration_summary(lead, &summary).unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.started_links().unwrap()[&child], lead);
    assert_eq!(store.orchestration_summaries().unwrap()[&lead], summary);
    assert_eq!(summary.tasks[0].label(&summary, false), "Working");
    store.delete(child).unwrap();
    store.delete(lead).unwrap();
    assert!(store.started_links().unwrap().is_empty());
    assert!(store.orchestration_summaries().unwrap().is_empty());
}
#[test]
fn pins_links_groups_and_reminders_survive_restart_and_delete_clears_them() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("native.db");
    let mut store = Store::open(&path).unwrap();
    let keep = store.create("codex", "/project").unwrap();
    let gone = store.create("claude", "/project").unwrap();
    store.set_pinned(&[keep.id], true).unwrap();
    let item = crate::session_groups::parse_github_work_item("https://github.com/acme/app/pull/9")
        .unwrap();
    store.set_work_item(keep.id, Some(&item)).unwrap();
    let mut groups = crate::session_groups::Groups::default();
    groups.create_folder(&[keep.id, gone.id], "f".into());
    groups.pinned_collapsed = true;
    store.save_groups("/project", &groups).unwrap();
    store.set_reminders(&[keep.id, gone.id], 100).unwrap();
    store.set_reminders(&[keep.id], 200).unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    let saved = store
        .sessions()
        .unwrap()
        .into_iter()
        .find(|s| s.id == keep.id)
        .unwrap();
    assert!(saved.pinned);
    assert_eq!(saved.work_item, Some(item));
    assert_eq!(store.groups("/project").unwrap(), groups);
    assert_eq!(store.groups("/other").unwrap(), Default::default());
    let due = store.take_due_reminders(150).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].session_id, gone.id);
    assert!(
        store.take_due_reminders(150).unwrap().is_empty(),
        "delivered once"
    );
    store.clear_reminders(&[keep.id], Some(100)).unwrap();
    assert_eq!(
        store.reminders().unwrap().len(),
        2,
        "a stale time clears nothing"
    );
    store.delete(gone.id).unwrap();
    assert_eq!(store.reminders().unwrap().len(), 1);
    assert!(store.sessions().unwrap().iter().all(|s| s.id != gone.id));
    assert!(store.load(gone.id).unwrap().rows.is_empty());
    store.clear_reminders(&[keep.id], None).unwrap();
    assert!(store.reminders().unwrap().is_empty());
    store.set_work_item(keep.id, None).unwrap();
    store.set_pinned(&[keep.id], false).unwrap();
    let saved = &store.sessions().unwrap()[0];
    assert!(!saved.pinned && saved.work_item.is_none());
}
#[test]
fn old_native_schema_migrates_idempotently_and_recovers_recorded_times() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("legacy-native.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE native_sessions(id INTEGER PRIMARY KEY,title TEXT NOT NULL,provider TEXT NOT NULL,cwd TEXT NOT NULL,provider_id TEXT);
            CREATE TABLE native_settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE native_rows(session_id INTEGER NOT NULL,row_ix INTEGER NOT NULL,role TEXT NOT NULL,body TEXT NOT NULL,first INTEGER NOT NULL,PRIMARY KEY(session_id,row_ix));
            INSERT INTO native_sessions VALUES(1,'Has a receipt','codex','/project','thread'),(2,'Unknown age','claude','/project',NULL);
            INSERT INTO native_rows VALUES(1,0,'user','Old prompt',1),(1,1,'assistant','Old answer',1),(2,0,'user','Undated prompt',1);
            INSERT INTO native_settings VALUES('agent:1','{\"turns\":{\"1\":{\"completed_at\":1700000000000,\"duration_ms\":5000}}}');").unwrap();
    drop(conn);
    for _ in 0..2 {
        let store = Store::open(&path).unwrap();
        let sessions = store.sessions().unwrap();
        assert_eq!(sessions[0].updated_at, 1_700_000_000_000);
        assert_eq!(sessions[1].updated_at, 0, "unknown ages are not invented");
        assert!(sessions.iter().all(|session| !session.archived));
        assert_eq!(
            store.load(1).unwrap().rows[0].sent_at,
            Some(1_699_999_995_000)
        );
        assert_eq!(store.load(2).unwrap().rows[0].sent_at, None);
    }
}
#[test]
fn send_times_survive_streaming_restart_and_queued_delivery() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("native.db");
    let mut store = Store::open(&path).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let sent_at = Some(1_700_000_000_000);
    let mut transcript = Transcript::default();
    transcript.begin("user");
    transcript.rows[0].sent_at = sent_at;
    transcript.append(&"A long queued prompt. ".repeat(300));
    assert!(transcript.rows.len() > 1);
    assert!(transcript.rows[1..].iter().all(|r| r.sent_at.is_none()));
    let answer = transcript.begin("assistant");
    transcript.append("Starting");
    store.write_rows(session.id, 0, &transcript.rows).unwrap();
    let answer_time = transcript.rows[answer].sent_at;
    transcript.append(" and finished");
    store
        .write_rows(session.id, answer, &transcript.rows[answer..])
        .unwrap();
    let mut roster = crate::bot::Roster::default();
    let bot = roster
        .create(crate::bot::Bot {
            name: "Test bot".into(),
            ..roster.draft(Vec::new())
        })
        .unwrap()
        .id
        .clone();
    roster
        .get_mut(&bot)
        .unwrap()
        .outbox
        .push(crate::bot::Message {
            prepared_prompt: None,
            peer: None,
            id: "queued".into(),
            text: "Tomorrow".into(),
            sent_at,
            internal: false,
            error: None,
            attachments: Vec::new(),
        });
    roster.save(&store).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    let saved = store.load(session.id).unwrap();
    assert_eq!(saved.rows, transcript.rows);
    assert_eq!(saved.rows[answer].sent_at, answer_time);
    assert_eq!(
        crate::bot::Roster::load(&store).unwrap().bots[0].outbox[0].sent_at,
        sent_at
    );
    let legacy: Row = serde_json::from_str(r#"{"role":"user","text":"Old","first":true}"#).unwrap();
    assert_eq!(legacy.sent_at, None);
}
#[test]
fn delayed_configuration_cannot_restore_an_old_model_or_provider() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let session = store.create("codex", "/tmp").unwrap();
    let old = crate::agent::Settings {
        revision: 1,
        model: Some("first-model".into()),
        ..Default::default()
    };
    let current = crate::agent::Settings {
        revision: 2,
        model: Some("selected-model".into()),
        effort: Some("high".into()),
        ..Default::default()
    };
    store
        .configure_agent(session.id, "claude", &current)
        .unwrap();
    store.provider_id(session.id, "new-thread").unwrap();
    store.configure_agent(session.id, "codex", &old).unwrap();
    let saved = store.agent_settings(session.id).unwrap().unwrap();
    assert_eq!(saved.model, current.model);
    assert_eq!(saved.effort, current.effort);
    let session = store.sessions().unwrap().remove(0);
    assert_eq!(session.provider, "claude");
    assert_eq!(session.provider_id.as_deref(), Some("new-thread"));
}
#[test]
fn wal_restart_preserves_partial_output_and_provider_context() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("native.db");
    let mut store = Store::open(&path).unwrap();
    let s = store.create("codex", "/tmp").unwrap();
    assert!(!s.has_messages);
    assert!(!store.sessions().unwrap()[0].has_messages);
    let mut t = Transcript::default();
    t.begin("user");
    t.append("hello");
    t.begin("assistant");
    t.append("partial 🙂");
    store.write_rows(s.id, 0, &t.rows).unwrap();
    store.provider_id(s.id, "thread-1").unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert!(store.sessions().unwrap()[0].has_messages);
    assert_eq!(store.load(s.id).unwrap().rows, t.rows);
    assert_eq!(
        store.sessions().unwrap()[0].provider_id.as_deref(),
        Some("thread-1")
    );
}
