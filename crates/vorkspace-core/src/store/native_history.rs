//! Atomic native-history publication: rows, dedup receipts and cursor advance
//! together. No composer, queue, attachment or provider-identity replacement.
use super::*;
use crate::native_history::{Batch, Binding, Cursor, Turn};
use rusqlite::{OptionalExtension, TransactionBehavior};
use std::collections::HashSet;

pub(super) fn delete_if_present(conn: &rusqlite::Connection, id: &str) -> Result<(), String> {
    for table in ["provider_history_cursors", "provider_history_receipts"] {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if exists {
            conn.execute(&format!("DELETE FROM {table} WHERE session_id=?1"), [id])
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub struct NativeHistoryRequest {
    pub session: Session,
    pub binding: Binding,
    pub cursor: Option<Cursor>,
    expected: Option<String>,
}
#[derive(Default, Debug)]
pub struct NativeHistoryResult {
    pub imported: usize,
    pub reload_provider: bool,
    pub active: bool,
    pub more: bool,
    pub superseded: bool,
}
impl Store {
    fn native_history_schema(&self) -> Result<(), String> {
        super::local_submissions::ensure_table(&self.conn).map_err(|e| e.to_string())?;
        self.conn.execute_batch("CREATE TABLE IF NOT EXISTS provider_history_cursors(
            session_id TEXT NOT NULL,binding TEXT NOT NULL,cursor_json TEXT NOT NULL,
            PRIMARY KEY(session_id,binding));
            CREATE TABLE IF NOT EXISTS provider_history_receipts(
            session_id TEXT NOT NULL,binding TEXT NOT NULL,native_id TEXT NOT NULL,native_turn_id TEXT NOT NULL,imported INTEGER NOT NULL,
            PRIMARY KEY(session_id,binding,native_id));").map_err(|e|e.to_string())
    }
    fn native_cursor(&self, id: i64, binding: &Binding) -> Result<Option<String>, String> {
        self.conn.query_row("SELECT cursor_json FROM provider_history_cursors WHERE session_id=?1 AND binding=?2",
            params![self.persisted_session_id(id)?,binding.key()],|r|r.get(0)).optional().map_err(|e|e.to_string())
    }
    pub fn native_history_request(
        &self,
        id: i64,
        root: &Path,
    ) -> Result<Option<NativeHistoryRequest>, String> {
        let Some(session) = self.bot_session(id)? else {
            return Ok(None);
        };
        let Some(thread) = session.provider_id.as_deref() else {
            return Ok(None);
        };
        let mut settings = if self.shared.is_some() {
            self.shared_agent_preferences(id)?
        } else {
            self.agent_settings(id)?
        }
        .unwrap_or_default();
        settings.account.root = self.data_dir().map(Path::to_path_buf);
        let Some(binding) = Binding::for_bot(&session.provider, thread, &settings.account, root)?
        else {
            return Ok(None);
        };
        self.native_history_schema()?;
        let expected = self.native_cursor(id, &binding)?;
        let cursor = expected
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| format!("Could not read native sync cursor: {e}"))?;
        Ok(Some(NativeHistoryRequest {
            session,
            binding,
            cursor,
            expected,
        }))
    }

    /// A native turn already recorded by the managed runtime is not an import.
    /// Legacy Claude receipts lack UUIDs; their recorded execution intervals are
    /// used only for those old managed turns, never as a general history cutoff.
    fn native_owned(
        turn: &Turn,
        known: &HashSet<String>,
        legacy: &[crate::agent::TurnInfo],
    ) -> bool {
        known.contains(&turn.id)
            || legacy.iter().any(|info| {
                turn.started_at
                    .zip(info.started_at)
                    .zip(info.completed_at)
                    .is_some_and(|((at, start), end)| at >= start && at <= end)
            })
    }

    pub fn import_native_history(
        &mut self,
        request: &NativeHistoryRequest,
        batch: Batch,
    ) -> Result<NativeHistoryResult, String> {
        let id = request.session.id;
        if self.shared.is_some() {
            self.shared_history_schema()?;
        }
        let tx = rusqlite::Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let current = self.bot_session(id)?;
        let settings = if self.shared.is_some() {
            self.shared_agent_preferences(id)?
        } else {
            self.agent_settings(id)?
        }
        .unwrap_or_default();
        let valid = current.as_ref().is_some_and(|s| {
            s.provider == request.binding.provider
                && s.provider_id.as_deref() == Some(&request.binding.thread)
                && s.cwd == request.session.cwd
                && settings.account.id == request.binding.account
        });
        let source = self.persisted_session_id(id)?;
        let active:bool=self.conn.query_row("SELECT EXISTS(SELECT 1 FROM local_submissions WHERE session_id=?1 AND phase IN ('reserved','dispatching'))",[&source],|r|r.get(0)).map_err(|e|e.to_string())?;
        if !valid || active || self.native_cursor(id, &request.binding)? != request.expected {
            return Ok(NativeHistoryResult {
                superseded: true,
                ..Default::default()
            });
        }
        let key = request.binding.key();
        let mut unseen = Vec::new();
        let mut reload_provider = batch.cursor.compaction.is_some()
            && batch.cursor.compaction
                != request
                    .cursor
                    .as_ref()
                    .and_then(|cursor| cursor.compaction.clone());
        if !batch.turns.is_empty() {
            let settings = self.agent_settings(id)?.unwrap_or_default();
            let mut known: HashSet<_> = settings
                .turns
                .values()
                .filter_map(|t| t.provider_turn_id.clone())
                .collect();
            let mut statement=self.conn.prepare("SELECT DISTINCT native_turn_id FROM provider_history_receipts WHERE session_id=?1 AND binding=?2 AND imported=1").map_err(|e|e.to_string())?;
            for id in statement
                .query_map(params![source, key], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?
            {
                known.remove(&id.map_err(|e| e.to_string())?);
            }
            let legacy = settings
                .turns
                .values()
                .filter(|t| t.provider_turn_id.is_none() && request.binding.provider == "claude")
                .cloned()
                .collect::<Vec<_>>();
            for turn in &batch.turns {
                let seen:bool=self.conn.query_row("SELECT EXISTS(SELECT 1 FROM provider_history_receipts WHERE session_id=?1 AND binding=?2 AND native_id=?3)",params![source,key,turn.receipt()],|r|r.get(0)).map_err(|e|e.to_string())?;
                let imported = !Self::native_owned(turn, &known, &legacy);
                reload_provider |= !seen && imported;
                if !seen && imported && !turn.messages.is_empty() {
                    unseen.push(turn);
                }
                self.conn.execute("INSERT OR IGNORE INTO provider_history_receipts(session_id,binding,native_id,native_turn_id,imported) VALUES(?1,?2,?3,?4,?5)",params![source,key,turn.receipt(),turn.id,imported]).map_err(|e|e.to_string())?;
            }
        }
        if !unseen.is_empty() {
            if self.shared.is_some() {
                self.shared_append_native(id, &request.binding, &unseen)?;
            } else {
                self.append_native_rows(id, &request.binding, &unseen)?;
            }
        }
        if let Some(context) = batch.cursor.context {
            // This transaction already validated the exact native binding and
            // excluded a managed writer. Never preserve usage from before an
            // externally recorded compaction.
            let mut context = context;
            if context.window.is_none() && context.used > 0 {
                context.window = self.context_usage(id)?.and_then(|usage| usage.window);
            }
            self.save_context_usage(id, &context)?;
        }
        self.conn.execute("INSERT INTO provider_history_cursors(session_id,binding,cursor_json) VALUES(?1,?2,?3) ON CONFLICT(session_id,binding) DO UPDATE SET cursor_json=excluded.cursor_json",
            params![source,key,serde_json::to_string(&batch.cursor).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        if !unseen.is_empty() && self.shared.is_some() {
            self.shared_native_published(id);
        }
        Ok(NativeHistoryResult {
            imported: unseen.len(),
            reload_provider,
            active: batch.active,
            more: batch.more,
            superseded: false,
        })
    }
    fn append_native_rows(
        &self,
        id: i64,
        binding: &Binding,
        turns: &[&Turn],
    ) -> Result<(), String> {
        let mut at = self
            .conn
            .query_row(
                "SELECT coalesce(max(row_ix)+1,0) FROM native_rows WHERE session_id=?1",
                [id],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| e.to_string())? as usize;
        let mut settings = self.agent_settings(id)?.unwrap_or_default();
        for turn in turns {
            let rows = turn.transcript().rows;
            let key = rows.iter().rposition(Row::is_prompt).map(|p| at + p + 1);
            for row in rows {
                self.conn.execute("INSERT INTO native_rows(session_id,row_ix,role,body,first,sent_at) VALUES(?1,?2,?3,?4,?5,?6)",params![id,at as i64,row.role,row.text,row.first,row.sent_at.and_then(|at|i64::try_from(at).ok())]).map_err(|e|e.to_string())?;
                at += 1;
            }
            if let Some(key) = key {
                settings.turns.insert(key, turn_info(binding, turn));
            }
        }
        self.save_agent_settings(id, &settings)?;
        self.conn
            .execute(
                "UPDATE native_sessions SET updated_at=?2 WHERE id=?1",
                params![id, crate::time::now_ms() as i64],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
pub(super) fn turn_info(binding: &Binding, turn: &Turn) -> crate::agent::TurnInfo {
    crate::agent::TurnInfo {
        provider_turn_id: Some(turn.id.clone()),
        provider: binding.provider.clone(),
        model_id: turn.model.clone(),
        started_at: turn.started_at,
        completed_at: turn.completed_at,
        duration_ms: turn
            .completed_at
            .zip(turn.started_at)
            .map_or(0, |(end, start)| end.saturating_sub(start)),
        duration_known: turn.completed_at.is_some(),
        ..Default::default()
    }
}
