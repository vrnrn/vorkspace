//! Read-only bindings for the original desktop's remote shell tabs. Original
//! session IDs, metadata, block history, and WebKit preferences stay intact.
use super::{desktop_session_handle, RemoteSession, Session, Store};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

const HIDDEN: &str = "remote-hidden-legacy.v1";
const OWNERS: &str = "remote-legacy-command-owners.v1";

impl Store {
    pub(super) fn legacy_remote_sessions(&self) -> Result<BTreeMap<i64, RemoteSession>, String> {
        let sql = if self.shared.is_some() {
            "SELECT id FROM sessions WHERE cwd >= 'remote://' AND cwd < 'remote:/0'"
        } else {
            "SELECT CAST(id AS TEXT) FROM native_sessions WHERE cwd >= 'remote://' AND cwd < 'remote:/0'"
        };
        let mut query = self.conn.prepare(sql).map_err(|error| error.to_string())?;
        let originals: Vec<String> = query
            .query_map([], |row| row.get(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|error| error.to_string())?;
        let mut result = BTreeMap::new();
        for original in originals {
            let id = if self.shared.is_some() {
                desktop_session_handle(&original)
            } else {
                original.parse::<i64>().map_err(|error| error.to_string())?
            };
            if let Some(remote) = self.legacy_remote_session(id)? {
                result.insert(id, remote);
            }
        }
        Ok(result)
    }

    pub(super) fn legacy_remote_session(&self, id: i64) -> Result<Option<RemoteSession>, String> {
        if id < 0 {
            return Ok(None);
        }
        let Some(original) = self.legacy_remote_original_id(id) else {
            return Ok(None);
        };
        let Some(summary) = self.legacy_source_summary(id, &original)? else {
            return Ok(None);
        };
        let projects = self.remote_projects()?;
        let preferences = self.desktop_preferences();
        let tabs = preferences
            .and_then(|prefs| prefs.json("vorkspace.remote-tabs.v2"))
            .unwrap_or(json!({}));
        let Some(project) = projects
            .values()
            .find(|project| {
                crate::files::path_key(&project.key) == crate::files::path_key(&summary.cwd)
            })
            .cloned()
        else {
            return Ok(None);
        };
        let session_id = tabs[&original]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        let cached = session_id.as_ref().and_then(|id| {
            preferences?
                .json(&format!("vorkspace.remote-history.v2:{}", project.key))?
                .as_array()?
                .iter()
                .find(|item| item["id"].as_str() == Some(id))
                .cloned()
        });
        let mut remote = RemoteSession {
            summary,
            project,
            session_id,
            outbox: vec![],
            uploads: vec![],
            rejected: None,
            running: false,
            needs_input: false,
            configuration: None,
            host_revision: None,
            configuration_revision: None,
        };
        if let Some(cached) = cached {
            apply_cached(&mut remote, &cached);
        }
        if let Some(preferences) = preferences {
            let owners = self.legacy_command_owners()?;
            let mut ambiguous = Vec::new();
            let prefix = format!(
                "vorkspace.remote-command.v1:{}:",
                json!([remote.project.key, remote.project.environment_id])
            );
            let candidate_creates = preferences
                .entries()
                .filter(|(key, _)| key.starts_with(&prefix))
                .filter_map(|(key, raw)| Some((key, serde_json::from_str::<Value>(raw).ok()?)))
                .filter(|(key, entry)| {
                    let command = entry.get("command").unwrap_or(entry);
                    command["type"] == "create"
                        && command["commandId"].is_string()
                        && entry["shellId"]
                            .as_str()
                            .is_none_or(|shell| shell == original)
                        && owners.get(*key).is_none_or(|owner| *owner == id)
                })
                .count();
            for (key, raw) in preferences
                .entries()
                .filter(|(key, _)| key.starts_with(&prefix))
            {
                let Ok(entry) = serde_json::from_str::<Value>(raw) else {
                    continue;
                };
                let command = entry.get("command").unwrap_or(&entry);
                let matches = if command["type"] == "create" {
                    if remote.session_id.is_some() {
                        false
                    } else if let Some(shell) = entry["shellId"].as_str() {
                        shell == original
                    } else if let Some(owner) = owners.get(key) {
                        *owner == id
                    } else {
                        let candidates = self.legacy_unbound_tabs(&remote.project, &tabs)?;
                        let unique = candidate_creates == 1
                            && candidates.len() == 1
                            && candidates[0] == id
                            && command["projectId"].as_str()
                                == Some(remote.project.project_id.as_str());
                        if !unique && candidates.contains(&id) {
                            ambiguous.push(command.clone());
                            if let Some(followup) = entry.get("followup") {
                                ambiguous.push(followup.clone());
                            }
                        }
                        unique
                    }
                } else {
                    command["sessionId"].as_str() == remote.session_id.as_deref()
                        && remote.session_id.is_some()
                };
                if matches && command["commandId"].is_string() {
                    remote.outbox.push(command.clone());
                    if let Some(mut followup) = entry
                        .get("followup")
                        .filter(|value| value.is_object())
                        .cloned()
                    {
                        if followup["sessionId"] == "" {
                            followup["sessionId"] = Value::Null;
                        }
                        remote.outbox.push(followup);
                    }
                }
            }
            if !ambiguous.is_empty() {
                remote.rejected = Some(
                    json!({"legacyRecovery":true,"error":"This project has older pending requests that cannot be linked to one tab. Open the original desktop app to recover them. New requests and Retry are blocked here to avoid sending them twice.","commands":ambiguous}),
                );
            }
        }
        Ok(Some(remote))
    }

    fn legacy_remote_original_id(&self, id: i64) -> Option<String> {
        (id >= 0)
            .then(|| self.persisted_session_id(id).ok())
            .flatten()
    }

    fn legacy_command_owners(&self) -> Result<BTreeMap<String, i64>, String> {
        self.setting(OWNERS)?
            .map(|raw| serde_json::from_str(&raw).map_err(|error| error.to_string()))
            .unwrap_or_else(|| Ok(BTreeMap::new()))
    }

    fn legacy_unbound_tabs(
        &self,
        project: &super::RemoteProject,
        tabs: &Value,
    ) -> Result<Vec<i64>, String> {
        let sql = if self.shared.is_some() {
            "SELECT id,cwd FROM sessions WHERE cwd >= 'remote://' AND cwd < 'remote:/0'"
        } else {
            "SELECT CAST(id AS TEXT),cwd FROM native_sessions WHERE cwd >= 'remote://' AND cwd < 'remote:/0'"
        };
        let mut query = self.conn.prepare(sql).map_err(|error| error.to_string())?;
        let rows = query
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?;
        let stored: BTreeMap<i64, RemoteSession> = self
            .setting("remote-sessions.v1")?
            .map(|raw| serde_json::from_str(&raw).map_err(|error| error.to_string()))
            .transpose()?
            .unwrap_or_default();
        let hidden = self.hidden_legacy_remote_sessions()?;
        let mut result = Vec::new();
        for row in rows {
            let (original, cwd) = row.map_err(|error| error.to_string())?;
            let id = if self.shared.is_some() {
                desktop_session_handle(&original)
            } else {
                original
                    .parse()
                    .map_err(|error: std::num::ParseIntError| error.to_string())?
            };
            if crate::files::path_key(&cwd) == crate::files::path_key(&project.key)
                && tabs[&original].as_str().is_none_or(str::is_empty)
                && !hidden.contains(&id)
                && stored
                    .get(&id)
                    .is_none_or(|remote| remote.session_id.is_none())
            {
                result.push(id);
            }
        }
        Ok(result)
    }

    /// Record a deterministic legacy claim with the journal, so immutable old
    /// WebKit entries cannot later be adopted by another newly blank tab.
    pub(super) fn claim_legacy_remote_commands(
        &self,
        remote: &RemoteSession,
    ) -> Result<(), String> {
        let Some(preferences) = self.desktop_preferences() else {
            return Ok(());
        };
        if remote.outbox.is_empty() {
            return Ok(());
        }
        let prefix = format!(
            "vorkspace.remote-command.v1:{}:",
            json!([remote.project.key, remote.project.environment_id])
        );
        let mut owners = self.legacy_command_owners()?;
        let mut changed = false;
        for (key, raw) in preferences
            .entries()
            .filter(|(key, _)| key.starts_with(&prefix))
        {
            let Ok(entry) = serde_json::from_str::<Value>(raw) else {
                continue;
            };
            let command = entry.get("command").unwrap_or(&entry);
            if command["type"] == "create"
                && entry["shellId"].as_str().is_none()
                && remote
                    .outbox
                    .iter()
                    .any(|queued| queued["commandId"] == command["commandId"])
            {
                if owners
                    .get(key)
                    .is_some_and(|owner| *owner != remote.summary.id)
                {
                    return Err("This legacy request is already assigned to another tab".into());
                }
                changed |= owners.insert(key.into(), remote.summary.id) != Some(remote.summary.id);
            }
        }
        if changed {
            self.save_setting(
                OWNERS,
                &serde_json::to_string(&owners).map_err(|error| error.to_string())?,
            )?;
        }
        Ok(())
    }

    fn legacy_source_summary(&self, id: i64, original: &str) -> Result<Option<Session>, String> {
        let sql = if self.shared.is_some() {
            format!("SELECT title,harness,cwd,provider_session_id,updated_at,{},{},{},{} FROM sessions WHERE id=?1 AND cwd >= 'remote://' AND cwd < 'remote:/0'",self.shared_field("has_user_message","0"),self.shared_field("archived","0"),self.shared_field("pinned","0"),self.shared_field("linked_work_item_json","NULL"))
        } else {
            "SELECT title,provider,cwd,provider_id,updated_at,0,archived,pinned,work_item FROM native_sessions WHERE id=?1 AND cwd >= 'remote://' AND cwd < 'remote:/0'".into()
        };
        self.conn
            .query_row(&sql, [original], |row| {
                Ok(Session {
                    id,
                    title: row.get(0)?,
                    provider: row.get(1)?,
                    cwd: row.get(2)?,
                    provider_id: row.get(3)?,
                    updated_at: row.get::<_, i64>(4)?.max(0) as u64,
                    has_messages: row.get(5)?,
                    archived: row.get(6)?,
                    pinned: row.get(7)?,
                    work_item: row
                        .get::<_, Option<String>>(8)?
                        .and_then(|raw| serde_json::from_str(&raw).ok()),
                })
            })
            .optional()
            .map_err(|error| error.to_string())
    }

    /// Execution directory for a pre-existing remote tab, qualified for the
    /// host filesystem even when its old pending-worktree preference was raw.
    pub(crate) fn legacy_remote_worktree(&self, id: i64) -> Result<Option<String>, String> {
        let Some(remote) = self.legacy_remote_session(id)? else {
            return Ok(None);
        };
        let Some(preferences) = self.desktop_preferences() else {
            return Ok(None);
        };
        let Some(original) = self.legacy_remote_original_id(id) else {
            return Ok(None);
        };
        let cached = remote.session_id.as_ref().and_then(|host| {
            preferences
                .json(&format!(
                    "vorkspace.remote-history.v2:{}",
                    remote.project.key
                ))?
                .as_array()?
                .iter()
                .find(|item| item["id"].as_str() == Some(host))
                .cloned()
        });
        let pending = preferences
            .json("vorkspace.remote-pending-worktrees.v1")
            .and_then(|all| all[&original].as_str().map(str::to_owned));
        let cwd = cached
            .as_ref()
            .and_then(|value| value["cwd"].as_str())
            .map(str::to_owned)
            .or(pending);
        Ok(cwd
            .filter(|cwd| cwd != &remote.project.cwd)
            .and_then(|cwd| {
                if let Some(path) = crate::remote::parse_remote_path(&cwd) {
                    (path.environment_id == remote.project.environment_id).then_some(cwd)
                } else {
                    Some(crate::remote::remote_path(
                        &remote.project.environment_id,
                        &cwd,
                    ))
                }
            }))
    }

    pub(super) fn hidden_legacy_remote_sessions(&self) -> Result<HashSet<i64>, String> {
        self.setting(HIDDEN)?
            .map(|raw| serde_json::from_str(&raw).map_err(|error| error.to_string()))
            .unwrap_or_else(|| Ok(HashSet::new()))
    }
    pub(super) fn hide_legacy_remote_sessions(
        &self,
        ids: impl IntoIterator<Item = i64>,
    ) -> Result<(), String> {
        let mut hidden = self.hidden_legacy_remote_sessions()?;
        hidden.extend(ids.into_iter().filter(|id| *id >= 0));
        self.save_setting(
            HIDDEN,
            &serde_json::to_string(&hidden).map_err(|error| error.to_string())?,
        )
    }
}

fn apply_cached(remote: &mut RemoteSession, cached: &Value) {
    let summary = &mut remote.summary;
    if let Some(title) = cached["title"].as_str() {
        summary.title = title.into();
    }
    if let Some(provider) = cached["harness"].as_str() {
        summary.provider = provider.into();
    }
    summary.provider_id = cached["providerSessionId"]
        .as_str()
        .map(str::to_owned)
        .or(summary.provider_id.clone());
    summary.has_messages = true;
    summary.updated_at = cached["updatedAt"].as_u64().unwrap_or(summary.updated_at);
    summary.archived = cached["archived"].as_bool().unwrap_or(summary.archived);
    summary.pinned = cached["pinned"].as_bool().unwrap_or(summary.pinned);
    remote.running = cached["status"] == "running";
    remote.needs_input = cached["needsInput"] == true;
}

#[cfg(test)]
mod tests;
