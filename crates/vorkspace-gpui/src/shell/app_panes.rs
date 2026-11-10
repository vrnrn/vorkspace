//! App resources use the workspace's existing tab, layout, and focus owner.
//! Entities own drafts and asynchronous work; pane descriptors own identity.
use super::*;
use crate::workspace::{AppPane, AppView};

/// Render after Shell releases its entity lease. Bot controls read workspace
/// state through BotsHost, so rendering them inline from Shell is reentrant.
pub(super) struct BotPaneView {
    shell: gpui::WeakEntity<Shell>,
    manager: Entity<crate::bots::Bots>,
    bot: String,
    width: f32,
    wide: bool,
    _changed: Subscription,
}

impl Render for BotPaneView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let wide = self.width >= crate::bots::ISLAND_BREAKPOINT;
        let resized = wide != self.wide;
        self.wide = wide;
        let linked = self
            .manager
            .read(cx)
            .pane_notification_session(&self.bot, cx)
            .and_then(|session| {
                self.shell
                    .update(cx, |shell, cx| shell.linked_notice(session, cx))
                    .ok()
                    .flatten()
            });
        self.manager.update(cx, |manager, cx| {
            if resized {
                manager.collapse_island(&self.bot);
            }
            manager.render_bot_pane(&self.bot, self.width, linked, window, cx)
        })
    }
}

impl Shell {
    pub(super) fn open_owned_bot(
        &mut self,
        bot: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.bot_hidden_in_vorkspace(bot)
            && let Some(vorkspace) = self
                .workspace
                .vorkspaces
                .iter()
                .find(|vorkspace| vorkspace.bots.iter().any(|id| id == bot))
                .map(|vorkspace| vorkspace.id.clone())
        {
            self.switch_vorkspace(&vorkspace, window, cx);
        }
        if let Some(manager) = self.bots.clone() {
            manager.update(cx, |manager, cx| manager.open(bot, window, cx));
        }
    }

    fn bot_pane_body(
        &mut self,
        pane: i64,
        bot: String,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let manager = self.bots.clone()?;
        let shell = cx.weak_entity();
        let view = self.bot_pane_views.entry(pane).or_insert_with(|| {
            cx.new(|cx| BotPaneView {
                _changed: cx.observe(&manager, |_, _, cx| cx.notify()),
                shell,
                manager,
                bot,
                width: 0.,
                wide: false,
            })
        });
        Some(view.clone().into_any_element())
    }
    /// Surrounding project tools follow the displayed layout, never the focused
    /// leaf. Focusing a note beside a session must not resize either pane.
    pub(super) fn has_session_panes(&self) -> bool {
        self.workspace.active_tab().is_some_and(|tab| {
            self.workspace
                .layout(tab)
                .leaf_ids()
                .into_iter()
                .any(|id| self.workspace.session_for_pane(id).is_some())
        })
    }
    pub(super) fn refresh_bot_document_panes(&mut self, cx: &mut Context<Self>) {
        let Some(manager) = &self.bots else {
            return;
        };
        for view in self.bot_document_tabs.values() {
            let (bot, document) = view.read(cx).reference();
            if let Some(source) = manager.read(cx).document_for_pane(bot, document) {
                view.update(cx, |view, cx| view.set_document(source, cx));
            }
        }
    }
    /// Reopening a saved mixed layout prepares every leaf before displaying it.
    /// Opening each resource may notify navigation, so retain the caller's final
    /// focus and avoid recursively preparing the same tree.
    pub(super) fn prepare_tab_views(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preparing_panes {
            return;
        }
        let Some(tab) = self.workspace.tab_of(id) else {
            return;
        };
        self.preparing_panes = true;
        for leaf in self.workspace.layout(tab).leaf_ids() {
            let ready = match self.workspace.app_panes.get(&leaf).map(|p| &p.content) {
                Some(AppPane::Codebase) => self.codebase_tabs.contains_key(&leaf),
                Some(AppPane::Note(_)) => self.note_tabs.contains_key(&leaf),
                Some(AppPane::ConnectorItem(_)) => self.item_tabs.contains_key(&leaf),
                Some(AppPane::BotDocument { .. }) => self.bot_document_tabs.contains_key(&leaf),
                Some(AppPane::App(AppView::Notes)) => self.notes.is_some(),
                Some(AppPane::App(AppView::Connector(provider))) => {
                    self.connector_pages.contains_key(provider)
                }
                Some(AppPane::App(AppView::Todos)) => self.todos_page.is_some(),
                Some(AppPane::App(AppView::Automations)) => self.automations.is_some(),
                Some(AppPane::Terminal { id, .. }) => self.terminal_tabs.contains_key(id),
                Some(AppPane::Search) => {
                    let cwd = self
                        .app_layout_scope(leaf, cx)
                        .map_or_else(|| self.working_copy(), |(_, worktree)| worktree);
                    self.app_search
                        .as_ref()
                        .or(self.parked_search.as_ref())
                        .is_some_and(|search| search.matches_working_copy(cwd))
                }
                Some(AppPane::Bot(_) | AppPane::App(AppView::Inbox) | AppPane::Review) => true,
                None => {
                    if let Some(owner) = self.workspace.session_for_pane(leaf) {
                        self.ensure_session(owner, cx);
                    }
                    true
                }
            };
            if !ready {
                self.open_app_pane(leaf, window, cx);
            }
        }
        self.preparing_panes = false;
    }
    pub(super) fn update_app_pane_visibility(
        &mut self,
        shown: &HashSet<i64>,
        cx: &mut Context<Self>,
    ) {
        let visible = |content: &AppPane| {
            self.workspace
                .app_panes
                .iter()
                .any(|(id, pane)| shown.contains(id) && pane.content.same_resource(content))
        };
        for (provider, page) in &self.connector_pages {
            let active = visible(&AppPane::App(AppView::Connector(*provider)));
            page.update(cx, |view, cx| view.set_active(active, cx));
        }
        if let Some(view) = &self.notes {
            let active = visible(&AppPane::App(AppView::Notes));
            view.update(cx, |view, cx| view.set_active(active, cx));
        }
        self.sync_search_visibility(visible(&AppPane::Search), cx);
        for (id, view) in &self.note_tabs {
            view.update(cx, |v, cx| v.set_active(shown.contains(id), cx));
        }
        for (id, view) in &self.item_tabs {
            view.update(cx, |v, cx| v.set_active(shown.contains(id), cx));
        }
        for (id, view) in &self.bot_document_tabs {
            view.update(cx, |v, cx| v.set_active(shown.contains(id), cx));
        }
        for (id, view) in &self.terminal_tabs {
            let active = self.workspace.app_panes.iter().any(|(pane, state)| shown.contains(pane) && matches!(state.content, AppPane::Terminal { id: terminal, .. } if terminal == *id));
            view.update(cx, |v, _| v.visible = active);
        }
        for (&id, review) in &self.reviews {
            if self
                .workspace
                .app_panes
                .get(&id)
                .is_some_and(|pane| pane.content == AppPane::Review)
            {
                review.update(cx, |view, cx| view.set_visible(shown.contains(&id), cx));
            }
        }
        if let Some(manager) = self.bots.clone() {
            let context = self.workspace_bot_context();
            let visible: HashSet<String> = shown
                .iter()
                .filter_map(
                    |id| match self.workspace.app_panes.get(id).map(|p| &p.content) {
                        Some(AppPane::Bot(id)) if !self.workspace.bot_hidden_in_vorkspace(id) => {
                            Some(id.clone())
                        }
                        _ => None,
                    },
                )
                .collect();
            manager.update(cx, |manager, cx| {
                manager.set_workspace_context(context.as_deref(), cx);
                manager.set_visible_panes(&visible, cx);
            });
        }
    }

    /// A global resource may be tiled beside a project-bound leaf. Restore that
    /// layout's project and working copy before configuring the resource view.
    pub(super) fn restore_app_layout_scope(&mut self, pane: i64, cx: &mut Context<Self>) {
        let Some((project, worktree)) = self
            .app_layout_scope(pane, cx)
            .map(|(project, worktree)| (project.to_owned(), worktree.to_owned()))
        else {
            return;
        };
        if !self.allow_project_entry(&worktree, cx) || !self.route_project(&project, cx) {
            return;
        }
        self.workspace.project = project.clone();
        if vorkspace_core::files::path_key(self.working_copy())
            != vorkspace_core::files::path_key(&worktree)
        {
            if vorkspace_core::files::path_key(&project)
                == vorkspace_core::files::path_key(&worktree)
            {
                self.worktree_focus.remove(&project);
            } else {
                self.worktree_focus.insert(
                    project,
                    crate::git_tools::worktree_switcher::WorktreeFocus {
                        path: worktree,
                        branch: None,
                    },
                );
            }
        }
    }

    pub(super) fn app_layout_scope<'a>(
        &'a self,
        pane: i64,
        cx: &'a App,
    ) -> Option<(&'a str, &'a str)> {
        let tab = self.workspace.tab_of(pane).unwrap_or(pane);
        self.workspace
            .layout(tab)
            .leaf_ids()
            .into_iter()
            .find_map(|id| {
                let (project, worktree) = self.pane_layout_anchor(id, cx)?;
                let worktree = if self.workspace.session_for_pane(id).is_some() {
                    self.workspace
                        .tab_workspaces
                        .get(&tab)
                        .map_or(worktree, String::as_str)
                } else {
                    worktree
                };
                Some((project, worktree))
            })
    }

    pub(super) fn app_pane_id(&self, content: &AppPane) -> Option<i64> {
        self.workspace.app_panes.iter().find_map(|(&id, pane)| {
            (pane.content.same_resource(content)
                && self.workspace.tab_of(id).is_some()
                && (!content.project_scoped()
                    || (vorkspace_core::files::path_key(&pane.project)
                        == vorkspace_core::files::path_key(&self.workspace.project)
                        && vorkspace_core::files::path_key(&pane.worktree)
                            == vorkspace_core::files::path_key(self.working_copy()))))
            .then_some(id)
        })
    }

    pub(super) fn close_app_resource(
        &mut self,
        content: AppPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.app_pane_id(&content) {
            self.close_pane(id, window, cx);
        }
    }

    pub(super) fn subscribe_notes(
        &mut self,
        view: &Entity<Notes>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.app_view_events.push(cx.subscribe_in(
            view,
            window,
            |this, origin, event: &crate::notes::Event, window, cx| match event {
                crate::notes::Event::Close => {
                    let id = this
                        .note_tabs
                        .iter()
                        .find_map(|(&id, view)| (view == origin).then_some(id))
                        .or_else(|| {
                            (this.notes.as_ref() == Some(origin))
                                .then(|| this.app_pane_id(&AppPane::App(AppView::Notes)))
                                .flatten()
                        });
                    if let Some(id) = id {
                        this.close_pane(id, window, cx);
                    }
                }
                crate::notes::Event::OpenTab(id) => this.open_note_tab(id.clone(), window, cx),
                crate::notes::Event::OpenSource(id) => this.open(*id, window, cx),
                crate::notes::Event::AddToChat(note) => this.note_to_chat(note.clone(), window, cx),
                crate::notes::Event::Notice(message) => {
                    this.error = Some(message.clone());
                    cx.notify();
                }
            },
        ));
        self.app_view_events.push(cx.observe(view, |this, _, cx| {
            this.invalidate_chrome(cx);
            cx.notify();
        }));
    }

    pub(super) fn open_codebase(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !crate::codebase::enabled(&self.workspace.project, cx) {
            return;
        }
        self.prepare_app_view(self.app_pane_id(&AppPane::Codebase), window, cx);
        if let Some(id) = self.admit_app_pane(AppPane::Codebase) {
            self.open_app_pane(id, window, cx);
        }
    }

    pub(super) fn configure_codebase(
        &mut self,
        project: String,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let service = match crate::codebase::service(cx) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        let enabled = !service.enabled(&project);
        let target = project.clone();
        let work = cx
            .background_executor()
            .spawn(async move { service.configure(&target, enabled) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) if enabled && this.workspace.project == project => {
                        this.open_codebase(window, cx)
                    }
                    Ok(()) => (),
                    Err(error) => this.error = Some(error),
                }
                this.invalidate_chrome(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn admit_app_pane(&mut self, content: AppPane) -> Option<i64> {
        let worktree = self.working_copy().to_owned();
        let id = self
            .workspace
            .app_pane(content, self.workspace.project.clone(), worktree)?;
        self.workspace.open(id);
        self.remember_title_tab(TitleTab::Workspace(self.workspace.tab_of(id).unwrap_or(id)));
        Some(id)
    }

    pub(super) fn pane_title(&self, id: i64, cx: &App) -> String {
        let Some(pane) = self.workspace.app_panes.get(&id) else {
            return self
                .workspace
                .session_for_pane(id)
                .filter(|id| self.session_summaries.contains_key(id))
                .map(|id| self.session_metadata(id, cx).display_title())
                .unwrap_or_default();
        };
        match &pane.content {
            AppPane::App(view) => view.label().into(),
            AppPane::Note(_) => self
                .note_tabs
                .get(&id)
                .map(|view| view.read(cx).pane_title(cx))
                .unwrap_or_else(|| "Note".into()),
            AppPane::ConnectorItem(item) => self
                .item_tabs
                .get(&id)
                .map(|view| view.read(cx).pane_title())
                .unwrap_or_else(|| item.title.clone()),
            AppPane::Bot(bot) => self
                .bots
                .as_ref()
                .and_then(|view| view.read(cx).presentation_bot(bot))
                .map(|bot| bot.label())
                .unwrap_or_else(|| "Bot".into()),
            AppPane::BotDocument { .. } => self
                .bot_document_tabs
                .get(&id)
                .map(|view| view.read(cx).title())
                .unwrap_or_else(|| "Document".into()),
            AppPane::Terminal { cwd, .. } => format!("Terminal · {}", project_name(cwd)),
            AppPane::Codebase => "Codebase".into(),
            AppPane::Search => "Search".into(),
            AppPane::Review => self
                .workspace
                .reviews
                .get(&id)
                .and_then(|state| state.tabs.get(state.active))
                .map_or_else(|| "Files".into(), |tab| tab.label()),
        }
    }

    pub(super) fn pane_icon(&self, id: i64) -> &'static str {
        match self.workspace.app_panes.get(&id).map(|pane| &pane.content) {
            Some(AppPane::App(AppView::Connector(provider))) => provider.icon(),
            Some(AppPane::ConnectorItem(item)) => {
                Connector::from_id(&item.provider).map_or("inbox", |p| p.icon())
            }
            Some(AppPane::App(AppView::Notes) | AppPane::Note(_)) => "note",
            Some(AppPane::App(AppView::Automations)) => "zap",
            Some(AppPane::App(AppView::Todos)) => "check-circle",
            Some(AppPane::Bot(_)) => "bot",
            Some(AppPane::BotDocument { .. } | AppPane::Review) => "file",
            Some(AppPane::Terminal { .. }) => "terminal",
            Some(AppPane::Codebase) => "network",
            Some(AppPane::Search) => "search",
            _ => "inbox",
        }
    }

    pub(super) fn pane_in_scope(&self, id: i64, cx: &App) -> bool {
        if let Some(pane) = self.workspace.app_panes.get(&id) {
            return !pane.content.project_scoped()
                || (vorkspace_core::files::path_key(&pane.project)
                    == vorkspace_core::files::path_key(&self.workspace.project)
                    && vorkspace_core::files::path_key(&pane.worktree)
                        == vorkspace_core::files::path_key(self.working_copy()));
        }
        self.workspace.session_for_pane(id).is_some_and(|owner| {
            self.session_summaries.contains_key(&owner)
                && self.session_metadata(owner, cx).summary.cwd == self.workspace.project
                && self.in_worktree_scope(id, cx)
        })
    }

    pub(super) fn app_pane_available(&self, pane: &AppPane, cx: &App) -> bool {
        match pane {
            AppPane::Bot(id) | AppPane::BotDocument { bot: id, .. } => {
                self.workspace.preferences.bots
                    && !self.workspace.bot_hidden_in_vorkspace(id)
                    && self
                        .bots
                        .as_ref()
                        .is_some_and(|m| m.read(cx).roster.get(id).is_some())
            }
            AppPane::App(AppView::Notes) | AppPane::Note(_) => self.workspace.preferences.notes,
            AppPane::App(AppView::Todos) => self.workspace.preferences.todos,
            AppPane::App(AppView::Connector(provider)) => {
                self.enabled_connectors(cx).contains(provider)
            }
            AppPane::ConnectorItem(item) => Connector::from_id(&item.provider)
                .is_some_and(|p| self.enabled_connectors(cx).contains(&p)),
            _ => true,
        }
    }

    pub(super) fn open_app_pane(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.workspace.app_panes.get(&id).cloned() else {
            return;
        };
        let layout_scope = self
            .app_layout_scope(id, cx)
            .map(|(project, worktree)| (project.to_owned(), worktree.to_owned()));
        if !self.allow_project_entry(&pane.project, cx)
            || !self.allow_project_entry(&pane.worktree, cx)
            || layout_scope.is_some_and(|(project, worktree)| {
                !self.allow_project_entry(&project, cx) || !self.allow_project_entry(&worktree, cx)
            })
        {
            return;
        }
        if matches!(&pane.content, AppPane::Bot(bot) | AppPane::BotDocument { bot, .. } if self.workspace.bot_hidden_in_vorkspace(bot))
        {
            return;
        }
        if let AppPane::Bot(bot) = &pane.content
            && let Some(manager) = self.bots.clone()
            && manager.update(cx, |manager, cx| manager.focus_chat_window(bot, cx))
        {
            return;
        }
        if pane.content.project_scoped() && !self.route_project(&pane.project, cx) {
            return;
        }
        if !self.app_pane_available(&pane.content, cx) {
            self.prepare_app_view(Some(id), window, cx);
            self.leave_app_views(cx);
            self.workspace.open(id);
            self.finish_app_pane_open(window, cx);
            return;
        }
        match pane.content {
            AppPane::Codebase => {
                self.prepare_app_view(Some(id), window, cx);
                self.leave_app_views(cx);
                self.workspace.project = pane.project.clone();
                match self.codebase_tabs.entry(id) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let view = cx
                            .new(|cx| crate::codebase::Page::new(pane.project, pane.worktree, cx));
                        entry.insert(view);
                    }
                    std::collections::hash_map::Entry::Occupied(entry) => {
                        entry.get().update(cx, |view, cx| view.activate(false, cx));
                    }
                }
                self.workspace.open(id);
                self.remember_title_tab(TitleTab::Workspace(
                    self.workspace.tab_of(id).unwrap_or(id),
                ));
                self.finish_app_pane_open(window, cx);
            }
            AppPane::App(view) => self.open_title_tab(&TitleTab::App(view), window, cx),
            AppPane::Note(note) => self.open_note_tab(note, window, cx),
            AppPane::ConnectorItem(item) => self.open_item_tab(*item, window, cx),
            AppPane::BotDocument { bot, document } => {
                self.open_bot_document(bot, document, window, cx)
            }
            AppPane::Bot(bot) => {
                self.prepare_app_view(Some(id), window, cx);
                self.leave_app_views(cx);
                self.workspace.project = pane.project;
                self.workspace.open(id);
                self.remember_title_tab(TitleTab::Workspace(
                    self.workspace.tab_of(id).unwrap_or(id),
                ));
                if let Some(manager) = self.bots.clone() {
                    manager.update(cx, |manager, cx| {
                        manager.open_workspace_pane(&bot, window, cx)
                    });
                }
                self.finish_app_pane_open(window, cx);
            }
            AppPane::Terminal { id, cwd } => self.show_terminal_tab(id, cwd, window, cx),
            AppPane::Search => self.open_app_search(window, cx),
            AppPane::Review => self.open_standalone_review(id, window, cx),
        }
    }

    pub(super) fn app_pane_body(
        &mut self,
        id: i64,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let pane = self.workspace.app_panes.get(&id)?.content.clone();
        if !self.app_pane_available(&pane, cx) {
            return Some(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .text_color(content(0.55))
                    .child("This pane is unavailable")
                    .child("Enable its feature in Settings, or close the pane.")
                    .into_any_element(),
            );
        }
        Some(match pane {
            AppPane::Codebase => self.codebase_tabs.get(&id)?.clone().into_any_element(),
            AppPane::Note(_) => self.note_tabs.get(&id)?.clone().into_any_element(),
            AppPane::ConnectorItem(_) => self.item_tabs.get(&id)?.clone().into_any_element(),
            AppPane::BotDocument { .. } => {
                self.bot_document_tabs.get(&id)?.clone().into_any_element()
            }
            AppPane::App(AppView::Notes) => self.notes.clone()?.into_any_element(),
            AppPane::App(AppView::Connector(provider)) => self
                .connector_pages
                .get(&provider)?
                .clone()
                .into_any_element(),
            AppPane::App(AppView::Todos) => self.todos_page.clone()?.into_any_element(),
            AppPane::App(AppView::Automations) => self.automations.clone()?.into_any_element(),
            AppPane::App(AppView::Inbox) | AppPane::Review => return None,
            AppPane::Terminal { id, .. } => div()
                .id(("terminal-tab-body", id))
                .debug_selector(|| "terminal-tab-body".into())
                .size_full()
                .child(self.terminal_tabs.get(&id)?.clone())
                .into_any_element(),
            AppPane::Search => self
                .app_search
                .as_ref()
                .or(self.parked_search.as_ref())?
                .view
                .clone()
                .into_any_element(),
            AppPane::Bot(bot) => self.bot_pane_body(id, bot, cx)?,
        })
    }

    /// Pages make responsive choices from the same allocation that the pane
    /// tree paints, including an in-progress split or sash resize.
    pub(super) fn set_app_pane_width(&mut self, id: i64, width: f32, cx: &mut Context<Self>) {
        let Some(pane) = self.workspace.app_panes.get(&id) else {
            return;
        };
        let content = pane.content.clone();
        match &content {
            AppPane::Codebase => {
                if let Some(view) = self.codebase_tabs.get(&id) {
                    view.update(cx, |view, cx| view.set_width(width, cx));
                }
            }
            AppPane::Bot(bot) => {
                self.bot_pane_body(id, bot.clone(), cx);
                if let Some(view) = self.bot_pane_views.get(&id) {
                    view.update(cx, |view, cx| {
                        if view.width != width {
                            view.width = width;
                            cx.notify();
                        }
                    });
                }
            }
            AppPane::Note(_) => {
                if let Some(view) = self.note_tabs.get(&id) {
                    view.update(cx, |view, cx| view.set_pane_width(width, cx));
                }
            }
            AppPane::ConnectorItem(_) => {
                if let Some(view) = self.item_tabs.get(&id) {
                    view.update(cx, |view, cx| view.set_pane_width(width, cx));
                }
            }
            AppPane::App(AppView::Notes) => {
                if let Some(view) = &self.notes {
                    view.update(cx, |view, cx| view.set_pane_width(width, cx));
                }
            }
            AppPane::App(AppView::Connector(provider)) => {
                if let Some(view) = self.connector_pages.get(provider) {
                    view.update(cx, |view, cx| view.set_pane_width(width, cx));
                }
            }
            AppPane::App(AppView::Automations) => {
                if let Some(view) = &self.automations {
                    view.update(cx, |view, cx| view.set_pane_width(width, cx));
                }
            }
            _ => {}
        }
    }

    pub(super) fn sync_note_tab_owners(&mut self, cx: &mut Context<Self>) {
        let ids = self
            .note_tabs
            .values()
            .filter_map(|v| v.read(cx).pane_note_id())
            .collect();
        if let Some(library) = &self.notes {
            library.update(cx, |view, cx| view.set_open_note_tabs(ids, cx));
        }
    }

    pub(super) fn open_note_tab(
        &mut self,
        note: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace.preferences.notes {
            return;
        }
        let previous = self.workspace.active;
        let previously_open = self.app_pane_id(&AppPane::Note(note.clone())).is_some();
        self.prepare_app_view(self.app_pane_id(&AppPane::Note(note.clone())), window, cx);
        self.leave_app_views(cx);
        let Some(id) = self.admit_app_pane(AppPane::Note(note.clone())) else {
            return;
        };
        if !self.note_tabs.contains_key(&id) {
            let selected = self
                .notes
                .as_ref()
                .is_some_and(|view| view.read(cx).selected_note_id().as_deref() == Some(&note));
            let view = if selected {
                let view = self.notes.take().unwrap();
                if !view.update(cx, |view, cx| view.pin_selected_note(&note, cx)) {
                    self.notes = Some(view);
                    if !previously_open {
                        self.workspace.close(id);
                    }
                    if let Some(previous) = previous {
                        self.open(previous, window, cx);
                    }
                    return;
                }
                view
            } else {
                let view = cx.new(|cx| Notes::open_note_tab(self.store.clone(), note, cx));
                self.subscribe_notes(&view, window, cx);
                view
            };
            self.note_tabs.insert(id, view);
            if selected {
                let library = cx.new(|cx| Notes::new(self.store.clone(), cx));
                self.subscribe_notes(&library, window, cx);
                library.update(cx, |view, cx| {
                    view.set_context(
                        self.workspace.project.clone(),
                        self.workspace.vorkspace_projects(),
                        cx,
                    );
                    view.configure_recording(&self.workspace.preferences.hidden_providers, cx);
                });
                self.notes = Some(library);
            }
            self.sync_note_tab_owners(cx);
        }
        self.active_app_view = Some(AppView::Notes);
        if let Some(view) = self.note_tabs.get(&id) {
            view.update(cx, |view, cx| {
                view.set_appearances(self.workspace.project_appearance.clone(), cx);
                view.set_context(
                    self.workspace.project.clone(),
                    self.workspace.vorkspace_projects(),
                    cx,
                );
                view.configure_recording(&self.workspace.preferences.hidden_providers, cx);
                view.set_active(true, cx);
            });
        }
        self.finish_app_pane_open(window, cx);
    }

    pub(super) fn open_item_tab(
        &mut self,
        item: vorkspace_core::inbox::Item,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(provider) = Connector::from_id(&item.provider) else {
            return;
        };
        self.ensure_background_inbox(window, cx);
        if !self.enabled_connectors(cx).contains(&provider) {
            return;
        }
        self.prepare_app_view(
            self.app_pane_id(&AppPane::ConnectorItem(Box::new(item.clone()))),
            window,
            cx,
        );
        self.leave_app_views(cx);
        let Some(id) = self.admit_app_pane(AppPane::ConnectorItem(Box::new(item.clone()))) else {
            return;
        };
        if !self.item_tabs.contains_key(&id) {
            let owner = self.inbox.as_ref().unwrap().clone();
            let view = owner.update(cx, |owner, cx| {
                owner.connector_item_page(provider.id(), item, cx)
            });
            self.subscribe_connector(&view, window, cx);
            self.item_tabs.insert(id, view);
        }
        self.active_app_view = Some(AppView::Connector(provider));
        self.finish_app_pane_open(window, cx);
    }

    pub(super) fn finish_app_pane_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_active_tab(cx);
        self.update_chat_visibility(cx);
        self.sync_right_panel(cx);
        self.invalidate_chrome(cx);
        self.focus_active(window, cx);
        self.persist(cx);
        cx.notify();
    }

    pub(super) fn prepare_app_pane_close(&mut self, id: i64, cx: &mut Context<Self>) -> bool {
        let note = self.note_tabs.get(&id).cloned().or_else(|| {
            self.workspace
                .app_panes
                .get(&id)
                .filter(|pane| matches!(pane.content, AppPane::App(AppView::Notes)))
                .and(self.notes.clone())
        });
        if let Some(note) = note
            && !note.update(cx, |view, cx| view.prepare_close(cx))
        {
            self.error = Some(
                "The note still has unsaved changes. Check its save status before closing.".into(),
            );
            cx.notify();
            return false;
        }
        true
    }

    pub(super) fn release_app_pane(&mut self, id: i64, cx: &mut Context<Self>) {
        if self
            .workspace
            .app_panes
            .get(&id)
            .is_some_and(|pane| pane.content == AppPane::Review)
            && !self.workspace.reviews.contains_key(&id)
        {
            self.workspace.app_panes.remove(&id);
        }
        self.codebase_tabs.remove(&id);
        self.bot_pane_views.remove(&id);
        if let Some(view) = self.bot_document_tabs.remove(&id) {
            view.update(cx, |v, cx| v.set_active(false, cx));
        }
        if let Some(view) = self.note_tabs.remove(&id) {
            view.update(cx, |view, cx| {
                view.flush(cx);
                view.set_active(false, cx);
            });
            self.sync_note_tab_owners(cx);
        }
        if let Some(view) = self.item_tabs.remove(&id) {
            let item = view.read(cx).pinned_item().cloned();
            if let Some(item) = item
                && let Some(owner) = &self.inbox
            {
                owner.update(cx, |owner, cx| owner.release_item_page(&item, cx));
            }
        }
        if let Some(AppPane::Terminal { id, .. }) =
            self.workspace.app_panes.get(&id).map(|pane| &pane.content)
        {
            self.terminal_tabs.remove(id);
        }
    }

    fn open_bot_document(
        &mut self,
        bot: String,
        document: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prepare_app_view(
            self.app_pane_id(&AppPane::BotDocument {
                bot: bot.clone(),
                document: document.clone(),
            }),
            window,
            cx,
        );
        self.leave_app_views(cx);
        let Some(id) = self.admit_app_pane(AppPane::BotDocument {
            bot: bot.clone(),
            document: document.clone(),
        }) else {
            return;
        };
        if !self.bot_document_tabs.contains_key(&id) {
            let initial = self
                .bots
                .as_ref()
                .and_then(|m| m.read(cx).document_for_pane(&bot, &document));
            let view = cx.new(|cx| {
                crate::bots::BotDocumentPane::new(self.store.clone(), bot, document, initial, cx)
            });
            self.app_view_events.push(cx.observe(&view, |this, _, cx| {
                this.invalidate_chrome(cx);
                cx.notify();
            }));
            self.bot_document_tabs.insert(id, view);
        }
        self.finish_app_pane_open(window, cx);
    }

    pub(super) fn open_bot_workspace_pane(
        &mut self,
        request: crate::bots::OpenWorkspacePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bot = match &request {
            crate::bots::OpenWorkspacePane::Session { bot, .. }
            | crate::bots::OpenWorkspacePane::Document { bot, .. } => bot,
        };
        if self.workspace.bot_hidden_in_vorkspace(bot) {
            return;
        }
        let target = self.app_pane_id(&AppPane::Bot(bot.clone()));
        let opened = match request {
            crate::bots::OpenWorkspacePane::Session { session, .. } => {
                self.open(session, window, cx);
                Some(session)
            }
            crate::bots::OpenWorkspacePane::Document { bot, document } => {
                self.open_bot_document(bot.clone(), document.clone(), window, cx);
                self.app_pane_id(&AppPane::BotDocument { bot, document })
            }
        };
        let Some(id) = opened.filter(|id| self.workspace.active == Some(*id)) else {
            return;
        };
        if let Some(target) =
            target.filter(|target| *target != id && self.workspace.tab_of(*target).is_some())
            && self.workspace.tab_of(id) != self.workspace.tab_of(target)
            && self.placement_allowed(DragSource::Pane(id), target, Edge::Right)
        {
            self.workspace.place(id, target, Edge::Right);
            self.fit_active_pane_width();
            self.open(id, window, cx);
        }
    }
}

#[cfg(test)]
mod tests;
