//! The one owner, per application, of what every window shares.
//!
//! Windows are views of a vorkspace. What must not run or exist twice lives
//! here instead of in a window's `Shell`:
//!
//! - Vorkspace definitions, saved under their own key, so every window lists
//!   the same vorkspaces and an edit in one appears in the others.
//! - The scheduler lease: exactly one window fires reminders and
//!   automations. It moves to another window when its holder closes.
//! - Session leases: a session is live in at most one window, so it never
//!   gets two `Conversation`s writing one transcript or running one turn.
//!
//! Leases name the window that holds them and are re-validated at every
//! claim: a lease whose window closed, or which no longer has the session
//! loaded, is free. Nothing has to remember to release one.
use super::*;
use crate::workspace::VorkspaceDef;
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
    generation: Arc<AtomicU64>,
    pub(crate) vorkspaces: Vec<VorkspaceDef>,
    pub(crate) seen_bots: HashSet<String>,
    scheduler: Option<WeakEntity<Shell>>,
    sessions: HashMap<i64, WeakEntity<Shell>>,
    /// Windows in the order they joined; the oldest open one is the default
    /// destination for work no window owns.
    windows: Vec<WeakEntity<Shell>>,
    /// The agent app bridge: one socket for the application.
    pub(crate) bridge: Option<vorkspace_core::bot::bridge::Server>,
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
        let core = cx.new(|_| AppCore {
            store: store.clone(),
            generation: Arc::new(AtomicU64::new(0)),
            vorkspaces: saved.vorkspaces,
            seen_bots: saved.seen_bots,
            scheduler: None,
            sessions: HashMap::new(),
            windows: Vec::new(),
            bridge: None,
        });
        cx.default_global::<Installed>().0.insert(key, core.clone());
        core
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
        .unwrap_or_default();
        let revision = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let generation = self.generation.clone();
        let store = self.store.clone();
        cx.background_executor()
            .spawn(async move {
                if let Ok(store) = locked(&store)
                    && generation.load(Ordering::SeqCst) == revision
                {
                    let _ = store.save_setting(STORE_KEY, &json);
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
        let alive = || self.windows.iter().filter_map(|w| w.upgrade());
        if let Some(holder) = self.sessions.get(&session).and_then(|w| w.upgrade())
            && holder.read(cx).sessions.contains_key(&session)
        {
            return Some(holder);
        }
        alive()
            .find(|shell| {
                shell
                    .read(cx)
                    .bots
                    .as_ref()
                    .is_some_and(|m| m.read(cx).is_bot_session(session))
            })
            .or_else(|| alive().next())
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
            if !self
                .workspace
                .vorkspaces
                .iter()
                .any(|v| v.id == dialog.editing)
            {
                dialog.editing = self.workspace.active_vorkspace.clone();
                dialog.project = None;
                dialog.confirm_delete = false;
                if let Some(name) = self.workspace.active_vorkspace().map(|v| v.name.clone()) {
                    dialog.name.update(cx, |input, cx| input.set_text(name, cx));
                }
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
        }
        let hidden = self.workspace.hidden_bots();
        if let Some(manager) = self.bots.clone() {
            manager.update(cx, |m, cx| {
                if m.active.as_ref().is_some_and(|id| hidden.contains(id)) {
                    m.close(cx);
                }
                m.vorkspace_hidden = hidden;
                cx.notify();
            });
        }
        self.invalidate_chrome(cx);
        cx.notify();
    }

    /// Hands this window's definitions to the other windows when they changed.
    pub(super) fn share_vorkspaces(&mut self, cx: &mut Context<Self>) {
        let Some(core) = self.app_core.clone() else {
            return;
        };
        let (defs, seen) = (
            self.workspace.vorkspaces.clone(),
            self.workspace.seen_bots.clone(),
        );
        core.update(cx, |core, cx| core.publish(&defs, &seen, cx));
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
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project").to_string_lossy().into_owned();
        std::fs::create_dir_all(&project).unwrap();
        let store = Store::open(&dir.path().join("native.sqlite")).unwrap();
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
        b.update(cx, |shell, _, _| {
            assert_ne!(shell.workspace.project, second_project);
            assert_eq!(
                shell.vorkspace_dialog.as_ref().map(|d| d.editing.as_str()),
                Some(shell.workspace.active_vorkspace.as_str())
            );
        })
        .unwrap();
    }
}
