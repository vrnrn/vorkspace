//! Snapshot-bound indexed Bot pages and window-relative writes.
use super::*;

#[derive(Clone, Debug)]
pub struct IndexedHistoryCursor {
    session: String,
    blocks: String,
    turns: String,
    revision: HistoryRevision,
    position_revision: [u8; 32],
    turn_revision: [u8; 32],
    before: i64,
}

#[derive(Debug)]
pub struct IndexedHistoryPage {
    pub rows: Vec<Row>,
    /// Turn receipts keyed by global projected-row offset in this snapshot.
    pub turns: HashMap<usize, crate::agent::TurnInfo>,
    /// Position of the first row in the complete projected transcript.
    pub global_start: usize,
    pub before: Option<IndexedHistoryCursor>,
    guard: IndexedHistoryCursor,
    saved_global_start: usize,
    saved_len: usize,
    tail: bool,
}

impl Store {
    pub(in crate::store) fn shared_indexed_history_page(
        &self,
        id: i64,
        cursor: Option<&IndexedHistoryCursor>,
        page_turns: usize,
    ) -> Result<Option<IndexedHistoryPage>, String> {
        if !(1..=1000).contains(&page_turns) {
            return Err("History page size must be between 1 and 1000 turns".into());
        }
        let original = self.persisted_session_id(id)?;
        // Owner discovery, boundaries, revision and projection must observe
        // the same SQLite snapshot, including external desktop writers.
        let tx = sql(self.conn.unchecked_transaction())?;
        let result = self.shared_indexed_history_page_in_snapshot(&original, cursor, page_turns)?;
        sql(tx.commit())?;
        Ok(result)
    }

    fn shared_indexed_history_page_in_snapshot(
        &self,
        original: &str,
        cursor: Option<&IndexedHistoryCursor>,
        page_turns: usize,
    ) -> Result<Option<IndexedHistoryPage>, String> {
        let Some(tables) = self.shared_bot_tables(&original)? else {
            if cursor.is_some() {
                return Err("Indexed history owner changed; reload the conversation".into());
            }
            return Ok(None);
        };
        if let Some(cursor) = cursor {
            if cursor.session != original
                || cursor.blocks != tables.blocks
                || cursor.turns != tables.turns
            {
                return Err("Indexed history owner changed; reload the conversation".into());
            }
        }
        let mut turn_hash = Sha256::new();
        let mut boundaries = Vec::new();
        {
            let mut statement = sql(self.conn.prepare(&format!(
                "SELECT t.seq, EXISTS(SELECT 1 FROM {blocks} b WHERE b.session_id=t.session_id AND b.seq=t.seq) FROM {turns} t WHERE t.session_id=?1 ORDER BY t.seq DESC",
                blocks = tables.blocks,
                turns = tables.turns,
            )))?;
            let mut records = sql(statement.query([&original]))?;
            while let Some(row) = sql(records.next())? {
                let seq: i64 = sql(row.get(0))?;
                if !sql(row.get::<_, bool>(1))? {
                    return Err("Indexed history changed or has an invalid turn boundary; reload the conversation".into());
                }
                turn_hash.update(seq.to_le_bytes());
                if cursor.is_none_or(|c| seq < c.before) && boundaries.len() <= page_turns {
                    boundaries.push(seq);
                }
            }
        }
        let turn_revision: [u8; 32] = turn_hash.finalize().into();
        let start = (boundaries.len() > page_turns).then(|| boundaries[page_turns - 1]);
        let mut rows = Vec::new();
        let mut turns = HashMap::new();
        let mut global_start = 0usize;
        let mut older = false;
        let mut count = 0usize;
        let mut positions = Sha256::new();
        let revision;
        {
            let mut statement = sql(self.conn.prepare(&format!(
                "SELECT seq,block_json,(SELECT count(*) FROM {0} WHERE session_id=?1),block_id FROM {0} WHERE session_id=?1 ORDER BY seq", tables.blocks
            )))?;
            let mut records = sql(statement.query([&original]))?;
            let mut hash = HistoryRevision::start(0);
            while let Some(row) = sql(records.next())? {
                if count == 0 {
                    hash = HistoryRevision::start(sql(row.get::<_, i64>(2))? as usize);
                }
                count += 1;
                let seq: i64 = sql(row.get(0))?;
                let block_id: String = sql(row.get(3))?;
                positions.update(seq.to_le_bytes());
                positions.update((block_id.len() as u64).to_le_bytes());
                positions.update(block_id.as_bytes());
                let block = parse_block(&sql(row.get::<_, String>(1))?)?;
                HistoryRevision::push(&mut hash, &block);
                if start.is_some_and(|start| seq < start) {
                    older = true;
                    global_start += blocks::project(std::slice::from_ref(&block))?.len();
                } else if cursor.is_none_or(|cursor| seq < cursor.before) {
                    project_turn(&block, global_start + rows.len(), &mut turns)?;
                    rows.extend(blocks::project(std::slice::from_ref(&block))?);
                }
            }
            revision = HistoryRevision::finish(hash, count);
        }
        let position_revision: [u8; 32] = positions.finalize().into();
        if let Some(cursor) = cursor {
            if revision != cursor.revision
                || turn_revision != cursor.turn_revision
                || position_revision != cursor.position_revision
            {
                return Err(
                    "Indexed history changed in another window; reload the conversation".into(),
                );
            }
        }
        let guard = IndexedHistoryCursor {
            session: original.to_owned(),
            blocks: tables.blocks,
            turns: tables.turns,
            revision,
            position_revision,
            turn_revision,
            before: i64::MAX,
        };
        let before = if older {
            start.map(|before| IndexedHistoryCursor {
                before,
                ..guard.clone()
            })
        } else {
            None
        };
        let saved_len = rows.len();
        Ok(Some(IndexedHistoryPage {
            rows,
            turns,
            global_start,
            before,
            guard,
            saved_global_start: global_start,
            saved_len,
            tail: cursor.is_none(),
        }))
    }

    pub(in crate::store) fn shared_write_indexed_history_page(
        &self,
        id: i64,
        page: &IndexedHistoryPage,
        start: usize,
        rows: &[Row],
    ) -> Result<(), String> {
        if start > page.saved_len || (!page.tail && rows.len() > page.saved_len - start) {
            return Err("Write exceeds the loaded history page; reload before editing".into());
        }
        self.shared_writable()?;
        self.shared_history_schema()?;
        self.shared_card_rows_current(id, rows)?;
        let original = self.persisted_session_id(id)?;
        let tx = sql(self.conn.unchecked_transaction())?;
        // Revalidate the owner, all block positions and content, and turns
        // under the same transaction as the merge and publication.
        self.shared_indexed_history_page_in_snapshot(&original, Some(&page.guard), 1)?
            .ok_or("Indexed history owner changed; reload the conversation")?;
        let current = self.shared_blocks(id)?;
        let global_start = page
            .saved_global_start
            .checked_add(start)
            .ok_or("History row offset overflow")?;
        let next = blocks::merge(&current, global_start, rows)?;
        self.shared_store_blocks(id, &next)?;
        if !rows.is_empty() {
            self.shared_patch(id, &[("updated_at", json!(crate::time::now_ms()))])?;
        }
        sql(tx.commit())?;
        // Existing complete-history readers must reload after this independent
        // write rather than inheriting a revision they never observed.
        let shared = self.shared.as_ref().unwrap();
        shared
            .snapshots
            .borrow_mut()
            .insert(id, HistoryRevision::Invalidated);
        shared.incremental.borrow_mut().remove(&id);
        shared.card_revisions.borrow_mut().remove(&id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicBool, Arc};

    #[test]
    fn pages_are_ordered_and_snapshot_bound_across_edits_and_retries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
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
        let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
        let id = store.create("codex", "/project").unwrap().id;
        assert!(store.indexed_history_page(id, None, 10).unwrap().is_none());
        assert!(store.indexed_history_page(id, None, 0).is_err());
    }

    #[test]
    fn page_edits_and_appends_preserve_unloaded_blocks_and_reject_stale_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
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
        let blocks: Vec<_> = (0..50)
            .map(|i| {
                json!({"id": format!("{i}"),
            "role": if i % 2 == 0 { "user" } else { "assistant" },
            "text": format!("message {i}"), "future": {"ordinal": i}})
            })
            .collect();
        store.shared_store_blocks(id, &blocks).unwrap();
        let complete = store.load(id).unwrap();
        let page = store.indexed_history_page(id, None, 10).unwrap().unwrap();
        assert_eq!(page.global_start, 30);
        let mut edited = page.rows[1].clone();
        edited.text = "edited in page".into();
        store
            .write_indexed_history_page(id, &page, 1, &[edited])
            .unwrap();
        assert!(store.write_rows(id, 0, &complete.rows[..1]).is_err());
        assert!(store
            .write_indexed_history_page(id, &page, 0, &[])
            .unwrap_err()
            .contains("changed"));
        let page = store.indexed_history_page(id, None, 10).unwrap().unwrap();
        let older = store
            .indexed_history_page(id, page.before.as_ref(), 10)
            .unwrap()
            .unwrap();
        let new_row = Row {
            role: "user".into(),
            text: "new tail".into(),
            first: true,
            sent_at: None,
        };
        assert!(store
            .write_indexed_history_page(
                id,
                &older,
                older.rows.len(),
                std::slice::from_ref(&new_row)
            )
            .is_err());
        let other = Connection::open(&path).unwrap();
        other.execute("UPDATE bot_blocks SET block_json=json_set(block_json,'$.future.external',true) WHERE session_id=?1 AND seq=1", [&original]).unwrap();
        assert!(store
            .write_indexed_history_page(id, &page, page.rows.len(), std::slice::from_ref(&new_row))
            .unwrap_err()
            .contains("changed"));
        let page = store.indexed_history_page(id, None, 10).unwrap().unwrap();
        store
            .write_indexed_history_page(id, &page, page.rows.len(), &[new_row])
            .unwrap();
        drop(store);
        let store = Store::open_vorkspace(&path).unwrap();
        let id = store.resolve_session_id(&original).unwrap();
        let saved = store.shared_blocks(id).unwrap();
        assert_eq!(saved[0]["future"]["external"], 1);
        assert_eq!(&saved[1..30], &blocks[1..30]);
        assert_eq!(saved[31]["text"], "edited in page");
        assert_eq!(saved[31]["future"]["ordinal"], 31);
        assert_eq!(saved[50]["text"], "new tail");
    }
}
