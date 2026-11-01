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
    pub(super) fn session_working_copy<'a>(&'a self, id: i64, cx: &'a App) -> &'a str {
        let id = self.workspace.session_for_pane(id);
        let session = self.session_metadata(id, cx);
        self.session_worktrees
            .get(&id)
            .and_then(|p| p.as_deref())
            .or_else(|| {
                self.sessions
                    .get(&id)
                    .and_then(|v| v.read(cx).settings.worktree_cwd.as_deref())
            })
            .unwrap_or(&session.summary.cwd)
    }
    pub(super) fn in_worktree_scope(&self, id: i64, cx: &App) -> bool {
        let scope = self
            .workspace
            .tab_of(id)
            .and_then(|tab| self.workspace.tab_workspaces.get(&tab))
            .map_or_else(|| self.session_working_copy(id, cx), String::as_str);
        path_key(scope) == path_key(self.working_copy())
    }
    pub(super) fn in_sidebar_worktree_scope(&self, id: i64, cx: &App) -> bool {
        self.worktree_focus
            .get(&self.workspace.project)
            .is_none_or(|focus| {
                path_key(self.session_working_copy(id, cx)) == path_key(&focus.path)
            })
    }
    pub(super) fn record_worktree_session(&mut self, id: i64, _cx: &App) {
        let project = self.workspace.project.clone();
        let path = self.working_copy().to_owned();
        // Opening a saved chat must not silently narrow the sidebar. Only an
        // explicit workspace selection changes that project-wide filter.
        if let Some(tab) = self.workspace.tab_of(id) {
            self.workspace.tab_workspaces.insert(tab, path.clone());
        }
        self.worktree_returns.insert((project, path), id);
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
                        if !self.session_summaries.contains_key(&id) {
                            return None;
                        }
                        let session = self.session_metadata(id, cx);
                        (session.summary.cwd == project).then(|| TabSession {
                            id,
                            tab: *tab,
                            cwd: self.session_working_copy(id, cx).to_owned(),
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
                            this.session_worktrees.insert(*id, path.clone());
                            if let Some(session) = this.sessions.get(id) {
                                let path = session.update(cx, |session, cx| {
                                    session.restore_worktree(path.clone(), None, cx)
                                });
                                this.session_worktrees.insert(*id, path);
                            }
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
        if let Some(id) = self.workspace.active {
            self.worktree_returns
                .insert((project.clone(), self.working_copy().into()), id);
        }
        let target = focus.as_ref().map_or(&project, |f| &f.path).clone();
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
        if self.restore_scope_tab(window, cx) {
            return;
        }
        let matches = |id: &i64| {
            self.session_summaries
                .contains_key(&self.workspace.session_for_pane(*id))
                && {
                    let s = self.session_metadata(self.workspace.session_for_pane(*id), cx);
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
        self.session_worktrees.insert(id, path.clone());
        if let Some(session) = self.sessions.get(&id) {
            session.update(cx, |session, cx| {
                session.restore_worktree(path, focus.and_then(|f| f.branch.clone()), cx);
            });
        }
    }
}

#[cfg(test)]
mod tests;
