//! Where Bot state meets the store. The Bot types stay below it, so these live here.
use super::Store;
use crate::bot::{
    launch::{CreatedWorktree, Launch, Workspace},
    merge_roster_value, new_id, overlay_roster_value, Bot, Roster,
};
use serde_json::Value;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

impl Roster {
    pub fn load(store: &Store) -> Result<Self, String> {
        let raw = store.setting("bot-roster")?;
        let mut roster: Self = raw
            .as_ref()
            .map(|text| {
                serde_json::from_str(text).map_err(|e| format!("Cannot read Bot roster: {e}"))
            })
            .transpose()?
            .unwrap_or_default();
        if raw.is_some() {
            *roster.storage_baseline.lock().map_err(|e| e.to_string())? =
                Some(serde_json::to_string(&roster).map_err(|e| e.to_string())?);
        }
        if store.uses_vorkspace_schema() {
            if let Some(preferences) = store.desktop_preferences() {
                roster.reconcile_desktop(store, preferences)?;
            }
        }
        let mut ids = HashSet::new();
        roster.bots.retain(|m| {
            !m.id.is_empty()
                && m.id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                && ids.insert(m.id.clone())
        });
        for bot in &mut roster.bots {
            bot.normalize();
        }
        roster.migrate_minions();
        roster.sanitize_team_queues();
        Ok(roster)
    }
    pub fn save(&self, store: &Store) -> Result<(), String> {
        self.save_snapshot(store).map(|_| ())
    }

    /// Returns the roster actually committed, including concurrent edits from
    /// other writers. The caller's queued snapshots keep their local baseline.
    pub fn save_snapshot(&self, store: &Store) -> Result<Self, String> {
        let value = serde_json::to_string(self).map_err(|e| e.to_string())?;
        let mut baseline = self.storage_baseline.lock().map_err(|e| e.to_string())?;
        let empty = serde_json::to_value(Roster::default()).map_err(|e| e.to_string())?;
        let base: serde_json::Value = baseline
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or(empty.clone());
        let local: serde_json::Value = serde_json::from_str(&value).map_err(|e| e.to_string())?;
        for _ in 0..8 {
            let current = store.setting("bot-roster")?;
            let raw: serde_json::Value = current
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|e| e.to_string())?
                .unwrap_or(empty.clone());
            let remote: serde_json::Value = if current.is_some() {
                let parsed: Roster = serde_json::from_value(raw.clone())
                    .map_err(|e| format!("Cannot read Bot roster: {e}"))?;
                serde_json::to_value(parsed).map_err(|e| e.to_string())?
            } else {
                empty.clone()
            };
            let merged = merge_roster_value(Some(&base), Some(&local), Some(&remote), "roster")?
                .ok_or("Cannot remove the Bot roster")?;
            let snapshot = Self::with_baseline(merged.clone())?;
            let merged = overlay_roster_value(Some(&remote), &merged, Some(&raw), "roster");
            let merged = serde_json::to_string(&merged).map_err(|e| e.to_string())?;
            match store.compare_swap_setting("bot-roster", current.as_deref(), &merged) {
                Ok(()) => {
                    *baseline = Some(value);
                    return Ok(snapshot);
                }
                Err(error) if error.contains("another window") => continue,
                Err(error) => return Err(error),
            }
        }
        Err("Bot roster is being changed in another window; your changes were preserved in this window.".into())
    }

    fn with_baseline(value: Value) -> Result<Self, String> {
        let baseline = serde_json::to_string(&value).map_err(|e| e.to_string())?;
        let roster: Self = serde_json::from_value(value).map_err(|e| e.to_string())?;
        *roster.storage_baseline.lock().map_err(|e| e.to_string())? = Some(baseline);
        Ok(roster)
    }

    /// Start a new editor with its own save lineage. Ordinary clones keep the
    /// same lineage for the editor's ordered in-flight saves.
    pub fn fork(&self) -> Self {
        let mut roster = self.clone();
        roster.storage_baseline = std::sync::Arc::new(std::sync::Mutex::new(
            self.storage_baseline.lock().unwrap().clone(),
        ));
        roster
    }

    /// Incorporates committed changes while preserving this editor's local
    /// edits. Use this after all saves of this lineage have completed.
    pub fn rebase_onto(&mut self, committed: &Self) -> Result<bool, String> {
        let local = serde_json::to_value(&*self).map_err(|e| e.to_string())?;
        let remote = serde_json::to_value(committed).map_err(|e| e.to_string())?;
        let base = self
            .storage_baseline
            .lock()
            .map_err(|e| e.to_string())?
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or(serde_json::to_value(Self::default()).map_err(|e| e.to_string())?);
        let merged = merge_roster_value(Some(&base), Some(&local), Some(&remote), "roster")?
            .ok_or("Cannot remove the Bot roster")?;
        let changed = merged != local;
        if changed {
            *self = serde_json::from_value(merged).map_err(|e| e.to_string())?;
        }
        *self.storage_baseline.lock().map_err(|e| e.to_string())? =
            Some(serde_json::to_string(&remote).map_err(|e| e.to_string())?);
        Ok(changed)
    }
}

impl Launch {
    pub fn create(
        &mut self,
        store: &mut crate::store::Store,
        project: &str,
    ) -> Result<crate::session::Session, String> {
        self.create_configured(store, project, None)
    }
    /// Keep preparation under one owner until its parent link is durable. A
    /// failed link cannot escape with resources no UI installation will own.
    pub fn create_started(
        &mut self,
        store: &mut crate::store::Store,
        project: &str,
        caller: i64,
    ) -> Result<crate::session::Session, String> {
        self.create_configured(store, project, Some(caller))
    }
    fn create_configured(
        &mut self,
        store: &mut crate::store::Store,
        project: &str,
        caller: Option<i64>,
    ) -> Result<crate::session::Session, String> {
        self.validate_workspace(project)?;
        let mut summary = store.create(self.provider.id(), project)?;
        let configured = (|| {
            self.create_workspace(project, summary.id)?;
            if matches!(self.workspace, Workspace::New(_)) {
                self.created_worktree = Some(CreatedWorktree::capture(
                    project,
                    self.settings
                        .worktree_cwd
                        .as_deref()
                        .ok_or("Created worktree path is unavailable")?,
                )?);
            }
            store.title(summary.id, &self.title)?;
            store.configure_agent(summary.id, self.provider.id(), &self.settings)?;
            if let Some(caller) = caller {
                store.save_started_link(summary.id, caller)?;
            }
            Ok::<_, String>(())
        })();
        if let Err(error) = configured {
            // Remove durable ownership before its working copy. If deletion
            // fails, preserve both and report the retained resource explicitly.
            let cleanup = match store.delete(summary.id) {
                Ok(()) => self.created_worktree.take().map(CreatedWorktree::rollback).transpose().err(),
                Err(error) => Some(format!(
                    "Unused session {} could not be removed; its working copy{} was preserved: {error}",
                    summary.id,
                    self.created_worktree.as_ref().map(|tree|format!(" {}",tree.path)).unwrap_or_default(),
                )),
            };
            return Err(format!(
                "Launch was not accepted: {error}{}",
                cleanup
                    .map(|error| format!(". {error}"))
                    .unwrap_or_default(),
            ));
        }
        summary.title = self.title.clone();
        Ok(summary)
    }
}

impl Bot {
    /// Refresh references without discarding native-created documents or launch
    /// monitoring state. Removed desktop references disappear on the next read.
    pub fn refresh_work(&mut self, store: &crate::store::Store) -> Result<bool, String> {
        let Some(session) = self.session_id else {
            return Ok(false);
        };
        let Some((documents, launches)) = store.bot_work(session)? else {
            return Ok(false);
        };
        Ok(self.apply_work(documents, launches))
    }
}

pub fn root(store: &Store) -> PathBuf {
    store
        .data_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("vorkspace-test-{}", new_id())))
        .join("bots")
}

impl Store {
    /// Unarchived sessions working in `path`.
    pub fn sessions_using(&self, path: &str) -> Result<Vec<i64>, String> {
        self.sessions_using_path(path, crate::files::is_equal_or_inside)
    }

    pub fn sessions_using_resolved(&self, path: &str) -> Result<Vec<i64>, String> {
        self.sessions_using_path(path, crate::files::is_resolved_inside)
    }

    fn sessions_using_path(
        &self,
        path: &str,
        within: fn(&str, &str) -> bool,
    ) -> Result<Vec<i64>, String> {
        let worktrees = self.session_worktrees()?;
        Ok(self
            .sessions()?
            .into_iter()
            .filter(|session| !session.archived)
            .filter(|session| {
                let execution = worktrees
                    .get(&session.id)
                    .and_then(Option::as_deref)
                    .unwrap_or(&session.cwd);
                within(execution, path)
            })
            .map(|session| session.id)
            .collect())
    }
}

/// Git runs without the database lock, which is held per session lookup.
pub fn describe_worktrees(
    project: &std::path::Path,
    store: &std::sync::Mutex<Store>,
) -> Result<Value, String> {
    crate::worktree_management::describe(project, |path| {
        store
            .lock()
            .map_err(|_| "The session store is unavailable.".to_owned())?
            .sessions_using(path)
    })
}
