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

    pub fn recovery_owner(&self) -> String {
        format!("composer-offer-{}", self.revision)
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
        .map_err(|e| e.to_string())?;
    ensure_transfer_schema(conn)
}
pub(super) fn ensure_transfer_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS composer_transfers (
            session_id TEXT NOT NULL, revision INTEGER NOT NULL,
            PRIMARY KEY(session_id, revision)
         ) WITHOUT ROWID;",
    )
    .map_err(|error| error.to_string())
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
    previous: [Option<String>; 3],
}

struct ComposerVersion {
    source: String,
    revision: u64,
    deleted: bool,
    remote: bool,
}

impl Sources {
    fn local(current: String) -> Self {
        Self {
            current,
            previous: [None, None, None],
        }
    }

    fn params(&self) -> [Option<&str>; 4] {
        [
            Some(&self.current),
            self.previous[0].as_deref(),
            self.previous[1].as_deref(),
            self.previous[2].as_deref(),
        ]
    }
}

impl Store {
    fn composer_sources(&self, id: i64) -> Result<Sources, String> {
        if self.shared.is_none() {
            return Ok(Sources::local(id.to_string()));
        }
        if id >= 0 {
            let source = self.persisted_session_id(id)?;
            let remote = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1 AND cwd >= 'remote://' AND cwd < 'remote:/0')",
                [&source],
                |row| row.get::<_, bool>(0),
            ).map_err(|error| error.to_string())?;
            if !remote {
                return Ok(Sources::local(source));
            }
        }
        if let Some(remote) = self.remote_session(id)? {
            return self.remote_composer_sources(&remote);
        }
        Ok(Sources::local(self.persisted_session_id(id)?))
    }

    fn remote_composer_sources(&self, remote: &super::RemoteSession) -> Result<Sources, String> {
        let id = remote.summary.id;
        if self.shared.is_none() {
            return Ok(Sources::local(id.to_string()));
        }
        let environment = &remote.project.environment_id;
        let bound = remote
            .session_id
            .as_ref()
            .map(|host| format!("remote:{environment}:{host}"));
        // Host can bind a new conversation after the desktop has saved input.
        // Keep the native handle as a fallback if a legacy tab later owns it.
        if id < 0 {
            return Ok(Sources {
                current: format!("remote-composer:{environment}:{id}"),
                previous: [bound, Some(format!("remote:{environment}:{id}")), None],
            });
        }
        let current = self.persisted_session_id(id)?;
        if let Some(alias) = remote.composer_alias.filter(|alias| *alias < 0) {
            return Ok(Sources {
                current,
                previous: [
                    Some(format!("remote-composer:{environment}:{alias}")),
                    bound,
                    Some(format!("remote:{environment}:{alias}")),
                ],
            });
        }
        Ok(Sources::local(current))
    }

    pub(super) fn remote_composer_source_matches(
        &self,
        remote: &super::RemoteSession,
        origin: &ComposerOrigin,
    ) -> Result<bool, String> {
        Ok(origin.session == remote.summary.id
            && self
                .remote_composer_sources(remote)?
                .params()
                .contains(&Some(origin.source.as_str())))
    }

    fn read_composer_row<T>(
        &self,
        sources: &Sources,
        columns: &str,
        read: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Option<T>, String> {
        self.conn
            .query_row(
                &format!("SELECT {columns} FROM composer_drafts WHERE session_id IN (?1,?2,?3,?4)
                          ORDER BY CASE session_id WHEN ?1 THEN 0 WHEN ?2 THEN 1 WHEN ?3 THEN 2 ELSE 3 END LIMIT 1"),
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

    fn composer_version(
        &self,
        id: i64,
        sources: Sources,
    ) -> Result<Option<ComposerVersion>, String> {
        if self.shared.is_some() {
            self.read_composer_row(
                &sources,
                "session_id,revision,deleted,cwd LIKE 'remote://%'",
                |row| {
                    Ok(ComposerVersion {
                        source: row.get(0)?,
                        revision: row.get::<_, i64>(1)?.max(0) as u64,
                        deleted: row.get(2)?,
                        remote: row.get(3)?,
                    })
                },
            )
        } else {
            Ok(self.composer_draft(id)?.map(|draft| ComposerVersion {
                source: sources.current,
                revision: draft.revision,
                deleted: draft.deleted,
                remote: draft.payload["cwd"]
                    .as_str()
                    .is_some_and(|cwd| cwd.starts_with("remote://")),
            }))
        }
    }

    pub fn composer_origin(&self, id: i64, revision: u64) -> Result<ComposerOrigin, String> {
        let version = self
            .composer_version(id, self.composer_sources(id)?)?
            .filter(|version| !version.deleted && version.revision == revision)
            .ok_or(DRAFT_CHANGED)?;
        if version.remote && self.remote_composer_transferred(id, &version.source, revision)? {
            return Err(DRAFT_CHANGED.into());
        }
        Ok(ComposerOrigin {
            session: id,
            source: version.source,
            revision,
        })
    }

    /// Resolve an interrupted notification from the draft and journal owners.
    pub fn composer_transfer_committed(&self, origin: &ComposerOrigin) -> Result<bool, String> {
        let sources = self.composer_sources(origin.session)?;
        if !sources.params().contains(&Some(origin.source.as_str())) {
            return Ok(false);
        }
        let revision = i64::try_from(origin.revision).map_err(|error| error.to_string())?;
        if shared::has_table(&self.conn, "composer_transfers")?
            && self
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM composer_transfers WHERE session_id=?1 AND revision=?2)",
                    params![origin.source, revision],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|error| error.to_string())?
        {
            return Ok(true);
        }
        let Some(version) = self.composer_version(origin.session, sources)? else {
            return Ok(false);
        };
        Ok(version.deleted
            && version.source == origin.source
            && version.revision == origin.revision
            || version.remote
                && self.remote_composer_transferred(
                    origin.session,
                    &origin.source,
                    origin.revision,
                )?)
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
        let sources = self.composer_sources(id)?;
        if origin.session != id || origin.source != sources.current {
            return Err("Composer submission belongs to another conversation".into());
        }
        let version = self
            .composer_version(id, sources)?
            .filter(|version| {
                version.revision >= origin.revision && version.source == origin.source
            })
            .ok_or(DRAFT_CHANGED)?;
        if version.remote || (version.revision == origin.revision && version.deleted) {
            return Err(DRAFT_CHANGED.into());
        }
        let revision = i64::try_from(origin.revision).map_err(|error| error.to_string())?;
        ensure_transfer_schema(transaction)?;
        let inserted = transaction
            .execute(
                "INSERT INTO composer_transfers(session_id,revision) VALUES(?1,?2)
                 ON CONFLICT(session_id,revision) DO NOTHING",
                params![origin.source, revision],
            )
            .map_err(|error| error.to_string())?;
        if inserted == 0 {
            return Err("This version of the draft was already accepted.".into());
        }
        if shared::has_table(transaction, "composer_draft_recoveries")? {
            transaction
                .execute(
                    "DELETE FROM composer_draft_recoveries
                     WHERE session_id=?1 AND owner=?2 AND revision=?3",
                    params![origin.source, origin.recovery_owner(), revision],
                )
                .map_err(|error| error.to_string())?;
        }
        if self.shared.is_some() {
            transaction
                .execute(
                    "UPDATE composer_drafts SET deleted=1,updated_at=?1
                     WHERE session_id=?2 AND revision=?3 AND deleted=0",
                    params![
                        crate::time::now_ms().min(i64::MAX as u64) as i64,
                        origin.source,
                        revision
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
                     PARTITION BY owner ORDER BY CASE session_id WHEN ?1 THEN 0 WHEN ?2 THEN 1 WHEN ?3 THEN 2 ELSE 3 END
                 ) AS rank FROM composer_draft_recoveries WHERE session_id IN (?1,?2,?3,?4)
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
        let source = self.composer_sources(id)?.current;
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        Self::write_composer_recovery(&tx, &source, &record)?;
        tx.commit().map_err(|e| e.to_string())
    }

    fn write_composer_recovery(
        tx: &rusqlite::Transaction<'_>,
        source: &str,
        record: &ComposerRecovery,
    ) -> Result<(), String> {
        validate(&record.payload)?;
        crate::accounts::validate_id(&record.owner)?;
        let raw = serde_json::to_string(&record.payload).map_err(|e| e.to_string())?;
        if raw.len() > 10 * 1024 * 1024 {
            return Err("Recovered input exceeds supported limits".into());
        }
        ensure_recovery_schema(tx)?;
        tx.execute("INSERT INTO composer_draft_recoveries VALUES(?1,?2,?3,?4,?5) ON CONFLICT(session_id,owner) DO UPDATE SET revision=excluded.revision,payload_json=excluded.payload_json,updated_at=excluded.updated_at",params![source,record.owner,record.revision.min(i64::MAX as u64) as i64,raw,crate::time::now_ms().min(i64::MAX as u64) as i64]).map_err(|e|e.to_string())?;
        Ok(())
    }

    /// Preserve a displaced local offer until its own receipt commits. The
    /// replacement and its predecessor's recovery are visible together.
    pub fn save_composer_after_local_offer(
        &self,
        origin: &ComposerOrigin,
        payload: &Value,
    ) -> Result<(u64, Option<ComposerRecovery>), String> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )
        .map_err(|error| error.to_string())?;
        let sources = self.composer_sources(origin.session)?;
        let version = self
            .composer_version(origin.session, sources)?
            .filter(|version| {
                !version.remote
                    && version.source == origin.source
                    && version.revision == origin.revision
            })
            .ok_or(DRAFT_CHANGED)?;
        let recovery = if version.deleted {
            None
        } else {
            let original = self.composer_draft(origin.session)?.ok_or(DRAFT_CHANGED)?;
            let record = ComposerRecovery {
                owner: origin.recovery_owner(),
                revision: origin.revision,
                payload: original.payload,
            };
            Self::write_composer_recovery(&tx, &origin.source, &record)?;
            Some(record)
        };
        let revision = self.save_composer_draft(origin.session, origin.revision, payload)?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok((revision, recovery))
    }
    pub fn clear_composer_recovery(&self, id: i64, owner: &str) -> Result<(), String> {
        if !shared::has_table(&self.conn, "composer_draft_recoveries")? {
            return Ok(());
        }
        let sources = self.composer_sources(id)?;
        let [current, first, second, third] = sources.params();
        self.conn
            .execute(
                "DELETE FROM composer_draft_recoveries WHERE session_id IN (?1,?2,?3,?4) AND owner=?5",
                params![current, first, second, third, owner],
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
        let tx = self
            .conn
            .is_autocommit()
            .then(|| {
                rusqlite::Transaction::new_unchecked(
                    &self.conn,
                    rusqlite::TransactionBehavior::Immediate,
                )
            })
            .transpose()
            .map_err(|error| error.to_string())?;
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
        } else {
            // New stores create the same ordinary draft table on first save. No
            // existing row is copied, converted or rewritten during an upgrade.
            ensure_draft_schema(&self.conn)?;
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
            self.conn.execute("INSERT INTO composer_drafts(session_id,cwd,revision,payload_json,deleted,updated_at) VALUES(?1,?2,?3,?4,?5,?6)
            ON CONFLICT(session_id) DO UPDATE SET cwd=excluded.cwd,revision=excluded.revision,payload_json=excluded.payload_json,deleted=excluded.deleted,updated_at=excluded.updated_at",
            params![sources.current,cwd,revision as i64,raw,deleted,crate::time::now_ms().min(i64::MAX as u64) as i64]).map_err(|e|e.to_string())?;
        }
        if let Some(tx) = tx {
            tx.commit().map_err(|e| e.to_string())?;
        }
        Ok(revision)
    }
}

const DRAFT_CHANGED: &str = "The saved draft changed before this send. Your input was preserved.";

#[cfg(test)]
mod tests;
