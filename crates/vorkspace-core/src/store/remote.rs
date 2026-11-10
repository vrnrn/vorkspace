//! Host-owned sessions keep only their desktop identity and command receipts here.
//! Transcripts remain on the host and use the same lossless block projection as
//! desktop conversations; no remote row is inserted into the local session table.
use super::{remote_legacy::OWNERS, shared, Session, Store};
use crate::{
    agent::{Settings, TurnInfo},
    remote::{Connections, Machine},
    transcript::{Row, Transcript},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

mod catalog;
pub(super) mod index;
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The last exact composer revision accepted into this desktop journal.
    /// Kept after dispatch so reopening cannot offer the same draft again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer_origin: Option<super::ComposerOrigin>,
    /// Earlier native handle retained when a legacy desktop tab owns the Host binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer_alias: Option<i64>,
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
    pub(super) fn rekey(&mut self, previous: i64, next: i64) {
        if previous < 0 && next >= 0 {
            self.composer_alias.get_or_insert(previous);
        }
        self.summary.id = next;
    }

    pub fn same_binding(&self, other: &Self) -> bool {
        self.summary.id == other.summary.id
            && self.session_id == other.session_id
            && self.project.environment_id == other.project.environment_id
            && self.project.project_id == other.project.project_id
    }

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

/// The desktop projection of an ordered Host transcript. The observer supplies
/// the earliest changed block in the next snapshot. Unchanged rows keep their
/// existing allocation and never pass through attachment or tool decoding.
#[derive(Default)]
pub struct RemoteHistory {
    rows: Vec<Row>,
    block_ends: Vec<usize>,
}

impl RemoteHistory {
    pub fn update(
        &mut self,
        snapshot: &Value,
        first: usize,
        environment: &str,
        session: &str,
    ) -> Result<(usize, Vec<Row>), String> {
        let blocks = remote_blocks(snapshot)?;
        let start = if first == 0 {
            0
        } else {
            *self
                .block_ends
                .get(first - 1)
                .ok_or("Remote history base changed")?
        };
        let affected = blocks.get(first..).ok_or("Invalid remote history range")?;
        let mut tail = Vec::new();
        let mut ends = Vec::with_capacity(affected.len());
        for block in affected {
            tail.extend(shared::blocks::project_remote_block(
                block,
                environment,
                session,
            )?);
            ends.push(start + tail.len());
        }
        let retained = self.rows[start..]
            .iter()
            .zip(&tail)
            .take_while(|(a, b)| a == b)
            .count();
        let patch = tail[retained..].to_vec();
        self.block_ends.truncate(first);
        self.block_ends.extend(ends);
        self.rows.truncate(start + retained);
        self.rows.extend(tail.into_iter().skip(retained));
        Ok((start + retained, patch))
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn into_rows(self) -> Vec<Row> {
        self.rows
    }
}

fn remote_blocks(snapshot: &Value) -> Result<&[Value], String> {
    snapshot["session"]["blocks"]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| "Host transcript is missing".into())
}

impl Store {
    pub fn remote_projects(&self) -> Result<BTreeMap<String, RemoteProject>, String> {
        self.remote_projects_from(self.setting(PROJECTS)?.as_deref())
    }

    pub fn remote_project(&self, cwd: &str) -> Result<RemoteProject, String> {
        let mut projects = self.remote_projects()?;
        if let Some(project) = projects.remove(cwd) {
            return Ok(project);
        }
        projects
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
            .ok_or_else(|| {
                "Open this folder on its machine again to restore its project identity".into()
            })
    }

    fn remote_projects_from(
        &self,
        saved: Option<&str>,
    ) -> Result<BTreeMap<String, RemoteProject>, String> {
        let mut projects: BTreeMap<String, RemoteProject> = self
            .desktop_preferences()
            .and_then(|prefs| prefs.get("vorkspace.remote-projects.v2"))
            .map(|raw| {
                serde_json::from_str(raw)
                    .map_err(|e| format!("Cannot read desktop remote projects: {e}"))
            })
            .unwrap_or_else(|| Ok(BTreeMap::new()))?;
        if let Some(raw) = saved {
            projects.extend(
                serde_json::from_str::<BTreeMap<String, RemoteProject>>(raw)
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
        self.update_settings(|settings| {
            let mut projects =
                self.remote_projects_from(settings.get(PROJECTS).map(String::as_str))?;
            projects.insert(remote.key.clone(), remote.clone());
            Ok((remote, vec![(PROJECTS.into(), shared::encode(&projects)?)]))
        })
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
        if self.shared.is_some() {
            return self.shared_remote_session(id);
        }
        let mut stored = decode_sessions(self.setting(SESSIONS)?.as_deref())?;
        self.take_remote_session(id, &mut stored, || self.hidden_legacy_remote_sessions())
    }

    fn take_remote_session(
        &self,
        id: i64,
        stored: &mut BTreeMap<i64, RemoteSession>,
        hidden: impl FnOnce() -> Result<HashSet<i64>, String>,
    ) -> Result<Option<RemoteSession>, String> {
        let exact = stored.remove(&id);
        self.resolve_remote_session(
            id,
            exact,
            || Ok(hidden()?.contains(&id)),
            |legacy| {
                Ok(legacy
                    .session_id
                    .as_ref()
                    .and_then(|host| {
                        stored.values().find(|saved| {
                            saved.project.environment_id == legacy.project.environment_id
                                && saved.session_id.as_ref() == Some(host)
                        })
                    })
                    .cloned())
            },
        )
    }

    fn resolve_remote_session(
        &self,
        id: i64,
        exact: Option<RemoteSession>,
        hidden: impl FnOnce() -> Result<bool, String>,
        alias: impl FnOnce(&RemoteSession) -> Result<Option<RemoteSession>, String>,
    ) -> Result<Option<RemoteSession>, String> {
        if let Some(remote) = exact {
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
        if id < 0 || hidden()? {
            return Ok(None);
        }
        let Some(legacy) = self.legacy_remote_session(id)? else {
            return Ok(None);
        };
        if let Some(mut saved) = alias(&legacy)? {
            saved.rekey(saved.summary.id, id);
            return Ok(Some(saved));
        }
        Ok(Some(legacy))
    }

    fn edit_remote_session(
        &self,
        id: i64,
        edit: impl FnOnce(Option<RemoteSession>) -> Result<Option<RemoteSession>, String>,
    ) -> Result<Option<RemoteSession>, String> {
        self.update_settings(|settings| {
            let mut stored = decode_sessions(settings.get(SESSIONS).map(String::as_str))?;
            let current = self.take_remote_session(id, &mut stored, || {
                decode_hidden(settings.get(HIDDEN).map(String::as_str))
            })?;
            let Some(next) = edit(current)? else {
                return Ok((None, Vec::new()));
            };
            if id >= 0 && next.session_id.is_some() {
                stored.retain(|_, existing| {
                    existing.project.environment_id != next.project.environment_id
                        || existing.session_id != next.session_id
                });
            }
            let claim =
                self.legacy_remote_claim(&next, settings.get(OWNERS).map(String::as_str))?;
            stored.insert(id, next);
            let encoded = serde_json::to_string(&stored).map_err(|error| error.to_string())?;
            let mut writes = vec![(SESSIONS.into(), encoded)];
            if let Some(claim) = claim {
                writes.push((OWNERS.into(), claim));
            }
            Ok((stored.remove(&id), writes))
        })
    }

    pub fn save_remote_session(&self, session: &RemoteSession) -> Result<(), String> {
        self.edit_remote_session(session.summary.id, |current| {
            let mut next = session.clone();
            if let Some(current) = current.filter(|current| current.same_binding(&next)) {
                next.composer_origin = current.composer_origin;
                next.composer_alias = current.composer_alias;
                // Host observations can commit while a command is in flight.
                // Legacy writes still preserve newer revisioned metadata.
                if next.host_revision < current.host_revision {
                    next.host_revision = current.host_revision;
                    next.summary = current.summary;
                    next.running = current.running;
                    next.needs_input = current.needs_input;
                }
                if next.configuration_revision < current.configuration_revision {
                    next.configuration_revision = current.configuration_revision;
                    next.configuration = current.configuration;
                }
            }
            Ok(Some(next))
        })
        .map(|_| ())
    }

    /// Publish the command journal only if its exact admitted base still owns
    /// this desktop handle. Host observations may advance independently.
    pub fn commit_remote_journal(
        &self,
        before: &RemoteSession,
        next: RemoteSession,
    ) -> Result<RemoteSession, String> {
        self.edit_remote_session(before.summary.id, |current| {
            let mut current = current
                .filter(|current| {
                    current.same_binding(before)
                        && current.summary.provider == before.summary.provider
                        && current.outbox == before.outbox
                        && current.uploads == before.uploads
                        && current.rejected == before.rejected
                        && current.composer_origin == before.composer_origin
                })
                .ok_or("The local remote command journal changed. Refresh the conversation before retrying.")?;
            if current.session_id.is_some() && current.session_id != next.session_id {
                return Err("A remote receipt cannot change an existing session binding".into());
            }
            if current.composer_origin != next.composer_origin {
                let origin = next.composer_origin.as_ref()
                    .ok_or("A remote journal cannot discard a composer transfer")?;
                if next.outbox.is_empty() || !self.remote_composer_source_matches(&current, origin)? {
                    return Err("Remote journal does not own this composer version".into());
                }
                current.composer_origin = next.composer_origin;
            }
            current.session_id = next.session_id;
            current.outbox = next.outbox;
            current.uploads = next.uploads;
            current.rejected = next.rejected;
            if next.summary.has_messages != before.summary.has_messages {
                current.summary.has_messages = next.summary.has_messages;
            }
            if next.summary.updated_at != before.summary.updated_at {
                current.summary.updated_at = current.summary.updated_at.max(next.summary.updated_at);
            }
            Ok(Some(current))
        })?
        .ok_or_else(|| "Remote journal did not publish".into())
    }

    /// A live poll owns Host metadata at its observed revision. It never
    /// republishes the command state that happened to be present at admission.
    pub fn observe_remote_activity(
        &self,
        binding: &RemoteSession,
        snapshot: &Value,
        needs_input: bool,
    ) -> Result<(), String> {
        self.edit_remote_session(binding.summary.id, |current| {
            let mut current = current
                .filter(|current| current.same_binding(binding))
                .ok_or("The observed remote conversation changed or was removed")?;
            let session = &snapshot["session"];
            if session["id"].as_str() != current.session_id.as_deref()
                || snapshot["projectId"]
                    .as_str()
                    .is_some_and(|project| project != current.project.project_id)
            {
                return Err("Host returned activity for another conversation".into());
            }
            let revision = snapshot["revision"].as_u64();
            if revision < current.host_revision {
                return Ok(None);
            }
            let running = snapshot["status"] == "running";
            let summary = catalog::host_summary(
                &current.project,
                current.summary.id,
                snapshot,
                session,
                Some(&current.summary),
            );
            if revision == current.host_revision
                && running == current.running
                && needs_input == current.needs_input
                && summary == current.summary
            {
                return Ok(None);
            }
            current.host_revision = revision;
            current.running = running;
            current.needs_input = needs_input;
            current.summary = summary;
            Ok(Some(current))
        })
        .map(|_| ())
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
        let project = self.remote_project(cwd)?;
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
            composer_origin: None,
            composer_alias: None,
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
        environment: &str,
        session: &str,
    ) -> Result<(Transcript, HashMap<usize, TurnInfo>), String> {
        let mut history = Transcript::default();
        let mut turns = HashMap::new();
        for block in remote_blocks(snapshot)? {
            let rows = shared::blocks::project_remote_block(block, environment, session)?;
            shared::record_projected_turn(block, &rows, history.rows.len(), &mut turns);
            history.rows.extend(rows);
        }
        Ok((history, turns))
    }
    pub fn patch_remote_session(&self, id: i64, patch: Value) -> Result<(), String> {
        let remote = self
            .remote_session(id)?
            .ok_or("Remote session no longer exists")?;
        if let Some(session_id) = &remote.session_id {
            let client = crate::remote::default_connections()?;
            let machine = remote.machine(&client)?;
            let clear_link = patch.get("linkedWorkItem").is_some_and(Value::is_null);
            let mut params = patch;
            params["sessionId"] = json!(session_id);
            params["projectId"] = json!(remote.project.project_id);
            let mut receipt = client.request(&machine.id, "sessions.update", params)?;
            if let Some(fields) = receipt.as_object_mut().filter(|_| clear_link) {
                // Older Host versions omitted the cleared field in their receipt.
                fields.entry("linkedWorkItem").or_insert(Value::Null);
            }
            self.observe_bound_remote_summary(&remote, &receipt)?;
            return Ok(());
        }
        self.edit_remote_session(id, |current| {
            let mut current = current
                .filter(|current| current.same_binding(&remote))
                .ok_or("Remote conversation changed before the edit could be saved")?;
            catalog::apply_card_fields(&mut current.summary, &patch, &patch);
            Ok(Some(current))
        })
        .map(|_| ())
    }

    pub(super) fn change_remote_provider(
        &self,
        binding: &RemoteSession,
        provider: &str,
    ) -> Result<(), String> {
        self.edit_remote_session(binding.summary.id, |current| {
            let mut current = current
                .filter(|current| current.same_binding(binding))
                .ok_or("Remote conversation changed before the provider could be saved")?;
            if current.summary.provider == provider {
                return Ok(None);
            }
            if current.session_id.is_some() || !current.outbox.is_empty() {
                return Err("Start a new remote conversation to change provider".into());
            }
            current.summary.provider = provider.into();
            Ok(Some(current))
        })
        .map(|_| ())
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
        self.forget_remote_binding(&remote)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod configuration_tests;
