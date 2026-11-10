//! Model records and the process-wide catalog cache, readable without querying a
//! provider. `catalog` fills this; transports and tool titles only read it.
use crate::{accounts::Profile, kind::Kind};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, PoisonError},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    /// Concrete id behind a moving provider alias, retained for comparisons.
    #[serde(default)]
    pub alias_target: Option<String>,
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
    pub fast: bool,
    pub thinking: bool,
    pub default: bool,
    pub tiers: Vec<(String, String)>,
    #[serde(default)]
    pub controls: Vec<ModelControl>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelControl {
    pub id: String,
    pub name: String,
    pub current: String,
    pub choices: Vec<(String, String)>,
}

pub type ModelRows = Arc<[Model]>;

impl Model {
    pub(crate) fn estimated_bytes(&self) -> usize {
        let pairs = |rows: &Vec<(String, String)>| {
            rows.capacity() * std::mem::size_of::<(String, String)>()
                + rows
                    .iter()
                    .map(|(id, name)| id.capacity() + name.capacity())
                    .sum::<usize>()
        };
        std::mem::size_of::<Self>()
            + self.id.capacity()
            + self.name.capacity()
            + self.alias_target.as_ref().map_or(0, String::capacity)
            + self.default_effort.as_ref().map_or(0, String::capacity)
            + self.efforts.capacity() * std::mem::size_of::<String>()
            + self.efforts.iter().map(String::capacity).sum::<usize>()
            + pairs(&self.tiers)
            + self.controls.capacity() * std::mem::size_of::<ModelControl>()
            + self
                .controls
                .iter()
                .map(|control| {
                    control.id.capacity()
                        + control.name.capacity()
                        + control.current.capacity()
                        + pairs(&control.choices)
                })
                .sum::<usize>()
    }
}
const FRESHNESS: Duration = Duration::from_secs(300);
const CACHE_BYTES: usize = 16 * 1024 * 1024;
type Key = (Kind, Profile);
type Load = Arc<OnceLock<Result<ModelRows, String>>>;

struct Ready {
    rows: ModelRows,
    at: Instant,
    bytes: usize,
}

#[derive(Default)]
struct Entry {
    generation: u64,
    ready: Option<Ready>,
    loading: Option<Load>,
}

#[derive(Default)]
struct CatalogCache {
    entries: HashMap<Key, Entry>,
}

fn cache() -> &'static Mutex<CatalogCache> {
    static CACHE: OnceLock<Mutex<CatalogCache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

impl CatalogCache {
    fn trim(&mut self) {
        let mut bytes = self
            .entries
            .values()
            .filter_map(|entry| entry.ready.as_ref())
            .map(|ready| ready.bytes)
            .sum::<usize>();
        if bytes <= CACHE_BYTES {
            return;
        }
        let mut retained = self
            .entries
            .iter()
            .filter_map(|(key, entry)| {
                entry
                    .ready
                    .as_ref()
                    .map(|ready| (key.clone(), ready.at, ready.bytes))
            })
            .collect::<Vec<_>>();
        retained.sort_unstable_by_key(|(_, at, _)| *at);
        for (key, _, weight) in retained {
            if bytes <= CACHE_BYTES {
                break;
            }
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.ready = None;
                bytes -= weight;
            }
        }
        self.entries.retain(|_, entry| {
            entry.generation != 0 || entry.loading.is_some() || entry.ready.is_some()
        });
    }
}

pub(crate) fn discover(
    kind: Kind,
    account: &Profile,
    read: impl FnOnce() -> Result<Vec<Model>, String>,
) -> Result<ModelRows, String> {
    let key = (kind, account.clone());
    let (generation, loading) = {
        let mut cache = cache().lock().unwrap_or_else(PoisonError::into_inner);
        let entry = cache.entries.entry(key.clone()).or_default();
        if let Some(ready) = entry
            .ready
            .as_ref()
            .filter(|ready| ready.at.elapsed() < FRESHNESS)
        {
            return Ok(ready.rows.clone());
        }
        (
            entry.generation,
            entry.loading.get_or_insert_default().clone(),
        )
    };
    let result = loading.get_or_init(|| read().map(Into::into)).clone();
    let mut cache = cache().lock().unwrap_or_else(PoisonError::into_inner);
    let Some(entry) = cache.entries.get_mut(&key) else {
        return result;
    };
    if entry.generation != generation {
        return Err("Provider account changed while loading models".into());
    }
    if entry
        .loading
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, &loading))
    {
        entry.loading = None;
        if let Ok(rows) = &result {
            let bytes = rows.iter().map(Model::estimated_bytes).sum();
            entry.ready = (bytes <= CACHE_BYTES).then(|| Ready {
                rows: rows.clone(),
                at: Instant::now(),
                bytes,
            });
        }
        if entry.generation == 0 && entry.ready.is_none() {
            cache.entries.remove(&key);
        } else if result.is_ok() {
            cache.trim();
        }
    }
    result
}

pub fn invalidate(kind: Kind, account: &Profile) {
    let mut cache = cache().lock().unwrap_or_else(PoisonError::into_inner);
    let entry = cache.entries.entry((kind, account.clone())).or_default();
    entry.generation = entry.generation.wrapping_add(1);
    entry.ready = None;
    entry.loading = None;
}

/// Generation changes whenever credentials invalidate this account's catalog.
pub fn generation(kind: Kind, account: &Profile) -> u64 {
    cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entries
        .get(&(kind, account.clone()))
        .map_or(0, |entry| entry.generation)
}

/// A model's catalog name from any account's cached catalog, without querying.
pub fn cached_name(kind: Kind, id: &str) -> Option<String> {
    cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entries
        .iter()
        .filter(|((provider, _), _)| *provider == kind)
        .filter_map(|(_, entry)| {
            let ready = entry.ready.as_ref()?;
            ready
                .rows
                .iter()
                .find(|model| model.id == id)
                .map(|model| (ready.at, model))
        })
        .max_by_key(|(at, _)| *at)
        .map(|(_, model)| model.name.clone())
}

/// Account-scoped catalog for immediate display while a refresh runs.
pub fn cached_models_for(kind: Kind, account: &Profile) -> Option<ModelRows> {
    cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entries
        .get(&(kind, account.clone()))?
        .ready
        .as_ref()
        .map(|ready| ready.rows.clone())
}

pub(crate) fn model(id: &str, name: &str) -> Model {
    Model {
        id: id.into(),
        name: name.into(),
        alias_target: None,
        efforts: Vec::new(),
        default_effort: None,
        fast: false,
        thinking: false,
        default: false,
        tiers: Vec::new(),
        controls: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_catalog_display_is_scoped_and_cleared_on_invalidation() {
        let account = Profile {
            id: "catalog-cache-test".into(),
            root: None,
        };
        let other = Profile {
            id: "catalog-cache-other".into(),
            root: None,
        };
        let rows = discover(Kind::Claude, &account, || {
            Ok(vec![model("opus", "Opus test")])
        })
        .unwrap();
        {
            let mut cache = cache().lock().unwrap();
            let entry = cache
                .entries
                .get_mut(&(Kind::Claude, account.clone()))
                .unwrap();
            entry.ready.as_mut().unwrap().at = Instant::now() - FRESHNESS;
        }
        assert!(Arc::ptr_eq(
            &cached_models_for(Kind::Claude, &account).unwrap(),
            &rows
        ));
        assert!(cached_models_for(Kind::Claude, &other).is_none());
        assert!(cached_models_for(Kind::Codex, &account).is_none());
        invalidate(Kind::Claude, &account);
        assert!(cached_models_for(Kind::Claude, &account).is_none());
    }

    #[test]
    fn old_credentials_cannot_publish_a_model_catalog_after_replacement() {
        let account = Profile {
            id: "catalog-epoch-test".into(),
            root: None,
        };
        let (began, ready) = std::sync::mpsc::sync_channel(0);
        let (release, resume) = std::sync::mpsc::sync_channel(0);
        std::thread::scope(|scope| {
            let account = &account;
            let old = scope.spawn(move || {
                discover(Kind::Codex, account, || {
                    began.send(()).unwrap();
                    resume.recv().unwrap();
                    Ok(vec![model("old", "Old account model")])
                })
            });
            ready.recv().unwrap();
            invalidate(Kind::Codex, account);
            let latest = discover(Kind::Codex, account, || {
                Ok(vec![model("new", "New account model")])
            })
            .unwrap();
            release.send(()).unwrap();
            assert!(old.join().unwrap().is_err());
            assert!(Arc::ptr_eq(
                &cached_models_for(Kind::Codex, account).unwrap(),
                &latest
            ));
        });
    }
}
