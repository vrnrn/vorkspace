//! Owns connector discovery once for the data store.
use super::{saved_preferences::SavedPreferences, *};
use gpui::{EntityId, Task};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

const DETAIL_INTERVAL: Duration = Duration::from_secs(30);
const VISIBLE_INTERVAL: Duration = Duration::from_secs(120);
const BACKGROUND_INTERVAL: Duration = Duration::from_secs(300);
const OWN_ACTIVITY_RETENTION: Duration = Duration::from_secs(600);

pub(crate) enum FeedEvent {
    DetailTick,
    Discovered {
        projects: Vec<model::NotificationProject>,
        changed: Vec<Item>,
        appeared: Vec<Item>,
    },
}

struct Poll {
    generation: u64,
    scope: String,
    connection_revision: u64,
}

pub(crate) struct InboxFeed {
    service: Service,
    preferences: Entity<SavedPreferences>,
    roots: HashMap<EntityId, Vec<PathBuf>>,
    projects: Vec<PathBuf>,
    active_pages: HashSet<EntityId>,
    clock: Option<Task<()>>,
    pub(super) snapshot: Rc<Snapshot>,
    pub(super) connection_revision: u64,
    connecting: HashSet<String>,
    generation: u64,
    pending: Option<Poll>,
    refresh_after_pending: bool,
    last_scope: Option<String>,
    last_refresh: Option<Instant>,
    notifications: model::NotificationTracker,
    self_activity: HashMap<String, (i64, Instant)>,
}

impl EventEmitter<FeedEvent> for InboxFeed {}

impl InboxFeed {
    pub(crate) fn new(store: &Database, data_dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let preferences = SavedPreferences::get(store, cx);
        cx.observe(&preferences, |this, _, cx| this.preferences_changed(cx))
            .detach();
        Self {
            service: Service {
                cwd: data_dir.clone(),
                data_dir,
            },
            preferences,
            roots: HashMap::new(),
            projects: Vec::new(),
            active_pages: HashSet::new(),
            clock: None,
            snapshot: Rc::new(Snapshot::default()),
            connection_revision: 0,
            connecting: HashSet::new(),
            generation: 0,
            pending: None,
            refresh_after_pending: false,
            last_scope: None,
            last_refresh: None,
            notifications: model::NotificationTracker::default(),
            self_activity: HashMap::new(),
        }
    }

    pub(super) fn set_scope(
        &mut self,
        root: EntityId,
        projects: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.roots.get(&root) == Some(&projects) {
            return;
        }
        let first = self.roots.is_empty();
        self.roots.insert(root, projects);
        let changed = self.update_projects();
        if first && !cfg!(test) {
            self.clock = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(DETAIL_INTERVAL).await;
                    if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                        break;
                    }
                }
            }));
        }
        if first || changed {
            self.schedule_scope_check(cx);
        }
    }

    pub(super) fn release(&mut self, entity: EntityId, cx: &mut Context<Self>) {
        self.active_pages.remove(&entity);
        if self.roots.remove(&entity).is_none() {
            return;
        }
        let changed = self.update_projects();
        if self.roots.is_empty() {
            self.clock = None;
            self.last_scope = None;
            self.snapshot = Rc::new(Snapshot::default());
        } else if changed {
            self.schedule_scope_check(cx);
        }
    }

    fn update_projects(&mut self) -> bool {
        let mut paths = BTreeMap::new();
        for path in self.roots.values().flatten() {
            let key = vorkspace_core::files::path_key(&path.to_string_lossy());
            let stored = paths.entry(key).or_insert(path);
            *stored = std::cmp::min(*stored, path);
        }
        let projects = paths.into_values().cloned().collect::<Vec<_>>();
        if self.projects == projects {
            return false;
        }
        self.projects = projects;
        true
    }

    pub(super) fn set_active(&mut self, entity: EntityId, active: bool) {
        if active {
            self.active_pages.insert(entity);
        } else {
            self.active_pages.remove(&entity);
        }
    }

    pub(crate) fn connectors(&self, cx: &App) -> model::Connectors {
        self.preferences.read(cx).state.connectors.clone()
    }

    pub(super) fn loading(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn refresh_if_stale(&mut self, cx: &mut Context<Self>) {
        self.refresh_at_interval(VISIBLE_INTERVAL, cx);
    }

    fn refresh_at_interval(&mut self, interval: Duration, cx: &mut Context<Self>) {
        if self.last_refresh.is_some_and(|at| at.elapsed() >= interval) {
            self.refresh(false, cx);
        }
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let interval = if self.active_pages.is_empty() {
            BACKGROUND_INTERVAL
        } else {
            cx.emit(FeedEvent::DetailTick);
            VISIBLE_INTERVAL
        };
        self.refresh_at_interval(interval, cx);
    }

    pub(super) fn preferences_changed(&mut self, cx: &mut Context<Self>) {
        let state = &self.preferences.read(cx).state;
        let enabled = |id: &str| state.connectors.enabled(id);
        if self
            .snapshot
            .items
            .iter()
            .any(|item| !enabled(&item.provider))
            || self.snapshot.connections.keys().any(|id| !enabled(id))
            || self.snapshot.errors.keys().any(|id| !enabled(id))
            || !enabled("jira") && !self.snapshot.jira_projects.is_empty()
        {
            let snapshot = Rc::make_mut(&mut self.snapshot);
            snapshot.items.retain(|item| enabled(&item.provider));
            snapshot.connections.retain(|id, _| enabled(id));
            snapshot.errors.retain(|id, _| enabled(id));
            if !enabled("jira") {
                snapshot.jira_projects.clear();
            }
            cx.notify();
        }
        self.schedule_scope_check(cx);
    }

    fn schedule_scope_check(&self, cx: &mut Context<Self>) {
        if cfg!(test) || self.roots.is_empty() {
            return;
        }
        // Read the union at delivery so simultaneous window restores coalesce.
        let feed = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = feed.update(cx, |this, cx| {
                let scope = this.preferences.read(cx).state.fetch_scope(&this.projects);
                if this.last_scope.as_ref() != Some(&scope) {
                    this.refresh(false, cx);
                }
            });
        });
    }

    /// A mutation requests a fresh observation even if an older poll is running.
    pub(super) fn refresh(&mut self, after_pending: bool, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let Some(generation) = self.begin_poll(after_pending, cx) else {
            return;
        };
        let state = self.preferences.read(cx).state.clone();
        if state.connectors.enabled_providers().next().is_none() {
            self.finish(generation, Snapshot::default(), cx);
            return;
        }
        let projects = self.projects.clone();
        let service = self.service.clone();
        let work = cx
            .background_executor()
            .spawn(async move { service.list(&projects, &state) });
        cx.spawn(async move |this, cx| {
            let snapshot = work.await;
            let _ = this.update(cx, |this, cx| this.finish(generation, snapshot, cx));
        })
        .detach();
        cx.notify();
    }

    fn begin_poll(&mut self, after_pending: bool, cx: &App) -> Option<u64> {
        if self.roots.is_empty() {
            return None;
        }
        if self.pending.is_some() || !self.connecting.is_empty() {
            self.refresh_after_pending |= after_pending;
            return None;
        }
        let scope = self.preferences.read(cx).state.fetch_scope(&self.projects);
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.pending = Some(Poll {
            generation,
            scope: scope.clone(),
            connection_revision: self.connection_revision,
        });
        self.refresh_after_pending = false;
        self.last_scope = Some(scope);
        self.last_refresh = Some(Instant::now());
        Some(generation)
    }

    fn finish(&mut self, generation: u64, mut snapshot: Snapshot, cx: &mut Context<Self>) {
        let Some(poll) = self.pending.take_if(|poll| poll.generation == generation) else {
            return;
        };
        let saved = self.preferences.read(cx).state.clone();
        let follow_up = std::mem::take(&mut self.refresh_after_pending);
        if self.roots.is_empty()
            || poll.scope != saved.fetch_scope(&self.projects)
            || poll.connection_revision != self.connection_revision
        {
            self.refresh(false, cx);
            cx.notify();
            return;
        }
        let observation = self.notifications.observe_snapshot(&snapshot, poll.scope);
        self.self_activity
            .retain(|_, (_, at)| at.elapsed() < OWN_ACTIVITY_RETENTION);
        let mut next = saved.clone();
        let changed: Vec<_> = observation
            .changed
            .into_iter()
            .filter(|item| {
                let key = item.detail_key();
                let expected = self.self_activity.get(&key).map(|(revision, _)| *revision);
                if expected.is_some_and(|revision| item.updated_ms() >= revision) {
                    self.self_activity.remove(&key);
                }
                if expected == Some(item.updated_ms()) {
                    next.seen.mark(item);
                    false
                } else {
                    saved
                        .filters_for(&item.provider)
                        .matches(item, &item.provider, "")
                }
            })
            .collect();
        // Keep a provider's last usable list across a temporary outage.
        for provider in snapshot.errors.keys() {
            snapshot.items.extend(
                self.snapshot
                    .items
                    .iter()
                    .filter(|item| &item.provider == provider)
                    .cloned(),
            );
            if provider == "jira" {
                snapshot
                    .jira_projects
                    .clone_from(&self.snapshot.jira_projects);
            }
        }
        if follow_up {
            let old = self
                .snapshot
                .items
                .iter()
                .map(|item| (item.key(), item))
                .collect::<HashMap<_, _>>();
            for item in &mut snapshot.items {
                if let Some(latest) = old
                    .get(&item.key())
                    .filter(|latest| latest.updated_ms() > item.updated_ms())
                {
                    item.clone_from(latest);
                }
            }
        }
        snapshot
            .items
            .sort_by_cached_key(|item| (std::cmp::Reverse(item.updated_ms()), item.title.clone()));
        next.seen.seed(&snapshot.items);
        let changed_list = *self.snapshot != snapshot;
        if changed_list {
            self.snapshot = Rc::new(snapshot);
        }
        if saved.seen != next.seen {
            let _ = self.preferences.update(cx, |preferences, cx| {
                preferences.save_edits(&saved, &next, false, false, cx)
            });
        }
        if changed_list || !observation.appeared.is_empty() || !changed.is_empty() {
            let mut projects = BTreeMap::<String, model::NotificationProject>::new();
            for item in &self.snapshot.items {
                let project = item.notification_project();
                let old = projects
                    .entry(project.id.clone())
                    .or_insert_with(|| project.clone());
                for path in project.paths {
                    if !old.paths.contains(&path) {
                        old.paths.push(path);
                    }
                }
            }
            cx.emit(FeedEvent::Discovered {
                projects: projects.into_values().collect(),
                changed,
                appeared: observation.appeared,
            });
        }
        if follow_up {
            self.refresh(false, cx);
        }
        cx.notify();
    }

    pub(super) fn note_mutation(&mut self, key: String, revision: i64) {
        // A receipt without a server revision cannot identify our activity.
        if revision > 0
            && self
                .self_activity
                .get(&key)
                .is_none_or(|(old, _)| revision >= *old)
        {
            self.self_activity.insert(key, (revision, Instant::now()));
        }
    }

    pub(super) fn update_item(&mut self, mut item: Item, cx: &mut Context<Self>) {
        let key = item.detail_key();
        if let Some(index) = self
            .snapshot
            .items
            .iter()
            .position(|old| old.detail_key() == key && old.updated_ms() <= item.updated_ms())
        {
            let old = &self.snapshot.items[index];
            item.project_path.clone_from(&old.project_path);
            item.other_project_paths
                .clone_from(&old.other_project_paths);
            if old != &item {
                Rc::make_mut(&mut self.snapshot).items[index] = item;
                cx.notify();
            }
        }
    }

    pub(super) fn begin_connection(
        &mut self,
        provider: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.preferences.read(cx).state.connectors.enabled(provider) {
            return Err("This connector is disabled in Settings.".into());
        }
        if !self.connecting.insert(provider.into()) {
            return Err("This connector is already being updated in another window.".into());
        }
        self.connection_revision = self.connection_revision.wrapping_add(1);
        cx.notify();
        Ok(())
    }

    pub(super) fn finish_connection(
        &mut self,
        provider: &str,
        status: Option<model::Status>,
        cx: &mut Context<Self>,
    ) {
        self.connecting.remove(provider);
        if let Some(status) = status
            && (provider != "github" || self.snapshot.connections.get(provider) != Some(&status))
        {
            self.notifications.reset_provider(provider);
            let snapshot = Rc::make_mut(&mut self.snapshot);
            snapshot.items.retain(|item| item.provider != provider);
            snapshot.errors.remove(provider);
            if provider == "jira" {
                snapshot.jira_projects.clear();
            }
            if self.preferences.read(cx).state.connectors.enabled(provider) {
                snapshot.connections.insert(provider.into(), status);
            }
        }
        self.connection_revision = self.connection_revision.wrapping_add(1);
        self.refresh(false, cx);
        cx.notify();
    }

    #[cfg(test)]
    pub(super) fn stage_fixture_poll(&mut self, cx: &App) -> u64 {
        self.begin_poll(false, cx).expect("fixture poll admitted")
    }

    #[cfg(test)]
    pub(super) fn finish_fixture_poll(
        &mut self,
        generation: u64,
        snapshot: Snapshot,
        cx: &mut Context<Self>,
    ) {
        self.finish(generation, snapshot, cx);
    }

    #[cfg(test)]
    pub(super) fn mutation_revision(&self, key: &str) -> Option<i64> {
        self.self_activity.get(key).map(|(revision, _)| *revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::sync::{Arc, Mutex};

    #[gpui::test]
    fn one_poll_serves_both_roots_and_a_closed_scope_cannot_publish(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = Arc::new(Mutex::new(
                vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
            ));
            let a = cx.new(|_| ());
            let b = cx.new(|_| ());
            let feed = cx.new(|cx| InboxFeed::new(&store, PathBuf::from("/fixtures"), cx));
            feed.update(cx, |feed, cx| {
                feed.set_scope(a.entity_id(), vec!["/fixtures/left".into()], cx);
                feed.set_scope(
                    b.entity_id(),
                    vec!["/fixtures/left".into(), "/fixtures/right".into()],
                    cx,
                );
                assert_eq!(
                    feed.projects,
                    [
                        PathBuf::from("/fixtures/left"),
                        PathBuf::from("/fixtures/right")
                    ]
                );
                let first = feed.begin_poll(false, cx).unwrap();
                assert!(feed.begin_poll(false, cx).is_none());
                assert!(!feed.refresh_after_pending);
                feed.release(b.entity_id(), cx);
                let item = Item {
                    provider: "github".into(),
                    number: 1,
                    project_path: "/fixtures/right".into(),
                    ..Default::default()
                };
                feed.finish(
                    first,
                    Snapshot {
                        items: vec![item],
                        ..Default::default()
                    },
                    cx,
                );
                assert!(feed.snapshot.items.is_empty());

                let second = feed.begin_poll(false, cx).unwrap();
                let item = Item {
                    provider: "github".into(),
                    number: 2,
                    project_path: "/fixtures/left".into(),
                    ..Default::default()
                };
                feed.finish(
                    second,
                    Snapshot {
                        items: vec![item.clone()],
                        ..Default::default()
                    },
                    cx,
                );
                assert_eq!(feed.snapshot.items, [item]);
                feed.release(a.entity_id(), cx);
                assert!(feed.snapshot.items.is_empty());
                assert!(feed.begin_poll(false, cx).is_none());
            });
        });
    }

    #[gpui::test]
    fn connection_claim_and_publication_fence_the_previous_poll(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = Arc::new(Mutex::new(
                vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
            ));
            let root = cx.new(|_| ());
            let feed = cx.new(|cx| InboxFeed::new(&store, PathBuf::from("/fixtures"), cx));
            feed.update(cx, |feed, cx| {
                feed.set_scope(root.entity_id(), Vec::new(), cx);
                let first = feed.begin_poll(false, cx).unwrap();
                feed.begin_connection("jira", cx).unwrap();
                assert!(feed.begin_connection("jira", cx).is_err());
                let status = model::Status {
                    site: "https://new.example".into(),
                    connected: true,
                    ..Default::default()
                };
                feed.finish_connection("jira", Some(status.clone()), cx);
                let old = Snapshot {
                    items: vec![Item {
                        provider: "jira".into(),
                        url: "https://old.example/browse/ENG-1".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                };
                feed.finish(first, old, cx);
                assert!(feed.snapshot.items.is_empty());
                assert_eq!(feed.snapshot.connections.get("jira"), Some(&status));
                assert!(feed.begin_poll(false, cx).is_some());
            });
        });
    }
}
