//! Model records and the process-wide catalog cache, readable without querying a
//! provider. `catalog` fills this; transports and tool titles only read it.
use crate::kind::Kind;
use serde::{Deserialize, Serialize};
use std::{
    sync::{Mutex, OnceLock},
    time::Instant,
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
#[derive(Default)]
pub(crate) struct CatalogCache {
    pub(crate) entries: Vec<(Kind, crate::accounts::Profile, Vec<Model>, Instant)>,
    pub(crate) generation: std::collections::HashMap<(Kind, crate::accounts::Profile), u64>,
}
pub(crate) fn cache() -> &'static Mutex<CatalogCache> {
    static CACHE: OnceLock<Mutex<CatalogCache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
pub fn invalidate(kind: Kind, account: &crate::accounts::Profile) {
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    *cache.generation.entry((kind, account.clone())).or_default() += 1;
    cache
        .entries
        .retain(|(k, p, _, _)| *k != kind || p != account);
}
/// Generation changes whenever credentials invalidate this account's catalog.
pub fn generation(kind: Kind, account: &crate::accounts::Profile) -> u64 {
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .generation
        .get(&(kind, account.clone()))
        .copied()
        .unwrap_or(0)
}
/// A model's catalog name from any account's cached catalog, without querying.
pub fn cached_name(kind: Kind, id: &str) -> Option<String> {
    let cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entries
        .iter()
        .filter(|(k, ..)| *k == kind)
        .find_map(|(_, _, rows, _)| rows.iter().find(|m| m.id == id).map(|m| m.name.clone()))
}
/// Account-scoped catalog for immediate display while a refresh runs.
pub fn cached_models_for(kind: Kind, account: &crate::accounts::Profile) -> Option<Vec<Model>> {
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entries
        .iter()
        .find(|(k, profile, _, _)| *k == kind && profile == account)
        .map(|(_, _, models, _)| models.clone())
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
