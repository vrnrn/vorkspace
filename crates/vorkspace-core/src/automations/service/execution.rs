//! One database executor owns recovery and the durable dispatch boundary.
use super::*;
use std::{
    fs::{File, OpenOptions, TryLockError},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
};

pub(super) struct Execution {
    lock: Option<File>,
}

impl Drop for Execution {
    fn drop(&mut self) {
        if let Some(lock) = &self.lock {
            // A concurrently forked child can share this open description
            // until exec. Release the owner's lease before closing our handle.
            let _ = lock.unlock();
        }
    }
}

pub(super) fn lock_path(database: &Path) -> Result<Option<PathBuf>, String> {
    if database == Path::new(":memory:") {
        return Ok(None);
    }
    let mut path = database
        .canonicalize()
        .map_err(|error| error.to_string())?
        .into_os_string();
    path.push(".automations.lock");
    Ok(Some(path.into()))
}

impl Execution {
    fn acquire(path: Option<&Path>) -> Result<Option<Self>, String> {
        let Some(path) = path else {
            return Ok(Some(Self { lock: None }));
        };
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|error| error.to_string())?;
        match lock.try_lock() {
            Ok(()) => Ok(Some(Self { lock: Some(lock) })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
}

impl Service {
    /// Fresh session data and its dispatch receipt share the Store writer.
    /// An interrupted commit can never leave a session outside run recovery.
    pub(crate) fn prepare_session(
        &self,
        conn: &Connection,
        database: &Path,
        expected: &AutomationRun,
        now: i64,
        prepare: impl FnOnce() -> Result<(crate::session::Session, String), String>,
    ) -> Result<(crate::session::Session, AutomationRun), String> {
        if self.execution.is_none() {
            return Err("This process does not own automation execution.".into());
        }
        if self.execution_path.is_none() || lock_path(database)? != self.execution_path {
            return Err("The automation and session databases do not match.".into());
        }
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let mut run = get_run(&tx, &expected.id)?;
        if run.automation_id != expected.automation_id
            || run.session_id != expected.session_id
            || !matches!(run.status.as_str(), "preparing" | "running")
        {
            return Err("The automation run changed before session preparation.".into());
        }
        let mut automation = get(&tx, &run.automation_id)?.ok_or("Automation not found.")?;
        let (session, persisted) = prepare()?;
        validate_id(&persisted, "session")?;
        run.status = "running".into();
        run.session_id = Some(persisted);
        run.started_at.get_or_insert(now);
        run.error = None;
        apply_run_summary(&mut automation, &run);
        write_automation(&tx, &automation)?;
        write_run(&tx, &run)?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok((session, run))
    }

    fn ensure_execution(&mut self, now: i64) -> Result<bool, String> {
        if self.execution.is_some() {
            return Ok(true);
        }
        let Some(execution) = Execution::acquire(self.execution_path.as_deref())? else {
            return Ok(false);
        };
        // The process lease proves every prior preparing or running record is
        // orphaned, independently of wall-clock changes since its admission.
        self.recover(i64::MAX, now)?;
        self.execution = Some(execution);
        Ok(true)
    }

    /// Queue creation and definition editing remain available to other processes.
    pub fn poll(&mut self, now: i64) -> Result<Vec<DueAutomationRun>, String> {
        if !self.ensure_execution(now)? {
            return Ok(Vec::new());
        }
        let mut due = {
            let mut statement = self
                .conn
                .prepare(&format!(
                    "SELECT run_json FROM automation_runs WHERE {RECOVERABLE_RUN}
                 AND json_extract(run_json, '$.status')='pending'
                 ORDER BY created_at, id LIMIT 100"
                ))
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|error| error.to_string())?;
            let mut pending = Vec::new();
            for row in rows {
                let run: AutomationRun =
                    serde_json::from_str(&row.map_err(|error| error.to_string())?)
                        .map_err(|error| error.to_string())?;
                if let Some(automation) = get(&self.conn, &run.automation_id)? {
                    pending.push(DueAutomationRun { automation, run });
                }
            }
            pending
        };
        due.extend(self.claim_due(now)?);
        Ok(due)
    }

    /// Preparing is durable before session work. Provider submission must wait
    /// for a later running receipt with the final session identity.
    pub fn claim_run(&mut self, id: &str, now: i64) -> Result<Option<DueAutomationRun>, String> {
        validate_id(id, "automation run")?;
        if !self.ensure_execution(now)? {
            return Ok(None);
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let mut run = get_run(&tx, id)?;
        if run.status != "pending" {
            return Ok(None);
        }
        let mut automation = get(&tx, &run.automation_id)?.ok_or("Automation not found.")?;
        run.status = "preparing".into();
        apply_run_summary(&mut automation, &run);
        write_run(&tx, &run)?;
        write_automation(&tx, &automation)?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(Some(DueAutomationRun { automation, run }))
    }
}
