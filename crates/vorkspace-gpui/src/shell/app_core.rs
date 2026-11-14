//! The one owner, per application, of what every window shares.
//!
//! Windows are views of a vorkspace. What must not run or exist twice lives
//! here instead of in a window's `Shell`:
//!
//! - Vorkspace definitions, saved under their own key, so every window lists
//!   the same vorkspaces and an edit in one appears in the others.
//! - The committed Bot roster, which decides when membership changes.
//! - The reminder schedule and native notification routing belong to the app.
//! - The automation scheduler lease moves when its window closes.
//! - Session leases: a session is live in at most one window, so it never
//!   gets two `Conversation`s writing one transcript or running one turn.
//!
//! Leases name the window that holds them and are re-validated at every
//! claim: a lease whose window closed is free. A window also releases the lease
//! when it drops the loaded conversation.
mod inbox_feed;
mod layouts;
mod notifications;
mod preferences;
mod reminders;
pub(super) use reminders::ReminderEdit;
mod scheduler;
mod sessions;
mod settings_writes;

use self::settings_writes::SettingsWrites;
use super::windows::{WindowLayouts, WindowShared};
use super::*;
use crate::{
    bots::{MembershipChanged, PreparedRoster, SharedRoster},
    workspace::{VorkspaceDef, move_bot, reconcile_bots},
};
use gpui::{AppContext as _, EventEmitter, WeakEntity};
use serde::{Deserialize, Serialize};

const STORE_KEY: &str = "vorkspaces";
const AUTOMATION_EVENTS_KEY: &str = "native-automation-inbox-retries";

#[derive(Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    vorkspaces: Arc<Vec<VorkspaceDef>>,
    seen_bots: Arc<HashSet<String>>,
}

#[derive(Default)]
pub(crate) struct PreparedCore {
    saved: Saved,
    pub(crate) layouts: WindowLayouts,
    automation_events: HashMap<String, vorkspace_core::inbox::Item>,
    data_dir: Option<std::path::PathBuf>,
    roster: Option<PreparedRoster>,
    reminders: Vec<vorkspace_core::store::Reminder>,
}

impl PreparedCore {
    pub(crate) fn read(store: &vorkspace_core::store::Store, layouts: WindowLayouts) -> Self {
        let saved = store
            .setting(STORE_KEY)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        let automation_events = store
            .setting(AUTOMATION_EVENTS_KEY)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str::<Vec<vorkspace_core::inbox::Item>>(&raw).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|item| (item.key(), item))
            .collect();
        Self {
            saved,
            layouts,
            automation_events,
            data_dir: store.data_dir().map(std::path::Path::to_path_buf),
            roster: Some(PreparedRoster::read(store)),
            reminders: store.reminders().unwrap_or_default(),
        }
    }
}

pub(crate) struct AppCore {
    pub(super) store: Database,
    data_dir: Option<std::path::PathBuf>,
    writes: Arc<SettingsWrites>,
    settings_flush: Option<gpui::Task<()>>,
    settings_flush_again: bool,
    pub(crate) vorkspaces: Arc<Vec<VorkspaceDef>>,
    pub(crate) seen_bots: Arc<HashSet<String>>,
    pub(super) bot_roster: Entity<SharedRoster>,
    pub(super) layouts: WindowLayouts,
    next_window_id: u64,
    pub(super) checkouts: Entity<crate::git_status::CheckoutCatalog>,
    pub(super) quick_shortcut: crate::composer::quick_composer::Shortcut,
    scheduler: Option<WeakEntity<Shell>>,
    scheduler_clock: Option<gpui::Task<()>>,
    reminders: reminders::ReminderSchedule,
    pub(super) github_activity: Entity<crate::notifications::github_activity::GithubActivity>,
    inbox_links: Entity<crate::inbox::SessionLinks>,
    inbox_feed: Option<Entity<crate::inbox::InboxFeed>>,
    linked_activity_clock: Option<gpui::Task<()>>,
    linked_activity_started_at: u64,
    pub(super) notification_permission: crate::notifications::delivery::Permission,
    notification_permission_probe: Option<gpui::Task<()>>,
    notification_clicks: Option<gpui::Task<()>>,
    automation_service: Option<super::automation_runs::ServiceHandle>,
    automation_opening: Option<gpui::Task<()>>,
    automation_events: HashMap<String, vorkspace_core::inbox::Item>,
    dock_badges: HashMap<u64, usize>,
    idle_protection: HashMap<u64, HashSet<i64>>,
    pub(super) directory: Entity<SessionDirectory>,
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

pub(crate) fn reminder_write_barriers(cx: &mut App) -> Vec<async_channel::Receiver<()>> {
    let cores = cx
        .try_global::<Installed>()
        .map(|installed| installed.0.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    cores
        .into_iter()
        .filter_map(|core| core.update(cx, |core, _| core.reminder_write_barrier()))
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
        let prepared = locked(store)
            .map(|db| PreparedCore::read(&db, WindowLayouts::load(&db)))
            .unwrap_or_default();
        Self::install(store, prepared, cx)
    }

    pub(crate) fn install(
        store: &Database,
        prepared: PreparedCore,
        cx: &mut App,
    ) -> Entity<AppCore> {
        let key = store_key(store);
        if let Some(core) = cx.try_global::<Installed>().and_then(|i| i.0.get(&key)) {
            return core.clone();
        }
        let PreparedCore {
            saved,
            layouts,
            automation_events,
            data_dir,
            roster,
            reminders,
        } = prepared;
        let next_window_id = layouts
            .entries
            .iter()
            .map(|entry| entry.id)
            .max()
            .unwrap_or(0)
            + 1;
        let writes = Arc::new(SettingsWrites::default());
        let quit_writes = writes.clone();
        let quit_store = store.clone();
        cx.on_app_quit(move |cx| {
            let writes = quit_writes.clone();
            let store = quit_store.clone();
            cx.background_executor().spawn(async move {
                if let Err(error) = writes.flush(&store) {
                    eprintln!("Could not save workspace settings on quit: {error}");
                }
            })
        })
        .detach();
        let bot_roster = match roster {
            Some(roster) => SharedRoster::install_prepared(store.clone(), roster, cx),
            None => SharedRoster::install(store.clone(), cx),
        };
        let checkouts = cx.new(|_| crate::git_status::CheckoutCatalog::default());
        let github_activity = cx.new(|_| Default::default());
        let inbox_links = cx.new(|_| Default::default());
        let directory = cx.new(|_| SessionDirectory::default());
        let core = cx.new(|cx: &mut Context<AppCore>| {
            AppCore::observe_linked_activity(&github_activity, cx);
            cx.subscribe(&bot_roster, |core, _, event: &MembershipChanged, cx| {
                core.reconcile_bots(
                    event.vorkspace.as_deref().unwrap_or_default(),
                    &event.ids,
                    cx,
                );
            })
            .detach();
            cx.observe(&bot_roster, |core, _, cx| core.share_projects_with_bots(cx))
                .detach();
            AppCore {
                store: store.clone(),
                data_dir,
                writes,
                settings_flush: None,
                settings_flush_again: false,
                vorkspaces: saved.vorkspaces,
                seen_bots: saved.seen_bots,
                bot_roster,
                layouts,
                next_window_id,
                checkouts,
                quick_shortcut: Default::default(),
                scheduler: None,
                scheduler_clock: None,
                reminders: reminders::ReminderSchedule::new(reminders),
                github_activity,
                inbox_links,
                inbox_feed: None,
                linked_activity_clock: None,
                linked_activity_started_at: vorkspace_core::time::now_ms(),
                notification_permission: crate::notifications::delivery::Permission::Unsupported,
                notification_permission_probe: None,
                notification_clicks: None,
                automation_service: None,
                automation_opening: None,
                automation_events,
                dock_badges: HashMap::new(),
                idle_protection: HashMap::new(),
                directory,
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
        core.update(cx, |core, cx| core.share_projects_with_bots(cx));
        core
    }

    /// Every placed Bot works on all the projects of its vorkspace, and on
    /// none while it is listed in no vorkspace.
    fn share_projects_with_bots(&mut self, cx: &mut Context<Self>) {
        let projects = self
            .seen_bots
            .iter()
            .map(|bot| {
                let home = self.vorkspaces.iter().find(|def| def.bots.contains(bot));
                // Match what the roster keeps, or the two would never agree.
                let mut listed = HashSet::new();
                let projects = home
                    .into_iter()
                    .flat_map(|def| &def.projects)
                    .filter(|path| {
                        !path.trim().is_empty()
                            && listed.insert(vorkspace_core::files::path_key(path))
                    })
                    .cloned()
                    .collect();
                (bot.clone(), projects)
            })
            .collect();
        self.bot_roster.update(cx, |roster, cx| {
            roster.follow_vorkspace_projects(projects, cx);
        });
    }

    fn reconcile_bots(&mut self, destination: &str, ids: &[String], cx: &mut Context<Self>) {
        let mut defs = self.vorkspaces.clone();
        let mut seen = self.seen_bots.clone();
        if reconcile_bots(
            Arc::make_mut(&mut defs).as_mut_slice(),
            Arc::make_mut(&mut seen),
            destination,
            ids,
        ) {
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
        let mutable = Arc::make_mut(&mut defs);
        let Some(destination) = mutable.iter_mut().find(|def| def.id == vorkspace) else {
            return;
        };
        if add {
            if !self.seen_bots.contains(bot) {
                return;
            }
            move_bot(mutable, vorkspace, bot);
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
        let mut definitions = self.vorkspaces.clone();
        let defs = Arc::make_mut(&mut definitions);
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
        self.publish(&definitions, &self.seen_bots.clone(), cx);
    }

    /// Replaces the shared definitions when a window changed them.
    fn publish(
        &mut self,
        vorkspaces: &Arc<Vec<VorkspaceDef>>,
        seen: &Arc<HashSet<String>>,
        cx: &mut Context<Self>,
    ) {
        if &self.vorkspaces == vorkspaces && &self.seen_bots == seen {
            return;
        }
        self.vorkspaces.clone_from(vorkspaces);
        self.seen_bots.clone_from(seen);
        self.writes.stage_definitions(Saved {
            vorkspaces: self.vorkspaces.clone(),
            seen_bots: self.seen_bots.clone(),
        });
        self.flush_settings(cx);
        self.share_projects_with_bots(cx);
        cx.notify();
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

    /// Starts the bridge once and hands each call to its owning window.
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
        if self.reconcile_project_admission(&core, cx) {
            self.sync_git(cx);
        }
        self.sync_inbox_scope(cx);
        self.observations.push(cx.observe(&core, |this, core, cx| {
            let definitions_changed = this.take_shared_vorkspaces(&core, cx);
            if this.take_shared_window_state(&core, cx) || definitions_changed {
                this.reconcile_after_shared_change(cx);
            }
        }));
        self.observe_session_catalog(&core, cx);
        self.observe_reminders(&core, cx);
        self.observations.push(cx.subscribe(
            &core,
            |this, _, ready: &scheduler::AutomationServiceReady, cx| {
                this.automation_service_ready(ready.0.clone(), cx);
            },
        ));
        let me = cx.weak_entity();
        core.update(cx, |core, cx| {
            core.register(me);
            core.start_scheduler_clock(cx);
            core.start_reminders(cx);
        });
        let release = core.downgrade();
        let window = self.window.layout;
        cx.on_release(move |_, cx| {
            let _ = release.update(cx, |core, _| {
                core.keep_visible_agents(window, HashSet::new());
                if !core.windows.iter().any(|window| window.upgrade().is_some()) {
                    core.scheduler_clock = None;
                    core.linked_activity_clock = None;
                    core.notification_clicks = None;
                    core.reminders.stop_discovery();
                }
            });
        })
        .detach();
        self.app_core = Some(core);
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

    /// Adopt the app's immutable definitions and discard the local edit copy.
    fn take_shared_vorkspaces(&mut self, core: &Entity<AppCore>, cx: &App) -> bool {
        let core = core.read(cx);
        let same_definitions = Arc::ptr_eq(&self.workspace.vorkspaces, &core.vorkspaces)
            || self.workspace.vorkspaces == core.vorkspaces;
        let same_membership = Arc::ptr_eq(&self.workspace.seen_bots, &core.seen_bots)
            || self.workspace.seen_bots == core.seen_bots;
        self.vorkspace_baseline.clone_from(&core.vorkspaces);
        self.workspace.vorkspaces.clone_from(&core.vorkspaces);
        self.workspace.seen_bots.clone_from(&core.seen_bots);
        if !same_definitions || !same_membership {
            self.workspace.ensure_vorkspaces();
        }
        !same_definitions || !same_membership
    }

    fn reconcile_after_shared_change(&mut self, cx: &mut Context<Self>) {
        if self
            .app_core
            .clone()
            .is_some_and(|core| self.reconcile_project_admission(&core, cx))
        {
            self.sync_git(cx);
        }
        self.sync_inbox_scope(cx);
        if let Some(dialog) = self.project_settings.vorkspace_dialog.as_mut() {
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
        if !self.sync_vorkspace_bot_visibility(cx) {
            self.refresh_pane_visibility(cx);
        }
        self.invalidate_chrome(cx);
        cx.notify();
    }

    fn reconcile_project_admission(&mut self, core: &Entity<AppCore>, cx: &App) -> bool {
        let blocked = core.read(cx).project_deletions.lock().unwrap();
        self.workspace.reconcile_project_scope_where(|path| {
            !blocked
                .iter()
                .any(|root| super::project_delete::within(path, root))
                && !vorkspace_core::worktree_management::is_being_removed(path)
        })
    }

    /// Hands this window's definitions to the other windows when they changed.
    pub(super) fn share_vorkspaces(&mut self, cx: &mut Context<Self>) {
        let Some(core) = self.app_core.clone() else {
            return;
        };
        if !Arc::ptr_eq(&self.workspace.vorkspaces, &self.vorkspace_baseline)
            && self.workspace.vorkspaces != self.vorkspace_baseline
        {
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

    /// Aggregate attention from each window for the app's Dock tile.
    pub(super) fn set_window_badge(&self, badge: usize, cx: &mut Context<Self>) {
        if let Some(core) = &self.app_core {
            core.update(cx, |core, _| {
                if badge == 0 {
                    core.dock_badges.remove(&self.window.layout);
                } else {
                    core.dock_badges.insert(self.window.layout, badge);
                }
                crate::notifications::platform::set_dock_badge(core.dock_badges.values().sum());
            });
        }
    }

    pub(super) fn other_window_uses_project(&self, path: &str, cx: &Context<Self>) -> bool {
        self.any_peer_window(cx, |shell, cx| shell.window_uses_project(path, cx))
    }

    pub(super) fn other_window_uses_worktree(&self, path: &str, cx: &Context<Self>) -> bool {
        self.any_peer_window(cx, |shell, cx| shell.window_uses_worktree(path, cx))
    }

    fn any_peer_window(&self, cx: &Context<Self>, check: impl Fn(&Self, &App) -> bool) -> bool {
        let Some(core) = &self.app_core else {
            return false;
        };
        let me = cx.weak_entity();
        core.read(cx)
            .windows
            .iter()
            .filter(|window| *window != &me)
            .filter_map(WeakEntity::upgrade)
            .any(|shell| check(shell.read(cx), cx))
    }
}

#[cfg(test)]
mod tests;
