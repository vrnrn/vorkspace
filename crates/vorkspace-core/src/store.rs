//! Session storage. Desktop launches use vorkspace's existing schema directly.
mod app_commands;
pub use app_commands::AppCommandAdmission;
mod attachments;
pub(crate) mod automations_schema;
mod bot_desktop;
pub(crate) mod bot_state;
pub use bot_state::{describe_worktrees, remove_unused_worktree};
mod bots;
pub mod inbox_preferences;
pub use bot_state::root as bot_root;
mod card;
mod composer_draft;
mod follow_up;
pub use composer_draft::{ComposerDraft, ComposerRecovery};
mod local_submissions;
pub use local_submissions::{
    SubmissionClaim, SubmissionInput, SubmissionOwner, SubmissionReceipt, SubmissionReservation,
};
mod managed_sessions;
mod notes;
mod peer_history;
mod remote;
mod remote_legacy;
mod vorkspace_import;
pub use remote::{RemoteProject, RemoteSession, RemoteUpload};
pub(crate) use shared::handle as desktop_session_handle;
mod session_metadata;
pub use session_metadata::{SessionMetadata, SessionSidebarMetadata};
mod session_preview;
pub use session_preview::SessionPreviewData;
mod edit;
mod shared;
pub use shared::{IndexedHistoryCursor, IndexedHistoryPage};
mod side_threads;
pub(crate) mod todos;
pub use crate::session::native_model;
use crate::transcript::{Row, Transcript};
pub use edit::EditSnapshot;
use rusqlite::{params, Connection};
pub use side_threads::DesktopSideThread;
use std::path::Path;
pub use vorkspace_import::{vorkspace_database, ImportReport};

pub use crate::session::Session;

/// A session's pending or delivered reminder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reminder {
    pub session_id: i64,
    /// Unix milliseconds.
    pub due_at: u64,
    pub fired_at: Option<u64>,
}

pub struct Store {
    conn: Connection,
    path: std::path::PathBuf,
    shared: Option<shared::Shared>,
    project_deletions: std::collections::HashSet<String>,
}
fn validate_bot_claim(raw: &str, bot: &str, message: &crate::bot::Message) -> Result<(), String> {
    let roster: crate::bot::Roster =
        serde_json::from_str(raw).map_err(|e| format!("Cannot verify Bot outbox: {e}"))?;
    let queued = roster
        .get(bot)
        .and_then(|bot| bot.outbox.iter().find(|queued| queued.id == message.id));
    if queued
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| e.to_string())?
        != Some(serde_json::to_value(message).map_err(|e| e.to_string())?)
        || message.error.is_some()
    {
        return Err("Bot outbox changed in another window; reopen it before sending.".into());
    }
    Ok(())
}

impl Store {
    /// Runtime admission guard held until the project deletion publishes its result.
    pub fn begin_project_deletion(&mut self, path: &str) -> Result<(), String> {
        let key = crate::files::path_key(path);
        if !self.project_deletions.insert(key) {
            return Err("Project deletion is already in progress.".into());
        }
        Ok(())
    }
    pub fn end_project_deletion(&mut self, path: &str) {
        self.project_deletions.remove(&crate::files::path_key(path));
    }
    fn check_project_creation(&self, path: &str) -> Result<(), String> {
        let key = crate::files::path_key(path);
        if self.project_deletions.iter().any(|root| {
            std::path::Path::new(&key).starts_with(std::path::Path::new(root))
                || match (std::fs::canonicalize(&key), std::fs::canonicalize(root)) {
                    (Ok(path), Ok(root)) => path.starts_with(root),
                    _ => false,
                }
        }) {
            return Err("This project is being deleted; try again afterward.".into());
        }
        Ok(())
    }
    /// Read a snapshot-bound page of indexed Bot history. Blob, ordinary and
    /// remote histories return None and retain their existing loading path.
    /// Use `write_indexed_history_page` for page-relative writes; `write_rows`
    /// requires a complete transcript and a separate revision guard.
    pub fn indexed_history_page(
        &self,
        session: i64,
        before: Option<&IndexedHistoryCursor>,
        page_turns: usize,
    ) -> Result<Option<IndexedHistoryPage>, String> {
        if self.shared.is_none() || self.remote_session(session)?.is_some() {
            return Ok(None);
        }
        self.shared_indexed_history_page(session, before, page_turns)
    }
    /// Merge a page-relative edit or tail append against the exact snapshot
    /// that produced the page. Older pages cannot append into newer history.
    pub fn write_indexed_history_page(
        &mut self,
        session: i64,
        page: &IndexedHistoryPage,
        start: usize,
        rows: &[Row],
    ) -> Result<(), String> {
        if self.shared.is_none() || self.remote_session(session)?.is_some() {
            return Err("Indexed Bot history is not available for this session".into());
        }
        self.shared_write_indexed_history_page(session, page, start, rows)
    }
    pub fn reset_provider_id_if(&self, id: i64, expected: Option<&str>) -> Result<(), String> {
        if self.shared.is_some() {
            return self.shared_reset_provider_id_if(id, expected);
        }

        self.conn
            .execute(
                "UPDATE native_sessions SET provider_id=NULL WHERE id=?1 AND provider_id IS ?2",
                params![id, expected],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn provider_accounts(&self) -> Result<crate::accounts::Book, String> {
        if self.shared.is_some() {
            return self.shared_accounts();
        }

        use rusqlite::OptionalExtension;
        let value: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM native_settings WHERE key='provider-accounts'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        value
            .map(|v| {
                serde_json::from_str(&v).map_err(|e| format!("Cannot read provider accounts: {e}"))
            })
            .unwrap_or_else(|| Ok(Default::default()))
    }
    pub fn save_provider_accounts(&self, book: &crate::accounts::Book) -> Result<(), String> {
        if self.shared.is_some() {
            return self.shared_save_accounts(book);
        }

        let value = serde_json::to_string(book).map_err(|e| e.to_string())?;
        self.conn.execute("INSERT INTO native_settings(key,value) VALUES('provider-accounts',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [value]).map_err(|e| e.to_string())?;
        Ok(())
    }
    /// Legacy prototype fixtures. Desktop launches use open_vorkspace, including
    /// isolated development launches, so production history needs no conversion.
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
          CREATE TABLE IF NOT EXISTS native_settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS native_sessions(id INTEGER PRIMARY KEY, title TEXT NOT NULL,
            provider TEXT NOT NULL, cwd TEXT NOT NULL, provider_id TEXT,
            updated_at INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0);
          CREATE TABLE IF NOT EXISTS native_rows(session_id INTEGER NOT NULL REFERENCES native_sessions(id),
            row_ix INTEGER NOT NULL, role TEXT NOT NULL, body TEXT NOT NULL, first INTEGER NOT NULL,
            PRIMARY KEY(session_id,row_ix));
          CREATE TABLE IF NOT EXISTS native_reminders(session_id INTEGER PRIMARY KEY REFERENCES native_sessions(id),
            due_at INTEGER NOT NULL, fired_at INTEGER);").map_err(|e| e.to_string())?;
        let columns = conn
            .prepare("PRAGMA table_info(native_sessions)")
            .map_err(|e| e.to_string())?
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        for (column, kind) in [
            ("updated_at", "INTEGER NOT NULL DEFAULT 0"),
            ("archived", "INTEGER NOT NULL DEFAULT 0"),
            ("pinned", "INTEGER NOT NULL DEFAULT 0"),
            ("work_item", "TEXT"),
        ] {
            if !columns.iter().any(|name| name == column) {
                conn.execute(
                    &format!("ALTER TABLE native_sessions ADD COLUMN {column} {kind}"),
                    [],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        let has_sent_at = conn
            .prepare("PRAGMA table_info(native_rows)")
            .map_err(|e| e.to_string())?
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
            .iter()
            .any(|name| name == "sent_at");
        if !has_sent_at {
            conn.execute("ALTER TABLE native_rows ADD COLUMN sent_at INTEGER", [])
                .map_err(|e| e.to_string())?;
        }
        // Older native histories have completion times in their turn receipts.
        // Preserve unknown ages instead of stamping all old conversations now.
        let recorded = conn.prepare("SELECT s.id,a.value FROM native_sessions s JOIN native_settings a ON a.key='agent:'||s.id WHERE s.updated_at=0")
            .map_err(|e| e.to_string())?
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
        for (id, settings) in recorded {
            if let Ok(settings) = serde_json::from_str::<crate::agent::Settings>(&settings) {
                if let Some(time) = settings
                    .turns
                    .values()
                    .filter_map(|turn| turn.completed_at)
                    .max()
                {
                    conn.execute(
                        "UPDATE native_sessions SET updated_at=?2 WHERE id=?1",
                        params![id, time.min(i64::MAX as u64) as i64],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(Self {
            conn,
            path: path.into(),
            shared: None,
            project_deletions: Default::default(),
        })
    }
    /// The selected desktop or isolated-development database.
    pub fn database_path(&self) -> &Path {
        &self.path
    }
    pub fn data_dir(&self) -> Option<&Path> {
        (self.path != Path::new(":memory:"))
            .then(|| self.path.parent())
            .flatten()
    }
    pub fn setting(&self, key: &str) -> Result<Option<String>, String> {
        if self.shared.is_some() {
            return self.shared_setting(key);
        }

        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                "SELECT value FROM native_settings WHERE key=?1",
                [key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())
    }
    /// Replace one setting only if it still matches the value the caller read.
    pub fn compare_swap_setting(
        &self,
        key: &str,
        expected: Option<&str>,
        value: &str,
    ) -> Result<(), String> {
        self.mark_attachment_activity();
        if self.shared.is_some() {
            return self.shared_compare_swap_setting(key, expected, value);
        }
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )
        .map_err(|e| e.to_string())?;
        use rusqlite::OptionalExtension;
        let current: Option<String> = tx
            .query_row(
                "SELECT value FROM native_settings WHERE key=?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if current.as_deref() != expected {
            return Err(
                "Setting changed in another window; your changes were preserved in this window."
                    .into(),
            );
        }
        tx.execute("INSERT INTO native_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key,value]).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    /// Persist a dispatch claim before any provider effect. Claims are never
    /// leased or automatically retried: a crash leaves an ambiguous outcome.
    pub fn claim_bot_message(
        &self,
        bot: &str,
        message: &crate::bot::Message,
        token: &str,
    ) -> Result<bool, String> {
        if self.shared.is_some() {
            return self.shared_claim_bot_message(bot, message, token);
        }
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )
        .map_err(|e| e.to_string())?;
        let roster = self
            .setting("bot-roster")?
            .ok_or("Bot outbox is unavailable")?;
        validate_bot_claim(&roster, bot, message)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS native_bot_dispatch_claims(bot_id TEXT NOT NULL,message_id TEXT NOT NULL,token TEXT NOT NULL,PRIMARY KEY(bot_id,message_id))").map_err(|e| e.to_string())?;
        let inserted = tx
            .execute(
                "INSERT OR IGNORE INTO native_bot_dispatch_claims VALUES(?1,?2,?3)",
                params![bot, message.id, token],
            )
            .map_err(|e| e.to_string())?
            == 1;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(inserted)
    }
    /// Only call when dispatch was rejected before any provider call.
    pub fn release_unsubmitted_bot_claim(
        &self,
        bot: &str,
        message: &str,
        token: &str,
    ) -> Result<(), String> {
        if !shared::has_table(&self.conn, "native_bot_dispatch_claims")? {
            return Ok(());
        }
        self.conn.execute("DELETE FROM native_bot_dispatch_claims WHERE bot_id=?1 AND message_id=?2 AND token=?3", params![bot,message,token]).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn save_setting(&self, key: &str, value: &str) -> Result<(), String> {
        self.mark_attachment_activity();
        if self.shared.is_some() {
            return self.shared_save_setting(key, value);
        }

        self.conn.execute("INSERT INTO native_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key,value])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    /// Publish related settings together, preserving their previous values if
    /// any write fails. The desktop sidecar publishes one atomic root.
    pub fn save_settings(&self, values: &[(&str, &str)]) -> Result<(), String> {
        self.mark_attachment_activity();
        if self.shared.is_some() {
            return self.shared_save_settings(values);
        }
        let transaction = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        for (key, value) in values {
            transaction.execute("INSERT INTO native_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key,value]).map_err(|e| e.to_string())?;
        }
        transaction.commit().map_err(|e| e.to_string())
    }
    pub fn ui_state(&self) -> Result<Option<String>, String> {
        if self.shared.is_some() {
            return self.shared_ui_state();
        }

        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                "SELECT value FROM native_settings WHERE key='workspace'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())
    }
    pub fn agent_settings(&self, id: i64) -> Result<Option<crate::agent::Settings>, String> {
        let remote = id < 0 || self.remote_session(id)?.is_some();
        if remote {
            if let Some(raw) = self.setting(&format!("remote-agent:{id}"))? {
                return serde_json::from_str(&raw)
                    .map(Some)
                    .map_err(|e| format!("Cannot read remote settings: {e}"));
            }
            if id < 0 || self.shared.is_none() {
                return Ok(None);
            }
        }
        if self.shared.is_some() {
            let mut settings = self.shared_agent(id)?;
            if remote {
                if let Some(settings) = settings.as_mut() {
                    if settings.worktree_cwd.is_none() {
                        settings.worktree_cwd = self.legacy_remote_worktree(id)?;
                    }
                }
            }
            return Ok(settings);
        }

        use rusqlite::OptionalExtension;
        let text: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM native_settings WHERE key=?1",
                [format!("agent:{id}")],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        text.map(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
            .transpose()
    }
    /// Read workspace membership without loading every session's full agent settings.
    pub fn session_worktrees(
        &self,
    ) -> Result<std::collections::HashMap<i64, Option<String>>, String> {
        if self.shared.is_some() {
            return self.shared_worktrees();
        }

        self.conn
            .prepare("SELECT s.id, CASE WHEN json_valid(a.value) THEN CASE WHEN json_type(a.value, '$.worktree_cwd')='text' THEN json_extract(a.value, '$.worktree_cwd') END END FROM native_sessions s LEFT JOIN native_settings a ON a.key='agent:'||s.id")
            .map_err(|e| e.to_string())?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())
    }
    pub fn context_usage(&self, id: i64) -> Result<Option<crate::context::Usage>, String> {
        if self.shared.is_some() {
            return self.shared_context(id);
        }

        use rusqlite::OptionalExtension;
        let text: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM native_settings WHERE key=?1",
                [format!("context:{id}")],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        text.map(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
            .transpose()
    }
    pub fn save_context_usage(&self, id: i64, usage: &crate::context::Usage) -> Result<(), String> {
        if self.shared.is_some() {
            return self.shared_patch(
                id,
                &[
                    ("context_used", serde_json::json!(usage.used)),
                    ("context_window", serde_json::json!(usage.window)),
                ],
            );
        }

        self.conn.execute("INSERT INTO native_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![format!("context:{id}"), serde_json::to_string(usage).map_err(|e| e.to_string())?]).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn save_note(&self, note: &crate::response::Note) -> Result<(), String> {
        self.save_response_note(note)
    }
    pub fn notes(&self) -> Result<Vec<crate::response::Note>, String> {
        Ok(self
            .list_notes()?
            .into_iter()
            .map(|note| {
                let source_session = self
                    .note_source_session(&note)
                    .or_else(|| {
                        self.shared
                            .is_none()
                            .then(|| note.source_session_id.as_deref()?.parse().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                crate::response::Note {
                    id: note
                        .id
                        .strip_prefix("gpui-note-")
                        .and_then(|id| id.parse().ok())
                        .unwrap_or_else(|| shared::handle(&note.id) as u64),
                    title: note.title,
                    body: note.body,
                    tags: note.tags,
                    source_session,
                    source_cwd: note.source_cwd.unwrap_or_default(),
                }
            })
            .collect())
    }
    /// Read-only fallback for early native settings; list_notes owns merging it
    /// with the desktop table, so this must never call the notes facade.
    fn legacy_notes(&self) -> Result<Vec<crate::response::Note>, String> {
        let mut query = self
            .conn
            .prepare("SELECT value FROM native_settings WHERE key LIKE 'note:%' ORDER BY key DESC")
            .map_err(|e| e.to_string())?;
        let rows = query
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|row| {
            serde_json::from_str(&row.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
        })
        .collect()
    }
    pub fn save_agent_settings(
        &self,
        id: i64,
        settings: &crate::agent::Settings,
    ) -> Result<(), String> {
        if id < 0 || self.remote_session(id)?.is_some() {
            return self.save_setting(
                &format!("remote-agent:{id}"),
                &serde_json::to_string(settings).map_err(|e| e.to_string())?,
            );
        }
        if self.shared.is_some() {
            return self.shared_save_agent(id, settings);
        }

        self.conn.execute("INSERT INTO native_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![format!("agent:{id}"), serde_json::to_string(settings).map_err(|e| e.to_string())?]).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn configure_agent(
        &self,
        id: i64,
        provider: &str,
        settings: &crate::agent::Settings,
    ) -> Result<(), String> {
        if self
            .agent_settings(id)?
            .is_some_and(|saved| saved.revision > settings.revision)
        {
            return Ok(());
        }
        self.change_provider(id, provider)?;
        self.save_agent_settings(id, settings)
    }
    pub fn change_provider(&self, id: i64, provider: &str) -> Result<(), String> {
        if let Some(mut remote) = self.remote_session(id)? {
            if remote.summary.provider != provider {
                // The host fixes the harness at creation, as React's remote view does.
                if remote.session_id.is_some() {
                    return Err("Start a new remote conversation to change provider".into());
                }
                remote.summary.provider = provider.into();
                self.save_remote_session(&remote)?;
            }
            return Ok(());
        }
        if self.shared.is_some() {
            return self.shared_change_provider(id, provider);
        }

        self.conn.execute("UPDATE native_sessions SET provider=?2,provider_id=NULL WHERE id=?1 AND provider<>?2", params![id,provider]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub fn save_ui_state(&self, state: &str) -> Result<(), String> {
        if self.shared.is_some() {
            return self.shared_save_setting("workspace", state);
        }

        self.conn.execute("INSERT INTO native_settings(key,value) VALUES('workspace',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[state]).map_err(|e|e.to_string())?;
        Ok(())
    }
    fn local_sessions(&self) -> Result<Vec<Session>, String> {
        if self.shared.is_some() {
            return self.shared_sessions();
        }

        let mut stmt = self
            .conn
            .prepare(
                "SELECT id,title,provider,cwd,provider_id,
                 (EXISTS(SELECT 1 FROM native_rows WHERE session_id=native_sessions.id)
                  OR EXISTS(SELECT 1 FROM native_settings WHERE key='follow-up:'||native_sessions.id AND json_valid(value) AND (json_type(value,'$.draft')='object' OR json_array_length(value,'$.messages')>0))),updated_at,archived,
                 pinned,work_item
                 FROM native_sessions WHERE NOT EXISTS(SELECT 1 FROM native_settings WHERE key='inbox-ask:'||native_sessions.id) ORDER BY updated_at DESC,id DESC",
            )
            .map_err(|e| e.to_string())?;
        let result = stmt
            .query_map([], |r| {
                Ok(Session {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    provider: r.get(2)?,
                    cwd: r.get(3)?,
                    provider_id: r.get(4)?,
                    has_messages: r.get(5)?,
                    updated_at: r.get::<_, i64>(6)?.max(0) as u64,
                    archived: r.get(7)?,
                    pinned: r.get(8)?,
                    work_item: r
                        .get::<_, Option<String>>(9)?
                        .and_then(|text| serde_json::from_str(&text).ok()),
                })
            })
            .map_err(|e| e.to_string())?;
        result
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }
    /// Each session another one started through the app CLI, and its caller.
    pub fn started_links(&self) -> Result<std::collections::HashMap<i64, i64>, String> {
        let mut links = if self.shared.is_some() {
            self.shared_started_links()?
        } else {
            std::collections::HashMap::new()
        };
        let saved: std::collections::HashMap<_, _> = self
            .settings_with_prefix("started-by:")?
            .into_iter()
            .map(|(child, value)| Ok((child, value.trim().parse().map_err(|_| "Invalid caller")?)))
            .collect::<Result<_, String>>()?;
        links.extend(saved);
        Ok(links)
    }
    pub fn save_started_link(&self, child: i64, caller: i64) -> Result<(), String> {
        self.save_setting(&format!("started-by:{child}"), &caller.to_string())
    }
    /// Every lead's last orchestration summary.
    pub fn orchestration_summaries(
        &self,
    ) -> Result<std::collections::HashMap<i64, crate::orchestration::Summary>, String> {
        Ok(self
            .settings_with_prefix("orchestration:")?
            .into_iter()
            .filter_map(|(lead, value)| Some((lead, serde_json::from_str(&value).ok()?)))
            .collect())
    }
    pub fn save_orchestration_summary(
        &self,
        lead: i64,
        summary: &crate::orchestration::Summary,
    ) -> Result<(), String> {
        let value = serde_json::to_string(summary).map_err(|e| e.to_string())?;
        self.save_setting(&format!("orchestration:{lead}"), &value)
    }
    /// `prefix<session id>` settings, by session id.
    fn settings_with_prefix(&self, prefix: &str) -> Result<Vec<(i64, String)>, String> {
        if self.shared.is_some() {
            return self.shared_settings_with_prefix(prefix);
        }

        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM native_settings WHERE key LIKE ?1 || '%'")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([prefix], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            let (key, value) = row.map_err(|e| e.to_string())?;
            if let Some(id) = key.strip_prefix(prefix).and_then(|id| id.parse().ok()) {
                out.push((id, value));
            }
        }
        Ok(out)
    }
    /// Sessions whose latest message is saved as a draft and not yet sent.
    pub fn draft_sessions(&self) -> Result<std::collections::HashSet<i64>, String> {
        if self.shared.is_some() {
            return self.shared_drafts();
        }

        let mut stmt = self
            .conn
            .prepare(
                "SELECT CAST(substr(key,11) AS INTEGER) FROM native_settings
                 WHERE key LIKE 'follow-up:%' AND json_valid(value)
                 AND json_type(value,'$.draft')='object'",
            )
            .map_err(|e| e.to_string())?;
        let ids = stmt
            .query_map([], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        ids.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }
    fn local_create(&self, provider: &str, cwd: &str) -> Result<Session, String> {
        if self.shared.is_some() {
            return self.shared_create(provider, cwd);
        }

        let updated_at = crate::response::now_ms();
        let title = format!(
            "New {} session",
            crate::kind::Kind::parse(provider)
                .ok_or_else(|| format!("Unknown provider: {provider}"))?
                .label()
        );
        self.conn
            .execute(
                "INSERT INTO native_sessions(title,provider,cwd,updated_at) VALUES(?1,?2,?3,?4)",
                params![title, provider, cwd, updated_at.min(i64::MAX as u64) as i64],
            )
            .map_err(|e| e.to_string())?;
        Ok(Session {
            id: self.conn.last_insert_rowid(),
            title,
            provider: provider.into(),
            cwd: cwd.into(),
            provider_id: None,
            has_messages: false,
            updated_at,
            archived: false,
            pinned: false,
            work_item: None,
        })
    }
    /// A WAL reader for background search, independent of the writer's mutex.
    /// In-memory fixture stores must continue using their original connection.
    pub fn search_reader(&self) -> Result<Option<Self>, String> {
        if self.path == Path::new(":memory:") {
            return Ok(None);
        }
        if self.shared.is_some() {
            return self.shared_search_reader().map(Some);
        }
        let conn =
            Connection::open_with_flags(&self.path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| e.to_string())?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        Ok(Some(Self {
            conn,
            path: self.path.clone(),
            shared: None,
            project_deletions: Default::default(),
        }))
    }
    /// Stream saved message bodies without loading conversations into the UI.
    /// Rejoin native row fragments so phrase matches span storage boundaries.
    pub fn search_sessions(
        &self,
        query: &str,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(Vec<crate::search::Hit>, bool), String> {
        if self.shared.is_some() {
            return self.shared_search(query, cancel);
        }

        use std::sync::atomic::Ordering;
        if query.trim().is_empty() || cancel.load(Ordering::Acquire) {
            return Ok((vec![], false));
        }
        let flag = cancel.clone();
        self.conn
            .progress_handler(2000, Some(move || flag.load(Ordering::Acquire)))
            .map_err(|e| e.to_string())?;
        let result = (|| {
            let summaries = self.sessions()?;
            // Walk each conversation's primary-key range in recency order.
            // Sorting a join containing every message body would copy the entire
            // transcript database into SQLite's sorter on every keystroke.
            let mut statement=self.conn.prepare("SELECT row_ix,role,body,first FROM native_rows WHERE session_id=?1 ORDER BY row_ix").map_err(|e|e.to_string())?;
            let mut user = self
                .conn
                .prepare(
                    "SELECT EXISTS(SELECT 1 FROM native_rows WHERE session_id=?1 AND role='user')",
                )
                .map_err(|e| e.to_string())?;
            let mut hits = Vec::new();
            let mut truncated = false;
            'sessions: for summary in summaries.iter().filter(|s| !s.archived && s.has_messages) {
                if cancel.load(Ordering::Acquire) {
                    return Ok((vec![], false));
                }
                if !user
                    .query_row([summary.id], |r| r.get::<_, bool>(0))
                    .map_err(|e| e.to_string())?
                {
                    continue;
                }
                let flush = |current: Option<(usize, String, String)>,
                             hits: &mut Vec<crate::search::Hit>| {
                    if let Some((row, role, text)) = current {
                        if let Some(hit) =
                            crate::search::message_hit(summary, row, &role, &text, query, false)
                        {
                            hits.push(hit);
                        }
                    }
                };
                let mut current: Option<(usize, String, String)> = None;
                let mut rows = statement.query([summary.id]).map_err(|e| e.to_string())?;
                while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                    if cancel.load(Ordering::Acquire) {
                        return Ok((vec![], false));
                    }
                    let ix = row.get::<_, i64>(0).map_err(|e| e.to_string())?.max(0) as usize;
                    let role: String = row.get(1).map_err(|e| e.to_string())?;
                    let body: String = row.get(2).map_err(|e| e.to_string())?;
                    let first: bool = row.get(3).map_err(|e| e.to_string())?;
                    if first || current.is_none() {
                        flush(current.take(), &mut hits);
                        if hits.len() >= 400 {
                            truncated = true;
                            break 'sessions;
                        }
                        current = Some((ix, role, String::new()));
                    }
                    if let Some((_, _, text)) = &mut current {
                        let mut end = body
                            .len()
                            .min((2 * 1024 * 1024usize).saturating_sub(text.len()));
                        while !body.is_char_boundary(end) {
                            end -= 1;
                        }
                        text.push_str(&body[..end]);
                        truncated |= end < body.len();
                    }
                }
                flush(current, &mut hits);
                if hits.len() >= 400 {
                    truncated = true;
                    break;
                }
            }
            let titles = summaries
                .into_iter()
                .filter(|s| !s.archived && s.has_messages)
                .collect::<Vec<_>>();
            hits.extend(crate::search::titles(&titles, query));
            Ok((hits, truncated))
        })();
        self.conn
            .progress_handler(0, None::<fn() -> bool>)
            .map_err(|e| e.to_string())?;
        if cancel.load(Ordering::Acquire) {
            Ok((vec![], false))
        } else {
            result
        }
    }
    pub fn session_history_revision(&self, session: i64) -> Result<i64, String> {
        if self.shared.is_some() {
            let original = self.persisted_session_id(session)?;
            self.history_revision(&original)
        } else {
            Ok(0)
        }
    }
    pub fn load(&self, session: i64) -> Result<Transcript, String> {
        if let Some(remote) = self.remote_session(session)? {
            let Some(id) = remote.session_id.as_deref() else {
                return Ok(Transcript::default());
            };
            let client = crate::remote::default_connections()?;
            let machine = remote.machine(&client)?;
            let mut snapshot = client.load_session(&machine.id, id, None)?;
            crate::remote::attachments::qualify_snapshot(
                &mut snapshot,
                &remote.project.environment_id,
                id,
            );
            return Self::project_remote_history(&snapshot).map(|(history, _)| history);
        }
        if self.shared.is_some() {
            return self.shared_load(session);
        }

        let mut stmt = self
            .conn
            .prepare("SELECT role,body,first,sent_at FROM native_rows WHERE session_id=? ORDER BY row_ix")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([session], |r| {
                Ok(Row {
                    role: r.get(0)?,
                    text: r.get(1)?,
                    first: r.get(2)?,
                    sent_at: r
                        .get::<_, Option<i64>>(3)?
                        .and_then(|at| u64::try_from(at).ok()),
                })
            })
            .map_err(|e| e.to_string())?;
        let mut rows = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        // Pre-timestamp histories recorded completion and duration per run.
        // Recover their start time where known, leaving undated history undated.
        if let Ok(Some(settings)) = self.agent_settings(session) {
            for (turn, info) in settings.turns {
                if turn > rows.len() {
                    continue;
                }
                let mut prompt = turn;
                while prompt > 0 && rows[prompt - 1].is_prompt() {
                    prompt -= 1;
                }
                if let Some(row) = rows.get_mut(prompt) {
                    if row.sent_at.is_none() {
                        row.sent_at = info
                            .completed_at
                            .and_then(|at| at.checked_sub(info.duration_ms));
                    }
                }
            }
        }
        Ok(Transcript { rows })
    }
    pub fn write_rows(&mut self, session: i64, start: usize, rows: &[Row]) -> Result<(), String> {
        self.write_rows_with_message(session, start, rows, None)
    }
    pub fn write_rows_with_message(
        &mut self,
        session: i64,
        start: usize,
        rows: &[Row],
        message_id: Option<&str>,
    ) -> Result<(), String> {
        if self.remote_session(session)?.is_some() {
            return Err("Remote transcripts are owned by the host".into());
        }
        if self.shared.is_some() {
            return self.shared_write_rows(session, start, rows, message_id);
        }

        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO native_rows(session_id,row_ix,role,body,first,sent_at) VALUES(?1,?2,?3,?4,?5,?6)
                ON CONFLICT(session_id,row_ix) DO UPDATE SET body=excluded.body",
                )
                .map_err(|e| e.to_string())?;
            for (offset, row) in rows.iter().enumerate() {
                stmt.execute(params![
                    session,
                    (start + offset) as i64,
                    row.role,
                    row.text,
                    row.first,
                    row.sent_at.and_then(|at| i64::try_from(at).ok())
                ])
                .map_err(|e| e.to_string())?;
            }
        }
        if !rows.is_empty() {
            tx.execute(
                "UPDATE native_sessions SET updated_at=?2 WHERE id=?1",
                params![
                    session,
                    crate::response::now_ms().min(i64::MAX as u64) as i64
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        if let Some(id) = message_id {
            tx.execute("INSERT INTO native_settings(key,value) VALUES(?1,'1') ON CONFLICT(key) DO UPDATE SET value='1'",[format!("bot-delivered:{session}:{id}")]).map_err(|e|e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
    /// Archiving preserves the transcript, provider context and configuration.
    pub fn set_archived(&self, id: i64, archived: bool) -> Result<(), String> {
        if self.remote_session(id)?.is_some() {
            return self.patch_remote_session(id, serde_json::json!({"archived": archived}));
        }
        if self.shared.is_some() {
            return self.shared_patch(id, &[("archived", serde_json::json!(archived))]);
        }

        self.conn
            .execute(
                "UPDATE native_sessions SET archived=?2 WHERE id=?1",
                params![id, archived],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn set_pinned(&self, ids: &[i64], pinned: bool) -> Result<(), String> {
        let any_remote = ids
            .iter()
            .map(|id| self.remote_session(*id).map(|remote| remote.is_some()))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .any(|remote| remote);
        if any_remote {
            for id in ids {
                if self.remote_session(*id)?.is_some() {
                    self.patch_remote_session(*id, serde_json::json!({"pinned": pinned}))?;
                } else {
                    self.set_pinned(&[*id], pinned)?;
                }
            }
            return Ok(());
        }
        if self.shared.is_some() {
            return self.shared_pins(ids, pinned);
        }

        for id in ids {
            self.conn
                .execute(
                    "UPDATE native_sessions SET pinned=?2 WHERE id=?1",
                    params![id, pinned],
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub fn set_work_item(
        &self,
        id: i64,
        item: Option<&crate::session_groups::WorkItem>,
    ) -> Result<(), String> {
        if self.remote_session(id)?.is_some() {
            return self.patch_remote_session(id, serde_json::json!({"linkedWorkItem": item}));
        }
        if self.shared.is_some() {
            return self.shared_patch(
                id,
                &[(
                    "linked_work_item_json",
                    item.map(shared::encode)
                        .transpose()?
                        .map(serde_json::Value::String)
                        .unwrap_or(serde_json::Value::Null),
                )],
            );
        }

        let text = item
            .map(|item| serde_json::to_string(item).map_err(|e| e.to_string()))
            .transpose()?;
        self.conn
            .execute(
                "UPDATE native_sessions SET work_item=?2 WHERE id=?1",
                params![id, text],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    /// A project's folders and folded groups.
    pub fn groups(&self, cwd: &str) -> Result<crate::session_groups::Groups, String> {
        if self.shared.is_some() {
            return self.shared_groups(cwd);
        }

        use rusqlite::OptionalExtension;
        let text: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM native_settings WHERE key=?1",
                [format!("groups:{cwd}")],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(text
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default())
    }
    pub fn save_groups(
        &self,
        cwd: &str,
        groups: &crate::session_groups::Groups,
    ) -> Result<(), String> {
        if self.shared.is_some() {
            return self.shared_save_setting(&format!("groups:{cwd}"), &shared::encode(groups)?);
        }

        self.conn.execute("INSERT INTO native_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![format!("groups:{cwd}"), serde_json::to_string(groups).map_err(|e| e.to_string())?]).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn reminders(&self) -> Result<Vec<Reminder>, String> {
        if self.shared.is_some() {
            return self.shared_reminders();
        }

        let mut stmt = self
            .conn
            .prepare("SELECT session_id,due_at,fired_at FROM native_reminders ORDER BY due_at,session_id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Reminder {
                    session_id: r.get(0)?,
                    due_at: r.get::<_, i64>(1)?.max(0) as u64,
                    fired_at: r.get::<_, Option<i64>>(2)?.map(|t| t.max(0) as u64),
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }
    /// Schedules (or reschedules, re-arming) each session's reminder.
    pub fn set_reminders(&mut self, ids: &[i64], due_at: u64) -> Result<(), String> {
        if self.shared.is_some() {
            return self.shared_set_reminders(ids, due_at);
        }

        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        for id in ids {
            tx.execute(
                "INSERT INTO native_reminders(session_id,due_at,fired_at) VALUES(?1,?2,NULL)
                 ON CONFLICT(session_id) DO UPDATE SET due_at=excluded.due_at,fired_at=NULL",
                params![id, due_at.min(i64::MAX as u64) as i64],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
    /// Re-arms only the reminder occurrence the user snoozed. A stale popup
    /// must not overwrite a newer schedule created in another window.
    pub fn snooze_reminder_if(
        &self,
        id: i64,
        expected_due: u64,
        new_due: u64,
    ) -> Result<bool, String> {
        if self.shared.is_some() {
            return self.shared_snooze_reminder_if(id, expected_due, new_due);
        }
        self.conn.execute("UPDATE native_reminders SET due_at=?3,fired_at=NULL WHERE session_id=?1 AND due_at=?2",
            params![id, expected_due.min(i64::MAX as u64) as i64, new_due.min(i64::MAX as u64) as i64]).map(|changed|changed==1).map_err(|e|e.to_string())
    }
    /// Clears the sessions' reminders; with `expected`, only that one, so a
    /// stale delivery never clears a newer reminder.
    pub fn clear_reminders(&self, ids: &[i64], expected: Option<u64>) -> Result<(), String> {
        if self.shared.is_some() {
            return self.shared_clear_reminders(ids, expected);
        }

        for id in ids {
            match expected {
                Some(due) => self.conn.execute(
                    "DELETE FROM native_reminders WHERE session_id=?1 AND due_at=?2",
                    params![id, due.min(i64::MAX as u64) as i64],
                ),
                None => self
                    .conn
                    .execute("DELETE FROM native_reminders WHERE session_id=?1", [id]),
            }
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    /// Marks due, undelivered reminders delivered and returns them, once.
    pub fn take_due_reminders(&mut self, now: u64) -> Result<Vec<Reminder>, String> {
        let due: Vec<_> = self
            .reminders()?
            .into_iter()
            .filter(|r| r.due_at <= now && r.fired_at.is_none())
            .collect();
        if self.shared.is_some() {
            return self.shared_fire_reminders(&due, now);
        }
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        for reminder in &due {
            tx.execute(
                "UPDATE native_reminders SET fired_at=?2 WHERE session_id=?1",
                params![reminder.session_id, now.min(i64::MAX as u64) as i64],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(due)
    }
    /// Deletes the conversation, its settings and reminder for good.
    pub fn delete(&mut self, id: i64) -> Result<(), String> {
        self.delete_checked(id, None)
    }
    pub(super) fn delete_checked(
        &mut self,
        id: i64,
        app: Option<(i64, &str)>,
    ) -> Result<(), String> {
        if let Some((caller, project)) = app {
            self.check_app_target(caller, id, project)?;
        }
        if self.remote_session(id)?.is_some() {
            return self.delete_remote_session(id);
        }
        if self.shared.is_some() {
            return self.shared_delete_checked(id, app);
        }

        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )
        .map_err(|e| e.to_string())?;
        if let Some((caller, project)) = app {
            self.check_app_target(caller, id, project)?;
        }
        local_submissions::delete_if_present(&tx, &id.to_string())?;
        if tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='composer_draft_recoveries')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|e| e.to_string())?
        {
            tx.execute(
                "DELETE FROM composer_draft_recoveries WHERE session_id=?1",
                [id.to_string()],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.execute("DELETE FROM native_rows WHERE session_id=?1", [id])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM native_reminders WHERE session_id=?1", [id])
            .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM native_settings WHERE key IN (?1,?2,?3,?5,?6,?7,?8,?9) OR key LIKE ?4",
            params![
                format!("agent:{id}"),
                format!("context:{id}"),
                format!("follow-up:{id}"),
                format!("bot-delivered:{id}:%"),
                format!("started-by:{id}"),
                format!("orchestration:{id}"),
                format!("inbox-ask:{id}"),
                format!("automation:{id}"),
                format!("composer:{id}")
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM native_sessions WHERE id=?1", [id])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn provider_id(&self, id: i64, provider_id: &str) -> Result<(), String> {
        if self.shared.is_some() {
            return self.shared_patch(
                id,
                &[("provider_session_id", serde_json::json!(provider_id))],
            );
        }

        self.conn
            .execute(
                "UPDATE native_sessions SET provider_id=?2 WHERE id=?1",
                params![id, provider_id],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    /// Keep conversation history while detaching a removed working copy.
    pub fn detach_worktree(
        &self,
        path: &str,
        main: &str,
    ) -> Result<Vec<(Session, crate::agent::Settings)>, String> {
        if self.shared.is_some() {
            return self.shared_detach_worktree(path, main);
        }

        let sessions = self.sessions()?;
        let mut changed = Vec::new();
        for mut session in sessions {
            let mut settings = self.agent_settings(session.id)?.unwrap_or_default();
            let execution = settings.worktree_cwd.as_deref().unwrap_or(&session.cwd);
            if !crate::files::is_equal_or_inside(execution, path) {
                continue;
            }
            settings.worktree_cwd = Some(execution.to_owned());
            settings.worktree_removed = true;
            settings.revision += 1;
            settings.new_worktree = false;
            settings.worktree_base = None;
            if crate::files::is_equal_or_inside(&session.cwd, path) {
                session.cwd = main.into();
            }
            session.provider_id = None;
            changed.push((session, settings));
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        for (session, settings) in &changed {
            tx.execute(
                "UPDATE native_sessions SET cwd=?2,provider_id=NULL WHERE id=?1",
                params![session.id, session.cwd],
            )
            .map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO native_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![format!("agent:{}",session.id),serde_json::to_string(settings).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
            tx.execute(
                "DELETE FROM native_settings WHERE key=?1",
                [format!("context:{}", session.id)],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(changed)
    }
    pub fn reset_provider_id(&self, id: i64) -> Result<(), String> {
        if self.shared.is_some() {
            return self.shared_patch(id, &[("provider_session_id", serde_json::Value::Null)]);
        }

        self.conn
            .execute(
                "UPDATE native_sessions SET provider_id=NULL WHERE id=?1",
                [id],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn title(&self, id: i64, title: &str) -> Result<(), String> {
        if self.remote_session(id)?.is_some() {
            return self.patch_remote_session(id, serde_json::json!({"title": title}));
        }
        if self.shared.is_some() {
            return self.shared_patch(id, &[("title", serde_json::json!(title))]);
        }

        self.conn
            .execute(
                "UPDATE native_sessions SET title=?2 WHERE id=?1",
                params![id, title],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    /// A late generated title cannot replace a manual rename or recreate a deleted session.
    pub fn replace_title(&self, id: i64, expected: &str, title: &str) -> Result<bool, String> {
        if self.shared.is_some() {
            return self.shared_replace_title(id, expected, title);
        }

        self.conn
            .execute(
                "UPDATE native_sessions SET title=?3 WHERE id=?1 AND title=?2",
                params![id, expected, title],
            )
            .map(|count| count == 1)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests;
