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

mod edit;
pub use edit::{EditAdmissionError, EditBarrier, EditClaim};

const MAX_PAYLOAD_BYTES: usize = 64 << 20;
const MAX_USER_TURN_BYTES: usize = 8 << 20;
const RECOVERY_INDEX: &str = "CREATE INDEX IF NOT EXISTS local_submissions_unfinished_idx
       ON local_submissions(session_id) WHERE phase IN ('reserved','dispatching');";

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
    )?;
    conn.execute_batch(RECOVERY_INDEX)
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

/// Attempts whose workers relinquished ownership before SQLite could settle them.
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

    fn has_released(&self, attempt: &str) -> bool {
        closed_attempts()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(self.0.clone(), attempt.to_owned()))
    }
}
fn recover_unresolved(conn: &Connection, source: Option<&str>) -> rusqlite::Result<()> {
    let sql = if source.is_some() {
        "SELECT attempt_id,owner_pid,owner_instance FROM local_submissions
         WHERE phase IN ('reserved','dispatching') AND session_id=?1"
    } else {
        "SELECT attempt_id,owner_pid,owner_instance FROM local_submissions
         WHERE phase IN ('reserved','dispatching')"
    };
    let mut statement = conn.prepare(sql)?;
    let owners = statement
        .query_map(rusqlite::params_from_iter(source), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let identity = SubmissionOwner(connection_identity(conn));
    for (attempt, pid, owner) in owners {
        if !identity.has_released(&attempt) && (owner == instance() || pid_alive(pid)) {
            continue;
        }
        if attempt.starts_with(edit::ATTEMPT_PREFIX) {
            conn.execute(
                "UPDATE local_submissions SET
                   phase=CASE phase WHEN 'reserved' THEN 'settled' ELSE 'uncertain' END,
                   outcome=CASE phase WHEN 'reserved' THEN 'failed' END,
                   error=?1,updated_at=?2
                 WHERE attempt_id=?3 AND owner_instance=?4 AND phase IN ('reserved','dispatching')",
                params![
                    "The message edit ended before local history was updated.",
                    now(),
                    attempt,
                    owner
                ],
            )?;
        } else {
            conn.execute("UPDATE local_submissions SET phase='uncertain',outcome=NULL,error='The local send ended before completion could be confirmed. The provider may have received it; it was not replayed.',updated_at=?1 WHERE attempt_id=?2 AND owner_instance=?3 AND phase IN ('reserved','dispatching')",params![now(),attempt,owner])?;
        }
        closed_attempts()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(identity.0.clone(), attempt));
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

#[cfg(test)]
fn reserve(conn: &Connection, input: SubmissionInput) -> Result<SubmissionReservation, String> {
    reserve_checked(conn, input, || Ok(()))
}

fn reserve_checked(
    conn: &Connection,
    input: SubmissionInput,
    check: impl FnOnce() -> Result<(), String>,
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
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    check()?;
    // Edits and compaction have no user turn and exclusively own provider context.
    // Prompt receipts can still overlap within a run to support steering.
    let competing_operation = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM local_submissions WHERE session_id=?1
          AND attempt_id<>?2 AND phase IN ('reserved','dispatching')
          AND (?3 OR user_turn_json IS NULL))",
            params![input.session_id, input.attempt_id, user_turn.is_none()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|e| e.to_string())?;
    if competing_operation {
        return Err("A local provider operation is already active in this conversation.".into());
    }
    let insert = "INSERT INTO local_submissions (attempt_id,user_turn_id,session_id,signature,phase,owner_pid,owner_instance,created_at,updated_at,user_turn_json) SELECT ?1,?2,?3,?4,'reserved',?5,?6,?7,?7,?8 WHERE NOT EXISTS (SELECT 1 FROM local_submission_deleted_sessions WHERE session_id=?3) AND EXISTS(SELECT 1 FROM sessions WHERE id=?3) AND NOT EXISTS(SELECT 1 FROM local_submissions WHERE session_id=?3 AND user_turn_id=?2 AND (phase IN ('reserved','dispatching','uncertain') OR outcome='completed')) ON CONFLICT(attempt_id) DO NOTHING";
    let created = tx
        .execute(
            insert,
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
    pub fn local_submission_owner(&self) -> SubmissionOwner {
        SubmissionOwner(connection_identity(&self.conn))
    }
    /// Recover the workspace when opening it for normal desktop use.
    /// A validation-only open does not call this.
    pub fn recover_local_submissions(&self) -> Result<(), String> {
        if super::shared::has_table(&self.conn, "local_submissions")? {
            self.conn
                .execute_batch(RECOVERY_INDEX)
                .map_err(|error| error.to_string())?;
            recover_unresolved(&self.conn, None).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// Refresh just this conversation after loading its history or closing a worker.
    pub fn session_submission_recovery(
        &self,
        session: i64,
    ) -> Result<Option<SubmissionReceipt>, String> {
        if !super::shared::has_table(&self.conn, "local_submissions")? {
            return Ok(None);
        }
        let source = self.submission_source_id(session)?;
        recover_unresolved(&self.conn, Some(&source)).map_err(|error| error.to_string())?;
        self.latest_unconfirmed_for_source(&source)
    }

    pub fn reserve_local_submission(
        &self,
        session: i64,
        input: SubmissionInput,
    ) -> Result<SubmissionReservation, String> {
        self.reserve_local_submission_after_native_history(session, input, None)
    }

    /// Validate the reconciled cursor in the same transaction that excludes
    /// native import. An importer that wins first makes the caller retry.
    pub fn reserve_local_submission_after_native_history(
        &self,
        session: i64,
        input: SubmissionInput,
        history: Option<&super::NativeHistoryRequest>,
    ) -> Result<SubmissionReservation, String> {
        if input.attempt_id.starts_with(edit::ATTEMPT_PREFIX) {
            return Err("Invalid local submission identity".into());
        }
        self.reserve_submission_checked(session, input, || {
            if let Some(reason) = self.edit_recovery_reason(session)? {
                return Err(reason);
            }
            if let Some(history) = history {
                if history.session.id != session || !self.native_history_current(history)? {
                    return Err("The Bot conversation changed during native history sync. Retry to continue the same session.".into());
                }
            }
            Ok(())
        })
    }

    fn reserve_submission_checked(
        &self,
        session: i64,
        mut input: SubmissionInput,
        check: impl FnOnce() -> Result<(), String>,
    ) -> Result<SubmissionReservation, String> {
        self.shared_writable()?;
        input.session_id = self.submission_source_id(session)?;
        ensure_table(&self.conn).map_err(|e| e.to_string())?;
        reserve_checked(&self.conn, input, check)
    }
    fn submission_source_id(&self, session: i64) -> Result<String, String> {
        self.persisted_session_id(session)
    }
    pub fn dispatch_local_submission(&self, attempt: &str) -> Result<SubmissionClaim, String> {
        dispatch(&self.conn, attempt)
    }
    pub fn dispatch_composer_submission(
        &self,
        attempt: &str,
        origin: &super::ComposerOrigin,
    ) -> Result<SubmissionClaim, String> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let claim = dispatch(&tx, attempt)?;
        if claim.claimed {
            if claim.receipt.session_id != origin.source
                || claim.receipt.user_turn_id != origin.message_id()
            {
                return Err("Local receipt does not own this composer version".into());
            }
            self.transfer_composer_origin(&tx, origin.session, origin)?;
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok(claim)
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
        self.latest_unconfirmed_for_source(&source)
    }

    fn latest_unconfirmed_for_source(
        &self,
        source: &str,
    ) -> Result<Option<SubmissionReceipt>, String> {
        self.conn.query_row("SELECT attempt_id,user_turn_id,session_id,signature,phase,outcome,error,user_turn_json FROM local_submissions WHERE session_id=?1 AND phase='uncertain' AND attempt_id NOT GLOB ?2 AND COALESCE(outcome,'') != 'reviewed' ORDER BY updated_at DESC,attempt_id LIMIT 1",params![source,edit::ATTEMPT_PATTERN],receipt_row).optional().map_err(|e|e.to_string())
    }
    /// Dismissing a notice records review, never proof of external completion.
    /// Its uncertainty and stable-message tombstone continue to forbid replay.
    pub fn acknowledge_local_submission(&self, session: i64, attempt: &str) -> Result<(), String> {
        self.shared_writable()?;
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
