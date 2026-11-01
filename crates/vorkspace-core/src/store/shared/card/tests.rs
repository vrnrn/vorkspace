
use super::*;

fn source(later: usize) -> Vec<Value> {
    let mut blocks = vec![json!({
        "id":"card", "role":"assistant", "text":"Card preface", "sentAt":123,
        "botCard":{"type":"session","sessionId":"external-child","title":"Old",
            "future":{"opaque":[1,2,3]}},
        "future":{"rich":true}, "appRequestId":"receipt", "providerTurnId":"turn",
        "turnModel":{"harness":"codex","id":"saved"}
    })];
    blocks.extend((0..later).map(|i| {
        json!({"id":format!("later-{i}"),
            "role":if i % 2 == 0 {"user"} else {"assistant"}, "text":"x".repeat(2048),
            "future":{"ordinal":i}, "appRequestId":format!("later-receipt-{i}")})
    }));
    blocks
}
fn setup(path: &Path, bot: bool, later: usize) -> (Store, i64, String, Vec<Value>, Value) {
    let store = Store::open_vorkspace(path).unwrap();
    let id = store.create("codex", "/synthetic-card-project").unwrap().id;
    let original = store.persisted_session_id(id).unwrap();
    if bot {
        store
            .conn
            .execute_batch(include_str!("../../../../tests/fixtures/bot-schema.sql"))
            .unwrap();
        store
            .conn
            .execute("INSERT INTO bot_transcripts VALUES(?1)", [&original])
            .unwrap();
    }
    store.shared_history_schema().unwrap();
    let blocks = source(later);
    store.shared_store_blocks(id, &blocks).unwrap();
    let history = store.load(id).unwrap();
    let expected = serde_json::from_str(
        &history
            .rows
            .iter()
            .find(|r| r.role == "bot-card")
            .unwrap()
            .text,
    )
    .unwrap();
    (store, id, original, blocks, expected)
}
fn title(expected: &Value, text: &str) -> Value {
    let mut next = expected.clone();
    next["title"] = json!(text);
    next
}

#[test]
fn card_save_reopen_preserves_rich_owner_later_blocks_receipts_and_references() {
    for bot in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cards.db");
        let (store, id, _, mut blocks, expected) = setup(&path, bot, 32);
        let desired = title(&expected, "New");
        let mut partial = expected.clone();
        partial.as_object_mut().unwrap().remove("future");
        assert!(store
            .refresh_bot_card(id, &partial, &title(&partial, "Partial"))
            .unwrap()
            .is_none());
        // A cold/partial UI is sufficient. No HistoryIndex or full rows are needed.
        store
            .shared
            .as_ref()
            .unwrap()
            .incremental
            .borrow_mut()
            .clear();
        store
            .shared
            .as_ref()
            .unwrap()
            .snapshots
            .borrow_mut()
            .clear();
        let row = store
            .refresh_bot_card(id, &expected, &desired)
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&row.text).unwrap(), desired);
        assert_eq!(row.sent_at, Some(123));
        drop(store);
        let reopened = Store::open_vorkspace(&path).unwrap();
        blocks[0]["botCard"]["title"] = json!("New");
        assert_eq!(reopened.shared_blocks(id).unwrap(), blocks);
        let history = reopened.load(id).unwrap();
        assert_eq!(
            history.rows.iter().filter(|r| r.role == "bot-card").count(),
            1
        );
    }
}

#[test]
fn card_cas_rejects_delayed_reply_deletion_replacement_and_reference_change() {
    for bot in [false, true] {
        let (store, id, original, _, expected) = setup(Path::new(":memory:"), bot, 2);
        let desired = title(&expected, "New");
        store
            .refresh_bot_card(id, &expected, &desired)
            .unwrap()
            .unwrap();
        let changes = store.conn.total_changes();
        assert!(store
            .refresh_bot_card(id, &expected, &desired)
            .unwrap()
            .is_some());
        assert_eq!(store.conn.total_changes(), changes);
        assert!(store
            .refresh_bot_card(id, &expected, &title(&expected, "Delayed"))
            .unwrap()
            .is_none());
        let mut invalid = desired.clone();
        invalid["sessionId"] = json!(123);
        assert!(store.refresh_bot_card(id, &desired, &invalid).is_err());
        let table = if bot { "bot_blocks" } else { "ordinary_blocks" };
        store.conn.execute(&format!("UPDATE {table} SET block_json=json_remove(block_json,'$.botCard') WHERE session_id=?1 AND block_id='card'"),[&original]).unwrap();
        assert!(store
            .refresh_bot_card(id, &desired, &expected)
            .unwrap()
            .is_none());
        store
            .conn
            .execute(
                &format!("DELETE FROM {table} WHERE session_id=?1 AND block_id='card'"),
                [&original],
            )
            .unwrap();
        assert!(store
            .refresh_bot_card(id, &desired, &expected)
            .unwrap()
            .is_none());
    }
}

#[test]
fn local_card_commit_keeps_warm_index_but_never_blesses_an_external_revision() {
    for bot in [false, true] {
        let (mut store, id, original, _, expected) = setup(Path::new(":memory:"), bot, 2);
        let mut transcript = store.load(id).unwrap();
        store.write_rows(id, transcript.rows.len(), &[]).unwrap();
        let desired = title(&expected, "New");
        store
            .refresh_bot_card(id, &expected, &desired)
            .unwrap()
            .unwrap();
        let index_revision = store.shared.as_ref().unwrap().incremental.borrow()[&id].revision;
        assert_eq!(index_revision, store.history_revision(&original).unwrap());
        // A provider loaded before the refresh appends only its new tail.
        let at = transcript.begin("assistant");
        transcript.append("Later provider append");
        store.write_rows(id, at, &transcript.rows[at..]).unwrap();
        let table = if bot { "bot_blocks" } else { "ordinary_blocks" };
        store.conn.execute(&format!("UPDATE {table} SET block_json=json_set(block_json,'$.external',1) WHERE session_id=?1 AND block_id='later-0'"),[&original]).unwrap();
        let next = title(&desired, "Newest");
        store
            .refresh_bot_card(id, &desired, &next)
            .unwrap()
            .unwrap();
        assert!(store.write_rows(id, at, &transcript.rows[at..]).is_err());
        assert_eq!(store.shared_blocks(id).unwrap()[1]["external"], 1);
        assert_eq!(
            store.shared_blocks(id).unwrap().last().unwrap()["text"],
            "Later provider append"
        );
    }
}

#[test]
fn cold_digest_allows_own_card_then_provider_tail_but_rejects_stale_card_rows_and_external_edits() {
    for bot in [false, true] {
        for external in [false, true] {
            let (mut store, id, original, _, expected) = setup(Path::new(":memory:"), bot, 2);
            // load() deliberately owns a digest, even with a stale/no index.
            let mut transcript = store.load(id).unwrap();
            store
                .shared
                .as_ref()
                .unwrap()
                .incremental
                .borrow_mut()
                .clear();
            let desired = title(&expected, "New");
            store
                .refresh_bot_card(id, &expected, &desired)
                .unwrap()
                .unwrap();
            let newest = title(&desired, "Newest");
            store
                .refresh_bot_card(id, &desired, &newest)
                .unwrap()
                .unwrap();
            assert!(store.write_rows(id, 0, &transcript.rows).is_err());
            if external {
                let table = if bot { "bot_blocks" } else { "ordinary_blocks" };
                store.conn.execute(&format!("UPDATE {table} SET block_json=json_set(block_json,'$.external',1) WHERE session_id=?1 AND block_id='later-0'"), [&original]).unwrap();
            }
            let at = transcript.begin("user");
            transcript.append("Synthetic follow up");
            let result =
                store.write_rows_with_message(id, at, &transcript.rows[at..], Some("queued"));
            assert_eq!(result.is_err(), external);
            assert_eq!(
                store.shared_blocks(id).unwrap()[0]["botCard"]["title"],
                "Newest"
            );
            if !external {
                assert_eq!(
                    store.shared_blocks(id).unwrap().last().unwrap()["appRequestId"],
                    "queued"
                );
            }
        }
    }
}

#[test]
fn local_card_guards_survive_tail_saves_and_reject_stale_full_history() {
    for warm in [true, false] {
        for bot in [false, true] {
            for append in [false, true] {
                let (mut store, id, _, _, expected) = setup(Path::new(":memory:"), bot, 2);
                let mut transcript = store.load(id).unwrap();
                if warm {
                    store.write_rows(id, transcript.rows.len(), &[]).unwrap();
                }
                let desired = title(&expected, "Refreshed");
                store
                    .refresh_bot_card(id, &expected, &desired)
                    .unwrap()
                    .unwrap();
                if append {
                    let at = transcript.begin("assistant");
                    transcript.append("Provider tail from before the card refresh");
                    store.write_rows(id, at, &transcript.rows[at..]).unwrap();
                }
                let before = store.shared_blocks(id).unwrap();
                assert!(
                        store.write_rows(id, 0, &transcript.rows).is_err(),
                        "An older transcript must not revert the card: warm={warm}, bot={bot}, append={append}"
                    );
                assert_eq!(store.shared_blocks(id).unwrap(), before);
            }
        }
    }
}

#[test]
fn failed_card_transaction_rolls_back_and_subsequent_refresh_can_retry() {
    let (store, id, _, before, expected) = setup(Path::new(":memory:"), true, 8);
    let desired = title(&expected, "New");
    store.conn.execute_batch("CREATE TRIGGER fail_card BEFORE UPDATE ON bot_blocks BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
    assert!(store.refresh_bot_card(id, &expected, &desired).is_err());
    assert_eq!(store.shared_blocks(id).unwrap(), before);
    store.conn.execute_batch("DROP TRIGGER fail_card").unwrap();
    assert!(store
        .refresh_bot_card(id, &expected, &desired)
        .unwrap()
        .is_some());
}

#[test]
fn early_card_work_is_constant_with_cold_and_warm_indices_and_unchanged_is_read_only() {
    for bot in [false, true] {
        let mut sizes = Vec::new();
        for later in [16, 2048] {
            let (mut store, id, original, _, expected) = setup(Path::new(":memory:"), bot, later);
            let history = store.load(id).unwrap();
            store.write_rows(id, history.rows.len(), &[]).unwrap();
            for warm in [true, false] {
                if !warm {
                    store
                        .shared
                        .as_ref()
                        .unwrap()
                        .incremental
                        .borrow_mut()
                        .clear();
                }
                let mut desired = expected.clone();
                // Restore baseline between measurements.
                let saved = store.shared_blocks(id).unwrap();
                let current: Value = serde_json::from_str(
                    &blocks::project(&saved)
                        .unwrap()
                        .into_iter()
                        .find(|r| r.role == "bot-card")
                        .unwrap()
                        .text,
                )
                .unwrap();
                desired["title"] = json!(if warm { "Warm" } else { "Cold" });
                HISTORY_READ_BYTES.with(|n| n.set(0));
                HISTORY_ENCODE_BYTES.with(|n| n.set(0));
                store
                    .refresh_bot_card(id, &current, &desired)
                    .unwrap()
                    .unwrap();
                let read = HISTORY_READ_BYTES.with(|n| n.get());
                let encoded = HISTORY_ENCODE_BYTES.with(|n| n.get());
                assert!(read < 1024, "Card read work must exclude the suffix");
                assert!(encoded < 1024, "Card encoding must exclude the suffix");
                sizes.push((read, encoded));
                let changes = store.conn.total_changes();
                store
                    .refresh_bot_card(id, &desired, &desired)
                    .unwrap()
                    .unwrap();
                assert_eq!(store.conn.total_changes(), changes);
            }
            let history = store.load(id).unwrap();
            store.write_rows(id, history.rows.len(), &[]).unwrap();
            let (at, mut baseline) = history
                .rows
                .iter()
                .enumerate()
                .find(|(_, row)| row.role == "bot-card")
                .map(|(at, row)| (at, row.clone()))
                .unwrap();
            let mut card: Value = serde_json::from_str(&baseline.text).unwrap();
            card["title"] = json!("Generic");
            baseline.text = card.to_string();
            HISTORY_READ_BYTES.with(|n| n.set(0));
            HISTORY_ENCODE_BYTES.with(|n| n.set(0));
            store.write_rows(id, at, &[baseline]).unwrap();
            let baseline_read = HISTORY_READ_BYTES.with(|n| n.get());
            let baseline_encoded = HISTORY_ENCODE_BYTES.with(|n| n.get());
            assert!(baseline_read > later * 2048);
            eprintln!("generic baseline bot={bot} later={later}: parsed={baseline_read} encoded={baseline_encoded} bytes");
            let table = if bot { "bot_blocks" } else { "ordinary_blocks" };
            let plan: String = store.conn.query_row(&format!("EXPLAIN QUERY PLAN SELECT block_json FROM {table} WHERE session_id=?1 AND block_id=?2"), params![original,"card"], |r| r.get(3)).unwrap();
            assert!(plan.contains("SEARCH") && !plan.contains("SCAN"));
        }
        assert_eq!(sizes[0], sizes[2]);
        assert_eq!(sizes[1], sizes[3]);
        eprintln!("card work bot={bot}: warm/cold parsed+encoded bytes {sizes:?}; later blocks 16 vs 2048");
    }
}
