//! Durable admission for authenticated app commands. SQLite and provider/UI/Git
//! effects cannot form one transaction: an admitted command with no saved result
//! is unconfirmed and is never dispatched again.
use super::Store;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, PartialEq)]
pub enum AppCommandAdmission {
    Admitted,
    Completed(Result<Value, String>),
}
fn fingerprint(action: &str, input: &Value) -> String {
    fn hash(value: &Value, state: &mut Sha256) {
        match value {
            Value::Object(object) => {
                state.update(b"{");
                let mut entries = object.iter().collect::<Vec<_>>();
                entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
                for (key, value) in entries {
                    state.update(serde_json::to_string(key).unwrap().as_bytes());
                    state.update(b":");
                    hash(value, state);
                    state.update(b",");
                }
                state.update(b"}");
            }
            Value::Array(array) => {
                state.update(b"[");
                for value in array {
                    hash(value, state);
                    state.update(b",");
                }
                state.update(b"]");
            }
            _ => state.update(value.to_string().as_bytes()),
        }
    }
    let mut state = Sha256::new();
    state.update(action.as_bytes());
    state.update(b"\0");
    hash(input, &mut state);
    format!("{:x}", state.finalize())
}
pub(super) fn ensure(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_command_receipts(
        caller_session_id TEXT NOT NULL, request_id TEXT NOT NULL,
        fingerprint TEXT NOT NULL, phase TEXT NOT NULL CHECK(phase IN ('admitted','settled')),
        result_json TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
        PRIMARY KEY(caller_session_id,request_id));",
    )
    .map_err(|e| e.to_string())
}
fn validate(request: &str, action: &str, input: &Value) -> Result<(), String> {
    if request.is_empty()
        || request.len() > 128
        || request.contains('\0')
        || action.is_empty()
        || action.len() > 128
        || action.contains('\0')
        || !input.is_object()
        || input.to_string().len() > 1024 * 1024
    {
        return Err("Invalid app command identity or input".into());
    }
    Ok(())
}
impl Store {
    /// Classification is read from durable owners, not the sidebar's partial
    /// list. Call this again at the effect boundary after any deferred work.
    pub fn check_app_target(&self, caller: i64, target: i64, project: &str) -> Result<(), String> {
        if target == caller {
            return Err("App commands cannot target the calling session".into());
        }
        let session = self.bot_session(target)?.ok_or("Session is unavailable")?;
        if crate::files::path_key(&session.cwd) != crate::files::path_key(project) {
            return Err("Session is outside the assigned project".into());
        }
        let roster = crate::bot::Roster::load(self)?;
        let channels = crate::bot::channels::Channels::load(self)?;
        let protected = roster.hidden_sessions.contains(&target)
            || roster.of_session(target).is_some()
            || roster
                .bots
                .iter()
                .any(|bot| bot.habit_sessions.contains(&target))
            || channels
                .channels
                .iter()
                .any(|channel| channel.member_sessions.values().any(|id| *id == target));
        if protected {
            return Err(
                "Bot, habit, channel and hidden sessions are protected from app commands".into(),
            );
        }
        if self.shared.is_some() {
            let legacy_launch = if let Some(parent) = self.shared_app_launch_caller(target)? {
                roster
                    .of_session(parent)
                    .is_some_and(|bot| bot.works_on(&session.cwd))
                    || self.bot_session(parent)?.is_some_and(|parent| {
                        crate::files::path_key(&parent.cwd) == crate::files::path_key(&session.cwd)
                    })
            } else {
                false
            };
            let ordinary_launch = legacy_launch
                || roster.bots.iter().any(|bot| {
                    bot.launches
                        .iter()
                        .any(|launch| launch.session_id == target)
                });
            if self.shared_sidebar_hidden()?.contains(&target) && !ordinary_launch {
                return Err(
                    "Internal or unclassified hidden sessions are protected from app commands"
                        .into(),
                );
            }
            if super::shared::has_table(&self.conn, "orchestration_workers")? {
                let original = self.persisted_session_id(target)?;
                if self
                    .conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM orchestration_workers WHERE session_id=?1)",
                        [original],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(|e| e.to_string())?
                {
                    return Err("Orchestration workers are protected from app commands".into());
                }
            }
        }
        for (_, raw) in self.settings_with_prefix("orchestration:")? {
            let summary: crate::orchestration::Summary = serde_json::from_str(&raw)
                .map_err(|e| format!("Cannot verify orchestration ownership: {e}"))?;
            if summary
                .tasks
                .iter()
                .any(|task| task.session == Some(target))
            {
                return Err("Orchestration workers are protected from app commands".into());
            }
        }
        if self
            .agent_settings(target)?
            .is_some_and(|settings| settings.bot.is_some())
        {
            return Err("Bot sessions are protected from app commands".into());
        }
        Ok(())
    }
    pub fn archive_app_session(
        &self,
        caller: i64,
        target: i64,
        project: &str,
    ) -> Result<(), String> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        self.check_app_target(caller, target, project)?;
        self.set_archived(target, true)?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn delete_app_session(
        &mut self,
        caller: i64,
        target: i64,
        project: &str,
    ) -> Result<(), String> {
        self.delete_checked(target, Some((caller, project)))
    }
    fn app_caller_identity(&self, caller: i64) -> Result<String, String> {
        if self.shared.is_some() {
            self.persisted_session_id(caller)
        } else {
            Ok(caller.to_string())
        }
    }
    pub fn app_command_known(&self, caller: i64, request: &str) -> Result<bool, String> {
        if !super::shared::has_table(&self.conn, "app_command_receipts")? {
            return Ok(false);
        }
        let caller = self.app_caller_identity(caller)?;
        self.conn.query_row("SELECT EXISTS(SELECT 1 FROM app_command_receipts WHERE caller_session_id=?1 AND request_id=?2)",params![caller,request],|row|row.get(0)).map_err(|e|e.to_string())
    }
    pub fn admit_app_command(
        &self,
        caller: i64,
        request: &str,
        action: &str,
        input: &Value,
    ) -> Result<AppCommandAdmission, String> {
        validate(request, action, input)?;
        if self.shared.is_some() {
            self.shared_writable()?;
            // Receipt DDL belongs to the same explicit-write schema owner as
            // history guards. Adding it after scalar snapshots would invalidate
            // active conversations as though an external writer changed schema.
            self.shared_history_schema()?;
        } else {
            ensure(&self.conn)?;
        }
        let caller = self.app_caller_identity(caller)?;
        let signature = fingerprint(action, input);
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let table = if self.shared.is_some() {
            "sessions"
        } else {
            "native_sessions"
        };
        let created=tx.execute(&format!("INSERT INTO app_command_receipts(caller_session_id,request_id,fingerprint,phase,created_at,updated_at)
            SELECT ?1,?2,?3,'admitted',?4,?4 WHERE EXISTS(SELECT 1 FROM {table} WHERE id=?1)
            ON CONFLICT(caller_session_id,request_id) DO NOTHING"),params![caller,request,signature,crate::time::now_ms().min(i64::MAX as u64) as i64]).map_err(|e|e.to_string())? ==1;
        let saved:Option<(String,String,Option<String>)>=tx.query_row("SELECT fingerprint,phase,result_json FROM app_command_receipts WHERE caller_session_id=?1 AND request_id=?2",params![caller,request],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())?;
        let (previous, phase, result) =
            saved.ok_or("The app command caller no longer exists; no action was admitted")?;
        if previous != signature {
            return Err("Request ID was already used with a different action or input".into());
        }
        let result = if created {
            AppCommandAdmission::Admitted
        } else if phase == "settled" {
            AppCommandAdmission::Completed(
                serde_json::from_str(
                    &result.ok_or("App command result is unavailable; it was not replayed")?,
                )
                .map_err(|e| {
                    format!("Cannot read the saved app result; it was not replayed: {e}")
                })?,
            )
        } else {
            return Err("This request was already admitted and its result is unconfirmed. It may have executed; it was not replayed.".into());
        };
        tx.commit().map_err(|e| e.to_string())?;
        Ok(result)
    }
    pub fn finish_app_command(
        &self,
        caller: i64,
        request: &str,
        action: &str,
        input: &Value,
        result: &Result<Value, String>,
    ) -> Result<(), String> {
        validate(request, action, input)?;
        let caller = self.app_caller_identity(caller)?;
        let raw = serde_json::to_string(result).map_err(|e| e.to_string())?;
        if raw.len() > 4 * 1024 * 1024 {
            return Err("App command result exceeds its receipt limit; its effect is unconfirmed and must not be replayed".into());
        }
        let changed=self.conn.execute("UPDATE app_command_receipts SET phase='settled',result_json=?4,updated_at=?5 WHERE caller_session_id=?1 AND request_id=?2 AND fingerprint=?3 AND phase='admitted'",params![caller,request,fingerprint(action,input),raw,crate::time::now_ms().min(i64::MAX as u64) as i64]).map_err(|e|format!("Could not save the app command result; it may have executed and must not be replayed: {e}"))?;
        if changed != 1 {
            return Err(
                "App command result no longer belongs to this admission; it was not replayed"
                    .into(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
