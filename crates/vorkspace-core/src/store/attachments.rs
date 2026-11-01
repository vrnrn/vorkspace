//! Private composer attachment ownership and conservative cold-start collection.
use super::Store;
use rusqlite::{params, Connection, Transaction, TransactionBehavior};
use std::io::Write;
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock, Weak,
    },
    time::{Duration, Instant, SystemTime},
};
const ATTACHMENT_DIRECTORY: &str = "composer-attachments";
const ATTACHMENT_PUBLICATION_GRACE: Duration = Duration::from_secs(60 * 60);
const ATTACHMENT_COLLECTION_BUDGET: Duration = Duration::from_millis(250);
const SIDECAR_SCAN_BYTES: u64 = 8 * 1024 * 1024;
fn activity_registry() -> &'static Mutex<BTreeMap<PathBuf, Weak<AtomicBool>>> {
    static ACTIVITIES: OnceLock<Mutex<BTreeMap<PathBuf, Weak<AtomicBool>>>> = OnceLock::new();
    ACTIVITIES.get_or_init(Mutex::default)
}
fn activity_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
impl Store {
    /// Real UI startup admits its process before any view can retain cached paths.
    /// Read-only validation and ordinary store reads never create leases or scan.
    pub fn start_attachment_cache(&self) -> Result<(), String> {
        if self.path == Path::new(":memory:") {
            return Ok(());
        }
        let instance = admit_attachment_cache(&self.conn).map_err(|e| e.to_string())?;
        initialize_attachment_cache(&self.conn, &self.path, instance).map_err(|e| e.to_string())
    }
    pub(super) fn mark_attachment_activity(&self) {
        if let Some(activity) = activity_registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&activity_key(&self.path))
            .and_then(Weak::upgrade)
        {
            activity.store(true, Ordering::SeqCst);
        }
    }
    pub fn cache_attachment(&self, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        self.mark_attachment_activity();
        let root = self.data_dir().ok_or("Attachment storage is unavailable")?;
        cache_attachment(root, name, bytes)
    }
}
fn pid_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    unsafe {
        libc::kill(pid as i32, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}
fn cache_attachment(root: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if bytes.len() > crate::composer::MAX_EMBED_BYTES as usize {
        return Err("Composer attachment exceeds 20 MiB".into());
    }
    let directory = root.join(ATTACHMENT_DIRECTORY);
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    if std::fs::symlink_metadata(&directory)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("Attachment cache must not be a symlink".into());
    }
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    let basename = name.rsplit(['/', '\\']).next().unwrap_or("attachment");
    let mut size = 0;
    let name: String = basename
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | ':'))
        .take_while(|c| {
            size += c.len_utf8();
            size <= 120
        })
        .collect();
    let name = if name.trim_matches('.').is_empty() {
        "attachment"
    } else {
        &name
    };
    let path = directory.join(format!("{}-{name}", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
    let result = (|| {
        let mut file = options.open(&path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        // A durable path needs both the file contents and its directory
        // entry, including first creation of the cache directory itself.
        std::fs::File::open(&directory)?.sync_all()?;
        std::fs::File::open(root)?.sync_all()?;
        Ok::<_, std::io::Error>(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&path);
        return Err(error.to_string());
    }
    Ok(path)
}
pub(crate) fn admit_attachment_cache(conn: &Connection) -> rusqlite::Result<Option<String>> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS composer_attachment_processes (owner_instance TEXT PRIMARY KEY, owner_pid INTEGER NOT NULL);")?;
    let instance = uuid::Uuid::new_v4().to_string();
    if !register_attachment_process(conn, &instance, std::process::id(), pid_alive)? {
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
    {
        let mut registry = activity_registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        registry.retain(|_, value| value.strong_count() > 0);
        registry.insert(activity_key(database), Arc::downgrade(&activity));
    }

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
                        &conn, root, cutoff, &instance, &activity, pid_alive,
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
    let sidecar = root.join(format!(
        "{}.gpui.json",
        conn.path()
            .and_then(|p| Path::new(p).file_name())
            .unwrap_or_default()
            .to_string_lossy()
    ));
    match std::fs::metadata(&sidecar) {
        Ok(metadata) => {
            if metadata.len() > SIDECAR_SCAN_BYTES {
                return Err(
                    "Attachment cleanup sidecar scan budget exceeded; all files preserved".into(),
                );
            }
            let raw = std::fs::read_to_string(&sidecar).map_err(|e| e.to_string())?;
            collect_attachment_references(&raw, &ids, &mut referenced, deadline)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
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

#[cfg(test)]
mod tests;
