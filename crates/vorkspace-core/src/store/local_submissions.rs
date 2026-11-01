//! Durable local acceptance receipts. A reservation never authorizes replay:
//! provider I/O and SQLite cannot form an exactly-once transaction.
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
};

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Store;

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
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    unsafe {
        libc::kill(pid as i32, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

/// Called on disk open, never on ordinary reads or a second window. Live
/// owners (including this process) retain their reservations untouched.
fn closed_attempts() -> &'static Mutex<HashSet<(String, String)>> {
    static CLOSED: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
    CLOSED.get_or_init(Mutex::default)
}
fn connection_identity(conn: &Connection) -> String {
    conn.path()
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("memory:{conn:p}"))
}
/// A worker can relinquish acceptance ownership even while unwinding through
/// a poisoned UI store lock. It contains identity only, never a connection.
#[derive(Clone, Debug)]
pub struct SubmissionOwner(String);
impl SubmissionOwner {
    pub fn release(&self, attempt: &str) {
        closed_attempts()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((self.0.clone(), attempt.to_owned()));
    }
}
pub(crate) fn recover_unresolved(conn: &Connection) -> rusqlite::Result<()> {
    let mut statement = conn.prepare("SELECT attempt_id,owner_pid,owner_instance FROM local_submissions WHERE phase IN ('reserved','dispatching')")?;
    let owners = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let identity = connection_identity(conn);
    for (attempt, pid, owner) in owners {
        let closed = closed_attempts()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(identity.clone(), attempt.clone()));
        if !closed && (owner == instance() || pid_alive(pid)) {
            continue;
        }
        conn.execute("UPDATE local_submissions SET phase='uncertain',outcome=NULL,error='The local send ended before completion could be confirmed. The provider may have received it; it was not replayed.',updated_at=?1 WHERE attempt_id=?2 AND owner_instance=?3 AND phase IN ('reserved','dispatching')",params![now(),attempt,owner])?;
        closed_attempts()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(identity.clone(), attempt));
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

pub(super) fn delete_if_present(conn: &Connection, source: &str) -> Result<(), String> {
    if super::shared::has_table(conn, "local_submissions")? {
        ensure_table(conn).map_err(|e| e.to_string())?;
        delete_session(conn, source).map_err(|e| e.to_string())?;
    }
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
    for key in ["internal", "vorkspace", "localSubmissionOwned"] {
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

fn reserve(
    conn: &Connection,
    input: SubmissionInput,
    session_table: &str,
) -> Result<SubmissionReservation, String> {
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
    let owner_table = match session_table {
        "sessions" => "sessions",
        "native_sessions" => "native_sessions",
        _ => return Err("Invalid local submission owner table".into()),
    };
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let insert = format!("INSERT INTO local_submissions (attempt_id,user_turn_id,session_id,signature,phase,owner_pid,owner_instance,created_at,updated_at,user_turn_json) SELECT ?1,?2,?3,?4,'reserved',?5,?6,?7,?7,?8 WHERE NOT EXISTS (SELECT 1 FROM local_submission_deleted_sessions WHERE session_id=?3) AND EXISTS(SELECT 1 FROM {owner_table} WHERE id=?3) AND NOT EXISTS(SELECT 1 FROM local_submissions WHERE session_id=?3 AND user_turn_id=?2 AND (phase IN ('reserved','dispatching','uncertain') OR outcome='completed')) ON CONFLICT(attempt_id) DO NOTHING");
    let created = tx
        .execute(
            &insert,
            params![
                input.attempt_id,
                input.user_turn_id,
                input.session_id,
                signature,
                std::process::id(),
                instance(),
                now(),
                user_turn.as_ref().map(serde_json::Value::to_string)
            ],
        )
        .map_err(|e| e.to_string())?
        == 1;
    let receipt = get(&tx, &input.attempt_id)?.ok_or(
        "The local send session has not been saved, was deleted, or this message already has an accepted or unconfirmed attempt; it was not replayed",
    )?;
    if receipt.session_id != input.session_id
        || receipt.user_turn_id != input.user_turn_id
        || receipt.signature != signature
        || receipt.user_turn != user_turn
    {
        return Err("Local submission ID was already used with different scope or content".into());
    }
    tx.commit().map_err(|e| e.to_string())?;
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
    conn.execute("UPDATE local_submissions SET phase=?1,outcome=?2,error=?3,updated_at=?4 WHERE attempt_id=?5 AND phase IN ('reserved','dispatching') AND owner_instance=?6", params![phase,if outcome == "uncertain" {None} else {Some(outcome)},error,now(),attempt_id,instance()]).map_err(|e| e.to_string())?;
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

impl Store {
    /// Recover only a previous process's unfinished attempts. Reading a store
    /// and validation probes do not create tables or mutate recovery state.
    pub fn local_submission_owner(&self) -> SubmissionOwner {
        SubmissionOwner(connection_identity(&self.conn))
    }
    pub fn release_local_submission(&self, attempt: &str) {
        closed_attempts()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((connection_identity(&self.conn), attempt.to_owned()));
    }
    pub fn recover_local_submissions(&self) -> Result<(), String> {
        if super::shared::has_table(&self.conn, "local_submissions")? {
            recover_unresolved(&self.conn).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub fn reserve_local_submission(
        &self,
        session: i64,
        mut input: SubmissionInput,
    ) -> Result<SubmissionReservation, String> {
        if self.shared.is_some() {
            self.shared_writable()?;
        }
        input.session_id = self.submission_source_id(session)?;
        let table = if self.shared.is_some() {
            "sessions"
        } else {
            "native_sessions"
        };
        ensure_table(&self.conn).map_err(|e| e.to_string())?;
        reserve(&self.conn, input, table)
    }
    fn submission_source_id(&self, session: i64) -> Result<String, String> {
        if self.shared.is_some() {
            self.persisted_session_id(session)
        } else {
            Ok(session.to_string())
        }
    }
    pub fn dispatch_local_submission(&self, attempt: &str) -> Result<SubmissionClaim, String> {
        dispatch(&self.conn, attempt)
    }
    pub fn settle_local_submission(
        &self,
        attempt: &str,
        outcome: &str,
        error: Option<String>,
    ) -> Result<SubmissionReceipt, String> {
        settle(&self.conn, attempt, outcome, error)
    }
    pub fn local_submissions(&self, session: i64) -> Result<Vec<SubmissionReceipt>, String> {
        if !super::shared::has_table(&self.conn, "local_submissions")? {
            return Ok(Vec::new());
        }
        let source = self.submission_source_id(session)?;
        let mut query = self.conn.prepare("SELECT attempt_id,user_turn_id,session_id,signature,phase,outcome,error,user_turn_json FROM local_submissions WHERE session_id=?1 ORDER BY created_at,attempt_id").map_err(|e|e.to_string())?;
        let rows = query
            .query_map([source], receipt_row)
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string());
        rows
    }
    /// Bounded recovery view: completed historical receipts never enter the UI.
    pub fn latest_unconfirmed_submission(
        &self,
        session: i64,
    ) -> Result<Option<SubmissionReceipt>, String> {
        if !super::shared::has_table(&self.conn, "local_submissions")? {
            return Ok(None);
        }
        let source = self.submission_source_id(session)?;
        self.conn.query_row("SELECT attempt_id,user_turn_id,session_id,signature,phase,outcome,error,user_turn_json FROM local_submissions WHERE session_id=?1 AND phase='uncertain' AND COALESCE(outcome,'') != 'reviewed' ORDER BY updated_at DESC,attempt_id LIMIT 1",[source],receipt_row).optional().map_err(|e|e.to_string())
    }
    /// Dismissing a notice records review, never proof of external completion.
    /// Its uncertainty and stable-message tombstone continue to forbid replay.
    pub fn acknowledge_local_submission(&self, session: i64, attempt: &str) -> Result<(), String> {
        if self.shared.is_some() {
            self.shared_writable()?;
        }
        let source = self.submission_source_id(session)?;
        self.conn.execute("UPDATE local_submissions SET outcome='reviewed',updated_at=?1 WHERE session_id=?2 AND attempt_id=?3 AND phase='uncertain'",params![now(),source,attempt]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub(super) fn delete_local_submissions(&self, source: &str) -> Result<(), String> {
        delete_if_present(&self.conn, source)
    }
}

#[cfg(test)]
mod tests;
