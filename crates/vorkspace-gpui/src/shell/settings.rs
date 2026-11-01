//! The settings surface replaces the rail body and the workspace, preserving
//! open conversations. Its breadcrumb/actions occupy the inset title bar.
use super::*;
use crate::settings::{ArchivedSession, Section, SettingsContext, SettingsEvent, SettingsView};
use std::path::Path;
impl Shell {
    fn settings_context(&self, cx: &App) -> SettingsContext {
        let mut archived: Vec<_> = self
            .order
            .iter()
            .map(|&id| self.session_metadata(id, cx).summary)
            .filter(|s| s.archived && s.cwd == self.workspace.project)
            .map(|s| ArchivedSession {
                id: s.id,
                title: s.title.clone(),
                provider: s.provider.clone(),
                updated_at: s.updated_at,
            })
            .collect();
        archived.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        let mut catalogs = HashMap::new();
        let active = self
            .bots
            .as_ref()
            .filter(|_| self.bot_active(cx))
            .and_then(|m| m.read(cx).chat())
            .or_else(|| {
                self.workspace.active.and_then(|id| {
                    self.sessions
                        .get(&self.workspace.session_for_pane(id))
                        .cloned()
                })
            });
        let mut profiles: HashMap<_, _> = Kind::ALL
            .into_iter()
            .map(|kind| {
                (
                    kind,
                    crate::accounts::get(cx)
                        .map(|center| center.read(cx).selected(kind, &self.workspace.project).id)
                        .unwrap_or_else(|| vorkspace_core::accounts::DEFAULT.into()),
                )
            })
            .collect();
        if let Some(active) = &active {
            let v = active.read(cx);
            profiles.insert(
                Kind::from_id(&v.summary.provider),
                v.settings.account.id.clone(),
            );
        }
        for session in active.iter().chain(self.sessions.values()) {
            let session = session.read(cx);
            if profiles.get(&Kind::from_id(&session.summary.provider))
                != Some(&session.settings.account.id)
            {
                continue;
            }
            if catalogs.contains_key(&session.summary.provider) {
                continue;
            }
            let models = session.settings_models();
            if !models.is_empty() {
                catalogs.insert(session.summary.provider.clone(), models);
            }
        }
        SettingsContext {
            vox_error: crate::vox::shortcut_error(cx),
            vox_devices: self
                .vox
                .as_ref()
                .map(|v| v.read(cx).devices.clone())
                .unwrap_or_default(),
            remote: Some(self.remote.clone()),
            quick_error: self.quick.error.clone(),
            inbox_connectors: self
                .inbox
                .as_ref()
                .map(|inbox| inbox.read(cx).connector_preferences())
                .unwrap_or_else(|| {
                    locked(&self.store)
                        .map(|store| vorkspace_core::inbox::Connectors::load(&store))
                        .unwrap_or_default()
                }),
            inbox_connections: self
                .inbox
                .as_ref()
                .map(|inbox| inbox.read(cx).connection_statuses())
                .unwrap_or_default(),
            inbox_data_dir: locked(&self.store)
                .ok()
                .and_then(|store| store.data_dir().map(std::path::PathBuf::from)),
            inbox_connection_revision: self
                .inbox
                .as_ref()
                .map(|inbox| inbox.read(cx).connection_revision())
                .unwrap_or_default(),
            jira_filter: self
                .inbox
                .as_ref()
                .map(|inbox| inbox.read(cx).jira_filter())
                .unwrap_or_else(|| {
                    locked(&self.store)
                        .map(|store| vorkspace_core::inbox::JiraFilter::load(&store))
                        .unwrap_or_default()
                }),
            inbox_bots: self
                .inbox
                .as_ref()
                .map(|inbox| inbox.read(cx).default_bots())
                .unwrap_or_else(|| {
                    locked(&self.store)
                        .map(|store| vorkspace_core::inbox::BotDefaults::load(&store))
                        .unwrap_or_default()
                }),
            jira_projects: self
                .inbox
                .as_ref()
                .map(|inbox| inbox.read(cx).jira_projects())
                .unwrap_or_default(),
            hidden_jira_projects: self
                .inbox
                .as_ref()
                .map(|inbox| inbox.read(cx).hidden_jira_projects())
                .unwrap_or_default(),
            sessions: self
                .order
                .iter()
                .map(|&id| {
                    (
                        id,
                        self.session_working_copy(id, cx).to_owned(),
                        self.session_metadata(id, cx).busy,
                    )
                })
                .collect(),
            bots: self
                .bots
                .as_ref()
                .map(|m| {
                    m.read(cx)
                        .roster
                        .bots
                        .iter()
                        .filter(|bot| !bot.is_minion())
                        .map(|m| crate::settings::BotSetting {
                            id: m.id.clone(),
                            name: m.label(),
                            mascot: m.mascot.clone(),
                            color: m.color,
                            projects: m.projects.iter().map(|p| self.project_label(p)).collect(),
                            show: m.show_started_sessions_in_sidebar,
                            folders: m.use_sidebar_folders,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            catalogs,
            cwd: self.workspace.project.clone(),
            project_label: self.project_label(&self.workspace.project),
            archived,
            archived_projects: {
                let mut paths: Vec<_> = self
                    .workspace
                    .project_archive
                    .iter()
                    .filter(|(_, archived)| **archived)
                    .map(|(path, _)| path.clone())
                    .collect();
                paths.sort();
                paths
            },
            projects: self
                .workspace
                .projects
                .iter()
                .map(|path| (path.clone(), self.project_label(path)))
                .collect(),
        }
    }
    pub(super) fn open_settings(
        &mut self,
        section: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_app_search(cx);
        self.close_preview(cx);
        self.menu = None;
        if self.settings.is_some() {
            self.select_settings(section, cx);
            return;
        }
        self.navigation_revision += 1;
        let context = self.settings_context(cx);
        let prefs = self.workspace.preferences.clone();
        let view = cx.new(|cx| SettingsView::new(section, prefs, context, cx));
        self.settings_events = Some(cx.subscribe_in(
            &view,
            window,
            |this, _, event: &SettingsEvent, window, cx| match event {
                SettingsEvent::VoxReset => {
                    if let Some(vox) = &this.vox {
                        vox.update(cx, |v, cx| v.stop(cx));
                    }
                    let result =
                        locked(&this.store).and_then(|db| vorkspace_core::voice::reset(&db));
                    if let Err(error) = result {
                        this.quick.error = Some(error);
                    }
                    this.sync_settings(cx);
                }
                SettingsEvent::VoxMemory => {
                    let result = locked(&this.store).and_then(|db| {
                        let root = vorkspace_core::bot::root(&db);
                        let memory =
                            vorkspace_core::bot::files::read(&root, "vox", None, "MEMORY.md")?;
                        if memory.text.is_none() {
                            vorkspace_core::bot::files::save(
                                &root,
                                "vox",
                                None,
                                "MEMORY.md",
                                "",
                                Some(&memory.hash),
                            )?;
                        }
                        vorkspace_core::bot::files::load(&root, "vox", None)
                    });
                    match result {
                        Ok(files) => {
                            this.close_settings(window, cx);
                            window.dispatch_action(
                                Box::new(crate::file_editor::OpenPath(files.memory_path)),
                                cx,
                            );
                        }
                        Err(error) => {
                            this.quick.error = Some(error);
                            this.sync_settings(cx);
                        }
                    }
                }
                SettingsEvent::Section(section) => this.select_settings(*section, cx),
                SettingsEvent::InboxConnectorEnabled(provider, enabled) => {
                    this.ensure_background_inbox(window, cx);
                    let result = this.inbox.as_ref().unwrap().update(cx, |inbox, cx| {
                        inbox.set_connector_enabled(provider, *enabled, cx)
                    });
                    match result {
                        Err(error) => this.quick.error = Some(error),
                        Ok(()) => {
                            if !enabled && let Some(provider) = Connector::from_id(provider) {
                                let tab = TitleTab::App(app_views::AppView::Connector(provider));
                                this.workspace.title_order.retain(|entry| *entry != tab);
                                this.workspace.scope_focus.retain(|_, entry| *entry != tab);
                                if this.active_app_view
                                    == Some(app_views::AppView::Connector(provider))
                                {
                                    this.leave_app_views(cx);
                                }
                                this.update_chat_visibility(cx);
                            }
                            this.invalidate_chrome(cx);
                            this.persist(cx);
                        }
                    }
                    this.sync_settings(cx);
                    cx.notify();
                }
                SettingsEvent::ManageInboxProvider(provider) => {
                    if let Some(provider) = Connector::from_id(provider) {
                        this.open_connector(provider, window, cx);
                        if let Some(page) = this.connector_pages.get(&provider) {
                            page.update(cx, |page, cx| {
                                page.open_connection(provider.id().into(), cx)
                            });
                        }
                    }
                }
                SettingsEvent::JiraFilter(filter) => {
                    this.ensure_background_inbox(window, cx);
                    if let Some(inbox) = &this.inbox
                        && let Err(error) =
                            inbox.update(cx, |inbox, cx| inbox.set_jira_filter(filter.clone(), cx))
                    {
                        this.quick.error = Some(error);
                    }
                    this.sync_settings(cx);
                }
                SettingsEvent::RefreshInbox => {
                    this.ensure_background_inbox(window, cx);
                    if let Some(inbox) = &this.inbox {
                        inbox.update(cx, |inbox, cx| inbox.refresh(cx));
                    }
                }
                SettingsEvent::JiraProject(id, visible) => {
                    this.ensure_background_inbox(window, cx);
                    if let Some(inbox) = &this.inbox {
                        inbox.update(cx, |inbox, cx| inbox.set_jira_project(id, *visible, cx));
                    }
                    this.sync_settings(cx);
                }
                SettingsEvent::InboxBot(source, bot) => {
                    this.ensure_background_inbox(window, cx);
                    if let Some(inbox) = &this.inbox
                        && let Err(error) =
                            inbox.update(cx, |inbox, cx| inbox.set_default_bot(source, bot, cx))
                    {
                        this.quick.error = Some(error);
                    }
                    this.sync_settings(cx);
                }
                SettingsEvent::WhatsNew => {
                    this.open_whats_new(env!("CARGO_PKG_VERSION").into(), cx)
                }
                SettingsEvent::CheckHarnessUpdates => {
                    let hidden = this.workspace.preferences.hidden_providers.clone();
                    this.notification_state
                        .harness
                        .update(cx, |notice, cx| notice.check_now(hidden, cx));
                }
                SettingsEvent::Changed(prefs) => {
                    window.set_ui_scale(prefs.interface_scale(), cx);
                    this.workspace.preferences = *prefs.clone();
                    this.apply_preferences(cx);
                    this.persist(cx);
                    this.invalidate_chrome(cx);
                    cx.notify();
                }
                SettingsEvent::OpenSession(id) => {
                    this.close_settings(window, cx);
                    this.open(*id, window, cx);
                }
                SettingsEvent::Archive(id, archived) => {
                    if this.session_summaries.contains_key(id)
                        && this.session_metadata(*id, cx).summary.archived != *archived
                    {
                        this.toggle_archive(*id, cx);
                    }
                }
                SettingsEvent::RestoreProject(path) => {
                    let mut next = this.workspace.clone();
                    next.restore_project(path);
                    this.commit_project_archive(next, cx);
                    this.sync_settings(cx);
                }
                SettingsEvent::Delete(id) => this.request_delete_sessions(vec![*id], window, cx),
                SettingsEvent::RemoveWorktree {
                    cwd,
                    path,
                    delete_sessions,
                } => this.remove_settings_worktree(
                    cwd.clone(),
                    path.clone(),
                    *delete_sessions,
                    window,
                    cx,
                ),
                SettingsEvent::ChooseBackground => this.choose_chat_background(cx),
                SettingsEvent::ResetBot(id) => {
                    if let Some(m) = this.bots.clone() {
                        let reset = m.update(cx, |m, cx| m.reset_defaults(id, cx));
                        cx.spawn(async move |this, cx| {
                            let result = reset.await;
                            let _ = this.update(cx, |this, cx| {
                                if let Some(settings) = &this.settings {
                                    settings.update(cx, |s, cx| s.management_finished(result, cx));
                                }
                                this.sync_settings(cx);
                            });
                        })
                        .detach();
                    }
                }
                SettingsEvent::BotSidebar(id, show, folders) => {
                    if let Some(m) = this.bots.clone() {
                        m.update(cx, |m, cx| m.sidebar_preferences(id, *show, *folders, cx));
                    }
                    this.sync_settings(cx);
                }
            },
        ));
        self.settings = Some(view.clone());
        // Settings has its own complete rail, even when the project rail was hidden.
        self.sidebar_slide = None;
        self.sidebar_fraction = 1.;
        window.focus(&view.focus_handle(cx), cx);
        this_update(self, cx);
    }
    pub(super) fn select_settings(&mut self, section: Section, cx: &mut Context<Self>) {
        if let Some(view) = &self.settings {
            view.update(cx, |v, cx| v.select_section(section, cx));
        }
        this_update(self, cx);
    }
    pub(super) fn sync_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = self.settings.clone() {
            let context = self.settings_context(cx);
            let prefs = self.workspace.preferences.clone();
            view.update(cx, |v, cx| {
                v.set_context(context, cx);
                v.set_preferences(&prefs, cx);
            });
        }
    }
    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings = None;
        self.settings_events = None;
        self.sidebar_fraction = if self.workspace.sidebar_visible {
            1.
        } else {
            0.
        };
        self.focus_active(window, cx);
        this_update(self, cx);
    }
    pub(super) fn default_kind(&self) -> Kind {
        let p = &self.workspace.preferences;
        p.default_provider
            .as_deref()
            .and_then(Kind::parse)
            .unwrap_or(Kind::Codex)
    }
    pub(super) fn apply_preferences(&mut self, cx: &mut Context<Self>) {
        self.workspace.preferences.ui_scale = self.workspace.preferences.ui_scale.clamp(
            crate::settings::Preferences::MIN_UI_SCALE_PERCENT,
            crate::settings::Preferences::MAX_UI_SCALE_PERCENT,
        );
        crate::clipboard::configure(self.workspace.preferences.clipboard_history, cx);
        if let Some(vox) = &self.vox {
            let prefs = self.workspace.preferences.clone();
            vox.update(cx, |v, cx| v.preferences(&prefs, cx));
        }
        self.refresh_notification_permission();
        self.configure_quick_composer(cx);
        crate::file_editor::menus(&self.workspace.preferences, cx);
        vorkspace_core::cli::initialize(&self.workspace.preferences.binary_paths);
        self.keep_visible_idle_agents(cx);
        vorkspace_core::provider::configure_idle_agents(
            self.workspace.preferences.idle_agents as usize,
        );
        if let Some(bots) = &self.bots {
            let prefs = self.workspace.preferences.clone();
            bots.update(cx, |m, cx| m.set_preferences(prefs, cx));
        }
        crate::settings::apply_keybindings(&self.workspace.preferences, cx);
        cx.set_global(self.workspace.preferences.clone());
        if let Some(center) = crate::accounts::get(cx) {
            center.update(cx, |center, cx| {
                let changed = center.mask_emails != self.workspace.preferences.mask_emails
                    || center.show_remaining != self.workspace.preferences.show_remaining_usage;
                center.mask_emails = self.workspace.preferences.mask_emails;
                center.show_remaining = self.workspace.preferences.show_remaining_usage;
                if changed {
                    cx.notify();
                }
            });
        }
        for session in self.sessions.values() {
            session.read(cx).transcript.clone().update(cx, |view, cx| {
                view.apply_preferences(&self.workspace.preferences, cx)
            });
        }
        self.usage_footer.update(cx, |footer, cx| {
            footer.set_remaining(self.workspace.preferences.show_remaining_usage, cx)
        });
    }
    pub(crate) fn observe_scale(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.observations
            .push(crate::settings::observe_window_scale(window, cx));
    }
    fn choose_chat_background(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await
                && let Some(path) = paths.first()
            {
                let source = path.clone();
                let directory = crate::data_dir().join("backgrounds");
                let work = cx.background_executor().spawn(async move {
                    vorkspace_core::project_appearance::import_image(
                        &source, &directory, "global", false,
                    )
                });
                let result = work.await;
                let _ = this.update_in(cx, |this, window, cx| match result {
                    Ok(path) => {
                        this.workspace.preferences.chat_background = Some(path);
                        this.persist(cx);
                        this.sync_settings(cx);
                        cx.notify();
                    }
                    Err(error) => {
                        let prompt = window.prompt(
                            gpui::PromptLevel::Critical,
                            "Cannot use this image",
                            Some(&error),
                            &["OK"],
                            cx,
                        );
                        cx.spawn(async move |_, _| {
                            let _ = prompt.await;
                        })
                        .detach();
                    }
                });
            }
        })
        .detach();
    }
    fn finish_worktree_action(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        if let Some(view) = &self.settings {
            view.update(cx, |v, cx| v.management_finished(result, cx));
        }
        self.sync_git(cx);
        self.sync_settings(cx);
        self.invalidate_chrome(cx);
        cx.notify();
    }
    fn remove_settings_worktree(
        &mut self,
        cwd: String,
        path: String,
        delete_sessions: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use vorkspace_core::files::is_equal_or_inside;
        if self
            .terminals
            .values()
            .flatten()
            .any(|t| is_equal_or_inside(&t.read(cx).cwd, &path))
        {
            self.finish_worktree_action(
                Err("Close the terminals using this worktree first.".into()),
                cx,
            );
            return;
        }
        let ids = self
            .session_summaries
            .keys()
            .filter(|id| is_equal_or_inside(self.session_working_copy(**id, cx), &path))
            .copied()
            .collect::<Vec<_>>();
        if ids
            .iter()
            .any(|id| self.sessions.get(id).is_some_and(|v| v.read(cx).busy))
        {
            self.finish_worktree_action(
                Err("Stop the running sessions using this worktree first.".into()),
                cx,
            );
            return;
        }
        let check_cwd = cwd.clone();
        let check_path = path.clone();
        let work = cx.background_executor().spawn(async move {
            vorkspace_core::worktree_management::check(
                Path::new(&check_cwd),
                Path::new(&check_path),
            )
        });
        cx.spawn(async move|this,cx|{
            let result=work.await;
            if let Err(e)=result {let _=this.update(cx,|this,cx|this.finish_worktree_action(Err(e),cx));return;}
            let pending=match this.update_in(cx,|this,window,cx| {
                // Recheck before deleting any conversations after the preflight worker.
                if this.terminals.values().flatten().any(|t|is_equal_or_inside(&t.read(cx).cwd,&path))||ids.iter().any(|id|this.sessions.get(id).is_some_and(|s|s.read(cx).busy)) {return Err("This worktree is now in use. Close its terminals and stop its sessions first.".to_owned());}
                if delete_sessions {for id in &ids{this.delete_session(*id,window,cx);}}
                Ok((this.group_writes.take(),this.store.clone()))
            }) {Ok(Ok(v))=>v,Ok(Err(e))=>{let _=this.update(cx,|this,cx|this.finish_worktree_action(Err(e),cx));return;},Err(_)=>return};
            if let Some(task)=pending.0 {task.await;}
            let worker_cwd=cwd.clone();let worker_path=path.clone();let store=pending.1;
            let work=cx.background_executor().spawn(async move{
                if delete_sessions {let store=locked(&store)?;let paths=store.session_worktrees()?;if store.sessions()?.iter().any(|s|is_equal_or_inside(paths.get(&s.id).and_then(|p|p.as_deref()).unwrap_or(&s.cwd),&worker_path)){return Err("Some sessions could not be deleted, so the worktree was kept.".into());}}
                let snapshot=vorkspace_core::git::snapshot(Path::new(&worker_cwd))?;
                let main=snapshot.worktrees.first().ok_or("No main working copy found")?;
                vorkspace_core::worktree_management::remove(Path::new(&main.path),Path::new(&worker_path))?;
                locked(&store)?.detach_worktree(&worker_path,&main.path)
            });let result=work.await;
            let _=this.update(cx,|this,cx|{let result=result.map(|detached|{for(summary,settings)in detached{this.session_summaries.insert(summary.id, summary.clone());this.session_worktrees.insert(summary.id,settings.worktree_cwd.clone());if let Some(view)=this.sessions.get(&summary.id){view.update(cx,|v,cx|v.worktree_deleted(summary,settings,cx));}}this.worktree_focus.retain(|_,focus|!is_equal_or_inside(&focus.path,&path));});this.finish_worktree_action(result,cx);});
        }).detach();
        let _ = window;
    }
}
fn this_update(this: &mut Shell, cx: &mut Context<Shell>) {
    this.invalidate_chrome(cx);
    this.update_chat_visibility(cx);
    cx.notify();
}

#[cfg(test)]
mod tests;
