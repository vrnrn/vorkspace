//! The one owner, per application, of what every window shares.
//!
//! Windows are views of a vorkspace. What must not run or exist twice lives
//! here instead of in a window's `Shell`:
//!
//! - Vorkspace definitions, saved under their own key, so every window lists
//!   the same vorkspaces and an edit in one appears in the others.
//! - The committed Bot roster, which decides when membership changes.
//! - The scheduler lease: exactly one window fires reminders and
//!   automations. It moves to another window when its holder closes.
//! - Session leases: a session is live in at most one window, so it never
//!   gets two `Conversation`s writing one transcript or running one turn.
//!
//! Leases name the window that holds them and are re-validated at every
//! claim: a lease whose window closed is free. A window also releases the lease
//! when it drops the loaded conversation.
use super::windows::{WINDOW_STATE_KEY, WindowLayouts, WindowShared};
use super::*;
use crate::{
    bots::{MembershipChanged, SharedRoster},
    workspace::{VorkspaceDef, move_bot, reconcile_bots},
};
use gpui::{AppContext as _, EventEmitter, WeakEntity};
use serde::{Deserialize, Serialize};

const STORE_KEY: &str = "vorkspaces";
const AUTOMATION_EVENTS_KEY: &str = "native-automation-inbox-retries";

#[derive(Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    vorkspaces: Vec<VorkspaceDef>,
    seen_bots: HashSet<String>,
}

pub(crate) struct AppCore {
    pub(super) store: Database,
    writes: Arc<SettingsWrites>,
    pub(crate) vorkspaces: Vec<VorkspaceDef>,
    pub(crate) seen_bots: HashSet<String>,
    pub(super) bot_roster: Entity<SharedRoster>,
    pub(super) layouts: WindowLayouts,
    next_window_id: u64,
    pub(super) remote: Arc<vorkspace_core::remote::Connections>,
    pub(super) remote_refreshes: HashMap<String, Option<std::time::Instant>>,
    pub(super) quick_shortcut: crate::composer::quick_composer::Shortcut,
    scheduler: Option<WeakEntity<Shell>>,
    pub(super) automation_cutoff: i64,
    pub(super) automation_recovered: bool,
    automation_events: HashMap<String, vorkspace_core::inbox::Item>,
    dock_badges: HashMap<u64, usize>,
    idle_protection: HashMap<u64, HashSet<i64>>,
    sessions: HashMap<i64, WeakEntity<Shell>>,
    session_deletions: HashSet<i64>,
    pub(super) project_deletions: Arc<Mutex<HashSet<String>>>,
    metadata_writes: HashMap<i64, WeakEntity<Shell>>,
    pub(super) catalog_revision: u64,
    pub(super) session_installs: crate::session_install::Registry,
    /// Windows in the order they joined; the oldest open one is the default
    /// destination for work no window owns.
    windows: Vec<WeakEntity<Shell>>,
    /// The agent app bridge: one socket for the application.
    pub(crate) bridge: Option<vorkspace_core::bot::bridge::Server>,
}

#[derive(Default)]
struct SettingsWrites {
    pending: Mutex<HashMap<&'static str, Arc<str>>>,
}

impl SettingsWrites {
    fn flush(&self, store: &Database) -> Result<(), String> {
        // Serialize writes through the store and pick the latest snapshot
        // only after acquiring it, including during app shutdown.
        let store = locked(store)?;
        let snapshot = self.pending.lock().unwrap().clone();
        if !snapshot.is_empty() {
            let values: Vec<_> = snapshot
                .iter()
                .map(|(&key, value)| (key, value.as_ref()))
                .collect();
            store.save_settings(&values)?;
            let mut pending = self.pending.lock().unwrap();
            pending.retain(|key, current| {
                !snapshot
                    .get(key)
                    .is_some_and(|saved| Arc::ptr_eq(current, saved))
            });
        }
        Ok(())
    }
}

/// One core per data store: windows on the same store share it, and a
/// separate store (as in tests that open several apps) has its own.
#[derive(Default)]
struct Installed(HashMap<usize, Entity<AppCore>>);
impl gpui::Global for Installed {}

pub(crate) fn bot_write_barriers(cx: &mut App) -> Vec<async_channel::Receiver<()>> {
    let rosters = cx
        .try_global::<Installed>()
        .map(|installed| {
            installed
                .0
                .values()
                .map(|core| core.read(cx).bot_roster.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    rosters
        .into_iter()
        .filter_map(|roster| roster.update(cx, |roster, cx| roster.write_barrier(cx)))
        .collect()
}

fn store_key(store: &Database) -> usize {
    Arc::as_ptr(store) as *const () as usize
}

impl AppCore {
    /// The application's core, created on first use from saved definitions.
    pub(crate) fn get(store: &Database, cx: &mut App) -> Entity<AppCore> {
        let key = store_key(store);
        if let Some(core) = cx.try_global::<Installed>().and_then(|i| i.0.get(&key)) {
            return core.clone();
        }
        let (saved, layouts, automation_events, remote_dir) = locked(store)
            .map(|db| {
                let saved: Saved = db
                    .setting(STORE_KEY)
                    .ok()
                    .flatten()
                    .and_then(|json| serde_json::from_str(&json).ok())
                    .unwrap_or_default();
                (
                    saved,
                    WindowLayouts::load(&db),
                    db.setting(AUTOMATION_EVENTS_KEY)
                        .ok()
                        .flatten()
                        .and_then(|raw| {
                            serde_json::from_str::<Vec<vorkspace_core::inbox::Item>>(&raw).ok()
                        })
                        .unwrap_or_default()
                        .into_iter()
                        .map(|item| (item.key(), item))
                        .collect(),
                    db.data_dir().map(std::path::Path::to_path_buf),
                )
            })
            .unwrap_or_default();
        let next_window_id = layouts
            .entries
            .iter()
            .map(|entry| entry.id)
            .max()
            .unwrap_or(0)
            + 1;
        let remote_dir = remote_dir.unwrap_or_else(|| {
            std::env::temp_dir().join(format!("vorkspace-remote-{}", std::process::id()))
        });
        let remote = Arc::new(vorkspace_core::remote::Connections::new(remote_dir));
        vorkspace_core::remote::set_default_connections(remote.clone());
        let writes = Arc::new(SettingsWrites::default());
        let quit_writes = writes.clone();
        let quit_store = store.clone();
        let quit_remote = remote.clone();
        cx.on_app_quit(move |cx| {
            let writes = quit_writes.clone();
            let store = quit_store.clone();
            let remote = quit_remote.clone();
            cx.background_executor().spawn(async move {
                if let Err(error) = writes.flush(&store) {
                    eprintln!("Could not save workspace settings on quit: {error}");
                }
                remote.shutdown();
            })
        })
        .detach();
        let bot_roster = SharedRoster::install(store.clone(), cx);
        let core = cx.new(|cx: &mut Context<AppCore>| {
            cx.subscribe(&bot_roster, |core, _, event: &MembershipChanged, cx| {
                core.reconcile_bots(
                    event.vorkspace.as_deref().unwrap_or_default(),
                    &event.ids,
                    cx,
                );
            })
            .detach();
            AppCore {
                store: store.clone(),
                writes,
                vorkspaces: saved.vorkspaces,
                seen_bots: saved.seen_bots,
                bot_roster,
                layouts,
                next_window_id,
                remote,
                remote_refreshes: HashMap::new(),
                quick_shortcut: Default::default(),
                scheduler: None,
                automation_cutoff: vorkspace_core::time::now_ms().min(i64::MAX as u64) as i64,
                automation_recovered: false,
                automation_events,
                dock_badges: HashMap::new(),
                idle_protection: HashMap::new(),
                sessions: HashMap::new(),
                session_deletions: HashSet::new(),
                project_deletions: Default::default(),
                metadata_writes: HashMap::new(),
                catalog_revision: 0,
                session_installs: Default::default(),
                windows: Vec::new(),
                bridge: None,
            }
        });
        cx.default_global::<Installed>().0.insert(key, core.clone());
        core
    }

    fn reconcile_bots(&mut self, destination: &str, ids: &[String], cx: &mut Context<Self>) {
        let mut defs = self.vorkspaces.clone();
        let mut seen = self.seen_bots.clone();
        if reconcile_bots(&mut defs, &mut seen, destination, ids) {
            self.publish(&defs, &seen, cx);
        }
    }

    fn set_bot_membership(
        &mut self,
        bot: &str,
        vorkspace: &str,
        add: bool,
        cx: &mut Context<Self>,
    ) {
        let mut defs = self.vorkspaces.clone();
        let Some(destination) = defs.iter_mut().find(|def| def.id == vorkspace) else {
            return;
        };
        if add {
            if !self.seen_bots.contains(bot) {
                return;
            }
            move_bot(&mut defs, vorkspace, bot);
        } else {
            destination.bots.retain(|id| id != bot);
        }
        self.publish(&defs, &self.seen_bots.clone(), cx);
    }

    fn apply_window_edits(
        &mut self,
        before: &[VorkspaceDef],
        edited: &[VorkspaceDef],
        cx: &mut Context<Self>,
    ) {
        let mut defs = self.vorkspaces.clone();
        for def in edited {
            match before.iter().find(|base| base.id == def.id) {
                Some(base) => {
                    if let Some(current) = defs.iter_mut().find(|current| current.id == def.id) {
                        current.apply_window_edits(base, def);
                    }
                }
                None if !defs.iter().any(|current| current.id == def.id) => {
                    let mut added = def.clone();
                    added.bots.clear();
                    defs.push(added);
                }
                None => {}
            }
        }
        let deleted = |id: &str| {
            before.iter().any(|def| def.id == id) && !edited.iter().any(|def| def.id == id)
        };
        if defs.iter().any(|def| !deleted(&def.id)) {
            defs.retain(|def| !deleted(&def.id));
        }
        self.publish(&defs, &self.seen_bots.clone(), cx);
    }

    /// Replaces the shared definitions when a window changed them.
    fn publish(
        &mut self,
        vorkspaces: &[VorkspaceDef],
        seen: &HashSet<String>,
        cx: &mut Context<Self>,
    ) {
        if self.vorkspaces == vorkspaces && &self.seen_bots == seen {
            return;
        }
        self.vorkspaces = vorkspaces.to_vec();
        self.seen_bots = seen.clone();
        let json = serde_json::to_string(&Saved {
            vorkspaces: self.vorkspaces.clone(),
            seen_bots: self.seen_bots.clone(),
        })
        .expect("vorkspace definitions serialize to JSON");
        self.writes
            .pending
            .lock()
            .unwrap()
            .insert(STORE_KEY, json.into());
        self.flush_settings(cx);
        cx.notify();
    }

    pub(super) fn flush_pending(&self, cx: &App) -> gpui::Task<Result<(), String>> {
        let writes = self.writes.clone();
        let store = self.store.clone();
        cx.background_executor()
            .spawn(async move { writes.flush(&store) })
    }

    fn flush_settings(&self, cx: &mut Context<Self>) {
        let work = self.flush_pending(cx);
        cx.spawn(async move |core, cx| {
            if let Err(error) = work.await {
                let _ = core.update(cx, |core, cx| {
                    for window in &core.windows {
                        let _ = window.update(cx, |shell, cx| {
                            shell.error =
                                Some(format!("Could not save workspace settings: {error}"));
                            cx.notify();
                        });
                    }
                });
            }
        })
        .detach();
    }

    pub(super) fn allocate_window(&mut self) -> u64 {
        let id = self.next_window_id;
        self.next_window_id += 1;
        id
    }

    fn layout_values(layouts: &WindowLayouts) -> (String, String) {
        let mut legacy: Workspace = layouts
            .legacy()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default();
        if let Some(shared) = &layouts.shared {
            shared.apply(&mut legacy);
        }
        (
            serde_json::to_string(layouts).expect("serialize windows"),
            serde_json::to_string(&legacy).expect("serialize the focused workspace"),
        )
    }

    fn stage_layouts(&self, cx: &mut Context<Self>) {
        let (windows, legacy) = Self::layout_values(&self.layouts);
        let mut pending = self.writes.pending.lock().unwrap();
        pending.insert(WINDOW_STATE_KEY, windows.into());
        pending.insert("workspace", legacy.into());
        drop(pending);
        self.flush_settings(cx);
    }

    fn record_layout(
        &mut self,
        window: u64,
        before: Option<&WindowShared>,
        workspace: &mut Workspace,
        focused: bool,
        durable: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let local = WindowShared::from_workspace(workspace);
        let mut next = self.layouts.clone();
        let shared = next.shared.get_or_insert_with(|| local.clone());
        if let Some(before) = before.filter(|before| *before != &local) {
            shared.apply_edits(before, local);
        }
        shared.apply(workspace);
        let shared_changed = next.shared != self.layouts.shared;
        let state = serde_json::to_string(workspace).expect("serialize workspace");
        if !next.record(window, state, focused) && !shared_changed {
            return Ok(());
        }
        if durable {
            let (windows, legacy) = Self::layout_values(&next);
            let store = locked(&self.store)?;
            store.save_settings(&[(WINDOW_STATE_KEY, &windows), ("workspace", &legacy)])?;
            // The lock also orders this commit after any older queued flush.
            let mut pending = self.writes.pending.lock().unwrap();
            pending.remove(WINDOW_STATE_KEY);
            pending.remove("workspace");
        }
        self.layouts = next;
        if !durable {
            self.stage_layouts(cx);
        }
        if shared_changed {
            cx.notify();
        }
        Ok(())
    }

    fn close_layout(&mut self, id: u64, cx: &mut Context<Self>) {
        self.keep_visible_agents(id, HashSet::new());
        if self.dock_badges.remove(&id).is_some() {
            crate::notifications::platform::set_dock_badge(self.dock_badges.values().sum());
        }
        if self.layouts.close(id) {
            self.stage_layouts(cx);
        }
    }

    pub(super) fn keep_visible_agents(&mut self, window: u64, ids: HashSet<i64>) {
        let changed = if ids.is_empty() {
            self.idle_protection.remove(&window).is_some()
        } else if self.idle_protection.get(&window) != Some(&ids) {
            self.idle_protection.insert(window, ids);
            true
        } else {
            false
        };
        if changed {
            vorkspace_core::provider::keep_idle_agents(
                self.idle_protection.values().flatten().copied().collect(),
            );
        }
    }

    /// Whether `shell` may fire scheduled work now. A vacant or stale lease
    /// is taken by the asking window.
    fn claim_scheduler(&mut self, shell: &WeakEntity<Shell>) -> bool {
        match &self.scheduler {
            Some(holder) if holder.upgrade().is_some() => holder == shell,
            _ => {
                self.scheduler = Some(shell.clone());
                true
            }
        }
    }

    pub(super) fn automation_events(&self) -> Vec<vorkspace_core::inbox::Item> {
        self.automation_events.values().cloned().collect()
    }

    pub(super) fn queue_automation_events(
        &mut self,
        items: Vec<vorkspace_core::inbox::Item>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for item in items {
            let key = item.key();
            let current = self.automation_events.get(&key);
            if current.is_none_or(|old| old != &item && old.updated_ms() <= item.updated_ms()) {
                self.automation_events.insert(key, item);
                changed = true;
            }
        }
        if !changed {
            return;
        }
        if self.automation_events.len() > 500 {
            let mut items = self.automation_events();
            items.sort_unstable_by_key(|item| std::cmp::Reverse(item.updated_ms()));
            self.automation_events = items
                .into_iter()
                .take(500)
                .map(|item| (item.key(), item))
                .collect();
        }
        self.stage_automation_events(cx);
    }

    pub(super) fn settle_automation_events(
        &mut self,
        attempted: Vec<vorkspace_core::inbox::Item>,
        retry: Vec<vorkspace_core::inbox::Item>,
        cx: &mut Context<Self>,
    ) {
        let mut retry: HashMap<_, _> = retry.into_iter().map(|item| (item.key(), item)).collect();
        let mut changed = false;
        for item in attempted {
            let key = item.key();
            // A newer connector refresh may have arrived during the claim.
            if self.automation_events.get(&key) != Some(&item) {
                continue;
            }
            match retry.remove(&key) {
                Some(pending) if pending != item => {
                    self.automation_events.insert(key, pending);
                    changed = true;
                }
                Some(_) => {}
                None => {
                    self.automation_events.remove(&key);
                    changed = true;
                }
            }
        }
        if changed {
            self.stage_automation_events(cx);
        }
    }

    fn stage_automation_events(&self, cx: &mut Context<Self>) {
        let snapshot =
            serde_json::to_string(&self.automation_events()).expect("serialize connector retries");
        self.writes
            .pending
            .lock()
            .unwrap()
            .insert(AUTOMATION_EVENTS_KEY, snapshot.into());
        self.flush_settings(cx);
    }

    /// Whether `shell` may load session `id` live. Another window keeps it
    /// only while that window exists and still has the session loaded.
    fn claim_session(
        &mut self,
        id: i64,
        shell: &WeakEntity<Shell>,
        install: Option<&crate::session_install::Install>,
    ) -> bool {
        if !self.session_installs.permits(id, install)
            || self.session_deletions.contains(&id)
            || self.metadata_is_writing(id)
            || self.other_session_window(id, shell).is_some()
        {
            return false;
        }
        self.sessions.insert(id, shell.clone());
        true
    }

    fn metadata_is_writing(&self, id: i64) -> bool {
        self.metadata_writes
            .get(&id)
            .is_some_and(|owner| owner.upgrade().is_some())
    }

    fn other_session_window(
        &self,
        id: i64,
        requester: &WeakEntity<Shell>,
    ) -> Option<Entity<Shell>> {
        self.sessions
            .get(&id)
            .filter(|holder| *holder != requester)
            .and_then(WeakEntity::upgrade)
    }
}

enum SessionCatalogChange {
    Saved {
        summary: Box<Session>,
        worktree: Option<String>,
        draft: bool,
    },
    Deleted(i64),
}

struct CatalogChanged {
    source: WeakEntity<Shell>,
    change: SessionCatalogChange,
}

impl EventEmitter<CatalogChanged> for AppCore {}

struct GroupsChanged {
    project: String,
    groups: vorkspace_core::session_groups::Groups,
}

impl EventEmitter<GroupsChanged> for AppCore {}

impl AppCore {
    fn register(&mut self, shell: WeakEntity<Shell>) {
        self.windows.retain(|w| w.upgrade().is_some());
        self.windows.push(shell);
    }

    /// The window an agent call from `session` belongs to: the one with that
    /// session live, else the one whose Bots run it, else the oldest open.
    pub(crate) fn route(&self, session: i64, cx: &App) -> Option<Entity<Shell>> {
        self.session_owner(session, cx)
            .or_else(|| self.windows.iter().find_map(|w| w.upgrade()))
    }

    pub(super) fn session_owner(&self, session: i64, cx: &App) -> Option<Entity<Shell>> {
        if let Some(holder) = self.sessions.get(&session).and_then(|w| w.upgrade()) {
            return Some(holder);
        }
        self.windows
            .iter()
            .filter_map(|w| w.upgrade())
            .find(|shell| {
                shell
                    .read(cx)
                    .bots
                    .as_ref()
                    .is_some_and(|m| m.read(cx).calling_view(session, cx).is_some())
            })
    }

    /// Starts the bridge once and hands each call to the window it belongs to.
    fn start_bridge(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if self.bridge.is_some() {
            return Ok(());
        }
        let (server, calls) = vorkspace_core::bot::bridge::Server::start()?;
        self.bridge = Some(server);
        cx.spawn(async move |core, cx| {
            while let Ok(call) = calls.recv().await {
                cx.update(|cx| {
                    let target = core.upgrade()?.read(cx).route(call.session, cx)?;
                    let handles: Vec<_> = cx
                        .windows()
                        .into_iter()
                        .filter_map(|w| w.downcast::<Shell>())
                        .collect();
                    let window = handles.into_iter().find(|h| {
                        cx.update_window((*h).into(), |root, _, _| {
                            root.entity_id() == target.entity_id()
                        })
                        .unwrap_or(false)
                    })?;
                    window
                        .update(cx, |shell, window, cx| {
                            shell.handle_app_call(call, window, cx)
                        })
                        .ok()
                });
            }
        })
        .detach();
        Ok(())
    }
}

impl Shell {
    pub(super) fn project_is_deleting(&self, path: &str, cx: &App) -> bool {
        self.app_core.as_ref().is_some_and(|core| {
            core.read(cx)
                .project_deletions
                .lock()
                .unwrap()
                .iter()
                .any(|root| super::project_delete::within(path, root))
        })
    }

    pub(super) fn allow_project_entry(&mut self, path: &str, cx: &mut Context<Self>) -> bool {
        let error = if self.project_is_deleting(path, cx) {
            "This project is being deleted."
        } else if vorkspace_core::worktree_management::is_being_removed(path) {
            "This worktree is being removed."
        } else {
            return true;
        };
        self.error = Some(error.into());
        cx.notify();
        false
    }

    /// Joins this window to the application's shared state. The first
    /// window seeds it from its own (possibly migrated) definitions.
    pub(super) fn adopt_app_core(&mut self, core: Entity<AppCore>, cx: &mut Context<Self>) {
        let new_projects = std::mem::take(&mut self.workspace.restored_new_projects);
        if core.read(cx).vorkspaces.is_empty() {
            let (defs, seen) = (
                self.workspace.vorkspaces.clone(),
                self.workspace.seen_bots.clone(),
            );
            core.update(cx, |core, cx| core.publish(&defs, &seen, cx));
        } else {
            self.take_shared_vorkspaces(&core, cx);
            let active = self.workspace.active_vorkspace.clone();
            let mut added = false;
            for path in new_projects {
                let key = vorkspace_core::files::path_key(&path);
                if let Some(v) = self.workspace.vorkspace_mut(&active)
                    && !v
                        .projects
                        .iter()
                        .any(|p| vorkspace_core::files::path_key(p) == key)
                {
                    v.projects.push(path);
                    added = true;
                }
            }
            if added {
                let (defs, seen) = (
                    self.workspace.vorkspaces.clone(),
                    self.workspace.seen_bots.clone(),
                );
                core.update(cx, |core, cx| core.publish(&defs, &seen, cx));
            }
        }
        let active = self.workspace.active_vorkspace.clone();
        core.update(cx, |core, cx| {
            if let Some(ids) = core.bot_roster.read(cx).member_ids() {
                core.reconcile_bots(&active, &ids, cx);
            }
        });
        self.take_shared_vorkspaces(&core, cx);
        if self.workspace.reconcile_project_scope() {
            self.sync_git(cx);
        }
        self.sync_inbox_scope(cx);
        self.observations.push(cx.observe(&core, |this, core, cx| {
            let definitions_changed = this.take_shared_vorkspaces(&core, cx);
            if this.take_shared_window_state(&core, cx) || definitions_changed {
                this.reconcile_after_shared_change(cx);
            }
        }));
        self.observations
            .push(cx.subscribe(&core, |this, _, event: &CatalogChanged, cx| {
                if cx.weak_entity() == event.source {
                    return;
                }
                match &event.change {
                    SessionCatalogChange::Saved {
                        summary,
                        worktree,
                        draft,
                    } => {
                        let id = summary.id;
                        if this.sessions.contains_key(&id) {
                            return;
                        }
                        if this
                            .session_summaries
                            .insert(id, (**summary).clone())
                            .is_none()
                        {
                            this.order.push(id);
                        }
                        this.session_worktrees.insert(id, worktree.clone());
                        if *draft {
                            this.draft_sessions.insert(id);
                        } else {
                            this.draft_sessions.remove(&id);
                        }
                        this.sync_search_session(id, cx);
                    }
                    SessionCatalogChange::Deleted(id) => {
                        this.remove_session_from_catalog(*id, cx);
                    }
                }
                this.invalidate_chrome(cx);
                cx.notify();
            }));
        self.observations
            .push(cx.subscribe(&core, |this, _, event: &GroupsChanged, cx| {
                if this.groups.get(&event.project) != Some(&event.groups) {
                    this.groups
                        .insert(event.project.clone(), event.groups.clone());
                    this.invalidate_chrome(cx);
                    cx.notify();
                }
            }));
        self.observations.push(cx.subscribe(
            &core,
            |this, _, event: &super::remote_sessions::RemoteCatalog, cx| {
                this.apply_remote_catalog(event, cx);
            },
        ));
        let me = cx.weak_entity();
        core.update(cx, |core, _| core.register(me));
        let release = core.downgrade();
        let window = self.window_layout;
        cx.on_release(move |_, cx| {
            let _ = release.update(cx, |core, _| {
                core.keep_visible_agents(window, HashSet::new())
            });
        })
        .detach();
        self.app_core = Some(core);
    }

    fn take_shared_window_state(&mut self, core: &Entity<AppCore>, cx: &mut Context<Self>) -> bool {
        let shared = core.read(cx).layouts.shared.clone();
        if shared == self.window_shared {
            return false;
        }
        let Some(shared) = shared else {
            return false;
        };
        let preferences = self.workspace.preferences.clone();
        shared.apply(&mut self.workspace);
        self.window_shared = Some(shared);
        if preferences != self.workspace.preferences {
            self.apply_preferences(cx);
            self.sync_settings(cx);
        }
        true
    }

    pub(super) fn save_window_state(
        &mut self,
        mut next: Option<Workspace>,
        durable: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.window_closing {
            return Ok(());
        }
        let core = self.app_core.clone().expect("registered workspace window");
        let workspace = next.as_mut().unwrap_or(&mut self.workspace);
        core.update(cx, |core, cx| {
            core.record_layout(
                self.window_layout,
                self.window_shared.as_ref(),
                workspace,
                self.window_active,
                durable,
                cx,
            )
        })?;
        self.window_shared = Some(WindowShared::from_workspace(workspace));
        if let Some(next) = next {
            self.workspace = next;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn latest_window_layout(&self, cx: &App) -> String {
        self.app_core
            .as_ref()
            .unwrap()
            .read(cx)
            .layouts
            .entries
            .iter()
            .find(|entry| entry.id == self.window_layout)
            .expect("persisted window layout")
            .workspace
            .clone()
    }

    pub(super) fn forget_closed_window(&self, cx: &mut Context<Self>) {
        if let Some(core) = self.app_core.clone() {
            core.update(cx, |core, cx| core.close_layout(self.window_layout, cx));
        }
    }

    pub(super) fn pending_last_window_bot_writes(&self, cx: &App) -> bool {
        self.app_core.as_ref().is_some_and(|core| {
            let core = core.read(cx);
            core.windows
                .iter()
                .filter(|window| window.upgrade().is_some())
                .count()
                == 1
                && core.bot_roster.read(cx).has_pending_writes()
        })
    }

    pub(super) fn unsaved_window_edits(&self, cx: &mut Context<Self>) -> bool {
        if self.unsaved_files(cx) || !self.prepare_notes_close(cx) {
            return true;
        }
        let Some(core) = &self.app_core else {
            return false;
        };
        let me = cx.weak_entity();
        let others: Vec<_> = core
            .read(cx)
            .windows
            .iter()
            .filter(|shell| *shell != &me)
            .filter_map(WeakEntity::upgrade)
            .collect();
        others.into_iter().any(|shell| {
            shell.update(cx, |shell, cx| {
                shell.unsaved_files(cx) || !shell.prepare_notes_close(cx)
            })
        })
    }

    /// Starts the application's agent bridge if no window has yet.
    pub(super) fn init_app_bridge(&mut self, cx: &mut Context<Self>) {
        // GPUI's deterministic executor cannot be woken by an OS socket thread.
        // Command tests exercise the same dispatcher directly without a listener.
        if cx.is_test() {
            return;
        }
        let Some(core) = self.app_core.clone() else {
            return;
        };
        if let Err(error) = core.update(cx, |core, cx| core.start_bridge(cx)) {
            self.error = Some(format!("Operator app access: {error}"));
        }
    }

    /// The environment that lets session `id`'s agent reach the bridge.
    pub(super) fn bridge_environment(&self, id: i64, cx: &App) -> Option<HashMap<String, String>> {
        let core = self.app_core.as_ref()?.read(cx);
        core.bridge.as_ref().map(|bridge| bridge.environment(id))
    }

    /// Copies the shared definitions in; returns whether anything changed.
    fn take_shared_vorkspaces(&mut self, core: &Entity<AppCore>, cx: &App) -> bool {
        let core = core.read(cx);
        self.vorkspace_baseline.clone_from(&core.vorkspaces);
        if self.workspace.vorkspaces == core.vorkspaces
            && self.workspace.seen_bots == core.seen_bots
        {
            return false;
        }
        self.workspace.vorkspaces = core.vorkspaces.clone();
        self.workspace.seen_bots = core.seen_bots.clone();
        self.workspace.ensure_vorkspaces();
        true
    }

    fn reconcile_after_shared_change(&mut self, cx: &mut Context<Self>) {
        if self.workspace.reconcile_project_scope() {
            self.sync_git(cx);
        }
        self.sync_inbox_scope(cx);
        if let Some(dialog) = self.vorkspace_dialog.as_mut() {
            let retargeted = !self
                .workspace
                .vorkspaces
                .iter()
                .any(|v| v.id == dialog.editing);
            if retargeted {
                dialog.editing = self.workspace.active_vorkspace.clone();
                dialog.project = None;
                dialog.confirm_delete = false;
            } else if let Some(project) = &dialog.project
                && !self
                    .workspace
                    .vorkspaces
                    .iter()
                    .find(|v| v.id == dialog.editing)
                    .is_some_and(|v| {
                        v.projects.iter().any(|p| {
                            vorkspace_core::files::path_key(p)
                                == vorkspace_core::files::path_key(project)
                        })
                    })
            {
                dialog.project = None;
            }
            if let Some(v) = self
                .workspace
                .vorkspaces
                .iter()
                .find(|v| v.id == dialog.editing)
            {
                let field = dialog.name.read(cx);
                if (retargeted || !field.is_composing())
                    && (retargeted || field.content.trim() != v.name)
                {
                    dialog
                        .name
                        .update(cx, |field, cx| field.set_text(v.name.clone(), cx));
                }
                let field = dialog.icon_letters.read(cx);
                if (retargeted || !field.is_composing())
                    && (retargeted || field.content.as_ref() != v.icon_letters())
                {
                    dialog.icon_letters.update(cx, |field, cx| {
                        field.set_text(v.icon_letters().to_owned(), cx)
                    });
                }
            }
        }
        self.sync_vorkspace_bot_visibility(cx);
        self.invalidate_chrome(cx);
        cx.notify();
    }

    /// Hands this window's definitions to the other windows when they changed.
    pub(super) fn share_vorkspaces(&mut self, cx: &mut Context<Self>) {
        let Some(core) = self.app_core.clone() else {
            return;
        };
        if self.workspace.vorkspaces != self.vorkspace_baseline {
            core.update(cx, |core, cx| {
                core.apply_window_edits(&self.vorkspace_baseline, &self.workspace.vorkspaces, cx);
            });
        }
        if self.take_shared_vorkspaces(&core, cx) {
            // A Bot host call can save while the Bot manager is leased.
            let shell = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = shell.update(cx, |shell, cx| shell.reconcile_after_shared_change(cx));
            });
        }
    }

    pub(super) fn set_bot_membership(
        &mut self,
        bot: &str,
        vorkspace: &str,
        add: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(core) = self.app_core.clone() else {
            return;
        };
        core.update(cx, |core, cx| {
            core.set_bot_membership(bot, vorkspace, add, cx)
        });
        self.take_shared_vorkspaces(&core, cx);
    }

    /// Only the lease holder fires reminders and automations.
    pub(super) fn holds_scheduler(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(core) = self.app_core.clone() else {
            return true;
        };
        let me = cx.weak_entity();
        core.update(cx, |core, _| core.claim_scheduler(&me))
    }

    pub(super) fn scheduler_window(&self, cx: &Context<Self>) -> Option<WeakEntity<Self>> {
        self.app_core
            .as_ref()?
            .read(cx)
            .scheduler
            .as_ref()
            .filter(|owner| **owner != cx.weak_entity())
            .cloned()
    }

    pub(super) fn set_window_badge(&self, badge: usize, cx: &mut Context<Self>) {
        if let Some(core) = &self.app_core {
            core.update(cx, |core, _| {
                if badge == 0 {
                    core.dock_badges.remove(&self.window_layout);
                } else {
                    core.dock_badges.insert(self.window_layout, badge);
                }
                crate::notifications::platform::set_dock_badge(core.dock_badges.values().sum());
            });
        }
    }

    /// Whether this window may load session `id` live.
    pub(super) fn claim_session(
        &mut self,
        id: i64,
        install: Option<&crate::session_install::Install>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(core) = self.app_core.clone() else {
            return true;
        };
        let me = cx.weak_entity();
        core.update(cx, |core, _| core.claim_session(id, &me, install))
    }

    pub(super) fn session_available_here(&self, id: i64, cx: &Context<Self>) -> bool {
        self.session_available_for_install(id, None, cx)
    }

    pub(super) fn session_available_for_install(
        &self,
        id: i64,
        install: Option<&crate::session_install::Install>,
        cx: &Context<Self>,
    ) -> bool {
        self.app_core.as_ref().is_none_or(|core| {
            let core = core.read(cx);
            !core.session_deletions.contains(&id)
                && core.session_installs.permits(id, install)
                && !core.metadata_is_writing(id)
                && core.other_session_window(id, &cx.weak_entity()).is_none()
        })
    }

    pub(super) fn begin_session_metadata(&self, ids: &[i64], cx: &mut Context<Self>) {
        if let Some(core) = &self.app_core {
            let owner = cx.weak_entity();
            core.update(cx, |core, _| {
                for id in ids {
                    core.metadata_writes.insert(*id, owner.clone());
                }
            });
        }
    }

    pub(super) fn finish_session_metadata(&self, ids: &[i64], cx: &mut Context<Self>) {
        if let Some(core) = &self.app_core {
            let owner = cx.weak_entity();
            core.update(cx, |core, _| {
                for id in ids {
                    if core.metadata_writes.get(id) == Some(&owner) {
                        core.metadata_writes.remove(id);
                    }
                }
            });
        }
    }

    pub(super) fn publish_groups(&mut self, project: String, cx: &mut Context<Self>) {
        match locked(&self.store).and_then(|store| store.groups(&project)) {
            Ok(groups) => {
                self.groups.insert(project.clone(), groups.clone());
                if let Some(core) = &self.app_core {
                    core.update(cx, |core, cx| {
                        core.catalog_revision = core.catalog_revision.wrapping_add(1);
                        cx.emit(GroupsChanged { project, groups });
                    });
                }
            }
            Err(error) => {
                self.error = Some(format!("Could not refresh the project folders: {error}"))
            }
        }
        let owner = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |shell, cx| {
                shell.invalidate_chrome(cx);
                cx.notify();
            });
        });
    }

    pub(super) fn session_install(&self, cx: &App) -> crate::session_install::Install {
        self.app_core
            .as_ref()
            .expect("registered workspace window")
            .read(cx)
            .session_installs
            .begin(&self.preparing_sessions)
    }

    pub(super) fn focus_session_window(&self, id: i64, cx: &mut Context<Self>) -> bool {
        self.focus_session_window_with(id, move |shell, window, cx| shell.open(id, window, cx), cx)
    }

    pub(super) fn peer_session_view(
        &self,
        id: i64,
        cx: &Context<Self>,
    ) -> Option<Entity<Conversation>> {
        let owner = self
            .app_core
            .as_ref()?
            .read(cx)
            .other_session_window(id, &cx.weak_entity())?;
        owner.read(cx).sessions.get(&id).cloned()
    }

    pub(super) fn focus_session_window_with(
        &self,
        id: i64,
        show: impl Fn(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(owner) = self
            .app_core
            .as_ref()
            .and_then(|core| core.read(cx).other_session_window(id, &cx.weak_entity()))
            .map(|owner| owner.downgrade())
        else {
            return false;
        };
        let source = self.native_window;
        cx.defer(move |cx| {
            // A peer can reference a view while rendering its rail. Address
            // the owning window independently of GPUI's render context.
            let target = owner
                .upgrade()
                .and_then(|shell| shell.read(cx).native_window);
            let shown = target.is_some_and(|target| {
                target
                    .update(cx, |shell, window, cx| {
                        show(shell, window, cx);
                        vorkspace_ui::traffic_lights::show(window);
                        window.activate_window();
                    })
                    .is_ok()
            });
            if !shown
                && owner.upgrade().is_none()
                && let Some(source) = source
            {
                let _ = source.update(cx, |shell, window, cx| show(shell, window, cx));
            }
        });
        true
    }

    pub(super) fn release_live_session(&mut self, id: i64, cx: &mut Context<Self>) {
        self.sessions.remove(&id);
        if let Some(core) = &self.app_core {
            let me = cx.weak_entity();
            core.update(cx, |core, _| {
                if core.sessions.get(&id) == Some(&me) {
                    core.sessions.remove(&id);
                }
            });
        }
    }

    pub(super) fn reserve_session_deletion(&self, id: i64, cx: &mut Context<Self>) -> bool {
        if !self.session_available_here(id, cx) {
            return false;
        }
        self.app_core
            .as_ref()
            .is_none_or(|core| core.update(cx, |core, _| core.session_deletions.insert(id)))
    }

    pub(super) fn release_session_deletion(&self, id: i64, cx: &mut Context<Self>) {
        if let Some(core) = &self.app_core {
            core.update(cx, |core, _| core.session_deletions.remove(&id));
        }
    }

    fn emit_catalog_change(&self, change: SessionCatalogChange, cx: &mut Context<Self>) {
        if let Some(core) = &self.app_core {
            let source = cx.weak_entity();
            core.update(cx, |core, cx| {
                core.catalog_revision = core.catalog_revision.wrapping_add(1);
                cx.emit(CatalogChanged { source, change });
            });
        }
    }

    pub(super) fn publish_session_summary(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(summary) = self.session_summaries.get(&id).cloned() else {
            return;
        };
        let worktree = self.session_worktrees.get(&id).cloned().flatten();
        self.emit_catalog_change(
            SessionCatalogChange::Saved {
                summary: Box::new(summary),
                worktree,
                draft: self.draft_sessions.contains(&id),
            },
            cx,
        );
    }

    pub(super) fn publish_session_deletion(&self, id: i64, cx: &mut Context<Self>) {
        self.emit_catalog_change(SessionCatalogChange::Deleted(id), cx);
    }

    pub(super) fn other_window_uses_project(&self, path: &str, cx: &Context<Self>) -> bool {
        let Some(core) = &self.app_core else {
            return false;
        };
        let me = cx.weak_entity();
        core.read(cx)
            .windows
            .iter()
            .filter(|window| *window != &me)
            .filter_map(WeakEntity::upgrade)
            .any(|shell| shell.read(cx).window_uses_project(path, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, WindowHandle};
    use vorkspace_core::store::Store;

    fn two_windows(
        cx: &mut TestAppContext,
    ) -> (
        WindowHandle<Shell>,
        WindowHandle<Shell>,
        i64,
        tempfile::TempDir,
    ) {
        two_windows_with(cx, |_, _| {})
    }

    fn two_windows_with(
        cx: &mut TestAppContext,
        seed: impl FnOnce(&Store, &str),
    ) -> (
        WindowHandle<Shell>,
        WindowHandle<Shell>,
        i64,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project").to_string_lossy().into_owned();
        std::fs::create_dir_all(&project).unwrap();
        let store = Store::open(&dir.path().join("native.sqlite")).unwrap();
        seed(&store, &project);
        let session = store.create("codex", &project).unwrap();
        let id = session.id;
        let store: Database = Arc::new(Mutex::new(store));
        let open = |cx: &mut TestAppContext| {
            let (store, session, project) = (store.clone(), session.clone(), project.clone());
            cx.update(|cx| {
                cx.open_window(Default::default(), |_, cx| {
                    cx.new(|cx| Shell::new(store, vec![session], project, None, cx))
                })
                .unwrap()
            })
        };
        let a = open(cx);
        cx.run_until_parked();
        let b = open(cx);
        cx.run_until_parked();
        (a, b, id, dir)
    }

    #[gpui::test]
    fn windows_share_vorkspace_definitions(cx: &mut TestAppContext) {
        let (a, b, _, _dir) = two_windows(cx);
        let id = a
            .update(cx, |shell, _, cx| {
                let id = shell.workspace.create_vorkspace("Shared");
                shell.persist(cx);
                id
            })
            .unwrap();
        cx.run_until_parked();
        b.update(cx, |shell, _, _| {
            assert!(shell.workspace.vorkspaces.iter().any(|v| v.id == id));
            // Each window keeps its own choice of which one it shows.
            assert_ne!(shell.workspace.active_vorkspace, id);
        })
        .unwrap();
    }

    #[gpui::test]
    fn concurrent_bot_creations_keep_their_own_vorkspaces_and_share_the_roster(
        cx: &mut TestAppContext,
    ) {
        let (a, b, _, _dir) = two_windows(cx);
        let side = b
            .update(cx, |shell, window, cx| {
                let id = shell.workspace.create_vorkspace("Side");
                let project = shell.workspace.project.clone();
                shell.workspace.add_project_to_vorkspace(&id, &project);
                shell.persist(cx);
                shell.switch_vorkspace(&id, window, cx);
                id
            })
            .unwrap();
        let create = |window: WindowHandle<Shell>, cx: &mut TestAppContext| {
            window
                .update(cx, |shell, w, cx| {
                    let project = shell.workspace.project.clone();
                    shell.bot_manager().update(cx, |bots, cx| {
                        bots.create_named(vec![project], w, cx);
                        bots.roster.bots.last().unwrap().id.clone()
                    })
                })
                .unwrap()
        };
        let first = create(a, cx);
        let second = create(b, cx);
        cx.run_until_parked();
        for (window, own, other) in [(a, &first, &second), (b, &second, &first)] {
            window
                .update(cx, |shell, _, cx| {
                    let manager = shell.bot_manager();
                    let bots = manager.read(cx);
                    assert!(bots.roster.get(own).unwrap().session_id.is_some());
                    assert!(bots.roster.get(other).unwrap().session_id.is_some());
                    assert!(shell.workspace.vorkspace_has_bot(own));
                    assert!(!shell.workspace.vorkspace_has_bot(other));
                    shell.persist(cx);
                })
                .unwrap();
        }

        let visual = &mut gpui::VisualTestContext::from_window(b.into(), cx);
        visual.refresh().unwrap();
        let delete = visual.debug_bounds("bot-delete").unwrap();
        visual.simulate_click(delete.center(), Default::default());
        visual.simulate_prompt_answer("Delete");
        visual.run_until_parked();
        a.update(visual, |shell, _, cx| {
            assert!(shell.bot_manager().read(cx).roster.get(&second).is_none());
            assert!(shell.workspace.vorkspace_has_bot(&first));
            assert!(
                shell
                    .workspace
                    .vorkspaces
                    .iter()
                    .find(|v| v.id == side)
                    .unwrap()
                    .bots
                    .is_empty()
            );
            let durable =
                vorkspace_core::bot::Roster::load(&locked(&shell.store).unwrap()).unwrap();
            assert!(durable.get(&second).is_none());
            assert!(durable.get(&first).is_some());
        })
        .unwrap();
    }

    #[gpui::test]
    fn accepted_bot_creation_finishes_when_its_window_closes(cx: &mut TestAppContext) {
        let (a, b, _, _dir) = two_windows(cx);
        let id = b
            .update(cx, |shell, w, cx| {
                let project = shell.workspace.project.clone();
                let id = shell.bot_manager().update(cx, |bots, cx| {
                    bots.create_named(vec![project], w, cx);
                    bots.roster.bots.last().unwrap().id.clone()
                });
                w.remove_window();
                id
            })
            .unwrap();
        cx.run_until_parked();
        a.update(cx, |shell, _, cx| {
            assert!(shell.bot_manager().read(cx).roster.get(&id).is_some());
            assert!(shell.workspace.vorkspace_has_bot(&id));
            let durable =
                vorkspace_core::bot::Roster::load(&locked(&shell.store).unwrap()).unwrap();
            assert!(durable.get(&id).is_some());
        })
        .unwrap();
    }

    #[gpui::test]
    fn two_windows_prepare_one_backing_conversation_even_if_the_first_closes(
        cx: &mut TestAppContext,
    ) {
        let id = "shared-new-bot";
        let (a, b, _, _dir) = two_windows_with(cx, |store, project| {
            let mut roster = vorkspace_core::bot::Roster::default();
            roster.bots.push(vorkspace_core::bot::Bot {
                id: id.into(),
                name: "Shared".into(),
                projects: vec![project.into()],
                ..Default::default()
            });
            roster.save(store).unwrap();
        });
        for window in [a, b] {
            window
                .update(cx, |shell, _, cx| {
                    assert!(
                        shell
                            .bot_manager()
                            .update(cx, |bots, cx| {
                                assert!(bots.roster.get(id).unwrap().session_id.is_none());
                                bots.chat_for_action(id, cx)
                            })
                            .is_none()
                    );
                })
                .unwrap();
        }
        cx.update_window(a.into(), |_, w, _| w.remove_window())
            .unwrap();
        cx.run_until_parked();
        b.update(cx, |shell, _, cx| {
            shell.bot_manager().update(cx, |bots, cx| {
                bots.chat_for_action(id, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        b.update(cx, |shell, _, cx| {
            let db = locked(&shell.store).unwrap();
            let durable = vorkspace_core::bot::Roster::load(&db).unwrap();
            let backing = durable.get(id).unwrap().session_id.unwrap();
            assert_eq!(durable.hidden_sessions, [backing]);
            let home = db.bot_session(backing).unwrap().unwrap().cwd;
            assert_eq!(
                db.sessions()
                    .unwrap()
                    .iter()
                    .filter(|s| s.cwd == home)
                    .count(),
                1
            );
            assert_eq!(
                shell.bot_manager().update(cx, |bots, cx| bots
                    .chat_for_action(id, cx)
                    .unwrap()
                    .read(cx)
                    .summary
                    .id),
                backing
            );
        })
        .unwrap();
    }

    #[gpui::test]
    fn one_window_fires_scheduled_work_and_hands_it_on_when_closed(cx: &mut TestAppContext) {
        let (a, b, _, _dir) = two_windows(cx);
        assert!(
            a.update(cx, |shell, _, cx| shell.holds_scheduler(cx))
                .unwrap()
        );
        assert!(
            !b.update(cx, |shell, _, cx| shell.holds_scheduler(cx))
                .unwrap()
        );
        cx.update_window(a.into(), |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
        assert!(
            b.update(cx, |shell, _, cx| shell.holds_scheduler(cx))
                .unwrap()
        );
    }

    #[gpui::test]
    fn scheduler_delivers_a_reminder_created_in_another_window(cx: &mut TestAppContext) {
        let (a, b, _, _dir) = two_windows(cx);
        assert!(
            a.update(cx, |shell, _, cx| shell.holds_scheduler(cx))
                .unwrap()
        );
        let id = b
            .update(cx, |shell, _, cx| {
                let session = locked(&shell.store)
                    .unwrap()
                    .create("codex", &shell.workspace.project)
                    .unwrap();
                let id = session.id;
                shell.add(session, cx);
                shell.store_write("set reminder", cx, move |store| {
                    store.set_reminders(&[id], 1)
                });
                shell.poll_reminders(cx);
                id
            })
            .unwrap();
        cx.run_until_parked();
        a.update(cx, |shell, _, cx| {
            assert!(shell.session_summaries.contains_key(&id));
            assert!(shell.reminders.is_empty());
            assert!(
                locked(&shell.store).unwrap().reminders().unwrap()[0]
                    .fired_at
                    .is_none()
            );
            shell.poll_reminders(cx);
        })
        .unwrap();
        cx.run_until_parked();
        a.update(cx, |shell, _, cx| {
            let durable = locked(&shell.store).unwrap().reminders().unwrap();
            assert_eq!(durable, shell.reminders);
            assert_eq!(durable[0].session_id, id);
            assert!(durable[0].fired_at.is_some());
            shell.sync_notices(cx);
            assert!(!shell.notification_state.reminders.read(cx).is_empty());
        })
        .unwrap();
        let later = vorkspace_core::time::now_ms() + 3_600_000;
        b.update(cx, |shell, _, _| {
            assert_ne!(shell.workspace.active, Some(id));
            locked(&shell.store)
                .unwrap()
                .set_reminders(&[id], later)
                .unwrap();
        })
        .unwrap();
        a.update(cx, |shell, _, cx| {
            shell.notification_state.reminders.update(cx, |_, cx| {
                cx.emit(crate::notifications::reminders::Action::Open {
                    session_id: id,
                    due_at: 1,
                });
            });
        })
        .unwrap();
        cx.run_until_parked();
        b.update(cx, |shell, _, _| {
            assert_eq!(shell.workspace.active, Some(id));
            assert_eq!(
                locked(&shell.store).unwrap().reminders().unwrap()[0].due_at,
                later
            );
        })
        .unwrap();
        a.update(cx, |shell, _, _| assert!(shell.reminders.is_empty()))
            .unwrap();
    }

    #[gpui::test]
    fn quick_composer_toggles_one_popup_across_windows(cx: &mut TestAppContext) {
        let (a, b, _, _dir) = two_windows(cx);
        for (window, expected) in [(a, 3), (b, 2), (b, 3), (a, 2)] {
            window
                .update(cx, |shell, window, cx| {
                    shell.toggle_quick_composer(window, cx)
                })
                .unwrap();
            cx.run_until_parked();
            assert_eq!(cx.update(|cx| cx.windows().len()), expected);
        }
    }

    #[gpui::test]
    fn a_session_is_private_during_preparation_and_peers_receive_its_worktree(
        cx: &mut TestAppContext,
    ) {
        let (a, b, _, _dir) = two_windows(cx);
        let (summary, mut install) = a
            .update(cx, |shell, window, cx| {
                let mut install = shell.session_install(cx);
                let summary = {
                    let db = locked(&shell.store).unwrap();
                    let summary = db.create("codex", &shell.workspace.project).unwrap();
                    install.track(summary.id);
                    assert!(
                        !shell
                            .app_core
                            .as_ref()
                            .unwrap()
                            .read(cx)
                            .session_installs
                            .visible_sessions(&db)
                            .unwrap()
                            .iter()
                            .any(|row| row.id == summary.id)
                    );
                    summary
                };
                assert!(!shell.allow_window_close(window, cx));
                (summary, install)
            })
            .unwrap();
        let id = summary.id;
        b.update(cx, |shell, _, cx| {
            assert!(!shell.session_available_here(id, cx));
            assert!(shell.add_loaded(summary.clone(), cx).is_none());
            assert!(!shell.session_summaries.contains_key(&id));
        })
        .unwrap();
        let worktree = format!("{}/checkout", summary.cwd);
        a.update(cx, |shell, _, cx| {
            let view = shell.install_prepared(summary, &install, cx).unwrap();
            view.update(cx, |view, cx| {
                view.seed_new_settings(
                    vorkspace_core::agent::Settings {
                        worktree_cwd: Some(worktree.clone()),
                        ..Default::default()
                    },
                    cx,
                )
            });
            shell.publish_prepared(&view, Some(&mut install), cx);
        })
        .unwrap();
        drop(install);
        cx.run_until_parked();
        b.update(cx, |shell, _, cx| {
            assert!(shell.session_summaries.contains_key(&id));
            assert_eq!(shell.session_worktrees.get(&id), Some(&Some(worktree)));
            assert!(shell.ensure_session(id, cx).is_none());
        })
        .unwrap();
    }

    #[gpui::test]
    fn a_peer_cannot_claim_a_cold_session_until_its_archive_commits(cx: &mut TestAppContext) {
        let (a, b, _, _dir) = two_windows(cx);
        let summary = a
            .update(cx, |shell, _, _| {
                locked(&shell.store)
                    .unwrap()
                    .create("codex", &shell.workspace.project)
                    .unwrap()
            })
            .unwrap();
        let id = summary.id;
        cx.update(|cx| {
            a.update(cx, |shell, _, cx| {
                shell.session_summaries.insert(id, summary.clone());
                shell.toggle_archive(id, cx);
            })
            .unwrap();
            b.update(cx, |shell, _, cx| {
                shell.session_summaries.insert(id, summary);
                assert!(shell.ensure_session(id, cx).is_none());
            })
            .unwrap();
        });
        cx.run_until_parked();
        b.update(cx, |shell, _, _| {
            assert!(shell.session_summaries[&id].archived)
        })
        .unwrap();
    }

    #[gpui::test]
    fn scheduler_handoff_keeps_new_connector_retries_and_the_durable_snapshot(
        cx: &mut TestAppContext,
    ) {
        let (a, b, _, _dir) = two_windows(cx);
        let item = vorkspace_core::inbox::Item {
            provider: "github".into(),
            kind: "issue".into(),
            id: "event".into(),
            updated_at: "2026-11-09T12:00:00Z".into(),
            ..Default::default()
        };
        let newer = vorkspace_core::inbox::Item {
            updated_at: "2026-11-09T12:01:00Z".into(),
            ..item.clone()
        };
        cx.update(|cx| {
            a.update(cx, |shell, _, cx| {
                assert!(shell.holds_scheduler(cx));
                shell.app_core.as_ref().unwrap().update(cx, |core, cx| {
                    core.queue_automation_events(vec![item.clone()], cx);
                });
            })
            .unwrap();
            b.update(cx, |shell, _, cx| {
                assert!(!shell.holds_scheduler(cx));
                shell.app_core.as_ref().unwrap().update(cx, |core, cx| {
                    core.queue_automation_events(vec![newer.clone()], cx);
                    core.settle_automation_events(vec![item], Vec::new(), cx);
                });
            })
            .unwrap();
            a.update(cx, |_, window, _| window.remove_window()).unwrap();
        });
        cx.run_until_parked();
        b.update(cx, |shell, _, cx| {
            assert!(shell.holds_scheduler(cx));
            assert_eq!(
                shell
                    .app_core
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .automation_events(),
                std::slice::from_ref(&newer)
            );
            let raw = locked(&shell.store)
                .unwrap()
                .setting(AUTOMATION_EVENTS_KEY)
                .unwrap()
                .unwrap();
            let saved: Vec<vorkspace_core::inbox::Item> = serde_json::from_str(&raw).unwrap();
            assert_eq!(saved, [newer]);
        })
        .unwrap();
    }

    #[gpui::test]
    fn a_project_pending_deletion_cannot_be_selected_in_a_peer(cx: &mut TestAppContext) {
        let (a, b, _, dir) = two_windows(cx);
        let path = dir.path().join("deleting");
        std::fs::create_dir(&path).unwrap();
        let path = path.to_string_lossy().into_owned();
        let admissions = a
            .update(cx, |shell, _, cx| {
                shell
                    .app_core
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .project_deletions
                    .clone()
            })
            .unwrap();
        let key = vorkspace_core::files::path_key(&path);
        admissions.lock().unwrap().insert(key.clone());
        b.update(cx, |shell, window, cx| {
            let original = shell.workspace.project.clone();
            shell.select_project(path.clone(), window, cx);
            assert_eq!(shell.workspace.project, original);
            assert!(
                shell
                    .error
                    .as_ref()
                    .is_some_and(|error| error.contains("being deleted"))
            );
        })
        .unwrap();
        admissions.lock().unwrap().remove(&key);
        b.update(cx, |shell, window, cx| {
            shell.select_project(path.clone(), window, cx);
            assert_eq!(shell.workspace.project, path);
        })
        .unwrap();
    }

    #[gpui::test]
    fn committed_folder_moves_are_applied_to_the_latest_groups_and_shared(cx: &mut TestAppContext) {
        let (a, b, first, _dir) = two_windows(cx);
        let (project, second) = a
            .update(cx, |shell, _, _| {
                let db = locked(&shell.store).unwrap();
                let project = shell.workspace.project.clone();
                let second = db.create("codex", &project).unwrap().id;
                (project, second)
            })
            .unwrap();
        cx.update(|cx| {
            for (window, id, label) in [(a, first, "First"), (b, second, "Second")] {
                window
                    .update(cx, |shell, _, cx| {
                        locked(&shell.store)
                            .unwrap()
                            .edit_groups(&project, |groups| {
                                groups.move_session(id, None, Some(label))
                            })
                            .unwrap();
                        shell.publish_groups(project.clone(), cx);
                    })
                    .unwrap();
            }
        });
        for window in [a, b] {
            window
                .update(cx, |shell, _, _| {
                    assert_eq!(
                        shell.groups[&project].folder_of(first).unwrap().name,
                        "First"
                    );
                    assert_eq!(
                        shell.groups[&project].folder_of(second).unwrap().name,
                        "Second"
                    );
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn a_session_keeps_one_live_owner_and_peers_follow_its_catalog(cx: &mut TestAppContext) {
        let (a, b, _, _dir) = two_windows(cx);
        let id = a
            .update(cx, |shell, window, cx| {
                let summary = locked(&shell.store)
                    .unwrap()
                    .create("codex", &shell.workspace.project)
                    .unwrap();
                let id = summary.id;
                shell.add(summary, cx);
                shell.open(id, window, cx);
                id
            })
            .unwrap();
        cx.run_until_parked();
        b.update(cx, |shell, window, cx| {
            assert!(shell.session_summaries.contains_key(&id));
            assert!(!shell.sessions.contains_key(&id));
            let previous = shell.workspace.active;
            shell.open(id, window, cx);
            assert_eq!(shell.workspace.active, previous);
            shell.delete_session(id, window, cx);
            assert!(shell.error.as_deref().unwrap().contains("another window"));
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| cx.active_window()), Some(a.into()));
        a.update(cx, |shell, _, cx| {
            assert_eq!(shell.workspace.active, Some(id));
            locked(&shell.store)
                .unwrap()
                .title(id, "Shared session")
                .unwrap();
            shell.sessions[&id].update(cx, |view, cx| view.rename("Shared session".into(), cx));
        })
        .unwrap();
        cx.run_until_parked();
        b.update(cx, |shell, _, _| {
            assert_eq!(shell.session_summaries[&id].title, "Shared session");
        })
        .unwrap();

        a.update(cx, |shell, window, cx| {
            shell.request_close_window(window, cx)
        })
        .unwrap();
        cx.run_until_parked();
        b.update(cx, |shell, window, cx| {
            shell.open(id, window, cx);
            assert!(shell.sessions.contains_key(&id));
            assert_eq!(shell.workspace.active, Some(id));
        })
        .unwrap();
    }

    #[gpui::test]
    fn quitting_saves_the_latest_vorkspace_definitions(cx: &mut TestAppContext) {
        let (a, _, _, _dir) = two_windows(cx);
        let store = a
            .update(cx, |shell, _, cx| {
                shell.workspace.create_vorkspace("Previous name");
                shell.persist(cx);
                let id = shell.workspace.create_vorkspace("Final name");
                shell
                    .workspace
                    .vorkspace_mut(&id)
                    .unwrap()
                    .set_icon_letters("FN");
                shell.persist(cx);
                shell.store.clone()
            })
            .unwrap();
        cx.quit();
        let saved = locked(&store).unwrap().setting(STORE_KEY).unwrap().unwrap();
        let saved: Saved = serde_json::from_str(&saved).unwrap();
        assert!(
            saved
                .vorkspaces
                .iter()
                .any(|v| v.name == "Final name" && v.icon_letters() == "FN")
        );
    }

    #[gpui::test]
    fn closing_a_deleted_vorkspace_background_returns_to_manage(cx: &mut TestAppContext) {
        let (a, b, _, _dir) = two_windows(cx);
        let deleted = b
            .update(cx, |shell, _, cx| {
                let id = shell.workspace.create_vorkspace("Background");
                shell.persist(cx);
                id
            })
            .unwrap();
        a.update(cx, |shell, window, cx| {
            shell.open_vorkspace_background(deleted.clone(), window, cx);
            assert!(shell.background_dialog.is_some());
        })
        .unwrap();
        b.update(cx, |shell, _, cx| {
            shell.workspace.delete_vorkspace(&deleted);
            shell.persist(cx);
        })
        .unwrap();
        a.update(cx, |shell, window, cx| {
            shell.close_background_dialog(window, cx);
            assert!(shell.background_dialog.is_none());
            assert_eq!(
                shell.vorkspace_dialog.as_ref().unwrap().editing,
                shell.workspace.active_vorkspace
            );
        })
        .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(a.into(), cx);
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("vorkspace-dialog").is_some());
    }

    #[gpui::test]
    fn a_session_is_live_in_one_window_at_a_time(cx: &mut TestAppContext) {
        let (a, b, id, _dir) = two_windows(cx);
        // Whichever window opened it first keeps it; the other only lists it.
        let a_has = a
            .update(cx, |shell, _, cx| shell.ensure_session(id, cx).is_some())
            .unwrap();
        let b_has = b
            .update(cx, |shell, _, cx| shell.ensure_session(id, cx).is_some())
            .unwrap();
        assert!(a_has ^ b_has, "exactly one window holds the live session");
        let (holder, other) = if a_has { (a, b) } else { (b, a) };
        other
            .update(cx, |shell, _, _| {
                assert!(shell.session_summaries.contains_key(&id))
            })
            .unwrap();
        // Once the holder lets it go, the other window can take it.
        holder
            .update(cx, |shell, _, cx| {
                shell.release_live_session(id, cx);
            })
            .unwrap();
        assert!(
            other
                .update(cx, |shell, _, cx| shell.ensure_session(id, cx).is_some())
                .unwrap()
        );
    }

    #[gpui::test]
    fn agent_calls_go_to_the_window_with_their_session(cx: &mut TestAppContext) {
        let (a, b, id, _dir) = two_windows(cx);
        // Make sure B is the window holding the session.
        a.update(cx, |shell, _, cx| {
            shell.release_live_session(id, cx);
        })
        .unwrap();
        assert!(
            b.update(cx, |shell, _, cx| shell.ensure_session(id, cx).is_some())
                .unwrap()
        );
        let (a_root, b_root) = (
            a.update(cx, |_, _, cx| cx.entity()).unwrap(),
            b.update(cx, |_, _, cx| cx.entity()).unwrap(),
        );
        let core = a
            .update(cx, |shell, _, _| shell.app_core.clone().unwrap())
            .unwrap();
        cx.update(|cx| {
            let core = core.read(cx);
            assert_eq!(core.route(id, cx), Some(b_root.clone()));
            // A call from no known session goes to the oldest open window.
            assert_eq!(core.route(-42, cx), Some(a_root.clone()));
        });
    }

    #[gpui::test]
    fn shared_vorkspace_changes_reconcile_other_windows_project_and_dialog(
        cx: &mut TestAppContext,
    ) {
        let (a, b, _, dir) = two_windows(cx);
        let second_project = dir.path().join("second").to_string_lossy().into_owned();
        std::fs::create_dir_all(&second_project).unwrap();
        let side_id = a
            .update(cx, |shell, _, cx| {
                let main = shell.workspace.active_vorkspace.clone();
                shell
                    .workspace
                    .add_project_to_vorkspace(&main, &second_project);
                let side = shell.workspace.create_vorkspace("Side");
                shell.persist(cx);
                side
            })
            .unwrap();
        cx.run_until_parked();
        b.update(cx, |shell, window, cx| {
            shell.select_project(second_project.clone(), window, cx);
            shell.open_vorkspace_dialog(Some(side_id.clone()), window, cx);
            assert_eq!(shell.workspace.project, second_project);
            assert_eq!(
                shell.vorkspace_dialog.as_ref().map(|d| d.editing.as_str()),
                Some(side_id.as_str())
            );
        })
        .unwrap();
        a.update(cx, |shell, _, cx| {
            let main = shell.workspace.active_vorkspace.clone();
            shell
                .workspace
                .remove_project_from_vorkspace(&main, &second_project);
            shell.workspace.delete_vorkspace(&side_id);
            shell.persist(cx);
        })
        .unwrap();
        cx.run_until_parked();
        b.update(cx, |shell, _, cx| {
            assert_ne!(shell.workspace.project, second_project);
            let dialog = shell.vorkspace_dialog.as_ref().unwrap();
            let active = shell.workspace.active_vorkspace().unwrap();
            assert_eq!(dialog.editing, active.id);
            assert_eq!(dialog.name.read(cx).content.as_ref(), active.name);
            dialog.name.update(cx, |field, cx| {
                field.set_text("Renamed after deletion".into(), cx)
            });
            dialog
                .icon_letters
                .update(cx, |field, cx| field.set_text("RD".into(), cx));
        })
        .unwrap();
        cx.run_until_parked();
        a.update(cx, |shell, _, _| {
            let active = shell.workspace.active_vorkspace().unwrap();
            assert_eq!(active.name, "Renamed after deletion");
            assert_eq!(active.icon_letters(), "RD");
            let durable = locked(&shell.store)
                .unwrap()
                .setting(STORE_KEY)
                .unwrap()
                .unwrap();
            let saved: Saved = serde_json::from_str(&durable).unwrap();
            assert!(saved.vorkspaces.contains(active));
        })
        .unwrap();
    }
}
