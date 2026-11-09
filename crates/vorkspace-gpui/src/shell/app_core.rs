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
//! claim: a lease whose window closed, or which no longer has the session
//! loaded, is free. Nothing has to remember to release one.
use super::*;
use crate::{
    bots::{MembershipChanged, SharedRoster},
    workspace::{VorkspaceDef, move_bot, reconcile_bots},
};
use gpui::{AppContext as _, WeakEntity};
use serde::{Deserialize, Serialize};

const STORE_KEY: &str = "vorkspaces";

#[derive(Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    vorkspaces: Vec<VorkspaceDef>,
    seen_bots: HashSet<String>,
}

pub(crate) struct AppCore {
    store: Database,
    writes: Arc<VorkspaceWrites>,
    pub(crate) vorkspaces: Vec<VorkspaceDef>,
    pub(crate) seen_bots: HashSet<String>,
    pub(super) bot_roster: Entity<SharedRoster>,
    scheduler: Option<WeakEntity<Shell>>,
    sessions: HashMap<i64, WeakEntity<Shell>>,
    /// Windows in the order they joined; the oldest open one is the default
    /// destination for work no window owns.
    windows: Vec<WeakEntity<Shell>>,
    /// The agent app bridge: one socket for the application.
    pub(crate) bridge: Option<vorkspace_core::bot::bridge::Server>,
}

#[derive(Default)]
struct VorkspaceWrites {
    pending: Mutex<Option<Arc<str>>>,
}

impl VorkspaceWrites {
    fn flush(&self, store: &Database) -> Result<(), String> {
        // Serialize writes through the store and pick the latest snapshot
        // only after acquiring it, including during app shutdown.
        let store = locked(store)?;
        let snapshot = self.pending.lock().unwrap().clone();
        if let Some(snapshot) = snapshot {
            store.save_setting(STORE_KEY, &snapshot)?;
            let mut pending = self.pending.lock().unwrap();
            if pending
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &snapshot))
            {
                *pending = None;
            }
        }
        Ok(())
    }
}

/// One core per data store: windows on the same store share it, and a
/// separate store (as in tests that open several apps) has its own.
#[derive(Default)]
struct Installed(HashMap<usize, Entity<AppCore>>);
impl gpui::Global for Installed {}

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
        let saved: Saved = locked(store)
            .ok()
            .and_then(|s| s.setting(STORE_KEY).ok().flatten())
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        let writes = Arc::new(VorkspaceWrites::default());
        let quit_writes = writes.clone();
        let quit_store = store.clone();
        cx.on_app_quit(move |cx| {
            let writes = quit_writes.clone();
            let store = quit_store.clone();
            cx.background_executor().spawn(async move {
                if let Err(error) = writes.flush(&store) {
                    eprintln!("Could not save vorkspaces on quit: {error}");
                }
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
                scheduler: None,
                sessions: HashMap::new(),
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
        *self.writes.pending.lock().unwrap() = Some(json.into());
        let writes = self.writes.clone();
        let store = self.store.clone();
        let work = cx
            .background_executor()
            .spawn(async move { writes.flush(&store) });
        cx.spawn(async move |core, cx| {
            if let Err(error) = work.await {
                let _ = core.update(cx, |core, cx| {
                    for window in &core.windows {
                        let _ = window.update(cx, |shell, cx| {
                            shell.error = Some(format!("Could not save vorkspaces: {error}"));
                            cx.notify();
                        });
                    }
                });
            }
        })
        .detach();
        cx.notify();
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

    /// Whether `shell` may load session `id` live. Another window keeps it
    /// only while that window exists and still has the session loaded.
    fn claim_session(&mut self, id: i64, shell: &WeakEntity<Shell>, cx: &App) -> bool {
        if let Some(holder) = self.sessions.get(&id)
            && holder != shell
            && holder
                .upgrade()
                .is_some_and(|other| other.read(cx).sessions.contains_key(&id))
        {
            return false;
        }
        self.sessions.insert(id, shell.clone());
        true
    }
}

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
        if let Some(holder) = self.sessions.get(&session).and_then(|w| w.upgrade())
            && holder.read(cx).sessions.contains_key(&session)
        {
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
                    .is_some_and(|m| m.read(cx).is_bot_session(session))
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
    /// Joins this window to the application's shared state. The first
    /// window seeds it from its own (possibly migrated) definitions.
    pub(super) fn adopt_app_core(&mut self, cx: &mut Context<Self>) {
        let new_projects = std::mem::take(&mut self.workspace.restored_new_projects);
        let core = AppCore::get(&self.store, cx);
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
            if this.take_shared_vorkspaces(&core, cx) {
                this.reconcile_after_shared_change(cx);
            }
        }));
        let me = cx.weak_entity();
        core.update(cx, |core, _| core.register(me));
        self.app_core = Some(core);
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

    /// Whether this window may load session `id` live.
    pub(super) fn claim_session(&mut self, id: i64, cx: &mut Context<Self>) -> bool {
        let Some(core) = self.app_core.clone() else {
            return true;
        };
        let me = cx.weak_entity();
        core.update(cx, |core, cx| core.claim_session(id, &me, cx))
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
            assert!(!shell.session_summaries.contains_key(&id));
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
            .update(cx, |shell, _, _| {
                shell.sessions.remove(&id);
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
        a.update(cx, |shell, _, _| {
            shell.sessions.remove(&id);
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
