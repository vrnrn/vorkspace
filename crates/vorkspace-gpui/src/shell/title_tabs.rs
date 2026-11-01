//! All title-strip commands operate on the same visible, ordered snapshot.
use super::*;
use crate::workspace::TitleTab;
use vorkspace_core::files::path_key;

impl Shell {
    /// The workspace keys a tab by one of its sessions. Replacing that session
    /// changes the key, but the same slot and selected width stay on screen.
    pub(super) fn replace_title_tab_key(&mut self, from: i64, to: i64) {
        if from == to {
            return;
        }
        let previous = TitleTab::Session(from);
        let next = TitleTab::Session(to);
        self.title_tab_motion.replace_key(&previous, next.clone());
        for selected in [
            &mut self.title_tab_selection.0,
            &mut self.title_tab_selection.1,
        ] {
            if selected.as_ref() == Some(&previous) {
                *selected = Some(next.clone());
            }
        }
        let shift = self.tab_shifts.remove(&previous);
        self.tab_shifts.remove(&next);
        if let Some(shift) = shift {
            self.tab_shifts.insert(next, shift);
        }
        self.pane_ticks.remove(&from);
        self.pane_ticks.remove(&to);
        if self.tab_hover == Some(from) {
            self.tab_hover = Some(to);
        }
    }

    pub(crate) fn click_bot_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.skip_tab_click) {
            return;
        }
        self.open_title_tab(&self.bot_tab(id.into()), window, cx);
    }
    pub(super) fn tab_scope(&self) -> String {
        format!(
            "{}\0{}",
            path_key(&self.workspace.project),
            path_key(self.working_copy())
        )
    }

    pub(super) fn bot_tab(&self, id: String) -> TitleTab {
        TitleTab::Bot {
            id,
            project: self.workspace.project.clone(),
            worktree: self.working_copy().into(),
        }
    }

    pub(super) fn visible_tabs(&self, cx: &App) -> Vec<TitleTab> {
        self.workspace
            .title_order
            .iter()
            .filter(|tab| match tab {
                TitleTab::Terminal { .. } => true,
                TitleTab::Session(tab) => {
                    self.workspace.tabs.contains(tab) && {
                        let id = self.workspace.tab_focus(*tab);
                        self.session_summaries
                            .contains_key(&self.workspace.session_for_pane(id))
                            && self
                                .session_metadata(self.workspace.session_for_pane(id), cx)
                                .summary
                                .cwd
                                == self.workspace.project
                            && self.in_worktree_scope(id, cx)
                    }
                }
                TitleTab::Search | TitleTab::App(_) | TitleTab::Bot { .. } => false,
            })
            .cloned()
            .collect()
    }
    pub(super) fn active_title_tab(&self, cx: &App) -> Option<TitleTab> {
        if let Some(active) = self.active_terminal_tab {
            return self
                .workspace
                .title_order
                .iter()
                .find(|tab| matches!(tab, TitleTab::Terminal { id, .. } if *id == active))
                .cloned();
        }
        if let Some(view) = self.active_app_view {
            return Some(TitleTab::App(view));
        }
        if self.app_search.is_some() {
            return Some(TitleTab::Search);
        }
        self.bots
            .as_ref()
            .and_then(|m| m.read(cx).active.clone())
            .map(|id| self.bot_tab(id))
            .or_else(|| self.workspace.active_tab().map(TitleTab::Session))
    }

    /// The active tab follows live focus; other tabs keep their last displayed
    /// pane instead of borrowing the currently selected tab's preview focus.
    pub(super) fn tab_review_focused(&self, tab: i64, cx: &App) -> bool {
        if self.active_title_tab(cx) == Some(TitleTab::Session(tab)) {
            self.review_focused
        } else {
            self.pane_ticks
                .get(&tab)
                .is_some_and(|tick| tick.shown == (self.workspace.tab_focus(tab), true))
        }
    }

    /// Sidebar pages fill the pane without a tab strip.
    pub(super) fn tabless_page(&self, cx: &App) -> bool {
        self.active_app_view.is_some() || self.app_search.is_some() || self.bot_active(cx)
    }

    pub(super) fn remember_title_tab(&mut self, tab: TitleTab) {
        if !tab.in_strip() {
            return;
        }
        if !self.workspace.title_order.contains(&tab) {
            self.workspace.title_order.push(tab.clone());
        }
        self.workspace.scope_focus.insert(self.tab_scope(), tab);
    }

    pub(super) fn bot_opened(&mut self, id: String, cx: &mut Context<Self>) {
        if !self
            .bots
            .as_ref()
            .is_some_and(|m| m.read(cx).active.as_deref() == Some(&id))
        {
            return;
        }
        self.navigation_revision += 1;
        self.selected_sessions.clear();
        self.selection_anchor = None;
        self.sidebar_target = None;
        self.context_selection = false;
        self.leave_app_views(cx);
        self.leave_app_search(cx);
        self.close_preview(cx);
        self.review_focused = false;
        self.remember_title_tab(self.bot_tab(id));
        self.reveal_active_tab(cx);
        self.persist(cx);
        cx.notify();
    }

    pub(super) fn open_title_tab(
        &mut self,
        tab: &TitleTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match tab {
            TitleTab::Terminal { id, cwd } => self.show_terminal_tab(*id, cwd.clone(), window, cx),
            TitleTab::Search => self.open_app_search(window, cx),
            TitleTab::App(view) => match view {
                app_views::AppView::Inbox => self.open_inbox(window, cx),
                app_views::AppView::Connector(provider) => {
                    self.open_connector(*provider, window, cx)
                }
                app_views::AppView::Notes => self.open_notes(window, cx),
                app_views::AppView::Automations => self.open_automations(window, cx),
                app_views::AppView::Todos => self.open_todos(window, cx),
            },
            TitleTab::Session(id) => {
                let focus = self.workspace.tab_focus(*id);
                let review_focused = self.tab_review_focused(*id, cx);
                self.open(focus, window, cx);
                if review_focused
                    && self.workspace.active == Some(focus)
                    && let Some(review) = self.active_review(cx)
                {
                    self.review_focused = true;
                    review.update(cx, |r, cx| r.focus_active(window, cx));
                }
            }
            TitleTab::Bot { id, .. } => {
                if let Some(bots) = self.bots.clone() {
                    bots.update(cx, |m, cx| m.open(id, window, cx));
                }
            }
        }
    }
    pub(super) fn restore_scope_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let tab = self.workspace.scope_focus.get(&self.tab_scope()).cloned();
        if let Some(tab) = tab.filter(|tab| self.visible_tabs(cx).contains(tab)) {
            if matches!(tab, TitleTab::Bot { .. }) {
                self.workspace.active = None;
            }
            self.open_title_tab(&tab, window, cx);
            return true;
        }
        false
    }

    pub(super) fn run_title_tab_menu(
        &mut self,
        tab: TitleTab,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tabs = self.visible_tabs(cx);
        let Some(index) = tabs.iter().position(|entry| *entry == tab) else {
            return;
        };
        if action == "close" {
            if let Some(id) = tab.session() {
                self.close(id, window, cx);
            } else {
                self.close_title_tabs(&[tab], None, window, cx);
            }
            return;
        }
        let closing = tabs
            .into_iter()
            .enumerate()
            .filter(|(ix, entry)| match action {
                "others" => *entry != tab,
                "right" => *ix > index,
                "left" => *ix < index,
                "all" => true,
                _ => false,
            })
            .map(|(_, entry)| entry)
            .collect::<Vec<_>>();
        self.close_title_tabs(&closing, (action == "others").then_some(tab), window, cx);
    }

    pub(super) fn close_title_tabs(
        &mut self,
        tabs: &[TitleTab],
        preferred: Option<TitleTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.confirm_terminal_tab_close(tabs, preferred.clone(), window, cx) {
            return;
        }
        self.close_title_tabs_confirmed(tabs, preferred, window, cx);
    }

    pub(super) fn close_title_tabs_confirmed(
        &mut self,
        tabs: &[TitleTab],
        preferred: Option<TitleTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.visible_tabs(cx);
        let closing: HashSet<_> = tabs
            .iter()
            .filter(|tab| self.workspace.title_order.contains(tab))
            .cloned()
            .collect();
        if closing.is_empty() {
            return;
        }
        let active = self.active_title_tab(cx);
        let replace = active.as_ref().is_some_and(|tab| closing.contains(tab));
        let next = if replace {
            preferred
                .filter(|tab| before.contains(tab) && !closing.contains(tab))
                .or_else(|| {
                    let index = before.iter().position(|tab| Some(tab) == active.as_ref())?;
                    // Browsers select the next tab to the right, then the one to the left.
                    before[index + 1..]
                        .iter()
                        .chain(before[..index].iter().rev())
                        .find(|tab| !closing.contains(*tab))
                        .cloned()
                })
        } else {
            None
        };
        self.navigation_revision += 1;
        self.close_menu(cx);
        self.tab_drag = None;
        self.tab_shifts.clear();
        self.tab_hover = None;
        self.tab_scroll_task = None;
        self.cancel_pane_drag(cx);
        let session_active = self.workspace.active;
        for tab in &closing {
            if let TitleTab::Terminal { id, .. } = tab {
                if self.active_terminal_tab.as_ref() == Some(id) {
                    self.leave_app_views(cx);
                }
                self.terminal_tabs.remove(id);
            }
            if let TitleTab::App(view) = tab
                && self.active_app_view == Some(*view)
            {
                self.leave_app_views(cx);
            }
            if *tab == TitleTab::Search {
                self.leave_app_search(cx);
                self.saved_search = None;
            }
            if let TitleTab::Session(id) = tab {
                self.workspace.close(*id);
            }
        }
        self.workspace
            .title_order
            .retain(|tab| !closing.contains(tab));
        self.workspace
            .scope_focus
            .retain(|_, tab| !closing.contains(tab));
        if replace {
            if let Some(bots) = self.bots.clone() {
                bots.update(cx, |m, cx| m.close(cx));
            }
            self.workspace.active = None;
            self.review_focused = false;
            self.review_slide = None;
            self.pane_motion = None;
            if let Some(tab) = next {
                self.open_title_tab(&tab, window, cx);
            } else {
                self.ensure_project_session(window, cx);
            }
        } else {
            self.workspace.active =
                session_active.filter(|id| self.workspace.tab_of(*id).is_some());
        }
        self.history
            .prune(&self.workspace.open_sessions(), self.workspace.active);
        if self.workspace.active.is_none() && !self.bot_active(cx) && !self.app_view_open() {
            self.focus_active(window, cx);
        }
        self.persist(cx);
        cx.notify();
    }

    pub(super) fn reorder_title_tab(&mut self, tab: &TitleTab, to: usize, cx: &App) {
        let mut visible = self.visible_tabs(cx);
        let Some(from) = visible.iter().position(|entry| entry == tab) else {
            return;
        };
        let entry = visible.remove(from);
        visible.insert(to.min(visible.len()), entry);
        let set: HashSet<_> = visible.iter().cloned().collect();
        let mut ordered = visible.into_iter();
        for entry in &mut self.workspace.title_order {
            if set.contains(entry) {
                *entry = ordered.next().unwrap();
            }
        }
        self.workspace.tabs = self
            .workspace
            .title_order
            .iter()
            .filter_map(TitleTab::session)
            .collect();
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod interaction_tests;
