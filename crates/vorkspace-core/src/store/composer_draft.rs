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

/// The durable version of the editor input offered to a submission owner.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ComposerOrigin {
    pub(super) session: i64,
    pub(super) source: String,
    pub(super) revision: u64,
}

impl ComposerOrigin {
    pub fn message_id(&self) -> String {
        format!("composer-draft-{}", self.revision)
    }
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

struct Sources {
    current: String,
    bound: Option<String>,
    unbound: Option<String>,
}

impl Sources {
    fn params(&self) -> [Option<&str>; 3] {
        [
            Some(&self.current),
            self.bound.as_deref(),
            self.unbound.as_deref(),
        ]
    }
}

impl Store {
    fn composer_sources(&self, id: i64) -> Result<Sources, String> {
        if self.shared.is_none() {
            return Ok(Sources {
                current: id.to_string(),
                bound: None,
                unbound: None,
            });
        }
        if let Some(remote) = if id < 0 {
            self.remote_session(id)?
        } else {
            None
        } {
            // The desktop draft exists before Host creates its conversation.
            // Keep its key stable across that binding and read old Host keys
            // until the next normal editor write copies them forward.
            let environment = remote.project.environment_id;
            return Ok(Sources {
                current: format!("remote-composer:{environment}:{id}"),
                bound: remote
                    .session_id
                    .map(|host| format!("remote:{environment}:{host}")),
                unbound: Some(format!("remote:{environment}:{id}")),
            });
        }
        Ok(Sources {
            current: self.persisted_session_id(id)?,
            bound: None,
            unbound: None,
        })
    }

    fn read_composer_row<T>(
        &self,
        sources: &Sources,
        columns: &str,
        read: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Option<T>, String> {
        self.conn
            .query_row(
                &format!("SELECT {columns} FROM composer_drafts WHERE session_id IN (?1,?2,?3)
                          ORDER BY CASE session_id WHEN ?1 THEN 0 WHEN ?2 THEN 1 ELSE 2 END LIMIT 1"),
                sources.params(),
                read,
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    fn remote_composer_transferred(
        &self,
        id: i64,
        source: &str,
        revision: u64,
    ) -> Result<bool, String> {
        Ok(self
            .remote_session(id)?
            .and_then(|remote| remote.composer_origin)
            .is_some_and(|origin| origin.source == source && origin.revision == revision))
    }

    pub fn composer_origin(&self, id: i64, revision: u64) -> Result<ComposerOrigin, String> {
        let sources = self.composer_sources(id)?;
        let version = if self.shared.is_some() {
            self.read_composer_row(
                &sources,
                "session_id,revision,deleted,cwd LIKE 'remote://%'",
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get::<_, i64>(1)?.max(0) as u64,
                        row.get(2)?,
                        row.get::<_, bool>(3)?,
                    ))
                },
            )?
        } else {
            self.composer_draft(id)?
                .map(|draft| (sources.current, draft.revision, draft.deleted, false))
        };
        let Some((source, current, false, remote)) = version else {
            return Err(
                "The saved draft changed before this send. Your input was preserved.".into(),
            );
        };
        if current != revision
            || (remote && self.remote_composer_transferred(id, &source, revision)?)
        {
            return Err(
                "The saved draft changed before this send. Your input was preserved.".into(),
            );
        }
        Ok(ComposerOrigin {
            session: id,
            source,
            revision,
        })
    }

    /// Resolve an interrupted notification from the draft and journal owners.
    pub fn composer_transfer_committed(&self, origin: &ComposerOrigin) -> Result<bool, String> {
        let sources = self.composer_sources(origin.session)?;
        if !sources.params().contains(&Some(origin.source.as_str())) {
            return Ok(false);
        }
        if self.shared.is_none() {
            return Ok(self
                .composer_draft(origin.session)?
                .is_some_and(|draft| draft.revision == origin.revision && draft.deleted));
        }
        let version = self.read_composer_row(
            &sources,
            "session_id,revision,deleted,cwd LIKE 'remote://%'",
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?.max(0) as u64,
                    row.get::<_, bool>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            },
        )?;
        let Some((source, revision, deleted, remote)) = version else {
            return Ok(false);
        };
        if source != origin.source || revision != origin.revision {
            return Ok(false);
        }
        Ok(deleted
            || (remote && self.remote_composer_transferred(origin.session, &source, revision)?))
    }

    /// The caller owns the transaction that accepts the input. Moving the
    /// saved version leaves its edit revision intact so an editor with newer
    /// pending text can still save against that revision.
    pub(super) fn transfer_composer_origin(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        id: i64,
        origin: &ComposerOrigin,
    ) -> Result<(), String> {
        if origin.session != id || origin.source != self.composer_sources(id)?.current {
            return Err("Composer submission belongs to another conversation".into());
        }
        if self.shared.is_some() {
            transaction
                .execute(
                    "UPDATE composer_drafts SET deleted=1,updated_at=?1
                     WHERE session_id=?2 AND revision=?3 AND deleted=0",
                    params![
                        crate::time::now_ms().min(i64::MAX as u64) as i64,
                        origin.source,
                        i64::try_from(origin.revision).map_err(|error| error.to_string())?
                    ],
                )
                .map_err(|error| error.to_string())?;
        } else if let Some(mut draft) = self
            .composer_draft(id)?
            .filter(|draft| draft.revision == origin.revision && !draft.deleted)
        {
            draft.deleted = true;
            transaction
                .execute(
                    "UPDATE native_settings SET value=?1 WHERE key=?2",
                    params![
                        serde_json::to_string(&draft).map_err(|error| error.to_string())?,
                        format!("composer:{id}")
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
    pub fn composer_recoveries(&self, id: i64) -> Result<Vec<ComposerRecovery>, String> {
        if !shared::has_table(&self.conn, "composer_draft_recoveries")? {
            return Ok(Vec::new());
        }
        let sources = self.composer_sources(id)?;
        let mut query=self.conn.prepare(
            "SELECT owner,revision,payload_json FROM (
                 SELECT owner,revision,payload_json,updated_at,ROW_NUMBER() OVER (
                     PARTITION BY owner ORDER BY CASE session_id WHEN ?1 THEN 0 WHEN ?2 THEN 1 ELSE 2 END
                 ) AS rank FROM composer_draft_recoveries WHERE session_id IN (?1,?2,?3)
             ) WHERE rank=1 ORDER BY updated_at,owner"
        ).map_err(|e|e.to_string())?;
        let rows = query
            .query_map(sources.params(), |row| {
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
        let source = self.composer_sources(id)?.current;
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
        let sources = self.composer_sources(id)?;
        let [current, bound, unbound] = sources.params();
        self.conn
            .execute(
                "DELETE FROM composer_draft_recoveries WHERE session_id IN (?1,?2,?3) AND owner=?4",
                params![current, bound, unbound, owner],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn composer_draft(&self, id: i64) -> Result<Option<ComposerDraft>, String> {
        if self.shared.is_none() {
            let mut draft: Option<ComposerDraft> = self
                .setting(&format!("composer:{id}"))?
                .map(|raw| serde_json::from_str(&raw).map_err(|e| e.to_string()))
                .transpose()?;
            if let Some(draft) = draft.as_mut().filter(|draft| {
                !draft.deleted
                    && draft.payload["cwd"]
                        .as_str()
                        .is_some_and(|cwd| cwd.starts_with("remote://"))
            }) {
                draft.deleted =
                    self.remote_composer_transferred(id, &id.to_string(), draft.revision)?;
            }
            return Ok(draft);
        }
        if !shared::has_table(&self.conn, "composer_drafts")? {
            return Ok(None);
        }
        self.read_composer_row(
            &self.composer_sources(id)?,
            "session_id,revision,payload_json,deleted",
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?.max(0) as u64,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            },
        )?
        .map(|(source, revision, raw, deleted)| {
            if revision == 0 || raw.len() > 10 * 1024 * 1024 {
                return Err("Composer draft has an invalid revision or size".into());
            }
            let mut draft = ComposerDraft {
                revision,
                payload: serde_json::from_str(&raw)
                    .map_err(|e| format!("Cannot read composer draft: {e}"))?,
                deleted,
            };
            validate(&draft.payload)?;
            if !draft.deleted
                && draft.payload["cwd"]
                    .as_str()
                    .is_some_and(|cwd| cwd.starts_with("remote://"))
            {
                draft.deleted = self.remote_composer_transferred(id, &source, revision)?;
            }
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
        let sources = self.composer_sources(id)?;
        let current = self
            .read_composer_row(&sources, "revision", |row| row.get::<_, i64>(0))?
            .unwrap_or(0)
            .max(0) as u64;
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
            params![sources.current,cwd,revision as i64,raw,deleted,crate::time::now_ms().min(i64::MAX as u64) as i64]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(revision)
    }
}

#[cfg(test)]
mod tests;
