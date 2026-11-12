//! The normalized source owns history. Scalar revisions are changed by SQLite
//! triggers, including writers outside this process; the in-memory index stores
//! only projection boundaries, never rich source payloads. Publish it after commit.
use super::*;

const CHANGED: &str =
    "This conversation changed in another window. Reopen it before saving; its data was preserved.";

/// Prepare every lazy queue/Bot table before a scalar history guard can go live.
/// Writers also call this inside their own transactions for databases that have
/// not saved history yet. Once a guard exists, bookkeeping must never add DDL.
pub(super) fn ensure_bookkeeping_schema(conn: &Connection) -> Result<(), String> {
    super::agent::ensure_schema(conn)?;
    sql(conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS native_bot_dispatch_claims(
            bot_id TEXT NOT NULL,message_id TEXT NOT NULL,token TEXT NOT NULL,
            PRIMARY KEY(bot_id,message_id));
         CREATE TABLE IF NOT EXISTS native_bot_draft_origins(
            key TEXT PRIMARY KEY,value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS native_follow_up_metadata(
            session_id TEXT PRIMARY KEY,state_json TEXT NOT NULL);",
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Ordinary,
    Bot,
}
impl Kind {
    fn position(self) -> &'static str {
        match self {
            Self::Ordinary => "position",
            Self::Bot => "seq",
        }
    }
    fn user_flag(self) -> &'static str {
        match self {
            Self::Ordinary => "has_user",
            Self::Bot => "is_user",
        }
    }
}
#[derive(Clone, Debug)]
pub(super) struct Entry {
    pub key: i64,
    pub offset: usize,
    pub rows: usize,
    pub handoff: bool,
    pub digest: HistoryRevision,
}
#[derive(Clone, Debug)]
pub(super) struct HistoryIndex {
    pub revision: i64,
    pub schema: i64,
    pub kind: Kind,
    pub entries: Vec<Entry>,
    pub rows: usize,
}

pub(in crate::store) struct HistoryAppend {
    revision: i64,
    entries: Vec<Entry>,
    rows: usize,
}

fn bot_turn_boundary(block: &Value, first: bool, after_handoff: bool) -> bool {
    first
        || after_handoff
        || (block["role"] == "user" && block["internal"] != true)
        || block.get("botHabit").is_some()
}

impl HistoryIndex {
    pub(super) fn patch_card(&mut self, key: i64, previous: &Value, next: &Value) -> bool {
        let Ok(at) = self.entries.binary_search_by_key(&key, |entry| entry.key) else {
            return false;
        };
        let entry = &mut self.entries[at];
        if !entry.digest.matches(std::slice::from_ref(previous)) {
            return false;
        }
        entry.digest = HistoryRevision::of(std::slice::from_ref(next));
        true
    }

    fn matches_prefix<'a>(&self, blocks: &mut impl Iterator<Item = &'a Value>) -> bool {
        self.entries.iter().all(|entry| {
            blocks
                .next()
                .is_some_and(|block| entry.digest.matches(std::slice::from_ref(block)))
        })
    }

    fn covers(&self, blocks: &[Value]) -> bool {
        self.entries.len() == blocks.len() && self.matches_prefix(&mut blocks.iter())
    }

    fn fast_forwards(&self, blocks: &[Value]) -> bool {
        if self.entries.is_empty() || self.entries.len() >= blocks.len() {
            return false;
        }
        if self.matches_prefix(&mut blocks.iter()) {
            return true;
        }
        let mut sent = blocks.iter().filter(|block| block["draft"] != true);
        self.matches_prefix(&mut sent) && sent.next().is_some()
    }
}

impl Store {
    /// Save context only for its exact provider, account and thread owner.
    /// The predicate and mutation share one SQL statement and avoid reading
    /// transcript payloads, historical receipts or unrelated sessions.
    pub fn save_context_usage_if_owner(
        &self,
        id: i64,
        usage: &crate::context::Usage,
        provider: &str,
        account: &str,
        expected_provider_id: Option<&str>,
    ) -> Result<bool, String> {
        self.shared_writable()?;
        let fields = ["context_used", "context_window"]
            .into_iter()
            .enumerate()
            .filter(|(_, name)| self.shared.columns.contains(*name))
            .map(|(index, name)| format!(",{name}=?{}", index + 2))
            .collect::<String>();
        let account_column = self.shared_field("provider_account_id", "NULL");
        let changed = sql(self.conn.execute(
            &format!(
                "UPDATE sessions SET title=title{fields}
                WHERE id=?1 AND harness=?4 AND COALESCE({account_column},?6)=?5
                  AND provider_session_id IS ?7"
            ),
            params![
                self.persisted_session_id(id)?,
                usage.used.min(i64::MAX as u64) as i64,
                usage.window.map(|n| n.min(i64::MAX as u64) as i64),
                provider,
                account,
                crate::accounts::DEFAULT,
                expected_provider_id
            ],
        ))?;
        Ok(changed != 0)
    }

    pub(in crate::store) fn history_revision(&self, original: &str) -> Result<i64, String> {
        if !has_table(&self.conn, "session_history_revisions")? {
            return Ok(0);
        }
        Ok(sql(self
            .conn
            .query_row(
                "SELECT revision FROM session_history_revisions WHERE session_id=?1",
                [original],
                |row| row.get(0),
            )
            .optional())?
        .unwrap_or(0))
    }

    pub(super) fn shared_history_matches(
        &self,
        id: i64,
        expected: &HistoryRevision,
        blocks: &[Value],
    ) -> Result<bool, String> {
        match expected {
            HistoryRevision::Digest { .. } => Ok(self
                .shared_card_history_matches(id, expected, blocks)
                .unwrap_or_else(|| expected.matches(blocks))),
            HistoryRevision::Invalidated => Ok(false),
            HistoryRevision::Stored(revision) => {
                let shared = &self.shared;
                let indices = shared.incremental.borrow();
                let Some(index) = indices.get(&id).filter(|index| index.revision == *revision)
                else {
                    return Ok(false);
                };
                if self.shared_schema_version()? == index.schema {
                    return Ok(self.history_revision(&self.persisted_session_id(id)?)? == *revision);
                }
                // A database-wide schema cookie only qualifies the fast path.
                // Verify the complete rich history before accepting old offsets.
                Ok(index.covers(blocks))
            }
        }
    }

    pub(super) fn shared_schema_version(&self) -> Result<i64, String> {
        sql(self
            .conn
            .query_row("PRAGMA schema_version", [], |row| row.get(0)))
    }

    pub(in crate::store) fn shared_turn_version(
        &self,
        id: i64,
    ) -> Result<Option<(i64, i64)>, String> {
        let schema = self.shared_schema_version()?;
        if self.shared.history_schema.get() != Some(schema) {
            return Ok(None);
        }
        Ok(Some((
            schema,
            self.history_revision(&self.persisted_session_id(id)?)?,
        )))
    }

    fn shared_history_fast_forwards(
        &self,
        id: i64,
        expected: &HistoryRevision,
        blocks: &[Value],
    ) -> bool {
        match expected {
            HistoryRevision::Stored(revision) => self
                .shared
                .incremental
                .borrow()
                .get(&id)
                .is_some_and(|index| index.revision == *revision && index.fast_forwards(blocks)),
            _ => expected.fast_forwards(blocks),
        }
    }

    pub(super) fn shared_validate_history(&self, id: i64, blocks: &[Value]) -> Result<(), String> {
        if let Some(expected) = self.shared.snapshots.borrow().get(&id) {
            if !self.shared_history_matches(id, expected, blocks)? {
                return Err(CHANGED.into());
            }
        }
        Ok(())
    }

    pub(super) fn shared_validate_history_or_fast_forward(
        &self,
        id: i64,
        blocks: &[Value],
    ) -> Result<bool, String> {
        if let Some(expected) = self.shared.snapshots.borrow().get(&id) {
            if !self.shared_history_matches(id, expected, blocks)? {
                if !self.shared_history_fast_forwards(id, expected, blocks) {
                    return Err(CHANGED.into());
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(in crate::store) fn shared_has_user_message(&self) -> Result<String, String> {
        if self.shared.columns.contains("has_user_message") {
            if let Some(tables) = &self.shared.earlier_bot_tables {
                let current = self.current_bot_owner_predicate("sessions.id")?;
                return Ok(format!(
                    "CASE WHEN EXISTS(SELECT 1 FROM {roots} WHERE session_id=sessions.id) AND NOT({current}) THEN EXISTS(SELECT 1 FROM {blocks} WHERE session_id=sessions.id AND is_user=1) ELSE has_user_message END",
                    roots = tables.transcripts,
                    blocks = tables.blocks
                ));
            }
            return Ok("has_user_message".into());
        }
        let mut source = String::from("CASE ");
        if has_table(&self.conn, "bot_transcripts")? {
            source.push_str(&format!("WHEN {} THEN EXISTS(SELECT 1 FROM bot_blocks WHERE session_id=sessions.id AND is_user=1) ",self.current_bot_owner_predicate("sessions.id")?));
        }
        if let Some(tables) = &self.shared.earlier_bot_tables {
            source.push_str(&format!("WHEN EXISTS(SELECT 1 FROM {roots} WHERE session_id=sessions.id) THEN EXISTS(SELECT 1 FROM {blocks} WHERE session_id=sessions.id AND is_user=1) ",roots=tables.transcripts,blocks=tables.blocks));
        }
        if has_table(&self.conn, "bot_transcripts")? {
            source.push_str("WHEN EXISTS(SELECT 1 FROM bot_transcripts WHERE session_id=sessions.id) THEN EXISTS(SELECT 1 FROM bot_blocks WHERE session_id=sessions.id AND is_user=1) ");
        }
        if has_table(&self.conn, "ordinary_transcripts")? {
            source.push_str("WHEN EXISTS(SELECT 1 FROM ordinary_transcripts WHERE session_id=sessions.id AND normalized=1) THEN EXISTS(SELECT 1 FROM ordinary_blocks WHERE session_id=sessions.id AND has_user=1) ");
        }
        if source == "CASE " {
            return Ok("blocks_json!='[]'".into());
        }
        source.push_str("ELSE blocks_json!='[]' END");
        Ok(source)
    }

    /// Opening, listing and reading an old database remain read-only. The first
    /// explicit history save or app mutation adds storage and mutation guards
    /// inside one coordinated transaction. Existing normalized rows and their identities stay put.
    pub(in crate::store) fn shared_history_schema(&self) -> Result<(), String> {
        let schema = self.shared_schema_version()?;
        if self.shared.history_schema.get() == Some(schema) {
            return Ok(());
        }
        let tx = sql(rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        ))?;
        super::super::app_commands::ensure(&tx)?;
        // A failed child draft can record recovery while its caller streams.
        // These local tables must exist before scalar history guards are live.
        super::super::composer_draft::ensure_draft_schema(&tx)?;
        super::super::composer_draft::ensure_recovery_schema(&tx)?;
        ensure_bookkeeping_schema(&tx)?;
        super::super::turn_history::ensure_recovery_schema(&tx)?;
        // All normal writers of this database must finish their lazy schema
        // work before a whole-database schema version guards active history.
        sql(super::super::local_submissions::ensure_table(&tx))?;
        super::super::native_history::ensure_receipt_schema(&tx)?;
        sql(super::super::notes::ensure_notes_table(&tx))?;
        sql(super::super::todos::ensure_schema(&tx))?;
        // Prepare automation DDL only; its service still owns run/claim backfill.
        sql(crate::automations::schema::ensure_schema(&tx))?;
        sql(tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS session_history_revisions(
                session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                revision INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS ordinary_transcripts(
                session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                revision INTEGER NOT NULL DEFAULT 0, normalized INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE IF NOT EXISTS ordinary_blocks(
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                block_id TEXT NOT NULL, position INTEGER NOT NULL, block_json TEXT NOT NULL,
                has_user INTEGER NOT NULL, is_draft INTEGER NOT NULL,
                PRIMARY KEY(session_id,block_id));
             CREATE INDEX IF NOT EXISTS ordinary_blocks_order ON ordinary_blocks(session_id,position);
             CREATE INDEX IF NOT EXISTS ordinary_blocks_flags ON ordinary_blocks(session_id,has_user,is_draft);"
        ))?;
        let mut guarded = vec![
            (
                "ordinary_blocks".to_owned(),
                "session_id",
                "OLD.block_json IS NOT NEW.block_json OR OLD.position IS NOT NEW.position OR OLD.block_id IS NOT NEW.block_id",
            ),
            (
                "ordinary_transcripts".to_owned(),
                "session_id",
                "OLD.normalized IS NOT NEW.normalized",
            ),
            (
                "bot_blocks".to_owned(),
                "session_id",
                "OLD.block_json IS NOT NEW.block_json OR OLD.seq IS NOT NEW.seq OR OLD.block_id IS NOT NEW.block_id",
            ),
            ("bot_transcripts".to_owned(), "session_id", "1"),
        ];
        if let Some(tables) = &self.shared.earlier_bot_tables {
            guarded.push((tables.blocks.clone(),"session_id","OLD.block_json IS NOT NEW.block_json OR OLD.seq IS NOT NEW.seq OR OLD.block_id IS NOT NEW.block_id"));
            guarded.push((tables.transcripts.clone(), "session_id", "1"));
        }
        for (table, key, changed) in guarded {
            if !has_table(&self.conn, &table)? {
                continue;
            }
            sql(tx.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS history_{table}_owner BEFORE UPDATE OF {key} ON {table}
                WHEN OLD.{key} IS NOT NEW.{key} BEGIN SELECT RAISE(ABORT,'History ownership cannot be changed'); END;")))?;
            for (event, owner, when) in [
                ("INSERT", "NEW", "1"),
                ("UPDATE", "NEW", changed),
                ("DELETE", "OLD", "1"),
            ] {
                sql(tx.execute_batch(&format!(
                    "CREATE TRIGGER IF NOT EXISTS history_{table}_{event} AFTER {event} ON {table}
                     WHEN {when} BEGIN
                        INSERT INTO session_history_revisions SELECT {owner}.{key},1 WHERE EXISTS(SELECT 1 FROM sessions WHERE id={owner}.{key})
                        ON CONFLICT(session_id) DO UPDATE SET revision=revision+1;
                     END;"
                )))?;
            }
        }
        // Read the cookie while the transaction still owns its DDL.
        let schema = self.shared_schema_version()?;
        sql(tx.commit())?;
        self.shared.history_schema.set(Some(schema));
        Ok(())
    }

    pub(super) fn shared_normalize_blocks(
        &self,
        original: &str,
        blocks: &[Value],
    ) -> Result<(), String> {
        // INSERT and clearing the old blob occur in the caller's transaction;
        // the normalized marker is published only after every rich block exists.
        self.shared_store_ordinary_blocks(original, blocks)?;
        sql(self.conn.execute(
            "UPDATE sessions SET blocks_json='[]' WHERE id=?1",
            [original],
        ))?;
        sql(self.conn.execute(
            "INSERT INTO ordinary_transcripts(session_id,revision,normalized) VALUES(?1,1,1)
             ON CONFLICT(session_id) DO UPDATE SET normalized=1,revision=revision+1",
            [original],
        ))?;
        Ok(())
    }

    pub(super) fn shared_history_index_kind(&self, original: &str) -> Result<Option<Kind>, String> {
        if !has_table(&self.conn, "session_history_revisions")? {
            return Ok(None);
        }
        if self.shared_bot_tables(original)?.is_some() {
            Ok(Some(Kind::Bot))
        } else if self.shared_ordinary_indexed(original)? {
            Ok(Some(Kind::Ordinary))
        } else {
            Ok(None)
        }
    }

    pub(super) fn shared_build_history_index(
        &self,
        id: i64,
        blocks: &[Value],
    ) -> Result<Option<HistoryIndex>, String> {
        let original = self.persisted_session_id(id)?;
        let Some(kind) = self.shared_history_index_kind(&original)? else {
            return Ok(None);
        };
        let schema = self.shared_schema_version()?;
        let keys = sql(sql(self.conn.prepare(&format!(
            "SELECT {} FROM {} WHERE session_id=?1 ORDER BY {}",
            kind.position(),
            self.shared_history_table(kind, &original)?,
            kind.position()
        )))?
        .query_map([&original], |row| row.get::<_, i64>(0)))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
        if keys.len() != blocks.len() {
            return Err(CHANGED.into());
        }
        if keys.windows(2).any(|keys| keys[0] >= keys[1]) {
            return Err("Stored conversation positions are not unique and increasing; its data was preserved".into());
        }
        if self.shared.history_schema.get() != Some(schema) {
            // DDL raced setup. Publish only the canonical digest for this write.
            return Ok(None);
        }
        let mut rows = 0;
        let mut entries = Vec::with_capacity(blocks.len());
        for (key, block) in keys.into_iter().zip(blocks) {
            let len = blocks::project(std::slice::from_ref(block))?.len();
            entries.push(Entry {
                key,
                offset: rows,
                rows: len,
                handoff: block["role"] == "handoff",
                digest: HistoryRevision::of(std::slice::from_ref(block)),
            });
            rows += len;
        }
        let revision = self.history_revision(&original)?;
        Ok(Some(HistoryIndex {
            revision,
            schema,
            kind,
            entries,
            rows,
        }))
    }

    pub(super) fn shared_write_incremental(
        &self,
        id: i64,
        start: usize,
        rows: &[Row],
        writer: Option<&mut TurnHistory>,
    ) -> Result<bool, String> {
        let indices = self.shared.incremental.borrow();
        let Some(index) = indices.get(&id) else {
            return Ok(false);
        };
        // A full-history owner (edits, drafts or turn metadata) replaces the
        // digest on commit. Its next delta rebuilds the compact index once.
        if self.shared.snapshots.borrow().get(&id) != Some(&HistoryRevision::Stored(index.revision))
        {
            return Ok(false);
        }
        if writer
            .as_ref()
            .is_some_and(|writer| writer.version != Some((index.schema, index.revision)))
        {
            return Ok(false);
        }
        let tx = sql(rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        ))?;
        let original = self.persisted_session_id(id)?;
        if self.shared_schema_version()? != index.schema {
            // The full path compares the saved rich blocks within its own
            // write transaction, then builds a fresh index on success.
            return Ok(false);
        }
        if self.history_revision(&original)? != index.revision {
            return if writer.is_some() {
                Ok(false)
            } else {
                Err(CHANGED.into())
            };
        }
        self.shared_card_rows_current(id, rows)?;
        if start > index.rows {
            return Err(
                "Transcript changed; refusing to overwrite a different conversation revision"
                    .into(),
            );
        }
        // Include the last visible source block for continuation rows and
        // attachment manifests. All older complete blocks remain untouched.
        let from = index
            .entries
            .iter()
            .rposition(|entry| entry.rows > 0 && entry.offset <= start)
            .unwrap_or(0);
        let offset = index.entries.get(from).map_or(0, |entry| entry.offset);
        let key = index.entries.get(from).map_or(0, |entry| entry.key);
        let mut query = sql(self.conn.prepare(&format!(
            "SELECT block_json FROM {} WHERE session_id=?1 AND {}>=?2 ORDER BY {}",
            self.shared_history_table(index.kind, &original)?,
            index.kind.position(),
            index.kind.position()
        )))?;
        let previous = sql(query.query_map(params![original, key], |row| row.get::<_, String>(0)))?
            .map(|raw| parse_block(&sql(raw)?))
            .collect::<Result<Vec<Value>, String>>()?;
        drop(query);
        let next = blocks::merge(&previous, start - offset, rows)?;
        let table = self.shared_history_table(index.kind, &original)?;
        let mut entries = Vec::with_capacity(next.len());
        let mut writer_rows = Vec::new();
        let mut row_offset = offset;
        let mut next_key = index
            .entries
            .last()
            .map_or(if index.kind == Kind::Bot { 1 } else { 0 }, |entry| {
                entry.key + 1
            });
        let mut after_handoff = from
            .checked_sub(1)
            .is_some_and(|ix| index.entries[ix].handoff);
        for (local, block) in next.iter().enumerate() {
            let position = index.entries.get(from + local).map_or_else(
                || {
                    let key = next_key;
                    next_key += 1;
                    key
                },
                |entry| entry.key,
            );
            let changed = previous.get(local) != Some(block);
            if changed {
                self.shared_write_indexed_block(
                    &original, index.kind, &table, position, block, true,
                )?;
            }
            if index.kind == Kind::Bot && bot_turn_boundary(block, from + local == 0, after_handoff)
            {
                let tables = self.shared_bot_tables(&original)?.ok_or(CHANGED)?;
                self.shared_insert_bot_turn(&tables.turns, &original, position)?;
            }
            let projected = blocks::project(std::slice::from_ref(block))?;
            let len = projected.len();
            if writer.is_some() {
                writer_rows.extend(TurnHistory::fingerprints(&projected));
            }
            entries.push(Entry {
                key: position,
                offset: row_offset,
                rows: len,
                handoff: block["role"] == "handoff",
                digest: if changed {
                    HistoryRevision::of(std::slice::from_ref(block))
                } else {
                    index.entries[from + local].digest
                },
            });
            row_offset += len;
            after_handoff = block["role"] == "handoff";
        }
        // merge() preserves original blocks; all mutation/removal owners use the
        // full guarded path. Advancing the public desktop revision is atomic too.
        self.shared_update_indexed_flags(id, &original, index.kind, &table)?;
        if !rows.is_empty() {
            self.shared_patch(id, &[("updated_at", json!(crate::time::now_ms()))])?;
        }
        let revision = self.history_revision(&original)?;
        sql(tx.commit())?;
        if let Some(writer) = writer {
            writer.committed(Some((index.schema, revision)), offset, writer_rows);
        }
        drop(indices);
        let mut indices = self.shared.incremental.borrow_mut();
        let index = indices.get_mut(&id).unwrap();
        index.entries.truncate(from);
        index.entries.extend(entries);
        index.rows = row_offset;
        index.revision = revision;
        self.shared
            .snapshots
            .borrow_mut()
            .insert(id, HistoryRevision::Stored(revision));
        Ok(true)
    }

    /// Append under the caller's transaction. Only the tail position, new
    /// blocks, and their turn boundaries are needed for an indexed owner.
    pub(super) fn shared_append_indexed_blocks(
        &self,
        id: i64,
        original: &str,
        kind: Kind,
        blocks: &[Value],
    ) -> Result<Option<HistoryAppend>, String> {
        let table = self.shared_history_table(kind, original)?;
        let position = kind.position();
        let check_handoff = kind == Kind::Bot
            && blocks
                .first()
                .is_some_and(|block| !bot_turn_boundary(block, false, false));
        let tail: Option<(i64, bool)> = sql(self.conn.query_row(
            &format!(
                "SELECT {position}, CASE WHEN ?2 THEN json_extract(block_json,'$.role') IS 'handoff' ELSE 0 END
                 FROM {table} WHERE session_id=?1 ORDER BY {position} DESC LIMIT 1"
            ),
            params![original, check_handoff],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional())?;
        let base_revision = self.history_revision(original)?;
        let schema = self.shared_schema_version()?;
        let indices = self.shared.incremental.borrow();
        let mut append = indices
            .get(&id)
            .filter(|index| {
                index.kind == kind && index.revision == base_revision && index.schema == schema
            })
            .map(|index| HistoryAppend {
                revision: base_revision,
                entries: Vec::with_capacity(blocks.len()),
                rows: index.rows,
            });
        let bot_turns = if kind == Kind::Bot {
            Some(self.shared_bot_tables(original)?.ok_or(CHANGED)?.turns)
        } else {
            None
        };
        let (mut key, mut after_handoff) =
            tail.unwrap_or((if kind == Kind::Bot { 0 } else { -1 }, false));
        for (index, block) in blocks.iter().enumerate() {
            key = key
                .checked_add(1)
                .ok_or("History block position overflow")?;
            self.shared_write_indexed_block(original, kind, &table, key, block, false)?;
            if let Some(turns) = &bot_turns {
                if bot_turn_boundary(block, index == 0 && tail.is_none(), after_handoff) {
                    self.shared_insert_bot_turn(turns, original, key)?;
                }
            }
            after_handoff = block["role"] == "handoff";
            if let Some(append) = &mut append {
                let rows = blocks::project(std::slice::from_ref(block))?.len();
                append.entries.push(Entry {
                    key,
                    offset: append.rows,
                    rows,
                    handoff: after_handoff,
                    digest: HistoryRevision::of(std::slice::from_ref(block)),
                });
                append.rows += rows;
            }
        }
        self.shared_update_indexed_flags(id, original, kind, &table)?;
        if let Some(append) = &mut append {
            append.revision = self.history_revision(original)?;
        }
        Ok(append)
    }

    pub(super) fn shared_publish_indexed_append(&self, id: i64, append: Option<HistoryAppend>) {
        let mut indices = self.shared.incremental.borrow_mut();
        if let Some(append) = append {
            let index = indices.get_mut(&id).unwrap();
            index.entries.extend(append.entries);
            index.rows = append.rows;
            index.revision = append.revision;
        } else {
            indices.remove(&id);
        }
    }

    fn shared_insert_bot_turn(&self, table: &str, original: &str, key: i64) -> Result<(), String> {
        sql(self.conn.execute(
            &format!("INSERT OR IGNORE INTO {table}(session_id,seq) VALUES(?1,?2)"),
            params![original, key],
        ))?;
        Ok(())
    }

    fn shared_update_indexed_flags(
        &self,
        id: i64,
        original: &str,
        kind: Kind,
        table: &str,
    ) -> Result<(), String> {
        if kind == Kind::Ordinary {
            sql(self.conn.execute("UPDATE ordinary_transcripts SET revision=CASE WHEN revision>=9007199254740990 THEN 0 ELSE revision+1 END WHERE session_id=?1",[original]))?;
        }
        let user = kind.user_flag();
        let (user, draft): (bool, bool) = sql(self.conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE session_id=?1 AND {user}=1),
                             EXISTS(SELECT 1 FROM {table} WHERE session_id=?1 AND {user}=1 AND is_draft=1)"),
            [original],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ))?;
        self.shared_patch(
            id,
            &[
                ("has_user_message", json!(user)),
                ("is_draft", json!(draft)),
            ],
        )
    }

    fn shared_history_table(&self, kind: Kind, original: &str) -> Result<String, String> {
        match kind {
            Kind::Ordinary => Ok("ordinary_blocks".into()),
            Kind::Bot => Ok(self.shared_bot_tables(original)?.ok_or(CHANGED)?.blocks),
        }
    }

    fn shared_write_indexed_block(
        &self,
        original: &str,
        kind: Kind,
        table: &str,
        position: i64,
        block: &Value,
        replace: bool,
    ) -> Result<(), String> {
        let id = block["id"]
            .as_str()
            .ok_or("Indexed conversation contains a missing block ID")?;
        let raw = encode(block)?;
        match kind {
            Kind::Ordinary => {
                let conflict = if replace {
                    "ON CONFLICT(session_id,block_id) DO UPDATE SET position=excluded.position,block_json=excluded.block_json,has_user=excluded.has_user,is_draft=excluded.is_draft"
                } else {
                    ""
                };
                sql(self.conn.execute(&format!("INSERT INTO {table}(session_id,block_id,position,block_json,has_user,is_draft) VALUES(?1,?2,?3,?4,?5,?6) {conflict}"),
                    params![original,id,position,raw,block["role"] == "user",block["role"] == "user" && block["draft"] == true]))?;
            }
            Kind::Bot => {
                let search = [
                    block["text"].as_str(),
                    block["tool"]["title"].as_str(),
                    block["tool"]["detail"].as_str(),
                    block["image"]["name"].as_str(),
                    block["image"]["alt"].as_str(),
                    block["tool"]["preview"]["path"].as_str(),
                    block["tool"]["preview"]["query"].as_str(),
                    block["tool"]["preview"]["output"].as_str(),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("\n")
                .to_lowercase();
                let conflict = if replace {
                    "ON CONFLICT(session_id,seq) DO UPDATE SET block_json=excluded.block_json,search_text=excluded.search_text,is_user=excluded.is_user,is_draft=excluded.is_draft"
                } else {
                    ""
                };
                sql(self.conn.execute(&format!("INSERT INTO {table}(session_id,seq,block_id,block_json,search_text,is_user,is_draft) VALUES(?1,?2,?3,?4,?5,?6,?7) {conflict}"),
                    params![original,position,id,raw,search,block["role"] == "user",block["draft"] == true]))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
