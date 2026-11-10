//! Project workspace selection, independent of the project's identity. Existing
//! conversations keep their execution directory and running provider process.
use super::*;
use crate::git_tools::worktree_switcher::{Event, TabSession, WorktreeFocus, WorktreeSwitcher};
use vorkspace_core::files::path_key;

impl Shell {
    pub(super) fn working_copy(&self) -> &str {
        self.worktree_focus
            .get(&self.workspace.project)
            .map_or(&self.workspace.project, |focus| &focus.path)
    }
    pub(super) fn pane_working_copy<'a>(&'a self, id: i64, cx: &'a App) -> &'a str {
        self.pane_execution_scope(id, cx)
            .map_or_else(|| self.working_copy(), |(_, worktree)| worktree)
    }

    /// Resolve a saved session directly. A pane can point at another session
    /// (such as detached review) and is never an authority for saved checkout membership.
    pub(super) fn saved_session_scope<'a>(
        &'a self,
        id: i64,
        cx: &'a App,
    ) -> Option<(&'a str, &'a str)> {
        let record = self.session_record(id, cx);
        let live = self.sessions.get(&id).map(|view| view.read(cx));
        let project = live
            .map(|view| view.summary.cwd.as_str())
            .or_else(|| record.map(|row| row.summary.cwd.as_str()))?;
        let checkout = record
            .and_then(SessionRecord::worktree)
            .or_else(|| live.and_then(|view| view.settings.worktree_cwd.as_deref()))
            .unwrap_or(project);
        Some((project, checkout))
    }

    /// A bound leaf supplies the project's identity when restoring a layout.
    pub(super) fn pane_layout_anchor<'a>(
        &'a self,
        pane: i64,
        cx: &'a App,
    ) -> Option<(&'a str, &'a str)> {
        if let Some(id) = self.workspace.session_for_pane(pane) {
            return self.saved_session_scope(id, cx);
        }
        self.workspace
            .app_panes
            .get(&pane)
            .filter(|state| state.content.project_scoped())
            .map(|state| (state.project.as_str(), state.worktree.as_str()))
    }

    /// Resolve project and checkout together before creating a session.
    pub(super) fn pane_execution_scope<'a>(
        &'a self,
        pane: i64,
        cx: &'a App,
    ) -> Option<(&'a str, &'a str)> {
        if let Some(scope) = self.pane_layout_anchor(pane, cx) {
            return Some(scope);
        }
        let state = self.workspace.app_panes.get(&pane)?;
        if let crate::workspace::AppPane::Terminal { cwd, .. } = &state.content {
            return Some((&state.project, cwd));
        }
        Some(
            self.app_layout_scope(pane, cx)
                .unwrap_or_else(|| (&self.workspace.project, self.working_copy())),
        )
    }
    pub(super) fn in_worktree_scope(&self, id: i64, cx: &App) -> bool {
        let scope = self
            .workspace
            .tab_of(id)
            .and_then(|tab| self.workspace.tab_workspaces.get(&tab))
            .map_or_else(|| self.pane_working_copy(id, cx), String::as_str);
        path_key(scope) == path_key(self.working_copy())
    }
    pub(super) fn record_worktree_session(&mut self, id: i64, cx: &App) {
        let project = self.workspace.project.clone();
        let session = self.workspace.session_for_pane(id).is_some();
        let path = if session {
            self.working_copy()
        } else {
            self.app_layout_scope(id, cx)
                .map_or_else(|| self.working_copy(), |(_, worktree)| worktree)
        }
        .to_owned();
        // This pin follows the displayed workspace. The execution directory
        // remains with the session or the resource that owns it.
        if let Some(tab) = self.workspace.tab_of(id) {
            self.workspace.tab_workspaces.insert(tab, path.clone());
        }
        if session {
            self.worktree_returns.insert((project, path), id);
        }
    }
    pub(super) fn worktree_flyout(
        &self,
        events: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Flyout {
        let project = self.workspace.project.clone();
        let ids = self
            .order
            .iter()
            .copied()
            .filter(|id| self.session_metadata(*id, cx).summary.cwd == project)
            .collect();
        let sessions = self
            .workspace
            .tabs
            .iter()
            .flat_map(|tab| {
                self.workspace
                    .layout(*tab)
                    .leaf_ids()
                    .into_iter()
                    .filter_map(|id| {
                        if !self.has_session(id, cx) {
                            return None;
                        }
                        let session = self.session_metadata(id, cx);
                        (session.summary.cwd == project).then(|| TabSession {
                            id,
                            tab: *tab,
                            cwd: self.pane_working_copy(id, cx).to_owned(),
                            workspace: self.workspace.tab_workspaces.get(tab).cloned(),
                            busy: session.busy,
                        })
                    })
            })
            .collect();
        let focus = self.worktree_focus.get(&project).cloned();
        let view = cx.new(|cx| {
            WorktreeSwitcher::new(
                project.clone(),
                focus,
                sessions,
                ids,
                self.store.clone(),
                cx,
            )
        });
        events.push(
            cx.subscribe_in(&view, window, move |this, _, event: &Event, window, cx| {
                match event {
                    Event::Metadata(paths) => {
                        for (id, path) in paths {
                            let path = if let Some(session) = this.sessions.get(id) {
                                session.update(cx, |session, cx| {
                                    session.restore_worktree(path.clone(), None, cx)
                                })
                            } else {
                                path.clone()
                            };
                            this.set_session_worktree(*id, path, cx);
                        }
                        this.sync_git(cx);
                        this.invalidate_chrome(cx);
                    }
                    Event::Selected(focus) if this.workspace.project == project => {
                        this.close_menu(cx);
                        this.select_worktree(focus.clone(), window, cx);
                    }
                    Event::Selected(_) | Event::Dismissed => {
                        this.close_menu(cx);
                        this.focus_workspace(window, cx);
                    }
                }
                cx.notify();
            }),
        );
        Flyout {
            focus: Some(view.read(cx).focus(cx)),
            view: view.into(),
            width: 280.,
            padding: 0.,
            height: None,
            max_height: Some(360.),
            surface: None,
        }
    }
    pub(super) fn select_worktree(
        &mut self,
        focus: Option<WorktreeFocus>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.workspace.project.clone();
        let target = focus.as_ref().map_or(&project, |f| &f.path).clone();
        if !self.allow_project_entry(&target, cx) {
            return;
        }
        if let Some(id) = self
            .workspace
            .active
            .filter(|id| self.workspace.session_for_pane(*id).is_some())
        {
            self.worktree_returns
                .insert((project.clone(), self.working_copy().into()), id);
        }
        match focus {
            Some(focus) => {
                self.worktree_focus.insert(project.clone(), focus);
            }
            None => {
                self.worktree_focus.remove(&project);
            }
        }
        self.navigation_revision += 1;
        self.leave_app_views(cx);
        self.close_preview(cx);
        if let Some(bots) = self.bots.clone() {
            bots.update(cx, |m, cx| m.close(cx));
        }
        if self.restore_scope_tab(true, window, cx) {
            return;
        }
        let matches = |id: &i64| {
            let Some(owner) = self.workspace.session_for_pane(*id) else {
                return false;
            };
            self.has_session(owner, cx)
                && {
                    let s = self.session_metadata(owner, cx);
                    s.summary.cwd == project && !s.summary.archived
                }
                && self.in_worktree_scope(*id, cx)
                && self.workspace.tab_of(*id).is_some()
        };
        let active = self
            .worktree_returns
            .get(&(project.clone(), target.clone()))
            .copied()
            .filter(matches)
            .or_else(|| self.workspace.active.filter(matches))
            .or_else(|| {
                self.workspace
                    .tabs
                    .iter()
                    .rev()
                    .flat_map(|tab| self.workspace.layout(*tab).leaf_ids())
                    .find(matches)
            });
        self.workspace.active = active;
        if let Some(id) = active {
            self.activate_session(id, window, cx);
        } else {
            self.ensure_project_session(window, cx);
        }
        self.focus_active(window, cx);
        self.persist(cx);
        cx.notify();
    }
    pub(super) fn seed_worktree(
        &mut self,
        id: i64,
        focus: Option<&WorktreeFocus>,
        cx: &mut Context<Self>,
    ) {
        let path = focus.map(|f| f.path.clone());
        self.set_session_worktree(id, path.clone(), cx);
        if let Some(session) = self.sessions.get(&id) {
            session.update(cx, |session, cx| {
                session.restore_worktree(path, focus.and_then(|f| f.branch.clone()), cx);
            });
        }
    }
}

#[cfg(test)]
mod tests;
