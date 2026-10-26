use super::*;

// A cold digest cannot be advanced using a scalar revision that did not guard
// its read. Remember only our changed owning blocks until the next load/save;
// generic validation can rewind these exact local changes before hashing. Any
// external change, including opaque fields, must still fail the original guard.
pub(super) struct CardRevision {
    base: HistoryRevision,
    changes: HashMap<String, (Value, Value)>,
}

impl Store {
    pub(in crate::store) fn shared_refresh_bot_card(
        &self,
        session: i64,
        id: &str,
        expected: &Value,
        desired: &Value,
    ) -> Result<Option<Row>, String> {
        self.shared_writable()?;
        self.shared_history_schema()?;
        let original = self.persisted_session_id(session)?;
        let tx = sql(self.conn.unchecked_transaction())?;
        let Some(kind) = self.shared_history_index_kind(&original)? else {
            // Do not normalize or merge an unloaded legacy blob for a cosmetic
            // refresh. A normal transcript save can establish its owning rows.
            return Err("Card refresh requires normalized history".into());
        };
        let (table, position) = match kind {
            incremental::Kind::Bot => (self.shared_bot_tables(&original)?.unwrap().blocks, "seq"),
            incremental::Kind::Ordinary => ("ordinary_blocks".to_owned(), "position"),
        };
        let saved: Option<(i64, String)> = sql(self
            .conn
            .query_row(
                &format!(
                    "SELECT {position},block_json FROM {table} WHERE session_id=?1 AND block_id=?2"
                ),
                params![original, id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional())?;
        let Some((key, raw)) = saved else {
            return Ok(None);
        };
        let mut block = parse_block(&raw)?;
        if block["id"].as_str() != Some(id) || !block["botCard"].is_object() {
            return Ok(None);
        }
        // Projection maps external session IDs to handles and injects the block
        // ID. Compare that representation, but never persist it over rich truth.
        let Some(mut row) = blocks::project(std::slice::from_ref(&block))?
            .into_iter()
            .find(|row| row.role == "bot-card")
        else {
            return Ok(None);
        };
        let current: Value = parse(&row.text)?;
        if current == *desired {
            return Ok(Some(row));
        }
        if current != *expected {
            return Ok(None);
        }
        let before = self.history_revision(&original)?;
        let previous = block.clone();
        let fields = block["botCard"].as_object_mut().unwrap();
        // Merge only changed fields. Keep opaque metadata and external IDs in
        // their original form; no prose, tool payload or receipt is reconstructed.
        for (name, value) in desired.as_object().unwrap() {
            if name != "id" && expected.get(name) != Some(value) {
                fields.insert(name.clone(), value.clone());
            }
        }
        for name in expected.as_object().unwrap().keys() {
            if name != "id" && !desired.as_object().unwrap().contains_key(name) {
                fields.remove(name);
            }
        }
        sql(self.conn.execute(
            &format!("UPDATE {table} SET block_json=?3 WHERE session_id=?1 AND {position}=?2 AND block_id=?4"),
            params![original,key,encode(&block)?,id],
        ))?;
        if kind == incremental::Kind::Ordinary {
            sql(self.conn.execute("UPDATE ordinary_transcripts SET revision=CASE WHEN revision>=9007199254740990 THEN 0 ELSE revision+1 END WHERE session_id=?1",[&original]))?;
        }
        let revision = self.history_revision(&original)?;
        sql(tx.commit())?;
        // Only advance a guard that covered the exact pre-commit history. If an
        // external writer changed anything, a stale generic writer must remain
        // refused until explicit reload. A card-local CAS cannot bless its tail.
        let shared = self.shared.as_ref().unwrap();
        let mut snapshots = shared.snapshots.borrow_mut();
        let mut indices = shared.incremental.borrow_mut();
        let mut local = shared.card_revisions.borrow_mut();
        match snapshots.get(&session).copied() {
            Some(HistoryRevision::Stored(guard)) if guard == before => {
                snapshots.insert(session, HistoryRevision::Stored(revision));
                if let Some(index) = indices.get_mut(&session) {
                    if index.revision == before {
                        index.revision = revision;
                    } else {
                        indices.remove(&session);
                    }
                }
                local.remove(&session);
            }
            Some(base @ HistoryRevision::Digest(_)) => {
                indices.remove(&session);
                let ledger = local.entry(session).or_insert_with(|| CardRevision {
                    base,
                    changes: HashMap::new(),
                });
                if ledger.base != base {
                    *ledger = CardRevision {
                        base,
                        changes: HashMap::new(),
                    };
                }
                let change = ledger
                    .changes
                    .entry(id.to_owned())
                    .or_insert_with(|| (previous.clone(), previous.clone()));
                if change.1 == previous {
                    change.1 = block;
                } else {
                    // A revision outside this owner intervened on the same
                    // block. Its richer contents survive, but cannot bless the
                    // old transcript's generic write guard.
                    snapshots.insert(session, HistoryRevision::Invalidated);
                    local.remove(&session);
                }
            }
            _ => {
                snapshots.insert(session, HistoryRevision::Invalidated);
                indices.remove(&session);
                local.remove(&session);
            }
        }
        // A generic writer can still hold the old projected card after its
        // tail revision advances. Retain only a fixed-size card-content guard
        // until authoritative history reload, independently of suffix guards.
        shared
            .refreshed_cards
            .borrow_mut()
            .entry(session)
            .or_default()
            .insert(
                id.to_owned(),
                HistoryRevision::of(std::slice::from_ref(desired)),
            );
        row.text = desired.to_string();
        Ok(Some(row))
    }
    pub(super) fn shared_card_rows_current(
        &self,
        session: i64,
        rows: &[Row],
    ) -> Result<(), String> {
        let refreshed = self.shared.as_ref().unwrap().refreshed_cards.borrow();
        let Some(cards) = refreshed.get(&session) else {
            return Ok(());
        };
        for row in rows.iter().filter(|row| row.role == "bot-card") {
            let card: Value = parse(&row.text)?;
            if let Some(current) = card["id"].as_str().and_then(|id| cards.get(id)) {
                if *current != HistoryRevision::of(std::slice::from_ref(&card)) {
                    return Err(
                        "Card changed while this transcript was saving; reload before retrying"
                            .into(),
                    );
                }
            }
        }
        Ok(())
    }
    pub(super) fn shared_card_history_matches(
        &self,
        session: i64,
        expected: &HistoryRevision,
        blocks: &[Value],
    ) -> Option<bool> {
        let local = self.shared.as_ref().unwrap().card_revisions.borrow();
        let ledger = local
            .get(&session)
            .filter(|ledger| &ledger.base == expected)?;
        let mut hash = HistoryRevision::start(blocks.len());
        let mut found = HashSet::new();
        for block in blocks {
            if let Some((id, (before, after))) = block["id"]
                .as_str()
                .and_then(|id| ledger.changes.get(id).map(|change| (id, change)))
            {
                if block != after || !found.insert(id) {
                    return Some(false);
                }
                HistoryRevision::push(&mut hash, before);
            } else {
                HistoryRevision::push(&mut hash, block);
            }
        }
        Some(
            found.len() == ledger.changes.len()
                && *expected == HistoryRevision::Digest(hash.finalize().into()),
        )
    }
}

#[cfg(test)]
mod tests {
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
                .execute_batch(include_str!("../../../tests/fixtures/bot-schema.sql"))
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
    fn cold_digest_allows_own_card_then_provider_tail_but_rejects_stale_card_rows_and_external_edits(
    ) {
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
                let (mut store, id, original, _, expected) =
                    setup(Path::new(":memory:"), bot, later);
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
}
