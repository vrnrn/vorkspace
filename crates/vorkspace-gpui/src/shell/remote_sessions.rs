//! Project indexes are fetched on workers; opening a row materializes the chat.
use super::*;
use serde_json::json;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use vorkspace_core::store::RemoteListing;

const REFRESH_INTERVAL: Duration = Duration::from_secs(3);
const MINIMUM_REPOLL: Duration = Duration::from_secs(2);
const MAX_IN_FLIGHT: usize = 2;

#[derive(Default)]
pub(super) struct RemotePoll {
    scopes: HashMap<u64, String>,
    projects: VecDeque<String>,
    attempts: HashMap<String, Attempt>,
    clock: Option<gpui::Task<()>>,
}

enum Attempt {
    Active,
    Complete(Instant),
}

impl RemotePoll {
    fn replace_projects(&mut self, projects: Vec<String>) -> bool {
        let wanted: HashSet<_> = projects.iter().map(String::as_str).collect();
        let previous_count = self.projects.len();
        self.projects
            .retain(|project| wanted.contains(project.as_str()));
        let mut changed = self.projects.len() != previous_count;
        let mut known: HashSet<_> = self.projects.iter().cloned().collect();
        for project in projects {
            if known.insert(project.clone()) {
                self.projects.push_back(project);
                changed = true;
            }
        }
        // A closed scope still occupies its slot until the worker settles.
        self.attempts.retain(|project, attempt| {
            matches!(attempt, Attempt::Active) || known.contains(project)
        });
        changed
    }

    fn admit(&mut self, now: Instant) -> Option<String> {
        if self
            .attempts
            .values()
            .filter(|state| matches!(state, Attempt::Active))
            .count()
            >= MAX_IN_FLIGHT
        {
            return None;
        }
        for _ in 0..self.projects.len() {
            self.projects.rotate_left(1);
            let project = self.projects.back().expect("nonempty rotation");
            let ready = match self.attempts.get(project) {
                Some(Attempt::Active) => false,
                Some(Attempt::Complete(at)) => now.saturating_duration_since(*at) >= MINIMUM_REPOLL,
                None => true,
            };
            if ready {
                self.attempts.insert(project.clone(), Attempt::Active);
                return Some(project.clone());
            }
        }
        None
    }

    fn complete(&mut self, project: &str, now: Instant) -> bool {
        if self.projects.iter().any(|visible| visible == project) {
            self.attempts
                .insert(project.to_owned(), Attempt::Complete(now));
            true
        } else {
            self.attempts.remove(project);
            false
        }
    }
}

impl Shell {
    pub(super) fn sync_remote_scope(&self, cx: &mut Context<Self>) {
        if self.window_closing {
            return;
        }
        let Some(core) = &self.app_core else {
            return;
        };
        core.update(cx, |core, cx| {
            core.set_remote_scope(
                self.window_layout,
                Some(&self.workspace.active_vorkspace),
                cx,
            );
        });
    }

    pub(super) fn apply_remote_catalog(&mut self, event: &RemoteCatalog, cx: &mut Context<Self>) {
        let project = &event.project;
        match &event.result {
            Ok(RemoteCatalogUpdate {
                entries,
                removed,
                changed,
            }) => {
                let mut repaint = *changed;
                let retired: HashSet<_> = removed
                    .iter()
                    .map(|(id, _)| *id)
                    .filter(|id| !self.directory.read(cx).rows.contains_key(id))
                    .collect();
                if !retired.is_empty() {
                    self.order.retain(|id| !retired.contains(id));
                }
                for (id, last_record) in removed {
                    if !retired.contains(id) {
                        continue;
                    }
                    self.session_removal_epoch = self.session_removal_epoch.wrapping_add(1);
                    self.remote_marks.remove(id);
                    if let Some(view) = self.sessions.get(id) {
                        view.update(cx, |view, cx| {
                            view.remote_connection_error(
                                "This conversation was deleted on the host.",
                                cx,
                            )
                        });
                    }
                    // An open tab keeps its identity and an explanatory placeholder.
                    if self.workspace.tab_of(*id).is_some() {
                        if let Some(record) = last_record {
                            self.remote_tombstones.insert(*id, record.clone());
                        }
                    } else {
                        self.release_live_session(*id, cx);
                    }
                    self.forget_search_session(*id, cx);
                }
                let mut known: HashSet<_> = self.order.iter().copied().collect();
                for &(id, marks) in entries {
                    let Some(summary) = self
                        .directory
                        .read(cx)
                        .rows
                        .get(&id)
                        .map(|row| &row.summary)
                    else {
                        continue;
                    };
                    self.remote_tombstones.remove(&id);
                    repaint |= self.remote_marks.insert(id, marks) != Some(marks);
                    if let Some(view) = self.sessions.get(&id)
                        && !view.read(cx).busy
                        && view.read(cx).summary.updated_at <= summary.updated_at
                        && view.read(cx).summary != *summary
                    {
                        let summary = summary.clone();
                        view.update(cx, |view, cx| {
                            view.summary = summary;
                            cx.notify();
                        });
                    }
                    if known.insert(id) {
                        self.order.push(id);
                        repaint = true;
                    }
                }
                if repaint && self.workspace.vorkspace_has_project(project) {
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
    result: Result<RemoteCatalogUpdate, String>,
}
impl gpui::EventEmitter<RemoteCatalog> for app_core::AppCore {}

struct RemoteCatalogUpdate {
    entries: Vec<(i64, (bool, bool))>,
    removed: Vec<(i64, Option<SessionRecord>)>,
    changed: bool,
}

impl app_core::AppCore {
    fn record_remote_catalog(
        &mut self,
        listing: RemoteListing,
        cx: &mut Context<Self>,
    ) -> RemoteCatalogUpdate {
        let RemoteListing {
            entries,
            removed,
            changed,
        } = listing;
        let update = self.directory.update(cx, |directory, _| {
            let mut projected = Vec::with_capacity(entries.len());
            let removed = removed
                .into_iter()
                .map(|id| (id, directory.rows.remove(&id)))
                .collect::<Vec<_>>();
            let mut changed = changed || !removed.is_empty();
            for entry in entries {
                let id = entry.summary.id;
                let before = directory.rows.get(&id);
                if before.is_some_and(|row| row.summary.updated_at > entry.summary.updated_at) {
                    continue;
                }
                if before.is_none_or(|row| row.summary != entry.summary || row.draft != entry.draft)
                {
                    let checkout = before.and_then(|row| row.checkout.clone());
                    directory.rows.insert(
                        id,
                        SessionRecord {
                            summary: entry.summary,
                            checkout,
                            draft: entry.draft,
                        },
                    );
                    changed = true;
                }
                projected.push((id, (entry.running, entry.needs_input)));
            }
            RemoteCatalogUpdate {
                entries: projected,
                removed,
                changed,
            }
        });
        if update.changed {
            self.sync_inbox_links(
                update.entries.iter().map(|(id, _)| *id),
                update.removed.iter().map(|(id, _)| *id),
                cx,
            );
            self.catalog_revision = self.catalog_revision.wrapping_add(1);
        }
        update
    }

    pub(super) fn set_remote_scope(
        &mut self,
        window: u64,
        vorkspace: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if self.remote_poll.scopes.get(&window).map(String::as_str) == vorkspace {
            return;
        }
        if let Some(vorkspace) = vorkspace {
            self.remote_poll.scopes.insert(window, vorkspace.to_owned());
        } else {
            self.remote_poll.scopes.remove(&window);
        }
        self.reconcile_remote_projects(cx);
    }

    pub(super) fn reconcile_remote_projects(&mut self, cx: &mut Context<Self>) {
        let active: HashSet<_> = self
            .remote_poll
            .scopes
            .values()
            .map(String::as_str)
            .collect();
        let shared = self.layouts.shared.as_deref();
        let projects = self
            .vorkspaces
            .iter()
            .filter(|def| active.contains(def.id.as_str()))
            .flat_map(|def| &def.projects)
            .filter(|path| {
                path.starts_with("remote://")
                    && shared.is_none_or(|shared| shared.is_project_available(path))
            })
            .cloned()
            .collect();
        let changed = self.remote_poll.replace_projects(projects);
        if self.remote_poll.projects.is_empty() {
            self.remote_poll.clock = None;
        } else if !cx.is_test() {
            if changed {
                self.poll_remote_projects(cx);
            }
            if self.remote_poll.clock.is_none() {
                self.remote_poll.clock = Some(cx.spawn(async move |core, cx| {
                    loop {
                        cx.background_executor().timer(REFRESH_INTERVAL).await;
                        if core
                            .update(cx, |core, cx| core.poll_remote_projects(cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                }));
            }
        }
    }

    fn poll_remote_projects(&mut self, cx: &mut Context<Self>) {
        let Some(project_key) = self.remote_poll.admit(Instant::now()) else {
            return;
        };
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
                json!({"projectId": project.project_id, "includeGit": false}),
            )?;
            let values = response.as_array().ok_or("Invalid remote session list")?;
            locked(&store)?.apply_remote_listing(&project, values, &baseline)
        });
        cx.spawn(async move |core, cx| {
            let result = work.await;
            let _ = core.update(cx, |core, cx| {
                let observed = core.remote_poll.complete(&requested, Instant::now());
                let result = result.map(|listing| core.record_remote_catalog(listing, cx));
                if observed || result.is_ok() {
                    cx.emit(RemoteCatalog {
                        project: requested,
                        result,
                    });
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_poll_rotates_projects_and_holds_slots_until_retired_reads_finish() {
        let mut poll = RemotePoll::default();
        let now = Instant::now();
        assert!(poll.replace_projects(vec!["a".into(), "b".into(), "a".into(), "c".into()]));
        assert_eq!(poll.admit(now).as_deref(), Some("a"));
        assert_eq!(poll.admit(now).as_deref(), Some("b"));
        assert_eq!(poll.admit(now), None);

        assert!(poll.replace_projects(vec!["c".into(), "b".into()]));
        assert_eq!(poll.admit(now), None);
        assert!(!poll.complete("a", now));
        assert_eq!(poll.admit(now).as_deref(), Some("c"));
        assert!(poll.complete("b", now));
        assert_eq!(poll.admit(now), None);
        assert_eq!(poll.admit(now + MINIMUM_REPOLL).as_deref(), Some("b"));
    }
}
