//! Side-question wire format and ordered, revision-checked saves.
use super::{Database, Kind, Row, locked};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

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
    #[serde(default)]
    pub(super) desktop: Option<serde_json::Value>,
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

#[derive(Clone, Default)]
pub(super) struct Writes(Arc<Mutex<Option<u64>>>);

impl Writes {
    pub(super) fn read(&self, store: Database, session: i64) -> Result<Saved, String> {
        let store = locked(&store)?;
        if let Some(value) = store.setting(&format!("btw:{session}"))? {
            drop(store);
            let saved: Saved = serde_json::from_str(&value)
                .map_err(|error| format!("Could not restore side questions: {error}"))?;
            *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(saved.revision);
            return Ok(saved);
        }
        let imported = store.desktop_side_threads(session)?;
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
                    desktop: Some(thread.original),
                }
            })
            .collect();
        Ok(Saved {
            revision: 0,
            active: None,
            threads,
        })
    }

    pub(super) fn publish(
        &self,
        store: Database,
        session: i64,
        snapshot: Saved,
    ) -> Result<(), String> {
        let key = format!("btw:{session}");
        let revision = snapshot.revision;
        let mut published = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if published.is_some_and(|published| revision <= published) {
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
        *published = Some(revision);
        Ok(())
    }
}
