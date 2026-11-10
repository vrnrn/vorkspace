//! Project model discovery and bounded, connection-scoped shared observations.
use super::SshTarget;
use crate::{
    kind::Kind,
    model_cache::{model, Model, ModelControl, ModelRows},
};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, PoisonError},
    time::{Duration, Instant},
};

/// The Host returns one complete list of provider availability and options.
#[derive(Debug)]
pub struct RemoteCatalog {
    pub providers: Vec<Kind>,
    pub models: HashMap<Kind, ModelRows>,
    pub errors: HashMap<Kind, String>,
}

impl RemoteCatalog {
    pub fn models_for(&self, kind: Kind) -> Result<ModelRows, String> {
        if !self.providers.contains(&kind) {
            return Err(format!(
                "{} CLI is not installed on this machine",
                kind.label()
            ));
        }
        if let Some(error) = self.errors.get(&kind) {
            return Err(error.clone());
        }
        self.models
            .get(&kind)
            .filter(|models| !models.is_empty())
            .cloned()
            .ok_or_else(|| format!("{} returned an empty model catalog", kind.label()))
    }

    fn estimated_bytes(&self) -> usize {
        self.errors.values().map(String::capacity).sum::<usize>()
            + self
                .models
                .values()
                .flat_map(|rows| rows.iter())
                .map(Model::estimated_bytes)
                .sum::<usize>()
    }
}

const RETENTION: Duration = Duration::from_secs(8);
const PARTIAL_RETENTION: Duration = Duration::from_secs(1);
const CACHE_BYTES: usize = 8 * 1024 * 1024;
const CACHE_PROJECTS: usize = 24;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct ConnectionStamp {
    pub environment_id: String,
    pub endpoint: String,
    pub ssh: Option<SshTarget>,
    pub credential: [u8; 32],
}

struct Observed {
    result: Result<Arc<RemoteCatalog>, String>,
    completed_at: Instant,
    bytes: usize,
}

impl Observed {
    fn fresh(&self) -> bool {
        self.result.as_ref().is_ok_and(|catalog| {
            self.completed_at.elapsed()
                < if catalog.errors.is_empty() {
                    RETENTION
                } else {
                    PARTIAL_RETENTION
                }
        })
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Scope {
    machine: String,
    project: String,
    connection: ConnectionStamp,
}

#[derive(Default)]
pub(crate) struct CatalogCache {
    projects: Mutex<HashMap<Scope, Arc<OnceLock<Observed>>>>,
}

impl CatalogCache {
    pub(crate) fn load(
        &self,
        machine: &str,
        project: &str,
        connection: &ConnectionStamp,
        read: impl FnOnce() -> Result<RemoteCatalog, String>,
    ) -> Result<Arc<RemoteCatalog>, String> {
        let observation = {
            let mut projects = self.projects.lock().unwrap_or_else(PoisonError::into_inner);
            let key = Scope {
                machine: machine.into(),
                project: project.into(),
                connection: connection.clone(),
            };
            let entry = projects.entry(key).or_default();
            if entry.get().is_some_and(|old| !old.fresh()) {
                *entry = Arc::new(OnceLock::new());
            }
            entry.clone()
        };
        let mut loaded_now = false;
        let loaded = observation.get_or_init(|| {
            loaded_now = true;
            let result = read().map(Arc::new);
            Observed {
                bytes: result
                    .as_ref()
                    .map_or(0, |catalog| catalog.estimated_bytes()),
                result,
                completed_at: Instant::now(),
            }
        });
        if loaded_now {
            self.trim();
        }
        loaded.result.clone()
    }

    pub(crate) fn remove_machine(&self, machine: &str) {
        self.projects
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|scope, _| scope.machine != machine);
    }

    fn trim(&self) {
        let mut projects = self.projects.lock().unwrap_or_else(PoisonError::into_inner);
        projects.retain(|_, entry| entry.get().is_none_or(Observed::fresh));
        let mut settled = projects
            .iter()
            .filter_map(|(key, entry)| {
                entry
                    .get()
                    .map(|observation| (key.clone(), observation.completed_at, observation.bytes))
            })
            .collect::<Vec<_>>();
        settled.sort_unstable_by_key(|(_, at, _)| std::cmp::Reverse(*at));
        let mut bytes = 0usize;
        let mut retained = 0;
        for (key, _, weight) in settled {
            if retained < CACHE_PROJECTS && bytes.saturating_add(weight) <= CACHE_BYTES {
                bytes += weight;
                retained += 1;
            } else {
                projects.remove(&key);
            }
        }
    }
}

pub(crate) fn remote_catalog_response(value: &Value) -> Result<RemoteCatalog, String> {
    if !value["models"].is_object() || !value["errors"].is_object() {
        return Err("Invalid remote model catalog".into());
    }
    let providers = Kind::ALL
        .into_iter()
        .filter(|kind| {
            value["models"].get(kind.id()).is_some() || value["errors"].get(kind.id()).is_some()
        })
        .collect::<Vec<_>>();
    let mut models = HashMap::new();
    let mut errors = HashMap::new();
    for kind in &providers {
        if let Some(error) = value["errors"][kind.id()].as_str() {
            errors.insert(*kind, error.into());
        }
        if let Some(rows) = value["models"][kind.id()].as_array() {
            let mut seen = std::collections::HashSet::new();
            let entries: Vec<_> = rows
                .iter()
                .filter_map(|row| {
                    let id = row["id"].as_str()?;
                    if row["hidden"] == true || !seen.insert(id) {
                        return None;
                    }
                    let mut model = model(id, row["name"].as_str().unwrap_or(id));
                    model.default = row["isDefault"] == true || rows.first() == Some(row);
                    for setting in row["settings"].as_array().into_iter().flatten() {
                        let Some(id) = setting["id"].as_str() else {
                            continue;
                        };
                        let choices: Vec<(String, String)> = setting["options"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|choice| {
                                let value = choice["value"].as_str()?;
                                Some((
                                    value.into(),
                                    choice["label"].as_str().unwrap_or(value).into(),
                                ))
                            })
                            .collect();
                        let current = setting["value"].as_str().unwrap_or("");
                        match id {
                            "effort" | "reasoningEffort" => {
                                model.efforts =
                                    choices.iter().map(|(value, _)| value.clone()).collect();
                                model.default_effort =
                                    (!current.is_empty()).then(|| current.into());
                            }
                            "serviceTier" => {
                                model.tiers = choices
                                    .into_iter()
                                    .filter(|(value, _)| value != "default")
                                    .collect()
                            }
                            "fast" => model.fast = true,
                            "thinking" if setting["kind"] == "toggle" => model.thinking = true,
                            _ if !choices.is_empty() => model.controls.push(ModelControl {
                                id: id.into(),
                                name: setting["label"].as_str().unwrap_or(id).into(),
                                current: current.into(),
                                choices,
                            }),
                            _ => {}
                        }
                    }
                    Some(model)
                })
                .collect();
            models.insert(*kind, Arc::from(entries));
        }
    }
    Ok(RemoteCatalog {
        providers,
        models,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn remote_catalog_keeps_host_ids_defaults_settings_and_provider_errors() {
        let catalog = remote_catalog_response(&json!({"models":{
            "codex":[{"id":"codex:gpt-test","nativeId":"gpt-test","harness":"codex","name":"Host model","settings":[
                {"id":"reasoningEffort","kind":"select","value":"high","options":[{"value":"low","label":"Low"},{"value":"high","label":"High"}]},
                {"id":"serviceTier","kind":"select","value":"default","options":[{"value":"default","label":"Standard"},{"value":"fast","label":"Fast"}]},
                {"id":"context","label":"Context window","kind":"select","value":"1m","options":[{"value":"1m","label":"1M"}]}]},
                {"id":"codex:gpt-test","name":"Duplicate"}],
            "claude":[{"id":"claude:opus","name":"Opus","settings":[{"id":"fast","kind":"toggle"},{"id":"thinking","kind":"toggle"}]}]
        },"errors":{"antigravity":"Sign in on the host"}})).unwrap();
        assert_eq!(catalog.models[&Kind::Codex].len(), 1);
        let model = &catalog.models[&Kind::Codex][0];
        assert_eq!(model.id, "codex:gpt-test");
        assert!(model.default);
        assert_eq!(model.efforts, ["low", "high"]);
        assert_eq!(model.default_effort.as_deref(), Some("high"));
        assert_eq!(model.tiers, [("fast".into(), "Fast".into())]);
        assert_eq!(model.controls[0].id, "context");
        assert_eq!(model.controls[0].current, "1m");
        assert!(catalog.models[&Kind::Claude][0].fast);
        assert!(catalog.models[&Kind::Claude][0].thinking);
        assert_eq!(catalog.errors[&Kind::Antigravity], "Sign in on the host");
        assert!(!catalog.models.contains_key(&Kind::Unsupported));
    }
}
