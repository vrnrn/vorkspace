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
    /// The scalar revision read with these exact rows and turn boundaries.
    pub revision: i64,
    /// Turn receipts keyed by global projected-row offset in this snapshot.
    pub turns: HashMap<usize, crate::agent::TurnInfo>,
    /// Position of the first row in the complete projected transcript.
    pub global_start: usize,
    pub before: Option<IndexedHistoryCursor>,
}

impl Store {
    /// Read a snapshot-bound page of indexed Bot history. Blob and ordinary
    /// histories return None and retain their existing loading path.
    /// Writes go through `write_rows`, which requires a complete transcript
    /// and a separate revision guard.
    pub fn indexed_history_page(
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
        let Some(tables) = self.shared_bot_tables(original)? else {
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
                    let projected = blocks::project_block(&block)?;
                    record_projected_turn(
                        &block,
                        &projected,
                        global_start + rows.len(),
                        &mut turns,
                    );
                    rows.extend(projected);
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
        Ok(Some(IndexedHistoryPage {
            rows,
            revision: self.history_revision(original)?,
            turns,
            global_start,
            before,
        }))
    }
}

#[cfg(test)]
mod tests;
