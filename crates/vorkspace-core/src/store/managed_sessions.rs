//! Desktop metadata for automation-created sessions.
use super::{Session, Store};
use crate::{
    agent::Settings,
    automations::{AutomationRun, DueAutomationRun, Service},
    files::path_key,
    kind::Kind,
    session_groups::WorkItem,
};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};
use serde_json::Value;

impl Store {
    /// Persist the complete fresh session before publishing its running root.
    pub fn create_automation_session(
        &self,
        service: &Service,
        due: &DueAutomationRun,
        settings: &Settings,
        item: Option<&WorkItem>,
        now: i64,
    ) -> Result<(Session, AutomationRun), String> {
        let automation = &due.automation;
        if Kind::parse(&automation.harness).is_none() {
            return Err("The automation requires an available local agent and project.".into());
        }
        if due.run.automation_id != automation.id {
            return Err("The automation run belongs to another definition.".into());
        }
        let worktree =
            (automation.workspace_mode == "existing").then_some(automation.worktree_cwd.as_str());
        if settings.worktree_removed
            || settings.worktree_cwd.as_deref().map(path_key) != worktree.map(path_key)
        {
            return Err("The automation checkout changed before session preparation.".into());
        }
        let mut created = None;
        let result =
            service.prepare_session(&self.conn, self.database_path(), &due.run, now, || {
                let mut session = self.create(&automation.harness, &automation.cwd)?;
                created = Some(session.id);
                if due.run.trigger != "event" {
                    self.title(session.id, &automation.name)?;
                    session.title.clone_from(&automation.name);
                }
                self.save_agent_preferences(session.id, settings)?;
                if !self.link_automation_session(
                    session.id,
                    &automation.id,
                    &automation.harness,
                    &automation.cwd,
                    worktree,
                    item,
                )? {
                    return Err(
                        "The automation session changed before preparation completed.".into(),
                    );
                }
                let persisted = self.persisted_session_id(session.id)?;
                Ok((session, persisted))
            });
        if let (Err(_), Some(id)) = (&result, created) {
            self.shared_forget_uncommitted_identity(id);
        }
        result
    }

    /// A prepared run may link only the exact local checkout it inspected.
    pub fn link_automation_session(
        &self,
        id: i64,
        automation: &str,
        provider: &str,
        project: &str,
        worktree: Option<&str>,
        item: Option<&WorkItem>,
    ) -> Result<bool, String> {
        if id < 0 {
            return Ok(false);
        }
        let tx = self
            .conn
            .is_autocommit()
            .then(|| Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate))
            .transpose()
            .map_err(|error| error.to_string())?;
        let read = |row: &rusqlite::Row<'_>| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, bool>(3)?,
            ))
        };
        let target = if self.shared.is_some() {
            self.conn.query_row(
                &format!(
                    "SELECT cwd,harness,{},{} FROM sessions WHERE id=?1",
                    self.shared_field("archived", "0"),
                    self.shared_field("is_draft", "0"),
                ),
                [self.persisted_session_id(id)?],
                read,
            )
        } else {
            self.conn.query_row(
                "SELECT cwd,provider,archived,0 FROM native_sessions WHERE id=?1",
                [id],
                read,
            )
        };
        let Some((cwd, harness, archived, draft)) =
            target.optional().map_err(|error| error.to_string())?
        else {
            return Ok(false);
        };
        if archived || draft || harness != provider || path_key(&cwd) != path_key(project) {
            return Ok(false);
        }
        let saved = self.agent_preferences(id)?.unwrap_or_default();
        if saved.worktree_removed
            || saved.worktree_cwd.as_deref().map(path_key) != worktree.map(path_key)
        {
            return Ok(false);
        }
        if self.shared.is_some() {
            let mut fields = vec![("automation_id", Value::String(automation.into()))];
            if let Some(item) = item {
                fields.push((
                    "linked_work_item_json",
                    Value::String(super::shared::encode(item)?),
                ));
            }
            self.shared_patch(id, &fields)?;
        } else {
            self.save_setting(&format!("automation:{id}"), automation)?;
            if let Some(item) = item {
                self.set_work_item(id, Some(item))?;
            }
        }
        if let Some(tx) = tx {
            tx.commit().map_err(|error| error.to_string())?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::{AutomationUpsert, DefinitionRevision};
    use std::path::Path;

    #[test]
    fn automation_session_and_dispatch_receipt_publish_in_one_transaction() {
        for shared in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("sessions.sqlite");
            let store = if shared {
                Store::open_vorkspace(&path)
            } else {
                Store::open(&path)
            }
            .unwrap();
            let mut service = Service::open(&path).unwrap();
            let mut draft =
                AutomationUpsert::new("/project".into(), "codex".into(), "model".into());
            draft.name = "Atomic automation".into();
            draft.prompt = "Inspect the prepared conversation.".into();
            draft.workspace_mode = "existing".into();
            draft.worktree_cwd = "/project-checkout".into();
            draft.triggers = Some(Vec::new());
            let automation = service
                .save_definition(draft, DefinitionRevision::New)
                .unwrap();
            let now = 1_000;
            let queued = service
                .run_now(&automation.id, automation.updated_at, now)
                .unwrap();
            let due = service.claim_run(&queued.run.id, now).unwrap().unwrap();
            let settings = Settings {
                model: Some("prepared-model".into()),
                worktree_cwd: Some("/project-checkout".into()),
                ..Settings::default()
            };
            let item = crate::session_groups::parse_github_work_item(
                "https://github.com/acme/app/pull/17",
            )
            .unwrap();
            let observer = rusqlite::Connection::open(&path).unwrap();
            let table = if shared {
                "sessions"
            } else {
                "native_sessions"
            };
            let count = || {
                observer
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap()
            };
            let saved_run = || {
                let raw: String = observer
                    .query_row(
                        "SELECT run_json FROM automation_runs WHERE id=?1",
                        [&due.run.id],
                        |row| row.get(0),
                    )
                    .unwrap();
                serde_json::from_str::<AutomationRun>(&raw).unwrap()
            };
            observer.execute_batch(&format!(
                "CREATE TRIGGER reject_prepared_root BEFORE UPDATE ON automation_runs
                 WHEN json_extract(NEW.run_json,'$.status')='running' BEGIN
                   SELECT RAISE(ABORT,'root rejected after session') WHERE (SELECT COUNT(*) FROM {table})=1;
                   SELECT RAISE(ABORT,'session was not prepared');
                 END;"
            )).unwrap();
            let error = store
                .create_automation_session(&service, &due, &settings, Some(&item), now)
                .unwrap_err();
            assert!(error.contains("root rejected after session"), "{error}");
            assert_eq!(count(), 0);
            assert_eq!(saved_run().status, "preparing");
            assert!(saved_run().session_id.is_none());
            assert_eq!(
                service.list().unwrap()[0].last_run_status.as_deref(),
                Some("preparing")
            );
            if !shared {
                let leftovers: i64 = observer.query_row("SELECT COUNT(*) FROM native_settings WHERE key LIKE 'agent:%' OR key LIKE 'automation:%'", [], |row| row.get(0)).unwrap();
                assert_eq!(leftovers, 0);
            }
            observer
                .execute_batch("DROP TRIGGER reject_prepared_root")
                .unwrap();
            let (session, run) = store
                .create_automation_session(&service, &due, &settings, Some(&item), now)
                .unwrap();
            assert_eq!(count(), 1);
            assert_eq!(session.title, "Atomic automation");
            assert_eq!(run.status, "running");
            assert_eq!(
                saved_run().session_id,
                Some(store.persisted_session_id(session.id).unwrap())
            );
            assert_eq!(run.started_at, Some(now));
            assert_eq!(
                store.agent_preferences(session.id).unwrap().unwrap().model,
                settings.model
            );
            assert_eq!(
                store
                    .agent_preferences(session.id)
                    .unwrap()
                    .unwrap()
                    .worktree_cwd,
                settings.worktree_cwd
            );
            assert_eq!(
                store.bot_session(session.id).unwrap().unwrap().work_item,
                Some(item)
            );
            assert!(store
                .create_automation_session(&service, &due, &settings, None, now)
                .is_err());
            assert_eq!(count(), 1);

            let peer = Service::open(&path).unwrap();
            let mut current = due.clone();
            current.run = run.clone();
            assert!(store
                .create_automation_session(&peer, &current, &settings, None, now)
                .is_err());
            let other = Store::open(&directory.path().join("other.sqlite")).unwrap();
            assert!(other
                .create_automation_session(&service, &current, &settings, None, now)
                .is_err());
            assert!(other.sessions().unwrap().is_empty());
            assert_eq!(count(), 1);
            drop(peer);
            drop(service);
            let mut successor = Service::open(&path).unwrap();
            assert!(successor.poll(now + 1).unwrap().is_empty());
            assert_eq!(saved_run().status, "cancelled");
            assert_eq!(saved_run().session_id, run.session_id);
            assert_eq!(count(), 1);
            assert!(store
                .create_automation_session(&successor, &current, &settings, None, now + 1)
                .is_err());
            assert_eq!(count(), 1);
        }
    }

    #[test]
    fn automation_metadata_uses_the_original_desktop_session_identity() {
        let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        assert!(store
            .link_automation_session(
                session.id,
                "automation-one",
                "codex",
                "/project",
                None,
                None
            )
            .unwrap());
        assert!(!store
            .link_automation_session(
                session.id,
                "stale-run",
                "codex",
                "/project",
                Some("/different-checkout"),
                None,
            )
            .unwrap());
        let persisted = store.persisted_session_id(session.id).unwrap();
        let raw: String = store
            .conn
            .query_row(
                "SELECT automation_id FROM sessions WHERE id=?1",
                [persisted],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(raw, "automation-one");
    }
}
