//! Application destinations share one navigation and focus boundary.
use super::*;
use crate::{automations, inbox, notes};

pub(super) use crate::workspace::AppView;

impl Shell {
    pub(super) fn app_view_open(&self) -> bool {
        self.active_app_view.is_some() || self.active_terminal_tab.is_some()
    }

    pub(super) fn leave_app_views(&mut self, cx: &mut Context<Self>) {
        if let Some(AppView::Connector(provider)) = self.active_app_view
            && let Some(page) = self.connector_pages.get(&provider)
        {
            page.update(cx, |page, cx| page.set_active(false, cx));
        }
        if self.active_app_view == Some(AppView::Notes)
            && let Some(notes) = self
                .workspace
                .active
                .and_then(|id| self.note_tabs.get(&id))
                .or(self.notes.as_ref())
        {
            notes.update(cx, |notes, cx| {
                notes.flush(cx);
                notes.set_active(false, cx);
            });
        }
        if let Some(id) = self.active_terminal_tab.take()
            && let Some(terminal) = self.terminal_tabs.get(&id)
        {
            terminal.update(cx, |terminal, cx| terminal.set_visible(false, cx));
        }
        self.active_app_view = None;
        self.invalidate_chrome(cx);
    }
    pub(super) fn close_app_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.workspace.active {
            self.close_pane(id, window, cx);
        }
    }

    pub(super) fn prepare_app_view(
        &mut self,
        pane: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(pane) = pane {
            self.restore_app_layout_scope(pane, cx);
        }
        self.selected_sessions.clear();
        self.selection_anchor = None;
        self.sidebar_target = None;
        self.context_selection = false;
        self.leave_app_search(cx);
        self.close_preview(cx);
        self.close_menu(cx);
        if self.settings.is_some() {
            self.close_settings(window, cx);
        }
        self.navigation_revision += 1;
        self.review_focused = false;
        self.pending_composer_focus = None;
    }

    fn show_app_view(&mut self, view: AppView, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_app_views(cx);
        self.active_app_view = Some(view);
        self.admit_app_pane(crate::workspace::AppPane::App(view));
        self.reveal_active_tab(cx);
        self.update_chat_visibility(cx);
        self.sync_right_panel(cx);
        self.invalidate_chrome(cx);
        self.focus_app_view(window, cx);
        self.persist(cx);
        cx.notify();
    }
    pub(super) fn focus_app_view(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(id) = &self.active_terminal_tab {
            if let Some(terminal) = self.terminal_tabs.get(id) {
                window.focus(&terminal.focus_handle(cx), cx);
            }
            return true;
        }
        if let Some(id) = self.workspace.active {
            let focus = self
                .note_tabs
                .get(&id)
                .map(|view| view.focus_handle(cx))
                .or_else(|| self.item_tabs.get(&id).map(|view| view.focus_handle(cx)));
            if let Some(focus) = focus {
                window.focus(&focus, cx);
                return true;
            }
        }
        let focus = match self.active_app_view {
            Some(AppView::Notes) => self.notes.as_ref().map(|view| view.focus_handle(cx)),
            Some(AppView::Inbox) => None,
            Some(AppView::Connector(provider)) => self
                .connector_pages
                .get(&provider)
                .map(|view| view.focus_handle(cx)),
            Some(AppView::Automations) => {
                self.automations.as_ref().map(|view| view.focus_handle(cx))
            }
            Some(AppView::Todos) => self.todos_page.as_ref().map(|view| view.focus_handle(cx)),
            None => return false,
        };
        if let Some(focus) = focus {
            window.focus(&focus, cx);
        }
        true
    }
    pub(super) fn open_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace.preferences.notes {
            self.open_settings(crate::settings::Section::Notes, window, cx);
            return;
        }
        self.prepare_app_view(
            self.app_pane_id(&crate::workspace::AppPane::App(AppView::Notes)),
            window,
            cx,
        );
        if self.notes.is_none() {
            let view = cx.new(|cx| notes::Notes::new(self.store.clone(), cx));
            self.subscribe_notes(&view, window, cx);
            self.notes = Some(view);
            self.sync_note_tab_owners(cx);
        }
        if let Some(notes) = &self.notes {
            let cwd = self.workspace.project.clone();
            let projects = self.workspace.vorkspace_projects();
            notes.update(cx, |notes, cx| {
                notes.set_context(cwd, projects, cx);
                notes.configure_recording(&self.workspace.preferences.hidden_providers, cx);
                notes.set_active(true, cx);
                notes.refresh(cx);
            });
        }
        self.show_app_view(AppView::Notes, window, cx);
    }

    pub(super) fn open_todos(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace.preferences.todos {
            self.open_settings(crate::settings::Section::Todos, window, cx);
            return;
        }
        self.prepare_app_view(
            self.app_pane_id(&crate::workspace::AppPane::App(AppView::Todos)),
            window,
            cx,
        );
        if self.todos_page.is_none() {
            let view = cx.new(|cx| crate::todos::TodosPage::new(self.store.clone(), cx));
            self.app_view_events.push(cx.subscribe_in(
                &view,
                window,
                |this, _, event: &crate::todos::PageEvent, window, cx| match event {
                    crate::todos::PageEvent::Close => this.close_app_resource(
                        crate::workspace::AppPane::App(AppView::Todos),
                        window,
                        cx,
                    ),
                },
            ));
            self.todos_page = Some(view);
        }
        let choices = self.todo_board_choices(cx);
        if let Some(page) = &self.todos_page {
            page.update(cx, |page, cx| page.set_choices(choices, cx));
        }
        self.show_app_view(AppView::Todos, window, cx);
    }

    /// Project boards are shared by people and Bots.
    fn todo_board_choices(&self, _cx: &App) -> Vec<crate::todos::BoardChoice> {
        self.workspace
            .projects
            .iter()
            .map(|path| crate::todos::BoardChoice {
                board: vorkspace_core::todos::TodoBoard::Project(path.clone()),
                label: vorkspace_core::files::leaf_name(path),
            })
            .collect()
    }

    /// The project sidebar's To-dos tab, pointed at the current project.
    pub(super) fn project_todo_board(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.clone();
        let board = self
            .project_todos
            .get_or_insert_with(|| cx.new(|cx| crate::todos::TodoBoardView::new(store, false, cx)))
            .clone();
        let project = vorkspace_core::todos::TodoBoard::Project(self.workspace.project.clone());
        board.update(cx, |board, cx| board.set_board(Some(project), cx));
        div().flex_1().min_h_0().child(board).into_any_element()
    }

    pub(super) fn open_inbox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_background_inbox(window, cx);
        let provider = self
            .inbox
            .as_ref()
            .and_then(|view| Connector::from_id(view.read(cx).saved_source()));
        if let Some(provider) = provider {
            self.open_connector(provider, window, cx);
        } else {
            self.open_settings(crate::settings::Section::Connectors, window, cx);
        }
    }
    pub(super) fn enabled_connectors(&self, cx: &App) -> Vec<Connector> {
        let preferences = self
            .inbox
            .as_ref()
            .map(|view| view.read(cx).connector_preferences())
            .unwrap_or_else(|| {
                locked(&self.store)
                    .map(|store| vorkspace_core::inbox::Connectors::load(&store))
                    .unwrap_or_default()
            });
        Connector::ALL
            .into_iter()
            .filter(|provider| preferences.enabled(provider.id()))
            .collect()
    }
    pub(super) fn connector_unread(&self, provider: Connector, cx: &App) -> bool {
        self.inbox
            .as_ref()
            .is_some_and(|view| view.read(cx).provider_unread_count(provider.id()) > 0)
    }
    pub(super) fn open_connector(
        &mut self,
        provider: Connector,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ensure_background_inbox(window, cx);
        if !self.enabled_connectors(cx).contains(&provider) {
            self.open_settings(crate::settings::Section::Connectors, window, cx);
            return;
        }
        self.prepare_app_view(
            self.app_pane_id(&crate::workspace::AppPane::App(AppView::Connector(
                provider,
            ))),
            window,
            cx,
        );
        if !self.connector_pages.contains_key(&provider) {
            let owner = self.inbox.as_ref().unwrap().clone();
            let view = owner.update(cx, |owner, cx| owner.connector_page(provider.id(), cx));
            self.subscribe_connector(&view, window, cx);
            self.connector_pages.insert(provider, view);
        }
        self.inbox
            .as_ref()
            .unwrap()
            .update(cx, |owner, cx| owner.remember_source(provider.id(), cx));
        self.show_app_view(AppView::Connector(provider), window, cx);
    }
    pub(super) fn subscribe_connector(
        &mut self,
        view: &Entity<inbox::Inbox>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.app_view_events.push(cx.subscribe_in(
            view,
            window,
            |this, origin, event: &inbox::Event, window, cx| {
                if matches!(event, inbox::Event::Close) {
                    let id = this
                        .item_tabs
                        .iter()
                        .find_map(|(&id, view)| (view == origin).then_some(id))
                        .or_else(|| {
                            this.connector_pages.iter().find_map(|(provider, view)| {
                                (view == origin)
                                    .then(|| {
                                        this.app_pane_id(&crate::workspace::AppPane::App(
                                            AppView::Connector(*provider),
                                        ))
                                    })
                                    .flatten()
                            })
                        });
                    if let Some(id) = id {
                        this.close_pane(id, window, cx);
                    }
                } else {
                    this.handle_connector_event(event, window, cx);
                }
            },
        ));
    }
    fn handle_connector_event(
        &mut self,
        event: &inbox::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            inbox::Event::Close => {}
            inbox::Event::ConnectorAvailabilityChanged => self.refresh_pane_visibility(cx),
            inbox::Event::OpenTab(item) => self.open_item_tab(item.as_ref().clone(), window, cx),
            inbox::Event::OpenSession(id) => self.open(*id, window, cx),
            inbox::Event::StartSession { item, prompt, bot } => {
                if let Some(id) = bot {
                    let result = if !self.workspace.preferences.bots {
                        Err("Enable Bots before sending this item".into())
                    } else {
                        self.bots
                            .as_ref()
                            .ok_or_else(|| "Bots unavailable".to_string())
                            .and_then(|bots| {
                                bots.update(cx, |bots, cx| {
                                    bots.send_message(id, prompt.clone(), Vec::new(), cx)
                                })
                            })
                    };
                    match result {
                        Ok(()) => {
                            if let Some(bots) = &self.bots {
                                bots.update(cx, |bots, cx| bots.open(id, window, cx));
                            }
                        }
                        Err(error) => {
                            self.error = Some(error);
                            cx.notify();
                        }
                    }
                } else {
                    self.start_inbox_session(item.as_ref().clone(), prompt.clone(), cx);
                }
            }
        }
    }
    pub(super) fn ensure_background_inbox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inbox.is_none() {
            let projects = self
                .workspace
                .vorkspace_projects()
                .into_iter()
                .map(Into::into)
                .collect();
            let cwd = self.workspace.project.clone().into();
            let poll_projects = self.inbox_poll_projects();
            let (links, feed) = self
                .app_core
                .as_ref()
                .expect("registered workspace window")
                .update(cx, |core, cx| core.inbox_sources(cx));
            let view = cx.new(|cx| {
                inbox::Inbox::new(
                    self.store.clone(),
                    projects,
                    cwd,
                    poll_projects,
                    (&links, &feed),
                    cx,
                )
            });
            self.subscribe_connector(&view, window, cx);
            self.app_view_events
                .push(cx.observe(&view, |this, inbox, cx| {
                    if let Some(automations) = &this.automations {
                        let connections = inbox
                            .read(cx)
                            .connection_statuses()
                            .into_iter()
                            .map(|(provider, (connected, _))| (provider, connected))
                            .collect();
                        automations
                            .update(cx, |view, cx| view.set_trigger_connections(connections, cx));
                    }
                    this.invalidate_chrome(cx);
                    this.sync_settings(cx);
                    cx.notify();
                }));
            self.inbox = Some(view);
        }
        self.sync_inbox_scope(cx);
        if let Some(inbox) = &self.inbox {
            let bots = self
                .bots
                .as_ref()
                .filter(|_| self.workspace.preferences.bots)
                .map(|bots| {
                    bots.read(cx)
                        .roster
                        .bots
                        .iter()
                        .filter(|bot| {
                            !bot.is_minion() && !self.workspace.bot_hidden_in_vorkspace(&bot.id)
                        })
                        .map(|bot| (bot.id.clone(), bot.name.clone()))
                        .collect()
                })
                .unwrap_or_default();
            inbox.update(cx, |inbox, cx| {
                inbox.set_bots(bots, cx);
            });
        }
    }

    fn inbox_poll_projects(&self) -> Vec<std::path::PathBuf> {
        self.workspace
            .projects
            .iter()
            .filter(|p| self.workspace.is_project_available(p))
            .map(Into::into)
            .collect()
    }

    pub(super) fn sync_inbox_scope(&self, cx: &mut Context<Self>) {
        if let Some(inbox) = &self.inbox {
            let visible = self
                .workspace
                .vorkspace_projects()
                .into_iter()
                .map(Into::into)
                .collect();
            inbox.update(cx, |inbox, cx| {
                inbox.set_context(
                    visible,
                    self.workspace.project.clone().into(),
                    self.inbox_poll_projects(),
                    cx,
                );
            });
        }
    }
    pub(super) fn open_automations(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = self.automation_state.service.clone() else {
            self.automation_state.open_requested = Some(self.navigation_revision);
            self.initialize_automations(cx);
            return;
        };
        self.prepare_app_view(
            self.app_pane_id(&crate::workspace::AppPane::App(AppView::Automations)),
            window,
            cx,
        );
        if self.automations.is_none() {
            let cwd = self.workspace.project.clone();
            let projects = self.workspace.vorkspace_projects();
            let view = cx.new(|cx| automations::Automations::new(service, cwd, projects, cx));
            view.update(cx, |view, _| view.set_database(self.store.clone()));
            self.app_view_events.push(cx.subscribe_in(
                &view,
                window,
                |this, _, event: &automations::Event, window, cx| match event {
                    automations::Event::Close => this.close_app_resource(
                        crate::workspace::AppPane::App(AppView::Automations),
                        window,
                        cx,
                    ),
                    automations::Event::Launch(run) => {
                        this.launch_automation(run.as_ref().clone(), true, cx)
                    }
                    automations::Event::OpenSession(id) => {
                        let native = locked(&this.store)
                            .ok()
                            .and_then(|store| store.resolve_existing_session_id(id));
                        if let Some(id) = native {
                            this.open(id, window, cx);
                        }
                    }
                },
            ));
            self.automations = Some(view);
        }
        if let Some(view) = &self.automations {
            let cwd = self.workspace.project.clone();
            let projects = self.workspace.vorkspace_projects();
            view.update(cx, |view, cx| {
                let connections = self
                    .inbox
                    .as_ref()
                    .map(|inbox| inbox.read(cx).connection_statuses())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(provider, (connected, _))| (provider, connected))
                    .collect();
                view.set_trigger_connections(connections, cx);
                view.set_context(cwd, projects, cx);
                view.refresh(cx);
            });
        }
        self.show_app_view(AppView::Automations, window, cx);
    }

    fn app_session_defaults(
        &self,
        kind: Kind,
        cwd: &str,
        cx: &App,
    ) -> vorkspace_core::agent::Settings {
        vorkspace_core::agent::Settings {
            model: self
                .workspace
                .preferences
                .default_models
                .get(kind.id())
                .cloned(),
            effort: crate::conversation::preferred_effort(kind.id(), cx),
            access: self
                .workspace
                .preferences
                .default_access
                .as_deref()
                .map(vorkspace_core::agent::AccessMode::from_id)
                .unwrap_or_default(),
            account: crate::accounts::get(cx)
                .map(|accounts| accounts.read(cx).selected(kind, cwd))
                .unwrap_or_default(),
            ..Default::default()
        }
    }

    pub(super) fn note_to_chat(
        &mut self,
        note: vorkspace_core::notes::Note,
        cx: &mut Context<Self>,
    ) {
        let cwd = note
            .source_cwd
            .clone()
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| self.workspace.project.clone());
        let kind = self.default_kind();
        let store = self.store.clone();
        let settings = self.app_session_defaults(kind, &cwd, cx);
        self.navigation_revision += 1;
        let revision = self.navigation_revision;
        let mut install = self.session_install(cx);
        let work = cx.background_executor().spawn(async move {
            let store = locked(&store)?;
            let mut summary = store.create(kind.id(), &cwd)?;
            install.track(summary.id);
            if !note.title.trim().is_empty() {
                store.title(summary.id, &note.title)?;
                summary.title = note.title.clone();
            }
            store.save_agent_settings(summary.id, &settings)?;
            Ok::<_, String>((summary, settings, note, install))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok((summary, settings, note, mut install)) => {
                    let id = summary.id;
                    let Some(view) = this.install_prepared(summary, &install, cx) else {
                        this.session_install_failed(cx);
                        return;
                    };
                    view.update(cx, |view, cx| {
                        view.seed_new_settings(settings, cx);
                        view.input.update(cx, |input, cx| {
                            input.set_text(format!("@note/{} ", note.slug), cx)
                        });
                    });
                    this.publish_prepared(&view, Some(&mut install), cx);
                    if this.navigation_revision == revision {
                        this.activate_session(id, window, cx);
                    }
                    this.persist(cx);
                }
                Err(error) => {
                    this.error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn start_inbox_session(
        &mut self,
        item: vorkspace_core::inbox::Item,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        let cwd = if item.project_path.is_empty() {
            self.workspace.project.clone()
        } else {
            item.project_path.clone()
        };
        let kind = self.default_kind();
        let settings = self.app_session_defaults(kind, &cwd, cx);
        let store = self.store.clone();
        self.navigation_revision += 1;
        let revision = self.navigation_revision;
        let mut install = self.session_install(cx);
        let work = cx.background_executor().spawn(async move {
            if !std::path::Path::new(&cwd).is_dir() {
                return Err("Choose a project folder before starting work on this item".into());
            }
            let store = locked(&store)?;
            let mut summary = store.create(kind.id(), &cwd)?;
            install.track(summary.id);
            summary.title = format!("{} · {}", item.reference(), item.title);
            summary.work_item = Some(item.work_item());
            store.title(summary.id, &summary.title)?;
            store.set_work_item(summary.id, summary.work_item.as_ref())?;
            store.save_agent_settings(summary.id, &settings)?;
            Ok::<_, String>((summary, settings, prompt, install))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok((summary, settings, prompt, mut install)) => {
                    let id = summary.id;
                    let Some(view) = this.install_prepared(summary, &install, cx) else {
                        this.session_install_failed(cx);
                        return;
                    };
                    view.update(cx, |view, cx| view.quick_submit(prompt, settings, cx));
                    this.publish_prepared(&view, Some(&mut install), cx);
                    if this.navigation_revision == revision {
                        this.activate_session(id, window, cx);
                    }
                    this.persist(cx);
                }
                Err(error) => {
                    this.error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
