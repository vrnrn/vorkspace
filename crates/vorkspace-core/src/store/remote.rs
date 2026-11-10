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
use std::collections::{BTreeMap, HashMap, HashSet};

mod catalog;
pub use catalog::RemoteListing;

const PROJECTS: &str = "remote-projects.v2";
const SESSIONS: &str = "remote-sessions.v1";

const HIDDEN: &str = "remote-hidden-legacy.v1";

pub(super) fn agent_key(id: i64) -> String {
    format!("remote-agent:{id}")
}

fn decode_sessions(raw: Option<&str>) -> Result<BTreeMap<i64, RemoteSession>, String> {
    raw.map(serde_json::from_str)
        .transpose()
        .map(Option::unwrap_or_default)
        .map_err(|error| format!("Cannot read remote sessions: {error}"))
}

fn decode_hidden(raw: Option<&str>) -> Result<HashSet<i64>, String> {
    raw.map(serde_json::from_str)
        .transpose()
        .map(Option::unwrap_or_default)
        .map_err(|error| format!("Cannot read hidden remote sessions: {error}"))
}

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
        settings.worktree_cwd = Some(crate::remote_path::remote_path(
            &project.environment_id,
            cwd,
        ));
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
            key: crate::remote_path::remote_path(
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
        decode_sessions(self.setting(SESSIONS)?.as_deref())
    }

    pub(super) fn hidden_legacy_remote_sessions(&self) -> Result<HashSet<i64>, String> {
        decode_hidden(self.setting(HIDDEN)?.as_deref())
    }

    fn read_remote_catalog(&self) -> Result<(HashSet<i64>, BTreeMap<i64, RemoteSession>), String> {
        let sidecar = self
            .shared
            .is_some()
            .then(|| self.shared_settings_snapshot())
            .transpose()?;
        let catalog = catalog::Catalog::load(self, sidecar.as_ref())?;
        Ok((catalog.hidden, catalog.sessions))
    }

    pub(super) fn read_remote_agent_settings(
        &self,
        id: i64,
        raw: Option<&str>,
        include_turns: bool,
    ) -> Result<Option<Settings>, String> {
        if let Some(raw) = raw {
            return serde_json::from_str(raw)
                .map(Some)
                .map_err(|error| format!("Cannot read remote settings: {error}"));
        }
        if id < 0 || self.shared.is_none() {
            return Ok(None);
        }
        let mut settings = if include_turns {
            self.shared_agent(id)?
        } else {
            self.shared_agent_preferences(id)?
        };
        if let Some(settings) = settings
            .as_mut()
            .filter(|settings| settings.worktree_cwd.is_none())
        {
            settings.worktree_cwd = self.legacy_remote_worktree(id)?;
        }
        Ok(settings)
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
        let (hidden, remote) = self.read_remote_catalog()?;
        let mut sessions: HashMap<_, _> = self
            .local_sessions()?
            .into_iter()
            .filter(|summary| !hidden.contains(&summary.id))
            .map(|summary| (summary.id, summary))
            .collect();
        sessions.extend(remote.into_iter().map(|(id, remote)| (id, remote.summary)));
        let mut sessions: Vec<_> = sessions.into_values().collect();
        sessions.sort_by_key(|s| (std::cmp::Reverse(s.updated_at), std::cmp::Reverse(s.id)));
        Ok(sessions)
    }
    pub fn create(&self, provider: &str, cwd: &str) -> Result<Session, String> {
        self.check_project_creation(cwd)?;
        if !cwd.starts_with("remote://") {
            return self.local_create(provider, cwd);
        }
        let kind = crate::kind::Kind::parse(provider).ok_or("Unknown remote provider")?;
        let project = self
            .remote_projects()?
            .into_values()
            .find(|project| {
                project.key.trim_end_matches('/') == cwd.trim_end_matches('/')
                    || crate::remote_path::remote_path(
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
        self.forget_remote_binding(id)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod configuration_tests;
