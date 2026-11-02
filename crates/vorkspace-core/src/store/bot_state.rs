//! Where Bot state meets the store. The Bot types stay below it, so these live here.
use super::Store;
use crate::bot::{
    channels::{Channel, Channels, KEY},
    launch::{CreatedWorktree, Launch, Workspace},
    merge_roster_value, new_id, overlay_roster_value, Bot, Roster,
};
use serde_json::{json, Value};
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
            let merged = overlay_roster_value(Some(&remote), &merged, Some(&raw), "roster");
            let merged = serde_json::to_string(&merged).map_err(|e| e.to_string())?;
            match store.compare_swap_setting("bot-roster", current.as_deref(), &merged) {
                Ok(()) => {
                    *baseline = Some(value);
                    return Ok(());
                }
                Err(error) if error.contains("another window") => continue,
                Err(error) => return Err(error),
            }
        }
        Err("Bot roster is being changed in another window; your changes were preserved in this window.".into())
    }
}

impl Channels {
    pub fn load(store: &Store) -> Result<Self, String> {
        if let Some(raw) = store.setting(KEY)? {
            return serde_json::from_str(&raw)
                .map_err(|e| format!("Cannot read Bot Channels: {e}"));
        }
        let mut result = Self::default();
        if let Some(prefs) = store.desktop_preferences() {
            if let Some(raw) = prefs.get("vorkspace:bot-channels") {
                let rows: Vec<Value> = serde_json::from_str(raw)
                    .map_err(|e| format!("Cannot read desktop Channels: {e}"))?;
                for mut row in rows {
                    let sessions = row
                        .as_object_mut()
                        .ok_or("Invalid channel")?
                        .remove("memberSessions")
                        .unwrap_or(json!({}));
                    if row.get("memberIds").is_none() {
                        row["memberIds"] = row["botIds"].clone();
                    }
                    let mut channel: Channel =
                        serde_json::from_value(row).map_err(|e| e.to_string())?;
                    if let Some(raw) =
                        prefs.get(&format!("vorkspace:bot-channel-messages:{}", channel.id))
                    {
                        channel.messages = serde_json::from_str(raw)
                            .map_err(|e| format!("Cannot read channel messages: {e}"))?;
                    }
                    // Preserve the original references even if a backing session
                    // is not currently resolvable. Do not manufacture history.
                    if let Some(sessions) = sessions.as_object() {
                        for (bot, id) in sessions {
                            let id = id.as_str().ok_or("Invalid channel session")?;
                            let native_id = store.resolve_session_id(id).map_err(|_| format!("Channel {} has an unavailable backing session for {bot}; its history was preserved", channel.name))?;
                            channel.member_sessions.insert(bot.clone(), native_id);
                        }
                    }
                    result.channels.push(channel);
                }
            }
        }
        Ok(result)
    }
    pub fn save(&self, store: &Store) -> Result<(), String> {
        store.save_setting(
            KEY,
            &serde_json::to_string(self).map_err(|e| e.to_string())?,
        )
    }
    pub fn save_with_draft(&self, store: &Store, key: &str, draft: &str) -> Result<(), String> {
        let state = serde_json::to_string(self).map_err(|e| e.to_string())?;
        store.save_settings(&[(KEY, &state), (key, draft)])
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
        self.apply_work(documents, launches)
    }
}

pub fn root(store: &Store) -> PathBuf {
    store
        .data_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("vorkspace-test-{}", new_id())))
        .join("bots")
}

impl Launch {
    /// Runs outside the database lock, because setup can take minutes.
    pub fn prepare_worktree(&self, project: &str) -> Option<String> {
        if !matches!(self.workspace, Workspace::New(_)) {
            return None;
        }
        let worktree = self.settings.worktree_cwd.as_deref()?;
        crate::worktree_management::prepare(
            std::path::Path::new(project),
            std::path::Path::new(worktree),
            &Default::default(),
        )
    }
}

impl Store {
    /// Unarchived sessions working in `path`.
    pub fn sessions_using(&self, path: &str) -> Result<Vec<i64>, String> {
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
                crate::files::is_equal_or_inside(execution, path)
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

/// Remove for an agent: nobody is working there and nothing would be lost. The
/// database lock is released while Git runs.
pub fn remove_unused_worktree(
    project: &std::path::Path,
    path: &str,
    store: &std::sync::Mutex<Store>,
) -> Result<(), String> {
    let lock = || {
        store
            .lock()
            .map_err(|_| "The session store is unavailable.".to_owned())
    };
    if crate::files::remote::is_remote(project) {
        return Err("Remote worktrees are removed from Settings.".into());
    }
    let sessions = lock()?.sessions_using(path)?;
    if !sessions.is_empty() {
        return Err(format!(
            "Archive the {} sessions using this worktree first.",
            sessions.len()
        ));
    }
    let main = crate::git::snapshot(project)?;
    let main = main.worktrees.first().ok_or("No main working copy found")?;
    crate::worktree_management::remove_if_safe(
        std::path::Path::new(&main.path),
        std::path::Path::new(path),
    )?;
    lock()?.detach_worktree(path, &main.path).map(|_| ())
}
