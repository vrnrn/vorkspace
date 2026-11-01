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
            Some(base @ HistoryRevision::Digest { .. }) => {
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
                && *expected == HistoryRevision::finish(hash, blocks.len()),
        )
    }
}

#[cfg(test)]
mod tests;
