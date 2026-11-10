//! One committed Inbox preference snapshot per data store.
use super::*;
use std::collections::HashMap;

#[derive(Default)]
struct Installed(HashMap<usize, Entity<SavedPreferences>>);
impl gpui::Global for Installed {}

pub(super) struct SavedPreferences {
    store: Database,
    pub(super) state: ViewState,
}

impl SavedPreferences {
    pub(super) fn get(store: &Database, cx: &mut App) -> Entity<Self> {
        let key = std::sync::Arc::as_ptr(store) as usize;
        if let Some(saved) = cx
            .try_global::<Installed>()
            .and_then(|owners| owners.0.get(&key))
        {
            return saved.clone();
        }
        let state = locked(store)
            .map(|db| ViewState::load(&db))
            .unwrap_or_default();
        let owner = cx.new(|_| Self {
            store: store.clone(),
            state,
        });
        cx.default_global::<Installed>()
            .0
            .insert(key, owner.clone());
        owner
    }

    pub(super) fn update_store(
        &mut self,
        edit: impl FnOnce(&vorkspace_core::store::Store) -> Result<(), String>,
        cx: &mut Context<Self>,
    ) -> Result<ViewState, String> {
        let store = locked(&self.store)?;
        edit(&store)?;
        let state = ViewState::load(&store);
        if self.state != state {
            self.state = state;
            cx.notify();
        }
        Ok(self.state.clone())
    }

    pub(super) fn save_edits(
        &mut self,
        before: &ViewState,
        next: &ViewState,
        save_layout: bool,
        save_source: bool,
        cx: &mut Context<Self>,
    ) -> Result<ViewState, String> {
        self.update_store(
            |store| {
                let current = ViewState::load(store);
                let mut merged = current.clone();
                if save_layout {
                    if before.source == next.source && before.filters != next.filters {
                        let provider = next.source.clone();
                        let mut filters = current.filters_for(&provider).clone();
                        let (before, next) = (&before.filters, &next.filters);
                        if before.assigned_to_me != next.assigned_to_me {
                            filters.assigned_to_me = next.assigned_to_me;
                        }
                        if before.time != next.time {
                            filters.time.clone_from(&next.time);
                        }
                        apply_membership(
                            &mut filters.hidden_projects,
                            &before.hidden_projects,
                            &next.hidden_projects,
                        );
                        apply_membership(
                            &mut filters.hidden_kinds,
                            &before.hidden_kinds,
                            &next.hidden_kinds,
                        );
                        for (current, before, next) in [
                            (
                                &mut filters.status.open,
                                before.status.open,
                                next.status.open,
                            ),
                            (
                                &mut filters.status.closed,
                                before.status.closed,
                                next.status.closed,
                            ),
                            (
                                &mut filters.status.draft,
                                before.status.draft,
                                next.status.draft,
                            ),
                            (
                                &mut filters.status.merged,
                                before.status.merged,
                                next.status.merged,
                            ),
                        ] {
                            if before != next {
                                *current = next;
                            }
                        }
                        merged.provider_filters.insert(provider, filters);
                    }
                    if before.list_width != next.list_width {
                        merged.list_width = next.list_width;
                    }
                    apply_membership(
                        &mut merged.hidden_jira_projects,
                        &before.hidden_jira_projects,
                        &next.hidden_jira_projects,
                    );
                }
                if save_source && before.source != next.source {
                    merged.source.clone_from(&next.source);
                    merged.normalize_source();
                }
                for (key, revision) in &next.seen.items {
                    if before.seen.items.get(key) != Some(revision) {
                        let seen = merged.seen.items.entry(key.clone()).or_default();
                        *seen = (*seen).max(*revision);
                    }
                }
                merged.seen.seeded |= next.seen.seeded;
                if merged != current {
                    merged.save(store)?;
                }
                Ok(())
            },
            cx,
        )
    }
}

fn apply_membership(current: &mut Vec<String>, before: &[String], next: &[String]) {
    current.retain(|id| !before.contains(id) || next.contains(id));
    for id in next {
        if !before.contains(id) && !current.contains(id) {
            current.push(id.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::sync::{Arc, Mutex};

    #[gpui::test]
    fn stale_connector_edits_keep_the_peers_filters_and_read_marks(cx: &mut TestAppContext) {
        let store = Arc::new(Mutex::new(
            vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        let owner = cx.update(|cx| SavedPreferences::get(&store, cx));
        let baseline = owner.read_with(cx, |saved, _| saved.state.clone());
        let mut a = baseline.clone();
        a.filters.hidden_projects.push("org/first".into());
        a.seen.items.insert("first-item".into(), 11);
        let mut b = baseline.clone();
        b.filters.hidden_projects.push("org/second".into());
        b.filters.status.closed = true;
        b.seen.items.insert("second-item".into(), 17);
        owner.update(cx, |owner, cx| {
            owner.save_edits(&baseline, &a, true, true, cx).unwrap();
            owner.save_edits(&baseline, &b, true, true, cx).unwrap();
        });
        let db = locked(&store).unwrap();
        let saved = ViewState::load(&db);
        let filters = saved.filters_for("github");
        assert_eq!(filters.hidden_projects, ["org/first", "org/second"]);
        assert!(filters.status.closed);
        assert_eq!(saved.seen.items.get("first-item"), Some(&11));
        assert_eq!(saved.seen.items.get("second-item"), Some(&17));
    }
}
