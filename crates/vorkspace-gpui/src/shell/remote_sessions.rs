//! Project indexes are fetched on workers; opening a row materializes the chat.
use super::*;
use serde_json::json;
use vorkspace_core::store::RemoteListing;

impl Shell {
    pub(super) fn refresh_remote_projects(&mut self, cx: &mut Context<Self>) {
        let projects: Vec<_> = self
            .workspace
            .vorkspace_projects()
            .into_iter()
            .filter(|path| path.starts_with("remote://"))
            .collect();
        let Some(core) = &self.app_core else {
            return;
        };
        // Rotate so a slow host cannot starve the other project slots.
        for _ in 0..projects.len() {
            let ix = self.remote_refresh_cursor % projects.len();
            self.remote_refresh_cursor = ix + 1;
            let project = &projects[ix];
            if core.update(cx, |core, cx| core.poll_remote_project(project, cx)) {
                break;
            }
        }
    }

    pub(super) fn apply_remote_catalog(&mut self, event: &RemoteCatalog, cx: &mut Context<Self>) {
        let project = &event.project;
        match &event.result {
            Ok(RemoteListing {
                entries, removed, ..
            }) => {
                let mut changed = !removed.is_empty();
                if !removed.is_empty() {
                    let removed: HashSet<_> = removed.iter().copied().collect();
                    self.order.retain(|id| !removed.contains(id));
                }
                for &id in removed {
                    self.session_removal_epoch = self.session_removal_epoch.wrapping_add(1);
                    self.remote_marks.remove(&id);
                    if let Some(view) = self.sessions.get(&id) {
                        view.update(cx, |view, cx| {
                            view.remote_connection_error(
                                "This conversation was deleted on the host.",
                                cx,
                            )
                        });
                    }
                    // An open tab keeps its identity and an explanatory placeholder.
                    if self.workspace.tab_of(id).is_none() {
                        self.session_summaries.remove(&id);
                        self.release_live_session(id, cx);
                    }
                }
                for entry in entries {
                    let summary = &entry.summary;
                    let id = summary.id;
                    if self
                        .session_summaries
                        .get(&id)
                        .is_some_and(|known| known.updated_at > summary.updated_at)
                    {
                        continue;
                    }
                    let marks = (entry.running, entry.needs_input);
                    changed |= self.remote_marks.insert(id, marks) != Some(marks);
                    changed |= if entry.draft {
                        self.draft_sessions.insert(id)
                    } else {
                        self.draft_sessions.remove(&id)
                    };
                    if let Some(view) = self.sessions.get(&id)
                        && !view.read(cx).busy
                        && view.read(cx).summary != *summary
                    {
                        view.update(cx, |view, cx| {
                            view.summary = summary.clone();
                            cx.notify();
                        });
                    }
                    if self.session_summaries.get(&id) != Some(summary) {
                        if self.session_summaries.insert(id, summary.clone()).is_none() {
                            self.order.push(id);
                        }
                        changed = true;
                    }
                }
                if changed && self.workspace.vorkspace_has_project(project) {
                    self.invalidate_chrome(cx);
                    cx.notify();
                }
            }
            Err(error) => {
                // A transient disconnect retains the last host index.
                if let Some(id) = self.workspace.active
                    && let Some(owner) = self.workspace.session_for_pane(id)
                    && let Some(view) = self.sessions.get(&owner)
                    && &view.read(cx).summary.cwd == project
                {
                    view.update(cx, |view, cx| view.remote_connection_error(error, cx));
                }
            }
        }
    }
}

pub(super) struct RemoteCatalog {
    project: String,
    result: Result<RemoteListing, String>,
}
impl gpui::EventEmitter<RemoteCatalog> for app_core::AppCore {}

impl app_core::AppCore {
    fn poll_remote_project(&mut self, project_key: &str, cx: &mut Context<Self>) -> bool {
        use std::time::{Duration, Instant};
        self.remote_refreshes
            .retain(|_, finished| finished.is_none_or(|at| at.elapsed() < Duration::from_secs(30)));
        if self
            .remote_refreshes
            .values()
            .filter(|finish| finish.is_none())
            .count()
            >= 2
            || self
                .remote_refreshes
                .get(project_key)
                .is_some_and(|finished| {
                    finished.is_none_or(|at| at.elapsed() < Duration::from_secs(2))
                })
        {
            return false;
        }
        let project_key = project_key.to_owned();
        self.remote_refreshes.insert(project_key.clone(), None);
        let store = self.store.clone();
        let client = self.remote.clone();
        let requested = project_key.clone();
        let work = cx.background_executor().spawn(async move {
            let (project, baseline) = {
                let db = locked(&store)?;
                let project = db.remote_projects()?.into_values()
                    .find(|p| p.key == project_key)
                    .ok_or("Remote project identity is missing. Open this folder on its machine again.")?;
                let baseline = db.remote_listing_baseline(&project)?;
                (project, baseline)
            };
            let machine = client.machine_for_environment(&project.environment_id)?;
            let response = client.request(
                &machine.id,
                "sessions.list",
                json!({"projectId":project.project_id}),
            )?;
            let values = response.as_array().ok_or("Invalid remote session list")?;
            locked(&store)?.apply_remote_listing(&project, values, &baseline)
        });
        cx.spawn(async move |core, cx| {
            let result = work.await;
            let _ = core.update(cx, |core, cx| {
                core.remote_refreshes
                    .insert(requested.clone(), Some(Instant::now()));
                if result.as_ref().is_ok_and(|listing| listing.changed) {
                    core.catalog_revision = core.catalog_revision.wrapping_add(1);
                }
                cx.emit(RemoteCatalog {
                    project: requested,
                    result,
                });
            });
        })
        .detach();
        true
    }
}
