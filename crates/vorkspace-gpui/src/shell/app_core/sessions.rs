//! Live session leases, admission and catalog events shared by workspace windows.
use super::*;

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
            || self
                .metadata_writes
                .get(&id)
                .is_some_and(|owner| owner != shell && owner.upgrade().is_some())
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

    pub(in crate::shell) fn session_owner(&self, session: i64, cx: &App) -> Option<Entity<Shell>> {
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
}

impl Shell {
    pub(super) fn observe_session_catalog(
        &mut self,
        core: &Entity<AppCore>,
        cx: &mut Context<Self>,
    ) {
        self.observations
            .push(cx.subscribe(core, |this, _, event: &CatalogChanged, cx| {
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
            .push(cx.subscribe(core, |this, _, event: &GroupsChanged, cx| {
                if this.groups.get(&event.project) != Some(&event.groups) {
                    this.groups
                        .insert(event.project.clone(), event.groups.clone());
                    this.invalidate_chrome(cx);
                    cx.notify();
                }
            }));
    }

    /// Whether this window may load session `id` live.
    pub(in crate::shell) fn claim_session(
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

    pub(in crate::shell) fn session_available_here(&self, id: i64, cx: &Context<Self>) -> bool {
        self.session_available_for_install(id, None, cx)
    }

    pub(in crate::shell) fn session_available_for_install(
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

    pub(in crate::shell) fn begin_session_metadata(&self, ids: &[i64], cx: &mut Context<Self>) {
        if let Some(core) = &self.app_core {
            let owner = cx.weak_entity();
            core.update(cx, |core, _| {
                for id in ids {
                    core.metadata_writes.insert(*id, owner.clone());
                }
            });
        }
    }

    pub(in crate::shell) fn finish_session_metadata(&self, ids: &[i64], cx: &mut Context<Self>) {
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

    pub(in crate::shell) fn publish_groups(&mut self, project: String, cx: &mut Context<Self>) {
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

    pub(in crate::shell) fn session_install(&self, cx: &App) -> crate::session_install::Install {
        self.app_core
            .as_ref()
            .expect("registered workspace window")
            .read(cx)
            .session_installs
            .begin(&self.preparing_sessions)
    }

    pub(in crate::shell) fn focus_session_window(&self, id: i64, cx: &mut Context<Self>) -> bool {
        self.focus_session_window_with(id, move |shell, window, cx| shell.open(id, window, cx), cx)
    }

    pub(in crate::shell) fn peer_session_view(
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

    pub(in crate::shell) fn focus_session_window_with(
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

    pub(in crate::shell) fn release_live_session(&mut self, id: i64, cx: &mut Context<Self>) {
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

    pub(in crate::shell) fn reserve_session_deletion(
        &self,
        id: i64,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.session_available_here(id, cx) {
            return false;
        }
        self.app_core
            .as_ref()
            .is_none_or(|core| core.update(cx, |core, _| core.session_deletions.insert(id)))
    }

    pub(in crate::shell) fn release_session_deletion(&self, id: i64, cx: &mut Context<Self>) {
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

    pub(in crate::shell) fn publish_session_summary(&mut self, id: i64, cx: &mut Context<Self>) {
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

    pub(in crate::shell) fn publish_session_deletion(&self, id: i64, cx: &mut Context<Self>) {
        self.emit_catalog_change(SessionCatalogChange::Deleted(id), cx);
    }
}
