use super::*;
use crate::{
    agent::{Settings, TurnInfo},
    store::Store,
};
use serde_json::json;
use std::io::Write;

fn append(path: &Path, values: &[Value]) {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .unwrap();
    for value in values {
        writeln!(file, "{value}").unwrap();
    }
}
fn meta() -> Value {
    json!({"type":"session_meta","payload":{"id":"thread"}})
}
fn turn(id: &str, prompt: &str, answer: &str) -> Vec<Value> {
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
fn fixture(
    root: &Path,
    shared: bool,
    provider: &str,
) -> (Store, crate::store::Session, PathBuf, PathBuf) {
    let path = root.join(if shared {
        "desktop.db"
    } else {
        "native.sqlite"
    });
    let store = if shared {
        Store::open_vorkspace(&path)
    } else {
        Store::open(&path)
    }
    .unwrap();
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
fn sync(store: &mut Store, id: i64, root: &Path) -> crate::store::NativeHistoryResult {
    let request = store
        .native_history_request(id, &root.join("bots"))
        .unwrap()
        .unwrap();
    let batch = read(&request.binding, request.cursor.as_ref())
        .unwrap()
        .unwrap();
    store.import_native_history(&request, batch).unwrap()
}

#[test]
fn native_history_import_is_durable_idempotent_and_keeps_managed_rows_and_unsent_work() {
    for shared in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (mut store, session, source, path) = fixture(root, shared, "codex");
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
        let mut store = if shared {
            Store::open_vorkspace(&path)
        } else {
            Store::open(&path)
        }
        .unwrap();
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
    let (mut store, session, source, path) = fixture(root, true, "codex");
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
    let request = store
        .native_history_request(session.id, &root.join("bots"))
        .unwrap()
        .unwrap();
    let batch = read(&request.binding, request.cursor.as_ref())
        .unwrap()
        .unwrap();
    assert!(store
        .import_native_history(&request, batch)
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
fn native_history_rejects_changed_binding_cursor_and_local_writer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (mut store, session, source, _) = fixture(root, true, "codex");
    append(&source, &[meta()]);
    append(&source, &turn("outside", "Question", "Answer"));
    let request = store
        .native_history_request(session.id, &root.join("bots"))
        .unwrap()
        .unwrap();
    let batch = read(&request.binding, None).unwrap().unwrap();
    store.provider_id(session.id, "replacement").unwrap();
    assert!(
        store
            .import_native_history(&request, batch)
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
    let stale = store
        .native_history_request(session.id, &root.join("bots"))
        .unwrap()
        .unwrap();
    let old_batch = read(&stale.binding, None).unwrap().unwrap();
    assert_eq!(sync(&mut store, session.id, root).imported, 1);
    assert!(
        store
            .import_native_history(&stale, old_batch)
            .unwrap()
            .superseded
    );
    assert_eq!(store.load(session.id).unwrap().rows.len(), 2);
}

#[test]
fn native_history_claude_ignores_tools_and_sidechains_but_keeps_late_final_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (mut store, session, source, _) = fixture(root, true, "claude");
    let user = |id: &str, body: &str| json!({"type":"user","uuid":id,"sessionId":"thread","message":{"content":body}});
    let answer = |id: &str, kind: &str, body: &str| json!({"type":"assistant","uuid":id,"sessionId":"thread","message":{"model":"claude-test","stop_reason":"end_turn","content":[{"type":kind,"text":body}]}});
    append(
        &source,
        &[
            user("u1", "Question"),
            answer("a1", "thinking", "Private reasoning"),
        ],
    );
    assert_eq!(sync(&mut store, session.id, root).imported, 1);
    append(&source, &[answer("a2", "text", "Answer")]);
    assert_eq!(sync(&mut store, session.id, root).imported, 1);
    let mut sidechain = answer("side", "text", "Ignore subagent");
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
            user("u2", "Next question"),
            json!({"type":"user","uuid":"tool","sessionId":"thread","message":{"content":[{"type":"tool_result","content":"not a prompt"}]}}),
            answer("a3", "text", "Next answer"),
        ],
    );
    assert_eq!(sync(&mut store, session.id, root).imported, 1);
    assert_eq!(sync(&mut store, session.id, root).imported, 0);
}

#[test]
fn native_history_reads_compressed_codex_with_append_tail_and_compaction_marker() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (mut store, session, source, _) = fixture(root, true, "codex");
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
    let (mut store, session, source, path) = fixture(root, true, "codex");
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
