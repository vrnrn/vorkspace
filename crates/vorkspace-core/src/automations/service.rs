//! Owns automation storage, transactional admission and recovery.
use super::{
    has_time_trigger, hydrate_triggers, new_id, next_run_at, normalize_triggers, now_millis,
    validate_id, validate_upsert, Automation, AutomationEventClaim, AutomationRun,
    AutomationUpsert, DueAutomationRun, MAX_PROMPT,
};
use super::schema::{ensure_schema, RECOVERABLE_RUN};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, TransactionBehavior};
use std::path::Path;

mod execution;

const MAX_RUNS_PER_AUTOMATION: i64 = 100;

const EVENT_CLAIMS_BACKFILL: &str = "event-claims-v1";
const DEFINITION_CHANGED: &str =
    "This automation changed in another view. Use Reset to load the saved version.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefinitionRevision {
    New,
    Saved(i64),
}

impl DefinitionRevision {
    fn of(saved: Option<&Automation>) -> Self {
        saved.map_or(Self::New, |value| Self::Saved(value.updated_at))
    }
}

fn claims_backfilled(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM automation_migrations WHERE name = ?1)",
        [EVENT_CLAIMS_BACKFILL],
        |row| row.get(0),
    )
}

fn prepare_storage(conn: &mut Connection) -> rusqlite::Result<()> {
    ensure_schema(conn)?;
    if claims_backfilled(conn)? {
        return Ok(());
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if claims_backfilled(&tx)? {
        return tx.commit();
    }
    // Backfill claims from pre-ledger run history. Invalid legacy rows should not
    // prevent the session store from opening.
    let mut statement =
        tx.prepare("SELECT automation_id, created_at, run_json FROM automation_runs")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut insert = tx.prepare(
        "INSERT OR IGNORE INTO automation_event_claims
         (automation_id, event_key, created_at) VALUES (?1, ?2, ?3)",
    )?;
    for row in rows {
        let (automation_id, created_at, raw) = row?;
        if let Ok(run) = serde_json::from_str::<AutomationRun>(&raw) {
            if let Some(event_key) = run.event_key {
                insert.execute(params![automation_id, event_key, created_at])?;
            }
        }
    }
    drop(insert);
    drop(statement);
    tx.execute(
        "INSERT INTO automation_migrations(name) VALUES (?1)",
        [EVENT_CLAIMS_BACKFILL],
    )?;
    tx.commit()
}

fn parse_automation(value: String) -> Result<Automation, String> {
    let mut automation: Automation =
        serde_json::from_str(&value).map_err(|error| error.to_string())?;
    hydrate_triggers(&mut automation);
    Ok(automation)
}

fn list(conn: &Connection, due_by: Option<i64>) -> Result<Vec<Automation>, String> {
    let sql = if due_by.is_some() {
        "SELECT definition_json FROM automations
         WHERE enabled = 1 AND next_run_at <= ?1 ORDER BY updated_at DESC, id"
    } else {
        "SELECT definition_json FROM automations ORDER BY updated_at DESC, id"
    };
    let mut statement = conn.prepare(sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params_from_iter(due_by), |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    rows.map(|row| {
        let value = row.map_err(|error| error.to_string())?;
        parse_automation(value)
    })
    .collect()
}

fn get(conn: &Connection, id: &str) -> Result<Option<Automation>, String> {
    let raw = conn
        .query_row(
            "SELECT definition_json FROM automations WHERE id = ?1",
            [id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    raw.map(parse_automation).transpose()
}

fn write_automation(conn: &Connection, automation: &Automation) -> Result<(), String> {
    let raw = serde_json::to_string(automation).map_err(|error| error.to_string())?;
    conn.execute(
        "INSERT INTO automations (id, definition_json, enabled, next_run_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET
           definition_json = excluded.definition_json,
           enabled = excluded.enabled,
           next_run_at = excluded.next_run_at,
           updated_at = excluded.updated_at",
        params![
            automation.id,
            raw,
            automation.enabled,
            automation.next_run_at,
            automation.updated_at
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn write_run(conn: &Connection, run: &AutomationRun) -> Result<(), String> {
    let raw = serde_json::to_string(run).map_err(|error| error.to_string())?;
    conn.execute(
        "INSERT INTO automation_runs (id, automation_id, created_at, run_json)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET run_json = excluded.run_json",
        params![run.id, run.automation_id, run.created_at, raw],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn get_run(conn: &Connection, id: &str) -> Result<AutomationRun, String> {
    let raw = conn
        .query_row(
            "SELECT run_json FROM automation_runs WHERE id = ?1",
            [id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or("Automation run not found.")?;
    serde_json::from_str(&raw).map_err(|error| error.to_string())
}

fn new_run(
    automation_id: &str,
    trigger: &str,
    scheduled_for: i64,
    now: i64,
    prompt: &str,
) -> AutomationRun {
    AutomationRun {
        id: new_id(),
        automation_id: automation_id.to_string(),
        trigger: trigger.to_string(),
        scheduled_for,
        created_at: now,
        started_at: None,
        completed_at: None,
        status: "pending".into(),
        session_id: None,
        error: None,
        event_key: None,
        event_kind: None,
        event: None,
        prompt: Some(prompt.to_string()),
    }
}

fn new_event_run(
    automation_id: &str,
    event_key: &str,
    event_kind: &str,
    event: &str,
    scheduled_for: i64,
    now: i64,
    prompt: &str,
) -> AutomationRun {
    let mut run = new_run(automation_id, "event", scheduled_for, now, prompt);
    run.event_key = Some(event_key.to_string());
    run.event_kind = Some(event_kind.to_string());
    run.event = Some(event.to_string());
    run
}

fn validate_event_key(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 400
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'/' | b'.')
        })
    {
        return Err("Invalid automation event key.".into());
    }
    Ok(())
}

fn has_event_trigger(automation: &Automation, kind: &str, event: &str) -> bool {
    match &automation.triggers {
        Some(triggers) => triggers
            .iter()
            .any(|trigger| trigger.kind == kind && trigger.event == event),
        None => automation.trigger_kind == kind && automation.trigger_event == event,
    }
}

fn list_runs(conn: &Connection, automation_id: &str) -> Result<Vec<AutomationRun>, String> {
    let mut statement = conn
        .prepare(
            "SELECT run_json FROM automation_runs WHERE automation_id = ?1
             ORDER BY created_at DESC LIMIT 100",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([automation_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    rows.map(|row| {
        let value = row.map_err(|error| error.to_string())?;
        serde_json::from_str(&value).map_err(|error| error.to_string())
    })
    .collect()
}

fn trim_history(conn: &Connection, automation_id: &str) -> Result<(), String> {
    let mut statement = conn
        .prepare(
            "SELECT id, run_json FROM automation_runs WHERE id IN (
                 SELECT id FROM automation_runs WHERE automation_id = ?1
                 ORDER BY created_at DESC, id DESC LIMIT -1 OFFSET ?2
             )",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![automation_id, MAX_RUNS_PER_AUTOMATION], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?;
    let mut delete_ids = Vec::new();
    for row in rows {
        let (id, raw) = row.map_err(|error| error.to_string())?;
        let run: AutomationRun = serde_json::from_str(&raw).map_err(|error| error.to_string())?;
        if matches!(
            run.status.as_str(),
            "succeeded" | "failed" | "skipped" | "cancelled"
        ) {
            delete_ids.push(id);
        }
    }
    drop(statement);
    for id in delete_ids {
        conn.execute("DELETE FROM automation_runs WHERE id = ?1", [id])
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn apply_run_summary(automation: &mut Automation, run: &AutomationRun) {
    if automation
        .last_run_at
        .is_none_or(|last_run_at| run.created_at >= last_run_at)
    {
        automation.last_run_at = Some(run.created_at);
        automation.last_run_status = Some(run.status.clone());
        automation.last_run_error = run.error.clone();
        if run.session_id.is_some() {
            automation.last_session_id = run.session_id.clone();
        }
    }
}

/// Opens automation definitions and history in the desktop session database.
pub struct Service {
    conn: Connection,
    execution_path: Option<std::path::PathBuf>,
    execution: Option<execution::Execution>,
}
impl Service {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        let mut conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(|e| e.to_string())?;
        prepare_storage(&mut conn).map_err(|e| e.to_string())?;
        Ok(Self {
            conn,
            execution_path: execution::lock_path(path)?,
            execution: None,
        })
    }
    pub fn list(&self) -> Result<Vec<Automation>, String> {
        list(&self.conn, None)
    }

    pub fn save_definition(
        &mut self,
        automation: AutomationUpsert,
        expected: DefinitionRevision,
    ) -> Result<Automation, String> {
        let now = now_millis();
        validate_upsert(&automation, now)?;
        let triggers = Some(normalize_triggers(&automation));
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let previous = get(&tx, &automation.id)?;
        if expected != DefinitionRevision::of(previous.as_ref()) {
            return Err(DEFINITION_CHANGED.into());
        }
        let created_at = previous
            .as_ref()
            .map(|value| value.created_at)
            .unwrap_or(now);
        let updated_at = match &previous {
            Some(value) => now.max(
                value
                    .updated_at
                    .checked_add(1)
                    .ok_or("Automation revision exhausted.")?,
            ),
            None => now,
        };
        let worktree_cwd = if automation.workspace_mode == "existing" {
            automation.worktree_cwd.trim().to_string()
        } else {
            String::new()
        };
        let saved = Automation {
            id: automation.id,
            name: automation.name.trim().to_string(),
            prompt: automation.prompt,
            harness: automation.harness,
            model: automation.model,
            model_settings: automation.model_settings,
            cwd: automation.cwd,
            workspace_mode: automation.workspace_mode,
            worktree_cwd,
            session_folder_id: automation.session_folder_id.trim().to_string(),
            reuse_session: automation.reuse_session,
            runtime_mode: automation.runtime_mode,
            trigger_kind: automation.trigger_kind,
            trigger_event: automation.trigger_event.trim().to_string(),
            schedule_kind: automation.schedule_kind,
            minute: automation.minute,
            time: automation.time,
            day_of_week: automation.day_of_week,
            triggers,
            missed_run_grace_minutes: automation.missed_run_grace_minutes,
            enabled: automation.enabled,
            next_run_at: automation.next_run_at,
            last_run_at: previous.as_ref().and_then(|value| value.last_run_at),
            last_run_status: previous
                .as_ref()
                .and_then(|value| value.last_run_status.clone()),
            last_run_error: previous
                .as_ref()
                .and_then(|value| value.last_run_error.clone()),
            last_session_id: previous.and_then(|value| value.last_session_id),
            created_at,
            updated_at,
        };
        write_automation(&tx, &saved)?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(saved)
    }

    pub fn delete_definition(&mut self, id: &str, revision: i64) -> Result<(), String> {
        validate_id(id, "automation")?;
        let removed = self
            .conn
            .execute(
                "DELETE FROM automations WHERE id = ?1 AND updated_at = ?2",
                params![id, revision],
            )
            .map_err(|error| error.to_string())?;
        if removed == 0 {
            Err(DEFINITION_CHANGED.into())
        } else {
            Ok(())
        }
    }

    pub fn runs(&self, automation_id: &str) -> Result<Vec<AutomationRun>, String> {
        validate_id(automation_id, "automation")?;
        let mut runs = list_runs(&self.conn, automation_id)?;
        // Launch prompts can be large and are only needed by restart recovery, not
        // by the run-history surface.
        for run in &mut runs {
            run.prompt = None;
        }
        Ok(runs)
    }

    fn recover(&mut self, started_before: i64, now: i64) -> Result<Vec<DueAutomationRun>, String> {
        let tx = self.conn.transaction().map_err(|error| error.to_string())?;
        let mut statement = tx
            .prepare(&format!(
                "SELECT run_json FROM automation_runs WHERE created_at <= ?1 AND {RECOVERABLE_RUN}
                 ORDER BY created_at, id"
            ))
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([started_before], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        let mut runs = Vec::new();
        for row in rows {
            let raw = row.map_err(|error| error.to_string())?;
            runs.push(
                serde_json::from_str::<AutomationRun>(&raw).map_err(|error| error.to_string())?,
            );
        }
        drop(statement);

        let mut due = Vec::new();
        let mut changed_automation_ids = Vec::new();
        for mut run in runs {
            let Some(mut automation) = get(&tx, &run.automation_id)? else {
                continue;
            };
            if run.status == "preparing" {
                run.status = "pending".into();
                run.started_at = None;
                write_run(&tx, &run)?;
                apply_run_summary(&mut automation, &run);
                write_automation(&tx, &automation)?;
            }
            // Historical runs remain visible, but a retired connector's queued
            // event must not be dispatched again after restart.
            if run.status == "pending"
                && run.trigger == "event"
                && !run
                    .event_kind
                    .as_deref()
                    .is_some_and(|kind| crate::inbox::PROVIDERS.contains(&kind))
            {
                run.status = "cancelled".into();
                run.completed_at = Some(now);
                run.error = Some("This connector is no longer supported.".into());
                apply_run_summary(&mut automation, &run);
                write_run(&tx, &run)?;
                write_automation(&tx, &automation)?;
                changed_automation_ids.push(run.automation_id.clone());
                continue;
            }
            if run.status == "running" {
                run.status = "cancelled".into();
                run.completed_at = Some(now);
                run.error = Some("Interrupted when vorkspace last stopped.".into());
                apply_run_summary(&mut automation, &run);
                write_run(&tx, &run)?;
                write_automation(&tx, &automation)?;
                changed_automation_ids.push(run.automation_id.clone());
                continue;
            }
            if run.trigger == "event"
                && run
                    .prompt
                    .as_deref()
                    .is_none_or(|prompt| prompt.trim().is_empty())
            {
                run.status = "failed".into();
                run.completed_at = Some(now);
                run.error =
                    Some("The connector event prompt was not available after restart.".into());
                apply_run_summary(&mut automation, &run);
                write_run(&tx, &run)?;
                write_automation(&tx, &automation)?;
                changed_automation_ids.push(run.automation_id.clone());
                continue;
            }
            due.push(DueAutomationRun { automation, run });
        }
        changed_automation_ids.sort();
        changed_automation_ids.dedup();
        for automation_id in changed_automation_ids {
            trim_history(&tx, &automation_id)?;
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok(due)
    }
    pub fn run_now(
        &mut self,
        automation_id: &str,
        revision: i64,
        now: i64,
    ) -> Result<DueAutomationRun, String> {
        validate_id(automation_id, "automation")?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let mut automation =
            get(&tx, automation_id)?.ok_or_else(|| "Automation not found.".to_string())?;
        if automation.updated_at != revision {
            return Err(DEFINITION_CHANGED.into());
        }
        let run = new_run(automation_id, "manual", now, now, &automation.prompt);
        automation.last_run_at = Some(now);
        automation.last_run_status = Some(run.status.clone());
        automation.last_run_error = None;
        write_run(&tx, &run)?;
        write_automation(&tx, &automation)?;
        trim_history(&tx, automation_id)?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(DueAutomationRun { automation, run })
    }

    pub fn claim_occurrence(
        &mut self,
        automation_id: &str,
        expected_next_run_at: i64,
        next_run_at: i64,
        now: i64,
    ) -> Result<Option<DueAutomationRun>, String> {
        validate_id(automation_id, "automation")?;
        if next_run_at <= expected_next_run_at {
            return Err("The following automation occurrence must advance the schedule.".into());
        }
        let tx = self.conn.transaction().map_err(|error| error.to_string())?;
        let current =
            get(&tx, automation_id)?.ok_or_else(|| "Automation not found.".to_string())?;
        if !has_time_trigger(&current) {
            tx.rollback().map_err(|error| error.to_string())?;
            return Ok(None);
        }
        let claimed = tx
            .execute(
                "UPDATE automations SET next_run_at = ?1
             WHERE id = ?2 AND enabled = 1 AND next_run_at = ?3 AND next_run_at <= ?4",
                params![next_run_at, automation_id, expected_next_run_at, now],
            )
            .map_err(|error| error.to_string())?;
        if claimed == 0 {
            tx.rollback().map_err(|error| error.to_string())?;
            return Ok(None);
        }
        let mut automation =
            get(&tx, automation_id)?.ok_or_else(|| "Automation not found.".to_string())?;
        automation.next_run_at = next_run_at;
        let mut run = new_run(
            automation_id,
            "scheduled",
            expected_next_run_at,
            now,
            &automation.prompt,
        );
        let grace_ms = automation.missed_run_grace_minutes.saturating_mul(60_000);
        let missed = now.saturating_sub(expected_next_run_at);
        let dispatch = missed <= grace_ms;
        if !dispatch {
            run.status = "skipped".into();
            run.completed_at = Some(now);
            run.error = Some("Missed the scheduled run beyond its grace period.".into());
        }
        automation.last_run_at = Some(now);
        automation.last_run_status = Some(run.status.clone());
        automation.last_run_error = run.error.clone();
        write_run(&tx, &run)?;
        write_automation(&tx, &automation)?;
        trim_history(&tx, automation_id)?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(Some(DueAutomationRun { automation, run }))
    }

    pub fn claim_event(
        &mut self,
        automation_id: &str,
        claim: AutomationEventClaim,
        now: i64,
    ) -> Result<Option<DueAutomationRun>, String> {
        validate_id(automation_id, "automation")?;
        validate_event_key(&claim.event_key)?;
        if !matches!(claim.event_kind.as_str(), "github" | "gitlab" | "jira") {
            return Err("Invalid automation trigger.".into());
        }
        if claim.event.is_empty() || claim.event.len() > 200 {
            return Err("Invalid automation trigger event.".into());
        }
        if claim.prompt.trim().is_empty() || claim.prompt.len() > MAX_PROMPT + 10_000 {
            return Err("Invalid automation run prompt.".into());
        }
        let scheduled_for = if claim.scheduled_for > 0 {
            claim.scheduled_for
        } else {
            now
        };
        let tx = self.conn.transaction().map_err(|error| error.to_string())?;
        let mut automation =
            get(&tx, automation_id)?.ok_or_else(|| "Automation not found.".to_string())?;
        if !automation.enabled || !has_event_trigger(&automation, &claim.event_kind, &claim.event) {
            tx.rollback().map_err(|error| error.to_string())?;
            return Ok(None);
        }
        let claimed = tx
            .execute(
                "INSERT OR IGNORE INTO automation_event_claims
             (automation_id, event_key, created_at) VALUES (?1, ?2, ?3)",
                params![automation_id, claim.event_key, now],
            )
            .map_err(|error| error.to_string())?;
        if claimed == 0 {
            tx.rollback().map_err(|error| error.to_string())?;
            return Ok(None);
        }
        let run = new_event_run(
            automation_id,
            &claim.event_key,
            &claim.event_kind,
            &claim.event,
            scheduled_for,
            now,
            &claim.prompt,
        );
        automation.last_run_at = Some(now);
        automation.last_run_status = Some(run.status.clone());
        automation.last_run_error = None;
        write_run(&tx, &run)?;
        write_automation(&tx, &automation)?;
        trim_history(&tx, automation_id)?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(Some(DueAutomationRun { automation, run }))
    }

    pub fn update_run(
        &mut self,
        run_id: &str,
        status: &str,
        session_id: Option<String>,
        error: Option<String>,
        now: i64,
    ) -> Result<AutomationRun, String> {
        validate_id(run_id, "automation run")?;
        if let Some(session_id) = session_id.as_deref() {
            validate_id(session_id, "session")?;
        }
        if !matches!(status, "running" | "succeeded" | "failed" | "cancelled") {
            return Err("Invalid automation run status.".into());
        }
        if self.execution.is_none() {
            return Err("This process does not own automation execution.".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let mut run = get_run(&tx, run_id)?;
        if !matches!(run.status.as_str(), "preparing" | "running") {
            return Err("This automation run has already settled or was not claimed.".into());
        }
        if run.status == "preparing" && status == "succeeded" {
            return Err("This automation run has not started.".into());
        }
        if status == "running" && session_id.is_none() && run.session_id.is_none() {
            return Err("The automation session must be saved before submission.".into());
        }
        run.status = status.to_owned();
        if run.status == "running" && run.started_at.is_none() {
            run.started_at = Some(now);
        }
        if matches!(
            run.status.as_str(),
            "succeeded" | "failed" | "skipped" | "cancelled"
        ) {
            run.completed_at = Some(now);
        }
        if session_id.is_some() {
            run.session_id = session_id;
        }
        run.error = error.filter(|value| !value.trim().is_empty());
        let mut automation =
            get(&tx, &run.automation_id)?.ok_or_else(|| "Automation not found.".to_string())?;
        apply_run_summary(&mut automation, &run);
        write_run(&tx, &run)?;
        write_automation(&tx, &automation)?;
        trim_history(&tx, &run.automation_id)?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(run)
    }
    pub fn set_enabled(
        &mut self,
        id: &str,
        revision: i64,
        enabled: bool,
    ) -> Result<Automation, String> {
        let automation = get(&self.conn, id)?.ok_or("Automation not found.")?;
        if automation.updated_at != revision {
            return Err(DEFINITION_CHANGED.into());
        }
        if automation.enabled == enabled {
            return Ok(automation);
        }
        let mut draft = AutomationUpsert::from_automation(&automation);
        draft.enabled = enabled;
        draft.sync_triggers(now_millis());
        self.save_definition(draft, DefinitionRevision::Saved(revision))
    }
    /// Atomically claims each due occurrence, including skipped occurrences outside
    /// the grace period. Bounded work prevents an offline period monopolizing UI polling.
    fn claim_due(&mut self, now: i64) -> Result<Vec<DueAutomationRun>, String> {
        let mut due = Vec::new();
        for mut automation in list(&self.conn, Some(now))? {
            if !automation.enabled || !has_time_trigger(&automation) {
                continue;
            }
            for _ in 0..100 {
                if automation.next_run_at > now {
                    break;
                }
                let next = next_run_at(&automation.triggers(), automation.next_run_at);
                if next <= automation.next_run_at {
                    break;
                }
                let claim = match self.claim_occurrence(
                    &automation.id,
                    automation.next_run_at,
                    next,
                    now,
                ) {
                    Ok(Some(claim)) => claim,
                    // A failed transaction leaves this occurrence due for the
                    // next poll. Preserve already committed claims so callers
                    // can dispatch them instead of stranding pending runs.
                    Ok(None) | Err(_) => break,
                };
                automation = claim.automation.clone();
                if claim.run.status == "pending" {
                    due.push(claim);
                }
            }
        }
        Ok(due)
    }
}

#[cfg(test)]
mod tests;
