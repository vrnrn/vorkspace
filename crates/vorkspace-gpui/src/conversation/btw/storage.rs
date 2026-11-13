//! Side-question wire format and ordered, revision-checked saves.
use super::{Database, Kind, Row, locked};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use vorkspace_core::store::Store;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Thread {
    pub(super) id: String,
    pub(super) source_turn: usize,
    pub(super) snapshot: String,
    pub(super) provider: String,
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) cwd: String,
    #[serde(default)]
    pub(super) account: vorkspace_core::accounts::Profile,
    #[serde(default)]
    pub(super) effort: Option<String>,
    #[serde(default)]
    pub(super) model_settings: HashMap<String, String>,
    #[serde(skip)]
    pub(super) environment: HashMap<String, String>,
    pub(super) rows: Vec<Row>,
    pub(super) draft: String,
    pub(super) status: String,
    pub(super) error: Option<String>,
    #[serde(default, rename = "desktop")]
    pub(super) original: Option<serde_json::Value>,
}

impl Thread {
    pub(super) fn empty(&self) -> bool {
        self.rows.is_empty() && self.draft.trim().is_empty()
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Saved {
    pub(super) revision: u64,
    #[serde(default)]
    pub(super) active: Option<String>,
    pub(super) threads: Vec<Thread>,
}

#[derive(Deserialize)]
struct SavedRevision {
    revision: u64,
}

#[derive(Default)]
enum Revision {
    #[default]
    Unloaded,
    Loaded(Option<u64>),
}

#[derive(Clone, Default)]
pub(super) struct Writes(Arc<Mutex<Revision>>);

impl Writes {
    pub(super) fn read(&self, store: Database, session: i64) -> Result<Saved, String> {
        let mut revision = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let store = locked(&store)?;
        let value = store.setting(&format!("btw:{session}"))?;
        let saved = if let Some(value) = &value {
            drop(store);
            decode(value)?
        } else {
            import(&store, session)?
        };
        *revision = Revision::Loaded(value.is_some().then_some(saved.revision));
        Ok(saved)
    }

    pub(super) fn publish(
        &self,
        store: Database,
        session: i64,
        snapshot: Saved,
    ) -> Result<(), String> {
        let key = format!("btw:{session}");
        let mut revision = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Revision::Loaded(published) = &mut *revision else {
            return Err("The saved side questions could not be loaded.".into());
        };
        if published.is_some_and(|published| snapshot.revision <= published) {
            return Ok(());
        }
        let value = serde_json::to_string(&snapshot).map_err(|error| error.to_string())?;
        let store = locked(&store)?;
        let existing = store.setting(&key)?;
        let current = existing
            .as_deref()
            .map(serde_json::from_str::<SavedRevision>)
            .transpose()
            .map_err(|error| error.to_string())?
            .map(|saved| saved.revision);
        if current != *published {
            return Err("Side questions changed in another window.".into());
        }
        store.compare_swap_setting(&key, existing.as_deref(), &value)?;
        *published = Some(snapshot.revision);
        Ok(())
    }

    /// Before hydration, the view can only create threads. Merge those drafts
    /// against durable history when quit cannot wait for a view publication.
    pub(super) fn append_new_threads(
        &self,
        store: Database,
        session: i64,
        mut pending: Saved,
    ) -> Result<(), String> {
        pending.threads.retain(|thread| !thread.empty());
        if pending.threads.is_empty() {
            return Ok(());
        }
        let key = format!("btw:{session}");
        let mut revision = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let store = locked(&store)?;
        let existing = store.setting(&key)?;
        let mut saved = match existing.as_deref() {
            Some(value) => decode(value)?,
            None => import(&store, session)?,
        };
        pending
            .threads
            .retain(|thread| !saved.threads.iter().any(|old| old.id == thread.id));
        if pending.threads.is_empty() {
            return Ok(());
        }
        saved.active = pending
            .active
            .filter(|id| pending.threads.iter().any(|thread| &thread.id == id))
            .or(saved.active);
        saved.threads.append(&mut pending.threads);
        saved.revision = saved.revision.saturating_add(1).max(pending.revision);
        let value = serde_json::to_string(&saved).map_err(|error| error.to_string())?;
        store.compare_swap_setting(&key, existing.as_deref(), &value)?;
        *revision = Revision::Loaded(Some(saved.revision));
        Ok(())
    }
}

fn decode(value: &str) -> Result<Saved, String> {
    serde_json::from_str(value)
        .map_err(|error| format!("Could not restore side questions: {error}"))
}

fn import(store: &Store, session: i64) -> Result<Saved, String> {
    let imported = store.saved_side_threads(session)?;
    let accounts = store.provider_accounts()?;
    let root = store.data_dir().map(std::path::Path::to_path_buf);
    let threads = imported
        .into_iter()
        .map(|thread| {
            let kind = Kind::from_id(&thread.provider);
            let account = vorkspace_core::accounts::Profile {
                id: accounts.selected(kind, &thread.cwd),
                root: root.clone(),
            };
            let effort = [
                "effort",
                "reasoning",
                "reasoningEffort",
                "reasoning_effort",
                "variant",
            ]
            .into_iter()
            .find_map(|key| thread.model_settings.get(key).cloned());
            Thread {
                id: thread.id,
                source_turn: thread.source_turn,
                snapshot: thread.snapshot,
                provider: thread.provider,
                model: thread.model,
                cwd: thread.cwd,
                account,
                effort,
                model_settings: thread.model_settings,
                environment: HashMap::new(),
                rows: thread.rows,
                draft: String::new(),
                status: match thread.status.as_str() {
                    "ready" => "completed",
                    "running" => "stopped",
                    status => status,
                }
                .into(),
                error: thread.error,
                original: Some(thread.original),
            }
        })
        .collect();
    Ok(Saved {
        revision: 0,
        active: None,
        threads,
    })
}
