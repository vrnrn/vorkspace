//! Host-owned sessions keep only their desktop identity and command receipts here.
//! Transcripts remain on the host and use the same lossless block projection as
//! desktop conversations; no remote row is inserted into the local session table.
use super::{shared, Session, Store};
use crate::{
    agent::{Settings, TurnInfo},
    remote::{Connections, Machine},
    transcript::Transcript,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

const PROJECTS: &str = "remote-projects.v2";
const SESSIONS: &str = "remote-sessions.v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteProject {
    pub key: String,
    pub environment_id: String,
    pub project_id: String,
    pub cwd: String,
}

/// Local bytes needed to retry the same journaled attachment references.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteUpload {
    pub command_id: String,
    pub id: String,
    pub path: std::path::PathBuf,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteSession {
    pub summary: Session,
    pub project: RemoteProject,
    pub session_id: Option<String>,
    /// Recorded before dispatch, and removed only after a definite receipt.
    #[serde(default)]
    pub outbox: Vec<Value>,
    #[serde(default)]
    pub uploads: Vec<RemoteUpload>,
    /// Definite host rejections retain the original text for explicit recovery.
    #[serde(default)]
    pub rejected: Option<Value>,
    /// Last full host snapshot, used to reconcile changes from other desktops.
    #[serde(default)]
    pub configuration: Option<Value>,
    /// Newest revision observed from either a host listing or a full snapshot.
    /// None means the host or saved binding did not provide a revision.
    #[serde(default)]
    pub host_revision: Option<u64>,
    /// Listings omit model controls. A full snapshot at their same revision
    /// may still hydrate those controls without accepting older host state.
    #[serde(default)]
    pub configuration_revision: Option<u64>,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub needs_input: bool,
}
impl RemoteSession {
    pub fn machine(&self, client: &Connections) -> Result<Machine, String> {
        client
            .list()?
            .into_iter()
            .find(|machine| machine.environment_id == self.project.environment_id)
            .ok_or_else(|| {
                "Machine is no longer connected. Reconnect in Settings → Connections.".into()
            })
    }
}
fn host_settings(project: &RemoteProject, host: &Value) -> Settings {
    let mut settings = Settings::default();
    settings.model = host["model"].as_str().map(str::to_owned);
    settings.access =
        crate::agent::AccessMode::from_id(host["runtimeMode"].as_str().unwrap_or("supervised"));
    settings.model_settings = host
        .get("modelSettings")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    settings.effort = settings
        .model_settings
        .get("reasoningEffort")
        .or_else(|| settings.model_settings.get("effort"))
        .cloned();
    settings.fast = settings
        .model_settings
        .get("fast")
        .is_some_and(|value| value == "true");
    settings.service_tier = settings.model_settings.get("serviceTier").cloned();
    settings.thinking = settings
        .model_settings
        .get("thinking")
        .and_then(|value| value.parse::<bool>().ok());
    if let Some(cwd) = host["cwd"].as_str().filter(|cwd| *cwd != project.cwd) {
        settings.worktree_cwd = Some(crate::remote::remote_path(&project.environment_id, cwd));
    }
    settings
}

fn adopt_host_configuration(settings: &mut Settings, host: Settings) {
    settings.model = host.model;
    settings.access = host.access;
    settings.model_settings = host.model_settings;
    settings.effort = host.effort;
    settings.fast = host.fast;
    settings.service_tier = host.service_tier;
    settings.thinking = host.thinking;
    settings.worktree_cwd = host.worktree_cwd;
    settings.revision = settings.revision.saturating_add(1);
}

impl Store {
    pub fn remote_projects(&self) -> Result<BTreeMap<String, RemoteProject>, String> {
        let mut projects: BTreeMap<String, RemoteProject> = self
            .desktop_preferences()
            .and_then(|prefs| prefs.get("vorkspace.remote-projects.v2"))
            .map(|raw| {
                serde_json::from_str(raw)
                    .map_err(|e| format!("Cannot read desktop remote projects: {e}"))
            })
            .unwrap_or_else(|| Ok(BTreeMap::new()))?;
        if let Some(raw) = self.setting(PROJECTS)? {
            projects.extend(
                serde_json::from_str::<BTreeMap<String, RemoteProject>>(&raw)
                    .map_err(|e| format!("Cannot read remote projects: {e}"))?,
            );
        }
        Ok(projects)
    }
    pub fn remember_remote_project(
        &self,
        environment_id: &str,
        project: &Value,
    ) -> Result<RemoteProject, String> {
        let cwd = project["cwd"]
            .as_str()
            .ok_or("Host project has no directory")?;
        let id = project["id"]
            .as_str()
            .ok_or("Host project has no identity")?;
        let remote = RemoteProject {
            key: crate::remote::remote_path(
                environment_id,
                cwd.replace('\\', "/").trim_end_matches('/'),
            ),
            environment_id: environment_id.into(),
            project_id: id.into(),
            cwd: cwd.into(),
        };
        let mut projects = self.remote_projects()?;
        projects.insert(remote.key.clone(), remote.clone());
        self.save_setting(
            PROJECTS,
            &serde_json::to_string(&projects).map_err(|e| e.to_string())?,
        )?;
        Ok(remote)
    }
    fn stored_remote_sessions(&self) -> Result<BTreeMap<i64, RemoteSession>, String> {
        self.setting(SESSIONS)?
            .map(|raw| {
                serde_json::from_str(&raw).map_err(|e| format!("Cannot read remote sessions: {e}"))
            })
            .unwrap_or_else(|| Ok(BTreeMap::new()))
    }
    fn remote_sessions(&self) -> Result<BTreeMap<i64, RemoteSession>, String> {
        let hidden = self.hidden_legacy_remote_sessions()?;
        let mut sessions = self.legacy_remote_sessions()?;
        sessions.retain(|id, _| !hidden.contains(id));
        for (id, mut saved) in self.stored_remote_sessions()? {
            let canonical = saved
                .session_id
                .as_ref()
                .and_then(|host| {
                    sessions.values().find(|legacy| {
                        legacy.summary.id >= 0
                            && legacy.project.environment_id == saved.project.environment_id
                            && legacy.session_id.as_ref() == Some(host)
                    })
                })
                .map(|legacy| legacy.summary.id)
                .unwrap_or(id);
            saved.summary.id = canonical;
            sessions.insert(canonical, saved);
        }
        Ok(sessions)
    }
    pub fn remote_session(&self, id: i64) -> Result<Option<RemoteSession>, String> {
        let mut stored = self.stored_remote_sessions()?;
        if let Some(remote) = stored.remove(&id) {
            if remote
                .rejected
                .as_ref()
                .is_some_and(|recovery| recovery["legacyRecovery"] == true)
            {
                // Re-check immutable source preferences on a subsequent store
                // open after the original app has completed recovery.
                if let Some(legacy) = self.legacy_remote_session(id)? {
                    return Ok(Some(legacy));
                }
            }
            return Ok(Some(remote));
        }
        if id >= 0 && !self.hidden_legacy_remote_sessions()?.contains(&id) {
            if let Some(legacy) = self.legacy_remote_session(id)? {
                if let Some(mut saved) = legacy
                    .session_id
                    .as_ref()
                    .and_then(|host| {
                        stored.values().find(|saved| {
                            saved.project.environment_id == legacy.project.environment_id
                                && saved.session_id.as_ref() == Some(host)
                        })
                    })
                    .cloned()
                {
                    saved.summary.id = id;
                    return Ok(Some(saved));
                }
                return Ok(Some(legacy));
            }
        }
        Ok(None)
    }
    pub fn save_remote_session(&self, session: &RemoteSession) -> Result<(), String> {
        self.claim_legacy_remote_commands(session)?;
        let mut sessions = self.stored_remote_sessions()?;
        if session.summary.id >= 0 && session.session_id.is_some() {
            sessions.retain(|id, existing| {
                *id == session.summary.id
                    || existing.project.environment_id != session.project.environment_id
                    || existing.session_id != session.session_id
            });
        }
        let mut next = session.clone();
        if let Some(current) = sessions.get(&session.summary.id) {
            if current.session_id == next.session_id
                && current.project.environment_id == next.project.environment_id
            {
                // The command journal owns outbox/receipt transitions, while
                // host snapshots own versioned metadata. A slow receipt must
                // not restore the worker's older view of that metadata.
                if current.host_revision.is_some() && next.host_revision < current.host_revision {
                    next.host_revision = current.host_revision;
                    next.summary = current.summary.clone();
                    next.running = current.running;
                    next.needs_input = current.needs_input;
                }
                if current.configuration_revision.is_some()
                    && next.configuration_revision < current.configuration_revision
                {
                    next.configuration_revision = current.configuration_revision;
                    next.configuration = current.configuration.clone();
                }
            }
        }
        sessions.insert(session.summary.id, next);
        self.save_setting(
            SESSIONS,
            &serde_json::to_string(&sessions).map_err(|e| e.to_string())?,
        )
    }
    pub fn sessions(&self) -> Result<Vec<Session>, String> {
        let hidden = self.hidden_legacy_remote_sessions()?;
        let mut sessions: BTreeMap<_, _> = self
            .local_sessions()?
            .into_iter()
            .filter(|summary| !hidden.contains(&summary.id))
            .map(|summary| (summary.id, summary))
            .collect();
        sessions.extend(
            self.remote_sessions()?
                .into_iter()
                .map(|(id, remote)| (id, remote.summary)),
        );
        let mut sessions: Vec<_> = sessions.into_values().collect();
        sessions.sort_by_key(|s| (std::cmp::Reverse(s.updated_at), std::cmp::Reverse(s.id)));
        Ok(sessions)
    }
    pub fn create(&self, provider: &str, cwd: &str) -> Result<Session, String> {
        self.check_project_creation(cwd)?;
        if !cwd.starts_with("remote://") {
            return self.local_create(provider, cwd);
        }
        let kind = crate::provider::Kind::parse(provider).ok_or("Unknown remote provider")?;
        let project = self
            .remote_projects()?
            .into_values()
            .find(|project| {
                project.key.trim_end_matches('/') == cwd.trim_end_matches('/')
                    || crate::remote::remote_path(
                        &project.environment_id,
                        project.cwd.replace('\\', "/").trim_end_matches('/'),
                    )
                    .trim_end_matches('/')
                        == cwd.trim_end_matches('/')
            })
            .ok_or("Open this folder on its machine again to restore its project identity")?;
        let id = -shared::handle(&format!("remote:blank:{}", uuid::Uuid::new_v4()));
        let summary = Session {
            id,
            title: format!("New {} session", kind.label()),
            provider: provider.into(),
            cwd: project.key.clone(),
            provider_id: None,
            has_messages: false,
            updated_at: crate::time::now_ms(),
            archived: false,
            pinned: false,
            work_item: None,
        };
        self.save_remote_session(&RemoteSession {
            summary: summary.clone(),
            project,
            session_id: None,
            outbox: vec![],
            uploads: vec![],
            rejected: None,
            configuration: None,
            host_revision: None,
            configuration_revision: None,
            running: false,
            needs_input: false,
        })?;
        Ok(summary)
    }
    /// Reconcile only after a complete, successful host session listing. Draft
    /// desktop handles and commands awaiting receipts survive external deletion.
    pub fn prune_remote_sessions(
        &self,
        project: &RemoteProject,
        live_host_ids: &std::collections::HashSet<String>,
    ) -> Result<(), String> {
        let mut sessions = self.remote_sessions()?;
        let before: std::collections::HashSet<_> = sessions.keys().copied().collect();
        sessions.retain(|_, remote| {
            remote.project.environment_id != project.environment_id
                || remote.project.project_id != project.project_id
                || remote
                    .session_id
                    .as_ref()
                    .is_none_or(|id| live_host_ids.contains(id))
                || !remote.outbox.is_empty()
                || !remote.uploads.is_empty()
        });
        self.hide_legacy_remote_sessions(
            before.into_iter().filter(|id| !sessions.contains_key(id)),
        )?;
        self.save_setting(
            SESSIONS,
            &serde_json::to_string(&sessions).map_err(|e| e.to_string())?,
        )
    }

    pub fn register_remote_summary(
        &self,
        project: &RemoteProject,
        value: &Value,
    ) -> Result<Session, String> {
        let host = value.get("session").unwrap_or(value);
        let host_id = host["id"]
            .as_str()
            .ok_or("Host session identity is missing")?;
        let mut sessions = self.remote_sessions()?;
        let id = sessions
            .values()
            .find(|r| {
                r.project.environment_id == project.environment_id
                    && r.session_id.as_deref() == Some(host_id)
            })
            .map_or_else(
                || -shared::handle(&format!("remote:{}:{host_id}", project.environment_id)),
                |r| r.summary.id,
            );
        let prior = sessions.remove(&id);
        let full_snapshot = value.get("session").is_some_and(Value::is_object);
        let revision = value["revision"].as_u64();
        // Unknown versions remain supported for older hosts, but cannot
        // supersede state after this binding has observed an ordered revision.
        if revision.is_none()
            && prior
                .as_ref()
                .is_some_and(|remote| remote.host_revision.is_some())
        {
            return Ok(prior.unwrap().summary);
        }
        if let (Some(prior), Some(revision)) = (&prior, revision) {
            if prior.host_revision.is_some_and(|known| revision < known)
                || (prior.host_revision == Some(revision)
                    && (!full_snapshot || prior.configuration_revision == Some(revision)))
            {
                return Ok(prior.summary.clone());
            }
        }
        let previous = prior.as_ref().map(|remote| &remote.summary);
        let summary = Session {
            id,
            title: host["title"]
                .as_str()
                .or_else(|| previous.map(|summary| summary.title.as_str()))
                .unwrap_or("Remote session")
                .into(),
            provider: host["harness"]
                .as_str()
                .or_else(|| previous.map(|summary| summary.provider.as_str()))
                .unwrap_or("codex")
                .into(),
            cwd: project.key.clone(),
            provider_id: host.get("providerSessionId").map_or_else(
                || previous.and_then(|summary| summary.provider_id.clone()),
                |value| value.as_str().map(str::to_owned),
            ),
            has_messages: host.get("blocks").and_then(Value::as_array).map_or_else(
                || previous.is_none_or(|summary| summary.has_messages),
                |b| !b.is_empty(),
            ),
            updated_at: value["updatedAt"]
                .as_u64()
                .or_else(|| previous.map(|summary| summary.updated_at))
                .unwrap_or(0),
            archived: value["archived"]
                .as_bool()
                .or_else(|| previous.map(|summary| summary.archived))
                .unwrap_or(false),
            pinned: value["pinned"]
                .as_bool()
                .or_else(|| previous.map(|summary| summary.pinned))
                .unwrap_or(false),
            work_item: host.get("linkedWorkItem").map_or_else(
                || previous.and_then(|summary| summary.work_item.clone()),
                |value| serde_json::from_value(value.clone()).ok(),
            ),
        };
        let imported = host_settings(project, host);
        let configuration = if full_snapshot {
            Some(crate::remote::protocol::settings_configuration(
                &summary.provider,
                &imported,
            ))
        } else {
            prior
                .as_ref()
                .and_then(|remote| remote.configuration.clone())
        };
        let existing = self.agent_settings(id)?;
        let mut settings_to_save = None;
        match existing {
            None => settings_to_save = Some(imported),
            Some(mut settings) if full_snapshot => {
                let pending = prior
                    .as_ref()
                    .is_some_and(|remote| !remote.outbox.is_empty() || !remote.uploads.is_empty());
                let local =
                    crate::remote::protocol::settings_configuration(&summary.provider, &settings);
                let unchanged = prior
                    .as_ref()
                    .and_then(|remote| remote.configuration.as_ref())
                    .map_or(settings.revision == 0, |baseline| baseline == &local);
                if !pending
                    && unchanged
                    && (configuration.as_ref() != Some(&local)
                        || settings.worktree_cwd != imported.worktree_cwd)
                {
                    adopt_host_configuration(&mut settings, imported);
                    settings_to_save = Some(settings);
                }
            }
            _ => {}
        }
        if let Some(settings) = settings_to_save {
            self.save_agent_settings(id, &settings)?;
        }
        let remote = RemoteSession {
            summary: summary.clone(),
            project: project.clone(),
            session_id: Some(host_id.into()),
            outbox: prior.as_ref().map(|r| r.outbox.clone()).unwrap_or_default(),
            uploads: prior
                .as_ref()
                .map(|r| r.uploads.clone())
                .unwrap_or_default(),
            rejected: prior.as_ref().and_then(|remote| remote.rejected.clone()),
            configuration,
            host_revision: revision
                .or_else(|| prior.as_ref().and_then(|remote| remote.host_revision)),
            configuration_revision: if full_snapshot {
                revision
            } else {
                prior
                    .as_ref()
                    .and_then(|remote| remote.configuration_revision)
            },
            running: value["status"].as_str().map_or_else(
                || prior.as_ref().is_some_and(|remote| remote.running),
                |status| status == "running",
            ),
            needs_input: if host.get("needsInput").is_some()
                || host.get("pendingQuestion").is_some()
                || host.get("blocks").is_some()
            {
                host["needsInput"] == true
                    || host["pendingQuestion"].is_object()
                    || host["blocks"].as_array().is_some_and(|blocks| {
                        blocks.iter().any(|b| {
                            b["approval"].is_object()
                                && b["approval"].get("decided").is_none_or(Value::is_null)
                        })
                    })
            } else {
                prior.as_ref().is_some_and(|remote| remote.needs_input)
            },
        };
        sessions.insert(id, remote);
        self.save_setting(
            SESSIONS,
            &serde_json::to_string(&sessions).map_err(|e| e.to_string())?,
        )?;
        Ok(summary)
    }
    pub fn project_remote_history(
        snapshot: &Value,
    ) -> Result<(Transcript, HashMap<usize, TurnInfo>), String> {
        let blocks = snapshot["session"]["blocks"]
            .as_array()
            .ok_or("Host transcript is missing")?;
        let blocks: Vec<_> = blocks
            .iter()
            .cloned()
            .map(|mut block| {
                if block["role"] == "plan" {
                    block["nativeRemotePlan"] = Value::Bool(true);
                }
                block
            })
            .collect();
        Ok((
            Transcript {
                rows: shared::blocks::project(&blocks)?,
            },
            shared::project_turns(&blocks)?,
        ))
    }
    pub fn patch_remote_session(&self, id: i64, patch: Value) -> Result<(), String> {
        let mut remote = self
            .remote_session(id)?
            .ok_or("Remote session no longer exists")?;
        if let Some(session_id) = &remote.session_id {
            let client = crate::remote::default_connections()?;
            let machine = remote.machine(&client)?;
            let mut params = patch.clone();
            params["sessionId"] = json!(session_id);
            params["projectId"] = json!(remote.project.project_id);
            client.request(&machine.id, "sessions.update", params)?;
        }
        if let Some(title) = patch["title"].as_str() {
            remote.summary.title = title.into();
        }
        if let Some(archived) = patch["archived"].as_bool() {
            remote.summary.archived = archived;
        }
        if let Some(pinned) = patch["pinned"].as_bool() {
            remote.summary.pinned = pinned;
        }
        if let Some(item) = patch.get("linkedWorkItem") {
            remote.summary.work_item = serde_json::from_value(item.clone()).ok();
        }
        self.save_remote_session(&remote)
    }
    pub fn delete_remote_session(&self, id: i64) -> Result<(), String> {
        let remote = self
            .remote_session(id)?
            .ok_or("Remote session no longer exists")?;
        if let Some(session_id) = &remote.session_id {
            let client = crate::remote::default_connections()?;
            let machine = remote.machine(&client)?;
            client.request(
                &machine.id,
                "sessions.delete",
                json!({"sessionId": session_id,"projectId": remote.project.project_id}),
            )?;
        }
        let mut sessions = self.remote_sessions()?;
        sessions.remove(&id);
        self.hide_legacy_remote_sessions([id])?;
        self.save_setting(
            SESSIONS,
            &serde_json::to_string(&sessions).map_err(|e| e.to_string())?,
        )
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod configuration_tests;
