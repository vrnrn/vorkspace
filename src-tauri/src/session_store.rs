pub(crate) mod incremental;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

const MIGRATION_V1: &str = r#"
CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY,
  cwd TEXT NOT NULL,
  harness TEXT NOT NULL,
  model TEXT NOT NULL,
  model_settings TEXT NOT NULL DEFAULT '{}',
  runtime_mode TEXT NOT NULL,
  title TEXT NOT NULL,
  provider_session_id TEXT,
  blocks_json TEXT NOT NULL DEFAULT '[]',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS sessions_cwd_updated_idx
  ON sessions (cwd, updated_at DESC);
"#;

pub struct SessionStore {
    conn: Mutex<Connection>,
    read_conn: Mutex<Option<Connection>>,
}

impl SessionStore {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(&path).map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")
            .map_err(|e| e.to_string())?;
        // Announce this opener before migrations/recovery start read-then-write
        // transactions, so a previous attachment collector yields to startup.
        let attachment_cache =
            crate::composer_drafts::admit_attachment_cache(&conn).map_err(|e| e.to_string())?;
        migrate(&conn).map_err(|e| e.to_string())?;
        crate::local_submissions::recover_unresolved(&conn).map_err(|e| e.to_string())?;
        crate::worktrees::reconcile_removals(&conn)?;
        crate::composer_drafts::initialize_attachment_cache(&conn, &path, attachment_cache)
            .map_err(|e| e.to_string())?;
        let read_conn = Connection::open(&path).map_err(|e| e.to_string())?;
        read_conn
            .execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 5000;")
            .map_err(|e| e.to_string())?;
        Ok(Self {
            conn: Mutex::new(conn),
            read_conn: Mutex::new(Some(read_conn)),
        })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|e| e.to_string())?;
        migrate(&conn).map_err(|e| e.to_string())?;
        Ok(Self {
            conn: Mutex::new(conn),
            read_conn: Mutex::new(None),
        })
    }

    pub(crate) fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
        self.conn
            .lock()
            .map_err(|_| "Session store is locked".into())
    }
}

pub fn init(app: &AppHandle) -> Result<(), String> {
    init_with(
        || app.path().app_data_dir().map_err(|e| e.to_string()),
        SessionStore::open,
        |store| {
            app.manage(store);
        },
    )
}

// Keep the complete startup path here so the transcript-read test covers it.
fn init_with(
    data_dir: impl FnOnce() -> Result<PathBuf, String>,
    open: impl FnOnce(PathBuf) -> Result<SessionStore, String>,
    manage: impl FnOnce(SessionStore),
) -> Result<(), String> {
    let store = open(data_dir()?.join("monocode.db"))?;
    manage(store);
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUpsert {
    #[serde(default)]
    pub sidebar_hidden: bool,
    pub id: String,
    pub cwd: String,
    pub harness: String,
    pub model: String,
    pub model_settings: Value,
    pub runtime_mode: String,
    pub title: String,
    #[serde(default)]
    pub provider_session_id: Option<String>,
    #[serde(default)]
    pub provider_account_id: Option<String>,
    pub blocks: Value,
    #[serde(default)]
    pub queued_messages: Vec<Value>,
    #[serde(default)]
    pub queue_status: Option<String>,
    /// Last context-window reading reported by the harness, if any.
    #[serde(default)]
    pub context_used: Option<i64>,
    #[serde(default)]
    pub context_window: Option<i64>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub worktree_cwd: Option<String>,
    #[serde(default)]
    pub worktree_removed: bool,
    #[serde(default)]
    pub linked_work_item: Option<Value>,
    #[serde(default)]
    pub automation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    #[serde(default)]
    pub sidebar_hidden: bool,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orchestration_lead_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orchestration: Option<Value>,
    pub cwd: String,
    pub harness: String,
    pub model: String,
    pub runtime_mode: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_cwd: Option<String>,
    #[serde(default)]
    pub worktree_removed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    pub additions: i64,
    pub deletions: i64,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub draft: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_work_item: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persistence_revision: Option<i64>,
    #[serde(default)]
    pub sidebar_hidden: bool,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orchestration_lead_id: Option<String>,
    pub cwd: String,
    pub harness: String,
    pub model: String,
    pub model_settings: Value,
    pub runtime_mode: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_account_id: Option<String>,
    pub blocks: Value,
    #[serde(default)]
    pub queued_messages: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_used: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_cwd: Option<String>,
    #[serde(default)]
    pub worktree_removed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_work_item: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[tauri::command(async)]
pub fn session_upsert(
    store: State<'_, SessionStore>,
    session: SessionUpsert,
) -> Result<SessionSummary, String> {
    validate_upsert(&session)?;
    let conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    let summary = upsert_session(&conn, &session).map_err(|e| e.to_string())?;
    Ok(summary)
}

fn validate_upsert(session: &SessionUpsert) -> Result<(), String> {
    validate_id(&session.id, "session")?;
    if session.cwd.trim().is_empty() {
        return Err("cwd is required".into());
    }
    if let Some(provider_session_id) = &session.provider_session_id {
        if !provider_session_id.is_empty() {
            validate_id(provider_session_id, "provider session")?;
        }
    }
    if let Some(provider_account_id) = &session.provider_account_id {
        if !provider_account_id.is_empty() {
            validate_id(provider_account_id, "provider account")?;
        }
    }
    if let Some(automation_id) = &session.automation_id {
        if !automation_id.is_empty() {
            validate_id(automation_id, "automation")?;
        }
    }
    if !session.model_settings.is_object() {
        return Err("modelSettings must be an object".into());
    }
    if !session.blocks.is_array() {
        return Err("blocks must be an array".into());
    }

    Ok(())
}

#[tauri::command(async)]
pub fn session_upsert_delta(
    store: State<'_, SessionStore>,
    session: SessionUpsert,
    block_ids: Vec<String>,
    expected_revision: Option<i64>,
) -> Result<(SessionSummary, i64), String> {
    validate_upsert(&session)?;
    if session.cwd.starts_with("remote://") {
        return Err("Remote sessions belong to their host".into());
    }
    if session.id.starts_with("bot/") {
        return Err("Bot sessions use their own transcript store".into());
    }
    let conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    incremental::save(&conn, &session, &block_ids, expected_revision)
}

fn generated_image_paths(blocks: &Value) -> Vec<String> {
    blocks
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block.get("role").and_then(Value::as_str) == Some("image"))
        .filter_map(|block| {
            block
                .get("image")
                .and_then(|image| image.get("path"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

#[tauri::command(async)]
pub fn session_list_by_project(
    store: State<'_, SessionStore>,
    cwd: String,
) -> Result<Vec<SessionSummary>, String> {
    if cwd.trim().is_empty() {
        return Err("cwd is required".into());
    }
    let conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    list_by_project(&conn, &cwd).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn session_rebase_project(
    store: State<'_, SessionStore>,
    from_cwd: String,
    to_cwd: String,
) -> Result<(), String> {
    if from_cwd.trim().is_empty() || to_cwd.trim().is_empty() {
        return Err("project paths are required".into());
    }
    let conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    rebase_project(&conn, &from_cwd, &to_cwd).map_err(|error| error.to_string())
}

fn rebase_project(conn: &Connection, from_cwd: &str, to_cwd: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sessions SET cwd = ?2 WHERE cwd = ?1",
        params![from_cwd, to_cwd],
    )?;
    Ok(())
}

#[tauri::command(async)]
pub fn session_list_linked(store: State<'_, SessionStore>) -> Result<Vec<SessionSummary>, String> {
    let conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    list_linked(&conn).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn session_get(
    store: State<'_, SessionStore>,
    session_id: String,
) -> Result<Option<SessionRecord>, String> {
    validate_id(&session_id, "session")?;
    let conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    let mut record = get_session(&conn, &session_id).map_err(|e| e.to_string())?;
    if let Some(record) = record.as_mut() {
        crate::tool_detail::omit_large_details(&mut record.blocks);
    }
    Ok(record)
}

/// Output the transcript omitted on load, fetched when its row is expanded.
#[tauri::command(async)]
pub fn session_tool_detail(
    store: State<'_, SessionStore>,
    session_id: String,
    block_id: String,
    step_id: Option<String>,
) -> Result<Option<String>, String> {
    validate_id(&session_id, "session")?;
    let conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    crate::tool_detail::stored_detail(&conn, &session_id, &block_id, step_id.as_deref())
        .map_err(|e| e.to_string())
}

const MAX_SEARCH_SCAN: usize = 400;
const MAX_CONVERSATION_HITS: usize = 40;
const MAX_MESSAGE_HITS: usize = 40;
const SNIPPET_RADIUS: usize = 42;
const SEARCH_PROGRESS_INTERVAL: usize = 1_000;
const MAX_SEARCH_BUFFER_BYTES: usize = 8 * 1024 * 1024;
static SESSION_SEARCH_TOKENS: Mutex<Option<HashMap<String, Arc<AtomicU64>>>> = Mutex::new(None);

#[derive(Clone)]
struct SessionSearchToken {
    owner: String,
    counter: Arc<AtomicU64>,
    generation: u64,
}

impl SessionSearchToken {
    fn is_current(&self) -> bool {
        self.counter.load(Ordering::Acquire) == self.generation
    }
}

fn begin_session_search(owner: &str) -> Option<SessionSearchToken> {
    if owner.is_empty() {
        return None;
    }
    let mut tokens = SESSION_SEARCH_TOKENS.lock().ok()?;
    let tokens = tokens.get_or_insert_with(HashMap::new);
    let counter = tokens
        .entry(owner.to_string())
        .or_insert_with(|| Arc::new(AtomicU64::new(0)))
        .clone();
    let generation = counter.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    Some(SessionSearchToken {
        owner: owner.to_string(),
        counter,
        generation,
    })
}

/// Release the entry a finished search was holding, on every exit path.
///
/// Every search runs under a fresh `searchOwner` (`SearchView` mints a UUID per
/// keystroke), so an entry left behind is never reused or overwritten — it just
/// accumulates for the life of the process. A plain call after the search would
/// not do: the search returns through `?` on any SQLite error, and `SearchView`
/// swallows the rejection, so the error path is the common case, not the rare
/// one. `Drop` covers the early return and the unwind both.
struct SessionSearchRelease(Option<SessionSearchToken>);

impl Drop for SessionSearchRelease {
    fn drop(&mut self) {
        end_session_search(self.0.as_ref());
    }
}

/// The generation check happens under the map lock, not before it.
///
/// `begin_session_search` bumps the counter while holding that same lock, so
/// checking `is_current` outside the critical section leaves a window: a newer
/// search can register in between, and since both tokens share one `Arc`, the
/// `ptr_eq` test cannot tell them apart. Removing the entry then would strip
/// the live search of its registration, leaving it running and impossible to
/// cancel. The superseded search must leave the newer counter alone, so the
/// whole decision has to be atomic with respect to registration.
fn end_session_search(token: Option<&SessionSearchToken>) {
    let Some(token) = token else {
        return;
    };
    let Ok(mut tokens) = SESSION_SEARCH_TOKENS.lock() else {
        return;
    };
    let ours = tokens
        .as_ref()
        .and_then(|tokens| tokens.get(&token.owner))
        .is_some_and(|registered| Arc::ptr_eq(registered, &token.counter) && token.is_current());
    if ours {
        if let Some(tokens) = tokens.as_mut() {
            tokens.remove(&token.owner);
        }
    }
}

#[cfg(test)]
fn session_search_token_registered(owner: &str) -> bool {
    SESSION_SEARCH_TOKENS
        .lock()
        .ok()
        .and_then(|tokens| tokens.as_ref().map(|tokens| tokens.contains_key(owner)))
        .unwrap_or(false)
}

fn cancel_owned_session_search(owner: &str) {
    if owner.is_empty() {
        return;
    }
    if let Ok(mut tokens) = SESSION_SEARCH_TOKENS.lock() {
        if let Some(counter) = tokens.as_mut().and_then(|tokens| tokens.remove(owner)) {
            counter.fetch_add(1, Ordering::AcqRel);
        }
    }
}

fn session_search_is_current(token: Option<&SessionSearchToken>) -> bool {
    token.is_none_or(SessionSearchToken::is_current)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchOptions {
    pub query: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub include_archived: bool,
    #[serde(default)]
    pub search_owner: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchHit {
    pub kind: String,
    pub session_id: String,
    pub cwd: String,
    pub harness: String,
    pub title: String,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchResult {
    pub hits: Vec<SessionSearchHit>,
    pub truncated: bool,
}

#[tauri::command(async)]
pub fn session_search(
    store: State<'_, SessionStore>,
    options: SessionSearchOptions,
) -> Result<SessionSearchResult, String> {
    search_store(&store, &options)
}

fn search_store(
    store: &SessionStore,
    options: &SessionSearchOptions,
) -> Result<SessionSearchResult, String> {
    let token = begin_session_search(&options.search_owner);
    let release = SessionSearchRelease(token.clone());
    let result = {
        let read_conn = store
            .read_conn
            .lock()
            .map_err(|_| "Session read store is locked")?;
        if let Some(conn) = read_conn.as_ref() {
            search_sessions_with_connection(conn, options, token.as_ref())?
        } else {
            let conn = store.conn.lock().map_err(|_| "Session store is locked")?;
            search_sessions_with_connection(&conn, options, token.as_ref())?
        }
    };
    drop(release);
    if session_search_is_current(token.as_ref()) {
        Ok(result)
    } else {
        Ok(SessionSearchResult {
            hits: Vec::new(),
            truncated: false,
        })
    }
}

#[tauri::command(async)]
pub fn cancel_session_search(search_owner: String) {
    cancel_owned_session_search(&search_owner);
}

#[tauri::command(async)]
pub fn session_delete(
    app: AppHandle,
    store: State<'_, SessionStore>,
    session_id: String,
    mut image_paths: Vec<String>,
    preserve_composer: Option<bool>,
) -> Result<(), String> {
    validate_id(&session_id, "session")?;
    let conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    let persisted_paths = get_session(&conn, &session_id)
        .ok()
        .flatten()
        .map(|record| generated_image_paths(&record.blocks))
        .unwrap_or_default();
    image_paths.extend(persisted_paths);
    image_paths.extend(
        crate::bot_transcript::generated_image_paths(&conn, &session_id)
            .map_err(|e| e.to_string())?,
    );
    delete_session_record(&conn, &session_id, preserve_composer.unwrap_or(false))
        .map_err(|e| e.to_string())?;
    drop(conn);
    if !image_paths.is_empty() {
        if let Err(error) = crate::fs::delete_generated_images_sync(&app, &image_paths) {
            eprintln!("Generated image cleanup will need a retry: {error}");
        }
    }
    let _ = app.emit(crate::reminders::CHANGED, ());
    Ok(())
}

#[tauri::command(async)]
pub fn session_set_archived(
    store: State<'_, SessionStore>,
    session_id: String,
    archived: bool,
) -> Result<(), String> {
    validate_id(&session_id, "session")?;
    let conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    set_archived(&conn, &session_id, archived).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn session_set_pinned(
    store: State<'_, SessionStore>,
    session_id: String,
    pinned: bool,
) -> Result<(), String> {
    validate_id(&session_id, "session")?;
    let conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    set_pinned(&conn, &session_id, pinned).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn session_set_linked_work_item(
    store: State<'_, SessionStore>,
    session_id: String,
    linked_work_item: Option<Value>,
) -> Result<(), String> {
    validate_id(&session_id, "session")?;
    let conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    set_linked_work_item(&conn, &session_id, linked_work_item.as_ref()).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InFlightSession {
    pub session_id: String,
    pub cwd: String,
}

#[tauri::command(async)]
pub fn session_set_in_flight(
    store: State<'_, SessionStore>,
    sessions: Vec<InFlightSession>,
) -> Result<(), String> {
    for session in &sessions {
        validate_id(&session.session_id, "session")?;
        if session.cwd.trim().is_empty() {
            return Err("cwd is required".into());
        }
    }
    let mut conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    replace_in_flight(&mut conn, &sessions).map_err(|e| e.to_string())
}

/// Read the quit snapshot without clearing it. Vite/dev reloads must not
/// consume the only copy.
#[tauri::command(async)]
pub fn session_list_in_flight(
    store: State<'_, SessionStore>,
) -> Result<Vec<InFlightSession>, String> {
    let conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    list_in_flight(&conn).map_err(|e| e.to_string())
}

/// Read and clear the quit snapshot so a restored window cannot take it twice.
#[tauri::command(async)]
pub fn session_take_in_flight(
    store: State<'_, SessionStore>,
) -> Result<Vec<InFlightSession>, String> {
    let mut conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    take_in_flight(&mut conn).map_err(|e| e.to_string())
}

const WORKSPACE_SNAPSHOT_MAX_BYTES: usize = 2_000_000;

#[tauri::command(async)]
pub fn workspace_set_snapshot(
    store: State<'_, SessionStore>,
    snapshot: Value,
) -> Result<(), String> {
    if !snapshot.is_object() {
        return Err("workspace snapshot must be an object".into());
    }
    let json = serde_json::to_string(&snapshot).map_err(|e| e.to_string())?;
    if json.len() > WORKSPACE_SNAPSHOT_MAX_BYTES {
        return Err("workspace snapshot is too large".into());
    }
    let conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    set_workspace_snapshot(&conn, &json).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn workspace_get_snapshot(store: State<'_, SessionStore>) -> Result<Option<Value>, String> {
    let conn = store.conn.lock().map_err(|_| "Session store is locked")?;
    let json = get_workspace_snapshot(&conn).map_err(|e| e.to_string())?;
    match json {
        None => Ok(None),
        Some(raw) => serde_json::from_str(&raw).map_err(|e| e.to_string()),
    }
}

/// Add a `sessions` column when it is absent, so a half-applied history cannot
/// leave the schema short of what the queries select.
fn ensure_session_column(conn: &Connection, column: &str, decl: &str) -> rusqlite::Result<()> {
    let present: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = ?1",
        params![column],
        |row| row.get(0),
    )?;
    if present == 0 {
        conn.execute(
            &format!("ALTER TABLE sessions ADD COLUMN {column} {decl}"),
            [],
        )?;
    }
    Ok(())
}

/// Add a `sessions` column when it is missing, tolerating the case where a
/// recorded migration version has no matching column.
fn ensure_column(conn: &Connection, name: &str, decl: &str) -> rusqlite::Result<()> {
    let present: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = ?1",
        params![name],
        |row| row.get(0),
    )?;
    if present > 0 {
        return Ok(());
    }
    conn.execute(
        &format!("ALTER TABLE sessions ADD COLUMN {name} {decl}"),
        [],
    )?;
    Ok(())
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
           version INTEGER PRIMARY KEY,
           applied_at INTEGER NOT NULL
         )",
        [],
    )?;
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;
    if current < 1 {
        conn.execute_batch(MIGRATION_V1)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (1, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 2 {
        conn.execute("ALTER TABLE sessions ADD COLUMN branch TEXT", [])?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (2, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 3 {
        conn.execute("ALTER TABLE sessions ADD COLUMN context_used INTEGER", [])?;
        conn.execute("ALTER TABLE sessions ADD COLUMN context_window INTEGER", [])?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (3, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 4 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS in_flight_sessions (
               session_id TEXT PRIMARY KEY,
               cwd TEXT NOT NULL,
               sort_index INTEGER NOT NULL
             );",
        )?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (4, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 5 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS workspace_snapshot (
               id INTEGER PRIMARY KEY CHECK (id = 1),
               snapshot_json TEXT NOT NULL,
               updated_at INTEGER NOT NULL
             );",
        )?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (5, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 6 {
        conn.execute(
            "ALTER TABLE sessions ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (6, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 7 {
        conn.execute("ALTER TABLE sessions ADD COLUMN worktree_cwd TEXT", [])?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (7, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 8 {
        // `list_by_project` used to filter with
        // `blocks_json LIKE '%"role":"user"%'`, which reads and substring-scans
        // every transcript in the project on each switch. Materialize the
        // predicate so the covering index can answer it instead.
        ensure_column(conn, "has_user_message", "INTEGER NOT NULL DEFAULT 0")?;
        conn.execute(
            "UPDATE sessions SET has_user_message =
               CASE WHEN blocks_json != '[]' AND blocks_json LIKE '%\"role\":\"user\"%'
                    THEN 1 ELSE 0 END",
            [],
        )?;
        conn.execute_batch(
            "DROP INDEX IF EXISTS sessions_cwd_updated_idx;
             CREATE INDEX IF NOT EXISTS sessions_cwd_listed_idx
               ON sessions (cwd, has_user_message, updated_at DESC);",
        )?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (8, ?1)",
            params![now_millis()],
        )?;
    }
    // A recorded version row can outlive the schema it describes when another
    // build reuses the same numbers, leaving columns that every listing query
    // (and the covering index below) depends on missing. Repair before use.
    for (column, decl) in [
        ("branch", "TEXT"),
        ("context_used", "INTEGER"),
        ("context_window", "INTEGER"),
        ("archived", "INTEGER NOT NULL DEFAULT 0"),
        ("worktree_cwd", "TEXT"),
        ("has_user_message", "INTEGER NOT NULL DEFAULT 0"),
        ("pinned", "INTEGER NOT NULL DEFAULT 0"),
        ("linked_work_item_json", "TEXT"),
        ("provider_account_id", "TEXT"),
        ("worktree_removed", "INTEGER NOT NULL DEFAULT 0"),
        ("is_draft", "INTEGER NOT NULL DEFAULT 0"),
        ("automation_id", "TEXT"),
        ("queued_messages_json", "TEXT NOT NULL DEFAULT '[]'"),
        ("queue_status", "TEXT"),
        ("sidebar_hidden", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        ensure_session_column(conn, column, decl)?;
    }
    if current < 9 {
        // v8 stopped the blob scan but still cost a table seek per row, and
        // every summary column (`created_at`, `updated_at`, `branch`,
        // `archived`) is stored *after* `blocks_json` in the record — so
        // reaching them meant walking past a ~180 KB transcript's overflow
        // pages, 120 times over, on each project switch. Widening the index to
        // cover the whole projection keeps the query inside the index and off
        // the table entirely: measured 7.3 ms -> 0.24 ms for 120 sessions.
        // Version rows are not proof the columns landed: a DB can carry a
        // recorded version from another build without the ALTER that went with
        // it, and indexing a missing column aborts the whole migration.
        ensure_column(conn, "branch", "TEXT")?;
        ensure_column(conn, "archived", "INTEGER NOT NULL DEFAULT 0")?;
        ensure_column(conn, "has_user_message", "INTEGER NOT NULL DEFAULT 0")?;
        conn.execute_batch(
            "DROP INDEX IF EXISTS sessions_cwd_listed_idx;
             CREATE INDEX IF NOT EXISTS sessions_cwd_cover_idx
               ON sessions (cwd, has_user_message, updated_at DESC, id, harness,
                            model, runtime_mode, title, provider_session_id,
                            created_at, branch, archived);",
        )?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (9, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 10 {
        crate::notes::ensure_notes_table(conn)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (10, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 11 {
        // Pinning is a listing column, so it has to live in the covering
        // index with `archived`. Selecting it off the table would walk past
        // `blocks_json` on every project switch.
        ensure_column(conn, "pinned", "INTEGER NOT NULL DEFAULT 0")?;
        conn.execute_batch(
            "DROP INDEX IF EXISTS sessions_cwd_cover_idx;
             CREATE INDEX IF NOT EXISTS sessions_cwd_cover_idx
               ON sessions (cwd, has_user_message, updated_at DESC, id, harness,
                            model, runtime_mode, title, provider_session_id,
                            created_at, branch, archived, pinned);",
        )?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (11, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 12 {
        // The sidebar renders this metadata for every row, so keep it in the
        // same covering index as the rest of the session-card projection.
        ensure_column(conn, "linked_work_item_json", "TEXT")?;
        conn.execute_batch(
            "DROP INDEX IF EXISTS sessions_cwd_cover_idx;
             CREATE INDEX IF NOT EXISTS sessions_cwd_cover_idx
               ON sessions (cwd, has_user_message, updated_at DESC, id, harness,
                            model, runtime_mode, title, provider_session_id,
                            created_at, branch, archived, pinned,
                            linked_work_item_json);",
        )?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (12, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 13 {
        crate::notes::ensure_notes_table(conn)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (13, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 14 {
        ensure_session_column(conn, "provider_account_id", "TEXT")?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (14, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 15 {
        ensure_session_column(conn, "worktree_removed", "INTEGER NOT NULL DEFAULT 0")?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (15, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 16 {
        conn.execute_batch(
            "DROP INDEX IF EXISTS sessions_cwd_cover_idx;
             CREATE INDEX sessions_cwd_cover_idx
               ON sessions (cwd, has_user_message, updated_at DESC, id, harness,
                            model, runtime_mode, title, provider_session_id,
                            created_at, branch, archived, pinned,
                            linked_work_item_json, worktree_cwd, worktree_removed);",
        )?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (16, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 17 {
        // Draft is rendered on every session card. Materialize it alongside
        // `has_user_message` so listing never has to read transcript blobs.
        ensure_session_column(conn, "is_draft", "INTEGER NOT NULL DEFAULT 0")?;
        conn.execute(
            "UPDATE sessions SET is_draft =
               CASE WHEN blocks_json LIKE '%\"role\":\"user\"%'
                          AND blocks_json LIKE '%\"draft\":true%'
                    THEN 1 ELSE 0 END",
            [],
        )?;
        conn.execute_batch(
            "DROP INDEX IF EXISTS sessions_cwd_cover_idx;
             CREATE INDEX sessions_cwd_cover_idx
               ON sessions (cwd, has_user_message, updated_at DESC, id, harness,
                            model, runtime_mode, title, provider_session_id,
                            created_at, branch, archived, pinned,
                            linked_work_item_json, worktree_cwd, worktree_removed,
                            is_draft);",
        )?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (17, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 18 {
        // Sidebar cards show whether a session came from an automation.
        ensure_session_column(conn, "automation_id", "TEXT")?;
        conn.execute_batch(
            "DROP INDEX IF EXISTS sessions_cwd_cover_idx;
             CREATE INDEX sessions_cwd_cover_idx
               ON sessions (cwd, has_user_message, updated_at DESC, id, harness,
                            model, runtime_mode, title, provider_session_id,
                            created_at, branch, archived, pinned,
                            linked_work_item_json, worktree_cwd, worktree_removed,
                            is_draft, automation_id);",
        )?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (18, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 19 {
        // Keep sidebar visibility in the projection without reading transcripts.
        conn.execute_batch(
            "DROP INDEX IF EXISTS sessions_cwd_cover_idx;
             CREATE INDEX sessions_cwd_cover_idx
               ON sessions (cwd, has_user_message, updated_at DESC, id, harness,
                            model, runtime_mode, title, provider_session_id,
                            created_at, branch, archived, pinned,
                            linked_work_item_json, worktree_cwd, worktree_removed,
                            is_draft, automation_id, sidebar_hidden);",
        )?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (19, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 20 {
        crate::composer_drafts::ensure_table(conn)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (20, ?1)",
            params![now_millis()],
        )?;
    }
    if current < 21 {
        crate::local_submissions::ensure_table(conn)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (21, ?1)",
            params![now_millis()],
        )?;
    }
    // Create even when a version row already exists (another build may have
    // used the same numbers, or a previous run recorded the version without
    // the table). Restore writes into these; missing tables look like a
    // blank homepage.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS in_flight_sessions (
           session_id TEXT PRIMARY KEY,
           cwd TEXT NOT NULL,
           sort_index INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS workspace_snapshot (
           id INTEGER PRIMARY KEY CHECK (id = 1),
           snapshot_json TEXT NOT NULL,
           updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS worktree_removals (
           path TEXT PRIMARY KEY,
           sessions_json TEXT NOT NULL
         );",
    )?;
    // Compatibility only: earlier Inbox Ask builds saved temporary chats here.
    // Keep those records off normal surfaces without deleting their transcripts.
    ensure_session_column(conn, "inbox_ask", "TEXT")?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS sessions_legacy_inbox
         ON sessions (id) WHERE inbox_ask IS NOT NULL;",
    )?;
    // Unscoped session search pins this index with `INDEXED BY`, and SQLite
    // rejects that statement outright when the index is missing instead of
    // falling back to another plan. The versioned blocks above are the normal
    // path, but a recorded version can outlive the schema it describes, so
    // restore the index here for the same reason the tables above are
    // restored: a missing index would fail every search without a `cwd`.
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS sessions_cwd_cover_idx
           ON sessions (cwd, has_user_message, updated_at DESC, id, harness,
                        model, runtime_mode, title, provider_session_id,
                        created_at, branch, archived, pinned,
                        linked_work_item_json, worktree_cwd, worktree_removed,
                        is_draft, automation_id, sidebar_hidden);",
    )?;
    crate::notes::ensure_notes_table(conn)?;
    crate::reminders::ensure_table(conn)?;
    crate::automations::ensure_tables(conn)?;
    incremental::ensure_tables(conn)?;
    ensure_orchestration_history(conn)?;
    crate::bot_transcript::ensure_tables(conn)?;
    crate::composer_drafts::ensure_table(conn)?;
    crate::local_submissions::ensure_table(conn)?;
    Ok(())
}

fn ensure_orchestration_history(conn: &Connection) -> rusqlite::Result<()> {
    let indexed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'orchestration_sidebar')",
        [],
        |row| row.get(0),
    )?;
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS orchestration_runs (lead_id TEXT PRIMARY KEY, state TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS orchestration_sidebar (lead_id TEXT PRIMARY KEY, summary TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS orchestration_workers (session_id TEXT PRIMARY KEY, lead_id TEXT NOT NULL);",
    )?;
    if !indexed {
        // One-time compatibility pass for the preview that listed workers as
        // separate chats. Normal sidebar reads never scan transcripts/run blobs.
        let runs = tx
            .prepare("SELECT lead_id, state FROM orchestration_runs")?
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (lead, state) in runs {
            if let Ok(run) = serde_json::from_str::<Value>(&state) {
                index_orchestration(&tx, &lead, &run)?;
            }
        }
        let workers = tx.prepare("SELECT id, blocks_json FROM sessions WHERE blocks_json LIKE '%orchestrationLeadId%'")?
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, blocks) in workers {
            if let Ok(blocks) = serde_json::from_str::<Value>(&blocks) {
                remember_worker_from_blocks(&tx, &id, &blocks)?;
            }
        }
    }
    tx.commit()
}

fn remember_worker(conn: &Connection, id: &str, lead: &str) -> rusqlite::Result<()> {
    if id != lead && validate_id(id, "Worker").is_ok() && validate_id(lead, "Lead").is_ok()
        && !conn.query_row("SELECT EXISTS(SELECT 1 FROM ordinary_detached_leads WHERE session_id=?1 AND lead_id=?2)",params![id,lead],|r|r.get::<_,bool>(0))? {
        // Keep earlier workers indexed when a lead starts a subsequent run.
        conn.execute(
            "INSERT OR IGNORE INTO orchestration_workers(session_id, lead_id) VALUES (?1, ?2)",
            params![id, lead],
        )?;
    }
    Ok(())
}

fn remember_worker_from_blocks(
    conn: &Connection,
    id: &str,
    blocks: &Value,
) -> rusqlite::Result<()> {
    if let Some(blocks) = blocks.as_array() {
        for block in blocks {
            if block["role"] == "user" {
                if let Some(lead) = block["orchestrationLeadId"].as_str() {
                    remember_worker(conn, id, lead)?;
                    break;
                }
            }
        }
    }
    Ok(())
}

fn index_orchestration(conn: &Connection, lead: &str, run: &Value) -> rusqlite::Result<()> {
    let Some(tasks) = run["tasks"].as_array() else {
        return Ok(());
    };
    let mut summaries = Vec::new();
    for task in tasks {
        let Some(id) = task["sessionId"].as_str() else {
            continue;
        };
        remember_worker(conn, id, lead)?;
        summaries.push(serde_json::json!({
            "sessionId": id, "title": task["title"], "harness": task["harness"],
            "model": task["model"], "status": task["status"],
        }));
    }
    let summary = serde_json::json!({ "status": run["status"], "tasks": summaries });
    conn.execute("INSERT INTO orchestration_sidebar(lead_id, summary) VALUES (?1, ?2) ON CONFLICT(lead_id) DO UPDATE SET summary = excluded.summary", params![lead, summary.to_string()])?;
    Ok(())
}

pub(crate) fn save_orchestration(
    conn: &Connection,
    lead: &str,
    run: &Value,
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("INSERT INTO orchestration_runs(lead_id, state) VALUES (?1, ?2) ON CONFLICT(lead_id) DO UPDATE SET state = excluded.state", params![lead, run.to_string()])?;
    index_orchestration(&tx, lead, run)?;
    tx.commit()
}

fn worker_parent(conn: &Connection, id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT lead_id FROM orchestration_workers WHERE session_id = ?1",
        [id],
        |row| row.get(0),
    )
    .optional()
}

fn orchestration_summary(conn: &Connection, id: &str) -> rusqlite::Result<Option<Value>> {
    Ok(optional_json(
        conn.query_row(
            "SELECT summary FROM orchestration_sidebar WHERE lead_id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?,
    ))
}

pub(crate) fn upsert_session(
    conn: &Connection,
    session: &SessionUpsert,
) -> rusqlite::Result<SessionSummary> {
    conn.execute_batch("SAVEPOINT ordinary_full_save")?;
    let result = (|| {
        let summary = upsert_session_inner(conn, session, false)?;
        conn.execute(
            "DELETE FROM ordinary_blocks WHERE session_id=?1",
            [&session.id],
        )?;
        conn.execute(
            "INSERT INTO ordinary_transcripts(session_id,revision,normalized) VALUES (?1,abs(random() % 9007199254740991),0) ON CONFLICT(session_id) DO UPDATE SET revision=excluded.revision,normalized=0",
            [&session.id],
        )?;
        Ok(summary)
    })();
    match result {
        Ok(summary) => {
            conn.execute_batch("RELEASE ordinary_full_save")?;
            Ok(summary)
        }
        Err(error) => {
            let _ =
                conn.execute_batch("ROLLBACK TO ordinary_full_save; RELEASE ordinary_full_save");
            Err(error)
        }
    }
}

fn upsert_session_inner(
    conn: &Connection,
    session: &SessionUpsert,
    incremental: bool,
) -> rusqlite::Result<SessionSummary> {
    let now = now_millis();
    let model_settings = serde_json::to_string(&session.model_settings)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let existing: Option<(i64, i64, String, i64, i64, String)> = conn
        .query_row(
            if incremental { "SELECT created_at, updated_at, '[]', archived, pinned, queued_messages_json FROM sessions WHERE id = ?1" }
            else { "SELECT created_at, updated_at, blocks_json, archived, pinned, queued_messages_json FROM sessions WHERE id = ?1" },
            params![session.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .optional()?;
    let previous_blocks: Option<Value> = if !incremental && incremental::indexed(conn, &session.id)?
    {
        Some(incremental::blocks(conn, &session.id)?)
    } else {
        existing
            .as_ref()
            .and_then(|(_, _, raw, _, _, _)| serde_json::from_str(raw).ok())
    };
    let blocks = if incremental {
        Cow::Owned(Value::Array(Vec::new()))
    } else {
        crate::artifacts::retain_existing_cards(conn, &session.blocks)?
    };
    // Loaded transcripts omit large tool output; never let a save erase it.
    let blocks = match &previous_blocks {
        Some(previous) => match crate::tool_detail::restore_details(previous, &blocks) {
            Cow::Owned(restored) => Cow::Owned(restored),
            Cow::Borrowed(_) => blocks,
        },
        None => blocks,
    };
    let blocks_json = serde_json::to_string(blocks.as_ref())
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let queued_messages_json = serde_json::to_string(&session.queued_messages)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let linked_work_item_json = session
        .linked_work_item
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let automation_id = session
        .automation_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let provider_session_id = session
        .provider_session_id
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let provider_account_id = session
        .provider_account_id
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let git = crate::fs::git_info_for(&crate::fs::expand_home(
        session
            .worktree_cwd
            .as_deref()
            .filter(|cwd| !cwd.is_empty())
            .unwrap_or(&session.cwd),
    ));
    let branch = if session.worktree_removed {
        None
    } else {
        git.branch
            .as_deref()
            .filter(|value| !value.is_empty())
            .or_else(|| {
                session
                    .branch
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            })
    };
    let worktree_cwd = session
        .worktree_cwd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let (has_user_message, is_draft) = if incremental {
        conn.query_row("SELECT COALESCE(MAX(has_user),0),COALESCE(MAX(is_draft),0) FROM ordinary_blocks WHERE session_id=?1", [&session.id], |r| Ok((r.get::<_,i64>(0)? != 0,r.get::<_,i64>(1)? != 0)))?
    } else {
        (
            has_user_block(&session.blocks),
            has_draft_block(&session.blocks),
        )
    };

    let created_at = existing
        .as_ref()
        .map(|(value, _, _, _, _, _)| *value)
        .unwrap_or(now);
    let updated_at = match &existing {
        Some((_, prev_updated, _, _, _, prev_queue))
            if previous_blocks.as_ref() == Some(blocks.as_ref())
                && prev_queue == &queued_messages_json =>
        {
            *prev_updated
        }
        _ => now,
    };
    let archived = existing
        .as_ref()
        .map(|(_, _, _, value, _, _)| *value != 0)
        .unwrap_or(false);
    let pinned = existing
        .as_ref()
        .map(|(_, _, _, _, value, _)| *value != 0)
        .unwrap_or(false);

    conn.execute(
        "INSERT INTO sessions (
           id, cwd, harness, model, model_settings, runtime_mode, title,
           provider_session_id, blocks_json, created_at, updated_at, branch,
           context_used, context_window, worktree_cwd, has_user_message,
           linked_work_item_json, provider_account_id, worktree_removed, is_draft,
           automation_id, queued_messages_json, queue_status, sidebar_hidden
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)
         ON CONFLICT(id) DO UPDATE SET
           cwd = excluded.cwd,
           harness = excluded.harness,
           model = excluded.model,
           model_settings = excluded.model_settings,
           runtime_mode = excluded.runtime_mode,
           title = excluded.title,
           provider_session_id = excluded.provider_session_id,
           blocks_json = excluded.blocks_json,
           updated_at = excluded.updated_at,
           branch = excluded.branch,
           context_used = excluded.context_used,
           context_window = excluded.context_window,
           worktree_cwd = excluded.worktree_cwd,
           has_user_message = excluded.has_user_message,
           linked_work_item_json = excluded.linked_work_item_json,
           provider_account_id = excluded.provider_account_id,
           worktree_removed = excluded.worktree_removed,
           is_draft = excluded.is_draft,
           automation_id = excluded.automation_id,
           queued_messages_json = excluded.queued_messages_json,
           queue_status = excluded.queue_status,
           sidebar_hidden = excluded.sidebar_hidden",
        params![
            session.id,
            session.cwd,
            session.harness,
            session.model,
            model_settings,
            session.runtime_mode,
            session.title,
            provider_session_id,
            blocks_json,
            created_at,
            updated_at,
            branch,
            session.context_used,
            session.context_window,
            worktree_cwd,
            i64::from(has_user_message),
            linked_work_item_json,
            provider_account_id,
            i64::from(session.worktree_removed),
            i64::from(is_draft),
            automation_id,
            queued_messages_json,
            session.queue_status,
            i64::from(session.sidebar_hidden),
        ],
    )?;

    remember_worker_from_blocks(conn, &session.id, &session.blocks)?;
    Ok(SessionSummary {
        sidebar_hidden: session.sidebar_hidden,
        id: session.id.clone(),
        orchestration_lead_id: worker_parent(conn, &session.id)?,
        orchestration: orchestration_summary(conn, &session.id)?,
        cwd: session.cwd.clone(),
        harness: session.harness.clone(),
        model: session.model.clone(),
        runtime_mode: session.runtime_mode.clone(),
        title: session.title.clone(),
        provider_session_id: provider_session_id.map(str::to_owned),
        branch: branch.map(str::to_owned),
        worktree_cwd: worktree_cwd.map(str::to_owned),
        worktree_removed: session.worktree_removed,
        repo: git.repo,
        additions: 0,
        deletions: 0,
        created_at,
        updated_at,
        archived,
        pinned,
        draft: is_draft,
        linked_work_item: session.linked_work_item.clone(),
        automation_id: automation_id.map(str::to_owned),
    })
}

struct SearchProgressGuard<'a>(&'a Connection);

impl Drop for SearchProgressGuard<'_> {
    fn drop(&mut self) {
        let _ = self.0.progress_handler(1, None::<fn() -> bool>);
    }
}

#[cfg(test)]
fn search_sessions(
    conn: &Connection,
    options: &SessionSearchOptions,
) -> Result<SessionSearchResult, String> {
    let token = begin_session_search(&options.search_owner);
    let _release = SessionSearchRelease(token.clone());
    search_sessions_with_connection(conn, options, token.as_ref())
}

fn search_sessions_sql(include_archived: bool, cwd_scoped: bool) -> String {
    let mut sql = String::from(
        "SELECT id, cwd, harness, title, updated_at, archived,
                CASE WHEN octet_length(blocks_json) <= ?2
                     THEN blocks_json ELSE NULL END
         FROM sessions",
    );
    if !cwd_scoped {
        // Without a `cwd` equality to anchor the leading index column, the
        // planner answers this from a table scan, and every column it reads
        // (`inbox_ask`, `archived`, `has_user_message`, `updated_at`) sits after
        // `blocks_json` in the record, so it walks each transcript's overflow
        // pages before the size guard can reject the row. The partial inbox
        // index answers the same filter without the table, and pinning the
        // covering index keeps the rest of the predicate and the projection
        // inside it: the table is then reached only for `blocks_json` rows that
        // are already known to fit the budget.
        sql.push_str(" INDEXED BY sessions_cwd_cover_idx");
    }
    sql.push_str(
        " WHERE id NOT IN (SELECT id FROM sessions WHERE inbox_ask IS NOT NULL)
            AND has_user_message = 1
            AND (LOWER(title) LIKE LOWER(?1) ESCAPE '\\'
                 OR CASE WHEN octet_length(blocks_json) <= ?2
                         THEN LOWER(blocks_json) LIKE LOWER(?1) ESCAPE '\\'
                         ELSE 0 END
                 OR EXISTS(SELECT 1 FROM ordinary_blocks
                           WHERE ordinary_blocks.session_id = sessions.id
                             AND CASE WHEN octet_length(block_json) <= ?2
                                      THEN LOWER(block_json) LIKE LOWER(?1) ESCAPE '\\'
                                      ELSE 0 END)
                 OR EXISTS(SELECT 1 FROM bot_blocks
                           WHERE bot_blocks.session_id = sessions.id
                             AND search_text LIKE LOWER(?1) ESCAPE '\\'))",
    );
    if !include_archived {
        sql.push_str(" AND archived = 0");
    }
    if cwd_scoped {
        sql.push_str(" AND cwd = ?3");
        sql.push_str(" ORDER BY updated_at DESC, id ASC LIMIT ?4");
    } else {
        sql.push_str(" ORDER BY updated_at DESC, id ASC LIMIT ?3");
    }
    sql
}

fn search_sessions_with_connection(
    conn: &Connection,
    options: &SessionSearchOptions,
    token: Option<&SessionSearchToken>,
) -> Result<SessionSearchResult, String> {
    let query = options.query.trim();
    if query.is_empty() {
        return Ok(SessionSearchResult {
            hits: Vec::new(),
            truncated: false,
        });
    }
    let needle = query.to_lowercase();
    let pattern = like_pattern(query);
    let cwd = options
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let budget = MAX_SEARCH_BUFFER_BYTES as i64;
    let sql = search_sessions_sql(options.include_archived, cwd.is_some());

    let mut statement = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let limit = (MAX_SEARCH_SCAN as i64) + 1;
    let mut rows = if let Some(cwd) = cwd {
        statement
            .query(params![pattern, budget, cwd, limit])
            .map_err(|e| e.to_string())?
    } else {
        statement
            .query(params![pattern, budget, limit])
            .map_err(|e| e.to_string())?
    };

    let progress_token = token.cloned();
    conn.progress_handler(
        SEARCH_PROGRESS_INTERVAL as i32,
        Some(move || !session_search_is_current(progress_token.as_ref())),
    )
    .map_err(|e| e.to_string())?;
    let _progress_guard = SearchProgressGuard(conn);
    let mut conversations = Vec::new();
    let mut messages = Vec::new();
    let mut scanned = 0usize;
    let mut truncated = false;
    loop {
        if !session_search_is_current(token) {
            return Ok(SessionSearchResult {
                hits: Vec::new(),
                truncated: false,
            });
        }
        let row = match rows.next() {
            Ok(None) => break,
            Ok(Some(row)) => row,
            Err(_) if !session_search_is_current(token) => {
                return Ok(SessionSearchResult {
                    hits: Vec::new(),
                    truncated: false,
                });
            }
            Err(error) => return Err(error.to_string()),
        };
        scanned += 1;
        if scanned > MAX_SEARCH_SCAN {
            truncated = true;
            break;
        }

        let id: String = row.get(0).map_err(|e| e.to_string())?;
        let cwd: String = row.get(1).map_err(|e| e.to_string())?;
        let harness: String = row.get(2).map_err(|e| e.to_string())?;
        let title: String = row.get(3).map_err(|e| e.to_string())?;
        let updated_at: i64 = row.get(4).map_err(|e| e.to_string())?;
        let title_hit = title.to_lowercase().contains(&needle);
        if title_hit && conversations.len() < MAX_CONVERSATION_HITS {
            conversations.push(SessionSearchHit {
                kind: "conversation".into(),
                session_id: id.clone(),
                cwd: cwd.clone(),
                harness: harness.clone(),
                title: title.clone(),
                updated_at,
                block_id: None,
                role: None,
                preview: String::new(),
            });
        }

        if messages.len() >= MAX_MESSAGE_HITS {
            if title_hit {
                continue;
            }
            if conversations.len() >= MAX_CONVERSATION_HITS {
                truncated = true;
                break;
            }
            continue;
        }

        let blocks_raw = match row.get_ref(6).map_err(|e| e.to_string())? {
            rusqlite::types::ValueRef::Null => {
                truncated = true;
                continue;
            }
            value => {
                String::from_utf8_lossy(value.as_bytes().map_err(|e| e.to_string())?).into_owned()
            }
        };
        let blocks = if crate::bot_transcript::is_indexed(conn, &id).map_err(|e| e.to_string())? {
            crate::bot_transcript::matching_blocks(
                conn,
                &id,
                &needle,
                MAX_MESSAGE_HITS - messages.len(),
            )
            .map_err(|e| e.to_string())?
        } else if incremental::indexed(conn, &id).map_err(|e| e.to_string())? {
            let (blocks, budget_exhausted) =
                incremental::matching_blocks(conn, &id, &pattern, MAX_SEARCH_BUFFER_BYTES)
                    .map_err(|e| e.to_string())?;
            truncated |= budget_exhausted;
            blocks
        } else {
            let Ok(blocks) = serde_json::from_str::<Value>(&blocks_raw) else {
                continue;
            };
            blocks
        };
        for hit in block_hits(&blocks, &needle) {
            messages.push(SessionSearchHit {
                kind: "message".into(),
                session_id: id.clone(),
                cwd: cwd.clone(),
                harness: harness.clone(),
                title: title.clone(),
                updated_at,
                block_id: Some(hit.0),
                role: Some(hit.1),
                preview: hit.2,
            });
            if messages.len() >= MAX_MESSAGE_HITS {
                truncated = true;
                break;
            }
        }
    }

    if conversations.len() >= MAX_CONVERSATION_HITS {
        truncated = true;
    }
    let mut hits = conversations;
    hits.extend(messages);
    Ok(SessionSearchResult { hits, truncated })
}

fn like_pattern(query: &str) -> String {
    let mut out = String::from("%");
    for ch in query.chars().take(200) {
        match ch {
            '%' | '_' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out.push('%');
    out
}

fn block_hits(blocks: &Value, needle: &str) -> Vec<(String, String, String)> {
    let Some(items) = blocks.as_array() else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    for block in items {
        let role = block
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(
            role,
            "user" | "assistant" | "tool" | "tasks" | "plan" | "image"
        ) {
            continue;
        }
        let id = block
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            continue;
        }
        for text in block_texts(block) {
            if !text.to_lowercase().contains(needle) {
                continue;
            }
            hits.push((id.clone(), role.to_string(), snippet_around(&text, needle)));
            break;
        }
    }
    hits
}

fn block_texts(block: &Value) -> Vec<String> {
    let mut texts = Vec::new();
    push_text(&mut texts, block.get("text"));
    if let Some(image) = block.get("image") {
        push_text(&mut texts, image.get("name"));
        push_text(&mut texts, image.get("alt"));
    }
    if let Some(tool) = block.get("tool") {
        push_text(&mut texts, tool.get("title"));
        push_text(&mut texts, tool.get("detail"));
        if let Some(preview) = tool.get("preview") {
            push_text(&mut texts, preview.get("query"));
            push_text(&mut texts, preview.get("path"));
            push_text(&mut texts, preview.get("output"));
            push_text(&mut texts, preview.get("title"));
        }
    }
    texts
}

fn push_text(texts: &mut Vec<String>, value: Option<&Value>) {
    if let Some(text) = value.and_then(Value::as_str) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            texts.push(trimmed.to_string());
        }
    }
}

fn snippet_around(text: &str, needle: &str) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = compact.to_lowercase();
    let Some(index) = lower.find(needle) else {
        if compact.chars().count() <= SNIPPET_RADIUS * 2 {
            return compact;
        }
        return format!(
            "{}…",
            compact.chars().take(SNIPPET_RADIUS * 2).collect::<String>()
        );
    };
    let start = floor_char_boundary(&compact, index.saturating_sub(SNIPPET_RADIUS));
    let end = ceil_char_boundary(
        &compact,
        (index + needle.len() + SNIPPET_RADIUS).min(compact.len()),
    );
    let mut snippet = compact[start..end].trim().to_string();
    if start > 0 {
        snippet = format!("…{snippet}");
    }
    if end < compact.len() {
        snippet = format!("{snippet}…");
    }
    snippet
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn list_by_project(conn: &Connection, cwd: &str) -> rusqlite::Result<Vec<SessionSummary>> {
    let git = crate::fs::git_info_for(&crate::fs::expand_home(cwd));
    let mut statement = conn.prepare(
        "SELECT id, cwd, harness, model, runtime_mode, title, provider_session_id,
                created_at, updated_at, branch, archived, pinned,
                linked_work_item_json,
                (SELECT summary FROM orchestration_sidebar WHERE lead_id = sessions.id), worktree_cwd,
                worktree_removed, is_draft, automation_id, sidebar_hidden
         FROM sessions
         WHERE cwd = ?1
           AND has_user_message = 1
           AND id NOT IN (SELECT id FROM sessions WHERE inbox_ask IS NOT NULL)
           AND id NOT IN (SELECT session_id FROM orchestration_workers)
         ORDER BY updated_at DESC, id ASC",
    )?;
    let rows = statement.query_map(params![cwd], |row| {
        let stored_branch: Option<String> = row.get(9)?;
        let archived: i64 = row.get(10)?;
        let pinned: i64 = row.get(11)?;
        let linked_work_item = optional_json(row.get(12)?);
        Ok(SessionSummary {
            sidebar_hidden: row.get::<_, i64>(18)? != 0,
            id: row.get(0)?,
            orchestration_lead_id: None,
            orchestration: optional_json(row.get(13)?),
            cwd: row.get(1)?,
            harness: row.get(2)?,
            model: row.get(3)?,
            runtime_mode: row.get(4)?,
            title: row.get(5)?,
            provider_session_id: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
            branch: if row.get::<_, i64>(15)? != 0 {
                None
            } else {
                nonempty(stored_branch).or_else(|| git.branch.clone())
            },
            worktree_cwd: row.get(14)?,
            worktree_removed: row.get::<_, i64>(15)? != 0,
            repo: git.repo.clone(),
            additions: 0,
            deletions: 0,
            archived: archived != 0,
            pinned: pinned != 0,
            draft: row.get::<_, i64>(16)? != 0,
            linked_work_item,
            automation_id: nonempty(row.get(17)?),
        })
    })?;
    rows.collect()
}

fn list_linked(conn: &Connection) -> rusqlite::Result<Vec<SessionSummary>> {
    let mut statement = conn.prepare(
        "SELECT id, cwd, harness, model, runtime_mode, title, provider_session_id,
                created_at, updated_at, branch, archived, pinned,
                linked_work_item_json,
                (SELECT summary FROM orchestration_sidebar WHERE lead_id = sessions.id), worktree_cwd,
                worktree_removed, is_draft, automation_id, sidebar_hidden
         FROM sessions
         WHERE has_user_message = 1
           AND linked_work_item_json IS NOT NULL
           AND id NOT IN (SELECT id FROM sessions WHERE inbox_ask IS NOT NULL)
           AND id NOT IN (SELECT session_id FROM orchestration_workers)
         ORDER BY updated_at DESC, id ASC",
    )?;
    let rows = statement.query_map([], |row| {
        let archived: i64 = row.get(10)?;
        let pinned: i64 = row.get(11)?;
        Ok(SessionSummary {
            sidebar_hidden: row.get::<_, i64>(18)? != 0,
            id: row.get(0)?,
            orchestration_lead_id: None,
            orchestration: optional_json(row.get(13)?),
            cwd: row.get(1)?,
            harness: row.get(2)?,
            model: row.get(3)?,
            runtime_mode: row.get(4)?,
            title: row.get(5)?,
            provider_session_id: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
            branch: nonempty(row.get(9)?),
            worktree_cwd: row.get(14)?,
            worktree_removed: row.get::<_, i64>(15)? != 0,
            repo: None,
            additions: 0,
            deletions: 0,
            archived: archived != 0,
            pinned: pinned != 0,
            draft: row.get::<_, i64>(16)? != 0,
            linked_work_item: optional_json(row.get(12)?),
            automation_id: nonempty(row.get(17)?),
        })
    })?;
    rows.collect()
}

/// Mirrors the sidebar's notion of a listable session: a transcript that the
/// user has actually said something in.
fn has_user_block(blocks: &Value) -> bool {
    blocks.as_array().is_some_and(|blocks| {
        blocks
            .iter()
            .any(|block| block.get("role").and_then(Value::as_str) == Some("user"))
    })
}

fn has_draft_block(blocks: &Value) -> bool {
    blocks.as_array().is_some_and(|blocks| {
        blocks.iter().any(|block| {
            block.get("role").and_then(Value::as_str) == Some("user")
                && block.get("draft").and_then(Value::as_bool) == Some(true)
        })
    })
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

fn optional_json(raw: Option<String>) -> Option<Value> {
    raw.and_then(|value| serde_json::from_str(&value).ok())
}

#[cfg(test)]
fn delete_session(conn: &Connection, session_id: &str) -> rusqlite::Result<()> {
    delete_session_record(conn, session_id, false)
}

fn delete_session_record(
    conn: &Connection,
    session_id: &str,
    preserve_composer: bool,
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    let parent = worker_parent(&tx, session_id)?;
    // Ownership is also carried in transcripts for older clients. Release
    // that metadata along with the index so reopening a worker stays detached.
    let workers = tx.prepare("SELECT id, blocks_json FROM sessions WHERE id IN (SELECT session_id FROM orchestration_workers WHERE lead_id = ?1)")?
        .query_map([session_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, raw) in workers {
        tx.execute(
            "INSERT OR IGNORE INTO ordinary_detached_leads(session_id,lead_id) VALUES (?1,?2)",
            params![id, session_id],
        )?;
        if incremental::indexed(&tx, &id)? {
            tx.execute("UPDATE ordinary_blocks SET block_json=json_remove(block_json,'$.orchestrationLeadId') WHERE session_id=?1 AND json_extract(block_json,'$.orchestrationLeadId')=?2",params![id,session_id])?;
            continue;
        }
        let mut blocks: Value = serde_json::from_str(&raw).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
        if let Some(blocks) = blocks.as_array_mut() {
            for block in blocks {
                if block["orchestrationLeadId"] == session_id {
                    if let Some(block) = block.as_object_mut() {
                        block.remove("orchestrationLeadId");
                    }
                }
            }
        }
        tx.execute(
            "UPDATE sessions SET blocks_json = ?1 WHERE id = ?2",
            params![blocks.to_string(), id],
        )?;
        tx.execute("UPDATE ordinary_transcripts SET revision=abs(random() % 9007199254740991) WHERE session_id=?1", [&id])?;
    }
    tx.execute(
        "DELETE FROM orchestration_runs WHERE lead_id = ?1",
        [session_id],
    )?;
    tx.execute(
        "DELETE FROM orchestration_sidebar WHERE lead_id = ?1",
        [session_id],
    )?;
    if let Some(parent) = parent.filter(|id| id != session_id) {
        let raw: Option<String> = tx
            .query_row(
                "SELECT state FROM orchestration_runs WHERE lead_id = ?1",
                [&parent],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(raw) = raw {
            let mut run: Value = serde_json::from_str(&raw).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            let mut removed = false;
            if let Some(tasks) = run["tasks"].as_array_mut() {
                let ids: Vec<String> = tasks
                    .iter()
                    .filter(|task| task["sessionId"] == session_id)
                    .filter_map(|task| task["id"].as_str().map(str::to_string))
                    .collect();
                let before = tasks.len();
                tasks.retain(|task| task["sessionId"] != session_id);
                removed = before != tasks.len();
                if removed {
                    for task in tasks {
                        if let Some(deps) = task["dependsOn"].as_array_mut() {
                            deps.retain(|dep| !ids.iter().any(|id| dep == id));
                        }
                        // Removing a prerequisite must not release queued work.
                        // The app stops the run before deleting any current member.
                        if matches!(
                            task["status"].as_str(),
                            Some("queued" | "running" | "cancelling")
                        ) {
                            task["status"] = json!("cancelled");
                            task["accepted"] = json!(false);
                            task["delivered"] = json!(true);
                        }
                    }
                }
            }
            if removed {
                if matches!(run["status"].as_str(), Some("active" | "paused")) {
                    run["status"] = json!("stopped");
                    run["error"] =
                        json!("A worker conversation was deleted. Start a new run to continue.");
                }
                run["requests"] = json!({});
                tx.execute(
                    "UPDATE orchestration_runs SET state = ?1 WHERE lead_id = ?2",
                    params![run.to_string(), parent],
                )?;
                index_orchestration(&tx, &parent, &run)?;
            }
        }
        // Also handle summaries left by an older client without a run record.
        if let Some(mut summary) = orchestration_summary(&tx, &parent)? {
            if let Some(tasks) = summary["tasks"].as_array_mut() {
                tasks.retain(|task| task["sessionId"] != session_id);
            }
            tx.execute(
                "UPDATE orchestration_sidebar SET summary = ?1 WHERE lead_id = ?2",
                params![summary.to_string(), parent],
            )?;
        }
    }
    tx.execute(
        "DELETE FROM orchestration_workers WHERE session_id = ?1 OR lead_id = ?1",
        [session_id],
    )?;
    if !preserve_composer {
        crate::composer_drafts::delete_session_draft(&tx, session_id)?;
        crate::local_submissions::delete_session(&tx, session_id)?;
    }
    tx.execute("DELETE FROM sessions WHERE id = ?1", [session_id])?;
    tx.commit()
}

fn set_archived(conn: &Connection, session_id: &str, archived: bool) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sessions SET archived = ?1 WHERE id = ?2",
        params![if archived { 1 } else { 0 }, session_id],
    )?;
    Ok(())
}

fn set_pinned(conn: &Connection, session_id: &str, pinned: bool) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sessions SET pinned = ?1 WHERE id = ?2",
        params![if pinned { 1 } else { 0 }, session_id],
    )?;
    Ok(())
}

fn set_linked_work_item(
    conn: &Connection,
    session_id: &str,
    linked_work_item: Option<&Value>,
) -> rusqlite::Result<()> {
    let linked_work_item_json = linked_work_item
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    conn.execute(
        "UPDATE sessions SET linked_work_item_json = ?1 WHERE id = ?2",
        params![linked_work_item_json, session_id],
    )?;
    Ok(())
}

pub(crate) fn get_session(
    conn: &Connection,
    session_id: &str,
) -> rusqlite::Result<Option<SessionRecord>> {
    get_session_record(conn, session_id, true)
}

pub(crate) fn get_session_metadata(
    conn: &Connection,
    session_id: &str,
) -> rusqlite::Result<Option<SessionRecord>> {
    get_session_record(conn, session_id, false)
}

fn get_session_record(
    conn: &Connection,
    session_id: &str,
    with_blocks: bool,
) -> rusqlite::Result<Option<SessionRecord>> {
    let blocks = if with_blocks { "blocks_json" } else { "'[]'" };
    conn.query_row(
        &format!(
            "SELECT id, cwd, harness, model, model_settings, runtime_mode, title,
                provider_session_id, {blocks}, created_at, updated_at,
                context_used, context_window, branch, worktree_cwd,
                linked_work_item_json, provider_account_id, worktree_removed,
                automation_id, queued_messages_json, queue_status, sidebar_hidden
         FROM sessions
         WHERE id = ?1 AND inbox_ask IS NULL"
        ),
        params![session_id],
        |row| {
            let model_settings_raw: String = row.get(4)?;
            let blocks_raw: String = row.get(8)?;
            let model_settings = serde_json::from_str(&model_settings_raw).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            let blocks = serde_json::from_str(&blocks_raw).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    8,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            let blocks = if with_blocks && incremental::indexed(conn, session_id)? {
                incremental::blocks(conn, session_id)?
            } else {
                blocks
            };
            Ok(SessionRecord {
                persistence_revision: conn
                    .query_row(
                        "SELECT revision FROM ordinary_transcripts WHERE session_id=?1",
                        [session_id],
                        |r| r.get(0),
                    )
                    .optional()?,
                sidebar_hidden: row.get::<_, i64>(21)? != 0,
                id: row.get(0)?,
                orchestration_lead_id: worker_parent(conn, session_id)?,
                cwd: row.get(1)?,
                harness: row.get(2)?,
                model: row.get(3)?,
                model_settings,
                runtime_mode: row.get(5)?,
                title: row.get(6)?,
                provider_session_id: row.get(7)?,
                blocks,
                queued_messages: serde_json::from_str(&row.get::<_, String>(19)?).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        19,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
                queue_status: row.get(20)?,
                context_used: row.get(11)?,
                context_window: row.get(12)?,
                branch: row.get(13)?,
                worktree_cwd: row.get(14)?,
                worktree_removed: row.get::<_, i64>(17)? != 0,
                linked_work_item: optional_json(row.get(15)?),
                provider_account_id: row.get(16)?,
                automation_id: nonempty(row.get(18)?),
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        },
    )
    .optional()
}

fn list_in_flight(conn: &Connection) -> rusqlite::Result<Vec<InFlightSession>> {
    let mut statement = conn.prepare(
        "SELECT session_id, cwd FROM in_flight_sessions ORDER BY sort_index ASC, session_id ASC",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(InFlightSession {
            session_id: row.get(0)?,
            cwd: row.get(1)?,
        })
    })?;
    rows.collect()
}

fn replace_in_flight(conn: &mut Connection, sessions: &[InFlightSession]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM in_flight_sessions", [])?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO in_flight_sessions (session_id, cwd, sort_index)
             VALUES (?1, ?2, ?3)",
        )?;
        for (index, session) in sessions.iter().enumerate() {
            insert.execute(params![session.session_id, session.cwd, index as i64])?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn take_in_flight(conn: &mut Connection) -> rusqlite::Result<Vec<InFlightSession>> {
    let tx = conn.transaction()?;
    let sessions = {
        let mut statement = tx.prepare(
            "SELECT session_id, cwd FROM in_flight_sessions ORDER BY sort_index ASC, session_id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(InFlightSession {
                session_id: row.get(0)?,
                cwd: row.get(1)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    tx.execute("DELETE FROM in_flight_sessions", [])?;
    tx.commit()?;
    Ok(sessions)
}

fn set_workspace_snapshot(conn: &Connection, json: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO workspace_snapshot (id, snapshot_json, updated_at)
         VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET
           snapshot_json = excluded.snapshot_json,
           updated_at = excluded.updated_at",
        params![json, now_millis()],
    )?;
    Ok(())
}

fn get_workspace_snapshot(conn: &Connection) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT snapshot_json FROM workspace_snapshot WHERE id = 1",
        [],
        |row| row.get(0),
    )
    .optional()
}

pub(crate) fn validate_id(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!("Invalid {label} id"));
    }
    Ok(())
}

pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
