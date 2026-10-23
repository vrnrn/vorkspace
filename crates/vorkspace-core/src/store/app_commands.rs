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
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn admission_survives_reopen_and_cannot_replay_an_unknown_effect_or_changed_payload() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sessions.db");
        let store = Store::open_vorkspace(&path).unwrap();
        let caller = store.create("codex", "/project").unwrap().id;
        let input = json!({"prompt":"hello","modelSettings":{"effort":"high"}});
        assert_eq!(
            store
                .admit_app_command(caller, "pending", "sessions.start", &input)
                .unwrap(),
            AppCommandAdmission::Admitted
        );
        drop(store);
        let store = Store::open_vorkspace(&path).unwrap();
        assert!(store
            .admit_app_command(caller, "pending", "sessions.start", &input)
            .unwrap_err()
            .contains("unconfirmed"));
        assert!(store
            .admit_app_command(
                caller,
                "pending",
                "sessions.start",
                &json!({"prompt":"other"})
            )
            .unwrap_err()
            .contains("different"));
        assert!(store
            .admit_app_command(caller, "pending", "notes.write", &input)
            .unwrap_err()
            .contains("different"));
    }
    #[test]
    fn completed_results_are_durable_beyond_prior_cache_eviction_and_scoped_to_original_caller() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sessions.db");
        let store = Store::open_vorkspace(&path).unwrap();
        let caller = store.create("codex", "/project").unwrap().id;
        let other = store.create("codex", "/project").unwrap().id;
        for i in 0..300 {
            let request = format!("command-{i}");
            let input = json!({"id":i});
            assert_eq!(
                store
                    .admit_app_command(caller, &request, "notes.write", &input)
                    .unwrap(),
                AppCommandAdmission::Admitted
            );
            store
                .finish_app_command(
                    caller,
                    &request,
                    "notes.write",
                    &input,
                    &Ok(json!({"saved":i})),
                )
                .unwrap();
        }
        drop(store);
        let store = Store::open_vorkspace(&path).unwrap();
        assert_eq!(
            store
                .admit_app_command(caller, "command-0", "notes.write", &json!({"id":0}))
                .unwrap(),
            AppCommandAdmission::Completed(Ok(json!({"saved":0})))
        );
        assert_eq!(
            store
                .admit_app_command(other, "command-0", "notes.write", &json!({"id":0}))
                .unwrap(),
            AppCommandAdmission::Admitted
        );
    }
    #[test]
    fn failed_admission_never_authorizes_an_effect_and_failed_completion_stays_unconfirmed() {
        let store = Store::open_vorkspace(std::path::Path::new(":memory:")).unwrap();
        let caller = store.create("codex", "/project").unwrap().id;
        ensure(&store.conn).unwrap();
        store.conn.execute_batch("CREATE TRIGGER refuse_admit BEFORE INSERT ON app_command_receipts BEGIN SELECT RAISE(ABORT,'storage failed'); END").unwrap();
        assert!(store
            .admit_app_command(caller, "blocked", "sessions.start", &json!({}))
            .is_err());
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM app_command_receipts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        store
            .conn
            .execute_batch("DROP TRIGGER refuse_admit")
            .unwrap();
        store
            .admit_app_command(caller, "unknown", "sessions.start", &json!({}))
            .unwrap();
        store.conn.execute_batch("CREATE TRIGGER refuse_result BEFORE UPDATE ON app_command_receipts BEGIN SELECT RAISE(ABORT,'storage failed'); END").unwrap();
        assert!(store
            .finish_app_command(
                caller,
                "unknown",
                "sessions.start",
                &json!({}),
                &Ok(json!({"accepted":true}))
            )
            .is_err());
        assert!(store
            .admit_app_command(caller, "unknown", "sessions.start", &json!({}))
            .unwrap_err()
            .contains("unconfirmed"));
    }
    #[test]
    fn only_one_connection_can_admit_the_same_effect() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("db");
        let first = Store::open_vorkspace(&path).unwrap();
        let caller = first.create("codex", "/project").unwrap().id;
        let second = Store::open_vorkspace(&path).unwrap();
        assert_eq!(
            first
                .admit_app_command(caller, "once", "sessions.start", &json!({}))
                .unwrap(),
            AppCommandAdmission::Admitted
        );
        assert!(second
            .admit_app_command(caller, "once", "sessions.start", &json!({}))
            .unwrap_err()
            .contains("unconfirmed"));
        first
            .finish_app_command(caller, "once", "sessions.start", &json!({}), &Ok(json!(12)))
            .unwrap();
        assert_eq!(
            second
                .admit_app_command(caller, "once", "sessions.start", &json!({}))
                .unwrap(),
            AppCommandAdmission::Completed(Ok(json!(12)))
        );
    }
    #[test]
    fn cold_hidden_habit_channel_and_worker_targets_are_protected_at_the_mutation_owner() {
        use crate::bot::{
            channels::{Channel, Channels},
            Roster,
        };
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("db");
        let mut store = Store::open_vorkspace(&path).unwrap();
        let caller = store.create("codex", "/project").unwrap().id;
        let target = store.create("codex", "/project").unwrap().id;
        let outside = store.create("codex", "/outside").unwrap().id;
        assert!(store.check_app_target(caller, caller, "/project").is_err());
        assert!(store.check_app_target(caller, outside, "/project").is_err());
        let mut roster = Roster::default();
        roster.hidden_sessions = vec![target];
        roster.save(&store).unwrap();
        drop(store);
        store = Store::open_vorkspace(&path).unwrap();
        assert!(store
            .archive_app_session(caller, target, "/project")
            .is_err());
        assert!(store
            .delete_app_session(caller, target, "/project")
            .is_err());
        assert!(!store.bot_session(target).unwrap().unwrap().archived);
        roster.hidden_sessions.clear();
        let bot = roster.create(vec!["/project".into()]).id.clone();
        roster.get_mut(&bot).unwrap().habit_sessions.push(target);
        roster.save(&store).unwrap();
        assert!(store.check_app_target(caller, target, "/project").is_err());
        roster.get_mut(&bot).unwrap().habit_sessions.clear();
        roster.save(&store).unwrap();
        Channels {
            channels: vec![Channel {
                member_sessions: [(bot, target)].into(),
                ..Default::default()
            }],
        }
        .save(&store)
        .unwrap();
        assert!(store.check_app_target(caller, target, "/project").is_err());
        Channels::default().save(&store).unwrap();
        store
            .save_orchestration_summary(
                caller,
                &crate::orchestration::Summary {
                    tasks: vec![crate::orchestration::SummaryTask {
                        session: Some(target),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(store
            .delete_app_session(caller, target, "/project")
            .is_err());
        store
            .save_orchestration_summary(caller, &Default::default())
            .unwrap();
        store
            .archive_app_session(caller, target, "/project")
            .unwrap();
        assert!(store.bot_session(target).unwrap().unwrap().archived);
        store
            .delete_app_session(caller, target, "/project")
            .unwrap();
        assert!(store.bot_session(target).unwrap().is_none());
        assert!(store.bot_session(caller).unwrap().is_some());
    }
    #[test]
    fn ordinary_background_launches_remain_mutable_after_reopen() {
        for open in [
            Store::open as fn(&std::path::Path) -> Result<Store, String>,
            Store::open_vorkspace,
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("db");
            let store = open(&path).unwrap();
            let caller = store.create("codex", "/project").unwrap().id;
            let child = store.create("codex", "/project").unwrap().id;
            let internal = store.create("codex", "/project").unwrap().id;
            let mut roster = crate::bot::Roster::default();
            let bot = roster.create(vec!["/project".into()]).id.clone();
            let bot = roster.get_mut(&bot).unwrap();
            bot.session_id = Some(caller);
            bot.show_started_sessions_in_sidebar = false;
            bot.launches.push(crate::bot::Launch {
                session_id: child,
                cwd: "/project".into(),
                title: "Background work".into(),
                harness: "codex".into(),
                model: None,
                origin_turn: 0,
                notify_on_complete: false,
                completed: false,
                dismissed: false,
            });
            roster.hidden_sessions.push(internal);
            roster.save(&store).unwrap();
            if store.shared.is_some() {
                store
                    .shared_patch(child, &[("sidebar_hidden", json!(true))])
                    .unwrap();
            }
            drop(store);
            let mut store = open(&path).unwrap();
            assert!(store.check_app_target(caller, child, "/project").is_ok());
            assert!(store.check_app_target(caller, caller, "/project").is_err());
            assert!(store
                .check_app_target(caller, internal, "/project")
                .is_err());
            store
                .archive_app_session(caller, child, "/project")
                .unwrap();
            assert!(store.bot_session(child).unwrap().unwrap().archived);
            store.delete_app_session(caller, child, "/project").unwrap();
            assert!(store.bot_session(child).unwrap().is_none());
            assert!(store.bot_session(internal).unwrap().is_some());
        }
    }
    #[test]
    fn legacy_background_launch_identity_is_independent_of_sidebar_visibility() {
        let store = Store::open_vorkspace(std::path::Path::new(":memory:")).unwrap();
        let caller = store.create("codex", "/project").unwrap().id;
        let source = store.persisted_session_id(caller).unwrap();
        let mut roster = crate::bot::Roster::default();
        let bot = roster
            .create(vec!["/project".into(), "/assigned".into()])
            .id
            .clone();
        roster.get_mut(&bot).unwrap().session_id = Some(caller);
        roster.hidden_sessions.push(caller);
        roster.save(&store).unwrap();
        for (original, project, allowed) in [
            (format!("app-{source}-background-task"), "/project", true),
            (
                format!("app-{source}-other-assigned-task"),
                "/assigned",
                true,
            ),
            (format!("app-{source}-unrelated-task"), "/unassigned", false),
            ("unclassified-hidden".into(), "/project", false),
        ] {
            store.conn.execute("INSERT INTO sessions(id,cwd,harness,model,model_settings,runtime_mode,title,blocks_json,created_at,updated_at,sidebar_hidden) VALUES(?1,?2,'codex','codex:default','{}','supervised','Background','[]',1,1,1)",params![original,project]).unwrap();
            let target = store.resolve_session_id(&original).unwrap();
            assert_eq!(
                store.check_app_target(caller, target, project).is_ok(),
                allowed,
                "{original}"
            );
            if allowed {
                store.archive_app_session(caller, target, project).unwrap();
                assert!(store.bot_session(target).unwrap().unwrap().archived);
            }
        }
        let internal = format!("app-{source}-internal-chat");
        store.conn.execute("INSERT INTO sessions(id,cwd,harness,model,model_settings,runtime_mode,title,blocks_json,created_at,updated_at,sidebar_hidden) VALUES(?1,'/project','codex','codex:default','{}','supervised','Internal','[]',1,1,1)",[&internal]).unwrap();
        let internal = store.resolve_session_id(&internal).unwrap();
        let other = roster.create(vec!["/project".into()]).id.clone();
        roster.get_mut(&other).unwrap().session_id = Some(internal);
        roster.save(&store).unwrap();
        assert!(
            store
                .check_app_target(caller, internal, "/project")
                .is_err(),
            "an app-shaped child identity never overrides a real Bot identity"
        );
        assert!(store.check_app_target(caller, caller, "/project").is_err());
    }
    #[test]
    fn admission_does_not_commit_a_post_insert_corrupted_receipt() {
        let store = Store::open_vorkspace(std::path::Path::new(":memory:")).unwrap();
        let caller = store.create("codex", "/project").unwrap().id;
        ensure(&store.conn).unwrap();
        store.conn.execute_batch("CREATE TRIGGER corrupt_receipt AFTER INSERT ON app_command_receipts BEGIN UPDATE app_command_receipts SET fingerprint='different' WHERE caller_session_id=NEW.caller_session_id AND request_id=NEW.request_id; END").unwrap();
        assert!(store
            .admit_app_command(caller, "corrupt", "sessions.start", &json!({}))
            .is_err());
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM app_command_receipts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn canonical_object_order_reuses_the_same_fingerprint() {
        let a: Value = serde_json::from_str(r#"{"x":1,"nested":{"a":1,"b":2}}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"nested":{"b":2,"a":1},"x":1}"#).unwrap();
        assert_eq!(
            fingerprint("notes.write", &a),
            fingerprint("notes.write", &b)
        );
    }
}
