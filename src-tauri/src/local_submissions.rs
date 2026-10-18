//! Durable local acceptance receipts. A reservation never authorizes replay:
//! provider I/O and SQLite cannot form an exactly-once transaction.
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::State;

use crate::session_store::SessionStore;

const MAX_PAYLOAD_BYTES: usize = 64 << 20;
const MAX_USER_TURN_BYTES: usize = 8 << 20;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionInput {
    pub attempt_id: String,
    pub user_turn_id: String,
    pub session_id: String,
    pub payload_json: String,
    pub user_turn: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionReceipt {
    pub attempt_id: String,
    pub user_turn_id: String,
    pub session_id: String,
    pub signature: String,
    pub phase: String,
    pub outcome: Option<String>,
    pub error: Option<String>,
    pub user_turn: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionReservation {
    pub created: bool,
    pub receipt: SubmissionReceipt,
}

#[derive(Serialize)]
pub struct SubmissionClaim {
    pub claimed: bool,
    pub receipt: SubmissionReceipt,
}

pub(crate) fn ensure_table(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS local_submissions (
           attempt_id TEXT PRIMARY KEY,
           user_turn_id TEXT NOT NULL,
           session_id TEXT NOT NULL,
           signature TEXT NOT NULL,
           user_turn_json TEXT,
           phase TEXT NOT NULL CHECK(phase IN ('reserved','dispatching','settled','uncertain')),
           outcome TEXT,
           error TEXT,
           owner_pid INTEGER NOT NULL,
           owner_instance TEXT NOT NULL,
           created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS local_submission_deleted_sessions (session_id TEXT PRIMARY KEY);
         CREATE INDEX IF NOT EXISTS local_submissions_session_idx
           ON local_submissions(session_id, created_at);",
    )
}

fn instance() -> &'static str {
    static INSTANCE: OnceLock<String> = OnceLock::new();
    INSTANCE.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub(crate) fn pid_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    #[cfg(unix)]
    {
        if pid == 0 || pid > i32::MAX as u32 {
            return false;
        }
        unsafe {
            libc::kill(pid as i32, 0) == 0
                || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                // Access denied does not prove a prior owner has gone.
                return std::io::Error::last_os_error().raw_os_error() == Some(5);
            }
            let mut code = 0;
            let queried = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            queried == 0 || code == 259
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        true
    }
}

/// Called on disk open, never on ordinary reads or a second window. Live
/// owners (including this process) retain their reservations untouched.
pub(crate) fn recover_unresolved(conn: &Connection) -> rusqlite::Result<()> {
    let mut statement = conn.prepare("SELECT DISTINCT owner_pid, owner_instance FROM local_submissions WHERE phase IN ('reserved','dispatching')")?;
    let owners = statement
        .query_map([], |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (pid, owner) in owners {
        if owner == instance() || pid_alive(pid) {
            continue;
        }
        conn.execute("UPDATE local_submissions SET phase='uncertain', outcome=NULL, error='The app closed before this local send could be confirmed. The provider may have received it; it was not replayed.', updated_at=?1 WHERE owner_instance=?2 AND phase IN ('reserved','dispatching')", params![now(), owner])?;
    }
    Ok(())
}

pub(crate) fn delete_session(conn: &Connection, session_id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO local_submission_deleted_sessions(session_id) VALUES (?1)",
        [session_id],
    )?;
    // Keep identity/signature tombstones; a delayed retry cannot reserve this
    // attempt again under a different session after transcript deletion.
    conn.execute("UPDATE local_submissions SET phase='settled',outcome='failed',error='This session was deleted; this attempt cannot be dispatched.',user_turn_json=NULL,updated_at=?1 WHERE session_id=?2", params![now(),session_id])?;
    Ok(())
}

fn validate_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 512
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-:".contains(&c))
    {
        return Err("Invalid local submission identity".into());
    }
    Ok(())
}

fn get(conn: &Connection, attempt_id: &str) -> Result<Option<SubmissionReceipt>, String> {
    conn.query_row("SELECT attempt_id,user_turn_id,session_id,signature,phase,outcome,error,user_turn_json FROM local_submissions WHERE attempt_id=?1", [attempt_id], receipt_row).optional().map_err(|e| e.to_string())
}

fn receipt_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SubmissionReceipt> {
    Ok(SubmissionReceipt {
        attempt_id: row.get(0)?,
        user_turn_id: row.get(1)?,
        session_id: row.get(2)?,
        signature: row.get(3)?,
        phase: row.get(4)?,
        outcome: row.get(5)?,
        error: row.get(6)?,
        user_turn: row
            .get::<_, Option<String>>(7)?
            .and_then(|raw| serde_json::from_str(&raw).ok()),
    })
}

fn sanitize_user_turn(
    value: serde_json::Value,
    user_turn_id: &str,
) -> Result<serde_json::Value, String> {
    if value["id"].as_str() != Some(user_turn_id) || value["role"].as_str() != Some("user") {
        return Err("Local submission user turn does not match its identity".into());
    }
    let text = value["text"]
        .as_str()
        .ok_or("Local submission user turn needs text")?;
    if text.len() > MAX_USER_TURN_BYTES {
        return Err("Local submission recovery row is too large".into());
    }
    let mut clean = serde_json::json!({"id":user_turn_id,"role":"user","text":text});
    for key in ["internal", "monocode", "localSubmissionOwned"] {
        if value[key] == true {
            clean[key] = serde_json::Value::Bool(true);
        }
    }
    if matches!(value["intent"].as_str(), Some("plan" | "orchestrate")) {
        clean["intent"] = value["intent"].clone();
    }
    if let Some(request) = value["appRequestId"].as_str().filter(|id| id.len() <= 512) {
        clean["appRequestId"] = request.into();
    }
    if let Some(attachments) = value["attachments"].as_array() {
        if attachments.len() > 512 {
            return Err("Too many local submission attachment metadata rows".into());
        }
        let mut metadata = Vec::new();
        for attachment in attachments {
            let mut file = serde_json::Map::new();
            for key in ["id", "name", "mimeType", "kind"] {
                let field = attachment[key]
                    .as_str()
                    .filter(|text| text.len() <= 65536)
                    .ok_or("Invalid local submission attachment metadata")?;
                file.insert(key.into(), field.into());
            }
            if !matches!(
                attachment["kind"].as_str(),
                Some("image" | "audio" | "file")
            ) {
                return Err("Invalid local submission attachment kind".into());
            }
            let size = attachment["size"]
                .as_u64()
                .ok_or("Invalid local submission attachment size")?;
            file.insert("size".into(), size.into());
            if let Some(path) = attachment["path"].as_str().filter(|path| {
                path.len() <= 65536 && !path.starts_with("blob:") && !path.starts_with("data:")
            }) {
                file.insert("path".into(), path.into());
            }
            metadata.push(serde_json::Value::Object(file));
        }
        clean["attachments"] = metadata.into();
    }
    if clean.to_string().len() > MAX_USER_TURN_BYTES {
        return Err("Local submission recovery row is too large".into());
    }
    Ok(clean)
}

fn reserve(conn: &Connection, input: SubmissionInput) -> Result<SubmissionReservation, String> {
    for value in [&input.attempt_id, &input.user_turn_id] {
        validate_id(value)?;
    }
    if input.session_id.is_empty()
        || input.session_id.len() > 4096
        || input.session_id.contains('\0')
    {
        return Err("Invalid local submission session identity".into());
    }
    if input.payload_json.len() > MAX_PAYLOAD_BYTES {
        return Err("Local submission payload exceeds the receipt limit".into());
    }
    let payload: serde_json::Value =
        serde_json::from_str(&input.payload_json).map_err(|e| e.to_string())?;
    if !payload.is_object() {
        return Err("Local submission payload must be an object".into());
    }
    let user_turn = input
        .user_turn
        .map(|turn| sanitize_user_turn(turn, &input.user_turn_id))
        .transpose()?;
    let signature = format!("{:x}", Sha256::digest(input.payload_json.as_bytes()));
    let created = conn.execute("INSERT INTO local_submissions (attempt_id,user_turn_id,session_id,signature,phase,owner_pid,owner_instance,created_at,updated_at,user_turn_json) SELECT ?1,?2,?3,?4,'reserved',?5,?6,?7,?7,?8 WHERE NOT EXISTS (SELECT 1 FROM local_submission_deleted_sessions WHERE session_id=?3) AND EXISTS (SELECT 1 FROM sessions WHERE id=?3) ON CONFLICT(attempt_id) DO NOTHING", params![input.attempt_id,input.user_turn_id,input.session_id,signature,std::process::id(),instance(),now(),user_turn.as_ref().map(serde_json::Value::to_string)]).map_err(|e| e.to_string())? == 1;
    let receipt = get(conn, &input.attempt_id)?.ok_or(
        "The local send session has not been saved or was deleted; the send was not reserved",
    )?;
    if receipt.session_id != input.session_id
        || receipt.user_turn_id != input.user_turn_id
        || receipt.signature != signature
        || receipt.user_turn != user_turn
    {
        return Err("Local submission ID was already used with different scope or content".into());
    }
    Ok(SubmissionReservation { created, receipt })
}

fn dispatch(conn: &Connection, attempt_id: &str) -> Result<SubmissionClaim, String> {
    let claimed = conn.execute("UPDATE local_submissions SET phase='dispatching',updated_at=?1 WHERE attempt_id=?2 AND phase='reserved' AND owner_instance=?3", params![now(),attempt_id,instance()]).map_err(|e| e.to_string())? == 1;
    let receipt = get(conn, attempt_id)?.ok_or("Local submission does not exist")?;
    Ok(SubmissionClaim { claimed, receipt })
}

fn settle(
    conn: &Connection,
    attempt_id: &str,
    outcome: &str,
    error: Option<String>,
) -> Result<SubmissionReceipt, String> {
    if !["completed", "failed", "uncertain"].contains(&outcome) {
        return Err("Invalid local submission outcome".into());
    }
    let phase = if outcome == "uncertain" {
        "uncertain"
    } else {
        "settled"
    };
    conn.execute("UPDATE local_submissions SET phase=?1,outcome=?2,error=?3,updated_at=?4 WHERE attempt_id=?5 AND phase='dispatching' AND owner_instance=?6", params![phase,if outcome == "uncertain" {None} else {Some(outcome)},error,now(),attempt_id,instance()]).map_err(|e| e.to_string())?;
    let receipt = get(conn, attempt_id)?.ok_or("Local submission does not exist")?;
    if receipt.phase != phase
        || receipt.outcome.as_deref()
            != (if outcome == "uncertain" {
                None
            } else {
                Some(outcome)
            })
    {
        return Err("Local submission can no longer be settled by this owner".into());
    }
    Ok(receipt)
}

#[tauri::command(async)]
pub fn local_submission_reserve(
    store: State<'_, SessionStore>,
    input: SubmissionInput,
) -> Result<SubmissionReservation, String> {
    let conn = store.lock_conn()?;
    reserve(&conn, input)
}
#[tauri::command(async)]
pub fn local_submission_dispatch(
    store: State<'_, SessionStore>,
    attempt_id: String,
) -> Result<SubmissionClaim, String> {
    let conn = store.lock_conn()?;
    dispatch(&conn, &attempt_id)
}
#[tauri::command(async)]
pub fn local_submission_settle(
    store: State<'_, SessionStore>,
    attempt_id: String,
    outcome: String,
    error: Option<String>,
) -> Result<SubmissionReceipt, String> {
    let conn = store.lock_conn()?;
    settle(&conn, &attempt_id, &outcome, error)
}
#[tauri::command(async)]
pub fn local_submission_get(
    store: State<'_, SessionStore>,
    attempt_id: String,
) -> Result<Option<SubmissionReceipt>, String> {
    let conn = store.lock_conn()?;
    get(&conn, &attempt_id)
}
#[tauri::command(async)]
pub fn local_submission_list(
    store: State<'_, SessionStore>,
    session_id: String,
) -> Result<Vec<SubmissionReceipt>, String> {
    let conn = store.lock_conn()?;
    let mut statement = conn.prepare("SELECT attempt_id,user_turn_id,session_id,signature,phase,outcome,error,user_turn_json FROM local_submissions WHERE session_id=?1 ORDER BY created_at,attempt_id").map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([session_id], receipt_row)
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn saved_owner(conn: &Connection) {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY);")
            .unwrap();
        conn.execute("INSERT INTO sessions(id) VALUES (?1)", ["bot/chat.中国"])
            .unwrap();
    }

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ensure_table(&conn).unwrap();
        saved_owner(&conn);
        conn
    }
    fn input(id: &str) -> SubmissionInput {
        SubmissionInput {
            attempt_id: id.into(),
            user_turn_id: id.into(),
            session_id: "bot/chat.中国".into(),
            payload_json: r#"{"cwd":"/repo","text":"hello","attachments":[{"data":"exact"}]}"#
                .into(),
            user_turn: Some(json!({"id":id,"role":"user","text":"hello"})),
        }
    }

    #[test]
    fn a_missing_session_owner_cannot_create_an_unreachable_receipt() {
        let conn = db();
        conn.execute("DELETE FROM sessions", []).unwrap();
        assert!(reserve(&conn, input("orphan"))
            .unwrap_err()
            .contains("has not been saved"));
        assert!(get(&conn, "orphan").unwrap().is_none());
        assert!(dispatch(&conn, "orphan").is_err());
    }

    #[test]
    fn matching_ids_share_one_reservation_and_only_one_dispatch_claim() {
        let conn = db();
        let first = reserve(&conn, input("same")).unwrap();
        assert!(first.created);
        assert!(!reserve(&conn, input("same")).unwrap().created);
        assert!(dispatch(&conn, "same").unwrap().claimed);
        assert!(!dispatch(&conn, "same").unwrap().claimed);
        settle(&conn, "same", "completed", None).unwrap();
        let duplicate = reserve(&conn, input("same")).unwrap();
        assert!(!duplicate.created);
        assert_eq!(duplicate.receipt.outcome.as_deref(), Some("completed"));
    }

    #[test]
    fn changed_payload_attachments_or_scope_never_reuses_an_id() {
        let conn = db();
        reserve(&conn, input("same")).unwrap();
        for field in ["payload", "attachments", "session", "turn"] {
            let mut changed = input("same");
            match field {
                "payload" => changed.payload_json = r#"{"text":"changed"}"#.into(),
                "attachments" => {
                    changed.payload_json = r#"{"attachments":[{"data":"other"}]}"#.into()
                }
                "session" => changed.session_id = "another/session".into(),
                "turn" => {
                    changed.user_turn_id = "different".into();
                    changed.user_turn = None;
                }
                _ => unreachable!(),
            }
            assert!(reserve(&conn, changed)
                .unwrap_err()
                .contains("different scope or content"));
        }
    }

    #[test]
    fn concurrent_connections_reserve_only_one_owner() {
        let path =
            std::env::temp_dir().join(format!("local-submission-{}.db", uuid::Uuid::new_v4()));
        let first = Connection::open(&path).unwrap();
        ensure_table(&first).unwrap();
        saved_owner(&first);
        drop(first);
        let barrier = Arc::new(Barrier::new(2));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    let conn = Connection::open(path).unwrap();
                    conn.busy_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    barrier.wait();
                    reserve(&conn, input("concurrent")).unwrap().created
                })
            })
            .collect();
        let count = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|created| *created)
            .count();
        assert_eq!(count, 1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn reopen_marks_both_crash_boundaries_uncertain_and_preserves_live_owners() {
        let path = std::env::temp_dir().join(format!(
            "local-submission-reopen-{}.db",
            uuid::Uuid::new_v4()
        ));
        let conn = Connection::open(&path).unwrap();
        ensure_table(&conn).unwrap();
        saved_owner(&conn);
        for id in ["before", "after", "live-owner", "this-owner"] {
            reserve(&conn, input(id)).unwrap();
        }
        dispatch(&conn, "after").unwrap();
        conn.execute("UPDATE local_submissions SET owner_instance='dead',owner_pid=2147483647 WHERE attempt_id IN ('before','after')", []).unwrap();
        conn.execute("UPDATE local_submissions SET owner_instance='another-live-instance' WHERE attempt_id='live-owner'", []).unwrap();
        drop(conn);
        let conn = Connection::open(&path).unwrap();
        ensure_table(&conn).unwrap();
        recover_unresolved(&conn).unwrap();
        for id in ["before", "after"] {
            assert_eq!(get(&conn, id).unwrap().unwrap().phase, "uncertain");
            assert!(!dispatch(&conn, id).unwrap().claimed);
        }
        for id in ["live-owner", "this-owner"] {
            assert_eq!(get(&conn, id).unwrap().unwrap().phase, "reserved");
        }
        drop(conn);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn deletion_keeps_identity_tombstones_and_defeats_delayed_new_reservations() {
        let conn = db();
        reserve(&conn, input("old")).unwrap();
        delete_session(&conn, "bot/chat.中国").unwrap();
        assert!(!dispatch(&conn, "old").unwrap().claimed);
        assert!(reserve(&conn, input("delayed")).is_err());
        let mut moved = input("old");
        moved.session_id = "other".into();
        assert!(reserve(&conn, moved).is_err());
        assert!(get(&conn, "old").unwrap().unwrap().user_turn.is_none());
    }

    #[test]
    fn reservation_store_errors_leave_no_dispatchable_attempt() {
        let conn = db();
        conn.execute_batch("CREATE TRIGGER fail_insert BEFORE INSERT ON local_submissions BEGIN SELECT RAISE(ABORT, 'disk failed'); END;").unwrap();
        assert!(reserve(&conn, input("error"))
            .unwrap_err()
            .contains("disk failed"));
        assert!(get(&conn, "error").unwrap().is_none());
        assert!(dispatch(&conn, "error").is_err());
    }

    #[test]
    fn recovery_snapshot_strips_live_bytes_and_rejects_mismatched_identity() {
        let conn = db();
        let mut value = input("safe");
        value.user_turn = Some(
            json!({"id":"safe","role":"user","text":"hello","internal":true,"credential":"private","attachments":[{"id":"image","kind":"image","name":"a.png","mimeType":"image/png","size":4,"data":"base64","previewUrl":"blob:private","path":"data:private"}]}),
        );
        let safe = reserve(&conn, value).unwrap().receipt.user_turn.unwrap();
        assert!(safe.get("credential").is_none());
        assert!(safe["attachments"][0].get("data").is_none());
        assert!(safe["attachments"][0].get("previewUrl").is_none());
        assert!(safe["attachments"][0].get("path").is_none());
        let mut bad = input("bad");
        bad.user_turn = Some(json!({"id":"different","role":"user","text":"hello"}));
        assert!(reserve(&conn, bad).is_err());
    }
}
