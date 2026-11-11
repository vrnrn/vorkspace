use super::*;
use crate::{
    agent::{Settings, TurnInfo},
    store::Store,
};
use serde_json::json;
use std::io::Write;

pub(crate) fn append(path: &Path, values: &[Value]) {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .unwrap();
    for value in values {
        writeln!(file, "{value}").unwrap();
    }
}
pub(crate) fn meta() -> Value {
    json!({"type":"session_meta","payload":{"id":"thread"}})
}
pub(crate) fn turn(id: &str, prompt: &str, answer: &str) -> Vec<Value> {
    vec![
        json!({"timestamp":"2026-11-06T00:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":id}}),
        json!({"type":"turn_context","payload":{"turn_id":id,"model":"test-model"}}),
        json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":id,"item":{"type":"UserMessage","id":format!("{id}-user"),"content":[{"type":"text","text":prompt}]}}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Injected instructions must not appear"}]}}),
        json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":id,"item":{"type":"AgentMessage","id":format!("{id}-answer"),"content":[{"type":"Text","text":answer}]}}}),
        json!({"type":"event_msg","payload":{"type":"agent_message","message":answer}}),
        json!({"timestamp":"2026-11-06T00:00:02Z","type":"event_msg","payload":{"type":"task_complete","turn_id":id}}),
    ]
}
pub(crate) fn fixture(
    root: &Path,
    provider: &str,
) -> (Store, crate::store::Session, PathBuf, PathBuf) {
    let path = root.join("desktop.db");
    let store = { Store::test(&path) }.unwrap();
    let session = store.create(provider, root.to_str().unwrap()).unwrap();
    let settings = Settings {
        account: Profile::new("fixture", root),
        ..Default::default()
    };
    store.save_agent_settings(session.id, &settings).unwrap();
    store.provider_id(session.id, "thread").unwrap();
    let home = root
        .join("provider-accounts")
        .join(provider)
        .join("fixture");
    let dir = home.join(if provider == "codex" {
        "sessions"
    } else {
        "projects/home"
    });
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join(if provider == "codex" {
        "rollout-2026-11-06-thread.jsonl"
    } else {
        "thread.jsonl"
    });
    (store, session, source, path)
}
pub(crate) fn sync(store: &mut Store, id: i64, root: &Path) -> crate::store::NativeHistoryResult {
    let mut request = store
        .native_history_request(id, &root.join("bots"))
        .unwrap()
        .unwrap();
    let batch = read(&request.binding, request.cursor.as_ref())
        .unwrap()
        .unwrap();
    store.import_native_history(&mut request, batch).unwrap()
}

fn claude_user(id: &str, body: Value) -> Value {
    json!({"type":"user","uuid":id,"sessionId":"thread","message":{"content":body}})
}

fn claude_answer(id: &str, kind: &str, body: &str) -> Value {
    json!({"type":"assistant","uuid":id,"sessionId":"thread","message":{"model":"claude-test","stop_reason":"end_turn","content":[{"type":kind,"text":body}]}})
}

#[test]
fn native_history_claude_rescans_keep_each_message_once_after_reopen() {
    {
        for (legacy_receipt, remove_leaf) in [(false, false), (true, false), (true, true)] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let (mut store, session, source, path) = fixture(root, "claude");
            let mut records = vec![
                claude_user("u1", json!("Question")),
                claude_answer("a1", "text", "First answer"),
                claude_user("tool", json!([{"type":"tool_result","content":"hidden"}])),
            ];
            append(&source, &records);
            assert_eq!(sync(&mut store, session.id, root).imported, 1);
            let before = store.load(session.id).unwrap().rows;
            assert_eq!(before.len(), 2);
            if legacy_receipt {
                let conn = rusqlite::Connection::open(&path).unwrap();
                // The old importer recorded only the final leaf, even when
                // that record had no visible text of its own.
                conn.execute_batch(
                    "DELETE FROM provider_history_receipts WHERE native_id='claude-message:u1';
                     UPDATE provider_history_receipts SET native_id='tool' WHERE native_id='claude-message:a1';
                     UPDATE provider_history_cursors SET cursor_json=json_set(
                        json_remove(cursor_json,'$.claude_message_receipts'),'$.claude_leaf','tool');"
                ).unwrap();
            }
            drop(store);
            if remove_leaf {
                records.pop();
            }
            records.push(claude_answer("a2", "text", "Late answer"));
            let replacement = source.with_extension("replacement");
            append(&replacement, &records);
            std::fs::rename(replacement, &source).unwrap();

            let mut store = { Store::test(&path) }.unwrap();
            let result = sync(&mut store, session.id, root);
            assert_eq!(result.imported, 1);
            assert!(result.reload_provider);
            let after = store.load(session.id).unwrap().rows;
            assert_eq!(&after[..before.len()], &before);
            assert_eq!(after.len(), 3);
            assert_eq!(after[2].text, "Late answer");

            let replacement = source.with_extension("replacement");
            append(&replacement, &records);
            std::fs::rename(replacement, &source).unwrap();
            let result = sync(&mut store, session.id, root);
            assert_eq!(result.imported, 0);
            assert!(!result.reload_provider);
            assert_eq!(store.load(session.id).unwrap().rows, after);
        }
    }
}

#[test]
fn native_history_claude_late_blocks_preserve_managed_ownership() {
    for has_identity in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (mut store, session, source, _) = fixture(root, "claude");
        let mut local = Transcript::default();
        local.begin("user");
        local.append("Managed prompt");
        local.begin("assistant");
        local.append("Managed rich answer");
        store.write_rows(session.id, 0, &local.rows).unwrap();
        let mut settings = store.agent_settings(session.id).unwrap().unwrap();
        settings.turns.insert(
            1,
            TurnInfo {
                provider_turn_id: has_identity.then(|| "managed".into()),
                provider: "claude".into(),
                started_at: Some(100),
                completed_at: Some(200),
                duration_ms: 100,
                ..Default::default()
            },
        );
        store.save_agent_settings(session.id, &settings).unwrap();
        let before = store.load(session.id).unwrap().rows;
        let mut prompt = claude_user("managed", json!("Managed prompt from provider"));
        prompt["timestamp"] = json!(110);
        append(
            &source,
            &[
                prompt,
                claude_answer("a1", "text", "Managed answer from provider"),
            ],
        );
        let result = sync(&mut store, session.id, root);
        assert_eq!(result.imported, 0);
        assert!(!result.reload_provider);
        append(
            &source,
            &[claude_answer("a2", "text", "Managed late block")],
        );
        let result = sync(&mut store, session.id, root);
        assert_eq!(result.imported, 0);
        assert!(!result.reload_provider);
        assert_eq!(store.load(session.id).unwrap().rows, before);
    }
}

#[test]
fn native_history_import_is_durable_idempotent_and_keeps_managed_rows_and_unsent_work() {
    {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (mut store, session, source, path) = fixture(root, "codex");
        let mut local = Transcript::default();
        local.begin("user");
        local.append("Local prompt");
        local.begin("assistant");
        local.append("Local rich answer");
        store.write_rows(session.id, 0, &local.rows).unwrap();
        let mut settings = store.agent_settings(session.id).unwrap().unwrap();
        settings.turns.insert(
            1,
            TurnInfo {
                provider_turn_id: Some("managed".into()),
                provider: "codex".into(),
                ..Default::default()
            },
        );
        store.save_agent_settings(session.id, &settings).unwrap();
        let draft = json!({"text":"Unsent", "cwd":session.cwd,"selectionStart":2,"selectionEnd":4,"attachments":[{"path":"/tmp/user-owned.txt"}],"mcpTags":[]});
        store.save_composer_draft(session.id, 0, &draft).unwrap();
        let queue = crate::follow_up::State {
            messages: vec![crate::follow_up::Message {
                id: "queued".into(),
                text: "Still pending".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        store.save_follow_ups(session.id, &queue).unwrap();
        append(&source, &[meta()]);
        append(
            &source,
            &turn("managed", "Injected local prompt", "Local rich answer"),
        );
        let own = sync(&mut store, session.id, root);
        assert_eq!(own.imported, 0);
        assert!(
            !own.reload_provider,
            "Managed history must preserve the warm process"
        );
        append(&source, &turn("outside", "CLI question", "CLI answer"));
        let external = sync(&mut store, session.id, root);
        assert_eq!(external.imported, 1);
        assert!(external.reload_provider);
        let rows = store.load(session.id).unwrap().rows;
        assert_eq!(&rows[..local.rows.len()], &local.rows);
        assert_eq!(
            rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(),
            [
                "Local prompt",
                "Local rich answer",
                "CLI question",
                "CLI answer"
            ]
        );
        assert_eq!(
            store.composer_draft(session.id).unwrap().unwrap().payload,
            draft
        );
        assert_eq!(
            store.follow_ups(session.id).unwrap().messages[0].text,
            "Still pending"
        );
        drop(store);
        let mut store = { Store::test(&path) }.unwrap();
        assert_eq!(sync(&mut store, session.id, root).imported, 0);
        assert_eq!(store.load(session.id).unwrap().rows, rows);
        // Atomic file replacement forces a rescan, not another copy of history.
        let contents = std::fs::read(&source).unwrap();
        let next = source.with_extension("tmp");
        std::fs::write(&next, contents).unwrap();
        std::fs::rename(next, &source).unwrap();
        assert_eq!(sync(&mut store, session.id, root).imported, 0);
        append(
            &source,
            &turn("outside-2", "Next CLI question", "Next CLI answer"),
        );
        assert_eq!(sync(&mut store, session.id, root).imported, 1);
    }
}

#[test]
fn native_history_waits_for_complete_records_and_commits_cursor_with_rows() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (mut store, session, source, path) = fixture(root, "codex");
    append(&source, &[meta()]);
    let records = turn("outside", "Question", "Answer");
    append(&source, &records[..6]);
    let result = sync(&mut store, session.id, root);
    assert!(result.active);
    assert_eq!(result.imported, 0);
    let last = records[6].to_string();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&source)
        .unwrap();
    file.write_all(&last.as_bytes()[..10]).unwrap();
    assert!(sync(&mut store, session.id, root).active);
    file.write_all(&last.as_bytes()[10..]).unwrap();
    file.write_all(b"\n").unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_cursor BEFORE UPDATE ON provider_history_cursors BEGIN SELECT RAISE(ABORT,'injected cursor write failure'); END;").unwrap();
    let mut request = store
        .native_history_request(session.id, &root.join("bots"))
        .unwrap()
        .unwrap();
    let batch = read(&request.binding, request.cursor.as_ref())
        .unwrap()
        .unwrap();
    assert!(store
        .import_native_history(&mut request, batch)
        .unwrap_err()
        .contains("injected"));
    assert!(store.load(session.id).unwrap().rows.is_empty());
    connection
        .execute_batch("DROP TRIGGER fail_cursor")
        .unwrap();
    assert_eq!(sync(&mut store, session.id, root).imported, 1);
    assert_eq!(store.load(session.id).unwrap().rows.len(), 2);
}

#[test]
fn unchanged_native_reads_have_no_cursor_to_publish_and_same_size_rewrites_are_seen() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (store, session, source, _) = fixture(root, "codex");
    append(&source, &[meta()]);
    append(&source, &turn("outside", "Question", "First"));
    let request = store
        .native_history_request(session.id, &root.join("bots"))
        .unwrap()
        .unwrap();
    let initial = read(&request.binding, None)
        .unwrap()
        .unwrap()
        .update
        .unwrap();
    let unchanged = read(&request.binding, Some(&initial.cursor))
        .unwrap()
        .unwrap();
    assert!(unchanged.update.is_none());
    let time = std::fs::metadata(&source).unwrap().modified().unwrap();
    let raw = std::fs::read_to_string(&source).unwrap();
    let edited = raw.replace("First", "Other");
    assert_eq!(raw.len(), edited.len());
    std::fs::write(&source, edited).unwrap();
    File::open(&source)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(time))
        .unwrap();
    let rewritten = read(&request.binding, Some(&initial.cursor))
        .unwrap()
        .unwrap()
        .update
        .unwrap();
    assert_eq!(rewritten.turns[0].messages.last().unwrap().text, "Other");
}

#[test]
fn native_history_recovery_commits_saved_text_with_confirmation_and_keeps_codex_suffix() {
    {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (mut store, session, source, path) = fixture(root, "codex");
        let records = turn("outside", "Question", "Repeated answer");
        append(&source, &[meta()]);
        append(&source, &records[..6]);
        assert!(sync(&mut store, session.id, root).active);
        let request = store
            .native_history_request(session.id, &root.join("bots"))
            .unwrap()
            .unwrap();
        let token = Store::native_history_recovery_token(&request).unwrap();

        let connection = rusqlite::Connection::open(&path).unwrap();
        let (session_key, binding, saved_cursor): (String, String, String) = connection
            .query_row(
                "SELECT session_id,binding,cursor_json FROM provider_history_cursors",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let old_token = crate::bot::files::content_hash(&format!("{binding}:{saved_cursor}"));
        connection
            .execute(
                "INSERT INTO provider_history_recoveries VALUES(?1,?2,?3)",
                rusqlite::params![session_key, binding, old_token],
            )
            .unwrap();
        assert!(!store.native_history_acknowledged(&request, &token).unwrap());
        connection
            .execute_batch(
                "CREATE TRIGGER fail_recovery BEFORE INSERT ON provider_history_recoveries
             BEGIN SELECT RAISE(ABORT,'injected recovery write failure'); END;",
            )
            .unwrap();
        assert!(store
            .acknowledge_native_history(&request, &token)
            .unwrap_err()
            .contains("injected"));
        assert!(store.load(session.id).unwrap().rows.is_empty());
        assert!(!store.native_history_acknowledged(&request, &token).unwrap());
        connection
            .execute_batch("DROP TRIGGER fail_recovery")
            .unwrap();

        let original = std::fs::read(&source).unwrap();
        store.acknowledge_native_history(&request, &token).unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert!(store.native_history_acknowledged(&request, &token).unwrap());
        let prefix = store.load(session.id).unwrap().rows;
        assert_eq!(
            prefix
                .iter()
                .map(|row| row.text.as_str())
                .collect::<Vec<_>>(),
            ["Question", "Repeated answer"]
        );

        append(
            &source,
            &[
                json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":"outside","item":{"type":"AgentMessage","id":"second-answer","text":"Repeated answer"}}}),
                records[6].clone(),
            ],
        );
        let result = sync(&mut store, session.id, root);
        assert!(!result.active);
        assert_eq!(result.imported, 1);
        let rows = store.load(session.id).unwrap().rows;
        assert_eq!(&rows[..2], prefix.as_slice());
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].text, "Repeated answer");
    }
}

#[test]
fn native_history_rejects_changed_binding_cursor_and_local_writer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (mut store, session, source, _) = fixture(root, "codex");
    append(&source, &[meta()]);
    append(&source, &turn("outside", "Question", "Answer"));
    let mut request = store
        .native_history_request(session.id, &root.join("bots"))
        .unwrap()
        .unwrap();
    let batch = read(&request.binding, None).unwrap().unwrap();
    store.provider_id(session.id, "replacement").unwrap();
    assert!(
        store
            .import_native_history(&mut request, batch)
            .unwrap()
            .superseded
    );
    store.provider_id(session.id, "thread").unwrap();
    let receipt = store
        .reserve_local_submission(
            session.id,
            crate::store::SubmissionInput {
                attempt_id: "local".into(),
                user_turn_id: "local".into(),
                session_id: String::new(),
                payload_json: "{}".into(),
                user_turn: None,
            },
        )
        .unwrap();
    assert!(receipt.created);
    assert!(sync(&mut store, session.id, root).superseded);
    store
        .settle_local_submission("local", "failed", None)
        .unwrap();
    let mut stale = store
        .native_history_request(session.id, &root.join("bots"))
        .unwrap()
        .unwrap();
    let old_batch = read(&stale.binding, None).unwrap().unwrap();
    assert_eq!(sync(&mut store, session.id, root).imported, 1);
    assert!(
        store
            .import_native_history(&mut stale, old_batch)
            .unwrap()
            .superseded
    );
    assert_eq!(store.load(session.id).unwrap().rows.len(), 2);
}

#[test]
fn native_history_claude_ignores_tools_and_sidechains_but_keeps_late_final_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (mut store, session, source, _) = fixture(root, "claude");
    append(
        &source,
        &[
            claude_user("u1", json!("Question")),
            claude_answer("a1", "thinking", "Private reasoning"),
        ],
    );
    assert_eq!(sync(&mut store, session.id, root).imported, 1);
    append(&source, &[claude_answer("a2", "text", "Answer")]);
    assert_eq!(sync(&mut store, session.id, root).imported, 1);
    let mut sidechain = claude_answer("side", "text", "Ignore subagent");
    sidechain["isSidechain"] = json!(true);
    append(&source, &[sidechain]);
    assert_eq!(sync(&mut store, session.id, root).imported, 0);
    assert_eq!(
        store
            .load(session.id)
            .unwrap()
            .rows
            .iter()
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>(),
        ["Question", "Answer"]
    );
    append(
        &source,
        &[
            claude_user("u2", json!("Next question")),
            claude_user(
                "tool",
                json!([{"type":"tool_result","content":"not a prompt"}]),
            ),
            claude_answer("a3", "text", "Next answer"),
        ],
    );
    assert_eq!(sync(&mut store, session.id, root).imported, 1);
    assert_eq!(sync(&mut store, session.id, root).imported, 0);
}

#[test]
fn native_history_reads_compressed_codex_with_append_tail_and_compaction_marker() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (mut store, session, source, _) = fixture(root, "codex");
    append(&source, &[meta()]);
    append(&source, &turn("old", "Before", "Before answer"));
    let compressed = source.with_extension("jsonl.zst");
    std::fs::write(
        &compressed,
        zstd::encode_all(std::fs::read(&source).unwrap().as_slice(), 1).unwrap(),
    )
    .unwrap();
    std::fs::write(&source, "").unwrap();
    append(&source, &turn("new", "After", "After answer"));
    append(
        &source,
        &[json!({"type":"compacted","timestamp":"2026-11-06T00:01:00Z","payload":{}})],
    );
    assert_eq!(sync(&mut store, session.id, root).imported, 2);
    let request = store
        .native_history_request(session.id, &root.join("bots"))
        .unwrap()
        .unwrap();
    assert!(request.cursor.unwrap().compaction.is_some());
    append(&source, &turn("last", "Last", "Last answer"));
    assert_eq!(sync(&mut store, session.id, root).imported, 1);
}

#[test]
fn native_history_refreshes_usage_and_discards_it_after_external_compaction() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let (mut store, session, source, path) = fixture(root, "codex");
    append(
        &source,
        &[
            meta(),
            json!({"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_tokens":95000},"model_context_window":100000}}}),
        ],
    );
    sync(&mut store, session.id, root);
    assert_eq!(
        store.context_usage(session.id).unwrap(),
        Some(crate::context::Usage {
            used: 95000,
            window: Some(100000)
        })
    );
    append(
        &source,
        &[json!({"type":"compacted","timestamp":"2026-11-06T01:00:00Z","payload":{}})],
    );
    sync(&mut store, session.id, root);
    assert_eq!(
        store.context_usage(session.id).unwrap(),
        Some(crate::context::Usage::default())
    );
    store.delete(session.id).unwrap();
    let connection = rusqlite::Connection::open(path).unwrap();
    let remaining: i64 = connection
        .query_row("SELECT count(*) FROM provider_history_cursors", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(remaining, 0);
}
