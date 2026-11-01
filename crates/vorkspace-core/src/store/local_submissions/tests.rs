use super::*;
use serde_json::json;
use std::sync::{Arc, Barrier};
use std::thread;

fn saved_owner(conn: &Connection) {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY);")
        .unwrap();
    conn.execute(
        "INSERT INTO sessions(id) VALUES (?1)",
        ["chat/example.中国"],
    )
    .unwrap();
}

fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    ensure_table(&conn).unwrap();
    saved_owner(&conn);
    conn
}
fn input(id: &str) -> SubmissionInput {
    SubmissionInput {
        attempt_id: id.into(),
        user_turn_id: id.into(),
        session_id: "chat/example.中国".into(),
        payload_json: r#"{"cwd":"/repo","text":"hello","attachments":[{"data":"exact"}]}"#.into(),
        user_turn: Some(json!({"id":id,"role":"user","text":"hello"})),
    }
}

#[test]
fn a_missing_session_owner_cannot_create_an_unreachable_receipt() {
    let conn = db();
    conn.execute("DELETE FROM sessions", []).unwrap();
    assert!(reserve(&conn, input("orphan"), "sessions")
        .unwrap_err()
        .contains("has not been saved"));
    assert!(get(&conn, "orphan").unwrap().is_none());
    assert!(dispatch(&conn, "orphan").is_err());
}

#[test]
fn matching_ids_share_one_reservation_and_only_one_dispatch_claim() {
    let conn = db();
    let first = reserve(&conn, input("same"), "sessions").unwrap();
    assert!(first.created);
    assert!(!reserve(&conn, input("same"), "sessions").unwrap().created);
    assert!(dispatch(&conn, "same").unwrap().claimed);
    assert!(!dispatch(&conn, "same").unwrap().claimed);
    settle(&conn, "same", "completed", None).unwrap();
    let duplicate = reserve(&conn, input("same"), "sessions").unwrap();
    assert!(!duplicate.created);
    assert_eq!(duplicate.receipt.outcome.as_deref(), Some("completed"));
}

#[test]
fn changed_payload_attachments_or_scope_never_reuses_an_id() {
    let conn = db();
    reserve(&conn, input("same"), "sessions").unwrap();
    for field in ["payload", "attachments", "session", "turn"] {
        let mut changed = input("same");
        match field {
            "payload" => changed.payload_json = r#"{"text":"changed"}"#.into(),
            "attachments" => changed.payload_json = r#"{"attachments":[{"data":"other"}]}"#.into(),
            "session" => changed.session_id = "another/session".into(),
            "turn" => {
                changed.user_turn_id = "different".into();
                changed.user_turn = None;
            }
            _ => unreachable!(),
        }
        assert!(reserve(&conn, changed, "sessions")
            .unwrap_err()
            .contains("different scope or content"));
    }
}

#[test]
fn concurrent_connections_reserve_only_one_owner() {
    let path = std::env::temp_dir().join(format!("local-submission-{}.db", uuid::Uuid::new_v4()));
    let first = Connection::open(&path).unwrap();
    ensure_table(&first).unwrap();
    saved_owner(&first);
    drop(first);
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let conn = Connection::open(path).unwrap();
                conn.busy_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                barrier.wait();
                reserve(&conn, input("concurrent"), "sessions")
                    .unwrap()
                    .created
            })
        })
        .collect();
    let count = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .filter(|created| *created)
        .count();
    assert_eq!(count, 1);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn reopen_marks_both_crash_boundaries_uncertain_and_preserves_live_owners() {
    let path = std::env::temp_dir().join(format!(
        "local-submission-reopen-{}.db",
        uuid::Uuid::new_v4()
    ));
    let conn = Connection::open(&path).unwrap();
    ensure_table(&conn).unwrap();
    saved_owner(&conn);
    for id in ["before", "after", "live-owner", "this-owner"] {
        reserve(&conn, input(id), "sessions").unwrap();
    }
    dispatch(&conn, "after").unwrap();
    conn.execute("UPDATE local_submissions SET owner_instance='dead',owner_pid=2147483647 WHERE attempt_id IN ('before','after')", []).unwrap();
    conn.execute("UPDATE local_submissions SET owner_instance='another-live-instance' WHERE attempt_id='live-owner'", []).unwrap();
    drop(conn);
    let conn = Connection::open(&path).unwrap();
    ensure_table(&conn).unwrap();
    recover_unresolved(&conn).unwrap();
    for id in ["before", "after"] {
        assert_eq!(get(&conn, id).unwrap().unwrap().phase, "uncertain");
        assert!(!dispatch(&conn, id).unwrap().claimed);
    }
    for id in ["live-owner", "this-owner"] {
        assert_eq!(get(&conn, id).unwrap().unwrap().phase, "reserved");
    }
    drop(conn);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn deletion_keeps_identity_tombstones_and_defeats_delayed_new_reservations() {
    let conn = db();
    reserve(&conn, input("old"), "sessions").unwrap();
    delete_session(&conn, "chat/example.中国").unwrap();
    assert!(!dispatch(&conn, "old").unwrap().claimed);
    assert!(reserve(&conn, input("delayed"), "sessions").is_err());
    let mut moved = input("old");
    moved.session_id = "other".into();
    assert!(reserve(&conn, moved, "sessions").is_err());
    assert!(get(&conn, "old").unwrap().unwrap().user_turn.is_none());
}

#[test]
fn reservation_store_errors_leave_no_dispatchable_attempt() {
    let conn = db();
    conn.execute_batch("CREATE TRIGGER fail_insert BEFORE INSERT ON local_submissions BEGIN SELECT RAISE(ABORT, 'disk failed'); END;").unwrap();
    assert!(reserve(&conn, input("error"), "sessions")
        .unwrap_err()
        .contains("disk failed"));
    assert!(get(&conn, "error").unwrap().is_none());
    assert!(dispatch(&conn, "error").is_err());
}

#[test]
fn recovery_snapshot_strips_live_bytes_and_rejects_mismatched_identity() {
    let conn = db();
    let mut value = input("safe");
    value.user_turn = Some(
        json!({"id":"safe","role":"user","text":"hello","internal":true,"credential":"private","attachments":[{"id":"image","kind":"image","name":"a.png","mimeType":"image/png","size":4,"data":"base64","previewUrl":"blob:private","path":"data:private"}]}),
    );
    let safe = reserve(&conn, value, "sessions")
        .unwrap()
        .receipt
        .user_turn
        .unwrap();
    assert!(safe.get("credential").is_none());
    assert!(safe["attachments"][0].get("data").is_none());
    assert!(safe["attachments"][0].get("previewUrl").is_none());
    assert!(safe["attachments"][0].get("path").is_none());
    let mut bad = input("bad");
    bad.user_turn = Some(json!({"id":"different","role":"user","text":"hello"}));
    assert!(reserve(&conn, bad, "sessions").is_err());
}
#[test]
fn real_store_uses_original_session_identity_and_reads_do_not_create_tables() {
    for shared in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sessions.db");
        let mut store = if shared {
            Store::open_vorkspace(&path)
        } else {
            Store::open(&path)
        }
        .unwrap();
        let session = store.create("codex", "/project").unwrap();
        assert!(store.local_submissions(session.id).unwrap().is_empty());
        store.recover_local_submissions().unwrap();
        assert!(!super::super::shared::has_table(&store.conn, "local_submissions").unwrap());
        let reserved = store
            .reserve_local_submission(session.id, input("durable"))
            .unwrap();
        let source = if shared {
            store.persisted_session_id(session.id).unwrap()
        } else {
            session.id.to_string()
        };
        assert_eq!(reserved.receipt.session_id, source);
        assert!(store.dispatch_local_submission("durable").unwrap().claimed);
        store
            .conn
            .execute(
                "UPDATE local_submissions SET owner_instance='dead',owner_pid=2147483647",
                [],
            )
            .unwrap();
        drop(store);
        store = if shared {
            Store::open_vorkspace(&path)
        } else {
            Store::open(&path)
        }
        .unwrap();
        store.recover_local_submissions().unwrap();
        let receipts = store.local_submissions(session.id).unwrap();
        assert_eq!(receipts[0].phase, "uncertain");
        assert!(!store.dispatch_local_submission("durable").unwrap().claimed);
        assert_eq!(receipts[0].user_turn.as_ref().unwrap()["text"], "hello");
        store.delete(session.id).unwrap();
        assert!(store
            .reserve_local_submission(session.id, input("late"))
            .is_err());
        assert!(get(&store.conn, "durable")
            .unwrap()
            .unwrap()
            .user_turn
            .is_none());
    }
}
#[test]
fn failed_native_delete_rolls_back_receipts_and_session_together() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native.db");
    let mut store = Store::open(&path).unwrap();
    let session = store.create("codex", "/project").unwrap().id;
    store
        .reserve_local_submission(session, input("kept"))
        .unwrap();
    store.conn.execute_batch("CREATE TRIGGER refuse_delete BEFORE DELETE ON native_sessions BEGIN SELECT RAISE(ABORT,'delete denied'); END").unwrap();
    assert!(store.delete(session).unwrap_err().contains("delete denied"));
    drop(store);
    let store = Store::open(&path).unwrap();
    let receipt = &store.local_submissions(session).unwrap()[0];
    assert_eq!(receipt.phase, "reserved");
    assert!(receipt.user_turn.is_some());
    assert!(store.dispatch_local_submission("kept").unwrap().claimed);
    store
        .settle_local_submission("kept", "failed", None)
        .unwrap();
    assert!(store
        .reserve_local_submission(session, input("next"))
        .is_ok());
}
#[test]
fn a_post_insert_read_error_rolls_back_the_entire_reservation() {
    use rusqlite::hooks::{Action, AuthAction, AuthContext, Authorization};
    use std::sync::atomic::{AtomicBool, Ordering};
    let conn = db();
    let inserted = Arc::new(AtomicBool::new(false));
    let flag = inserted.clone();
    conn.update_hook(Some(move |action: Action, _: &str, table: &str, _: i64| {
        if action == Action::SQLITE_INSERT && table == "local_submissions" {
            flag.store(true, Ordering::SeqCst);
        }
    }))
    .unwrap();
    let flag = inserted.clone();
    conn.authorizer(Some(move |context: AuthContext<'_>| {
        if matches!(
            context.action,
            AuthAction::Read {
                table_name: "local_submissions",
                ..
            }
        ) && flag.load(Ordering::SeqCst)
        {
            Authorization::Deny
        } else {
            Authorization::Allow
        }
    }))
    .unwrap();
    assert!(reserve(&conn, input("post-insert"), "sessions").is_err());
    assert!(inserted.load(Ordering::SeqCst));
    conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
    assert!(get(&conn, "post-insert").unwrap().is_none());
}
#[test]
fn a_finished_worker_recovers_in_the_same_process_after_transient_settlement_failure() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let session = store.create("codex", "/project").unwrap().id;
    store
        .reserve_local_submission(session, input("transient"))
        .unwrap();
    store.dispatch_local_submission("transient").unwrap();
    let owner = store.local_submission_owner();
    store.conn.execute_batch("CREATE TRIGGER refuse_settle BEFORE UPDATE ON local_submissions BEGIN SELECT RAISE(ABORT,'busy storage'); END").unwrap();
    assert!(store
        .settle_local_submission("transient", "completed", None)
        .is_err());
    owner.release("transient");
    store
        .conn
        .execute_batch("DROP TRIGGER refuse_settle")
        .unwrap();
    store.recover_local_submissions().unwrap();
    assert_eq!(
        store
            .latest_unconfirmed_submission(session)
            .unwrap()
            .unwrap()
            .attempt_id,
        "transient"
    );
    assert!(
        !store
            .dispatch_local_submission("transient")
            .unwrap()
            .claimed
    );
}
