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
use gpui::WeakEntity;
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
    pub(crate) vorkspaces: Vec<VorkspaceDef>,
    pub(crate) seen_bots: HashSet<String>,
    scheduler: Option<WeakEntity<Shell>>,
    sessions: HashMap<i64, WeakEntity<Shell>>,
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
            vorkspaces: saved.vorkspaces,
            seen_bots: saved.seen_bots,
            scheduler: None,
            sessions: HashMap::new(),
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
        let store = self.store.clone();
        cx.background_executor()
            .spawn(async move {
                if let Ok(store) = locked(&store) {
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

impl Shell {
    /// Joins this window to the application's shared state. The first
    /// window seeds it from its own (possibly migrated) definitions.
    pub(super) fn adopt_app_core(&mut self, cx: &mut Context<Self>) {
        let core = AppCore::get(&self.store, cx);
        if core.read(cx).vorkspaces.is_empty() {
            let (defs, seen) = (
                self.workspace.vorkspaces.clone(),
                self.workspace.seen_bots.clone(),
            );
            core.update(cx, |core, cx| core.publish(&defs, &seen, cx));
        } else {
            self.take_shared_vorkspaces(&core, cx);
        }
        self.observations.push(cx.observe(&core, |this, core, cx| {
            if this.take_shared_vorkspaces(&core, cx) {
                this.reconcile_after_shared_change(cx);
            }
        }));
        self.app_core = Some(core);
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
        let hidden = self.workspace.hidden_bots();
        if let Some(manager) = self.bots.clone() {
            manager.update(cx, |m, cx| {
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
}
