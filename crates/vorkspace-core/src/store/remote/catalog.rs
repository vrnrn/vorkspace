//! Reconcile a complete host project listing against durable desktop bindings.
use super::*;
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashSet},
};

#[derive(Debug)]
pub struct RemoteListEntry {
    pub summary: Session,
    pub running: bool,
    pub needs_input: bool,
    pub draft: bool,
}

#[derive(Debug)]
pub struct RemoteListing {
    pub entries: Vec<RemoteListEntry>,
    pub removed: Vec<i64>,
    pub changed: bool,
}

pub struct ListingBaseline(HashMap<i64, ListingStamp>);

struct ListingStamp {
    host_id: String,
    revision: Option<u64>,
    updated_at: u64,
}

impl ListingStamp {
    fn matches(&self, remote: &RemoteSession) -> bool {
        remote.session_id.as_ref() == Some(&self.host_id)
            && remote.host_revision == self.revision
            && remote.summary.updated_at == self.updated_at
    }
}

type Sidecar = BTreeMap<String, String>;

pub(super) struct Catalog<'a> {
    store: &'a Store,
    sidecar: Option<&'a Sidecar>,
    pub(super) sessions: BTreeMap<i64, RemoteSession>,
    pub(super) hidden: HashSet<i64>,
    changed: bool,
    hidden_changed: bool,
    writes: Vec<(String, String)>,
}

impl<'a> Catalog<'a> {
    fn read_setting(
        store: &Store,
        sidecar: Option<&'a Sidecar>,
        key: &str,
    ) -> Result<Option<Cow<'a, str>>, String> {
        match sidecar {
            Some(values) => Ok(values.get(key).map(|value| Cow::Borrowed(value.as_str()))),
            None => Ok(store.setting(key)?.map(Cow::Owned)),
        }
    }

    pub(super) fn load(store: &'a Store, sidecar: Option<&'a Sidecar>) -> Result<Self, String> {
        let hidden = decode_hidden(Self::read_setting(store, sidecar, HIDDEN)?.as_deref())?;
        let stored = decode_sessions(Self::read_setting(store, sidecar, SESSIONS)?.as_deref())?;
        let mut sessions = store.legacy_remote_sessions()?;
        sessions.retain(|id, _| !hidden.contains(id));

        // Imported tabs keep their positive IDs, including any stored overlay
        // that was written using the generated negative ID.
        let mut positive: HashMap<_, BTreeSet<_>> = HashMap::new();
        for (&id, remote) in &sessions {
            if let Some(key) = host_key(remote) {
                positive.entry(key).or_default().insert(id);
            }
        }
        for (id, mut remote) in stored {
            let key = (id >= 0 || !positive.is_empty())
                .then(|| host_key(&remote))
                .flatten();
            let canonical = key
                .as_ref()
                .and_then(|key| positive.get(key)?.first().copied())
                .unwrap_or(id);
            remote.rekey(id, canonical);
            let previous = sessions.insert(canonical, remote);
            if canonical >= 0 {
                if let Some(old_key) = previous.as_ref().and_then(host_key) {
                    if let Some(ids) = positive.get_mut(&old_key) {
                        ids.remove(&canonical);
                        if ids.is_empty() {
                            positive.remove(&old_key);
                        }
                    }
                }
                if let Some(key) = key {
                    positive.entry(key).or_default().insert(canonical);
                }
            }
        }
        Ok(Self {
            store,
            sidecar,
            sessions,
            hidden,
            changed: false,
            hidden_changed: false,
            writes: Vec::new(),
        })
    }

    fn import(
        &mut self,
        project: &RemoteProject,
        id: i64,
        host_id: &str,
        value: &Value,
        session: Option<&Value>,
    ) -> Result<(), String> {
        let prior = self.sessions.get(&id);
        let full_snapshot = session.is_some();
        let host = session.unwrap_or(value);
        let revision = value["revision"].as_u64();
        if let Some(prior) = prior {
            if prior.project.environment_id != project.environment_id
                || prior.session_id.as_deref() != Some(host_id)
            {
                return Err("Remote session handle conflicts with an existing binding".into());
            }
            // An older or unversioned response cannot overwrite ordered state.
            // A full snapshot can still hydrate configuration at its current revision.
            if prior.host_revision.is_some_and(|known| {
                revision.is_none_or(|revision| {
                    revision < known
                        || (revision == known
                            && (!full_snapshot || prior.configuration_revision == Some(revision)))
                })
            }) {
                return Ok(());
            }
        }
        let summary = host_summary(
            project,
            id,
            value,
            host,
            prior.map(|remote| &remote.summary),
        );
        let imported = host_settings(project, host);
        let configuration = if full_snapshot {
            Some(crate::remote::protocol::settings_configuration(
                &summary.provider,
                &imported,
            ))
        } else {
            prior.and_then(|remote| remote.configuration.clone())
        };
        let key = agent_key(id);
        let raw = Self::read_setting(self.store, self.sidecar, &key)?;
        let existing = self
            .store
            .read_remote_agent_settings(id, raw.as_deref(), full_snapshot)?;
        let mut settings_to_save = None;
        match existing {
            None => settings_to_save = Some(imported),
            Some(mut settings) if full_snapshot => {
                let pending = prior
                    .is_some_and(|remote| !remote.outbox.is_empty() || !remote.uploads.is_empty());
                let local =
                    crate::remote::protocol::settings_configuration(&summary.provider, &settings);
                let unchanged = prior
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
            self.writes.push((key, encode(&settings)?));
        }
        let mut remote = RemoteSession {
            summary,
            project: project.clone(),
            session_id: Some(host_id.into()),
            outbox: Vec::new(),
            uploads: Vec::new(),
            rejected: None,
            composer_origin: None,
            composer_alias: prior.and_then(|remote| remote.composer_alias),
            configuration,
            host_revision: revision.or_else(|| prior.and_then(|remote| remote.host_revision)),
            configuration_revision: if full_snapshot {
                revision
            } else {
                prior.and_then(|remote| remote.configuration_revision)
            },
            running: value["status"].as_str().map_or_else(
                || prior.is_some_and(|remote| remote.running),
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
                prior.is_some_and(|remote| remote.needs_input)
            },
        };
        if let Some(prior) = self.sessions.remove(&id) {
            remote.outbox = prior.outbox;
            remote.uploads = prior.uploads;
            remote.rejected = prior.rejected;
            remote.composer_origin = prior.composer_origin;
        }
        self.sessions.insert(id, remote);
        self.changed = true;
        Ok(())
    }

    fn remove_missing(
        &mut self,
        project: &RemoteProject,
        live_ids: &HashSet<&str>,
        baseline: &ListingBaseline,
    ) -> Vec<i64> {
        let mut removed = Vec::new();
        self.sessions.retain(|id, remote| {
            let deleted = in_project(remote, project)
                && remote
                    .session_id
                    .as_deref()
                    .is_some_and(|host| !live_ids.contains(host))
                && remote.outbox.is_empty()
                && remote.uploads.is_empty()
                && remote.rejected.is_none()
                && baseline
                    .0
                    .get(id)
                    .is_some_and(|stamp| stamp.matches(remote));
            if deleted {
                removed.push(*id);
            }
            !deleted
        });
        for &id in &removed {
            self.record_removal(id);
        }
        removed
    }

    fn record_removal(&mut self, id: i64) {
        self.changed = true;
        if id >= 0 && self.hidden.insert(id) {
            // The hidden markers and binding map publish through the same root.
            self.hidden_changed = true;
        }
    }

    fn writes(mut self) -> Result<Vec<(String, String)>, String> {
        if self.changed {
            self.writes.push((SESSIONS.into(), encode(&self.sessions)?));
        }
        if self.hidden_changed {
            self.writes.push((HIDDEN.into(), encode(&self.hidden)?));
        }
        Ok(self.writes)
    }
}

fn in_project(remote: &RemoteSession, project: &RemoteProject) -> bool {
    remote.project.environment_id == project.environment_id
        && remote.project.project_id == project.project_id
}

pub(super) fn host_summary(
    project: &RemoteProject,
    id: i64,
    value: &Value,
    host: &Value,
    previous: Option<&Session>,
) -> Session {
    let mut summary = previous.cloned().unwrap_or_else(|| Session {
        id,
        title: "Remote session".into(),
        provider: "codex".into(),
        cwd: project.key.clone(),
        provider_id: None,
        has_messages: true,
        updated_at: 0,
        archived: false,
        pinned: false,
        work_item: None,
    });
    summary.cwd.clone_from(&project.key);
    if let Some(provider) = host["harness"].as_str() {
        summary.provider = provider.into();
    }
    if let Some(provider_id) = host.get("providerSessionId") {
        summary.provider_id = provider_id.as_str().map(str::to_owned);
    }
    if let Some(blocks) = host["blocks"].as_array() {
        summary.has_messages = !blocks.is_empty();
    }
    if let Some(updated_at) = value["updatedAt"].as_u64() {
        summary.updated_at = updated_at;
    }
    apply_card_fields(&mut summary, value, host);
    summary
}

pub(super) fn apply_card_fields(summary: &mut Session, value: &Value, host: &Value) {
    if let Some(title) = host["title"].as_str() {
        summary.title = title.into();
    }
    if let Some(archived) = value["archived"].as_bool() {
        summary.archived = archived;
    }
    if let Some(pinned) = value["pinned"].as_bool() {
        summary.pinned = pinned;
    }
    if let Some(item) = host.get("linkedWorkItem") {
        summary.work_item = serde_json::from_value(item.clone()).ok();
    }
}

fn host_key(remote: &RemoteSession) -> Option<(String, String)> {
    Some((
        remote.project.environment_id.clone(),
        remote.session_id.clone()?,
    ))
}

fn host_identity(host: &Value) -> Result<&str, String> {
    host["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "Invalid remote session identity".into())
}

fn generated_id(project: &RemoteProject, host_id: &str) -> i64 {
    -shared::handle(&format!("remote:{}:{host_id}", project.environment_id))
}

fn encode(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| error.to_string())
}

impl Store {
    pub(super) fn edit_remote_catalog<T>(
        &self,
        edit: impl FnOnce(&mut Catalog<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
        self.update_settings(|settings| {
            let mut catalog = Catalog::load(self, Some(settings))?;
            let result = edit(&mut catalog)?;
            Ok((result, catalog.writes()?))
        })
    }

    pub fn remote_listing_baseline(
        &self,
        project: &RemoteProject,
    ) -> Result<ListingBaseline, String> {
        let (_, sessions) = self.read_remote_catalog()?;
        let stamps = sessions
            .into_iter()
            .filter_map(|(id, remote)| {
                if !in_project(&remote, project) {
                    return None;
                }
                Some((
                    id,
                    ListingStamp {
                        host_id: remote.session_id?,
                        revision: remote.host_revision,
                        updated_at: remote.summary.updated_at,
                    },
                ))
            })
            .collect();
        Ok(ListingBaseline(stamps))
    }

    /// Apply the whole response in one publication. The baseline is captured
    /// before requesting the listing, so absence cannot delete newer work.
    pub fn apply_remote_listing(
        &self,
        project: &RemoteProject,
        values: &[Value],
        baseline: &ListingBaseline,
    ) -> Result<RemoteListing, String> {
        let ids = values
            .iter()
            .map(host_identity)
            .collect::<Result<Vec<_>, _>>()?;
        let live_ids: HashSet<_> = ids.iter().copied().collect();
        if live_ids.len() != ids.len() {
            return Err("Remote listing has duplicate session identities".into());
        }
        self.edit_remote_catalog(|catalog| {
            let handles: Vec<_> =
                {
                    let mut bindings = HashMap::new();
                    for (&id, remote) in catalog.sessions.iter().filter(|(_, remote)| {
                        remote.project.environment_id == project.environment_id
                    }) {
                        if let Some(host) = remote.session_id.as_deref() {
                            bindings.entry(host).or_insert(id);
                        }
                    }
                    ids.iter()
                        .map(|&host| {
                            bindings
                                .get(host)
                                .copied()
                                .unwrap_or_else(|| generated_id(project, host))
                        })
                        .collect()
                };
            let mut entries = Vec::with_capacity(values.len());
            for ((value, host_id), id) in values.iter().zip(ids).zip(handles) {
                catalog.import(project, id, host_id, value, None)?;
                let remote = &catalog.sessions[&id];
                entries.push(RemoteListEntry {
                    summary: remote.summary.clone(),
                    running: remote.running,
                    needs_input: remote.needs_input,
                    draft: value["draft"] == true,
                });
            }
            let removed = catalog.remove_missing(project, &live_ids, baseline);
            Ok(RemoteListing {
                entries,
                removed,
                changed: catalog.changed,
            })
        })
    }

    pub fn register_remote_summary(
        &self,
        project: &RemoteProject,
        value: &Value,
    ) -> Result<Session, String> {
        let session = value.get("session").filter(|host| host.is_object());
        let host_id = host_identity(session.unwrap_or(value))?;
        self.edit_remote_catalog(|catalog| {
            let id = catalog
                .sessions
                .values()
                .find(|remote| {
                    remote.project.environment_id == project.environment_id
                        && remote.session_id.as_deref() == Some(host_id)
                })
                .map_or_else(
                    || generated_id(project, host_id),
                    |remote| remote.summary.id,
                );
            catalog.import(project, id, host_id, value, session)?;
            Ok(catalog.sessions[&id].summary.clone())
        })
    }

    /// Publish a response only into the binding that admitted the request.
    pub fn observe_bound_remote_summary(
        &self,
        binding: &RemoteSession,
        value: &Value,
    ) -> Result<RemoteSession, String> {
        let session = value.get("session").filter(|host| host.is_object());
        let host_id = host_identity(session.unwrap_or(value))?;
        if Some(host_id) != binding.session_id.as_deref()
            || value["projectId"]
                .as_str()
                .is_some_and(|project| project != binding.project.project_id)
        {
            return Err("Host returned metadata for another conversation".into());
        }
        self.edit_remote_catalog(|catalog| {
            let id = binding.summary.id;
            let project = &catalog
                .sessions
                .get(&id)
                .filter(|current| current.same_binding(binding))
                .ok_or("The observed remote conversation changed or was removed")?
                .project
                .clone();
            catalog.import(project, id, host_id, value, session)?;
            Ok(catalog.sessions[&id].clone())
        })
    }

    pub(super) fn forget_remote_binding(&self, binding: &RemoteSession) -> Result<(), String> {
        self.edit_remote_catalog(|catalog| {
            let id = binding.summary.id;
            if let Some(current) = catalog.sessions.get(&id) {
                if !current.same_binding(binding) {
                    return Err("Remote conversation changed before it could be removed".into());
                }
                catalog.sessions.remove(&id);
                catalog.record_removal(id);
            }
            Ok(())
        })
    }
}
