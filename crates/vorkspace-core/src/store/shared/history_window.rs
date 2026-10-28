//! Snapshot-bound, read-only indexed Bot pages. These pages do not install a
//! write revision: existing mutation APIs require a complete transcript.
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
    /// Position of the first row in the complete projected transcript.
    pub global_start: usize,
    pub before: Option<IndexedHistoryCursor>,
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
        let before = if older {
            start.map(|before| IndexedHistoryCursor {
                session: original,
                blocks: tables.blocks,
                turns: tables.turns,
                revision,
                position_revision,
                turn_revision,
                before,
            })
        } else {
            None
        };
        sql(tx.commit())?;
        Ok(Some(IndexedHistoryPage {
            rows,
            global_start,
            before,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicBool};

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
                    "text": format!("needle {i}"), "future": {"keep": i}
                })
            })
            .collect();
        store.shared_store_blocks(id, &blocks).unwrap();
        let full = blocks::project(&blocks).unwrap();
        let newest = store.indexed_history_page(id, None, 10).unwrap().unwrap();
        assert_eq!(newest.global_start, 30);
        assert_eq!(newest.rows, full[30..]);
        let cursor = newest.before.unwrap();
        let middle = store
            .indexed_history_page(id, Some(&cursor), 10)
            .unwrap()
            .unwrap();
        assert_eq!(middle.global_start, 10);
        assert_eq!(middle.rows, full[10..30]);
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
        assert!(
            store
                .indexed_history_page(id, Some(&cursor), 10)
                .unwrap_err()
                .contains("changed")
        );
        let retry = store.indexed_history_page(id, None, 10).unwrap().unwrap();
        assert!(
            store
                .indexed_history_page(id, retry.before.as_ref(), 10)
                .unwrap()
                .is_some()
        );

        other
            .execute(
                "DELETE FROM bot_turns WHERE session_id=?1 AND seq=1",
                [&original],
            )
            .unwrap();
        assert!(
            store
                .indexed_history_page(id, retry.before.as_ref(), 10)
                .unwrap_err()
                .contains("changed")
        );
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
        assert!(
            store
                .indexed_history_page(id, retry.before.as_ref(), 10)
                .unwrap_err()
                .contains("changed")
        );
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
        assert!(
            store
                .indexed_history_page(id, retry.before.as_ref(), 10)
                .unwrap_err()
                .contains("changed")
        );
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
}
