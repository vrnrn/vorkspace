//! Local unsent composer state. This is deliberately separate from Block.draft
//! transcript turns, including for remote-backed and not-yet-persisted sessions.

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Manager, State};

use crate::session_store::{now_millis, SessionStore};

const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_PAYLOAD_BYTES: usize = 10 * 1024 * 1024;
const MAX_ITEMS: usize = 128;
const MAX_SAFE_REVISION: u64 = 9_007_199_254_740_991;
const ATTACHMENT_DIRECTORY: &str = "composer-attachments";
const ATTACHMENT_PUBLICATION_GRACE: Duration = Duration::from_secs(60 * 60);
const ATTACHMENT_COLLECTION_BUDGET: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerDraftPayload {
    pub cwd: String,
    pub text: String,
    pub selection_start: u32,
    pub selection_end: u32,
    pub attachments: Vec<ComposerDraftAttachment>,
    pub mcp_tags: Vec<ComposerDraftMcpTag>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerDraftAttachment {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub kind: ComposerDraftAttachmentKind,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComposerDraftAttachmentKind {
    Image,
    Audio,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerDraftMcpTag {
    pub token: String,
    pub server: ComposerDraftMcpServer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerDraftMcpServer {
    pub provider: ComposerDraftMcpProvider,
    pub name: String,
    pub scope: ComposerDraftMcpScope,
    pub config_path: String,
    pub transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComposerDraftMcpProvider {
    Claude,
    ClaudeDesktop,
    Codex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComposerDraftMcpScope {
    Local,
    Project,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerDraftEnvelope {
    pub session_id: String,
    pub revision: u64,
    pub payload: ComposerDraftPayload,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerDraftWrite {
    pub session_id: String,
    pub expected_revision: u64,
    pub payload: ComposerDraftPayload,
}

/// Serializable, recoverable errors: callers must keep their unsaved input.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerDraftError {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<Box<ComposerDraftEnvelope>>,
}

impl ComposerDraftError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            current: None,
        }
    }

    fn storage(error: impl std::fmt::Display) -> Self {
        Self::new(
            "storageUnavailable",
            format!("Composer draft storage is unavailable: {error}"),
        )
    }
}

pub(crate) fn ensure_table(conn: &Connection) -> rusqlite::Result<()> {
    // No session FK: input may predate the first transcript save, and remote
    // session input belongs to this installation rather than the host store.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS composer_drafts (
           session_id TEXT PRIMARY KEY,
           cwd TEXT NOT NULL,
           revision INTEGER NOT NULL CHECK (revision >= 1 AND revision <= 9007199254740991),
           payload_json TEXT NOT NULL,
           deleted INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1)),
           updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS composer_attachment_processes (
           owner_instance TEXT PRIMARY KEY,
           owner_pid INTEGER NOT NULL
         );",
    )
}

/// Reclaim removed/cleared/deleted draft files on the next cold app start.
///
/// An active renderer can retain a path after its draft was cleared, before a
/// send/checkpoint/CAS retry has durably published the next owner. A process
/// lease therefore defers *all* collection while any previous store owner is
/// alive, including another store in this process. Leases retire only after
/// their process has exited. Never run this sweep on typing or session deletes.
/// Files younger than one hour also survive a crash during copy/publication.
/// Only lease admission runs on startup's first-paint path. The background
/// scan defers until a later cold start if foreground writes begin, another
/// owner appears, any read fails, or its 250ms collection budget is exhausted.
pub(crate) fn admit_attachment_cache(conn: &Connection) -> rusqlite::Result<Option<String>> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS composer_attachment_processes (owner_instance TEXT PRIMARY KEY, owner_pid INTEGER NOT NULL);")?;
    let instance = uuid::Uuid::new_v4().to_string();
    if !register_attachment_process(
        conn,
        &instance,
        std::process::id(),
        crate::local_submissions::pid_alive,
    )? {
        return Ok(None);
    }
    Ok(Some(instance))
}

pub(crate) fn initialize_attachment_cache(
    conn: &Connection,
    database: &Path,
    instance: Option<String>,
) -> rusqlite::Result<()> {
    let Some(instance) = instance else {
        return Ok(());
    };
    let activity = Arc::new(AtomicBool::new(false));
    let changed = Arc::clone(&activity);
    conn.update_hook(Some(
        move |_: rusqlite::hooks::Action, _: &str, _: &str, _: i64| {
            changed.store(true, Ordering::SeqCst);
        },
    ))?;
    let database = database.to_path_buf();
    let cutoff = SystemTime::now();
    // Never scan transcripts on the first-paint path. Any foreground write
    // before the worker's transaction cancels this cold-start attempt instead
    // of deleting a path handed from a cleared draft to an uncheckpointed send.
    let _ = std::thread::Builder::new()
        .name("composer-attachment-gc".into())
        .spawn(move || {
            let result = (|| {
                let conn = Connection::open(&database)?;
                if let Some(root) = database.parent() {
                    collect_cached_attachments(
                        &conn,
                        root,
                        cutoff,
                        &instance,
                        &activity,
                        crate::local_submissions::pid_alive,
                    )?;
                }
                Ok::<_, rusqlite::Error>(())
            })();
            if let Err(error) = result {
                eprintln!("Composer attachment cleanup deferred: {error}");
            }
        });
    Ok(())
}

fn register_attachment_process(
    conn: &Connection,
    instance: &str,
    pid: u32,
    alive: impl Fn(u32) -> bool,
) -> rusqlite::Result<bool> {
    // Every store gets an independent lease, including same-process opens.
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let owners = tx
        .prepare("SELECT owner_instance, owner_pid FROM composer_attachment_processes")?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut live_owner = false;
    for (owner, owner_pid) in owners {
        if alive(owner_pid) {
            live_owner = true;
        } else {
            tx.execute(
                "DELETE FROM composer_attachment_processes WHERE owner_instance = ?1",
                [owner],
            )?;
        }
    }
    tx.execute(
        "INSERT INTO composer_attachment_processes(owner_instance, owner_pid) VALUES (?1, ?2)
         ON CONFLICT(owner_instance) DO UPDATE SET owner_pid = excluded.owner_pid",
        params![instance, pid],
    )?;
    tx.commit()?;
    Ok(!live_owner)
}

fn collect_cached_attachments(
    conn: &Connection,
    root: &Path,
    cutoff: SystemTime,
    instance: &str,
    activity: &AtomicBool,
    alive: impl Fn(u32) -> bool,
) -> rusqlite::Result<usize> {
    if activity.load(Ordering::SeqCst) {
        return Ok(0);
    }
    let before: i64 = conn.query_row("PRAGMA data_version", [], |row| row.get(0))?;
    // A WAL read snapshot does not reserve the writer during history scanning.
    let read = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
    let other_owners = read
        .prepare("SELECT owner_pid FROM composer_attachment_processes WHERE owner_instance != ?1")?
        .query_map([instance], |row| row.get::<_, u32>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if activity.load(Ordering::SeqCst) || other_owners.into_iter().any(&alive) {
        read.commit()?;
        return Ok(0);
    }
    let collection =
        match scan_attachment_collection(&read, root, cutoff, ATTACHMENT_COLLECTION_BUDGET) {
            Ok(collection) => collection,
            Err(error) => {
                eprintln!("Composer attachment cleanup deferred: {error}");
                None
            }
        };
    read.commit()?;
    let Some(collection) = collection else {
        return Ok(0);
    };
    if activity.load(Ordering::SeqCst) || Instant::now() >= collection.deadline {
        return Ok(0);
    }
    // Only the final recheck/unlink phase reserves the writer. Startup lease
    // admission precedes every other startup transaction, so concurrent opens
    // either finish before this gate or are detected here before recovery runs.
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let after: i64 = tx.query_row("PRAGMA data_version", [], |row| row.get(0))?;
    let other_owners = tx
        .prepare("SELECT owner_pid FROM composer_attachment_processes WHERE owner_instance != ?1")?
        .query_map([instance], |row| row.get::<_, u32>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let removed = if before != after
        || activity.load(Ordering::SeqCst)
        || other_owners.into_iter().any(alive)
    {
        0
    } else {
        collection.remove_unreferenced(activity)
    };
    tx.commit()?;
    Ok(removed)
}

#[derive(Debug)]
struct AttachmentCandidate {
    path: PathBuf,
    uuid: String,
}

#[derive(Debug)]
struct AttachmentCollection {
    directory: PathBuf,
    candidates: Vec<AttachmentCandidate>,
    deadline: Instant,
}

impl AttachmentCollection {
    fn remove_unreferenced(self, activity: &AtomicBool) -> usize {
        let Ok(metadata) = std::fs::symlink_metadata(&self.directory) else {
            return 0;
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return 0;
        }
        let mut removed = 0;
        for candidate in self.candidates {
            if activity.load(Ordering::SeqCst) || Instant::now() >= self.deadline {
                break;
            }
            match std::fs::remove_file(&candidate.path) {
                Ok(()) => removed += 1,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => eprintln!(
                    "Composer attachment cleanup deferred for {}: {error}",
                    candidate.path.display()
                ),
            }
        }
        removed
    }
}

fn scan_attachment_collection(
    conn: &Connection,
    root: &Path,
    now: SystemTime,
    budget: Duration,
) -> Result<Option<AttachmentCollection>, String> {
    let deadline = Instant::now() + budget;
    struct ProgressBudget<'a>(&'a Connection);
    impl Drop for ProgressBudget<'_> {
        fn drop(&mut self) {
            let _ = self.0.progress_handler(0, None::<fn() -> bool>);
        }
    }
    conn.progress_handler(1000, Some(move || Instant::now() >= deadline))
        .map_err(|error| error.to_string())?;
    let _progress = ProgressBudget(conn);
    let directory = root.join(ATTACHMENT_DIRECTORY);
    match std::fs::symlink_metadata(&directory) {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => return Ok(None),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    }
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut candidates = Vec::new();
    for entry in entries {
        check_collection_budget(deadline)?;
        let entry = entry.map_err(|error| error.to_string())?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(uuid) = name
            .get(..36)
            .filter(|uuid| uuid::Uuid::parse_str(uuid).is_ok())
        else {
            continue;
        };
        if name.as_bytes().get(36) != Some(&b'-') {
            continue;
        }
        // Never follow directory entries or symlinks outside this owned cache.
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .map_err(|error| error.to_string())?;
        if now.duration_since(modified).unwrap_or_default() < ATTACHMENT_PUBLICATION_GRACE {
            continue;
        }
        candidates.push(AttachmentCandidate {
            path: entry.path(),
            uuid: uuid.to_owned(),
        });
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    let ids = candidates
        .iter()
        .map(|candidate| candidate.uuid.as_str())
        .collect::<HashSet<_>>();
    let mut referenced = HashSet::new();
    // One streaming pass, independent of candidate count. All durable text
    // owners participate, including ordinary/queued/saved turns, Bot blocks,
    // receipts, snapshots, notes/artifacts and automation/orchestration state.
    // Scanning raw strings also retains references in malformed recovery rows.
    // New durable tables/columns are covered without a fragile ownership list.
    let tables = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name != 'composer_attachment_processes'")
        .map_err(|error| error.to_string())?
        .query_map([], |row| row.get::<_, String>(0)).map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>().map_err(|error| error.to_string())?;
    for table in tables {
        check_collection_budget(deadline)?;
        let columns = conn
            .prepare(&format!("PRAGMA table_info({})", quote_identifier(&table)))
            .map_err(|error| error.to_string())?
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|(_, kind)| {
                let kind = kind.to_ascii_uppercase();
                kind.contains("TEXT") || kind.contains("CHAR") || kind.is_empty()
            })
            .map(|(name, _)| quote_identifier(&name))
            .collect::<Vec<_>>();
        if columns.is_empty() {
            continue;
        }
        let mut statement = conn
            .prepare(&format!(
                "SELECT {} FROM {}",
                columns.join(","),
                quote_identifier(&table)
            ))
            .map_err(|error| error.to_string())?;
        let mut rows = statement.query([]).map_err(|error| error.to_string())?;
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            check_collection_budget(deadline)?;
            for index in 0..columns.len() {
                match row.get_ref(index).map_err(|error| error.to_string())? {
                    rusqlite::types::ValueRef::Text(bytes) => {
                        let raw = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
                        collect_attachment_references(raw, &ids, &mut referenced, deadline)?;
                    }
                    rusqlite::types::ValueRef::Blob(_) => {
                        return Err("Invalid blob in durable text column".into())
                    }
                    _ => {}
                }
            }
        }
    }
    // No files are removed until every reference source has been inspected.
    check_collection_budget(deadline)?;
    candidates.retain(|candidate| !referenced.contains(&candidate.uuid));
    Ok((!candidates.is_empty()).then_some(AttachmentCollection {
        directory,
        candidates,
        deadline,
    }))
}

fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn collect_attachment_references(
    raw: &str,
    candidates: &HashSet<&str>,
    referenced: &mut HashSet<String>,
    deadline: Instant,
) -> Result<(), String> {
    let mut start = 0;
    while start < raw.len() {
        check_collection_budget(deadline)?;
        let mut end = (start + 64 * 1024).min(raw.len());
        while !raw.is_char_boundary(end) {
            end -= 1;
        }
        let chunk = &raw[start..end];
        for (offset, _) in chunk.match_indices(ATTACHMENT_DIRECTORY) {
            let tail = &raw.as_bytes()[start + offset + ATTACHMENT_DIRECTORY.len()..];
            // Paths emitted by this module have a UUID immediately after the cache
            // directory. Also tolerate JSON-escaped separators and URL separators.
            for token in tail[..tail.len().min(64)].windows(36) {
                if token[8] != b'-' || token[13] != b'-' || token[18] != b'-' || token[23] != b'-' {
                    continue;
                }
                if let Ok(uuid) = std::str::from_utf8(token) {
                    if candidates.contains(uuid) {
                        referenced.insert(uuid.to_owned());
                    }
                }
            }
        }
        if end == raw.len() {
            break;
        }
        start = end - 64;
        while !raw.is_char_boundary(start) {
            start += 1;
        }
    }
    Ok(())
}

fn check_collection_budget(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        Err("Attachment cleanup scan budget exceeded; all files preserved".into())
    } else {
        Ok(())
    }
}

#[tauri::command(async)]
pub fn composer_draft_get(
    store: State<'_, SessionStore>,
    session_id: String,
) -> Result<ComposerDraftEnvelope, ComposerDraftError> {
    let conn = store.lock_conn().map_err(ComposerDraftError::storage)?;
    get_draft(&conn, &session_id)
}

#[tauri::command(async)]
pub fn composer_draft_set(
    store: State<'_, SessionStore>,
    request: ComposerDraftWrite,
) -> Result<ComposerDraftEnvelope, ComposerDraftError> {
    let conn = store.lock_conn().map_err(ComposerDraftError::storage)?;
    set_draft(&conn, &request)
}

/// Clipboard images have no durable file by default. Keep their bytes in
/// private app-owned files; the SQLite payload contains only the returned path.
#[tauri::command]
pub async fn composer_draft_cache_attachment(
    app: AppHandle,
    name: String,
    data: String,
) -> Result<String, String> {
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || cache_attachment(&root, &name, &data))
        .await
        .map_err(|error| error.to_string())?
}

fn cache_attachment(root: &std::path::Path, name: &str, data: &str) -> Result<String, String> {
    use std::io::Write;
    const MAX_BYTES: usize = 20 * 1024 * 1024;
    if data.len() > MAX_BYTES.div_ceil(3) * 4 {
        return Err("Composer attachment exceeds 20 MiB".into());
    }
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
        .map_err(|_| "Composer attachment is not valid base64")?;
    if bytes.len() > MAX_BYTES {
        return Err("Composer attachment exceeds 20 MiB".into());
    }
    let dir = root.join("composer-attachments");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    let basename = name.rsplit(['/', '\\']).next().unwrap_or("attachment");
    let safe: String = basename
        .chars()
        .filter(|character| !character.is_control() && !matches!(character, '/' | '\\' | ':'))
        .scan(0, |bytes, character| {
            *bytes += character.len_utf8();
            (*bytes <= 120).then_some(character)
        })
        .collect();
    let safe = if safe.trim_matches('.').is_empty() {
        "attachment"
    } else {
        &safe
    };
    let path = dir.join(format!("{}-{safe}", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let saved = options.open(&path).and_then(|mut file| {
        file.write_all(&bytes)?;
        file.sync_all()
    });
    if let Err(error) = saved {
        let _ = std::fs::remove_file(&path);
        return Err(error.to_string());
    }
    Ok(path.to_string_lossy().replace('\\', "/"))
}

fn empty_payload(cwd: String) -> ComposerDraftPayload {
    ComposerDraftPayload {
        cwd,
        text: String::new(),
        selection_start: 0,
        selection_end: 0,
        attachments: Vec::new(),
        mcp_tags: Vec::new(),
    }
}

fn validate_session_id(session_id: &str) -> Result<(), ComposerDraftError> {
    if session_id.is_empty()
        || session_id.len() > 512
        || session_id.trim() != session_id
        || session_id.chars().any(char::is_control)
    {
        return Err(ComposerDraftError::new(
            "invalidDraft",
            "Invalid composer draft sessionId",
        ));
    }
    Ok(())
}

fn field(value: &str, name: &str, max: usize, empty: bool) -> Result<(), ComposerDraftError> {
    if value.len() > max || value.contains('\0') || (!empty && value.trim().is_empty()) {
        return Err(ComposerDraftError::new(
            "invalidDraft",
            format!("Invalid composer draft {name}"),
        ));
    }
    Ok(())
}

fn reference(value: &str, name: &str) -> Result<(), ComposerDraftError> {
    field(value, name, 32 * 1024, false)?;
    let lower = value.trim_start().to_ascii_lowercase();
    if lower.starts_with("data:") || lower.starts_with("blob:") || lower.contains("://") {
        return Err(ComposerDraftError::new(
            "invalidDraft",
            format!("Composer draft {name} must be a filesystem reference"),
        ));
    }
    Ok(())
}

fn validate_payload(payload: &ComposerDraftPayload) -> Result<String, ComposerDraftError> {
    // Desktop remote projects deliberately use remote://<environment>/<path>.
    // This is a context key, not an attachment URL or a request to read bytes.
    if let Some(remote) = payload.cwd.strip_prefix("remote://") {
        field(&payload.cwd, "cwd", 32 * 1024, false)?;
        if remote
            .split_once('/')
            .is_none_or(|(owner, _)| owner.is_empty())
        {
            return Err(ComposerDraftError::new(
                "invalidDraft",
                "Invalid remote composer draft cwd",
            ));
        }
    } else {
        reference(&payload.cwd, "cwd")?;
    }
    if payload.text.len() > MAX_TEXT_BYTES {
        return Err(ComposerDraftError::new(
            "invalidDraft",
            "Composer draft text exceeds 8 MiB",
        ));
    }
    // DOM selectionStart/End count UTF-16 code units, not Rust bytes/scalars.
    let utf16_len = payload.text.encode_utf16().count();
    if payload.selection_start > payload.selection_end || payload.selection_end as usize > utf16_len
    {
        return Err(ComposerDraftError::new(
            "invalidDraft",
            "Composer draft selection is outside its UTF-16 text",
        ));
    }
    if payload.attachments.len() > MAX_ITEMS || payload.mcp_tags.len() > MAX_ITEMS {
        return Err(ComposerDraftError::new(
            "invalidDraft",
            "Composer draft exceeds 128 attachments or MCP tags",
        ));
    }
    let mut attachment_ids = std::collections::HashSet::new();
    for attachment in &payload.attachments {
        field(&attachment.id, "attachment id", 512, false)?;
        field(&attachment.name, "attachment name", 4096, false)?;
        field(&attachment.mime_type, "attachment mimeType", 256, true)?;
        if attachment.size > MAX_SAFE_REVISION || !attachment_ids.insert(&attachment.id) {
            return Err(ComposerDraftError::new(
                "invalidDraft",
                "Invalid composer draft attachment size or duplicate id",
            ));
        }
        if let Some(path) = &attachment.path {
            reference(path, "attachment path")?;
        }
    }
    let mut tokens = std::collections::HashSet::new();
    for tag in &payload.mcp_tags {
        field(&tag.token, "MCP token", 4096, false)?;
        field(&tag.server.name, "MCP name", 4096, false)?;
        // Some provider listings omit the configuration path and transport.
        field(&tag.server.config_path, "MCP configPath", 32 * 1024, true)?;
        field(&tag.server.transport, "MCP transport", 256, true)?;
        if !tag.server.config_path.is_empty() {
            reference(&tag.server.config_path, "MCP configPath")?;
        }
        if !tokens.insert(&tag.token) {
            return Err(ComposerDraftError::new(
                "invalidDraft",
                "Duplicate composer draft MCP token",
            ));
        }
    }
    let raw = serde_json::to_string(payload).map_err(ComposerDraftError::storage)?;
    if raw.len() > MAX_PAYLOAD_BYTES {
        return Err(ComposerDraftError::new(
            "invalidDraft",
            "Composer draft metadata and text exceed 10 MiB",
        ));
    }
    Ok(raw)
}

fn session_scope(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<(String, Option<String>)>, ComposerDraftError> {
    conn.query_row(
        "SELECT cwd, worktree_cwd FROM sessions WHERE id = ?1",
        [session_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(ComposerDraftError::storage)
}

fn validate_scope(
    conn: &Connection,
    session_id: &str,
    cwd: &str,
) -> Result<(), ComposerDraftError> {
    if let Some((project, worktree)) = session_scope(conn, session_id)? {
        if cwd != project && worktree.as_deref() != Some(cwd) {
            return Err(ComposerDraftError::new("scopeMismatch", "Composer draft cwd does not match its session. Keep the input and retry after the session context is saved."));
        }
    }
    Ok(())
}

fn get_draft(
    conn: &Connection,
    session_id: &str,
) -> Result<ComposerDraftEnvelope, ComposerDraftError> {
    validate_session_id(session_id)?;
    let row: Option<(String, i64, String, i64)> = conn.query_row(
        "SELECT cwd, revision, payload_json, deleted FROM composer_drafts WHERE session_id = ?1",
        [session_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).optional().map_err(ComposerDraftError::storage)?;
    if let Some((cwd, revision, raw, deleted)) = row {
        if deleted == 1 {
            return Err(ComposerDraftError::new(
                "sessionDeleted",
                "This session was deleted. Its composer draft cannot be restored or replaced.",
            ));
        }
        let corrupt = || {
            ComposerDraftError::new("corruptDraft", "The stored composer draft is invalid and was preserved for recovery. Keep the current input.")
        };
        if revision < 1
            || revision as u64 > MAX_SAFE_REVISION
            || deleted != 0
            || raw.len() > MAX_PAYLOAD_BYTES
        {
            return Err(corrupt());
        }
        let payload: ComposerDraftPayload = serde_json::from_str(&raw).map_err(|_| corrupt())?;
        if payload.cwd != cwd || validate_payload(&payload).is_err() {
            return Err(corrupt());
        }
        // A session can change cwd before its composer is hydrated. Keep the
        // draft's original context; the next explicit CAS write validates the
        // new context against the current session, never silently rebinding it.
        Ok(ComposerDraftEnvelope {
            session_id: session_id.into(),
            revision: revision as u64,
            payload,
        })
    } else {
        let cwd = session_scope(conn, session_id)?
            .map(|(cwd, _)| cwd)
            .unwrap_or_default();
        Ok(ComposerDraftEnvelope {
            session_id: session_id.into(),
            revision: 0,
            payload: empty_payload(cwd),
        })
    }
}

fn set_draft(
    conn: &Connection,
    request: &ComposerDraftWrite,
) -> Result<ComposerDraftEnvelope, ComposerDraftError> {
    validate_session_id(&request.session_id)?;
    if request.expected_revision > MAX_SAFE_REVISION {
        return Err(ComposerDraftError::new(
            "invalidDraft",
            "Invalid composer draft expectedRevision",
        ));
    }
    let raw = validate_payload(&request.payload)?;
    // IMMEDIATE also serializes independent windows/connections. A deferred
    // read-then-update transaction could fail its lock upgrade under contention.
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(ComposerDraftError::storage)?;
    let current = get_draft(&tx, &request.session_id)?;
    if current.revision != request.expected_revision {
        // Retrying a committed write after a lost response is safe only for the
        // immediately preceding revision and exactly the same typed payload.
        if current.revision == request.expected_revision.saturating_add(1)
            && current.payload == request.payload
        {
            return Ok(current);
        }
        return Err(ComposerDraftError {
            code: "conflict",
            message:
                "A newer composer draft exists. Keep the current input and resolve the conflict."
                    .into(),
            current: Some(Box::new(current)),
        });
    }
    validate_scope(&tx, &request.session_id, &request.payload.cwd)?;
    if current.revision > 0 && current.payload == request.payload {
        return Ok(current);
    }
    let revision = current
        .revision
        .checked_add(1)
        .filter(|revision| *revision <= MAX_SAFE_REVISION)
        .ok_or_else(|| {
            ComposerDraftError::new("invalidDraft", "Composer draft revision is exhausted")
        })?;
    tx.execute(
        "INSERT INTO composer_drafts (session_id, cwd, revision, payload_json, deleted, updated_at)
           VALUES (?1, ?2, ?3, ?4, 0, ?5)
         ON CONFLICT(session_id) DO UPDATE SET cwd = excluded.cwd, revision = excluded.revision,
           payload_json = excluded.payload_json, updated_at = excluded.updated_at",
        params![
            request.session_id,
            request.payload.cwd,
            revision as i64,
            raw,
            now_millis()
        ],
    )
    .map_err(ComposerDraftError::storage)?;
    tx.commit().map_err(ComposerDraftError::storage)?;
    Ok(ComposerDraftEnvelope {
        session_id: request.session_id.clone(),
        revision,
        payload: request.payload.clone(),
    })
}

/// Called inside session deletion's transaction. Keep a permanent identity fence
/// even if there was no row yet: a delayed first autosave must not resurrect it.
pub(crate) fn delete_session_draft(conn: &Connection, session_id: &str) -> rusqlite::Result<()> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT cwd, payload_json FROM composer_drafts WHERE session_id = ?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((cwd, raw)) = row {
        let valid = serde_json::from_str::<ComposerDraftPayload>(&raw)
            .ok()
            .filter(|payload| payload.cwd == cwd && validate_payload(payload).is_ok());
        if valid.is_some() {
            let empty =
                serde_json::to_string(&empty_payload(cwd)).expect("draft payload serializes");
            conn.execute("UPDATE composer_drafts SET deleted = 1, payload_json = ?2, updated_at = ?3 WHERE session_id = ?1", params![session_id, empty, now_millis()])?;
        } else {
            // Do not destroy a malformed row even during cleanup: retain its
            // bytes for recovery while denying any future reads/writes to it.
            conn.execute(
                "UPDATE composer_drafts SET deleted = 1, updated_at = ?2 WHERE session_id = ?1",
                params![session_id, now_millis()],
            )?;
        }
    } else {
        let cwd: String = conn
            .query_row(
                "SELECT cwd FROM sessions WHERE id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or_default();
        let empty =
            serde_json::to_string(&empty_payload(cwd.clone())).expect("draft payload serializes");
        conn.execute("INSERT INTO composer_drafts (session_id, cwd, revision, payload_json, deleted, updated_at) VALUES (?1, ?2, 1, ?3, 1, ?4)", params![session_id, cwd, empty, now_millis()])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};

    struct CacheDirectory(PathBuf);

    impl CacheDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("composer-gc-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &str, age: Duration) -> PathBuf {
            let path = PathBuf::from(cache_attachment(&self.0, name, "Ynl0ZXM=").unwrap());
            std::fs::File::open(&path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(SystemTime::now() - age))
                .unwrap();
            path
        }
    }

    impl Drop for CacheDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn cold_collect(conn: &Connection, directory: &CacheDirectory) -> usize {
        let instance = uuid::Uuid::new_v4().to_string();
        assert!(register_attachment_process(conn, &instance, 1, |_| false).unwrap());
        collect_cached_attachments(
            conn,
            &directory.0,
            SystemTime::now(),
            &instance,
            &AtomicBool::new(false),
            |_| false,
        )
        .unwrap()
    }

    fn with_file(path: &Path) -> ComposerDraftPayload {
        let mut draft = payload("Unsent files");
        draft.attachments.push(ComposerDraftAttachment {
            id: "file".into(),
            name: "image.png".into(),
            mime_type: "image/png".into(),
            kind: ComposerDraftAttachmentKind::Image,
            size: 5,
            path: Some(path.to_string_lossy().into_owned()),
        });
        draft
    }

    fn persist_history(conn: &Connection, id: &str, path: &Path, queued: bool) {
        let blocks = json!([{"id":"turn", "role":"user", "text":"Saved input", "attachments":[{"id":"file", "path":path.to_string_lossy()}]}]).to_string();
        conn.execute("INSERT INTO sessions(id,cwd,harness,model,runtime_mode,title,created_at,updated_at,blocks_json,queued_messages_json) VALUES (?1,'/projects/example','codex','test','supervised','History',1,1,?2,?3)", params![id, if queued { "[]" } else { &blocks }, if queued { &blocks } else { "[]" }]).unwrap();
    }

    #[test]
    fn composer_attachment_gc_reclaims_removed_cleared_and_deleted_draft_files() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let directory = CacheDirectory::new();
        for id in ["removed", "cleared", "deleted"] {
            let file = directory.file(id, Duration::from_secs(7200));
            let first = set_draft(&conn, &write(id, 0, with_file(&file))).unwrap();
            assert_eq!(cold_collect(&conn, &directory), 0);
            if id == "deleted" {
                delete_session_draft(&conn, id).unwrap();
            } else {
                set_draft(
                    &conn,
                    &write(
                        id,
                        first.revision,
                        payload(if id == "removed" { "Text remains" } else { "" }),
                    ),
                )
                .unwrap();
            }
            assert_eq!(cold_collect(&conn, &directory), 1);
            assert!(!file.exists());
        }
    }

    #[test]
    fn composer_attachment_gc_preserves_transferred_history_and_all_durable_owners() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let directory = CacheDirectory::new();
        let history = directory.file("history", Duration::from_secs(7200));
        set_draft(&conn, &write("history", 0, with_file(&history))).unwrap();
        persist_history(&conn, "history", &history, false);
        set_draft(&conn, &write("history", 1, payload(""))).unwrap();
        let bot = directory.file("bot", Duration::from_secs(7200));
        persist_history(&conn, "bot", &bot, false);
        crate::bot_transcript::migrate_transcript(&conn, "bot").unwrap();
        let queued = directory.file("queued", Duration::from_secs(7200));
        persist_history(&conn, "queued", &queued, true);
        let receipt = directory.file("receipt", Duration::from_secs(7200));
        conn.execute("INSERT INTO local_submissions(attempt_id,user_turn_id,session_id,signature,user_turn_json,phase,owner_pid,owner_instance,created_at,updated_at) VALUES ('attempt','turn','history','hash',?1,'uncertain',1,'old',1,1)", [json!({"attachments":[{"path":receipt.to_string_lossy()}]}).to_string()]).unwrap();
        let snapshot = directory.file("snapshot", Duration::from_secs(7200));
        conn.execute(
            "INSERT INTO workspace_snapshot(id,snapshot_json,updated_at) VALUES (1,?1,1)",
            [json!({"attachments":[{"path":snapshot.to_string_lossy()}]}).to_string()],
        )
        .unwrap();
        let note = directory.file("note", Duration::from_secs(7200));
        conn.execute("INSERT INTO notes(id,slug,title,body,created_at,updated_at) VALUES ('note','note','Note',?1,1,1)", [format!("[file]({})", note.display())]).unwrap();
        let future = directory.file("future", Duration::from_secs(7200));
        conn.execute_batch("CREATE TABLE \"future\"\"store\" (\"reference\"\"field\" TEXT);")
            .unwrap();
        // Raw malformed rows are still retained for recovery, without relying
        // on a particular JSON schema or parsing a whole transcript in memory.
        conn.execute(
            "INSERT INTO \"future\"\"store\" VALUES (?1)",
            [format!("{{broken {}", future.display())],
        )
        .unwrap();
        assert_eq!(cold_collect(&conn, &directory), 0);
        for file in [history, bot, queued, receipt, snapshot, note, future] {
            assert!(file.exists());
        }
    }

    #[test]
    fn composer_attachment_gc_keeps_publication_grace_and_never_follows_symlinks() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let directory = CacheDirectory::new();
        let fresh = directory.file("in-flight", Duration::from_secs(1800));
        let old = directory.file("orphan", Duration::from_secs(7200));
        #[cfg(unix)]
        {
            let external = directory.0.join("outside-cache");
            std::fs::write(&external, "keep").unwrap();
            let link = directory
                .0
                .join(ATTACHMENT_DIRECTORY)
                .join(format!("{}-link", uuid::Uuid::new_v4()));
            std::os::unix::fs::symlink(&external, &link).unwrap();
        }
        assert_eq!(cold_collect(&conn, &directory), 1);
        assert!(fresh.exists());
        assert!(!old.exists());
        #[cfg(unix)]
        assert!(directory.0.join("outside-cache").exists());
    }

    #[test]
    fn composer_attachment_gc_live_and_same_process_store_leases_fence_collection() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let directory = CacheDirectory::new();
        let file = directory.file("live-unsaved-after-conflict", Duration::from_secs(7200));
        assert!(register_attachment_process(&conn, "first", 100, |_| true).unwrap());
        assert!(
            !register_attachment_process(&conn, "same-process-second-store", 100, |_| true)
                .unwrap()
        );
        assert!(!register_attachment_process(&conn, "another-process", 200, |_| true).unwrap());
        assert_eq!(
            collect_cached_attachments(
                &conn,
                &directory.0,
                SystemTime::now(),
                "first",
                &AtomicBool::new(false),
                |_| true
            )
            .unwrap(),
            0
        );
        assert!(file.exists());
        // Once prior owners have exited, a later cold process can reclaim it.
        assert!(register_attachment_process(&conn, "cold", 300, |_| false).unwrap());
        assert_eq!(
            collect_cached_attachments(
                &conn,
                &directory.0,
                SystemTime::now(),
                "cold",
                &AtomicBool::new(false),
                |_| false
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn composer_attachment_gc_foreground_mutation_cancels_pending_transfer_collection() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let directory = CacheDirectory::new();
        let file = directory.file("pending-send", Duration::from_secs(7200));
        set_draft(&conn, &write("sending", 0, with_file(&file))).unwrap();
        assert!(register_attachment_process(&conn, "cold", 100, |_| false).unwrap());
        let activity = Arc::new(AtomicBool::new(false));
        let hook = Arc::clone(&activity);
        conn.update_hook(Some(
            move |_: rusqlite::hooks::Action, _: &str, _: &str, _: i64| {
                hook.store(true, Ordering::SeqCst);
            },
        ))
        .unwrap();
        // Send clears the draft before the subsequent history write; the
        // activity fence protects this now-only-in-memory attachment path.
        set_draft(&conn, &write("sending", 1, payload(""))).unwrap();
        assert!(activity.load(Ordering::SeqCst));
        assert_eq!(
            collect_cached_attachments(
                &conn,
                &directory.0,
                SystemTime::now(),
                "cold",
                &activity,
                |_| false
            )
            .unwrap(),
            0
        );
        assert!(file.exists());
        persist_history(&conn, "sending", &file, false);
        assert_eq!(cold_collect(&conn, &directory), 0);
        assert!(file.exists());
    }

    #[test]
    fn composer_attachment_gc_streams_history_once_and_preserves_everything_on_read_error() {
        use rusqlite::hooks::{AuthAction, Authorization};
        use std::sync::atomic::AtomicUsize;
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let directory = CacheDirectory::new();
        let kept = directory.file("history", Duration::from_secs(7200));
        persist_history(&conn, "history", &kept, false);
        for _ in 0..20 {
            directory.file("orphan", Duration::from_secs(7200));
        }
        let reads = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&reads);
        conn.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Read {
                    table_name: "sessions",
                    column_name: "blocks_json"
                }
            ) {
                observed.fetch_add(1, Ordering::SeqCst);
            }
            Authorization::Allow
        }))
        .unwrap();
        assert_eq!(cold_collect(&conn, &directory), 20);
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert!(kept.exists());
        let orphan = directory.file("keep-on-read-error", Duration::from_secs(7200));
        conn.authorizer(Some(|context: rusqlite::hooks::AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Read {
                    table_name: "sessions",
                    column_name: "blocks_json"
                }
            ) {
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }))
        .unwrap();
        assert_eq!(cold_collect(&conn, &directory), 0);
        assert!(orphan.exists());
    }

    #[test]
    fn composer_attachment_gc_parallel_openers_register_one_cold_start_atomically() {
        let directory = CacheDirectory::new();
        let database = directory.0.join("fixture.db");
        let conn = Connection::open(&database).unwrap();
        ensure_table(&conn).unwrap();
        drop(conn);
        let file = directory.file("unpublished", Duration::from_secs(7200));
        let barrier = Arc::new(Barrier::new(2));
        let threads = ["one", "two"]
            .into_iter()
            .map(|instance| {
                let database = database.clone();
                let root = directory.0.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let conn = Connection::open(database).unwrap();
                    conn.busy_timeout(Duration::from_secs(5)).unwrap();
                    barrier.wait();
                    let eligible =
                        register_attachment_process(&conn, instance, 100, |_| true).unwrap();
                    let removed = if eligible {
                        collect_cached_attachments(
                            &conn,
                            &root,
                            SystemTime::now(),
                            instance,
                            &AtomicBool::new(false),
                            |_| true,
                        )
                        .unwrap()
                    } else {
                        0
                    };
                    (eligible, removed)
                })
            })
            .collect::<Vec<_>>();
        let results = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|(eligible, _)| *eligible).count(), 1);
        let removed: usize = results.iter().map(|(_, count)| count).sum();
        assert!(removed <= 1);
        assert_eq!(file.exists(), removed == 0);
        let conn = Connection::open(database).unwrap();
        let owners: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM composer_attachment_processes",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owners, 2);
    }

    #[test]
    fn composer_attachment_gc_expired_scan_budget_preserves_all_candidates() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let directory = CacheDirectory::new();
        let first = directory.file("first", Duration::from_secs(7200));
        let second = directory.file("second", Duration::from_secs(7200));
        let result =
            scan_attachment_collection(&conn, &directory.0, SystemTime::now(), Duration::ZERO);
        assert!(result.unwrap_err().contains("budget exceeded"));
        assert!(first.exists());
        assert!(second.exists());
    }

    #[test]
    fn composer_attachment_gc_read_scan_allows_publication_and_rechecks_data_version() {
        use rusqlite::hooks::{AuthAction, Authorization};
        let directory = CacheDirectory::new();
        let database = directory.0.join("publication.db");
        let conn = Connection::open(&database).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; CREATE TABLE durable_roots(reference TEXT);",
        )
        .unwrap();
        conn.busy_timeout(Duration::from_millis(100)).unwrap();
        ensure_table(&conn).unwrap();
        assert!(register_attachment_process(&conn, "collector", 1, |_| false).unwrap());
        let file = directory.file("publishing", Duration::from_secs(7200));
        let (entered, scanning) = std::sync::mpsc::sync_channel(0);
        let (resume, resumed) = std::sync::mpsc::sync_channel(0);
        let root = directory.0.clone();
        let worker = std::thread::spawn(move || {
            let conn = Connection::open(database).unwrap();
            let mut paused = false;
            conn.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
                if !paused
                    && matches!(
                        context.action,
                        AuthAction::Read {
                            table_name: "durable_roots",
                            column_name: "reference"
                        }
                    )
                {
                    paused = true;
                    entered.send(()).unwrap();
                    resumed.recv().unwrap();
                }
                Authorization::Allow
            }))
            .unwrap();
            collect_cached_attachments(
                &conn,
                &root,
                SystemTime::now(),
                "collector",
                &AtomicBool::new(false),
                |_| false,
            )
            .unwrap()
        });
        scanning.recv_timeout(Duration::from_secs(5)).unwrap();
        let published = conn.execute(
            "INSERT INTO durable_roots(reference) VALUES (?1)",
            [file.to_string_lossy().as_ref()],
        );
        // Always release the scanner before asserting, including on a regression.
        resume.send(()).unwrap();
        let removed = worker.join().unwrap();
        assert!(
            published.is_ok(),
            "A background reference scan reserved the writer: {published:?}"
        );
        assert_eq!(removed, 0);
        assert!(file.exists());
    }

    #[cfg(unix)]
    #[test]
    fn composer_attachment_gc_never_collects_through_a_symlink_cache_root() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let directory = CacheDirectory::new();
        let file = directory.file("outside-owned-cache", Duration::from_secs(7200));
        let target = directory.0.join("different-directory");
        std::fs::rename(directory.0.join(ATTACHMENT_DIRECTORY), &target).unwrap();
        std::os::unix::fs::symlink(&target, directory.0.join(ATTACHMENT_DIRECTORY)).unwrap();
        assert_eq!(cold_collect(&conn, &directory), 0);
        assert!(target.join(file.file_name().unwrap()).exists());
    }

    struct Database(PathBuf);

    impl Database {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("composer-drafts-{}.db", uuid::Uuid::new_v4())))
        }

        fn open(&self) -> SessionStore {
            SessionStore::open(self.0.clone()).unwrap()
        }
    }

    impl Drop for Database {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_file(self.0.with_extension("db-wal"));
            let _ = std::fs::remove_file(self.0.with_extension("db-shm"));
        }
    }

    fn payload(text: &str) -> ComposerDraftPayload {
        ComposerDraftPayload {
            text: text.into(),
            selection_start: text.encode_utf16().count() as u32,
            selection_end: text.encode_utf16().count() as u32,
            ..empty_payload("/projects/example".into())
        }
    }

    fn write(
        session_id: &str,
        expected_revision: u64,
        payload: ComposerDraftPayload,
    ) -> ComposerDraftWrite {
        ComposerDraftWrite {
            session_id: session_id.into(),
            expected_revision,
            payload,
        }
    }

    #[test]
    fn composer_draft_cached_clipboard_bytes_survive_reopen_privately() {
        let directory =
            std::env::temp_dir().join(format!("composer-attachment-{}", uuid::Uuid::new_v4()));
        let path = cache_attachment(&directory, "../../pasted-照片.png", "aGVsbG8=").unwrap();
        assert!(std::path::Path::new(&path).starts_with(directory.join("composer-attachments")));
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(directory.join("composer-attachments"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert!(cache_attachment(&directory, "bad", "not-base64").is_err());
        assert!(cache_attachment(&directory, "large", &"x".repeat(28 * 1024 * 1024)).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn composer_draft_missing_unsaved_or_remote_identity_is_empty() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        for id in ["unsaved-local", "remote:host:session"] {
            let draft = get_draft(&conn, id).unwrap();
            assert_eq!(draft.revision, 0);
            assert_eq!(draft.session_id, id);
            assert_eq!(draft.payload, empty_payload(String::new()));
            assert_eq!(
                set_draft(&conn, &write(id, 0, payload("input")))
                    .unwrap()
                    .revision,
                1
            );
        }
        for id in ["", " trailing", "line\nbreak"] {
            assert_eq!(get_draft(&conn, id).unwrap_err().code, "invalidDraft");
        }
    }

    #[test]
    fn composer_draft_exact_unicode_selection_and_ordered_metadata_survive_reopen() {
        let database = Database::new();
        let mut expected = payload("before 👩🏽‍💻\ne\u{301} 中文\r\nafter");
        expected.selection_start = 7;
        expected.selection_end = 14;
        expected.attachments = serde_json::from_value(json!([
            {"id":"second","name":"照片.png","mimeType":"image/png","kind":"image","size":42,"path":"/projects/example/照片.png"},
            {"id":"first","name":"notes.txt","mimeType":"text/plain","kind":"file","size":12,"path":"C:\\work\\notes.txt"},
            {"id":"missing","name":"clipboard.png","mimeType":"image/png","kind":"image","size":20}
        ])).unwrap();
        expected.mcp_tags = serde_json::from_value(json!([
            {"token":"@mcp/工具","server":{"provider":"claude_desktop","name":"工具","scope":"user","configPath":"/config/desktop.json","transport":"stdio","enabled":false}},
            {"token":"@mcp/project","server":{"provider":"codex","name":"project","scope":"project","configPath":"","transport":""}}
        ])).unwrap();
        let committed;
        {
            let store = database.open();
            let conn = store.lock_conn().unwrap();
            committed =
                set_draft(&conn, &write("remote:host:session", 0, expected.clone())).unwrap();
        }
        let reopened = database.open();
        let conn = reopened.lock_conn().unwrap();
        let restored = get_draft(&conn, "remote:host:session").unwrap();
        assert_eq!(restored, committed);
        assert_eq!(restored.payload, expected);
        assert_eq!(restored.revision, 1);
    }

    #[test]
    fn composer_draft_utf16_offsets_are_not_byte_or_scalar_counts() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mut draft = payload("A😀中");
        assert_eq!(draft.selection_end, 4);
        assert!(set_draft(&conn, &write("unicode", 0, draft.clone())).is_ok());
        // DOM APIs can place a selection between surrogate code units. Keep
        // the exact offset rather than snapping it to a Rust char boundary.
        draft.selection_start = 2;
        draft.selection_end = 2;
        assert!(set_draft(&conn, &write("unicode", 1, draft.clone())).is_ok());
        for (start, end) in [(0, 5), (3, 2)] {
            draft.selection_start = start;
            draft.selection_end = end;
            assert_eq!(
                set_draft(&conn, &write("unicode", 2, draft.clone()))
                    .unwrap_err()
                    .code,
                "invalidDraft"
            );
        }
        assert_eq!(get_draft(&conn, "unicode").unwrap().revision, 2);
    }

    #[test]
    fn composer_draft_duplicate_retry_and_matching_noop_are_idempotent() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let request = write("retry", 0, payload("one"));
        let committed = set_draft(&conn, &request).unwrap();
        assert_eq!(set_draft(&conn, &request).unwrap(), committed);
        assert_eq!(
            set_draft(&conn, &write("retry", 1, request.payload)).unwrap(),
            committed
        );
        assert_eq!(
            set_draft(&conn, &write("retry", 0, payload("different")))
                .unwrap_err()
                .code,
            "conflict"
        );
    }

    #[test]
    fn composer_draft_two_connections_cannot_overwrite_a_newer_revision() {
        let database = Database::new();
        let first = database.open();
        let second = database.open();
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = [first, second]
            .into_iter()
            .enumerate()
            .map(|(index, store)| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let conn = store.lock_conn().unwrap();
                    conn.busy_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    assert_eq!(get_draft(&conn, "two-windows").unwrap().revision, 0);
                    barrier.wait();
                    set_draft(
                        &conn,
                        &write("two-windows", 0, payload(&format!("window-{index}"))),
                    )
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let conflict = results
            .iter()
            .find_map(|result| result.as_ref().err())
            .unwrap();
        assert_eq!(conflict.code, "conflict");
        let winner = results
            .iter()
            .find_map(|result| result.as_ref().ok())
            .unwrap();
        assert_eq!(conflict.current.as_deref().unwrap(), winner);
        let reopened = database.open();
        assert_eq!(
            get_draft(&reopened.lock_conn().unwrap(), "two-windows").unwrap(),
            *winner
        );
    }

    #[test]
    fn composer_draft_clear_tombstone_defeats_delayed_write_after_reopen() {
        let database = Database::new();
        {
            let store = database.open();
            let conn = store.lock_conn().unwrap();
            let first = set_draft(&conn, &write("clear", 0, payload("send this"))).unwrap();
            let cleared = set_draft(
                &conn,
                &write("clear", first.revision, empty_payload(first.payload.cwd)),
            )
            .unwrap();
            assert_eq!(cleared.revision, 2);
            // Clearing an untouched composer also creates a revision fence.
            assert_eq!(
                set_draft(&conn, &write("never-saved", 0, payload("")))
                    .unwrap()
                    .revision,
                1
            );
        }
        let store = database.open();
        let conn = store.lock_conn().unwrap();
        for expected in [0, 1] {
            let failure = set_draft(
                &conn,
                &write("clear", expected, payload("old delayed text")),
            )
            .unwrap_err();
            assert_eq!(failure.code, "conflict");
            assert_eq!(failure.current.unwrap().revision, 2);
        }
        assert!(get_draft(&conn, "clear").unwrap().payload.text.is_empty());
    }

    #[test]
    fn composer_draft_cwd_changes_require_cas_and_current_session_scope() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        conn.execute("INSERT INTO sessions (id,cwd,worktree_cwd,harness,model,runtime_mode,title,created_at,updated_at) VALUES ('scoped','/projects/example','/worktrees/example','codex','test','supervised','Example',1,1)", []).unwrap();
        assert_eq!(
            get_draft(&conn, "scoped").unwrap().payload.cwd,
            "/projects/example"
        );
        set_draft(&conn, &write("scoped", 0, payload("draft"))).unwrap();
        let mut changed = payload("worktree");
        changed.cwd = "/worktrees/example".into();
        set_draft(&conn, &write("scoped", 1, changed.clone())).unwrap();
        changed.cwd = "/unrelated".into();
        assert_eq!(
            set_draft(&conn, &write("scoped", 2, changed))
                .unwrap_err()
                .code,
            "scopeMismatch"
        );
        conn.execute(
            "UPDATE sessions SET cwd = '/moved', worktree_cwd = NULL WHERE id = 'scoped'",
            [],
        )
        .unwrap();
        // Hydration retains the old saved context for an explicit frontend
        // decision; a matching CAS may then save the new session context.
        assert_eq!(
            get_draft(&conn, "scoped").unwrap().payload.cwd,
            "/worktrees/example"
        );
        let mut moved = payload("moved");
        moved.cwd = "/moved".into();
        assert_eq!(
            set_draft(&conn, &write("scoped", 2, moved))
                .unwrap()
                .revision,
            3
        );
    }

    #[test]
    fn composer_draft_malformed_rows_are_never_overwritten() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mut cases = vec!["{broken".to_string()];
        let mut extra = serde_json::to_value(payload("recover me")).unwrap();
        extra["data"] = json!("bytes");
        cases.push(extra.to_string());
        let mut wrong_cwd = payload("recover me");
        wrong_cwd.cwd = "/different".into();
        cases.push(serde_json::to_string(&wrong_cwd).unwrap());
        let mut invalid_selection = payload("😀");
        invalid_selection.selection_end = 3;
        cases.push(serde_json::to_string(&invalid_selection).unwrap());
        for (index, raw) in cases.into_iter().enumerate() {
            let id = format!("corrupt-{index}");
            conn.execute("INSERT INTO composer_drafts (session_id,cwd,revision,payload_json,updated_at) VALUES (?1,'/projects/example',7,?2,1)", params![id, raw]).unwrap();
            assert_eq!(get_draft(&conn, &id).unwrap_err().code, "corruptDraft");
            assert_eq!(
                set_draft(&conn, &write(&id, 7, payload("replacement")))
                    .unwrap_err()
                    .code,
                "corruptDraft"
            );
            let preserved: (String, i64) = conn
                .query_row(
                    "SELECT payload_json,revision FROM composer_drafts WHERE session_id = ?1",
                    [&id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(preserved, (raw, 7));
        }
    }

    #[test]
    fn composer_draft_validation_limits_and_metadata_only_contract() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mut too_long = payload("");
        too_long.text = "x".repeat(MAX_TEXT_BYTES + 1);
        let mut too_many = payload("");
        too_many.attachments = (0..=MAX_ITEMS)
            .map(|index| ComposerDraftAttachment {
                id: index.to_string(),
                name: "a".into(),
                mime_type: "".into(),
                kind: ComposerDraftAttachmentKind::File,
                size: 0,
                path: None,
            })
            .collect();
        let mut too_large_aggregate = payload("");
        too_large_aggregate.text = "\0".repeat(2 * 1024 * 1024);
        for invalid in [too_long, too_many, too_large_aggregate] {
            assert_eq!(
                set_draft(&conn, &write("limits", 0, invalid))
                    .unwrap_err()
                    .code,
                "invalidDraft"
            );
        }
        assert_eq!(get_draft(&conn, "limits").unwrap().revision, 0);
        let attachment = json!({"id":"file","name":"image","kind":"image","mimeType":"image/png","size":1,"path":"/image.png"});
        for forbidden in ["data", "previewUrl", "copyFromPath"] {
            let mut metadata = attachment.clone();
            metadata[forbidden] = json!("must not persist");
            assert!(serde_json::from_value::<ComposerDraftAttachment>(metadata).is_err());
        }
        for path in [
            "data:image/png;base64,AAAA",
            "blob:https://example.com/id",
            "https://example.com/image",
        ] {
            let mut draft = payload("");
            let mut metadata = attachment.clone();
            metadata["path"] = json!(path);
            draft
                .attachments
                .push(serde_json::from_value(metadata).unwrap());
            assert_eq!(
                set_draft(&conn, &write("limits", 0, draft))
                    .unwrap_err()
                    .code,
                "invalidDraft"
            );
        }
        let mut remote = payload("Remote input");
        remote.cwd = "remote://machine/home/user/project".into();
        let saved = set_draft(&conn, &write("remote-owner", 0, remote.clone())).unwrap();
        assert_eq!(saved.payload, remote);
        assert_eq!(get_draft(&conn, "remote-owner").unwrap(), saved);
    }

    #[test]
    fn composer_draft_deletion_fences_existing_and_never_saved_inputs() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        set_draft(&conn, &write("existing", 0, payload("remove me"))).unwrap();
        for id in ["existing", "never-saved"] {
            delete_session_draft(&conn, id).unwrap();
            delete_session_draft(&conn, id).unwrap();
            for expected in [0, 1] {
                assert_eq!(
                    set_draft(&conn, &write(id, expected, payload("delayed")))
                        .unwrap_err()
                        .code,
                    "sessionDeleted"
                );
            }
            assert_eq!(get_draft(&conn, id).unwrap_err().code, "sessionDeleted");
            let raw: String = conn
                .query_row(
                    "SELECT payload_json FROM composer_drafts WHERE session_id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(serde_json::from_str::<ComposerDraftPayload>(&raw)
                .unwrap()
                .text
                .is_empty());
        }
    }

    #[test]
    fn composer_draft_deletion_preserves_malformed_raw_payload() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        conn.execute("INSERT INTO composer_drafts (session_id,cwd,revision,payload_json,updated_at) VALUES ('corrupt','/projects/example',7,'{broken',1)", []).unwrap();
        delete_session_draft(&conn, "corrupt").unwrap();
        let raw: String = conn
            .query_row(
                "SELECT payload_json FROM composer_drafts WHERE session_id = 'corrupt'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(raw, "{broken");
        assert_eq!(
            set_draft(&conn, &write("corrupt", 7, payload("replace")))
                .unwrap_err()
                .code,
            "sessionDeleted"
        );
    }
}
