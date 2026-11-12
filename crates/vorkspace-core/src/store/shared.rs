//! A view adapter over the desktop schema. Opening preserves existing session
//! identities and storage. Native bookkeeping is created on explicit writes.
mod accounts;
mod agent;
pub(super) mod blocks;
mod bot_tables;
mod card;
mod edit;
mod follow_up;
mod history_window;
mod incremental;
pub use history_window::{IndexedHistoryCursor, IndexedHistoryPage};
pub(super) use incremental::HistoryAppend;
mod native_history;
mod ordinary;
mod settings;
#[cfg(any(test, feature = "test-support"))]
mod test_support;
#[cfg(test)]
mod tests;

use super::{LocalAgentBinding, Reminder, Session, Store, TurnHistory};
use crate::{
    agent::Settings,
    context::Usage,
    transcript::{Row, Transcript},
};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

pub(super) struct Shared {
    columns: HashSet<String>,
    earlier_bot_tables: Option<bot_tables::BotTables>,
    ids: RefCell<HashMap<i64, String>>,
    settings: settings::Sidecar,
    snapshots: RefCell<HashMap<i64, HistoryRevision>>,
    incremental: RefCell<HashMap<i64, incremental::HistoryIndex>>,
    history_schema: std::cell::Cell<Option<i64>>,
    card_revisions: RefCell<HashMap<i64, card::CardRevision>>,
    refreshed_cards: RefCell<HashMap<i64, HashMap<String, HistoryRevision>>>,
    queues: RefCell<HashMap<i64, String>>,
    preferences: std::sync::Arc<crate::desktop_preferences::DesktopPreferences>,
    read_only: bool,
    #[cfg(any(test, feature = "test-support"))]
    setting_write_failure: RefCell<Option<(String, String)>>,
    pub(super) pending_turn_recoveries:
        RefCell<HashMap<i64, Vec<super::turn_history::PendingTurnRecovery>>>,
}

/// A revision guard, rather than another owned copy of every rich source block.
/// Keep this small guard even when a view releases its history: pending writes
/// must still refuse a source edit made by another window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryRevision {
    Digest { hash: [u8; 32], len: usize },
    Stored(i64),
    Invalidated,
}
impl HistoryRevision {
    fn of(blocks: &[Value]) -> Self {
        let mut hash = Self::start(blocks.len());
        for block in blocks {
            Self::push(&mut hash, block);
        }
        Self::finish(hash, blocks.len())
    }
    fn finish(hash: Sha256, len: usize) -> Self {
        Self::Digest {
            hash: hash.finalize().into(),
            len,
        }
    }
    fn start(len: usize) -> Sha256 {
        let mut hash = Sha256::new();
        hash.update((len as u64).to_le_bytes());
        hash
    }
    fn push(hash: &mut Sha256, block: &Value) {
        #[cfg(test)]
        HISTORY_REVISION_HASH_BLOCKS.with(|count| count.set(count.get() + 1));
        fn size(hash: &mut Sha256, len: usize) {
            hash.update((len as u64).to_le_bytes());
        }
        fn value(hash: &mut Sha256, item: &Value) {
            match item {
                Value::Null => hash.update([0]),
                Value::Bool(boolean) => hash.update([1, u8::from(*boolean)]),
                Value::Number(number) => {
                    hash.update([2]);
                    // serde_json treats floating negative zero as equal to
                    // positive zero, while retaining its distinction from an integer.
                    let number = if number.is_f64() && number.as_f64() == Some(0.0) {
                        "0.0".into()
                    } else {
                        number.to_string()
                    };
                    size(hash, number.len());
                    hash.update(number.as_bytes());
                }
                Value::String(text) => {
                    hash.update([3]);
                    size(hash, text.len());
                    hash.update(text.as_bytes());
                }
                Value::Array(values) => {
                    hash.update([4]);
                    size(hash, values.len());
                    for item in values {
                        value(hash, item);
                    }
                }
                Value::Object(fields) => {
                    hash.update([5]);
                    size(hash, fields.len());
                    // Be canonical even if another crate enables preserve_order.
                    let mut fields: Vec<_> = fields.iter().collect();
                    fields.sort_unstable_by(|a, b| a.0.cmp(b.0));
                    for (key, item) in fields {
                        size(hash, key.len());
                        hash.update(key.as_bytes());
                        value(hash, item);
                    }
                }
            }
        }
        value(hash, block);
    }
    fn matches(&self, blocks: &[Value]) -> bool {
        *self == Self::of(blocks)
    }
    fn fast_forwards(&self, blocks: &[Value]) -> bool {
        let Self::Digest { len, .. } = *self else {
            return false;
        };
        if len == 0 || len > blocks.len() {
            return false;
        }
        // The guard already knows its source length. Only that prefix can
        // match; hashing every possible prefix made a stale settings/draft
        // save quadratic while the shared store mutex was held.
        if len < blocks.len() && self.matches(&blocks[..len]) {
            return true;
        }
        // Drafts can be interspersed with externally appended source blocks.
        // Hash borrowed blocks so this fallback never clones rich histories.
        let mut non_draft = blocks.iter().filter(|b| b["draft"] != true);
        let mut hash = Self::start(len);
        for _ in 0..len {
            let Some(block) = non_draft.next() else {
                return false;
            };
            Self::push(&mut hash, block);
        }
        *self == Self::finish(hash, len)
    }
}

fn sql<T>(result: rusqlite::Result<T>) -> Result<T, String> {
    result.map_err(|e| e.to_string())
}
#[cfg(test)]
thread_local! {
    static HISTORY_READ_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static HISTORY_ENCODE_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static HISTORY_REVISION_HASH_BLOCKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
fn parse<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, String> {
    #[cfg(test)]
    HISTORY_READ_BYTES.with(|count| count.set(count.get() + raw.len()));
    serde_json::from_str(raw).map_err(|e| e.to_string())
}
/// Desktop app CLI requests name children `app-<caller>-<request>`.
/// Both parts can contain hyphens, so the longest existing caller wins.
fn started_caller<'a>(id: &'a str, ids: &HashSet<String>) -> Option<&'a str> {
    let rest = id.strip_prefix("app-")?;
    rest.match_indices('-')
        .rev()
        .map(|(index, _)| &rest[..index])
        .find(|candidate| ids.contains(*candidate))
}
pub(super) fn encode<T: serde::Serialize>(value: &T) -> Result<String, String> {
    let encoded = serde_json::to_string(value).map_err(|e| e.to_string())?;
    #[cfg(test)]
    HISTORY_ENCODE_BYTES.with(|count| count.set(count.get() + encoded.len()));
    Ok(encoded)
}
pub(crate) use crate::session::desktop_session_handle as handle;
fn parse_block(raw: &str) -> Result<Value, String> {
    let mut value = parse(raw)?;
    crate::compatibility::block(&mut value);
    Ok(value)
}
fn parse_blocks(raw: &str) -> Result<Vec<Value>, String> {
    let mut blocks: Vec<Value> = parse(raw)?;
    for block in &mut blocks {
        crate::compatibility::block(block);
    }
    Ok(blocks)
}
pub(super) fn has_table(conn: &Connection, table: &str) -> Result<bool, String> {
    sql(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    ))
}
/// Turn keys are row offsets, including invisible document/card metadata rows.
/// Never substitute the session's current model for missing historical provenance.
pub(super) fn project_turns(
    blocks: &[Value],
) -> Result<HashMap<usize, crate::agent::TurnInfo>, String> {
    let mut turns = HashMap::new();
    let mut offset = 0;
    for block in blocks {
        offset += project_turn(block, offset, &mut turns)?;
    }
    Ok(turns)
}

fn project_turn(
    block: &Value,
    offset: usize,
    turns: &mut HashMap<usize, crate::agent::TurnInfo>,
) -> Result<usize, String> {
    let rows = blocks::project_block(block)?;
    record_projected_turn(block, &rows, offset, turns);
    Ok(rows.len())
}

pub(super) fn record_projected_turn(
    block: &Value,
    rows: &[Row],
    offset: usize,
    turns: &mut HashMap<usize, crate::agent::TurnInfo>,
) {
    if let Some(prompt) = rows.iter().rposition(Row::is_prompt) {
        let model = &block["turnModel"];
        let duration = block["durationMs"].as_u64();
        let started_at = block["startedAt"].as_u64();
        let metrics = block
            .get("turnMetrics")
            .filter(|value| value.is_object())
            .and_then(|value| serde_json::from_value(value.clone()).ok());
        if model.is_object()
            || duration.is_some()
            || started_at.is_some()
            || metrics.is_some()
            || block["providerTurnId"].is_string()
        {
            merge_turn_info(
                turns.entry(offset + prompt + 1).or_default(),
                crate::agent::TurnInfo {
                    provider_turn_id: block["providerTurnId"].as_str().map(str::to_owned),
                    provider: model["harness"].as_str().unwrap_or("").into(),
                    model: model["name"].as_str().unwrap_or("").into(),
                    model_id: model["id"].as_str().map(str::to_owned),
                    duration_ms: duration.unwrap_or(0),
                    duration_known: duration.is_some(),
                    started_at,
                    completed_at: started_at.and_then(|at| at.checked_add(duration.unwrap_or(0))),
                    metrics,
                },
            );
        }
    }
}

fn merge_turn_info(info: &mut crate::agent::TurnInfo, source: crate::agent::TurnInfo) {
    if !source.provider.is_empty() {
        info.provider = source.provider;
    }
    if !source.model.is_empty() {
        info.model = source.model;
    }
    if source.model_id.is_some() {
        info.model_id = source.model_id;
    }
    if source.provider_turn_id.is_some() {
        info.provider_turn_id = source.provider_turn_id;
    }
    if source.duration_known {
        info.duration_ms = source.duration_ms;
        info.duration_known = true;
    }
    if source.started_at.is_some() {
        info.started_at = source.started_at;
        info.completed_at = source.completed_at;
    }
    if source.metrics.is_some() {
        info.metrics = source.metrics;
    }
}

impl Store {
    /// Open the same sessions, notes and reminders as the desktop app. Only a
    /// brand-new file gets a schema; existing schema/data is not migrated.
    pub fn open_vorkspace(path: &Path) -> Result<Self, String> {
        Self::open_shared(path, false, None)
    }
    pub fn read_vorkspace(path: &Path) -> Result<Self, String> {
        Self::open_shared(path, true, None)
    }
    fn open_shared(
        path: &Path,
        read_only: bool,
        preferences: Option<std::sync::Arc<crate::desktop_preferences::DesktopPreferences>>,
    ) -> Result<Self, String> {
        let memory = path == Path::new(":memory:");
        let exists = !memory && path.exists();
        if !read_only && !exists {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        let flags = if read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
        };
        let conn = sql(Connection::open_with_flags(path, flags))?;
        sql(conn.busy_timeout(std::time::Duration::from_secs(5)))?;
        sql(conn.execute_batch("PRAGMA foreign_keys=ON;"))?;
        if !exists && !read_only {
            sql(conn.execute_batch("PRAGMA journal_mode=WAL;"))?;
            let tx = sql(conn.unchecked_transaction())?;
            sql(tx.execute_batch(include_str!("shared_schema.sql")))?;
            sql(tx.commit())?;
        }
        let columns = sql(sql(conn.prepare("PRAGMA table_info(sessions)"))?
            .query_map([], |r| r.get::<_, String>(1)))?
        .collect::<rusqlite::Result<HashSet<_>>>()
        .map_err(|e| e.to_string())?;
        for required in [
            "id",
            "cwd",
            "harness",
            "model",
            "model_settings",
            "runtime_mode",
            "title",
            "provider_session_id",
            "blocks_json",
            "created_at",
            "updated_at",
        ] {
            if !columns.contains(required) {
                return Err(format!(
                    "Unsupported vorkspace database: missing sessions.{required}. No schema changes were made."
                ));
            }
        }
        let extras_path = (!memory).then(|| {
            path.with_file_name(format!(
                "{}.gpui.json",
                path.file_name().unwrap_or_default().to_string_lossy()
            ))
        });
        let settings = settings::Sidecar::open(extras_path)?;
        let earlier_bot_tables = bot_tables::BotTables::discover(&conn)?;
        let store = Self {
            conn,
            path: path.into(),
            project_deletions: Default::default(),
            #[cfg(any(test, feature = "test-support"))]
            test_directory: None,
            shared: Shared {
                earlier_bot_tables,
                columns,
                ids: RefCell::new(HashMap::new()),
                settings,
                snapshots: RefCell::new(HashMap::new()),
                card_revisions: RefCell::new(HashMap::new()),
                refreshed_cards: RefCell::new(HashMap::new()),
                incremental: RefCell::new(HashMap::new()),
                history_schema: std::cell::Cell::new(None),
                queues: RefCell::new(HashMap::new()),
                preferences: preferences.unwrap_or_else(|| {
                    std::sync::Arc::new(
                        path.parent()
                            .filter(|_| !memory)
                            .map(crate::desktop_preferences::DesktopPreferences::load)
                            .unwrap_or_default(),
                    )
                }),
                read_only,
                #[cfg(any(test, feature = "test-support"))]
                setting_write_failure: Default::default(),
                pending_turn_recoveries: Default::default(),
            },
        };
        // Register only identities here. No blocks_json is read at startup.
        let mut query = sql(store.conn.prepare("SELECT id FROM sessions"))?;
        for id in sql(query.query_map([], |r| r.get::<_, String>(0)))? {
            store.shared_register(&sql(id)?)?;
        }
        drop(query);
        Ok(store)
    }
    /// A read-only snapshot of desktop preferences, shared by this store's
    /// consumers. Reopening the store refreshes it without per-message WebKit I/O.
    pub fn desktop_preferences(&self) -> &crate::desktop_preferences::DesktopPreferences {
        &self.shared.preferences
    }

    pub fn search_reader(&self) -> Result<Option<Self>, String> {
        if self.path == Path::new(":memory:") {
            return Ok(None);
        }
        let reader = Self::open_shared(&self.path, true, Some(self.shared.preferences.clone()))?;
        #[cfg(any(test, feature = "test-support"))]
        let reader = Self {
            test_directory: self.test_directory.clone(),
            ..reader
        };
        Ok(Some(reader))
    }
    /// Resolve only a persisted identity; unlike CLI handle parsing this never
    /// treats a stale numeric-looking desktop ID as an existing session.
    pub fn resolve_existing_session_id(&self, id: &str) -> Option<i64> {
        let exists = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
                [id],
                |r| r.get::<_, bool>(0),
            )
            .ok()?;
        exists.then(|| self.shared_register(id).ok()).flatten()
    }
    /// UI handles are stable hashes; the database always retains its original IDs.
    pub fn persisted_session_id(&self, id: i64) -> Result<String, String> {
        let shared = &self.shared;
        shared
            .ids
            .borrow()
            .get(&id)
            .cloned()
            .ok_or_else(|| "Session no longer exists".into())
    }
    pub fn resolve_session_id(&self, id: &str) -> Result<i64, String> {
        {
            let shared = &self.shared;
            if let Some((key, _)) = shared
                .ids
                .borrow()
                .iter()
                .find(|(_, value)| value.as_str() == id)
            {
                return Ok(*key);
            }
            if sql(self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
                [id],
                |r| r.get::<_, bool>(0),
            ))? {
                return self.shared_register(id);
            }
        }
        id.parse::<i64>().map_err(|_| "Unknown session ID".into())
    }
    pub(super) fn shared_register(&self, id: &str) -> Result<i64, String> {
        let key = handle(id);
        let mut ids = self.shared.ids.borrow_mut();
        if ids.get(&key).is_some_and(|existing| existing != id) {
            return Err("Session identity collision; the database has not been modified".into());
        }
        ids.insert(key, id.into());
        Ok(key)
    }
    pub(super) fn shared_forget_uncommitted_identity(&self, id: i64) {
        let shared = &self.shared;
        shared.ids.borrow_mut().remove(&id);
    }
    pub(super) fn shared_writable(&self) -> Result<(), String> {
        if self.shared.read_only {
            Err("This vorkspace store is read-only".into())
        } else {
            Ok(())
        }
    }
    pub(super) fn shared_field(&self, name: &str, fallback: &str) -> String {
        if self.shared.columns.contains(name) {
            name.into()
        } else {
            fallback.into()
        }
    }
    pub(super) fn shared_patch(&self, id: i64, fields: &[(&str, Value)]) -> Result<(), String> {
        self.shared_writable()?;
        let id = self.persisted_session_id(id)?;
        let fields: Vec<_> = fields
            .iter()
            .filter(|(name, _)| self.shared.columns.contains(*name))
            .collect();
        if fields.is_empty() {
            return Ok(());
        }
        let updates = fields
            .iter()
            .enumerate()
            .map(|(ix, (name, _))| format!("{name}=?{}", ix + 2))
            .collect::<Vec<_>>()
            .join(",");
        let mut values = vec![rusqlite::types::Value::Text(id)];
        values.extend(fields.iter().map(|(_, v)| match v {
            Value::Null => rusqlite::types::Value::Null,
            Value::Bool(v) => rusqlite::types::Value::Integer(i64::from(*v)),
            Value::Number(v) => rusqlite::types::Value::Integer(v.as_i64().unwrap_or(i64::MAX)),
            Value::String(v) => rusqlite::types::Value::Text(v.clone()),
            v => rusqlite::types::Value::Text(v.to_string()),
        }));
        sql(self.conn.execute(
            &format!("UPDATE sessions SET {updates} WHERE id=?1"),
            rusqlite::params_from_iter(values),
        ))?;
        Ok(())
    }
    pub fn setting(&self, key: &str) -> Result<Option<String>, String> {
        if key.starts_with("bot-draft-origin:")
            && has_table(&self.conn, "native_bot_draft_origins")?
        {
            let value: Option<String> = sql(self
                .conn
                .query_row(
                    "SELECT value FROM native_bot_draft_origins WHERE key=?1",
                    [key],
                    |row| row.get(0),
                )
                .optional())?;
            if value.is_some() {
                return Ok(value);
            }
        }
        if let Some((id, message)) = key
            .strip_prefix("bot-delivered:")
            .and_then(|v| v.split_once(':'))
        {
            if let Ok(id) = id.parse::<i64>() {
                if self
                    .shared_message_receipts(&self.persisted_session_id(id)?, &[message])?
                    .contains(message)
                {
                    return Ok(Some("1".into()));
                }
            }
        }
        let shared = &self.shared;
        shared.settings.read(|settings| settings.get(key).cloned())
    }
    /// Persist a dispatch claim before any provider effect. Claims are never
    /// leased or automatically retried: a crash leaves an ambiguous outcome.
    pub fn claim_bot_message(
        &self,
        bot: &str,
        message: &crate::bot::Message,
        token: &str,
    ) -> Result<bool, String> {
        self.shared_writable()?;
        let shared = &self.shared;
        // Transactions precede the sidecar lock everywhere both are needed.
        // Keep both through validation and commit so roster publication cannot
        // race the claim, including across independent processes.
        let tx = sql(rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        ))?;
        let _lock = shared.settings.lock()?;
        shared.settings.read(|settings| {
            super::validate_bot_claim(
                settings
                    .get("bot-roster")
                    .ok_or("Bot outbox is unavailable")?,
                bot,
                message,
            )
        })??;
        incremental::ensure_bookkeeping_schema(&tx)?;
        let inserted = sql(tx.execute(
            "INSERT OR IGNORE INTO native_bot_dispatch_claims VALUES(?1,?2,?3)",
            params![bot, message.id, token],
        ))? == 1;
        sql(tx.commit())?;
        Ok(inserted)
    }
    /// Replace one setting only if it still matches the value the caller read.
    pub fn compare_swap_setting(
        &self,
        key: &str,
        expected: Option<&str>,
        value: &str,
    ) -> Result<(), String> {
        self.mark_attachment_activity();

        self.shared_writable()?;
        if key.starts_with("bot-draft-origin:") {
            let tx = sql(rusqlite::Transaction::new_unchecked(
                &self.conn,
                rusqlite::TransactionBehavior::Immediate,
            ))?;
            let current = self.setting(key)?;
            if current.as_deref() != expected {
                return Err("Setting changed in another window; your changes were preserved in this window.".into());
            }
            incremental::ensure_bookkeeping_schema(&tx)?;
            sql(tx.execute("INSERT INTO native_bot_draft_origins(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key,value]))?;
            return sql(tx.commit());
        }
        let shared = &self.shared;
        let _lock = shared.settings.lock()?;
        let mut next = self.shared_settings_snapshot()?;
        if next.get(key).map(String::as_str) != expected {
            return Err(
                "Setting changed in another window; your changes were preserved in this window."
                    .into(),
            );
        }
        #[cfg(any(test, feature = "test-support"))]
        self.check_setting_for_test(key, value, next.get(key).map(String::as_str))?;
        next.insert(key.into(), value.into());
        shared.settings.save(next)
    }
    pub fn save_setting(&self, key: &str, value: &str) -> Result<(), String> {
        self.mark_attachment_activity();

        if key.starts_with("bot-draft-origin:") {
            self.shared_writable()?;
            incremental::ensure_bookkeeping_schema(&self.conn)?;
            self.conn.execute("INSERT INTO native_bot_draft_origins(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key,value]).map_err(|e| e.to_string())?;
            return Ok(());
        }
        self.update_settings(|_| Ok(((), vec![(key.into(), value.into())])))
    }
    /// Publish related settings together, preserving their previous values if
    /// any write fails. The desktop sidecar publishes one atomic root.
    pub fn save_settings(&self, values: &[(&str, &str)]) -> Result<(), String> {
        self.mark_attachment_activity();

        self.update_settings(|_| {
            Ok((
                (),
                values
                    .iter()
                    .map(|&(key, value)| (key.into(), value.into()))
                    .collect(),
            ))
        })
    }
    pub(super) fn shared_settings_snapshot(&self) -> Result<BTreeMap<String, String>, String> {
        let shared = &self.shared;
        shared.settings.read(Clone::clone)
    }
    /// Read and publish related sidecar settings while holding the same lock.
    pub(in crate::store) fn update_settings<T>(
        &self,
        update: impl FnOnce(&BTreeMap<String, String>) -> Result<(T, Vec<(String, String)>), String>,
    ) -> Result<T, String> {
        self.shared_writable()?;
        let shared = &self.shared;
        let _lock = shared.settings.lock()?;
        let mut next = self.shared_settings_snapshot()?;
        let (result, values) = update(&next)?;
        #[cfg(any(test, feature = "test-support"))]
        for (key, value) in &values {
            self.check_setting_for_test(key, value, next.get(key).map(String::as_str))?;
        }
        let changed = values
            .iter()
            .any(|(key, value)| next.get(key) != Some(value));
        if changed {
            self.mark_attachment_activity();
            next.extend(values);
            shared.settings.save(next)?;
        }
        Ok(result)
    }
    pub(super) fn read_sessions(&self) -> Result<Vec<Session>, String> {
        let user_presence = self.shared_has_user_message()?;
        let columns = [
            "id",
            "title",
            "harness",
            "cwd",
            "provider_session_id",
            "updated_at",
        ]
        .into_iter()
        .map(str::to_string)
        .chain(
            [
                ("has_user_message", user_presence.as_str()),
                ("archived", "0"),
                ("pinned", "0"),
                ("linked_work_item_json", "NULL"),
                ("sidebar_hidden", "0"),
            ]
            .into_iter()
            .map(|(name, default)| self.shared_field(name, default)),
        )
        .collect::<Vec<_>>()
        .join(",");
        let index = if sql(self.conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='sessions_cwd_cover_idx')",[],|r|r.get::<_,bool>(0)))? {
            " INDEXED BY sessions_cwd_cover_idx"
        } else { "" };
        let mut filters = vec!["1=1".to_string()];
        if self.shared.columns.contains("inbox_ask") {
            filters.push("id NOT IN (SELECT id FROM sessions WHERE inbox_ask IS NOT NULL)".into());
        }
        if has_table(&self.conn, "orchestration_workers")? {
            filters.push("id NOT IN (SELECT session_id FROM orchestration_workers)".into());
        }
        let mut query = sql(self.conn.prepare(&format!(
            "SELECT {columns} FROM sessions{index} WHERE {} ORDER BY updated_at DESC,id",
            filters.join(" AND ")
        )))?;
        let mut rows = sql(query.query([]))?;
        let mut result = Vec::new();
        let mut hidden = HashSet::new();
        let mut originals = HashMap::new();
        while let Some(row) = sql(rows.next())? {
            let original: String = sql(row.get(0))?;
            let id = self.shared_register(&original)?;
            if sql(row.get::<_, bool>(10))? {
                hidden.insert(id);
            }
            originals.insert(id, original);
            result.push(Session {
                id,
                title: sql(row.get(1))?,
                provider: sql(row.get(2))?,
                cwd: sql(row.get(3))?,
                provider_id: sql(row.get(4))?,
                updated_at: sql(row.get::<_, i64>(5))?.max(0) as u64,
                has_messages: sql(row.get(6))?,
                archived: sql(row.get(7))?,
                pinned: sql(row.get(8))?,
                work_item: sql(row.get::<_, Option<String>>(9))?.and_then(|raw| parse(&raw).ok()),
            });
        }
        // Keep the source sidebar's compatibility exception for hidden
        // /operator launches. A Bot's own hidden conversation cannot be a
        // listed caller. The UI also checks project/worktree and Bot scope.
        let shown: HashMap<_, _> = result
            .iter()
            .filter(|s| !hidden.contains(&s.id) && s.has_messages)
            .map(|s| (originals[&s.id].clone(), s.cwd.clone()))
            .collect();
        let shown_ids = shown.keys().cloned().collect();
        result.retain(|s| {
            !hidden.contains(&s.id)
                || started_caller(&originals[&s.id], &shown_ids)
                    .is_some_and(|caller| shown[caller] == s.cwd)
        });
        Ok(result)
    }
    pub fn create(&self, provider: &str, cwd: &str) -> Result<Session, String> {
        self.check_project_creation(cwd)?;

        self.shared_writable()?;
        let label = crate::kind::Kind::parse(provider)
            .ok_or_else(|| format!("Unknown provider: {provider}"))?
            .label();
        let original = crate::accounts::request_id();
        let now = crate::time::now_ms().min(i64::MAX as u64) as i64;
        let title = format!("New {label} session");
        sql(self.conn.execute("INSERT INTO sessions(id,cwd,harness,model,model_settings,runtime_mode,title,blocks_json,created_at,updated_at)
            VALUES(?1,?2,?3,?4,'{}','supervised',?5,'[]',?6,?6)",
            params![original,cwd,provider,format!("{provider}:default"),title,now]))?;
        Ok(Session {
            id: self.shared_register(&original)?,
            title,
            provider: provider.into(),
            cwd: cwd.into(),
            provider_id: None,
            has_messages: false,
            updated_at: now as u64,
            archived: false,
            pinned: false,
            work_item: None,
        })
    }
    /// Add native turn receipts to their original desktop prompt objects.
    /// Unknown rich fields and existing provenance are retained verbatim.
    pub(super) fn shared_save_turns(
        &self,
        id: i64,
        turns: &HashMap<usize, crate::agent::TurnInfo>,
        writer: Option<&mut TurnHistory>,
    ) -> Result<bool, String> {
        if turns.is_empty() {
            return Ok(true);
        }
        self.shared_writable()?;
        let tx = sql(rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        ))?;
        let mut blocks = self.shared_blocks(id)?;
        if let Some(writer) = writer.as_ref() {
            if !self.shared_writer_matches(id, &blocks, writer)? {
                return Ok(false);
            }
        } else {
            self.shared_validate_history_or_fast_forward(id, &blocks)?;
        }
        let (session_provider, session_model): (String, String) = sql(self.conn.query_row(
            "SELECT harness,model FROM sessions WHERE id=?1",
            [self.persisted_session_id(id)?],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ))?;
        let mut offset = 0;
        let mut changed = false;
        for block in &mut blocks {
            let rows = blocks::project(std::slice::from_ref(block))?;
            let turn = rows
                .iter()
                .rposition(Row::is_prompt)
                .map(|prompt| offset + prompt + 1);
            offset += rows.len();
            let Some(info) = turn.and_then(|turn| turns.get(&turn)) else {
                continue;
            };
            let mut fields = Vec::new();
            if !info.provider.is_empty() && (!info.model.is_empty() || info.model_id.is_some()) {
                let mut model = json!({"harness":info.provider});
                if !info.model.is_empty() {
                    model["name"] = json!(info.model);
                }
                if let Some(id) = &info.model_id {
                    model["id"] = json!(if info.provider == session_provider
                        && crate::session::native_model(&session_model, &session_provider).as_ref()
                            == Some(id)
                    {
                        session_model.clone()
                    } else if id.starts_with(&format!("{}:", info.provider)) {
                        id.clone()
                    } else {
                        format!("{}:{id}", info.provider)
                    });
                }
                fields.push(("turnModel", model));
            }
            if info.has_duration() {
                fields.push(("durationMs", json!(info.duration_ms)));
            }
            if let Some(id) = &info.provider_turn_id {
                fields.push(("providerTurnId", json!(id)));
            }
            if let Some(at) = info.started_at {
                // A native root prompt is written with its send time before
                // its turn receipt arrives; the desktop schema calls it startedAt.
                if block["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("gpui-"))
                    && block["startedAt"].is_null()
                {
                    block["startedAt"] = json!(at);
                    if block["sentAt"].as_u64() == Some(at) {
                        block.as_object_mut().unwrap().remove("sentAt");
                    }
                    changed = true;
                }
            }
            if let Some(metrics) = info.metrics.filter(|metrics| metrics.reported()) {
                fields.push(("turnMetrics", json!({"inputTokens":metrics.input_tokens,
                    "outputTokens":metrics.output_tokens,"cacheReadTokens":metrics.cache_read_tokens,
                    "cacheWriteTokens":metrics.cache_write_tokens,"cacheHitPercent":metrics.cache_hit_percent})));
            }
            for (key, value) in fields {
                if block[key].is_null() {
                    block[key] = value;
                    changed = true;
                } else if key == "turnModel" {
                    if let (Some(saved), Some(incoming)) =
                        (block[key].as_object_mut(), value.as_object())
                    {
                        for (name, value) in incoming {
                            if !saved.contains_key(name) {
                                saved.insert(name.clone(), value.clone());
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        if changed {
            self.shared_store_blocks(id, &blocks)?;
        }
        let writer_state = if writer.is_some() && changed {
            Some((
                self.shared_turn_version(id)?,
                TurnHistory::fingerprints(&blocks::project(&blocks)?).collect(),
            ))
        } else {
            None
        };
        sql(tx.commit())?;
        if let (Some(writer), Some((version, rows))) = (writer, writer_state) {
            writer.committed(version, 0, rows);
        }
        if changed {
            self.shared
                .snapshots
                .borrow_mut()
                .insert(id, HistoryRevision::of(&blocks));
        }
        Ok(true)
    }

    fn shared_writer_matches(
        &self,
        id: i64,
        blocks: &[Value],
        writer: &TurnHistory,
    ) -> Result<bool, String> {
        let version = self.shared_turn_version(id)?;
        // Metadata or an unrelated migration may advance the scalar. Row
        // equality still proves that the writer's numeric offsets are valid.
        Ok((version.is_some() && writer.version == version)
            || writer.matches(&blocks::project(blocks)?))
    }

    pub fn context_usage(&self, id: i64) -> Result<Option<Usage>, String> {
        let fields = format!(
            "{},{}",
            self.shared_field("context_used", "NULL"),
            self.shared_field("context_window", "NULL")
        );
        let (used, window) = sql(self.conn.query_row(
            &format!("SELECT {fields} FROM sessions WHERE id=?1"),
            [self.persisted_session_id(id)?],
            |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?)),
        ))?;
        Ok(used.filter(|u| *u >= 0).map(|used| Usage {
            used: used as u64,
            window: window.filter(|w| *w > 0).map(|w| w as u64),
        }))
    }
    pub(super) fn shared_blocks(&self, id: i64) -> Result<Vec<Value>, String> {
        let original = self.persisted_session_id(id)?;
        if let Some(tables) = self.shared_bot_tables(&original)? {
            let mut query = sql(self.conn.prepare(&format!(
                "SELECT block_json FROM {} WHERE session_id=?1 ORDER BY seq",
                tables.blocks
            )))?;
            let mut result = Vec::new();
            for row in sql(query.query_map([&original], |r| r.get::<_, String>(0)))? {
                result.push(parse_block(&sql(row)?)?);
            }
            Ok(result)
        } else if self.shared_ordinary_indexed(&original)? {
            self.shared_ordinary_blocks(&original)
        } else {
            let raw: String = sql(self.conn.query_row(
                "SELECT blocks_json FROM sessions WHERE id=?1",
                [original],
                |r| r.get(0),
            ))?;
            parse_blocks(&raw)
        }
    }
    fn shared_indexed_query(&self, original: &str) -> Result<Option<String>, String> {
        let query = if let Some(tables) = self.shared_bot_tables(original)? {
            Some(format!(
                "SELECT block_json,(SELECT count(*) FROM {table} WHERE session_id=?1) FROM {table} WHERE session_id=?1 ORDER BY seq",
                table = tables.blocks
            ))
        } else if self.shared_ordinary_indexed(original)? {
            Some(
                "SELECT block_json,(SELECT count(*) FROM ordinary_blocks WHERE session_id=?1)
                 FROM ordinary_blocks WHERE session_id=?1 ORDER BY position"
                    .to_owned(),
            )
        } else {
            None
        };
        Ok(query)
    }

    fn shared_project_turns(
        &self,
        id: i64,
    ) -> Result<HashMap<usize, crate::agent::TurnInfo>, String> {
        let original = self.persisted_session_id(id)?;
        let Some(query) = self.shared_indexed_query(&original)? else {
            return project_turns(&self.shared_blocks(id)?);
        };
        let mut query = sql(self.conn.prepare(&query))?;
        let mut records = sql(query.query([&original]))?;
        let mut turns = HashMap::new();
        let mut offset = 0;
        while let Some(record) = sql(records.next())? {
            let raw: String = sql(record.get(0))?;
            let block: Value = parse_block(&raw)?;
            offset += project_turn(&block, offset, &mut turns)?;
        }
        Ok(turns)
    }

    pub fn load(&self, id: i64) -> Result<Transcript, String> {
        self.shared_load_projected(id, None)
    }

    pub(super) fn shared_load_projected(
        &self,
        id: i64,
        mut turns: Option<&mut HashMap<usize, crate::agent::TurnInfo>>,
    ) -> Result<Transcript, String> {
        self.shared.refreshed_cards.borrow_mut().remove(&id);
        self.shared.card_revisions.borrow_mut().remove(&id);
        let original = self.persisted_session_id(id)?;
        let indexed_query = self.shared_indexed_query(&original)?;
        let mut rows = Vec::new();
        let mut append = |block: &Value| {
            let projected = blocks::project_block(block)?;
            if let Some(turns) = turns.as_deref_mut() {
                record_projected_turn(block, &projected, rows.len(), turns);
            }
            rows.extend(projected);
            Ok::<_, String>(())
        };
        let revision = if let Some(query) = indexed_query {
            // Project one source block at a time. Indexed conversations must
            // not retain complete JSON histories alongside their display rows.
            // The scalar count and the rows share one SQLite read snapshot.
            let mut query = sql(self.conn.prepare(&query))?;
            let mut records = sql(query.query([&original]))?;
            let mut digest = HistoryRevision::start(0);
            let mut len = 0;
            let mut first = true;
            while let Some(record) = sql(records.next())? {
                if first {
                    len = sql(record.get::<_, i64>(1))? as usize;
                    digest = HistoryRevision::start(len);
                    first = false;
                }
                let raw: String = sql(record.get(0))?;
                let block: Value = parse_block(&raw)?;
                HistoryRevision::push(&mut digest, &block);
                append(&block)?;
            }
            HistoryRevision::finish(digest, len)
        } else {
            let blocks = self.shared_blocks(id)?;
            for block in &blocks {
                append(block)?;
            }
            HistoryRevision::of(&blocks)
        };
        self.shared.snapshots.borrow_mut().insert(id, revision);
        Ok(Transcript { rows })
    }
    pub(super) fn shared_store_blocks(&self, id: i64, blocks: &[Value]) -> Result<(), String> {
        let original = self.persisted_session_id(id)?;
        if self.shared_bot_tables(&original)?.is_some() {
            self.shared_store_indexed_blocks(&original, blocks)?;
        } else if self.shared_ordinary_indexed(&original)? {
            self.shared_store_ordinary_blocks(&original, blocks)?;
        } else if has_table(&self.conn, "session_history_revisions")? {
            self.shared_normalize_blocks(&original, blocks)?;
        } else {
            self.shared_patch(id, &[("blocks_json", json!(encode(&blocks)?))])?;
        }
        self.shared_patch(
            id,
            &[
                (
                    "has_user_message",
                    json!(blocks.iter().any(|b| b["role"] == "user")),
                ),
                (
                    "is_draft",
                    json!(blocks
                        .iter()
                        .any(|b| b["role"] == "user" && b["draft"] == true)),
                ),
            ],
        )
    }
    fn shared_store_indexed_blocks(&self, original: &str, blocks: &[Value]) -> Result<(), String> {
        let tables = self
            .shared_bot_tables(original)?
            .ok_or("Bot transcript owner disappeared; its data was preserved")?;
        let mut query = sql(self.conn.prepare(&format!(
            "SELECT seq,block_id,block_json FROM {} WHERE session_id=?1 ORDER BY seq",
            tables.blocks
        )))?;
        let mut old = HashMap::new();
        let mut max_seq = 0;
        for row in sql(query.query_map([original], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        }))? {
            let (seq, id, raw) = sql(row)?;
            max_seq = max_seq.max(seq);
            old.insert(id, (seq, parse_block(&raw)?));
        }
        let retained: HashSet<_> = blocks.iter().filter_map(|b| b["id"].as_str()).collect();
        if retained.len() != blocks.len() {
            return Err("Indexed conversation contains missing or duplicate block IDs".into());
        }
        let mut previous = 0;
        let mut appended = false;
        let mut resequence = false;
        for block in blocks {
            if let Some((seq, _)) = old.get(block["id"].as_str().unwrap()) {
                if appended || *seq <= previous {
                    resequence = true;
                }
                previous = *seq;
            } else {
                appended = true;
            }
        }
        if resequence {
            // An explicit insertion/reorder changes turn positions. Payloads
            // and block identities survive, in one enclosing transaction.
            sql(self.conn.execute(
                &format!(
                    "DELETE FROM {turns} WHERE session_id=?1",
                    turns = tables.turns
                ),
                [original],
            ))?;
            sql(self.conn.execute(
                &format!(
                    "DELETE FROM {blocks} WHERE session_id=?1",
                    blocks = tables.blocks
                ),
                [original],
            ))?;
            old.clear();
            max_seq = 0;
        } else {
            for (id, (seq, _)) in &old {
                if !retained.contains(id.as_str()) {
                    sql(self.conn.execute(
                        &format!(
                            "DELETE FROM {turns} WHERE session_id=?1 AND seq=?2",
                            turns = tables.turns
                        ),
                        params![original, seq],
                    ))?;
                    sql(self.conn.execute(
                        &format!(
                            "DELETE FROM {blocks} WHERE session_id=?1 AND block_id=?2",
                            blocks = tables.blocks
                        ),
                        params![original, id],
                    ))?;
                }
            }
        }
        let mut after_handoff = false;
        for (ix, block) in blocks.iter().enumerate() {
            let id = block["id"].as_str().unwrap();
            let (seq, changed) = match old.get(id) {
                Some((seq, old)) => (*seq, old != block),
                None => {
                    max_seq += 1;
                    (max_seq, true)
                }
            };
            if changed {
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
                sql(self.conn.execute(&format!("INSERT INTO {table}(session_id,seq,block_id,block_json,search_text,is_user,is_draft)
                    VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(session_id,seq) DO UPDATE SET block_json=excluded.block_json,
                    search_text=excluded.search_text,is_user=excluded.is_user,is_draft=excluded.is_draft",table=tables.blocks),
                    params![original,seq,id,encode(block)?,search,block["role"]=="user",block["draft"]==true]))?;
            }
            if ix == 0
                || after_handoff
                || (block["role"] == "user" && block["internal"] != true)
                || block.get("botHabit").is_some()
            {
                sql(self.conn.execute(
                    &format!(
                        "INSERT OR IGNORE INTO {turns}(session_id,seq) VALUES(?1,?2)",
                        turns = tables.turns
                    ),
                    params![original, seq],
                ))?;
            }
            after_handoff = block["role"] == "handoff";
        }
        Ok(())
    }
    pub(super) fn shared_write_rows(
        &self,
        id: i64,
        start: usize,
        rows: &[Row],
        message_id: Option<&str>,
        mut writer: Option<&mut TurnHistory>,
    ) -> Result<bool, String> {
        self.shared_writable()?;
        self.shared_history_schema()?;
        if message_id.is_none()
            && self.shared_write_incremental(id, start, rows, writer.as_deref_mut())?
        {
            return Ok(true);
        }
        let tx = sql(rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        ))?;
        let current = self.shared_blocks(id)?;
        if let Some(writer) = writer.as_ref() {
            if !self.shared_writer_matches(id, &current, writer)? {
                return Ok(false);
            }
        } else {
            self.shared_validate_history(id, &current)?;
        }
        self.shared_card_rows_current(id, rows)?;
        let mut next = blocks::merge(&current, start, rows)?;
        if let Some(message) = message_id {
            if let Some(prompt) = next
                .iter_mut()
                .rev()
                .find(|b| b["role"] == "user" && b["draft"] != true)
            {
                prompt["appRequestId"] = json!(message);
            }
            next.retain(|b| !(b["draft"] == true && b["id"].as_str() == Some(message)));
        }
        self.shared_store_blocks(id, &next)?;
        let mut next_queue = None;
        if let Some(message) = message_id {
            let field = self.shared_field("queued_messages_json", "'[]'");
            let raw: String = sql(self.conn.query_row(
                &format!("SELECT {field} FROM sessions WHERE id=?1"),
                [self.persisted_session_id(id)?],
                |r| r.get(0),
            ))?;
            let mut queued: Vec<Value> = parse(&raw)?;
            queued.retain(|m| m["id"].as_str() != Some(message));
            let encoded = encode(&queued)?;
            self.shared_patch(id, &[("queued_messages_json", json!(encoded))])?;
            next_queue = Some(encoded);
        }

        if !rows.is_empty() {
            self.shared_patch(id, &[("updated_at", json!(crate::time::now_ms()))])?;
        }
        let index = self.shared_build_history_index(id, &next)?;
        let writer_state = if writer.is_some() {
            Some((
                self.shared_turn_version(id)?,
                TurnHistory::fingerprints(&blocks::project(&next)?).collect(),
            ))
        } else {
            None
        };
        sql(tx.commit())?;
        if let (Some(writer), Some((version, rows))) = (writer, writer_state) {
            writer.committed(version, 0, rows);
        }
        if let Some(queue) = next_queue {
            self.shared.queues.borrow_mut().insert(id, queue);
        }
        let shared = &self.shared;
        let mut indices = shared.incremental.borrow_mut();
        let revision = if let Some(index) = index {
            let revision = HistoryRevision::Stored(index.revision);
            indices.insert(id, index);
            revision
        } else {
            indices.remove(&id);
            HistoryRevision::of(&next)
        };
        shared.snapshots.borrow_mut().insert(id, revision);
        self.shared.card_revisions.borrow_mut().remove(&id);
        // The source appRequestId is the durable, transactional receipt.
        Ok(true)
    }
    pub fn reset_provider_id_if(&self, id: i64, expected: Option<&str>) -> Result<(), String> {
        self.shared_writable()?;
        sql(self.conn.execute("UPDATE sessions SET provider_session_id=NULL WHERE id=?1 AND provider_session_id IS ?2",
            params![self.persisted_session_id(id)?,expected]))?;
        Ok(())
    }
    pub fn change_provider(&self, id: i64, provider: &str) -> Result<(), String> {
        self.shared_writable()?;
        sql(self.conn.execute(
            "UPDATE sessions SET harness=?2,provider_session_id=NULL,model_settings='{}' WHERE id=?1 AND harness<>?2",
            params![self.persisted_session_id(id)?, provider],
        ))?;
        Ok(())
    }
    /// A late generated title cannot replace a manual rename or recreate a deleted session.
    pub fn replace_title(&self, id: i64, expected: &str, title: &str) -> Result<bool, String> {
        self.shared_writable()?;
        sql(self.conn.execute(
            "UPDATE sessions SET title=?3 WHERE id=?1 AND title=?2",
            params![self.persisted_session_id(id)?, expected, title],
        ))
        .map(|n| n == 1)
    }
    /// Read workspace membership without loading every session's full agent settings.
    pub fn session_worktrees(&self) -> Result<HashMap<i64, Option<String>>, String> {
        let mut query = sql(self.conn.prepare(&format!(
            "SELECT id,{} FROM sessions",
            self.shared_field("worktree_cwd", "NULL")
        )))?;
        let mut out = HashMap::new();
        for row in sql(query.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        }))? {
            let (id, path) = sql(row)?;
            out.insert(self.shared_register(&id)?, path);
        }
        Ok(out)
    }
    pub(super) fn shared_sidebar_hidden(&self) -> Result<HashSet<i64>, String> {
        let mut query = sql(self.conn.prepare(&format!(
            "SELECT id FROM sessions WHERE {}!=0",
            self.shared_field("sidebar_hidden", "0")
        )))?;
        let mut out = HashSet::new();
        for id in sql(query.query_map([], |r| r.get::<_, String>(0)))? {
            out.insert(self.shared_register(&sql(id)?)?);
        }
        Ok(out)
    }
    /// Reuse the persisted child parser from shared_started_links without its
    /// sidebar visibility filter: a legitimate Bot caller may itself be hidden.
    /// The command owner separately validates the caller's project relationship.
    pub(super) fn shared_app_launch_caller(&self, id: i64) -> Result<Option<i64>, String> {
        let original = self.persisted_session_id(id)?;
        if !original.starts_with("app-") {
            return Ok(None);
        }
        let mut query = sql(self.conn.prepare("SELECT id FROM sessions"))?;
        let ids = sql(query.query_map([], |row| row.get::<_, String>(0)))?
            .collect::<rusqlite::Result<HashSet<_>>>()
            .map_err(|e| e.to_string())?;
        started_caller(&original, &ids)
            .map(|caller| self.shared_register(caller))
            .transpose()
    }
    pub(super) fn shared_started_links(&self) -> Result<HashMap<i64, i64>, String> {
        let mut query = sql(self.conn.prepare(&format!(
            "SELECT id,{} FROM sessions",
            self.shared_field("sidebar_hidden", "0")
        )))?;
        let rows: Vec<(String, bool)> = sql(query.query_map([], |r| Ok((r.get(0)?, r.get(1)?))))?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())?;
        let ids = rows.iter().map(|(id, _)| id.clone()).collect();
        let shown = rows
            .iter()
            .filter(|(_, hidden)| !hidden)
            .map(|(id, _)| id.clone())
            .collect();
        rows.iter()
            .filter_map(|(id, hidden)| {
                started_caller(id, if *hidden { &shown } else { &ids }).map(|caller| (id, caller))
            })
            .map(|(id, caller)| Ok((self.shared_register(id)?, self.shared_register(caller)?)))
            .collect()
    }
    pub fn set_pinned(&self, ids: &[i64], pinned: bool) -> Result<(), String> {
        let tx = sql(self.conn.unchecked_transaction())?;
        for id in ids {
            self.shared_patch(*id, &[("pinned", json!(pinned))])?;
        }
        sql(tx.commit())
    }
    /// `prefix<session id>` settings, by session id.
    pub(in crate::store) fn settings_with_prefix(
        &self,
        prefix: &str,
    ) -> Result<Vec<(i64, String)>, String> {
        let shared = &self.shared;
        shared.settings.read(|settings| {
            settings
                .range(prefix.to_owned()..)
                .take_while(|(key, _)| key.starts_with(prefix))
                .filter_map(|(key, value)| {
                    Some((key.strip_prefix(prefix)?.parse().ok()?, value.clone()))
                })
                .collect()
        })
    }
    /// Sessions whose latest message is saved as a draft and not yet sent.
    pub(crate) fn draft_sessions(&self) -> Result<HashSet<i64>, String> {
        let mut query = sql(self.conn.prepare(&format!(
            "SELECT id FROM sessions WHERE {}!=0",
            self.shared_field("is_draft", "0")
        )))?;
        let mut out = HashSet::new();
        for id in sql(query.query_map([], |r| r.get::<_, String>(0)))? {
            out.insert(self.shared_register(&sql(id)?)?);
        }
        Ok(out)
    }
    pub fn ui_state(&self) -> Result<Option<String>, String> {
        let saved = self.setting("workspace")?;
        let workspace = saved.as_deref().map(parse::<Value>).transpose()?;
        let mut merged = workspace.clone().unwrap_or_else(|| json!({}));
        self.desktop_preferences().apply_workspace(&mut merged);
        if merged.as_object().is_some_and(|object| !object.is_empty()) {
            Ok(Some(merged.to_string()))
        } else {
            Ok(workspace.map(|value| value.to_string()))
        }
    }
    pub fn reminders(&self) -> Result<Vec<Reminder>, String> {
        if !has_table(&self.conn, "session_reminders")? {
            return Ok(Vec::new());
        }
        let mut query = sql(self.conn.prepare(
            "SELECT session_id,due_at,fired_at FROM session_reminders ORDER BY due_at,session_id",
        ))?;
        let mut out = Vec::new();
        for row in sql(query.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        }))? {
            let (id, due, fired) = sql(row)?;
            out.push(Reminder {
                session_id: self.shared_register(&id)?,
                due_at: due.max(0) as u64,
                fired_at: fired.map(|at| at.max(0) as u64),
            });
        }
        Ok(out)
    }
    /// Schedules (or reschedules, re-arming) each session's reminder.
    pub fn set_reminders(&self, ids: &[i64], due: u64) -> Result<(), String> {
        self.shared_writable()?;
        let tx = sql(self.conn.unchecked_transaction())?;
        for id in ids {
            sql(self.conn.execute(
                "INSERT INTO session_reminders(session_id,due_at,fired_at) VALUES(?1,?2,NULL)
                ON CONFLICT(session_id) DO UPDATE SET due_at=excluded.due_at,fired_at=NULL",
                params![
                    self.persisted_session_id(*id)?,
                    due.min(i64::MAX as u64) as i64
                ],
            ))?;
        }
        sql(tx.commit())
    }
    /// Re-arms only the reminder occurrence the user snoozed. A stale popup
    /// must not overwrite a newer schedule created in another window.
    pub fn snooze_reminder_if(
        &self,
        id: i64,
        expected_due: u64,
        new_due: u64,
    ) -> Result<bool, String> {
        self.shared_writable()?;
        sql(self.conn.execute("UPDATE session_reminders SET due_at=?3,fired_at=NULL WHERE session_id=?1 AND due_at=?2",
            params![self.persisted_session_id(id)?, expected_due.min(i64::MAX as u64) as i64, new_due.min(i64::MAX as u64) as i64])).map(|changed|changed==1)
    }
    /// Clears the sessions' reminders; with `expected`, only that one, so a
    /// stale delivery never clears a newer reminder.
    pub fn clear_reminders(&self, ids: &[i64], expected: Option<u64>) -> Result<(), String> {
        self.shared_writable()?;
        for id in ids {
            sql(self.conn.execute(
                "DELETE FROM session_reminders WHERE session_id=?1 AND (?2 IS NULL OR due_at=?2)",
                params![
                    self.persisted_session_id(*id)?,
                    expected.map(|at| at.min(i64::MAX as u64) as i64)
                ],
            ))?;
        }
        Ok(())
    }
    pub fn claim_due_reminders(
        &self,
        snapshot: &[Reminder],
        now: u64,
    ) -> Result<Vec<Reminder>, String> {
        let due: Vec<_> = snapshot
            .iter()
            .filter(|r| r.due_at <= now && r.fired_at.is_none())
            .cloned()
            .collect();
        if due.is_empty() {
            return Ok(Vec::new());
        }

        self.shared_writable()?;
        let tx = sql(self.conn.unchecked_transaction())?;
        let mut fired = Vec::new();
        for reminder in &due {
            let changed=sql(self.conn.execute("UPDATE session_reminders SET fired_at=?2 WHERE session_id=?1 AND fired_at IS NULL AND due_at=?3",
                params![self.persisted_session_id(reminder.session_id)?, now.min(i64::MAX as u64) as i64, reminder.due_at.min(i64::MAX as u64) as i64]))?;
            if changed == 1 {
                fired.push(reminder.clone());
            }
        }
        sql(tx.commit())?;
        Ok(fired)
    }
    fn shared_delete_session_settings(&self, id: i64) -> Result<(), String> {
        let shared = &self.shared;
        let exact = [
            "agent",
            "context",
            "follow-up",
            "started-by",
            "orchestration",
            "inbox-ask",
            "automation",
            "composer",
            "bot-draft-origin",
            "edit-recovery",
        ]
        .map(|prefix| format!("{prefix}:{id}"));
        let delivered = format!("bot-delivered:{id}:");
        let _lock = shared.settings.lock()?;
        let mut next = self.shared_settings_snapshot()?;
        let before = next.len();
        next.retain(|key, _| !exact.contains(key) && !key.starts_with(&delivered));
        if next.len() == before {
            return Ok(());
        }
        shared.settings.save(next)
    }
    pub(crate) fn delete_checked(&self, id: i64, app: Option<(i64, &str)>) -> Result<(), String> {
        if let Some((caller, project)) = app {
            self.check_app_target(caller, id, project)?;
        }

        self.shared_writable()?;
        // Unreadable settings must leave the session recoverable before SQL deletion.
        self.shared.settings.read(|_| ())?;
        let original = self.persisted_session_id(id)?;
        let tx = sql(rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        ))?;
        if let Some((caller, project)) = app {
            self.check_app_target(caller, id, project)?;
        }
        self.delete_local_submissions(&original)?;
        if let Some(tables) = &self.shared.earlier_bot_tables {
            for table in [&tables.blocks, &tables.turns, &tables.transcripts] {
                sql(self.conn.execute(
                    &format!("DELETE FROM {table} WHERE session_id=?1"),
                    [&original],
                ))?;
            }
        }
        for (table, column) in [
            ("session_reminders", "session_id"),
            ("bot_blocks", "session_id"),
            ("bot_turns", "session_id"),
            ("bot_transcripts", "session_id"),
            ("ordinary_blocks", "session_id"),
            ("ordinary_transcripts", "session_id"),
            ("ordinary_detached_leads", "session_id"),
            ("composer_drafts", "session_id"),
            ("composer_draft_recoveries", "session_id"),
            ("composer_transfers", "session_id"),
            ("native_follow_up_metadata", "session_id"),
            ("provider_history_cursors", "session_id"),
            ("provider_history_receipts", "session_id"),
            ("provider_history_recoveries", "session_id"),
            ("in_flight_sessions", "session_id"),
            ("orchestration_workers", "session_id"),
            ("orchestration_sidebar", "lead_id"),
            ("orchestration_runs", "lead_id"),
        ] {
            if has_table(&self.conn, table)? {
                sql(self.conn.execute(
                    &format!("DELETE FROM {table} WHERE {column}=?1"),
                    [&original],
                ))?;
            }
        }
        if has_table(&self.conn, "native_bot_draft_origins")? {
            sql(self.conn.execute(
                "DELETE FROM native_bot_draft_origins WHERE key=?1",
                [format!("bot-draft-origin:{id}")],
            ))?;
        }
        sql(self
            .conn
            .execute("DELETE FROM sessions WHERE id=?1", [original]))?;
        sql(tx.commit())?;
        self.shared.pending_turn_recoveries.borrow_mut().remove(&id);
        self.shared.snapshots.borrow_mut().remove(&id);
        self.shared.incremental.borrow_mut().remove(&id);
        self.shared.queues.borrow_mut().remove(&id);
        self.shared.card_revisions.borrow_mut().remove(&id);
        self.shared.refreshed_cards.borrow_mut().remove(&id);
        // SQL deletion has committed. Publish sidecar removal atomically too;
        // failure is an explicit partial cleanup, never a successful rollback.
        self.shared_delete_session_settings(id).map_err(|error| {
            format!("Conversation was deleted, but its settings could not be removed: {error}")
        })
    }
    pub fn detach_worktree_paths(
        &self,
        paths: &[&str],
        main: &str,
    ) -> Result<Vec<(Session, Settings)>, String> {
        let tx = sql(self.conn.unchecked_transaction())?;
        let within = |cwd: &str| {
            paths
                .iter()
                .any(|path| crate::files::is_equal_or_inside(cwd, path))
        };
        let mut out = Vec::new();
        for mut session in self.read_sessions()? {
            let mut settings = self.agent_settings(session.id)?.unwrap_or_default();
            let execution = settings.worktree_cwd.as_deref().unwrap_or(&session.cwd);
            if !within(execution) {
                continue;
            }
            settings.worktree_cwd = Some(execution.into());
            settings.worktree_removed = true;
            settings.revision += 1;
            settings.new_worktree = false;
            settings.worktree_base = None;
            if within(&session.cwd) {
                session.cwd = main.into();
            }
            session.provider_id = None;
            self.shared_patch(
                session.id,
                &[
                    ("cwd", json!(session.cwd)),
                    ("provider_session_id", Value::Null),
                ],
            )?;
            self.save_agent_preferences(session.id, &settings)?;
            out.push((session, settings));
        }
        sql(tx.commit())?;
        Ok(out)
    }
    /// Stream saved message bodies without loading conversations into the UI.
    /// Rejoin native row fragments so phrase matches span storage boundaries.
    pub fn search_sessions(
        &self,
        query: &str,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(Vec<crate::search::Hit>, bool), String> {
        use std::sync::atomic::Ordering;
        if query.trim().is_empty() || cancel.load(Ordering::Acquire) {
            return Ok((Vec::new(), false));
        }
        let flag = cancel.clone();
        sql(self
            .conn
            .progress_handler(2000, Some(move || flag.load(Ordering::Acquire))))?;
        let result = (|| {
            let summaries = self.read_sessions()?;
            let mut hits = Vec::new();
            let mut truncated = false;
            'sessions: for summary in summaries.iter().filter(|s| !s.archived && s.has_messages) {
                if cancel.load(Ordering::Acquire) {
                    return Ok((Vec::new(), false));
                }
                let rows = blocks::project(&self.shared_blocks(summary.id)?)?;
                let mut ix = 0;
                while ix < rows.len() {
                    if cancel.load(Ordering::Acquire) {
                        return Ok((Vec::new(), false));
                    }
                    let from = ix;
                    let mut text = String::new();
                    loop {
                        let body = &rows[ix].text;
                        let mut end = body
                            .len()
                            .min((2 * 1024 * 1024usize).saturating_sub(text.len()));
                        while !body.is_char_boundary(end) {
                            end -= 1;
                        }
                        text.push_str(&body[..end]);
                        truncated |= end < body.len();
                        ix += 1;
                        if ix == rows.len() || rows[ix].first {
                            break;
                        }
                        if cancel.load(Ordering::Acquire) {
                            return Ok((Vec::new(), false));
                        }
                    }
                    if let Some(hit) = crate::search::message_hit(
                        summary,
                        from,
                        &rows[from].role,
                        &text,
                        query,
                        false,
                    ) {
                        hits.push(hit);
                    }
                    if hits.len() >= 400 {
                        truncated = true;
                        break 'sessions;
                    }
                }
            }
            hits.extend(crate::search::titles(&summaries, query));
            Ok((hits, truncated))
        })();
        sql(self.conn.progress_handler(0, None::<fn() -> bool>))?;
        if cancel.load(Ordering::Acquire) {
            Ok((Vec::new(), false))
        } else {
            result
        }
    }
}
