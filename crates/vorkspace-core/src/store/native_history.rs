//! Atomic native-history publication: rows, dedup receipts and cursor advance
//! together. No composer, queue, attachment or provider-identity replacement.
use super::*;
use crate::agent::Settings;
use crate::native_history::{Batch, BatchUpdate, Binding, Cursor, Turn};
use rusqlite::{OptionalExtension, TransactionBehavior};
use std::collections::HashSet;

pub(super) fn ensure_receipt_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS provider_history_cursors(
        session_id TEXT NOT NULL,binding TEXT NOT NULL,cursor_json TEXT NOT NULL,
        PRIMARY KEY(session_id,binding));
        CREATE TABLE IF NOT EXISTS provider_history_receipts(
        session_id TEXT NOT NULL,binding TEXT NOT NULL,native_id TEXT NOT NULL,native_turn_id TEXT NOT NULL,imported INTEGER NOT NULL,
        PRIMARY KEY(session_id,binding,native_id));
        CREATE TABLE IF NOT EXISTS provider_history_recoveries(
        session_id TEXT NOT NULL,binding TEXT NOT NULL,cursor_hash TEXT NOT NULL,
        PRIMARY KEY(session_id,binding));
        CREATE INDEX IF NOT EXISTS provider_history_receipt_turn
        ON provider_history_receipts(session_id,binding,native_turn_id,imported);").map_err(|e|e.to_string())
}

pub(super) fn delete_if_present(conn: &rusqlite::Connection, id: &str) -> Result<(), String> {
    for table in [
        "provider_history_cursors",
        "provider_history_receipts",
        "provider_history_recoveries",
    ] {
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
    /// An unfinished log is evidence of uncertainty, not a live process.
    pub recovery: Option<String>,
}

#[derive(Default)]
struct NativePublication {
    imported: usize,
    reload_provider: bool,
    append: Option<shared::HistoryAppend>,
}

impl NativePublication {
    fn committed(self, store: &Store, id: i64) {
        if self.imported != 0 && store.shared.is_some() {
            store.shared_native_published(id, self.append);
        }
    }
}

impl Store {
    fn native_history_schema(&self) -> Result<(), String> {
        super::local_submissions::ensure_table(&self.conn).map_err(|e| e.to_string())?;
        ensure_receipt_schema(&self.conn)
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

    pub fn native_history_current(&self, request: &NativeHistoryRequest) -> Result<bool, String> {
        Ok(self.native_history_settings_if_current(request)?.is_some())
    }

    fn native_history_settings_if_current(
        &self,
        request: &NativeHistoryRequest,
    ) -> Result<Option<Settings>, String> {
        let id = request.session.id;
        let current = self.bot_session(id)?;
        let settings = if self.shared.is_some() {
            self.shared_agent_preferences(id)?
        } else {
            self.agent_settings(id)?
        }
        .unwrap_or_default();
        let active: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_submissions WHERE session_id=?1 AND phase IN ('reserved','dispatching'))",
            [self.persisted_session_id(id)?], |r| r.get(0),
        ).map_err(|e| e.to_string())?;
        let valid = !active
            && current.as_ref().is_some_and(|s| {
                s.provider == request.binding.provider
                    && s.provider_id.as_deref() == Some(&request.binding.thread)
                    && s.cwd == request.session.cwd
                    && settings.account.id == request.binding.account
            })
            && self.native_cursor(id, &request.binding)? == request.expected;
        Ok(valid.then_some(settings))
    }

    pub fn native_history_recovery_token(request: &NativeHistoryRequest) -> Option<String> {
        request.expected.as_ref().map(|cursor| {
            // An older acknowledgement may have released admission without
            // publishing the interrupted exchange.
            crate::bot::files::content_hash(&format!(
                "publish-v1:{}:{cursor}",
                request.binding.key()
            ))
        })
    }

    pub fn native_history_acknowledged(
        &self,
        request: &NativeHistoryRequest,
        token: &str,
    ) -> Result<bool, String> {
        self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM provider_history_recoveries WHERE session_id=?1 AND binding=?2 AND cursor_hash=?3)",
            params![self.persisted_session_id(request.session.id)?, request.binding.key(), token],
            |row| row.get(0),
        ).map_err(|e| e.to_string())
    }

    /// The user confirmed the external CLI has stopped. Save the interrupted
    /// exchange before releasing admission for this exact snapshot.
    pub fn acknowledge_native_history(
        &self,
        request: &NativeHistoryRequest,
        expected: &str,
    ) -> Result<(), String> {
        let id = request.session.id;
        if self.shared.is_some() {
            self.shared_history_schema()?;
        }
        let tx = rusqlite::Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let changed = "The native conversation changed. Refresh before recovering it.";
        let settings = self
            .native_history_settings_if_current(request)?
            .ok_or(changed)?;
        let turn = request
            .cursor
            .as_ref()
            .filter(|_| Self::native_history_recovery_token(request).as_deref() == Some(expected))
            .and_then(|cursor| cursor.pending_for_recovery(&request.binding.provider))
            .ok_or(changed)?;
        let publication = self.stage_native_turns(request, settings, vec![turn], true)?;
        self.conn.execute(
            "INSERT INTO provider_history_recoveries(session_id,binding,cursor_hash) VALUES(?1,?2,?3) ON CONFLICT(session_id,binding) DO UPDATE SET cursor_hash=excluded.cursor_hash",
            params![self.persisted_session_id(id)?, request.binding.key(), expected],
        ).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        publication.committed(self, id);
        Ok(())
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
        request: &mut NativeHistoryRequest,
        batch: Batch,
    ) -> Result<NativeHistoryResult, String> {
        let Batch { active, update } = batch;
        let Some(BatchUpdate {
            cursor,
            turns,
            more,
        }) = update
        else {
            return Ok(NativeHistoryResult {
                active,
                ..Default::default()
            });
        };
        let id = request.session.id;
        if self.shared.is_some() {
            self.shared_history_schema()?;
        }
        let tx = rusqlite::Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let Some(settings) = self.native_history_settings_if_current(request)? else {
            return Ok(NativeHistoryResult {
                superseded: true,
                ..Default::default()
            });
        };
        let publication = self.stage_native_turns(request, settings, turns, false)?;
        let reload_provider = publication.reload_provider
            || (cursor.compaction.is_some()
                && cursor.compaction.as_ref()
                    != request
                        .cursor
                        .as_ref()
                        .and_then(|prior| prior.compaction.as_ref()));
        if let Some(context) = cursor.context {
            // This transaction already validated the exact native binding and
            // excluded a managed writer. Never preserve usage from before an
            // externally recorded compaction.
            let mut context = context;
            if context.window.is_none() && context.used > 0 {
                context.window = self.context_usage(id)?.and_then(|usage| usage.window);
            }
            self.save_context_usage(id, &context)?;
        }
        let published_cursor = serde_json::to_string(&cursor).map_err(|e| e.to_string())?;
        self.conn.execute("INSERT INTO provider_history_cursors(session_id,binding,cursor_json) VALUES(?1,?2,?3) ON CONFLICT(session_id,binding) DO UPDATE SET cursor_json=excluded.cursor_json",
            params![self.persisted_session_id(id)?,request.binding.key(),published_cursor]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        // Admission must check this commit, even when a separate connection
        // publishes another cursor before the caller reserves its operation.
        request.expected = Some(published_cursor);
        request.cursor = Some(cursor);
        let imported = publication.imported;
        publication.committed(self, id);
        Ok(NativeHistoryResult {
            imported,
            reload_provider,
            active,
            more,
            superseded: false,
            recovery: None,
        })
    }

    /// Stage receipt and transcript writes in the caller's transaction.
    fn stage_native_turns(
        &self,
        request: &NativeHistoryRequest,
        settings: Settings,
        turns: Vec<Turn>,
        recovering: bool,
    ) -> Result<NativePublication, String> {
        if turns.is_empty() {
            return Ok(NativePublication::default());
        }
        let id = request.session.id;
        let source = self.persisted_session_id(id)?;
        let key = request.binding.key();
        let mut unseen = Vec::new();
        let mut reload_provider = false;
        {
            let owners = if self.shared.is_some() {
                self.shared_native_turn_owners(id, &request.binding, &turns, &settings)?
            } else {
                settings.turns.into_values().collect()
            };
            let known: HashSet<_> = owners
                .iter()
                .filter_map(|t| t.provider_turn_id.clone())
                .collect();
            let legacy = owners
                .into_iter()
                .filter(|t| t.provider_turn_id.is_none() && request.binding.provider == "claude")
                .collect::<Vec<_>>();
            let mut ownership = self.conn.prepare(
                "SELECT imported FROM provider_history_receipts WHERE session_id=?1 AND binding=?2 AND native_turn_id=?3 ORDER BY imported DESC LIMIT 1",
            ).map_err(|e|e.to_string())?;
            let mut receipt = self.conn.prepare(
                "INSERT INTO provider_history_receipts(session_id,binding,native_id,native_turn_id,imported)
                 VALUES(?1,?2,?3,?4,?5) ON CONFLICT(session_id,binding,native_id) DO NOTHING",
            ).map_err(|e|e.to_string())?;
            let mut seen_receipt = self.conn.prepare(
                "SELECT 1 FROM provider_history_receipts WHERE session_id=?1 AND binding=?2 AND native_id=?3 AND native_turn_id=?4",
            ).map_err(|e|e.to_string())?;
            for mut turn in turns {
                let prior: Option<bool> = ownership
                    .query_row(params![source, key, turn.id], |r| r.get(0))
                    .optional()
                    .map_err(|e| e.to_string())?;
                let imported = prior.unwrap_or_else(|| !Self::native_owned(&turn, &known, &legacy));
                let claude = request.binding.provider == "claude";
                if !claude {
                    if recovering {
                        // A truncated log can expose a turn whose completion
                        // has already been saved.
                        if prior.is_some()
                            && seen_receipt
                                .exists(params![source, key, turn.id, turn.id])
                                .map_err(|e| e.to_string())?
                        {
                            continue;
                        }
                    } else {
                        let inserted = receipt
                            .execute(params![source, key, turn.id, turn.id, imported])
                            .map_err(|e| e.to_string())?
                            != 0;
                        if !inserted || !imported {
                            continue;
                        }
                        reload_provider |= prior.is_none();
                    }
                }
                // Existing Codex receipts without a completed-turn receipt
                // came from a recovery. Publish only the unseen messages.
                if claude || recovering || prior.is_some() {
                    if !claude {
                        turn.assign_codex_message_receipts();
                    }
                    // Backfill receipts from the prefix represented by a leaf
                    // receipt written by the earlier importer. This also
                    // works for leaves with no visible text, such as a tool result.
                    let mut legacy_prefix = 0;
                    if claude && prior.is_some() {
                        for record in turn.claude_records.iter().rev() {
                            if seen_receipt
                                .exists(params![source, key, record.uuid, turn.id])
                                .map_err(|e| e.to_string())?
                            {
                                legacy_prefix = record.messages;
                                break;
                            }
                        }
                    }
                    let messages = std::mem::take(&mut turn.messages);
                    for (index, message) in messages.into_iter().enumerate() {
                        let native_id = message
                            .receipt()
                            .ok_or("Native message has no receipt identity")?;
                        let inserted = receipt
                            .execute(params![source, key, native_id, turn.id, imported])
                            .map_err(|e| e.to_string())?
                            != 0;
                        let legacy_published = claude
                            && (index < legacy_prefix
                                || (legacy_prefix == 0
                                    && prior == Some(true)
                                    && request.cursor.as_ref().is_some_and(|cursor| {
                                        cursor.legacy_claude_published(&turn.id, &message)
                                    })));
                        if inserted && imported && !legacy_published {
                            turn.messages.push(message);
                        }
                    }
                    reload_provider |= !turn.messages.is_empty();
                }
                if !turn.messages.is_empty() {
                    unseen.push(turn);
                }
            }
        }
        let append = if unseen.is_empty() {
            None
        } else if self.shared.is_some() {
            self.shared_append_native(id, &request.binding, &unseen)?
        } else {
            self.append_native_rows(id, &request.binding, &unseen)?;
            None
        };
        Ok(NativePublication {
            imported: unseen.len(),
            reload_provider,
            append,
        })
    }
    fn append_native_rows(&self, id: i64, binding: &Binding, turns: &[Turn]) -> Result<(), String> {
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
