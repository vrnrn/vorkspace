use super::*;
use std::sync::{atomic::AtomicBool, Arc};

#[test]
fn pages_are_ordered_and_snapshot_bound_across_edits_and_retries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let mut store = Store::open_vorkspace(&path).unwrap();
    store
        .conn
        .execute_batch(include_str!("../../../../tests/fixtures/bot-schema.sql"))
        .unwrap();
    let id = store.create("codex", "/project").unwrap().id;
    let original = store.persisted_session_id(id).unwrap();
    store
        .conn
        .execute("INSERT INTO bot_transcripts VALUES(?1)", [&original])
        .unwrap();
    let blocks: Vec<_> = (0..50)
            .map(|i| {
                json!({
                    "id": format!("{i}"), "role": if i % 2 == 0 { "user" } else { "assistant" },
                    "text": format!("needle {i}"), "future": {"keep": i},
                    "turnModel": if i % 2 == 0 { json!({"harness":"codex", "name":format!("model-{i}")}) } else { Value::Null }
                })
            })
            .collect();
    store.shared_store_blocks(id, &blocks).unwrap();
    let full = blocks::project(&blocks).unwrap();
    let newest = store.indexed_history_page(id, None, 10).unwrap().unwrap();
    assert_eq!(newest.global_start, 30);
    assert_eq!(newest.rows, full[30..]);
    let full_turns = project_turns(&blocks).unwrap();
    assert_eq!(newest.turns.len(), 10);
    for (row, info) in &newest.turns {
        assert_eq!(info.model, full_turns[row].model);
    }
    let cursor = newest.before.unwrap();
    let middle = store
        .indexed_history_page(id, Some(&cursor), 10)
        .unwrap()
        .unwrap();
    assert_eq!(middle.global_start, 10);
    assert_eq!(middle.rows, full[10..30]);
    for (row, info) in &middle.turns {
        assert_eq!(info.model, full_turns[row].model);
    }
    let first = store
        .indexed_history_page(id, middle.before.as_ref(), 10)
        .unwrap()
        .unwrap();
    assert_eq!(first.global_start, 0);
    assert_eq!(first.rows, full[..10]);
    assert!(first.before.is_none());
    let (hits, _) = store
        .search_sessions("needle 0", Arc::new(AtomicBool::new(false)))
        .unwrap();
    assert!(hits.iter().any(|hit| hit.target
        == crate::search::Target::Message {
            session: id,
            row: 0
        }));

    let other = Connection::open(&path).unwrap();
    other.execute("UPDATE bot_blocks SET block_json=json_set(block_json,'$.text','external') WHERE session_id=?1 AND seq=1", [&original]).unwrap();
    assert!(store
        .indexed_history_page(id, Some(&cursor), 10)
        .unwrap_err()
        .contains("changed"));
    let retry = store.indexed_history_page(id, None, 10).unwrap().unwrap();
    assert!(store
        .indexed_history_page(id, retry.before.as_ref(), 10)
        .unwrap()
        .is_some());

    other
        .execute(
            "DELETE FROM bot_turns WHERE session_id=?1 AND seq=1",
            [&original],
        )
        .unwrap();
    assert!(store
        .indexed_history_page(id, retry.before.as_ref(), 10)
        .unwrap_err()
        .contains("changed"));
    other
        .execute(
            "INSERT INTO bot_turns(session_id,seq) VALUES(?1,1)",
            [&original],
        )
        .unwrap();
    let retry = store.indexed_history_page(id, None, 10).unwrap().unwrap();
    other
        .execute(
            "UPDATE bot_blocks SET seq=9999 WHERE session_id=?1 AND seq=50",
            [&original],
        )
        .unwrap();
    assert!(store
        .indexed_history_page(id, retry.before.as_ref(), 10)
        .unwrap_err()
        .contains("changed"));
    other
        .execute(
            "UPDATE bot_blocks SET seq=50 WHERE session_id=?1 AND seq=9999",
            [&original],
        )
        .unwrap();

    let mut complete = store.load(id).unwrap();
    let at = complete.begin("user");
    complete.append("new tail");
    store.write_rows(id, at, &complete.rows[at..]).unwrap();
    assert!(store
        .indexed_history_page(id, retry.before.as_ref(), 10)
        .unwrap_err()
        .contains("changed"));
    let stored = store.shared_blocks(id).unwrap();
    assert_eq!(stored.len(), 51);
    assert_eq!(stored[0]["text"], "external");
    assert_eq!(stored[0]["future"]["keep"], 0);
}

#[test]
fn unsupported_histories_and_invalid_page_sizes_fail_closed() {
    let store = Store::test(Path::new(":memory:")).unwrap();
    let id = store.create("codex", "/project").unwrap().id;
    assert!(store.indexed_history_page(id, None, 10).unwrap().is_none());
    assert!(store.indexed_history_page(id, None, 0).is_err());
}
