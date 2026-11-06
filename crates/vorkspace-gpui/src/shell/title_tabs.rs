//! All title-strip commands operate on the same visible, ordered snapshot.
use super::*;
use crate::workspace::TitleTab;
use vorkspace_core::files::path_key;
use vorkspace_ui::radius;

impl Shell {
    /// Bot and terminal tabs share their controls and geometry. The Bot
    /// runtime stays with its manager when this tab is switched or closed.
    pub(super) fn simple_title_tab(
        &self,
        tab: &TitleTab,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        let (identity, label, description, mark) = match tab {
            TitleTab::Terminal { id, cwd } => (
                format!("terminal-title-tab-{id}"),
                format!("Terminal · {}", project_name(cwd)),
                cwd.clone(),
                icon("terminal", 14.).into_any_element(),
            ),
            TitleTab::Bot { id, worktree, .. } => {
                let bot = self.bots.as_ref()?.read(cx).presentation_bot(id)?;
                (
                    format!("bot-title-tab-{id}"),
                    bot.label(),
                    worktree.clone(),
                    crate::avatar::bot_avatar(&bot, "idle", 16., true),
                )
            }
            _ => return None,
        };
        let tab = tab.clone();
        let active = self.active_title_tab(cx).as_ref() == Some(&tab);
        let close_identity = format!("close-{identity}");
        let close_label = format!("Close {label}");
        let close_tab = tab.clone();
        let drag_tab = tab.clone();
        let menu_tab = tab.clone();
        let middle_tab = tab.clone();
        let close = active || width >= TAB_CLOSE_MIN_WIDTH;
        Some(
            div()
                .id(SharedString::from(identity.clone()))
                .debug_selector(move || identity.clone())
                .group("simple-title-tab")
                .role(Role::Tab)
                .aria_label(label.clone())
                .aria_description(description)
                .aria_selected(active)
                .cursor(CursorStyle::Arrow)
                .w(px(width))
                .h(px(TITLE_BAR_HEIGHT))
                .flex_shrink_0()
                .flex()
                .items_center()
                .relative()
                .child(
                    title_tab_plate(active, false)
                        .relative()
                        .text_color(content(if active { 1. } else { 0.5 }))
                        .w_full()
                        .min_w_0()
                        .when(close && width >= TAB_CLOSE_MIN_WIDTH, |d| d.pr_7())
                        .when(width < 30., |d| d.px(px(((width - 14.) / 2.).max(0.))))
                        .gap(px(6.))
                        .group_hover("simple-title-tab", |s| {
                            s.bg(title_tab_fill(active, true)).text_color(content(1.))
                        })
                        .child(mark)
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .text_size(px(TITLE_TAB_TEXT_SIZE))
                                .child(label),
                        ),
                )
                .when(close, |d| {
                    d.child(
                        div()
                            .id(SharedString::from(close_identity.clone()))
                            .debug_selector(move || close_identity.clone())
                            .role(Role::Button)
                            .aria_label(close_label)
                            .absolute()
                            .right_1()
                            .top(px(10.))
                            .size(px(20.))
                            .rounded(px(radius::SMALL))
                            .flex()
                            .items_center()
                            .justify_center()
                            .opacity(0.)
                            .group_hover("simple-title-tab", |s| s.opacity(1.))
                            .hover(|s| s.bg(content(0.1)))
                            .child(icon("close", 12.))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_title_tabs(
                                    std::slice::from_ref(&close_tab),
                                    None,
                                    window,
                                    cx,
                                );
                            })),
                    )
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event, _, cx| {
                        this.start_tab_drag(drag_tab.clone(), event, cx)
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        this.open_menu(
                            MenuTarget::TitleTab(menu_tab.clone()),
                            Anchor::Point(event.position),
                            window,
                            cx,
                        )
                    }),
                )
                .on_mouse_up(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_title_tabs(std::slice::from_ref(&middle_tab), None, window, cx);
                    }),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if std::mem::take(&mut this.skip_tab_click) {
                        return;
                    }
                    this.open_title_tab(&tab, window, cx);
                })),
        )
    }

    /// The workspace keys a tab by one of its panes. Replacing that pane
    /// changes the key, but the same slot and selected width stay on screen.
    pub(super) fn replace_title_tab_key(&mut self, from: i64, to: i64) {
        if from == to {
            return;
        }
        let previous = TitleTab::Workspace(from);
        let next = TitleTab::Workspace(to);
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
                TitleTab::Workspace(tab) => {
                    self.workspace.tabs.contains(tab)
                        && self
                            .workspace
                            .layout(*tab)
                            .leaf_ids()
                            .iter()
                            .all(|id| self.pane_in_scope(*id, cx))
                        && self.workspace.layout(*tab).leaf_ids().iter().any(|id| {
                            self.workspace
                                .app_panes
                                .get(id)
                                .is_none_or(|pane| self.app_pane_available(&pane.content, cx))
                        })
                }
                TitleTab::Bot {
                    id,
                    project,
                    worktree,
                } => {
                    path_key(project) == path_key(&self.workspace.project)
                        && path_key(worktree) == path_key(self.working_copy())
                        && self.workspace.preferences.bots
                        && self
                            .bots
                            .as_ref()
                            .is_some_and(|m| m.read(cx).roster.get(id).is_some())
                }
                TitleTab::Search | TitleTab::App(_) => false,
            })
            .cloned()
            .collect()
    }
    pub(super) fn active_title_tab(&self, _cx: &App) -> Option<TitleTab> {
        self.workspace.active_tab().map(TitleTab::Workspace)
    }

    /// The active tab follows live focus; other tabs keep their last displayed
    /// pane instead of borrowing the currently selected tab's preview focus.
    pub(super) fn tab_review_focused(&self, tab: i64, cx: &App) -> bool {
        if self.active_title_tab(cx) == Some(TitleTab::Workspace(tab)) {
            self.review_focused
        } else {
            self.pane_ticks
                .get(&tab)
                .is_some_and(|tick| tick.shown == (self.workspace.tab_focus(tab), true))
        }
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
        self.admit_app_pane(crate::workspace::AppPane::Bot(id));
        self.update_chat_visibility(cx);
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
            TitleTab::Workspace(id) => {
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
        for tab in &closing {
            if let TitleTab::Workspace(id) = tab {
                for pane in self.workspace.layout(*id).leaf_ids() {
                    if !self.prepare_app_pane_close(pane, cx) {
                        return;
                    }
                }
            }
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
            }
            if let TitleTab::Workspace(id) = tab {
                for pane in self.workspace.layout(*id).leaf_ids() {
                    self.release_app_pane(pane, cx);
                }
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
            self.leave_app_views(cx);
            self.leave_app_search(cx);
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
