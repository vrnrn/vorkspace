//! Unsent input stays in the desktop's revisioned draft table. Reading never
//! migrates a row or confuses composer input with a saved transcript draft.
use super::*;
use rusqlite::OptionalExtension;
use serde_json::Value;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ComposerDraft {
    pub revision: u64,
    pub payload: Value,
    pub deleted: bool,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ComposerRecovery {
    pub owner: String,
    pub revision: u64,
    pub payload: Value,
}
pub(super) fn ensure_draft_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS composer_drafts (
        session_id TEXT PRIMARY KEY,cwd TEXT NOT NULL,
        revision INTEGER NOT NULL CHECK(revision>=1 AND revision<=9007199254740991),
        payload_json TEXT NOT NULL,deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN(0,1)),updated_at INTEGER NOT NULL);")
        .map_err(|e| e.to_string())
}
pub(super) fn ensure_recovery_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS composer_draft_recoveries(session_id TEXT NOT NULL,owner TEXT NOT NULL,revision INTEGER NOT NULL,payload_json TEXT NOT NULL,updated_at INTEGER NOT NULL,PRIMARY KEY(session_id,owner));")
        .map_err(|e| e.to_string())
}
fn validate(payload: &Value) -> Result<(), String> {
    let text = payload["text"]
        .as_str()
        .ok_or("Composer draft text is missing")?;
    let utf16 = text.encode_utf16().count() as u64;
    let start = payload["selectionStart"]
        .as_u64()
        .ok_or("Composer draft selection is missing")?;
    let end = payload["selectionEnd"]
        .as_u64()
        .ok_or("Composer draft selection is missing")?;
    if payload["cwd"].as_str().is_none()
        || start > end
        || end > utf16
        || text.len() > 8 * 1024 * 1024
        || payload["attachments"]
            .as_array()
            .is_none_or(|a| a.len() > 128)
        || payload["mcpTags"].as_array().is_none_or(|a| a.len() > 128)
    {
        return Err("Composer draft has invalid fields or exceeds supported limits".into());
    }
    if payload["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["path"].as_str().is_none_or(str::is_empty))
    {
        return Err("A saved attachment has no file reference; its draft was preserved".into());
    }
    if payload["mcpTags"].as_array().unwrap().iter().any(|tag| {
        tag["token"].as_str().is_none_or(str::is_empty)
            || ["provider", "name", "scope", "configPath", "transport"]
                .into_iter()
                .any(|field| tag["server"][field].as_str().is_none())
            || (!tag["server"]["enabled"].is_null() && !tag["server"]["enabled"].is_boolean())
    }) {
        return Err("A saved MCP tag is unreadable; its draft was preserved".into());
    }
    Ok(())
}

impl Store {
    fn composer_source_id(&self, id: i64) -> Result<String, String> {
        if self.shared.is_some() {
            self.persisted_session_id(id)
        } else {
            Ok(id.to_string())
        }
    }
    pub fn composer_recoveries(&self, id: i64) -> Result<Vec<ComposerRecovery>, String> {
        if !shared::has_table(&self.conn, "composer_draft_recoveries")? {
            return Ok(Vec::new());
        }
        let source = self.composer_source_id(id)?;
        let mut query=self.conn.prepare("SELECT owner,revision,payload_json FROM composer_draft_recoveries WHERE session_id=?1 ORDER BY updated_at,owner").map_err(|e|e.to_string())?;
        let rows = query
            .query_map([source], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.map(|row| {
            let (owner, revision, raw) = row.map_err(|e| e.to_string())?;
            let payload: Value = serde_json::from_str(&raw)
                .map_err(|e| format!("Cannot read recovered input: {e}"))?;
            validate(&payload)?;
            Ok(ComposerRecovery {
                owner,
                revision: revision.max(0) as u64,
                payload,
            })
        })
        .collect()
    }
    pub fn save_composer_recovery(&self, id: i64, record: ComposerRecovery) -> Result<(), String> {
        validate(&record.payload)?;
        crate::accounts::validate_id(&record.owner)?;
        let raw = serde_json::to_string(&record.payload).map_err(|e| e.to_string())?;
        if raw.len() > 10 * 1024 * 1024 {
            return Err("Recovered input exceeds supported limits".into());
        }
        let source = self.composer_source_id(id)?;
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        ensure_recovery_schema(&tx)?;
        tx.execute("INSERT INTO composer_draft_recoveries VALUES(?1,?2,?3,?4,?5) ON CONFLICT(session_id,owner) DO UPDATE SET revision=excluded.revision,payload_json=excluded.payload_json,updated_at=excluded.updated_at",params![source,record.owner,record.revision.min(i64::MAX as u64) as i64,raw,crate::time::now_ms().min(i64::MAX as u64) as i64]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn clear_composer_recovery(&self, id: i64, owner: &str) -> Result<(), String> {
        if !shared::has_table(&self.conn, "composer_draft_recoveries")? {
            return Ok(());
        }
        self.conn
            .execute(
                "DELETE FROM composer_draft_recoveries WHERE session_id=?1 AND owner=?2",
                params![self.composer_source_id(id)?, owner],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn composer_draft(&self, id: i64) -> Result<Option<ComposerDraft>, String> {
        if self.shared.is_none() {
            return self
                .setting(&format!("composer:{id}"))?
                .map(|raw| serde_json::from_str(&raw).map_err(|e| e.to_string()))
                .transpose();
        }
        if !shared::has_table(&self.conn, "composer_drafts")? {
            return Ok(None);
        }
        let source = self.persisted_session_id(id)?;
        self.conn
            .query_row(
                "SELECT revision,payload_json,deleted FROM composer_drafts WHERE session_id=?1",
                [source],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?.max(0) as u64,
                        row.get::<_, String>(1)?,
                        row.get::<_, bool>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?
            .map(|(revision, raw, deleted)| {
                if revision == 0 || raw.len() > 10 * 1024 * 1024 {
                    return Err("Composer draft has an invalid revision or size".into());
                }
                let draft = ComposerDraft {
                    revision,
                    payload: serde_json::from_str(&raw)
                        .map_err(|e| format!("Cannot read composer draft: {e}"))?,
                    deleted,
                };
                validate(&draft.payload)?;
                Ok(draft)
            })
            .transpose()
    }

    /// Compare-and-save at the owning row. A second window cannot silently
    /// overwrite a newer draft. Deleted rows retain a revision tombstone.
    pub fn save_composer_draft(
        &self,
        id: i64,
        expected: u64,
        payload: &Value,
    ) -> Result<u64, String> {
        validate(payload)?;
        let raw = serde_json::to_string(payload).map_err(|e| e.to_string())?;
        let text = payload["text"]
            .as_str()
            .ok_or("Composer draft text is missing")?;
        if !payload.is_object()
            || text.len() > 8 * 1024 * 1024
            || raw.len() > 10 * 1024 * 1024
            || payload["attachments"]
                .as_array()
                .is_none_or(|a| a.len() > 128)
            || payload["mcpTags"].as_array().is_none_or(|a| a.len() > 128)
        {
            return Err("Composer draft exceeds supported limits".into());
        }
        let deleted =
            text.is_empty() && payload["attachments"].as_array().is_some_and(Vec::is_empty);
        let revision = expected
            .checked_add(1)
            .filter(|v| *v <= 9_007_199_254_740_991)
            .ok_or("Composer draft revision is exhausted")?;
        if self.shared.is_none() {
            if self.composer_draft(id)?.map_or(0, |d| d.revision) != expected {
                return Err(
                    "Composer draft changed in another window; your input was preserved".into(),
                );
            }
            self.save_setting(
                &format!("composer:{id}"),
                &serde_json::to_string(&ComposerDraft {
                    revision,
                    payload: payload.clone(),
                    deleted,
                })
                .map_err(|e| e.to_string())?,
            )?;
            return Ok(revision);
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        // New stores create the same ordinary draft table on first save. No
        // existing row is copied, converted or rewritten during an upgrade.
        ensure_draft_schema(&tx)?;
        let source = self.persisted_session_id(id)?;
        let current = tx
            .query_row(
                "SELECT revision FROM composer_drafts WHERE session_id=?1",
                [&source],
                |r| r.get::<_, i64>(0).map(|v| v.max(0) as u64),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .unwrap_or(0);
        if current != expected {
            return Err(
                "Composer draft changed in another window; your input was preserved".into(),
            );
        }
        let cwd = payload["cwd"]
            .as_str()
            .ok_or("Composer draft directory is missing")?;
        tx.execute("INSERT INTO composer_drafts(session_id,cwd,revision,payload_json,deleted,updated_at) VALUES(?1,?2,?3,?4,?5,?6)
            ON CONFLICT(session_id) DO UPDATE SET cwd=excluded.cwd,revision=excluded.revision,payload_json=excluded.payload_json,deleted=excluded.deleted,updated_at=excluded.updated_at",
            params![source,cwd,revision as i64,raw,deleted,crate::time::now_ms().min(i64::MAX as u64) as i64]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(revision)
    }
}

#[cfg(test)]
mod tests;
